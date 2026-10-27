//! Isolate X11's process-fatal GLX context errors until graphics are ready.
//!
//! The parent never initializes graphics/audio. A context failure before the
//! ready marker can safely retry the unchanged command with Mesa software GL.
use crate::cli::Cli;
use anyhow::Result;

pub fn supervise(cli: &Cli) -> Result<Option<i32>> {
    if cli.headless {
        return Ok(None);
    }
    #[cfg(target_os = "linux")]
    return linux::supervise(cli.software_rendering);
    #[cfg(not(target_os = "linux"))]
    {
        if cli.software_rendering {
            anyhow::bail!("--software-rendering is supported on Linux only");
        }
        Ok(None)
    }
}

pub fn ready() {
    #[cfg(target_os = "linux")]
    linux::ready();
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result};
    use std::{
        io::{self, BufRead, Write},
        os::unix::process::CommandExt,
        process::{Command, ExitStatus, Stdio},
    };

    const ATTEMPT: &str = "ATOMIC_BOMBERMAN_GRAPHICS_ATTEMPT";
    const READY: &[u8] = b"\x1eATOMIC_BOMBERMAN_GRAPHICS_READY\x1e\n";
    const MAX_STARTUP_LOG: usize = 64 * 1024;

    pub(super) fn ready() {
        if std::env::var_os(ATTEMPT).is_some() {
            let _ = io::stderr().write_all(READY);
        }
    }

    struct Outcome {
        status: ExitStatus,
        ready: bool,
        startup_log: Vec<u8>,
    }

    fn run(software: bool) -> Result<Outcome> {
        let mut command = Command::new(std::env::current_exe().context("locate game executable")?);
        command
            .args(std::env::args_os().skip(1))
            .env(ATTEMPT, if software { "software" } else { "driver" })
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped());
        if software {
            command
                .env("LIBGL_ALWAYS_SOFTWARE", "1")
                .env("__GLX_VENDOR_LIBRARY_NAME", "mesa");
        }
        let parent = std::process::id() as libc::pid_t;
        // If a launcher/test runner terminates this supervisor, its window
        // must close too. Only async-signal-safe syscalls run after fork.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::from_raw_os_error(libc::ECHILD));
                }
                Ok(())
            });
        }
        let mut child = command.spawn().context("start graphics process")?;
        let mut reader = io::BufReader::new(child.stderr.take().expect("piped graphics stderr"));
        let mut startup_log = Vec::new();
        let mut ready = false;
        let mut line = Vec::new();
        let mut stderr = io::stderr().lock();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if line == READY {
                ready = true;
                let _ = stderr.write_all(&startup_log);
                startup_log.clear();
            } else if ready {
                let _ = stderr.write_all(&line);
            } else {
                // The child can log an arbitrary error; keep bounded startup
                // diagnostics while continuing to drain its pipe.
                let remaining = MAX_STARTUP_LOG.saturating_sub(startup_log.len());
                startup_log.extend_from_slice(&line[..line.len().min(remaining)]);
            }
        }
        Ok(Outcome {
            status: child.wait().context("wait for graphics process")?,
            ready,
            startup_log,
        })
    }

    fn context_failure(log: &[u8]) -> bool {
        let text = String::from_utf8_lossy(log);
        text.contains("(GLX)")
            || text.contains("GLX:")
            || text.contains("GLXError")
            || text.contains("EGL:")
            || (text.contains("native/linux_x11.rs") && text.contains("InitializeFailed"))
    }

    fn retry(outcome: &Outcome) -> bool {
        !outcome.status.success() && !outcome.ready && context_failure(&outcome.startup_log)
    }

    pub(super) fn supervise(force_software: bool) -> Result<Option<i32>> {
        if std::env::var_os(ATTEMPT).is_some() {
            return Ok(None);
        }
        let mut outcome = run(force_software)?;
        if !force_software && retry(&outcome) {
            eprintln!("Atomic Bomberman: graphics driver initialization failed; using software rendering.");
            outcome = run(true)?;
        }
        if !outcome.startup_log.is_empty() {
            let _ = io::stderr().write_all(&outcome.startup_log);
        }
        Ok(Some(outcome.status.code().unwrap_or(1)))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::process::ExitStatusExt;

        fn outcome(code: i32, ready: bool, text: &str) -> Outcome {
            Outcome {
                status: ExitStatus::from_raw(code << 8),
                ready,
                startup_log: text.as_bytes().to_vec(),
            }
        }

        #[test]
        fn retries_reported_glx_context_failure_before_the_game_loads() {
            let error =
                "X Error of failed request: BadValue\nMajor opcode: 152 (GLX)\nMinor opcode: 34";
            assert!(retry(&outcome(1, false, error)));
            assert!(!retry(&outcome(1, true, error)));
            assert!(!retry(&outcome(0, false, error)));
        }

        #[test]
        fn ordinary_errors_do_not_restart_the_game() {
            assert!(!retry(&outcome(
                1,
                false,
                "validate imported original assets: missing file"
            )));
            assert!(!retry(&outcome(1, false, "unknown option --bad")));
            assert!(!retry(&outcome(1, false, "No display server found")));
        }

        #[test]
        fn recognizes_egl_initialization_panic() {
            assert!(retry(&outcome(
                101,
                false,
                "miniquad/src/native/linux_x11.rs:563: Err value: InitializeFailed"
            )));
        }
    }
}
