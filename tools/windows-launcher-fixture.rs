//! Synthetic launcher helpers. No original assets, graphics or network access.
use std::{env, fs, path::PathBuf, process, thread, time::Duration};

fn main() {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    if arguments.first().is_some_and(|arg| arg == "--grandchild") {
        thread::sleep(Duration::from_secs(3));
        fs::write(&arguments[1], "escaped cancellation group").unwrap();
        return;
    }
    if arguments.first().is_some_and(|arg| arg == "--help") {
        println!("synthetic game help: --windowed --assets");
        return;
    }
    if arguments.first().is_some_and(|arg| arg == "validate") {
        process::exit(if PathBuf::from(&arguments[2]).join("valid").is_file() { 0 } else { 2 });
    }
    if arguments.first().is_some_and(|arg| arg == "import") {
        let source = PathBuf::from(&arguments[2]);
        let output = PathBuf::from(&arguments[4]);
        fs::create_dir_all(&output).unwrap();
        let content = fs::read_to_string(&source).unwrap();
        if content == "fail" { process::exit(3); }
        if content == "slow" {
            let parent = source.parent().unwrap();
            let mut child = process::Command::new(env::current_exe().unwrap()).arg("--grandchild").arg(parent.join("escaped")).spawn().unwrap();
            fs::write(parent.join("started"), child.id().to_string()).unwrap();
            let _ = child.wait();
        }
        fs::write(output.join("valid"), content).unwrap();
        return;
    }
    println!("synthetic game arguments: {arguments:?}");
}
