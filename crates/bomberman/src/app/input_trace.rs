//! Opt-in debug instrumentation for an isolated OS-key-to-render test.
//! Timing frames stay in memory; F12 writes them outside measured intervals.
use super::*;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_FRAMES: usize = 3600;

pub(super) fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
pub(super) struct Observed {
    timestamp: u64,
    keys: Vec<String>,
}
pub(super) struct InputTrace {
    enabled: bool,
    frames: VecDeque<Value>,
}
impl InputTrace {
    pub(super) fn new() -> Self {
        Self {
            enabled: std::env::var("ATOMIC_BOMBERMAN_INPUT_TRACE").as_deref() == Ok("1"),
            frames: VecDeque::new(),
        }
    }
    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }
    pub(super) fn observe(&self) -> Option<Observed> {
        if !self.enabled {
            return None;
        }
        let timestamp = now_ns();
        let mut keys = get_keys_down()
            .iter()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>();
        keys.sort();
        Some(Observed { timestamp, keys })
    }
    pub(super) fn push(&mut self, sample: Value) {
        if self.frames.len() == MAX_FRAMES {
            self.frames.pop_front();
        }
        self.frames.push_back(sample);
    }
    pub(super) fn frames(&self) -> &VecDeque<Value> {
        &self.frames
    }
    pub(super) fn presented(&mut self) {
        if let Some(frame) = self.frames.back_mut() {
            if frame["swap_returned_ns"].is_null() {
                frame["swap_returned_ns"] = now_ns().into();
            }
        }
    }
}

fn players(simulation: &Simulation) -> Vec<Value> {
    simulation.players.iter().map(|p| json!({"id":p.id,"alive":p.alive,
        "visible":p.presentation.render_visible,"render_alive":p.presentation.render_alive,
        "position":p.presentation.render_position,"physics":p.position,"facing":p.presentation.render_facing,
        "moving":p.presentation.render_moving,"animation":p.presentation.render_frame,
        "special":p.presentation.render_special,"walk_pixels":p.presentation.walk_pixels,
        "airborne_ms":p.airborne_ms})).collect()
}
fn bombs(simulation: &Simulation) -> Vec<Value> {
    simulation
        .bombs
        .iter()
        .map(|bomb| {
            json!({"id":bomb.id,"owner":bomb.owner,
        "position":bomb.position,"fuse_ms":bomb.fuse_ms})
        })
        .collect()
}
pub(super) fn sample(app: &App, observed: Option<Observed>) -> Option<Value> {
    let observed = observed?;
    let simulation = app.simulation.as_ref()?;
    let network = app.network.as_ref();
    let replay = network.and_then(|network| network.replay.as_ref());
    let canonical = replay.map_or(simulation, |replay| &replay.simulation);
    if !matches!(app.screen, Screen::Battle) || network.is_some_and(|network| network.is_starting())
    {
        return None;
    }
    Some(json!({
        "frame":app.frame,"version":bomberman_net::APP_VERSION,"protocol":bomberman_net::PROTOCOL_VERSION,
        "input_observed_ns":observed.timestamp,"keys":observed.keys,
        "draw_submitted_ns":now_ns(),"swap_returned_ns":Value::Null,
        "tick":replay.map_or(u64::from(app.frame), |replay| replay.tick),"round":canonical.round,
        "generation":network.map(|network|network.generation),
        "canonical_hash":replay.map(|replay|replay.hash()),
        "rtt_ms":network.and_then(|network| network.gameplay_rtt_ms),"peer":network.map(|network| network.id),
        "transport":if network.is_some() { "network" } else { "offline" },
        "players":players(simulation),"bombs":bombs(simulation),
        "flames":simulation.flames.iter().map(|flame| flame.tile).collect::<Vec<_>>(),
        "confirmed_players":players(canonical),"confirmed_bombs":bombs(canonical),
        "confirmed_flames":canonical.flames.iter().map(|flame| flame.tile).collect::<Vec<_>>(),
    }))
}
