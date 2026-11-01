//! Shared lobby setup carried by protocol 8's reliable control connection.
//! The frozen active roster is compiled once before the existing Start message.
use crate::*;
use bomberman_core::{ControlKind, GameConfig, PlayerConfig};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct MatchRoster {
    pub config: GameConfig,
    pub owners: Vec<Option<PeerId>>,
    pub colors: [u8; MAX_PLAYERS],
    pub slots: Vec<usize>,
}

impl Lobby {
    pub fn enable_setup(&mut self, nickname: String) -> Result<(), NetError> {
        if !valid_nickname(&nickname) {
            return Err(NetError::Protocol);
        }
        if self.setup.is_some() {
            return self.set_profile(HOST, nickname);
        }
        let old_len = self.config.players.len();
        self.config
            .players
            .resize_with(MAX_PLAYERS, PlayerConfig::default);
        for slot in old_len..MAX_PLAYERS {
            self.config.players[slot].team = u8::from(slot >= MAX_PLAYERS / 2);
        }
        self.owners.resize(MAX_PLAYERS, None);
        let mut slots = [SlotState::Open; MAX_PLAYERS];
        let mut devices = [None; MAX_PLAYERS];
        let mut local = 0;
        for (slot, owner) in self.owners.iter().enumerate() {
            if *owner == Some(HOST) {
                slots[slot] = SlotState::Human(HOST);
                devices[slot] = Some(local);
                local += 1;
            }
        }
        self.setup = Some(SetupLobby {
            slots,
            devices,
            device_counts: [(HOST, local)].into(),
            phase: SetupPhase::Lobby,
            random_arena: false,
            scheme_file: None,
            nicknames: [(HOST, nickname)].into(),
        });
        self.sync_setup();
        self.validate()
    }

    pub fn set_profile(&mut self, peer: PeerId, nickname: String) -> Result<(), NetError> {
        if !self.peers.contains(&peer) || !valid_nickname(&nickname) {
            return Err(NetError::Protocol);
        }
        self.setup
            .as_mut()
            .ok_or(NetError::Protocol)?
            .nicknames
            .insert(peer, nickname);
        self.sync_setup();
        Ok(())
    }

    pub fn peer_label(&self, peer: PeerId) -> String {
        let Some(setup) = &self.setup else {
            return if peer == HOST {
                "Host".into()
            } else {
                format!("Player {peer}")
            };
        };
        let Some(name) = setup.nicknames.get(&peer) else {
            return format!("Player {peer} (updating setup)");
        };
        if setup
            .nicknames
            .values()
            .filter(|other| *other == name)
            .count()
            > 1
        {
            format!("{name} #{peer}")
        } else {
            name.clone()
        }
    }

    pub fn set_slot(&mut self, slot: usize, state: SlotState) -> Result<(), NetError> {
        let setup = self.setup.as_mut().ok_or(NetError::Protocol)?;
        if slot >= MAX_PLAYERS || matches!(state, SlotState::Human(_)) {
            return Err(NetError::Ownership);
        }
        setup.slots[slot] = state;
        setup.devices[slot] = None;
        self.sync_setup();
        Ok(())
    }

