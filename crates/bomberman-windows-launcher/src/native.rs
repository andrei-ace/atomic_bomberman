use crate::{cache, ui};
use anyhow::{bail, ensure, Context, Result};
use bomberman_net::windows::{data_directory, wide};
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::{File, OpenOptions},
    os::windows::{ffi::OsStrExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

type Handle = *mut c_void;
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[repr(C)]
#[derive(Default)]
struct StartupInfo {
    size: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    count_x: u32,
    count_y: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved_size: u16,
    reserved_data: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}
#[repr(C)]
#[derive(Default)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}
#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min_working: usize,
    max_working: usize,
    active_processes: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}
#[repr(C)]
#[derive(Default)]
struct Accounting {
    time: [i64; 4],
    faults: u32,
    total: u32,
    active: u32,
    terminated: u32,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn AttachConsole(pid: u32) -> i32;
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        class: i32,
        information: *const c_void,
        size: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn TerminateJobObject(job: Handle, code: u32) -> i32;
    fn QueryInformationJobObject(
        job: Handle,
        class: i32,
        information: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    fn CreateProcessW(
        application: *const u16,
        command: *mut u16,
        process_security: *const c_void,
        thread_security: *const c_void,
        inherit: i32,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        process: *mut ProcessInfo,
    ) -> i32;
    fn ResumeThread(thread: Handle) -> u32;
    fn TerminateProcess(process: Handle, code: u32) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(window: Handle, message: *const u16, title: *const u16, flags: u32) -> i32;
}

struct Owned(Handle);
impl Owned {
    fn new(handle: Handle) -> Result<Self> {
        ensure!(
            !handle.is_null() && handle as isize != -1,
            "Windows handle: {}",
            std::io::Error::last_os_error()
        );
        Ok(Self(handle))
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Job(Owned);
impl Job {
    fn stop(&self) -> Result<()> {
        // Wait for all descendants to release their archive/log handles before
        // the import staging directory is removed by its owner.
        ensure!(
            unsafe { TerminateJobObject(self.0 .0, 1) } != 0,
            "stop importer process group"
        );
        let started = std::time::Instant::now();
        loop {
            let mut accounting = Accounting::default();
            ensure!(
                unsafe {
                    QueryInformationJobObject(
                        self.0 .0,
                        1,
                        (&mut accounting as *mut Accounting).cast(),
                        std::mem::size_of::<Accounting>() as u32,
                        ptr::null_mut(),
                    )
                } != 0,
                "read importer process group"
            );
            if accounting.active == 0 {
                return Ok(());
            }
            ensure!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "importer process group did not stop"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn show_error(message: &str) {
    eprintln!("Atomic Bomberman: {message}");
    if std::env::args_os().skip(1).any(|arg| {
        matches!(
            arg.to_str(),
            Some("--import-source" | "--import-only" | "--assets" | "--headless" | "--help" | "-h")
        )
    }) {
        return;
    }
    let message: Vec<_> = message.encode_utf16().chain([0]).collect();
    let title: Vec<_> = "Atomic Bomberman".encode_utf16().chain([0]).collect();
    unsafe {
        MessageBoxW(ptr::null_mut(), message.as_ptr(), title.as_ptr(), 0x10);
    }
}

#[derive(Default)]
struct Options {
    root: Option<PathBuf>,
    source: Option<PathBuf>,
    import_only: bool,
    repair: bool,
    arguments: Vec<OsString>,
}
impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--import-source" | "--data-directory") => {
                    let value = args.next().context("launcher option needs a path")?;
                    if arg == "--import-source" {
                        options.source = Some(value.into());
                    } else {
                        options.root = Some(value.into());
                    }
                }
                Some("--import-only") => options.import_only = true,
                Some("--repair") => options.repair = true,
                _ => options.arguments.push(arg),
            }
        }
        Ok(options)
    }
}

pub fn run() -> Result<()> {
    // Attach only when invoked from an existing terminal. Explorer launches
    // remain GUI applications and never allocate a console.
    unsafe {
        AttachConsole(u32::MAX);
    }
    let options = Options::parse(std::env::args_os().skip(1))?;
    let executable = std::env::current_exe()?;
    let bin = executable
        .parent()
        .context("locate launcher directory")?
        .join("bin");
    if options
        .arguments
        .iter()
        .any(|arg| arg == "--help" || arg == "-h" || arg == "--assets")
    {
        if options
            .arguments
            .iter()
            .any(|arg| arg == "--help" || arg == "-h")
        {
            println!("Windows launcher: --import-source <path> --import-only --data-directory <path> --repair\n");
        }
        let status = Command::new(bin.join("bomberman.exe"))
            .args(&options.arguments)
            .creation_flags(CREATE_NO_WINDOW)
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
    let root = match options.root {
        Some(root) => root,
        None => data_directory()?,
    };
    cache::prepare(&root)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let assets = if let Some(source) = options.source {
        Some(import(&root, &bin, &source, &cancel)?)
    } else {
        let _lock = cache::lock(&root)?;
        let current = cache::current(&root).ok().flatten();
        let current =
            current.filter(|path| !options.repair && validate(&root, &bin, path, &cancel).is_ok());
        drop(_lock);
        match current {
            Some(path) => Some(path),
            None => ui::choose_data(&root, &bin)?,
        }
    };
    let Some(assets) = assets else {
        return Ok(());
    };
    if options.import_only {
        println!("Imported and validated game data.");
        return Ok(());
    }
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs)?;
    cache::real_directory(&logs)?;
    let log = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(logs.join("game.log"))?;
    let status = Command::new(bin.join("bomberman.exe"))
        .args(options.arguments)
        .arg("--assets")
        .arg(assets)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .context("start bundled game")?;
    if !status.success() {
        bail!(
            "The game exited with {}. See {}.",
            status.code().unwrap_or(1),
            logs.join("game.log").display()
        );
    }
    Ok(())
}

fn log(root: &Path, truncate: bool) -> Result<File> {
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs)?;
    cache::real_directory(&logs)?;
    Ok(OpenOptions::new()
        .write(true)
        .create(true)
        .append(!truncate)
        .truncate(truncate)
        .open(logs.join("import.log"))?)
}

