#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
mod cache;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod ui;

fn main() {
    #[cfg(windows)]
    if let Err(error) = native::run() {
        native::show_error(&format!("{error:#}"));
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    eprintln!("This launcher is for Windows. Use the native Linux or macOS launcher.");
}
