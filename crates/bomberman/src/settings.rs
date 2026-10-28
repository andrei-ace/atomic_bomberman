use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_ACTION: &str = if cfg!(target_os = "macos") {
    "RightShift"
} else {
    "RightControl"
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Device {
    Bot,
    Keyboard1,
    Keyboard2,
    Controller(u8),
}

impl Device {
    pub fn name(&self) -> String {
        match self {
            Self::Bot => "Computer".into(),
            Self::Keyboard1 => "Keyboard 1".into(),
            Self::Keyboard2 => "Keyboard 2".into(),
            Self::Controller(n) => format!("Controller {}", n + 1),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Participant {
    pub device: Device,
    pub team: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyLayout {
    /// Up, right, down, left, bomb, action. Names match Macroquad's KeyCode.
    pub keys: [String; 6],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub participant_count: usize,
    pub participants: Vec<Participant>,
    pub arena: usize,
    pub scheme: usize,
    pub wins: u16,
    pub win_by_kills: bool,
    pub round_seconds: u32,
    pub teams: bool,
    pub random_starts: bool,
    pub sudden_death: bool,
    pub enclosement_depth: u8,
    pub gold: bool,
    pub conveyor_speed: u8,
    pub fullscreen: bool,
    pub volume: f32,
    pub keyboard: [KeyLayout; 2],
    /// Standard gamepad buttons; selectable in the Controls screen.
    pub pad_bomb: String,
    pub pad_action: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            participant_count: 10,
            participants: (0..10)
                .map(|i| Participant {
                    device: if i == 0 {
                        Device::Keyboard1
                    } else {
                        Device::Bot
                    },
                    team: u8::from(i >= 5),
                })
                .collect(),
            arena: 0,
            scheme: 0,
            wins: 2,
            win_by_kills: false,
            round_seconds: 150,
            teams: false,
            random_starts: true,
            sudden_death: true,
            enclosement_depth: 1,
            gold: true,
            conveyor_speed: 1,
            fullscreen: false,
            volume: 0.65,
            keyboard: [
                KeyLayout {
                    keys: ["Up", "Right", "Down", "Left", "Space", DEFAULT_ACTION]
                        .map(String::from),
                },
                KeyLayout {
                    keys: ["W", "D", "S", "A", "LeftShift", "E"].map(String::from),
                },
            ],
            pad_bomb: "South".into(),
            pad_action: "East".into(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes =
            std::fs::read(path).with_context(|| format!("read settings {}", path.display()))?;
        let mut settings: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse settings {}", path.display()))?;
        settings.normalize();
        Ok(settings)
    }

    pub fn normalize(&mut self) {
        self.version = 1;
        self.participant_count = self.participant_count.clamp(2, 10);
        self.participants.resize_with(10, || Participant {
            device: Device::Bot,
            team: 0,
        });
        self.participants.truncate(10);
        let mut used = Vec::new();
        for participant in &mut self.participants {
            participant.team %= 5;
            if participant.device != Device::Bot {
                if used.contains(&participant.device) {
                    participant.device = Device::Bot;
                } else {
                    used.push(participant.device.clone());
                }
            }
        }
        self.wins = self.wins.clamp(1, 10);
        self.round_seconds = self.round_seconds.min(900);
        self.conveyor_speed = self.conveyor_speed.min(2);
        self.enclosement_depth = self.enclosement_depth.min(3);
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0., 1.)
        } else {
            0.65
        };
    }

    pub fn assign(&mut self, index: usize, device: Device) {
        if device != Device::Bot {
            for (other, participant) in self.participants.iter_mut().enumerate() {
                if other != index && participant.device == device {
                    participant.device = Device::Bot;
                }
            }
        }
        self.participants[index].device = device;
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create settings directory {}", parent.display()))?;
        }
        let mut temp = path.as_os_str().to_os_string();
        temp.push(format!(".{}.tmp", std::process::id()));
        let temp = PathBuf::from(temp);
        std::fs::write(&temp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write settings {}", temp.display()))?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("save settings {}", path.display()))?;
        Ok(())
    }
}

pub fn default_path() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(base).join("atomic-bomberman/settings.json")
    } else if let Some(base) = std::env::var_os("HOME") {
        let relative = if cfg!(target_os = "macos") {
            "Library/Application Support/Atomic Bomberman/settings.json"
        } else {
            ".config/atomic-bomberman/settings.json"
        };
        PathBuf::from(base).join(relative)
    } else {
        PathBuf::from("bomberman-settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_human_device_belongs_to_only_one_participant() {
        let mut settings = Settings::default();
        settings.assign(4, Device::Keyboard1);
        assert_eq!(settings.participants[0].device, Device::Bot);
        assert_eq!(settings.participants[4].device, Device::Keyboard1);
    }

    #[test]
    fn settings_roundtrip_and_normalize_old_input() {
        let root =
            std::env::temp_dir().join(format!("bomberman-settings-test-{}", std::process::id()));
        let path = root.join("settings.json");
        let mut settings = Settings::default();
        settings.keyboard[0].keys[4] = "Z".into();
        settings.participant_count = 100;
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded.keyboard[0].keys[4], "Z");
        assert_eq!(loaded.participant_count, 10);
        std::fs::remove_dir_all(root).unwrap();
    }
}
