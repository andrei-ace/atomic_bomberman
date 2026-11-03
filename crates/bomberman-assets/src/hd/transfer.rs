//! Portable HD packs contain the manifest and enhanced pages, independently of
//! the enhancement executable, platform and locally installed original data.
use super::{
    audit_page, cache_path, lock_cache, prepare::check_cancel, source_key, Pack, Progress,
};
use crate::Manifest;
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{SystemTime, UNIX_EPOCH},
};

const EXPORT_FOLDER: &str = "Atomic Bomberman HD";

struct Stage(PathBuf);
impl Stage {
    fn new(parent: &Path) -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = parent.join(format!(".hd-transfer-{}-{nonce}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn output_directory(base: &Path, directory: &Path) -> Result<PathBuf> {
    crate::ensure_safe_directory(directory)?;
    let base = fs::canonicalize(base)?;
    let directory = crate::absolute_normalized(directory)?;
    ensure!(
        !directory.starts_with(&base),
        "HD output must be outside the original game asset directory"
    );
    fs::create_dir_all(&directory)?;
    let directory = fs::canonicalize(directory)?;
    ensure!(
        !directory.starts_with(base),
        "HD output must be outside the original game asset directory"
    );
    Ok(directory)
}

fn selected_pack(directory: &Path, manifest: &Manifest) -> Result<PathBuf> {
    crate::ensure_safe_directory(directory)?;
    for path in [
        directory.to_path_buf(),
        directory.join(source_key(manifest)?),
        directory.join(EXPORT_FOLDER),
    ] {
        if path.join("hd.json").is_file() {
            // A directly selected pack with a different identity must produce a
            // mismatch diagnostic, rather than silently selecting other data.
            Pack::load(&path, manifest)?;
            return Ok(fs::canonicalize(path)?);
        }
    }
    bail!("The selected folder has no HD pack for the active game assets or mod. Select the exported HD folder or an existing hd-cache directory.")
}

fn copy_pack(
    source: &Path,
    stage: &Path,
    pack: &Pack,
    manifest: &Manifest,
    cancel: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<()> {
    let total = pack.pages.len();
    for (index, (name, page)) in pack.pages.iter().enumerate() {
        check_cancel(cancel)?;
        let input = source.join(name);
        crate::ensure_safe_directory(&input)?;
        let output = stage.join(name);
        fs::create_dir_all(output.parent().context("HD page directory")?)?;
        let mut reader = fs::File::open(input)?;
        let mut writer = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output)?;
        let mut copied = 0;
        let mut buffer = [0; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            ensure!(copied <= page.bytes, "HD source changed during transfer");
            writer.write_all(&buffer[..n])?;
        }
        ensure!(
            copied == page.bytes,
            "HD source was truncated during transfer"
        );
        writer.flush()?;
        drop(writer);
        audit_page(&output).with_context(|| format!("Invalid HD page: {name}"))?;
        progress(Progress {
            message: "Copying and checking HD artwork".into(),
            completed: index + 1,
            total,
        });
    }
    check_cancel(cancel)?;
    fs::write(stage.join("hd.json"), serde_json::to_vec(pack)?)?;
    progress(Progress::message("Validating transferred HD artwork"));
    Pack::load(stage, manifest)?;
    check_cancel(cancel)?;
    Ok(())
}

/// Copies a matching external pack into the owned local cache. Removing the
/// external drive or selecting Classic cannot remove this completed copy.
pub fn import_pack(
    base: &Path,
    manifest: &Manifest,
    root: &Path,
    selected: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<PathBuf> {
    check_cancel(cancel)?;
    progress(Progress::message("Checking selected HD artwork"));
    let source = selected_pack(selected, manifest)?;
    let pack = Pack::load(&source, manifest)?;
    check_cancel(cancel)?;
    let root = output_directory(base, root)?;
    let _lock = lock_cache(&root)?;
    let destination = cache_path(&root, manifest)?;
    if source == destination || Pack::load(&destination, manifest).is_ok_and(|p| p == pack) {
        for (i, name) in pack.pages.keys().enumerate() {
            check_cancel(cancel)?;
            audit_page(&source.join(name))?;
            progress(Progress {
                message: "Checking cached HD artwork".into(),
                completed: i + 1,
                total: pack.pages.len(),
            });
        }
        check_cancel(cancel)?;
        progress(Progress::message("Matching HD artwork is already cached"));
        return Ok(destination);
    }
    let stage = Stage::new(&root)?;
    copy_pack(&source, &stage.0, &pack, manifest, cancel, &progress)?;
    check_cancel(cancel)?;
    let old = root.join(format!(
        ".old-import-{}",
        stage
            .0
            .file_name()
            .context("HD stage name")?
            .to_string_lossy()
    ));
    if destination.exists() {
        fs::rename(&destination, &old)?;
    }
    if let Err(error) = fs::rename(&stage.0, &destination) {
        if old.exists() {
            let _ = fs::rename(&old, &destination);
        }
        return Err(error.into());
    }
    let _ = fs::remove_dir_all(old);
    progress(Progress::message("Imported HD artwork is ready"));
    Ok(destination)
}

/// Exports the current matching cache into a new child of the chosen folder.
/// Existing unrelated exports are retained, and no inference tools are needed.
pub fn export_pack(
    base: &Path,
    manifest: &Manifest,
    root: &Path,
    selected: &Path,
    cancel: &AtomicBool,
    progress: impl Fn(Progress),
) -> Result<PathBuf> {
    check_cancel(cancel)?;
    let root = output_directory(base, root)?;
    let _lock = lock_cache(&root)?;
    let source = cache_path(&root, manifest)?;
    progress(Progress::message("Checking HD artwork for export"));
    let pack = Pack::load(&source, manifest).context(
        "Prepare or import HD artwork for the active game assets or mod before exporting",
    )?;
    let parent = output_directory(base, selected)?;
    ensure!(
        !parent.starts_with(&source),
        "Choose an export destination outside the current HD pack"
    );
    let destination = parent.join(EXPORT_FOLDER);
    crate::ensure_safe_directory(&destination)?;
    if destination.exists() {
        if Pack::load(&destination, manifest).is_ok() {
            return Ok(destination);
        }
        bail!("An existing 'Atomic Bomberman HD' folder was preserved. Choose a different export destination.");
    }
    let stage = Stage::new(&parent)?;
    copy_pack(&source, &stage.0, &pack, manifest, cancel, &progress)?;
    fs::write(stage.0.join("README.txt"),
        "Atomic Bomberman HD artwork\n\nOn the other computer, install matching original game data and select Options > Graphics > Use existing HD folder. Choose this folder. Graphical mods require matching game artwork.\n\nSwitching to Classic keeps imported HD artwork cached locally.\n\nOriginal artwork: Hudson Soft and Interplay. Enhancement: Real-ESRGAN, Xintao Wang and contributors.\n")?;
    fs::write(
        stage.0.join("THIRD-PARTY-NOTICES.txt"),
        include_str!("../../../../docs/third-party-hd-notices.txt"),
    )?;
    check_cancel(cancel)?;
    fs::rename(&stage.0, &destination)?;
    progress(Progress::message("HD artwork export complete"));
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hd::{file_hash, tests::Fixture};
    use std::sync::atomic::Ordering;

    fn original_directory(f: &Fixture) -> PathBuf {
        let base = f.root.join("originals");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("keep.txt"), b"original data").unwrap();
        base
    }
    fn change_pixels(f: &mut Fixture) {
        image::RgbaImage::from_pixel(8, 8, image::Rgba([70, 80, 90, 255]))
            .save(f.root.join("page.png"))
            .unwrap();
        let (bytes, sha256) = file_hash(&f.root.join("page.png")).unwrap();
        let page = f.pack.pages.get_mut("page.png").unwrap();
        page.bytes = bytes;
        page.sha256 = sha256;
        f.save();
    }
    #[test]
    fn portable_export_import_survives_external_removal_and_reuses_local_files() {
        let f = Fixture::new();
        let base = original_directory(&f);
        let cancel = AtomicBool::new(false);
        let local = f.root.join("cache-a");
        let first = import_pack(&base, &f.manifest, &local, &f.root, &cancel, |_| {}).unwrap();
        let expected = fs::read(first.join("page.png")).unwrap();
        let exported = export_pack(
            &base,
            &f.manifest,
            &local,
            &f.root.join("portable"),
            &cancel,
            |_| {},
        )
        .unwrap();
        assert!(exported.join("README.txt").is_file());
        let other = f.root.join("cache-b");
        let imported = import_pack(
            &base,
            &f.manifest,
            &other,
            exported.parent().unwrap(),
            &cancel,
            |_| {},
        )
        .unwrap();
        let before = fs::metadata(imported.join("page.png"))
            .unwrap()
            .modified()
            .unwrap();
        import_pack(&base, &f.manifest, &other, &exported, &cancel, |_| {}).unwrap();
        assert_eq!(
            fs::metadata(imported.join("page.png"))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
        fs::remove_dir_all(exported).unwrap();
        assert_eq!(Pack::load(&imported, &f.manifest).unwrap(), f.pack);
        assert_eq!(fs::read(imported.join("page.png")).unwrap(), expected);
        assert_eq!(fs::read(base.join("keep.txt")).unwrap(), b"original data");
        assert!(!other.join("upscaler-20220424-macos").exists());
    }
    #[test]
    fn cancelled_replacement_preserves_the_working_local_cache() {
        let mut f = Fixture::new();
        let base = original_directory(&f);
        let root = f.root.join("cache");
        let cancel = AtomicBool::new(false);
        let local = import_pack(&base, &f.manifest, &root, &f.root, &cancel, |_| {}).unwrap();
        let previous = Pack::load(&local, &f.manifest).unwrap();
        change_pixels(&mut f);
        let error = import_pack(&base, &f.manifest, &root, &f.root, &cancel, |p| {
            if p.completed > 0 {
                cancel.store(true, Ordering::Release);
            }
        })
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert_eq!(Pack::load(&local, &f.manifest).unwrap(), previous);
        assert!(!fs::read_dir(&root).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".hd-transfer-")));
    }
    #[test]
    fn incompatible_or_corrupt_external_artwork_cannot_replace_the_local_cache() {
        let mut f = Fixture::new();
        let base = original_directory(&f);
        let root = f.root.join("cache");
        let cancel = AtomicBool::new(false);
        let local = import_pack(&base, &f.manifest, &root, &f.root, &cancel, |_| {}).unwrap();
        let previous = Pack::load(&local, &f.manifest).unwrap();
        f.pack.source = "f".repeat(64);
        f.save();
        assert!(
            import_pack(&base, &f.manifest, &root, &f.root, &cancel, |_| {})
                .unwrap_err()
                .to_string()
                .contains("match")
        );
        f.pack.source = source_key(&f.manifest).unwrap();
        f.save();
        fs::write(f.root.join("page.png"), b"broken").unwrap();
        assert!(import_pack(&base, &f.manifest, &root, &f.root, &cancel, |_| {}).is_err());
        assert_eq!(Pack::load(&local, &f.manifest).unwrap(), previous);
    }
    #[test]
    fn export_preserves_existing_files_and_original_game_data() {
        let f = Fixture::new();
        let base = original_directory(&f);
        let root = f.root.join("cache");
        let cancel = AtomicBool::new(false);
        import_pack(&base, &f.manifest, &root, &f.root, &cancel, |_| {}).unwrap();
        let parent = f.root.join("portable");
        fs::create_dir_all(parent.join(EXPORT_FOLDER)).unwrap();
        fs::write(parent.join(EXPORT_FOLDER).join("keep.txt"), b"user export").unwrap();
        assert!(export_pack(&base, &f.manifest, &root, &parent, &cancel, |_| {}).is_err());
        assert_eq!(
            fs::read(parent.join(EXPORT_FOLDER).join("keep.txt")).unwrap(),
            b"user export"
        );
        assert!(export_pack(&base, &f.manifest, &root, &base, &cancel, |_| {}).is_err());
        assert!(import_pack(
            &base,
            &f.manifest,
            &base.join("hd"),
            &f.root,
            &cancel,
            |_| {}
        )
        .is_err());
        assert!(!base.join("hd").exists());
        assert_eq!(fs::read(base.join("keep.txt")).unwrap(), b"original data");
    }
}
