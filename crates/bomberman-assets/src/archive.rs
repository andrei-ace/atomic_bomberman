//! macOS ships libarchive's bsdtar; extracting to stdout avoids trusting paths
//! from an archive or needing an independently installed 7-Zip executable.
use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const MAX_IMAGE: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) struct ArchiveImage {
    directory: PathBuf,
    pub(crate) path: PathBuf,
}

impl Drop for ArchiveImage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn image_member(listing: &[u8]) -> Result<&str> {
    ensure!(
        listing.len() <= 4 * 1024 * 1024,
        "archive file listing is too large"
    );
    let listing = std::str::from_utf8(listing).context("archive filenames must be UTF-8")?;
    let images: Vec<_> = listing
        .lines()
        .filter(|name| {
            Path::new(name).extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("img") || ext.eq_ignore_ascii_case("iso")
            })
        })
        .collect();
    ensure!(
        images.len() == 1,
        "archive must contain exactly one .img or .iso disc image (found {}); select an extracted image instead",
        images.len()
    );
    let member = images[0];
    super::source::check_relative(member)?;
    // bsdtar's member selector is a glob, even when passed as one argument.
    ensure!(
        !member.contains(['*', '?', '[', ']', '\r', '\n']),
        "archive disc filename contains unsupported pattern characters"
    );
    Ok(member)
}

impl ArchiveImage {
    pub(crate) fn extract(archive: &Path, stage: &Path) -> Result<Self> {
        let listing = Command::new("/usr/bin/bsdtar")
            .arg("-tf")
            .arg(archive)
            .output()
            .context("list 7-Zip archive with macOS bsdtar")?;
        ensure!(
            listing.status.success(),
            "cannot read archive: {}",
            String::from_utf8_lossy(&listing.stderr)
        );
        let member = image_member(&listing.stdout)?;
        let directory = stage.join(".archive-source");
        fs::create_dir(&directory)?;
        let image = Self {
            path: directory.join("disc.img"),
            directory,
        };
        let mut output = File::create_new(&image.path)?;
        let diagnostics = image.directory.join("extract.stderr");
        let mut child = Command::new("/usr/bin/bsdtar")
            .arg("-xOf")
            .arg(archive)
            .arg("--")
            .arg(member)
            .stdout(Stdio::piped())
            .stderr(File::create(&diagnostics)?)
            .spawn()
            .context("extract disc image with macOS bsdtar")?;
        let result = (|| -> Result<()> {
            let count = std::io::copy(
                &mut child
                    .stdout
                    .take()
                    .context("archive stdout")?
                    .take(MAX_IMAGE + 1),
                &mut output,
            )?;
            ensure!(count <= MAX_IMAGE, "archive disc image exceeds 2 GiB");
            output.flush()?;
            let status = child.wait()?;
            let mut diagnostic = String::new();
            File::open(&diagnostics)?
                .take(16 * 1024)
                .read_to_string(&mut diagnostic)?;
            ensure!(status.success(), "disc extraction failed: {diagnostic}");
            ensure!(count > 0, "archive disc image is empty");
            Ok(())
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result?;
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_selection_rejects_ambiguous_and_unsafe_images() {
        assert_eq!(
            image_member(b"Game Files/Disc.IMG\nDisc.sub\n").unwrap(),
            "Game Files/Disc.IMG"
        );
        for listing in [
            "readme.txt\n",
            "a.img\nb.iso\n",
            "../a.img\n",
            "/a.img\n",
            "a[1].img\n",
        ] {
            assert!(image_member(listing.as_bytes()).is_err(), "{listing}");
        }
    }

    #[test]
    fn archive_stream_preserves_source_bytes_and_cleans_up() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("bomberman-archive-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let original = b"synthetic disc bytes, not original game data";
        fs::write(root.join("disc with spaces.img"), original).unwrap();
        let archive = root.join("source.7z");
        assert!(Command::new("/usr/bin/bsdtar")
            .args(["-c", "--format=7zip", "-f"])
            .arg(&archive)
            .arg("-C")
            .arg(&root)
            .arg("disc with spaces.img")
            .status()
            .unwrap()
            .success());
        let before = fs::read(&archive).unwrap();
        let extracted = ArchiveImage::extract(&archive, &root).unwrap();
        assert_eq!(fs::read(&extracted.path).unwrap(), original);
        let extracted_path = extracted.path.clone();
        drop(extracted);
        assert!(!extracted_path.exists());
        assert_eq!(fs::read(&archive).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }
}
