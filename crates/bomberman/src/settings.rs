use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Participant {
    pub device: Device,
    pub team: u8,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

/// The original bitmap fonts cover the printable ASCII nickname alphabet.
pub fn normalize_nickname(value: &str) -> String {
    let value: String = value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_graphic() || *ch == ' ')
        .take(24)
        .collect();
    let value = value.trim();
    if value.is_empty() {
        "Player".into()
    } else {
        value.into()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyLayout {
    /// Up, right, down, left, bomb, action. Names match Macroquad's KeyCode.
    pub keys: [String; 6],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreferredColor {
    pub device: Device,
    pub color: u8,
}

/// Network hosting preferences do not rewrite the local game's roster/rules.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostingSettings {
    pub arena: usize,
    pub random_arena: bool,
    pub scheme: usize,
    pub wins: u16,
    pub win_by_kills: bool,
    pub round_seconds: u32,
    pub teams: bool,
    pub random_starts: bool,
    pub enclosement_depth: u8,
    pub conveyor_speed: u8,
    pub stomped_bombs_detonate: bool,
    pub slot_teams: [u8; 10],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub participant_count: usize,
    pub participants: Vec<Participant>,
    pub arena: usize,
    /// Pick a concrete arena at match start; `arena` remains a valid preview.
    pub random_arena: bool,
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
    pub nickname: String,
    pub stomped_bombs_detonate: bool,
    pub diseases_destroyable: bool,
    pub disable_gameplay_music: bool,
    pub assign_keyboard_player: bool,
    pub fullscreen: bool,
    pub volume: f32,
    pub keyboard: [KeyLayout; 2],
    /// Standard gamepad buttons; selectable in the Controls screen.
    pub pad_bomb: String,
    pub pad_action: String,
    pub network_devices: Option<Vec<Device>>,
    pub network_colors: Vec<PreferredColor>,
    pub hosting: Option<HostingSettings>,
    pub lan_address: String,
    pub asset_mod: Option<std::path::PathBuf>,
    /// This window's last loaded/saved values, not another window's edits.
    #[serde(skip)]
    pub(crate) saved_snapshot: RefCell<Option<serde_json::Value>>,
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
                    enabled: true,
                })
                .collect(),
            arena: 0,
            random_arena: false,
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
            nickname: "Player".into(),
            stomped_bombs_detonate: true,
            diseases_destroyable: true,
            disable_gameplay_music: false,
            assign_keyboard_player: true,
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
            network_devices: None,
            network_colors: Vec::new(),
            hosting: None,
            asset_mod: None,
            lan_address: String::new(),
            saved_snapshot: RefCell::default(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        let mut settings: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parse settings {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("read settings {}", path.display()))
            }
        };
        settings.normalize();
        *settings.saved_snapshot.get_mut() = Some(serde_json::to_value(&settings)?);
        Ok(settings)
    }

    pub fn normalize(&mut self) {
        self.version = 1;
        self.participant_count = self.participant_count.clamp(2, 10);
        self.participants.resize_with(10, || Participant {
            device: Device::Bot,
            team: 0,
            enabled: true,
        });
        self.participants.truncate(10);
        let mut used = Vec::new();
        for (slot, participant) in self.participants.iter_mut().enumerate() {
            participant.team %= 5;
            if slot < self.participant_count
                && participant.enabled
                && participant.device != Device::Bot
            {
                if used.contains(&participant.device) {
                    participant.device = Device::Bot;
                } else {
                    used.push(participant.device.clone());
                }
            }
        }
        self.nickname = normalize_nickname(&self.nickname);
        self.wins = self.wins.clamp(1, 100);
        self.round_seconds = self.round_seconds.min(900);
        self.conveyor_speed = self.conveyor_speed.min(2);
        self.enclosement_depth = self.enclosement_depth.min(3);
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0., 1.)
        } else {
            0.65
        };
        if let Some(devices) = &mut self.network_devices {
            let mut selected = Vec::new();
            devices.retain(|device| {
                if *device == Device::Bot
                    || selected.contains(device)
                    || matches!(device, Device::Controller(index) if *index >= 10)
                {
                    false
                } else {
                    selected.push(device.clone());
                    true
                }
            });
            devices.truncate(10);
            if devices.is_empty() {
                devices.push(Device::Keyboard1);
            }
        }
        self.network_colors
            .retain(|preference| preference.color < 10 && preference.device != Device::Bot);
        if let Some(hosting) = &mut self.hosting {
            hosting.arena %= 11;
            hosting.wins = hosting.wins.clamp(1, 100);
            hosting.round_seconds = hosting.round_seconds.min(900);
            hosting.enclosement_depth = hosting.enclosement_depth.min(3);
            hosting.conveyor_speed = hosting.conveyor_speed.min(2);
            for team in &mut hosting.slot_teams {
                *team %= 2;
            }
            if hosting.teams {
                hosting.win_by_kills = false;
            }
        }
        if self.lan_address.len() > 128 {
            self.lan_address.clear();
        }
    }

    pub fn local_network_devices(&self) -> Vec<Device> {
        if let Some(devices) = &self.network_devices {
            return devices.clone();
        }
        let mut devices: Vec<_> = self
            .active_participants()
            .filter(|(_, participant)| participant.device != Device::Bot)
            .map(|(_, participant)| participant.device.clone())
            .collect();
        if devices.is_empty() {
            devices.push(Device::Keyboard1);
        }
        devices
    }

    pub fn host_settings(&self) -> Self {
        let mut settings = self.clone();
        if let Some(hosting) = &self.hosting {
            settings.arena = hosting.arena;
            settings.random_arena = hosting.random_arena;
            settings.scheme = hosting.scheme;
            settings.wins = hosting.wins;
            settings.win_by_kills = hosting.win_by_kills;
            settings.round_seconds = hosting.round_seconds;
            settings.teams = hosting.teams;
            settings.random_starts = hosting.random_starts;
            settings.sudden_death = hosting.enclosement_depth != 0;
            settings.enclosement_depth = hosting.enclosement_depth;
            settings.conveyor_speed = hosting.conveyor_speed;
            settings.stomped_bombs_detonate = hosting.stomped_bombs_detonate;
            for (participant, team) in settings.participants.iter_mut().zip(hosting.slot_teams) {
                participant.team = team;
            }
        }
        settings
    }

    pub fn remember_hosting(
        &mut self,
        config: &bomberman_core::GameConfig,
        scheme: usize,
        random_arena: bool,
    ) {
        self.hosting = Some(HostingSettings {
            arena: config.arena.index(),
            random_arena,
            scheme,
            wins: config.wins_to_match,
            win_by_kills: config.win_by_kills,
            round_seconds: config.round_time_ms.map_or(0, |ms| ms / 1000),
            teams: config.team_play,
            random_starts: config.random_starts,
            enclosement_depth: config.enclosement_depth,
            conveyor_speed: config.conveyor_speed,
            stomped_bombs_detonate: config.stomped_bombs_detonate,
            slot_teams: std::array::from_fn(|slot| {
                config
                    .players
                    .get(slot)
                    .map_or(u8::from(slot >= 5), |player| player.team)
            }),
        });
    }

    pub fn remember_network_color(&mut self, device: Device, color: u8) {
        if let Some(preference) = self.network_colors.iter_mut().find(|p| p.device == device) {
            preference.color = color;
        } else {
            self.network_colors.push(PreferredColor { device, color });
        }
    }

    /// Enable a slot without activating the unused slots of an older profile.
    /// Existing roster choices and saved device/team assignments are retained.
    pub fn enable_slot(&mut self, index: usize) {
        for participant in self
            .participants
            .iter_mut()
            .take(index)
            .skip(self.participant_count)
        {
            participant.enabled = false;
        }
        self.participants[index].enabled = true;
        self.participant_count = self.participant_count.max(index + 1);
    }

    pub fn assign(&mut self, index: usize, device: Device) {
        self.enable_slot(index);
        if device != Device::Bot {
            for (other, participant) in self
                .participants
                .iter_mut()
                .take(self.participant_count)
                .enumerate()
            {
                if other != index && participant.enabled && participant.device == device {
                    participant.device = Device::Bot;
                }
            }
        }
        self.participants[index].device = device;
    }

    pub fn active_participants(&self) -> impl Iterator<Item = (usize, &Participant)> {
        self.participants
            .iter()
            .take(self.participant_count)
            .enumerate()
            .filter(|(_, participant)| participant.enabled)
    }

    /// Apply an explicit opt-in to automatic keyboard assignment. Existing
    /// active choices are retained; returning to setup must not undo saved OFFs.
    pub fn assign_default_keyboard(&mut self) -> Option<usize> {
        if !self.assign_keyboard_player
            || self.active_participants().any(|(_, participant)| {
                matches!(participant.device, Device::Keyboard1 | Device::Keyboard2)
            })
        {
            return None;
        }
        let slot = self
            .participants
            .iter()
            .enumerate()
            .find(|(slot, participant)| *slot >= self.participant_count || !participant.enabled)
            .map(|(slot, _)| slot)?;
        self.assign(slot, Device::Keyboard1);
        Some(slot)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create settings directory {}", parent.display()))?;
        }
        // Atomic rename prevents torn JSON, but two windows can still overwrite
        // each other's preferences. Serialize read/merge/write using a stable
        // sidecar lock, then apply only fields changed by this window.
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("open settings lock {}", lock_path.display()))?;
        lock.lock().context("lock settings for save")?;
        let current = serde_json::to_value(self)?;
        let mut merged: serde_json::Map<String, serde_json::Value> = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("read current settings {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read current settings {}", path.display()))
            }
        };
        let baseline = self.saved_snapshot.borrow();
        for (key, value) in current.as_object().context("settings must be an object")? {
            if !merged.contains_key(key)
                || baseline.as_ref().and_then(|snapshot| snapshot.get(key)) != Some(value)
            {
                merged.insert(key.clone(), value.clone());
            }
        }
        drop(baseline);
        let mut temp = path.as_os_str().to_os_string();
        temp.push(format!(".{}.tmp", std::process::id()));
        let temp = PathBuf::from(temp);
        std::fs::write(&temp, serde_json::to_vec_pretty(&merged)?)
            .with_context(|| format!("write settings {}", temp.display()))?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("save settings {}", path.display()))?;
        // Keep our own baseline: recording merged fields here would make this
        // window's stale in-memory copy look like an edit on its next save.
        *self.saved_snapshot.borrow_mut() = Some(current);
        Ok(())
    }
}

