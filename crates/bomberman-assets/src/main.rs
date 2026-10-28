use anyhow::{bail, Context, Result};
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("Asset error: {error:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_help();
        return Ok(());
    };
    if command == "--help" || command == "-h" {
        print_help();
        return Ok(());
    }
    let mut source = None;
    let mut output = None;
    let mut assets = None;
    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--source" | "--output" | "--assets" => {
                PathBuf::from(args.next().with_context(|| format!("{arg} needs a path"))?)
            }
            _ => bail!("unknown argument {arg}"),
        };
        match arg.as_str() {
            "--source" => source = Some(value),
            "--output" => output = Some(value),
            "--assets" => assets = Some(value),
            _ => unreachable!(),
        }
    }
    let manifest = match command.as_str() {
        "import" => {
            let source = source.context("import needs --source <image-or-directory>")?;
            let output = output.context("import needs --output <cache-directory>")?;
            eprintln!("Importing original graphics, animation, fonts, schemes, and audio. This can take several minutes.");
            let m = bomberman_assets::import(&source, &output)?;
            println!("Imported cache: {}", output.display());
            m
        }
        "validate" => {
            let assets = assets.context("validate needs --assets <cache-directory>")?;
            let m = bomberman_assets::validate(&assets)?;
            println!("Cache validated: {}", assets.display());
            m
        }
        _ => bail!("unknown command {command}; use --help"),
    };
    println!("{} arenas, {} schemes, {} animation files ({} frames), {} images, {} bitmap fonts, {} sounds; {} source SHA-256 records; {} generated files validated.",
        manifest.arenas.len(), manifest.schemes.len(), manifest.animations.len(), manifest.animations.iter().map(|a| a.frames.len()).sum::<usize>(), manifest.textures.len(), manifest.fonts.len(), manifest.sounds.len(), manifest.sources.len(), manifest.generated.len());
    for warning in &manifest.warnings {
        eprintln!("Note: {warning}");
    }
    Ok(())
}
fn print_help() {
    println!("Original Atomic Bomberman asset importer\n\n  bomberman-assets import --source <MODE1-2352.img-or-game-directory> --output <new-cache-directory>\n  bomberman-assets validate --assets <cache-directory>\n\nOriginal source files are opened read-only. Output must be new or empty; distribute the native program separately from original game assets.");
    #[cfg(target_os = "macos")]
    println!("macOS also accepts .7z archives containing exactly one .img or .iso image, using the system bsdtar.");
}
