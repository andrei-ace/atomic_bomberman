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
    let mut workers = None;
    while let Some(arg) = args.next() {
        if arg == "--jobs" {
            let value = args
                .next()
                .context("--jobs needs a positive worker count")?;
            workers = Some(
                value
                    .parse::<usize>()
                    .context("--jobs needs a positive worker count")?,
            );
            anyhow::ensure!(workers != Some(0), "--jobs needs a positive worker count");
            continue;
        }
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
    anyhow::ensure!(
        workers.is_none() || command == "prepare-hd",
        "--jobs is only available with prepare-hd"
    );
    if command == "hd-resources" {
        println!(
            "{}",
            serde_json::to_string_pretty(&bomberman_assets::hd::Resources::detect())?
        );
        return Ok(());
    }
    if command == "import-hd" || command == "export-hd" {
        let assets = assets.context("HD transfer needs --assets <original-cache>")?;
        let source = source.context("HD transfer needs --source <HD-folder>")?;
        let output = output.context("HD transfer needs --output <destination-folder>")?;
        let manifest = bomberman_assets::validate(&assets)?;
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let progress = |p: bomberman_assets::hd::Progress| {
            eprintln!("{} ({}/{})", p.message, p.completed, p.total)
        };
        let path = if command == "import-hd" {
            bomberman_assets::hd::import_pack(
                &assets, &manifest, &output, &source, &cancel, progress,
            )?
        } else {
            bomberman_assets::hd::export_pack(
                &assets, &manifest, &source, &output, &cancel, progress,
            )?
        };
        println!("HD artwork ready: {}", path.display());
        return Ok(());
    }
    if command == "hd-key" {
        let assets = assets.context("hd-key needs --assets <cache-directory>")?;
        println!(
            "{}",
            bomberman_assets::hd::source_key(&bomberman_assets::validate(&assets)?)?
        );
        return Ok(());
    }
    if command == "prepare-hd" {
        let assets = assets.context("prepare-hd needs --assets <cache-directory>")?;
        let output = output.context("prepare-hd needs --output <HD-cache-root>")?;
        let path = bomberman_assets::hd::prepare_with_workers(
            &assets,
            &output,
            &std::sync::atomic::AtomicBool::new(false),
            workers,
            |p| {
                eprintln!("{} ({}/{})", p.message, p.completed, p.total);
            },
        )?;
        println!("HD cache ready: {}", path.display());
        return Ok(());
    }
    if command == "validate-hd" {
        let assets = assets.context("validate-hd needs --assets <original-cache>")?;
        let output = output.context("validate-hd needs --output <HD-pack-directory>")?;
        let original = bomberman_assets::validate(&assets)?;
        let pack = bomberman_assets::hd::Pack::load(&output, &original)?;
        pack.audit_pixels(&output)?;
        println!(
            "Validated HD cache: {} image groups, {} texture pages",
            pack.images.len(),
            pack.pages.len()
        );
        return Ok(());
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
    println!("Portable HD artwork:\n  bomberman-assets import-hd --assets <original-cache> --source <exported-HD-folder> --output <HD-cache-root>\n  bomberman-assets export-hd --assets <original-cache> --source <HD-cache-root> --output <destination-folder>\n");
    println!("Optional local HD artwork:\n  bomberman-assets prepare-hd --assets <original-cache> --output <HD-cache-root> [--jobs <count>]\n  bomberman-assets validate-hd --assets <original-cache> --output <HD-pack-directory>\n  bomberman-assets hd-resources\n\nHD preparation selects resource-bounded parallelism automatically; --jobs overrides calibration within those limits.\n");
    println!("Original Atomic Bomberman asset importer\n\n  bomberman-assets import --source <MODE1-2352.img-or-game-directory> --output <new-cache-directory>\n  bomberman-assets validate --assets <cache-directory>\n\nOriginal source files are opened read-only. Output must be new or empty; distribute the native program separately from original game assets.");
    println!("Also accepts .7z archives containing exactly one .img or .iso image. Linux packages bundle bsdtar; source builds need libarchive-tools. macOS uses system bsdtar; Windows uses system tar.exe.");
}