pub fn import(root: &Path, bin: &Path, source: &Path, cancel: &Arc<AtomicBool>) -> Result<PathBuf> {
    let _lock = cache::lock(root)?;
    let stage = cache::Import::new(root)?;
    let args = [
        OsString::from("import"),
        OsString::from("--source"),
        source.as_os_str().into(),
        OsString::from("--output"),
        stage.assets().into_os_string(),
    ];
    helper(
        &bin.join("bomberman-assets.exe"),
        &args,
        log(root, true)?,
        cancel,
    )
    .with_context(|| {
        format!(
            "Import failed. Your previous game data is preserved. See {}.",
            root.join("logs/import.log").display()
        )
    })?;
    validate(root, bin, &stage.assets(), cancel)?;
    cache::require_not_cancelled(cancel.load(Ordering::Acquire))?;
    stage.publish(root)
}

fn validate(root: &Path, bin: &Path, assets: &Path, cancel: &Arc<AtomicBool>) -> Result<()> {
    helper(
        &bin.join("bomberman-assets.exe"),
        &[
            OsString::from("validate"),
            OsString::from("--assets"),
            assets.as_os_str().into(),
        ],
        log(root, false)?,
        cancel,
    )
    .context("game data validation failed; choose another source or see import.log")
}

// Windows command-line quoting: CreateProcess receives UTF-16, and the CRT
// decodes backslash runs specially only before quotes or the closing quote.
fn quote(argument: &OsStr) -> Result<Vec<u16>> {
    let input: Vec<_> = argument.encode_wide().collect();
    ensure!(!input.contains(&0), "argument contains NUL");
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for c in input {
        if c == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if c == b'"' as u16 {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        slashes = 0;
        result.push(c);
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    Ok(result)
}

fn helper(executable: &Path, arguments: &[OsString], log: File, cancel: &AtomicBool) -> Result<()> {
    cache::require_not_cancelled(cancel.load(Ordering::Acquire))?;
    let job = Job(Owned::new(unsafe {
        CreateJobObjectW(ptr::null(), ptr::null())
    })?);
    let limits = ExtendedLimits {
        basic: BasicLimits {
            flags: 0x2000,
            ..Default::default()
        },
        ..Default::default()
    };
    ensure!(
        unsafe {
            SetInformationJobObject(
                job.0 .0,
                9,
                (&limits as *const ExtendedLimits).cast(),
                std::mem::size_of::<ExtendedLimits>() as u32,
            )
        } != 0,
        "configure import process group: {}",
        std::io::Error::last_os_error()
    );
    let null = File::open("NUL")?;
    let handles = [log.as_raw_handle(), null.as_raw_handle()];
    for handle in handles {
        ensure!(
            unsafe { SetHandleInformation(handle, 1, 1) } != 0,
            "inherit import log handle"
        );
    }
    let mut command = quote(executable.as_os_str())?;
    for argument in arguments {
        command.push(b' ' as u16);
        command.extend(quote(argument)?);
    }
    command.push(0);
    let application = wide(executable.as_os_str())?;
    let startup = StartupInfo {
        size: std::mem::size_of::<StartupInfo>() as u32,
        flags: 0x100,
        stdin: null.as_raw_handle(),
        stdout: log.as_raw_handle(),
        stderr: log.as_raw_handle(),
        ..Default::default()
    };
    let mut process = ProcessInfo::default();
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_NO_WINDOW | 4,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut process,
        )
    };
    let error = std::io::Error::last_os_error();
    for handle in handles {
        unsafe {
            SetHandleInformation(handle, 1, 0);
        }
    }
    ensure!(created != 0, "start importer: {error}");
    let process_handle = Owned::new(process.process)?;
    let thread = Owned::new(process.thread)?;
    // The helper starts suspended: its archive extractor cannot escape the
    // job between process creation and assignment.
    if unsafe { AssignProcessToJobObject(job.0 .0, process_handle.0) } == 0 {
        unsafe {
            TerminateProcess(process_handle.0, 1);
            WaitForSingleObject(process_handle.0, 5000);
        }
        bail!(
            "assign importer to cancellation group: {}",
            std::io::Error::last_os_error()
        );
    }
    ensure!(
        unsafe { ResumeThread(thread.0) } != u32::MAX,
        "resume importer"
    );
    drop(thread);
    loop {
        if cancel.load(Ordering::Acquire) {
            job.stop()?;
            cache::require_not_cancelled(true)?;
        }
        match unsafe { WaitForSingleObject(process_handle.0, 50) } {
            0 => break,
            258 => (),
            _ => bail!("wait for importer: {}", std::io::Error::last_os_error()),
        }
    }
    let mut code = 1;
    ensure!(
        unsafe { GetExitCodeProcess(process_handle.0, &mut code) } != 0,
        "read importer result"
    );
    ensure!(code == 0, "importer exited with {code}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_unicode_spaces_quotes_and_trailing_slashes() {
        for (input, expected) in [
            ("", "\"\""),
            ("C:\\game files\\", "\"C:\\game files\\\\\""),
            ("a\"b", "\"a\\\"b\""),
            ("ș data", "\"ș data\""),
        ] {
            assert_eq!(
                String::from_utf16(&quote(OsStr::new(input)).unwrap()).unwrap(),
                expected
            );
        }
    }
    #[test]
    fn parses_launcher_arguments_without_losing_game_arguments() {
        let options = Options::parse(
            [
                "--data-directory",
                "C:\\ș data",
                "--import-only",
                "--windowed",
                "--seed",
                "2",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(options.root, Some(PathBuf::from("C:\\ș data")));
        assert!(options.import_only);
        assert_eq!(
            options.arguments,
            ["--windowed", "--seed", "2"].map(OsString::from)
        );
        assert!(Options::parse([OsString::from("--import-source")]).is_err());
    }
}
