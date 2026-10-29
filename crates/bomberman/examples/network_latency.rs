//! Exercise the production session and its renderer-selected presentation state.
//! This fixture has no graphics, audio device, or original asset dependency.
#[allow(dead_code)]
#[path = "../src/network.rs"]
mod network;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;

use anyhow::{ensure, Context, Result};
use bomberman_core::{Direction, GameConfig, PlayerInput, Position, RoundPhase, TilePos};
use bomberman_net::{
    transport::{Connection, Destination},
    Action, Compatibility, Event, Invitation, PORT,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_nanos(1_000_000_000 / 60);
const SAMPLE_COUNT: usize = 10;
const SAMPLE_PERIOD: Duration = Duration::from_millis(400);
const PULSE: Duration = Duration::from_millis(40);

#[derive(Serialize)]
struct Sample {
    direction: Direction,
    canonical_tick: u64,
    sample_to_visible_ms: Option<f64>,
    sample_to_confirmation_ms: Option<f64>,
    visible_before_confirmation: bool,
    actual_release_ms: Option<f64>,
    backward_correction_pixels: f64,
    max_backward_step_pixels: f64,
    last_movement_after_release_ms: Option<f64>,
    settled_after_release_ms: Option<f64>,
}

struct Trial {
    started: Instant,
    display: Position,
    canonical: Position,
    previous_display: Position,
    result: Sample,
}
impl Trial {
    fn observe(&mut self, display: Position, canonical: Position) {
        let elapsed = self.started.elapsed().as_secs_f64() * 1000.;
        let signed_step = match self.result.direction {
            Direction::Right => display.x - self.previous_display.x,
            Direction::Left => self.previous_display.x - display.x,
            _ => unreachable!(),
        };
        if signed_step < 0 {
            let backward = f64::from(-signed_step) / 100.;
            self.result.backward_correction_pixels += backward;
            self.result.max_backward_step_pixels =
                self.result.max_backward_step_pixels.max(backward);
        }
        if display != self.previous_display {
            if let Some(released) = self.result.actual_release_ms {
                self.result.last_movement_after_release_ms = Some((elapsed - released).max(0.));
            }
        }
        self.previous_display = display;
        let changed = |position: Position, before: Position| match self.result.direction {
            Direction::Right => position.x > before.x,
            Direction::Left => position.x < before.x,
            _ => unreachable!(),
        };
        if self.result.sample_to_confirmation_ms.is_none() && changed(canonical, self.canonical) {
            self.result.sample_to_confirmation_ms = Some(elapsed);
        }
        if self.result.sample_to_visible_ms.is_none() && changed(display, self.display) {
            self.result.sample_to_visible_ms = Some(elapsed);
            self.result.visible_before_confirmation =
                self.result.sample_to_confirmation_ms.is_none();
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Datagram coordinator verification: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len() >= 3, "Usage: network_latency <host|join> <lan|online> <report.json> [host-address|room-code]; online link on stdin");
    ensure!(matches!(args[0].as_str(), "host" | "join"), "Unknown role");
    ensure!(
        matches!(args[1].as_str(), "lan" | "online"),
        "Unknown transport"
    );
    let host = args[0] == "host";
    let online = args[1] == "online";
    let report = PathBuf::from(&args[2]);
    let option = |name: &str, default: usize| -> Result<usize> {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map_or(Ok(default), |pair| {
                pair[1].parse().context("Invalid fixture count")
            })
    };
    let local_seats = option("--seats", 1)?;
    let players = option("--players", 2)?;
    ensure!(
        (1..=9).contains(&local_seats) && (2..=10).contains(&players),
        "Invalid fixture seat count"
    );
    let compatibility = Compatibility::new("a".repeat(64));
    let destination = if online {
        let mut link = String::new();
        std::io::stdin().read_to_string(&mut link)?;
        let mut invitation = Invitation::parse(&link, None)?;
        invitation.room = None;
        if !host {
            let expected = args.get(3).context("Hosted room code required")?;
            let browser = Connection::connect(
                Destination::Directory(invitation.server.clone()),
                compatibility.clone(),
            );
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                ensure!(Instant::now() < deadline, "Hosted game was not discovered");
                match browser.poll()? {
                    Some(Event::Rooms(rooms)) => {
                        if rooms.iter().any(|room| {
                            &room.code == expected && room.joinable(&compatibility).is_ok()
                        }) {
                            invitation.room = Some(expected.clone());
                            break;
                        }
                    }
                    Some(Event::Error(error)) => return Err(error.into()),
                    _ => {}
                }
                thread::sleep(POLL);
            }
        }
        Destination::Online(invitation)
    } else {
        let address = if host {
            SocketAddr::from((Ipv4Addr::LOCALHOST, PORT))
        } else {
            args.get(3).context("LAN host address required")?.parse()?
        };
        Destination::Lan { address, host }
    };
    let mut config = GameConfig {
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        ..Default::default()
    };
    config.players.truncate(players);
    config.scheme.brick_density = 0;
    config.scheme.rows.fill("...............".into());
    config.scheme.starts[0].tile = TilePos::new(2, 2);
    config.scheme.starts[1].tile = TilePos::new(8, 6);
    let mut session = network::Network::new(
        destination,
        compatibility,
        host.then_some(config),
        vec![settings::Device::Keyboard1; local_seats],
    )?;
    let started = Instant::now();
    let mut previous = started;
    let mut announced = false;
    let mut requested_start = false;
    let mut paused = false;
    let mut trial: Option<Trial> = None;
    let mut samples = Vec::new();
    let mut measured_rtt_ms = Vec::new();
    loop {
        let iteration = Instant::now();
        ensure!(
            started.elapsed() < Duration::from_secs(60),
            "Session verification timed out"
        );
        if !host && !session.is_starting() && trial.is_none() && samples.len() < SAMPLE_COUNT {
            if let Some(replay) = &session.replay {
                if replay.tick >= 10 && matches!(replay.simulation.phase, RoundPhase::Playing) {
                    let seat = *session
                        .lobby
                        .as_ref()
                        .context("Missing lobby")?
                        .owned_seats(session.id)
                        .first()
                        .context("Missing owned seat")?;
                    let display = session.predicted.as_ref().unwrap_or(&replay.simulation);
                    trial = Some(Trial {
                        started: iteration,
                        display: display.players[seat].presentation.render_position,
                        canonical: replay.simulation.players[seat].presentation.render_position,
                        previous_display: display.players[seat].presentation.render_position,
                        result: Sample {
                            direction: if samples.len() % 2 == 0 {
                                Direction::Right
                            } else {
                                Direction::Left
                            },
                            canonical_tick: replay.tick,
                            sample_to_visible_ms: None,
                            sample_to_confirmation_ms: None,
                            visible_before_confirmation: false,
                            actual_release_ms: None,
                            backward_correction_pixels: 0.,
                            max_backward_step_pixels: 0.,
                            last_movement_after_release_ms: None,
                            settled_after_release_ms: None,
                        },
                    });
                }
            }
        }
        if let Some(current) = &mut trial {
            if current.started.elapsed() >= PULSE && current.result.actual_release_ms.is_none() {
                current.result.actual_release_ms =
                    Some(current.started.elapsed().as_secs_f64() * 1000.);
            }
        }
        let input = trial
            .as_ref()
            .filter(|trial| trial.started.elapsed() < PULSE)
            .map(|trial| PlayerInput {
                direction: Some(trial.result.direction),
                ..Default::default()
            })
            .unwrap_or_default();
        session.pump(
            iteration.duration_since(previous).as_millis() as u32,
            started.elapsed().as_millis() as u64,
            vec![input; local_seats],
        )?;
        previous = iteration;
        session.prepared(started.elapsed().as_millis() as u64)?;
        if let Some(rtt) = session.gameplay_rtt_ms {
            if measured_rtt_ms.last() != Some(&rtt) {
                measured_rtt_ms.push(rtt);
            }
        }
        if host && session.id != 0 && !announced {
            write_json(
                &report.with_extension("ready.json"),
                &serde_json::json!({"room": session.room}),
            )?;
            announced = true;
        }
        if host
            && !requested_start
            && session
                .lobby
                .as_ref()
                .is_some_and(|lobby| lobby.can_start())
        {
            session.start(1997, started.elapsed().as_millis() as u64)?;
            requested_start = true;
        }
        if host
            && session
                .replay
                .as_ref()
                .is_some_and(|replay| replay.tick >= 250)
            && !paused
        {
            session.action(Action::Pause(true))?;
            paused = true;
        }
        if let Some(replay) = &session.replay {
            if let Some(current) = &mut trial {
                let seat = session.lobby.as_ref().unwrap().owned_seats(session.id)[0];
                let display = session.predicted.as_ref().unwrap_or(&replay.simulation);
                current.observe(
                    display.players[seat].presentation.render_position,
                    replay.simulation.players[seat].presentation.render_position,
                );
                if current.started.elapsed() >= SAMPLE_PERIOD {
                    ensure!(
                        current.result.sample_to_visible_ms.is_some(),
                        "Local movement did not become visible"
                    );
                    let last_movement = current.result.last_movement_after_release_ms.unwrap_or(0.);
                    // A full correction interval of quiet artwork distinguishes
                    // settling from simply reaching the observation limit.
                    if current.started.elapsed().as_secs_f64() * 1000.
                        - current
                            .result
                            .actual_release_ms
                            .unwrap_or(PULSE.as_secs_f64() * 1000.)
                        - last_movement
                        >= 80.
                    {
                        current.result.settled_after_release_ms = Some(last_movement);
                    }
                    samples.push(trial.take().unwrap().result);
                }
            }
            if replay.tick >= 300 {
                ensure!(replay.simulation.paused, "Pause did not synchronize");
                ensure!(
                    host || samples.len() == SAMPLE_COUNT,
                    "Movement samples did not finish before pause"
                );
                let hash = format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&(replay.stage, &replay.simulation))?)
                );
                let value = serde_json::json!({
                    "passed": true,
                    "platform": std::env::consts::OS,
                    "architecture": std::env::consts::ARCH,
                    "host": host,
                    "version": bomberman_net::APP_VERSION,
                    "protocol": bomberman_net::PROTOCOL_VERSION,
                    "fixture_sha256": format!("{:x}", Sha256::digest(include_bytes!("network_latency.rs"))),
                    "transport": if online { "Internet QUIC datagrams" } else { "LAN QUIC datagrams" },
                    "presentation": "Production predicted render_position sampled at 60 Hz; no graphics window",
                    "tick": replay.tick,
                    "paused": replay.simulation.paused,
                    "state_hash": hash,
                    "gameplay_rtt_ms": measured_rtt_ms,
                    "samples": samples,
                });
                write_json(&report, &value)?;
                // Keep the room open while the client's delayed frames arrive.
                if host {
                    let finish = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < finish {
                        session.pump(0, 0, vec![PlayerInput::default(); local_seats])?;
                        thread::sleep(POLL);
                    }
                }
                println!("{value}");
                return Ok(());
            }
        }
        if let Some(wait) = POLL.checked_sub(iteration.elapsed()) {
            thread::sleep(wait);
        }
    }
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
