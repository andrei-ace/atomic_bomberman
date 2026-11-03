use super::{
    file_hash,
    prepare::{check_cancel, Cancelled, Progress},
    MODEL,
};
use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Distribution {
    url: String,
    sha256: String,
    files: BTreeMap<String, String>,
}

pub(super) struct Tool {
    binary: PathBuf,
    models: PathBuf,
}

#[derive(Debug)]
pub(super) struct InferenceFailure(anyhow::Error);
impl std::fmt::Display for InferenceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}
impl std::error::Error for InferenceFailure {}

struct Inference(std::process::Child);
impl std::ops::Deref for Inference {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for Inference {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for Inference {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Tool {
    pub(super) fn obtain(
        root: &Path,
        cancel: &AtomicBool,
        progress: &dyn Fn(Progress),
    ) -> Result<Self> {
        ensure!(
            cfg!(target_os = "macos") || cfg!(target_arch = "x86_64"),
            "Automatic HD preparation is unavailable on this CPU architecture"
        );
        let distributions: BTreeMap<String, Distribution> =
            serde_json::from_str(include_str!("tools.json"))?;
        let dist = distributions
            .get(std::env::consts::OS)
            .context("HD preparation is unavailable on this operating system")?;
        let directory = root.join(format!("upscaler-20220424-{}", std::env::consts::OS));
        ensure!(
            directory.as_os_str().as_encoded_bytes().len() < 190,
            "HD tool path is too long; use a shorter settings-directory path"
        );
        crate::ensure_safe_directory(&directory)?;
        fs::create_dir_all(&directory)?;
        let valid = || {
            dist.files.iter().all(|(name, sha)| {
                let path = directory.join(name);
                crate::ensure_safe_directory(&path).is_ok()
                    && file_hash(&path).is_ok_and(|(_, actual)| actual == *sha)
            })
        };
        if !valid() {
            let archive = directory.join("download.zip");
            crate::ensure_safe_directory(&archive)?;
            if !archive.is_file() || file_hash(&archive)?.1 != dist.sha256 {
                progress(Progress::message(
                    "Downloading HD tools and model (about 50 MB)",
                ));
                let result = download(&dist.url, &archive, cancel);
                if result.is_err() {
                    let _ = fs::remove_file(&archive);
                }
                result?;
            }
            ensure!(
                file_hash(&archive)?.1 == dist.sha256,
                "HD tool download failed its SHA-256 check; retry preparation"
            );
            let mut zip = zip::ZipArchive::new(fs::File::open(&archive)?)?;
            // Extract only the pinned executable, model and required DLL. No archive paths are trusted.
            for (name, expected) in &dist.files {
                check_cancel(cancel)?;
                let mut member = zip.by_name(name)?;
                ensure!(
                    member.size() <= 64 * 1024 * 1024,
                    "HD tool file exceeds its size limit"
                );
                let path = directory.join(name);
                crate::ensure_safe_directory(&path)?;
                fs::create_dir_all(path.parent().context("HD tool directory")?)?;
                let temporary = path.with_extension("part");
                crate::ensure_safe_directory(&temporary)?;
                let mut out = fs::File::create(&temporary)?;
                let n = std::io::copy(&mut member.by_ref().take(64 * 1024 * 1024 + 1), &mut out)?;
                ensure!(n <= 64 * 1024 * 1024, "HD tool file is too large");
                out.flush()?;
                drop(out);
                ensure!(
                    file_hash(&temporary)?.1 == *expected,
                    "HD tool file failed its SHA-256 check"
                );
                if path.exists() {
                    fs::remove_file(&path)?;
                }
                fs::rename(temporary, &path)?;
            }
        }
        ensure!(valid(), "HD tools are incomplete");
        let notices = directory.join("THIRD-PARTY-NOTICES.txt");
        crate::ensure_safe_directory(&notices)?;
        fs::write(
            notices,
            include_str!("../../../../docs/third-party-hd-notices.txt"),
        )?;
        let binary = directory.join(if cfg!(windows) {
            "realesrgan-ncnn-vulkan.exe"
        } else {
            "realesrgan-ncnn-vulkan"
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
        }
        Ok(Self {
            binary,
            models: directory.join("models"),
        })
    }

    pub(super) fn run(&self, input: &Path, output: &Path, cancel: &AtomicBool) -> Result<()> {
        self.run_inner(input, output, cancel).map_err(|error| {
            if error.downcast_ref::<Cancelled>().is_some() {
                error
            } else {
                InferenceFailure(error).into()
            }
        })
    }

    fn run_inner(&self, input: &Path, output: &Path, cancel: &AtomicBool) -> Result<()> {
        check_cancel(cancel)?;
        #[cfg(windows)]
        ensure!(
            crate::windows_system_directory()
                .join("vulkan-1.dll")
                .is_file(),
            "HD preparation requires a Vulkan-capable graphics driver"
        );
        let log_path = output.join("inference.log");
        let mut command = Command::new(&self.binary);
        command
            .args(["-i"])
            .arg(input)
            .arg("-o")
            .arg(output)
            .arg("-m")
            .arg(&self.models)
            .args([
                "-n", MODEL, "-s", "4", "-t", "64", "-j", "1:1:1", "-f", "png",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log_path)?);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = Inference(command.spawn().context("start HD upscaler")?);
        let started = Instant::now();
        loop {
            if let Err(error) = check_cancel(cancel) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    let mut log = String::new();
                    fs::File::open(&log_path)?
                        .take(4096)
                        .read_to_string(&mut log)?;
                    bail!(
                        "HD upscaler could not use the GPU ({status}). {}",
                        log.lines().take(8).collect::<Vec<_>>().join(" ")
                    );
                }
                return Ok(());
            }
            if started.elapsed() > Duration::from_secs(30 * 60) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("HD upscaler timed out; Classic graphics are still available");
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn download(url: &str, path: &Path, cancel: &AtomicBool) -> Result<()> {
    check_cancel(cancel)?;
    let agent = ureq::AgentBuilder::new()
        .https_only(true)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(15))
        .timeout_write(Duration::from_secs(15))
        .build();
    let mut reader = agent
        .get(url)
        .call()
        .context("download HD tools; an Internet connection is needed on first use")?
        .into_reader();
    let mut out = fs::File::create(path)?;
    let mut buffer = [0; 64 * 1024];
    let mut count = 0;
    loop {
        check_cancel(cancel)?;
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        count += n;
        ensure!(
            count <= 96 * 1024 * 1024,
            "HD download exceeds its size limit"
        );
        out.write_all(&buffer[..n])?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{atomic::Ordering, Arc};

    #[test]
    #[ignore = "requires the three pinned official upscaler archives"]
    fn official_archives_parse_and_match_every_pinned_member() -> Result<()> {
        use sha2::{Digest, Sha256};
        let root = PathBuf::from(
            std::env::var_os("ATOMIC_BOMBERMAN_HD_ARCHIVES")
                .context("Set ATOMIC_BOMBERMAN_HD_ARCHIVES")?,
        );
        let distributions: BTreeMap<String, Distribution> =
            serde_json::from_str(include_str!("tools.json"))?;
        for (os, dist) in distributions {
            let filename = if os == "linux" {
                "upstream-linux.zip"
            } else if os == "windows" {
                "upstream-windows.zip"
            } else {
                "upstream-macos.zip"
            };
            let path = root.join(filename);
            ensure!(
                file_hash(&path)?.1 == dist.sha256,
                "Official archive checksum differs"
            );
            let mut archive = zip::ZipArchive::new(fs::File::open(path)?)?;
            for (name, expected) in &dist.files {
                let mut member = archive.by_name(name)?;
                ensure!(member.size() <= 64 * 1024 * 1024, "Oversized pinned member");
                let mut data = Vec::new();
                member
                    .by_ref()
                    .take(64 * 1024 * 1024 + 1)
                    .read_to_end(&mut data)?;
                ensure!(
                    format!("{:x}", Sha256::digest(&data)) == *expected,
                    "Pinned member changed: {name}"
                );
            }
            eprintln!(
                "Verified {os} official archive and {} pinned files",
                dist.files.len()
            );
        }
        Ok(())
    }

    #[test]
    fn failed_or_cancelled_helpers_are_reported_and_reaped() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("atomic-hd-helper-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let script = root.join("helper");
        fs::write(&script, "#!/bin/sh\nexit 42\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let tool = Tool {
            binary: script.clone(),
            models: root.clone(),
        };
        assert!(tool
            .run(&root, &root, &AtomicBool::new(false))
            .unwrap_err()
            .to_string()
            .contains("GPU"));
        fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$4/pid\"\nexec sleep 20\n",
        )
        .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::Release);
        });
        let start = Instant::now();
        assert!(tool
            .run(&root, &root, &cancelled)
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        setter.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        let pid = fs::read_to_string(root.join("pid")).unwrap();
        assert!(!Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelling_parallel_inference_reaps_every_child() {
        let root = std::env::temp_dir().join(format!("atomic-hd-pool-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let script = root.join("helper");
        fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$4/pid\"\nexec sleep 20\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let tool = Tool {
            binary: script,
            models: root.clone(),
        };
        for i in 0..3 {
            fs::create_dir(root.join(i.to_string())).unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let result = std::thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(3);
                while !(0..3).all(|i| root.join(i.to_string()).join("pid").is_file())
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(10));
                }
                cancelled.store(true, Ordering::Release);
            });
            super::super::scheduler::run(
                3,
                3,
                &cancelled,
                |i, stop| tool.run(&root, &root.join(i.to_string()), stop),
                |_, ()| Ok(()),
            )
        });
        assert!(result.unwrap_err().downcast_ref::<Cancelled>().is_some());
        for i in 0..3 {
            let pid = fs::read_to_string(root.join(i.to_string()).join("pid")).unwrap();
            assert!(!Command::new("/bin/kill")
                .args(["-0", &pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
