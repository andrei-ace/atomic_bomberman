//! Run the production native coordinator across machines without graphics/assets.
#[allow(dead_code)]
#[path = "../src/network.rs"]
mod network;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;
use anyhow::{ensure, Context, Result};
use bomberman_core::{GameConfig, PlayerInput};
use bomberman_net::{
    transport::{Connection, Destination},
    Action, Compatibility, Event, Invitation, PORT,
};
use std::{
    io::Read,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("Network fixture: {error:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(args.len()>=3,"Usage: network_fixture <host|join> <lan|online> <report.json> [host-address or room-code]; online link on stdin");
    let host = args[0] == "host";
    let online = args[1] == "online";
    let report = PathBuf::from(&args[2]);
    let compatibility = Compatibility::new("a".repeat(64));
    let destination = if online {
        let mut link = String::new();
        std::io::stdin().read_to_string(&mut link)?;
        let mut invitation = Invitation::parse(&link, None)?;
        invitation.room = None;
        if !host {
            let browser = Connection::connect(
                Destination::Directory(invitation.server.clone()),
                compatibility.clone(),
            );
            let deadline = Instant::now() + Duration::from_secs(20);
            let room = loop {
                ensure!(Instant::now() < deadline, "Hosted game was not discovered");
                match browser.poll()? {
                    Some(Event::Rooms(rooms)) => {
                        if let Some(room) = rooms.into_iter().find(|room| {
                            room.joinable(&compatibility).is_ok()
                                && args.get(3).is_none_or(|code| &room.code == code)
                        }) {
                            break room.code;
                        }
                    }
                    Some(Event::Error(error)) => return Err(error.into()),
                    _ => {}
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            invitation.room = Some(room);
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
        ..Default::default()
    };
    config.players.truncate(2);
    let mut session = network::Network::new(
        destination,
        compatibility,
        host.then_some(config),
        vec![settings::Device::Keyboard1],
    )?;
    let started = Instant::now();
    let mut ready = false;
    let mut paused = false;
    let mut last_tick = 0;
    loop {
        ensure!(
            started.elapsed() < Duration::from_secs(60),
            "Network session did not complete"
        );
        let elapsed = if session.replay.as_ref().is_some_and(|r| r.tick >= 200) {
            0
        } else {
            20
        };
        session.pump(
            elapsed,
            started.elapsed().as_millis() as u64,
            vec![PlayerInput::default()],
        )?;
        if host && session.id != 0 && !ready {
            if let Some(parent) = report.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                report.with_extension("ready.json"),
                serde_json::to_vec(
                    &serde_json::json!({"room":session.room,"addresses":bomberman_net::discovery::host_addresses()?.iter().map(ToString::to_string).collect::<Vec<_>>()}),
                )?,
            )?;
            ready = true;
        }
        if host && session.replay.is_none() && session.lobby.as_ref().is_some_and(|l| l.can_start())
        {
            session.start(1997, started.elapsed().as_millis() as u64)?;
        }
        if host && session.replay.as_ref().is_some_and(|r| r.tick >= 150) && !paused {
            session.action(Action::Pause(true))?;
            paused = true;
        }
        if let Some(replay) = &session.replay {
            last_tick = replay.tick;
            if replay.tick >= 200 {
                ensure!(replay.simulation.paused, "Pause was not synchronized");
                let hash: String = replay.hash().iter().map(|b| format!("{b:02x}")).collect();
                if let Some(parent) = report.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let value = serde_json::json!({"passed":true,"platform":std::env::consts::OS,"host":host,"transport":if online {"relay"} else {"LAN"},"tick":replay.tick,"hash":hash,"paused":replay.simulation.paused});
                std::fs::write(&report, serde_json::to_vec_pretty(&value)?)?;
                if host {
                    let finish = Instant::now();
                    while finish.elapsed() < Duration::from_secs(2) {
                        session.pump(
                            0,
                            started.elapsed().as_millis() as u64,
                            vec![PlayerInput::default()],
                        )?;
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                println!("{value}");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(if host || last_tick == 0 {
            20
        } else {
            5
        }));
    }
}
