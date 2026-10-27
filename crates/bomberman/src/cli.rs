use anyhow::{bail, Context, Result};
use std::path::PathBuf;

#[derive(Debug)]
pub struct Cli {
    pub assets: PathBuf,
    pub settings: PathBuf,
    pub headless: bool,
    pub matches: usize,
    pub seed: u64,
    pub smoke_frames: Option<u32>,
    pub screenshot: Option<PathBuf>,
    pub smoke_menu: bool,
    pub mute: bool,
    /// None follows the saved preference; explicit flags override it in order.
    pub fullscreen: Option<bool>,
}

pub const HELP: &str = "Atomic Bomberman — native Rust local battle\n\n\
Usage: bomberman --assets <cache-directory> [options]\n\n\
  --settings <file>       Settings JSON (default: XDG configuration directory)\n\
  --seed <number>         Deterministic match seed (default: current time)\n\
  --headless             Run complete computer matches without graphics/audio\n\
  --matches <number>      Number of headless matches (default: 1)\n\
  --smoke-test <frames>   Start a battle, run a fixed number of GUI frames, exit\n\
  --smoke-menu           Keep the main menu during a GUI smoke test\n\
  --screenshot <file>     Save the final 640 x 480 smoke-test frame as PNG\n\
  --mute                 Disable original disc audio\n\
  --windowed             Start in a resizable window, overriding saved fullscreen\n\
  --fullscreen           Start fullscreen (last display-mode flag wins)\n\
  -h, --help             Show this help\n\n\
Import assets first:\n\
  cargo run -p bomberman-assets -- import --source <disc.img> --output <cache>\n\n\
Menu: arrows / Enter / Escape; controller D-pad / south / east.\n\
Battle: keyboard 1 arrows / Space / right Ctrl; keyboard 2 WASD / left Shift / E.\n\
F1 help, Escape pause, F11 fullscreen. Controls are remappable in Settings.\n";

impl Cli {
    pub fn parse() -> Result<Option<Self>> {
        Self::parse_from(std::env::args().skip(1))
    }

    fn parse_from(arguments: impl IntoIterator<Item = String>) -> Result<Option<Self>> {
        let mut assets = None;
        let mut settings = crate::settings::default_path();
        let mut headless = false;
        let mut matches = 1;
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut smoke_frames = None;
        let mut screenshot = None;
        let mut smoke_menu = false;
        let mut mute = false;
        let mut fullscreen = None;
        let mut arguments = arguments.into_iter();
        while let Some(arg) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .with_context(|| format!("{arg} requires a value"))
            };
            match arg.as_str() {
                "--assets" => assets = Some(PathBuf::from(value()?)),
                "--settings" => settings = PathBuf::from(value()?),
                "--headless" => headless = true,
                "--matches" => {
                    matches = value()?
                        .parse()
                        .context("--matches requires a positive integer")?
                }
                "--seed" => {
                    seed = value()?
                        .parse()
                        .context("--seed requires an unsigned integer")?
                }
                "--smoke-test" => {
                    smoke_frames = Some(
                        value()?
                            .parse::<u32>()
                            .context("--smoke-test requires a positive frame count")?,
                    )
                }
                "--screenshot" => screenshot = Some(PathBuf::from(value()?)),
                "--smoke-menu" => smoke_menu = true,
                "--mute" => mute = true,
                "--windowed" => fullscreen = Some(false),
                "--fullscreen" => fullscreen = Some(true),
                "--help" | "-h" => {
                    print!("{HELP}");
                    return Ok(None);
                }
                _ => bail!("unknown option {arg:?}; use --help"),
            }
        }
        let Some(assets) = assets else {
            bail!("--assets <cache-directory> is required; use --help for import instructions");
        };
        if matches == 0 || matches > 100_000 {
            bail!("--matches must be between 1 and 100000");
        }
        if smoke_frames == Some(0) {
            bail!("--smoke-test must specify at least one frame");
        }
        if screenshot.is_some() && smoke_frames.is_none() {
            bail!("--screenshot requires --smoke-test <frames>");
        }
        Ok(Some(Self {
            assets,
            settings,
            headless,
            matches,
            seed,
            smoke_frames,
            screenshot,
            smoke_menu,
            mute,
            fullscreen,
        }))
    }

    pub fn display_mode(&self, saved_fullscreen: bool) -> bool {
        self.fullscreen.unwrap_or(saved_fullscreen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(flags: &[&str]) -> Cli {
        Cli::parse_from(
            ["--assets", "cache"]
                .into_iter()
                .chain(flags.iter().copied())
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn explicit_window_mode_overrides_the_saved_preference() {
        assert!(!parse(&["--windowed"]).display_mode(true));
        assert!(parse(&["--fullscreen"]).display_mode(false));
        assert!(parse(&[]).display_mode(true));
        assert!(!parse(&[]).display_mode(false));
    }

    #[test]
    fn last_display_flag_wins() {
        assert!(!parse(&["--fullscreen", "--windowed"]).display_mode(true));
        assert!(parse(&["--windowed", "--fullscreen"]).display_mode(false));
    }
}