    /// Assign a local control to a colored row without moving its color or
    /// touching other devices. A foreign NET row needs an explicit host override.
    pub fn select_player(
        &mut self,
        peer: PeerId,
        slot: usize,
        device: Option<u8>,
    ) -> Result<(), NetError> {
        if slot >= MAX_PLAYERS || !self.peers.contains(&peer) {
            return Err(NetError::Ownership);
        }
        let setup = self.setup.as_ref().ok_or(NetError::Protocol)?;
        let count = setup.device_counts.get(&peer).ok_or(NetError::Ownership)?;
        if device.is_some_and(|device| device >= *count) {
            return Err(NetError::Ownership);
        }
        match setup.slots[slot] {
            SlotState::Human(owner) if owner != peer => return Err(NetError::ColorTaken),
            SlotState::Ai | SlotState::Closed if peer != HOST => return Err(NetError::ColorTaken),
            _ => {}
        }
        let mut next = self.clone();
        let setup = next.setup.as_mut().unwrap();
        if let Some(device) = device {
            for old in 0..MAX_PLAYERS {
                if setup.slots[old] == SlotState::Human(peer) && setup.devices[old] == Some(device)
                {
                    setup.slots[old] = SlotState::Open;
                    setup.devices[old] = None;
                }
            }
            setup.slots[slot] = SlotState::Human(peer);
            setup.devices[slot] = Some(device);
        } else {
            setup.slots[slot] = SlotState::Open;
            setup.devices[slot] = None;
        }
        next.sync_setup();
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn set_setup_phase(&mut self, phase: SetupPhase) -> Result<(), NetError> {
        self.setup.as_mut().ok_or(NetError::Protocol)?.phase = phase;
        Ok(())
    }

    pub fn set_random_arena(&mut self, random: bool) -> Result<(), NetError> {
        self.setup.as_mut().ok_or(NetError::Protocol)?.random_arena = random;
        Ok(())
    }

    pub fn set_scheme_file(&mut self, filename: String) -> Result<(), NetError> {
        if !valid_scheme_file(&filename) {
            return Err(NetError::Protocol);
        }
        self.setup.as_mut().ok_or(NetError::Protocol)?.scheme_file = Some(filename);
        Ok(())
    }

    pub fn set_team(&mut self, slot: usize, team: u8) -> Result<(), NetError> {
        if slot >= self.config.players.len() || team > 1 {
            return Err(NetError::Ownership);
        }
        self.config.players[slot].team = team;
        Ok(())
    }

    pub fn active_slots(&self) -> Vec<usize> {
        match &self.setup {
            Some(setup) => setup
                .slots
                .iter()
                .enumerate()
                .filter_map(|(slot, state)| state.active().then_some(slot))
                .collect(),
            None => (0..self.config.players.len()).collect(),
        }
    }

    pub fn match_roster(&self) -> Result<MatchRoster, NetError> {
        let slots = self.active_slots();
        let mut config = self.config.clone();
        config.players = slots
            .iter()
            .map(|slot| self.config.players[*slot].clone())
            .collect();
        config.player_slots = if slots.iter().enumerate().all(|(i, slot)| i == *slot) {
            vec![]
        } else {
            slots.clone()
        };
        config.validate().map_err(|_| NetError::WaitingForPlayers)?;
        let owners = slots.iter().map(|slot| self.owners[*slot]).collect();
        let mut colors = DEFAULT_COLORS;
        let mut palette_slots = slots.clone();
        palette_slots.extend((0..MAX_PLAYERS).filter(|slot| !slots.contains(slot)));
        for (i, slot) in palette_slots.iter().enumerate() {
            colors[i] = self.colors[*slot];
        }
        Ok(MatchRoster {
            config,
            owners,
            colors,
            slots,
        })
    }

    pub(crate) fn assign_open(&mut self, peer: PeerId, count: u8) -> Result<(), NetError> {
        if count as usize > MAX_PLAYERS || !self.peers.contains(&peer) {
            return Err(NetError::Ownership);
        }
        let mut next = self.clone();
        let setup = next.setup.as_mut().ok_or(NetError::Protocol)?;
        setup.device_counts.insert(peer, count);
        let mut retained = BTreeSet::new();
        for slot in 0..MAX_PLAYERS {
            if setup.slots[slot] == SlotState::Human(peer) {
                let device = setup.devices[slot].ok_or(NetError::Protocol)?;
                if device >= count {
                    setup.slots[slot] = SlotState::Open;
                    setup.devices[slot] = None;
                } else {
                    retained.insert(device);
                }
            }
        }
        let missing: Vec<_> = (0..count)
            .filter(|device| !retained.contains(device))
            .collect();
        if setup
            .slots
            .iter()
            .filter(|s| **s == SlotState::Open)
            .count()
            < missing.len()
        {
            return Err(NetError::Full);
        }
        let mut selected = Vec::new();
        for _ in &missing {
            let active_by_team = [0, 1].map(|team| {
                setup
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(slot, state)| {
                        state.active() && next.config.players[*slot].team == team
                    })
                    .count()
            });
            let slot = (0..MAX_PLAYERS)
                .filter(|slot| setup.slots[*slot] == SlotState::Open)
                .min_by_key(|slot| {
                    (
                        if next.config.team_play {
                            active_by_team[next.config.players[*slot].team as usize]
                        } else {
                            0
                        },
                        *slot,
                    )
                })
                .ok_or(NetError::Full)?;
            setup.slots[slot] = SlotState::Human(peer);
            selected.push(slot);
        }
        // The first assignment also works for old clients' ordered device lists.
        selected.sort_unstable();
        for (device, slot) in missing.into_iter().zip(selected) {
            setup.devices[slot] = Some(device);
        }
        next.sync_setup();
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Register available controls without reactivating rows deliberately set
    /// OFF. First registration may take free rows; later inventory changes
    /// require explicit row selection and never activate an unrelated color.
    pub fn declare_devices(&mut self, peer: PeerId, count: u8) -> Result<(), NetError> {
        if count as usize > MAX_LOCAL_DEVICES || !self.peers.contains(&peer) {
            return Err(NetError::Ownership);
        }
        let mut next = self.clone();
        let setup = next.setup.as_mut().ok_or(NetError::Protocol)?;
        let initial_devices = if setup.device_counts.insert(peer, count).is_none() {
            0..count
        } else {
            0..0
        };
        for slot in 0..MAX_PLAYERS {
            if setup.slots[slot] == SlotState::Human(peer)
                && setup.devices[slot].is_some_and(|device| device >= count)
            {
                setup.slots[slot] = SlotState::Open;
                setup.devices[slot] = None;
            }
        }
        let mut selected = Vec::new();
        for _ in initial_devices.clone() {
            let active_by_team = [0, 1].map(|team| {
                setup
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(slot, state)| {
                        state.active() && next.config.players[*slot].team == team
                    })
                    .count()
            });
            let Some(slot) = (0..MAX_PLAYERS)
                .filter(|slot| setup.slots[*slot] == SlotState::Open)
                .min_by_key(|slot| {
                    (
                        if next.config.team_play {
                            active_by_team[next.config.players[*slot].team as usize]
                        } else {
                            0
                        },
                        *slot,
                    )
                })
            else {
                break;
            };
            setup.slots[slot] = SlotState::Human(peer);
            selected.push(slot);
        }
        selected.sort_unstable();
        for (device, slot) in initial_devices.zip(selected) {
            setup.devices[slot] = Some(device);
        }
        next.sync_setup();
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn sync_setup(&mut self) {
        let Some(setup) = &self.setup else {
            return;
        };
        for slot in 0..MAX_PLAYERS {
            self.owners[slot] = if let SlotState::Human(peer) = setup.slots[slot] {
                Some(peer)
            } else {
                None
            };
            self.config.players[slot].control = if self.owners[slot].is_some() {
                ControlKind::Human
            } else {
                ControlKind::Bot
            };
        }
        self.update_color_names();
    }

    pub(crate) fn validate_setup(&self) -> Result<(), NetError> {
        let Some(setup) = &self.setup else {
            return Ok(());
        };
        if self.config.players.len() != MAX_PLAYERS
            || self.owners.len() != MAX_PLAYERS
            || setup
                .scheme_file
                .as_ref()
                .is_some_and(|filename| !valid_scheme_file(filename))
            || setup.device_counts.iter().any(|(peer, count)| {
                !self.peers.contains(peer)
                    || !setup.nicknames.contains_key(peer)
                    || *count as usize > MAX_LOCAL_DEVICES
            })
            || setup
                .nicknames
                .iter()
                .any(|(peer, name)| !self.peers.contains(peer) || !valid_nickname(name))
        {
            return Err(NetError::Protocol);
        }
        let mut devices = BTreeSet::new();
        for slot in 0..MAX_PLAYERS {
            match setup.slots[slot] {
                SlotState::Human(peer) => {
                    let device = setup.devices[slot].ok_or(NetError::Protocol)?;
                    if self.owners[slot] != Some(peer)
                        || self.config.players[slot].control != ControlKind::Human
                        || setup
                            .device_counts
                            .get(&peer)
                            .is_none_or(|count| device >= *count)
                        || !devices.insert((peer, device))
                    {
                        return Err(NetError::Protocol);
                    }
                }
                _ if self.owners[slot].is_some()
                    || setup.devices[slot].is_some()
                    || self.config.players[slot].control != ControlKind::Bot =>
                {
                    return Err(NetError::Protocol)
                }
                _ => {}
            }
        }
        Ok(())
    }
}
