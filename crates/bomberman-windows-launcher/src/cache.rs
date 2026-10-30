use anyhow::{bail, ensure, Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "data location must be a real directory"
    );
    #[cfg(windows)]
    ensure!(
        !bomberman_net::windows::is_reparse(&metadata),
        "data location must not be a reparse point"
    );
    Ok(())
}

fn ordinary_file(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "unsafe launcher state file"
        );
        #[cfg(windows)]
        ensure!(
            !bomberman_net::windows::is_reparse(&metadata),
            "launcher state must not be a reparse point"
        );
    }
    Ok(())
}

pub fn prepare(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    real_directory(root)?;
    fs::create_dir_all(root.join("caches"))?;
    real_directory(&root.join("caches"))
}

pub fn lock(root: &Path) -> Result<File> {
    let path = root.join(".import.lock");
    ordinary_file(&path)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock()
        .context("another launcher is importing game data")?;
    Ok(file)
}

pub fn current(root: &Path) -> Result<Option<PathBuf>> {
    let marker = root.join("current-cache");
    if !marker.try_exists()? {
        return Ok(None);
    }
    ordinary_file(&marker)?;
    ensure!(fs::metadata(&marker)?.len() <= 128, "invalid cache marker");
    let contents = fs::read_to_string(marker)?;
    let name = contents.trim_end_matches('\n');
    ensure!(
        name.starts_with("cache-")
            && name.len() > 6
            && name
                .bytes()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == b'-'),
        "invalid cache marker"
    );
    let generation = root.join("caches").join(name);
    real_directory(&generation)?;
    let cache = generation.join("assets");
    real_directory(&cache)?;
    Ok(Some(cache))
}

pub struct Import {
    stage: PathBuf,
    generation: PathBuf,
    name: String,
    published: bool,
}
impl Import {
    pub fn new(root: &Path) -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let suffix = format!("{}-{nonce}", std::process::id());
        let stage = root.join(format!(".import-{suffix}"));
        fs::create_dir(&stage)?;
        let name = format!("cache-{suffix}");
        Ok(Self {
            stage,
            generation: root.join("caches").join(&name),
            name,
            published: false,
        })
    }

    pub fn assets(&self) -> PathBuf {
        self.stage.join("assets")
    }

    pub fn publish(mut self, root: &Path) -> Result<PathBuf> {
        real_directory(&self.assets())?;
        ordinary_file(&root.join("current-cache"))?;
        fs::rename(&self.stage, &self.generation).context("publish validated cache directory")?;
        let marker = root.join(format!(".current-{}", self.name));
        let result = (|| -> Result<()> {
            let mut file = File::create_new(&marker)?;
            writeln!(file, "{}", self.name)?;
            file.sync_all()?;
            drop(file);
            #[cfg(windows)]
            bomberman_net::windows::replace_file(&marker, &root.join("current-cache"))?;
            #[cfg(not(windows))]
            fs::rename(&marker, root.join("current-cache"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(marker);
        }
        result?;
        self.published = true;
        Ok(self.generation.join("assets"))
    }
}
impl Drop for Import {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.stage);
            let _ = fs::remove_dir_all(&self.generation);
        }
    }
}

pub fn require_not_cancelled(cancelled: bool) -> Result<()> {
    if cancelled {
        bail!("Import cancelled. Your previous game data is preserved.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "atomic windows cache spaces-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            prepare(&path).unwrap();
            Self(path)
        }
        fn import(&self, bytes: &[u8]) -> Import {
            let import = Import::new(&self.0).unwrap();
            fs::create_dir(import.assets()).unwrap();
            fs::write(import.assets().join("test"), bytes).unwrap();
            import
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn repairs_keep_active_generations_and_failures_keep_the_marker() {
        let temp = Temp::new();
        assert!(current(&temp.0).unwrap().is_none());
        let first = temp.import(b"first").publish(&temp.0).unwrap();
        let marker = fs::read(temp.0.join("current-cache")).unwrap();
        drop(temp.import(b"cancelled"));
        assert_eq!(fs::read(temp.0.join("current-cache")).unwrap(), marker);
        let next = temp.import(b"second").publish(&temp.0).unwrap();
        assert_eq!(current(&temp.0).unwrap(), Some(next.clone()));
        assert_eq!(fs::read(first.join("test")).unwrap(), b"first");
        assert_eq!(fs::read(next.join("test")).unwrap(), b"second");
        assert!(!fs::read_dir(&temp.0).unwrap().any(|p| p
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".import-")));
    }
    #[test]
    fn rejects_marker_traversal_and_concurrent_imports() {
        assert!(require_not_cancelled(false).is_ok());
        assert!(require_not_cancelled(true).is_err());
        let temp = Temp::new();
        for name in [
            "../outside",
            "cache-../outside",
            "cache-C:\\outside",
            "/absolute",
            "cache-a\ncache-b",
        ] {
            fs::write(temp.0.join("current-cache"), name).unwrap();
            assert!(current(&temp.0).is_err());
        }
        let held = lock(&temp.0).unwrap();
        assert!(lock(&temp.0).is_err());
        drop(held);
        assert!(lock(&temp.0).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_a_redirected_cache_and_preserves_its_target() {
        let temp = Temp::new();
        let keep = temp.0.join("keep");
        fs::create_dir(&keep).unwrap();
        fs::write(keep.join("data"), b"unchanged").unwrap();
        std::os::unix::fs::symlink(&keep, temp.0.join("current-cache")).unwrap();
        assert!(temp.import(b"new").publish(&temp.0).is_err());
        assert_eq!(fs::read(keep.join("data")).unwrap(), b"unchanged");
    }
}