pub fn default_path() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(base).join("atomic-bomberman/settings.json")
    } else if cfg!(windows) {
        bomberman_net::invitation::profile_path().with_file_name("settings.json")
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
    fn two_windows_preserve_host_rules_when_the_stale_client_saves_and_closes() {
        let root =
            std::env::temp_dir().join(format!("bomberman-shared-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for existing in [false, true] {
            let path = root.join(format!("settings-{existing}.json"));
            if existing {
                Settings::default().save(&path).unwrap();
            }
            let mut host = Settings::load(&path).unwrap();
            let mut client = Settings::load(&path).unwrap();
            let config = bomberman_core::GameConfig {
                arena: bomberman_core::Arena::ALL[5],
                wins_to_match: 37,
                round_time_ms: Some(90_000),
                enclosement_depth: 3,
                random_starts: false,
                conveyor_speed: 2,
                stomped_bombs_detonate: false,
                ..Default::default()
            };
            host.remember_hosting(&config, 42, true);
            host.save(&path).unwrap();
            client.nickname = "Other window".into();
            client.save(&path).unwrap();
            client.save(&path).unwrap(); // Closing must not turn stale fields into changes.
            assert_eq!(Settings::load(&path).unwrap().host_settings().wins, 37);
            host.save(&path).unwrap(); // Closing the older host must preserve the nickname too.
            let loaded = Settings::load(&path).unwrap();
            let hosted = loaded.host_settings();
            assert_eq!(loaded.nickname, "Other window");
            assert_eq!((hosted.arena, hosted.wins, hosted.scheme), (5, 37, 42));
            assert!(hosted.random_arena);
            assert_eq!(hosted.round_seconds, 90);
            assert_eq!(hosted.enclosement_depth, 3);
            assert!(!hosted.random_starts);
            assert_eq!(hosted.conveyor_speed, 2);
            assert!(!hosted.stomped_bombs_detonate);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn simultaneous_windows_merge_distinct_settings_and_can_explicitly_clear_a_mod() {
        let root =
            std::env::temp_dir().join(format!("bomberman-settings-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        let initial = Settings {
            asset_mod: Some(root.join("mod")),
            ..Default::default()
        };
        initial.save(&path).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut first = Settings::load(&path).unwrap();
        let mut second = Settings::load(&path).unwrap();
        first.asset_mod = None;
        second.wins = 19;
        let first_barrier = barrier.clone();
        let first_path = path.clone();
        let second_path = path.clone();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                first_barrier.wait();
                first.save(&first_path).unwrap();
                first.save(&first_path).unwrap();
            });
            scope.spawn(move || {
                barrier.wait();
                second.save(&second_path).unwrap();
                second.save(&second_path).unwrap();
            });
        });
        let saved = Settings::load(&path).unwrap();
        assert_eq!(saved.wins, 19);
        assert!(saved.asset_mod.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn network_preferences_reload_without_rewriting_local_rules_or_players() {
        let root =
            std::env::temp_dir().join(format!("bomberman-network-settings-{}", std::process::id()));
        let path = root.join("settings.json");
        let mut settings = Settings {
            teams: true,
            wins: 4,
            arena: 2,
            ..Settings::default()
        };
        let local_devices: Vec<_> = settings
            .active_participants()
            .map(|(_, p)| p.device.clone())
            .collect();
        let config = bomberman_core::GameConfig {
            arena: bomberman_core::Arena::ALL[5],
            wins_to_match: 37,
            round_time_ms: Some(90_000),
            enclosement_depth: 3,
            random_starts: false,
            conveyor_speed: 2,
            stomped_bombs_detonate: false,
            ..bomberman_core::GameConfig::default()
        };
        settings.remember_hosting(&config, 42, true);
        settings.network_devices = Some(vec![Device::Keyboard2, Device::Controller(0)]);
        settings.remember_network_color(Device::Keyboard2, 8);
        settings.lan_address = "[::1]:2007".into();
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert!(loaded.teams);
        assert_eq!(loaded.wins, 4);
        assert_eq!(loaded.arena, 2);
        assert_eq!(
            loaded
                .active_participants()
                .map(|(_, p)| p.device.clone())
                .collect::<Vec<_>>(),
            local_devices
        );
        assert_eq!(
            loaded.local_network_devices(),
            vec![Device::Keyboard2, Device::Controller(0)]
        );
        assert_eq!(loaded.network_colors[0].color, 8);
        assert_eq!(loaded.lan_address, "[::1]:2007");
        let hosted = loaded.host_settings();
        assert_eq!((hosted.arena, hosted.wins, hosted.scheme), (5, 37, 42));
        assert!(hosted.random_arena);
        assert!(!hosted.teams);
        assert_eq!(hosted.round_seconds, 90);
        assert_eq!(hosted.enclosement_depth, 3);
        assert!(!hosted.stomped_bombs_detonate);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_profiles_keep_local_assignments_until_network_controls_are_saved() {
        let mut settings = Settings::default();
        settings.assign(1, Device::Keyboard2);
        assert_eq!(
            settings.local_network_devices(),
            vec![Device::Keyboard1, Device::Keyboard2]
        );
        settings.network_devices = Some(vec![Device::Keyboard2]);
        assert_eq!(settings.local_network_devices(), vec![Device::Keyboard2]);
        assert_eq!(settings.participants[0].device, Device::Keyboard1);
        settings.remember_network_color(Device::Keyboard2, 4);
        settings.remember_network_color(Device::Keyboard2, 6);
        assert_eq!(settings.network_colors.len(), 1);
        assert_eq!(settings.network_colors[0].color, 6);
    }

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
        settings.random_arena = true;
        settings.wins = 100;
        settings.asset_mod = Some(root.join("mod with spaces"));
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded.keyboard[0].keys[4], "Z");
        assert_eq!(loaded.participant_count, 10);
        assert!(loaded.random_arena);
        assert_eq!(loaded.wins, 100);
        assert_eq!(loaded.asset_mod, settings.asset_mod);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_settings_keep_enabled_players_bindings_and_option_defaults() {
        let mut settings: Settings = serde_json::from_str(
            r#"{"participant_count":2,"participants":[{"device":"Keyboard1","team":0},{"device":"Keyboard2","team":1}],"keyboard":[{"keys":["Up","Right","Down","Left","Z","RightShift"]},{"keys":["W","D","S","A","LeftShift","E"]}]}"#,
        )
        .unwrap();
        settings.normalize();
        assert_eq!(settings.active_participants().count(), 2);
        assert!(settings
            .participants
            .iter()
            .all(|participant| participant.enabled));
        assert_eq!(settings.keyboard[0].keys[4], "Z");
        assert_eq!(settings.nickname, "Player");
        assert!(settings.asset_mod.is_none());
        assert!(!settings.random_arena);
        assert!(settings.stomped_bombs_detonate);
        assert!(settings.diseases_destroyable);
        assert!(settings.assign_keyboard_player);
        assert!(!settings.disable_gameplay_music);
    }

    #[test]
    fn disabled_players_keep_their_assignment_without_reserving_it() {
        let mut settings = Settings::default();
        settings.participants[0].enabled = false;
        settings.participants[9].device = Device::Keyboard1;
        settings.normalize();
        assert_eq!(settings.participants[0].device, Device::Keyboard1);
        assert_eq!(settings.participants[9].device, Device::Keyboard1);
        assert_eq!(
            settings
                .active_participants()
                .map(|(slot, _)| slot)
                .collect::<Vec<_>>(),
            (1..10).collect::<Vec<_>>()
        );
        settings.assign(0, Device::Keyboard1);
        assert!(settings.participants[0].enabled);
        assert_eq!(settings.participants[9].device, Device::Bot);
    }

    #[test]
    fn sparse_slot_activation_keeps_legacy_unused_slots_off() {
        let mut settings: Settings = serde_json::from_str(
            r#"{"participant_count":2,"participants":[{"device":"Keyboard1","team":0},{"device":{"Controller":0},"team":1}]}"#,
        )
        .unwrap();
        settings.normalize();
        settings.participants[7].device = Device::Controller(1);
        settings.participants[7].team = 1;
        settings.assign(9, Device::Keyboard2);
        assert_eq!(settings.participant_count, 10);
        assert_eq!(
            settings
                .active_participants()
                .map(|(slot, _)| slot)
                .collect::<Vec<_>>(),
            [0, 1, 9]
        );
        assert_eq!(settings.participants[0].device, Device::Keyboard1);
        assert_eq!(settings.participants[1].device, Device::Controller(0));
        assert_eq!(settings.participants[1].team, 1);
        assert_eq!(settings.participants[7].device, Device::Controller(1));
        assert_eq!(settings.participants[7].team, 1);

        settings.participants[1].enabled = false;
        settings.enable_slot(7);
        assert!(!settings.participants[1].enabled);
        assert_eq!(settings.participants[7].device, Device::Controller(1));
        assert_eq!(settings.participants[7].team, 1);
        assert!(settings.participants[9].enabled);
        settings.normalize();
        assert_eq!(
            settings
                .active_participants()
                .map(|(slot, _)| slot)
                .collect::<Vec<_>>(),
            [0, 7, 9]
        );
    }

    #[test]
    fn automatic_keyboard_opt_in_fills_only_an_unused_slot() {
        let mut settings = Settings::default();
        settings.assign(0, Device::Controller(0));
        settings.participants[3].enabled = false;
        assert_eq!(settings.assign_default_keyboard(), Some(3));
        assert_eq!(settings.participants[0].device, Device::Controller(0));
        assert_eq!(settings.participants[1].device, Device::Bot);
        assert!(settings.participants[1].enabled);
        assert_eq!(settings.participants[3].device, Device::Keyboard1);
        assert!(settings.participants[3].enabled);
        assert_eq!(settings.assign_default_keyboard(), None);

        settings.assign(3, Device::Keyboard2);
        settings.participants[2].enabled = false;
        assert_eq!(settings.assign_default_keyboard(), None);
        assert!(!settings.participants[2].enabled);
    }

    #[test]
    fn automatic_keyboard_respects_disabled_option_and_full_roster() {
        let mut settings = Settings::default();
        settings.assign(0, Device::Controller(0));
        assert_eq!(settings.assign_default_keyboard(), None);
        assert_eq!(settings.participants[0].device, Device::Controller(0));
        assert!(settings.participants[1..]
            .iter()
            .all(|participant| participant.enabled && participant.device == Device::Bot));

        settings.participants[1].enabled = false;
        settings.assign_keyboard_player = false;
        assert_eq!(settings.assign_default_keyboard(), None);
        assert!(!settings.participants[1].enabled);

        settings.assign_keyboard_player = true;
        settings.participant_count = 2;
        settings.participants[1].enabled = true;
        assert_eq!(settings.assign_default_keyboard(), Some(2));
        assert_eq!(settings.participant_count, 3);
    }

    #[test]
    fn nicknames_are_trimmed_bounded_and_renderable() {
        assert_eq!(normalize_nickname("  Andrei\n\t "), "Andrei");
        assert_eq!(normalize_nickname("\n\u{0}\t"), "Player");
        assert_eq!(
            normalize_nickname("12345678901234567890123456789"),
            "123456789012345678901234"
        );
        assert_eq!(
            normalize_nickname("  12345678901234567890123456789  "),
            "123456789012345678901234"
        );
        assert_eq!(normalize_nickname("Player \u{1f4a3}"), "Player");
    }
}
