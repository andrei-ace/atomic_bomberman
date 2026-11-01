use anyhow::{Context, Result};
use std::{path::PathBuf, process::Command, sync::mpsc};

pub type FolderChoice = mpsc::Receiver<std::result::Result<Option<PathBuf>, String>>;

pub fn choose_folder() -> Result<FolderChoice> {
    let (sender, receiver) = mpsc::channel();
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("ATOMIC_BOMBERMAN_TEST_MOD_PICKER") {
        let _ = sender.send(Ok((!path.is_empty()).then(|| PathBuf::from(path))));
        return Ok(receiver);
    }
    #[cfg(target_os = "macos")]
    choose_native(sender)?;
    #[cfg(not(target_os = "macos"))]
    std::thread::spawn(move || {
        let _ = sender.send(choose_native().map_err(|error| format!("{error:#}")));
    });
    Ok(receiver)
}

#[cfg(target_os = "macos")]
fn choose_native(sender: mpsc::Sender<std::result::Result<Option<PathBuf>, String>>) -> Result<()> {
    use objc::{class, msg_send, rc::autoreleasepool, runtime::Object, sel, sel_impl};
    use std::ffi::{c_char, CStr};
    autoreleasepool(|| unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let mut window: *mut Object = msg_send![app, mainWindow];
        if window.is_null() {
            window = msg_send![app, keyWindow];
        }
        // Miniquad pumps AppKit itself; a visible game window can exist while
        // NSApplication's main/key-window properties are still unset.
        if window.is_null() {
            let windows: *mut Object = msg_send![app, windows];
            let count: usize = msg_send![windows, count];
            for index in 0..count {
                let candidate: *mut Object = msg_send![windows, objectAtIndex:index];
                let visible: objc::runtime::BOOL = msg_send![candidate, isVisible];
                if visible == objc::runtime::YES {
                    window = candidate;
                    break;
                }
            }
        }
        anyhow::ensure!(
            !window.is_null(),
            "Game window unavailable for mod folder chooser"
        );
        let panel: *mut Object = msg_send![class!(NSOpenPanel), openPanel];
        let _: () = msg_send![panel, setCanChooseFiles: objc::runtime::NO];
        let _: () = msg_send![panel, setCanChooseDirectories: objc::runtime::YES];
        let _: () = msg_send![panel, setAllowsMultipleSelection: objc::runtime::NO];
        let title: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c"Choose an Atomic Bomberman mod folder".as_ptr()];
        let _: () = msg_send![panel, setTitle: title];
        let _: *mut Object = msg_send![panel, retain];
        let completion = block::ConcreteBlock::new(move |response: isize| {
            let folder = if response == 1 {
                let url: *mut Object = msg_send![panel, URL];
                if url.is_null() {
                    None
                } else {
                    let path: *mut Object = msg_send![url, path];
                    let bytes: *const c_char = msg_send![path, UTF8String];
                    (!bytes.is_null())
                        .then(|| CStr::from_ptr(bytes).to_str().ok().map(PathBuf::from))
                        .flatten()
                }
            } else {
                None
            };
            let _ = sender.send(Ok(folder));
            let _: () = msg_send![panel, release];
        })
        .copy();
        // A sheet keeps Miniquad's event pump running. A nested modal loop
        // inside its frame callback can prevent AppKit's panel service setup.
        let _: () =
            msg_send![panel, beginSheetModalForWindow:window completionHandler:&*completion];
        Ok(())
    })
}

#[cfg(target_os = "windows")]
fn choose_native() -> Result<Option<PathBuf>> {
    let shell =
        bomberman_assets::windows_system_directory().join("WindowsPowerShell/v1.0/powershell.exe");
    let output = Command::new(shell).args(["-NoProfile", "-STA", "-Command",
        "Add-Type -AssemblyName System.Windows.Forms; [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false); $p = New-Object System.Windows.Forms.FolderBrowserDialog; $p.Description = 'Choose an Atomic Bomberman mod folder'; if ($p.ShowDialog() -eq 'OK') { [Console]::Write($p.SelectedPath) }"])
        .output().context("open mod folder chooser")?;
    anyhow::ensure!(
        output.status.success(),
        "folder chooser failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text = String::from_utf8(output.stdout).context("folder path is not UTF-8")?;
    Ok((!text.is_empty()).then(|| PathBuf::from(text)))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn choose_native() -> Result<Option<PathBuf>> {
    for (program, args) in [
        (
            "zenity",
            vec![
                "--file-selection",
                "--directory",
                "--title=Choose a mod folder",
            ],
        ),
        (
            "kdialog",
            vec![
                "--getexistingdirectory",
                ".",
                "--title",
                "Choose a mod folder",
            ],
        ),
    ] {
        match Command::new(program).args(args).output() {
            Ok(output) if output.status.success() => {
                let path = String::from_utf8(output.stdout).context("folder path is not UTF-8")?;
                return Ok(Some(PathBuf::from(path.trim_end_matches('\n'))));
            }
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("open mod folder chooser"),
        }
    }
    anyhow::bail!("Folder chooser unavailable. Install zenity/kdialog or use --mod <directory>.")
}

pub fn restart_arguments(arguments: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut args = arguments.into_iter();
    let mut kept = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--mod" {
            args.next();
        } else if arg != "--no-mod" {
            kept.push(arg);
        }
    }
    if !kept.iter().any(|arg| arg == "--skip-intro") {
        kept.push("--skip-intro".into());
    }
    kept
}

pub fn restart() -> Result<()> {
    let executable = std::env::current_exe().context("locate game to restart")?;
    let mut command = Command::new(executable);
    command.args(restart_arguments(std::env::args().skip(1)));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("restart game with selected assets")
    }
    #[cfg(not(unix))]
    {
        command
            .spawn()
            .context("restart game with selected assets")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_uses_saved_mod_without_reapplying_temporary_cli_overrides() {
        let args = [
            "--assets",
            "/base with spaces",
            "--mod",
            "/old mod",
            "--settings",
            "/profile.json",
            "--no-mod",
        ];
        assert_eq!(
            restart_arguments(args.map(str::to_owned)),
            [
                "--assets",
                "/base with spaces",
                "--settings",
                "/profile.json",
                "--skip-intro"
            ]
        );
    }
}
