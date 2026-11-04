//! Real TLS/QUIC clients and production session simulation, without media.
#[allow(dead_code, unused_imports)]
#[path = "../src/network.rs"]
mod network;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;

use anyhow::{ensure, Context, Result};
use bomberman_core::{Direction, GameConfig, PlayerInput, TilePos};
use bomberman_net::{
    transport::Destination,
    verification::{position, Agreement, AuditSample, FailureTrace, RenderGuard, TimingHistogram},
    Action, Compatibility, GameMessage, Invitation, STEP_MS,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
    io::Read,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

struct Room {
    clients: Vec<network::Session>,
    agreement: AgreementWorker,
    requested_start: bool,
    render_guards: Vec<RenderGuard>,
    rematches: u64,
}

enum AuditCommand {
    Sample(usize, Box<AuditSample>),
    Measure(bool),
}
#[derive(Default)]
struct AuditProgress {
    processed: u64,
    compared: u64,
    pending: usize,
    failure: Option<String>,
}

/// Hashing and comparing delayed snapshots must not stop unrelated clients
/// from draining their gameplay queues. Every sample still crosses the same
/// strict Agreement oracle, with a bounded queue and an explicit final drain.
struct AgreementWorker {
    sender: Option<mpsc::SyncSender<AuditCommand>>,
    worker: Option<thread::JoinHandle<Agreement>>,
    progress: Arc<Mutex<AuditProgress>>,
    submitted: u64,
    completed: Option<Agreement>,
}
impl AgreementWorker {
    fn new(players: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(512);
        let progress = Arc::new(Mutex::new(AuditProgress::default()));
        let status = progress.clone();
        let worker = thread::spawn(move || {
            let mut agreement = Agreement::new(players);
            let mut failure = None;
            while let Ok(command) = receiver.recv() {
                match command {
                    AuditCommand::Sample(peer, sample) => {
                        if failure.is_none() {
                            failure = agreement.record(peer, *sample).err();
                        }
                        let mut progress = status.lock().unwrap();
                        progress.processed += 1;
                        progress.compared = agreement.compared_ticks;
                        progress.pending = agreement.pending_ticks();
                        progress.failure.clone_from(&failure);
                    }
                    AuditCommand::Measure(true) => agreement.start_measurement(),
                    AuditCommand::Measure(false) => agreement.stop_measurement(),
                }
            }
            agreement
        });
        Self {
            sender: Some(sender),
            worker: Some(worker),
            progress,
            submitted: 0,
            completed: None,
        }
    }
    fn send(&self, command: AuditCommand) -> std::result::Result<(), String> {
        self.sender
            .as_ref()
            .ok_or("audit worker already finished")?
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "audit worker queue exhausted",
                mpsc::TrySendError::Disconnected(_) => "audit worker disconnected",
            })
            .map_err(str::to_owned)
    }
    fn record(&mut self, peer: usize, sample: AuditSample) -> std::result::Result<(), String> {
        if let Some(failure) = &self.progress.lock().unwrap().failure {
            return Err(failure.clone());
        }
        self.send(AuditCommand::Sample(peer, Box::new(sample)))?;
        self.submitted += 1;
        Ok(())
    }
    fn compared_ticks(&self) -> u64 {
        self.progress.lock().unwrap().compared
    }
    fn pending_ticks(&self) -> usize {
        let progress = self.progress.lock().unwrap();
        progress.pending + self.submitted.saturating_sub(progress.processed) as usize
    }
    fn measure(&self, enabled: bool) -> Result<()> {
        self.send(AuditCommand::Measure(enabled))
            .map_err(anyhow::Error::msg)
    }
    fn finish(&mut self) -> Result<Option<String>> {
        self.sender.take();
        self.completed = Some(
            self.worker
                .take()
                .context("audit worker already finished")?
                .join()
                .map_err(|_| anyhow::anyhow!("audit worker panicked"))?,
        );
        let progress = self.progress.lock().unwrap();
        Ok(progress.failure.clone().or_else(|| {
            (progress.pending != 0).then(|| {
                format!(
                    "audit final drain has {} missing committed ticks",
                    progress.pending
                )
            })
        }))
    }
}

/// Optional diagnostics for the load driver itself. A late loop can be caused
/// by simulation/auditing work or by sleeping too long; keep those separate.
/// This does not change the workload, transport, or correctness gates.
struct DriverProfile {
    path: PathBuf,
    work: TimingHistogram,
    sleep_overshoot: TimingHistogram,
    stages_us: [u64; 4],
    peak_stages_us: [u64; 4],
    peers: Vec<serde_json::Value>,
    slow: VecDeque<serde_json::Value>,
    evictions: BTreeMap<(usize, usize), u64>,
    failure: Option<String>,
}
impl DriverProfile {
    fn new(report: &std::path::Path) -> Self {
        Self {
            path: report.with_extension("driver.json"),
            work: TimingHistogram::default(),
            sleep_overshoot: TimingHistogram::default(),
            stages_us: [0; 4],
            peak_stages_us: [0; 4],
            peers: Vec::new(),
            slow: VecDeque::new(),
            evictions: BTreeMap::new(),
            failure: None,
        }
    }
    fn peer(&mut self, room: usize, peer: usize, stages_us: [u64; 4], evictions: u64) {
        for (i, value) in stages_us.iter().enumerate() {
            self.stages_us[i] += value;
            self.peak_stages_us[i] = self.peak_stages_us[i].max(*value);
        }
        let previous = self.evictions.insert((room, peer), evictions).unwrap_or(0);
        if stages_us.iter().sum::<u64>() >= 10_000 || evictions > previous {
            self.peers.push(json!({"room":room,"peer":peer,"stages_us":stages_us,"queue_evictions":evictions,"new_evictions":evictions.saturating_sub(previous)}));
        }
    }
    fn iteration(&mut self, now: u64, work: Duration, requested: Duration, actual: Duration) {
        let overshoot = actual.saturating_sub(requested);
        self.work.record(work.as_millis() as u64);
        self.sleep_overshoot.record(overshoot.as_millis() as u64);
        if work >= Duration::from_millis(20)
            || overshoot >= Duration::from_millis(20)
            || !self.peers.is_empty()
        {
            if self.slow.len() == 128 {
                self.slow.pop_front();
            }
            self.slow.push_back(json!({"at_ms":now,"work_us":work.as_micros(),"requested_sleep_us":requested.as_micros(),"actual_sleep_us":actual.as_micros(),"peers":std::mem::take(&mut self.peers)}));
        }
    }
}
impl Drop for DriverProfile {
    fn drop(&mut self) {
        let result = json!({"stage_order":["pump","prepared_and_confirmations","audit_submission","render_oracle_and_trace"],"stage_total_us":self.stages_us,"stage_max_us":self.peak_stages_us,"loop_work_ms":self.work.summary(),"sleep_overshoot_ms":self.sleep_overshoot.summary(),"slow_iterations":self.slow,"unfinished_iteration_peers":self.peers,"failure":self.failure,"scope":"whole driver lifetime including startup and warmup; bounded to the last 128 slow iterations; instrumentation enabled explicitly"});
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&result) {
            let _ = std::fs::write(&self.path, bytes);
        }
    }
}

fn config(players: usize) -> GameConfig {
    let mut config = GameConfig {
        countdown_ms: 0,
        random_starts: false,
        enclosement_depth: 0,
        ..Default::default()
    };
    config.players.truncate(players);
    config.scheme.brick_density = 0;
    config.scheme.rows.fill("#.............#".into());
    config.scheme.rows[0] = "###############".into();
    config.scheme.rows[10] = "###############".into();
    for (start, (x, y)) in config.scheme.starts.iter_mut().zip([
        (2, 2),
        (12, 8),
        (12, 2),
        (2, 8),
        (6, 2),
        (8, 8),
        (2, 5),
        (12, 5),
        (6, 5),
        (8, 5),
    ]) {
        start.tile = TilePos::new(x, y);
    }
    for power in &mut config.scheme.powerups {
        power.born_with = match power.kind {
            bomberman_core::Powerup::Bomb => 3,
            bomberman_core::Powerup::Trigger => 1,
            _ => power.born_with,
        };
        power.override_count = Some(0);
    }
    config
}

fn input(tick: u64, seat: usize, seed: u64) -> PlayerInput {
    PlayerInput {
        direction: match (tick / 35 + seat as u64 + seed) % 5 {
            0 => Some(Direction::Right),
            1 => Some(Direction::Down),
            2 => Some(Direction::Left),
            3 => Some(Direction::Up),
            _ => None,
        },
        bomb: (tick + seat as u64 * 7 + seed) % 137 == 23,
        action: (tick + seat as u64 * 11 + seed) % 137 == 79,
        ..Default::default()
    }
}

fn main() {
    if let Err(error) = run() {
        // NetError is a fixed enum; never emit invitations or destinations.
        eprintln!("Gameplay load generator: {error:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let arg = |key: &str, default: &str| {
        args.windows(2)
            .find(|p| p[0] == key)
            .map(|p| p[1].clone())
            .unwrap_or_else(|| default.to_owned())
    };
    let players: usize = arg("--players", "2").parse()?;
    let count: usize = arg("--rooms", "1").parse()?;
    let warmup: u64 = arg("--warmup", "10").parse()?;
    let duration: u64 = arg("--duration", "60").parse()?;
    let seed: u64 = arg("--seed", "1997").parse()?;
    let report = PathBuf::from(arg("--report", "build/relay-load.json"));
    let mut profile = args
        .iter()
        .any(|arg| arg == "--profile-driver")
        .then(|| DriverProfile::new(&report));
    ensure!(
        [2, 10].contains(&players) && (1..=4).contains(&count) && duration > 0,
        "invalid load dimensions"
    );
    let mut link = String::new();
    std::io::stdin().read_to_string(&mut link)?;
    let invitation = Invitation::parse(link.trim(), None)
        .map_err(|_| anyhow::anyhow!("invalid private link on stdin"))?;
    let compatibility = Compatibility::new("a".repeat(64));
    let mut rooms = Vec::new();
    for _ in 0..count {
        let mut invite = invitation.clone();
        invite.room = None;
        let mut host = network::Session::new(
            Destination::Online(invite),
            compatibility.clone(),
            Some(config(players)),
            vec![settings::Device::Keyboard1],
        )?;
        host.enable_audit();
        rooms.push(Room {
            clients: vec![host],
            agreement: AgreementWorker::new(players),
            requested_start: false,
            rematches: 0,
            render_guards: (0..players).map(|_| RenderGuard::default()).collect(),
        });
    }
    let started = Instant::now();
    let mut previous_ms = 0;
    let mut playing_at = None;
    let mut measured_at = None;
    let mut initial_traffic = None;
    let mut trace = FailureTrace::new(
        seed,
        json!({"players":players,"rooms":count,"warmup_seconds":warmup,"measurement_seconds":duration,"schedule":"input(tick, seat, seed) in relay_load.rs"}),
    );
    let mut first_failure = None;
    let mut delays = TimingHistogram::default();
    let mut rtts = TimingHistogram::default();
    let mut input_confirmation = TimingHistogram::default();
    let mut measured_compared_at = 0;
    let mut outcomes = BTreeMap::<&str, u64>::new();
    let mut frames_sampled = 0u64;
    let mut repair_bucket = 0;
    let mut final_tick = None;
    let mut pause_requested = false;
    let mut drain_at = 0;
    let mut measurement_unix_ms = 0u64;
    let mut measurement_end_unix_ms = 0u64;
    let mut measurement_end_traffic = None;
    let mut last_drain_repeat = 0;
    loop {
        let iteration = Instant::now();
        let now = started.elapsed().as_millis() as u64;
        if now >= (warmup + duration + 45) * 1000 {
            trace.fail("load timed out");
            trace.save(&report.with_extension("failure.json"))?;
            anyhow::bail!("load timed out");
        }
        let elapsed = now.saturating_sub(previous_ms).min(250) as u32;
        if measured_at.is_some() && final_tick.is_none() {
            delays.record(now.saturating_sub(previous_ms).saturating_sub(5));
        }
        previous_ms = now;
        for (room_index, room) in rooms.iter_mut().enumerate() {
            if room.clients.len() < players && room.clients[0].id != 0 {
                let mut invite = invitation.clone();
                invite.room = Some(room.clients[0].room.clone());
                let mut client = network::Session::new(
                    Destination::Online(invite),
                    compatibility.clone(),
                    None,
                    vec![settings::Device::Keyboard1],
                )?;
                client.enable_audit();
                room.clients.push(client);
            }
            let frozen = final_tick.is_some()
                && room.clients[0]
                    .replay
                    .as_ref()
                    .is_some_and(|r| r.simulation.paused);
            for (peer, client) in room.clients.iter_mut().enumerate() {
                let peer_started = profile.as_ref().map(|_| Instant::now());
                let controls = if final_tick.is_some() {
                    PlayerInput::default()
                } else {
                    input(now / u64::from(STEP_MS), peer, seed)
                };
                if let Err(error) =
                    client.pump(if frozen { 0 } else { elapsed }, now, vec![controls])
                {
                    let reason = format!("room {room_index} peer {peer} at {now}ms: {error}");
                    if let Some(profile) = &mut profile {
                        profile.failure = Some(reason.clone());
                    }
                    trace.fail(reason.clone());
                    trace.save(&report.with_extension("failure.json"))?;
                    anyhow::bail!("{reason}");
                }
                let pumped = profile.as_ref().map(|_| Instant::now());
                client.prepared(now)?;
                for latency in client.take_confirmation_latencies() {
                    if measured_at.is_some() && final_tick.is_none() {
                        input_confirmation.record(latency);
                    }
                }
                let prepared = profile.as_ref().map(|_| Instant::now());
                for sample in client.take_audit() {
                    if peer == 0 {
                        for event in &sample.events {
                            let kind = match event {
                                bomberman_core::GameEvent::BombPlaced { .. } => "bomb_placements",
                                bomberman_core::GameEvent::BombExploded { .. } => "detonations",
                                bomberman_core::GameEvent::PowerupCollected { .. } => "pickups",
                                bomberman_core::GameEvent::PlayerDied { .. } => "deaths",
                                bomberman_core::GameEvent::RoundEnded { .. } => "round_results",
                                bomberman_core::GameEvent::MatchEnded { .. } => "match_results",
                                _ => continue,
                            };
                            *outcomes.entry(kind).or_default() += 1;
                        }
                    }
                    if first_failure.is_none() {
                        if let Err(error) = room.agreement.record(peer, sample) {
                            first_failure = Some(error);
                        }
                    }
                }
                let audited = profile.as_ref().map(|_| Instant::now());
                if let Some(display) = &client.predicted {
                    frames_sampled += 1;
                    let owned = client
                        .lobby
                        .as_ref()
                        .context("missing lobby")?
                        .owned_seats(client.id);
                    for seat in owned {
                        let p = &display.players[seat];
                        if let Some((predicted, reconciliation)) = client.prediction_audit(seat) {
                            if first_failure.is_none() {
                                if let Err(error) = room.render_guards[peer].observe(
                                    client.connection.gameplay_diagnostics().generation,
                                    predicted,
                                    display,
                                    seat,
                                    controls,
                                    reconciliation,
                                ) {
                                    first_failure = Some(format!(
                                        "room={room_index} peer={peer} at={now}ms: {error}"
                                    ));
                                    trace.push(json!({"failure_at_ms":now,"room":room_index,"peer":peer,"seat":seat,"arena":display.config.arena,"reconciliation":reconciliation,"input":controls,"predicted_input":predicted.inputs[seat],"predicted_tick":predicted.tick,"canonical_tick":client.replay.as_ref().map(|r|r.tick),"player":p,"canonical_player":client.replay.as_ref().map(|r|&r.simulation.players[seat])}));
                                }
                            }
                        }
                        if trace.first_invariant.is_none() {
                            trace.push(json!({"at_ms":now,"room":room_index,"peer":peer,"tick":client.replay.as_ref().map(|r|r.tick),"predicted_tick":client.prediction_audit(seat).map(|(r,_)|r.tick),"generation":client.connection.gameplay_diagnostics().generation,"round":display.round,"phase":display.phase,"paused":display.paused,"input":controls,"control":p.config.control,"owner":position(p.position),"canonical":client.replay.as_ref().map(|r| position(r.simulation.players[seat].position)),"render":position(p.presentation.render_position)}));
                            if let Some(error) = &first_failure {
                                trace.fail(error.clone());
                            }
                        }
                    }
                }
                if measured_at.is_some() && final_tick.is_none() {
                    if let Some(rtt) = client.gameplay_rtt_ms {
                        rtts.record(rtt);
                    }
                }
                if let Some(profile) = &mut profile {
                    let points = [
                        peer_started.unwrap(),
                        pumped.unwrap(),
                        prepared.unwrap(),
                        audited.unwrap(),
                        Instant::now(),
                    ];
                    let stages_us = std::array::from_fn(|i| {
                        points[i + 1].duration_since(points[i]).as_micros() as u64
                    });
                    profile.peer(
                        room_index,
                        peer,
                        stages_us,
                        client
                            .connection
                            .gameplay_diagnostics()
                            .traffic
                            .queue_evictions,
                    );
                }
            }
            if final_tick.is_none()
                && room.agreement.pending_ticks() == 0
                && room.clients.len() == players
                && room.clients.iter().all(|c| {
                    !c.is_starting()
                        && c.replay
                            .as_ref()
                            .is_some_and(|r| r.stage == bomberman_net::Stage::Results)
                })
            {
                room.clients[0].action(Action::Rematch)?;
                room.rematches += 1;
            }
            if !room.requested_start
                && room.clients.len() == players
                && room.clients[0]
                    .lobby
                    .as_ref()
                    .is_some_and(|l| l.peers.len() == players && l.can_start())
            {
                room.clients[0].start(seed + room_index as u64, now)?;
                room.requested_start = true;
            }
        }
        let all_playing = rooms.iter().all(|r| {
            r.clients.len() == players
                && r.clients
                    .iter()
                    .all(|c| c.replay.is_some() && !c.is_starting())
        });
        if all_playing && playing_at.is_none() {
            playing_at = Some(now);
        }
        if playing_at.is_some_and(|at| now >= at + warmup * 1000) && measured_at.is_none() {
            measured_at = Some(now);
            measured_compared_at = rooms
                .iter()
                .map(|r| r.agreement.compared_ticks())
                .sum::<u64>();
            for room in &mut rooms {
                room.agreement.measure(true)?;
            }
            measurement_unix_ms = bomberman_net::diagnostics::unix_ms();
            if let Some(parent) = report.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                report.with_extension("progress.json.tmp"),
                serde_json::to_vec(
                    &json!({"measurement_start_unix_ms":measurement_unix_ms,"measurement_end_unix_ms":measurement_unix_ms+duration*1000}),
                )?,
            )?;
            std::fs::rename(
                report.with_extension("progress.json.tmp"),
                report.with_extension("progress.json"),
            )?;
            initial_traffic = Some(
                rooms
                    .iter()
                    .flat_map(|r| {
                        r.clients
                            .iter()
                            .map(|c| c.connection.gameplay_diagnostics())
                    })
                    .collect::<Vec<_>>(),
            );
        }
        // Exercise requested reliable repair alongside ordinary probes, frames,
        // and supersedable motion. These are real TLS control requests.
        if all_playing && now / 5000 > repair_bucket {
            repair_bucket = now / 5000;
            for room in &rooms {
                for client in room.clients.iter().skip(1) {
                    let tick = client.replay.as_ref().unwrap().tick;
                    if tick > 2 {
                        let generation = client.connection.gameplay_diagnostics().generation;
                        client.connection.message(
                            Some(bomberman_net::HOST),
                            GameMessage::FrameRange {
                                generation,
                                first: tick - 1,
                                last: tick,
                            },
                        )?;
                    }
                }
            }
        }
        if measured_at.is_some_and(|at| now >= at + duration * 1000) && final_tick.is_none() {
            // Freeze only the host simulation clock. Continue real networking
            // until every final committed tick is observed by every client.
            final_tick = Some(
                rooms
                    .iter()
                    .map(|r| r.clients[0].replay.as_ref().unwrap().tick)
                    .collect::<Vec<_>>(),
            );
            measurement_end_traffic = Some(
                rooms
                    .iter()
                    .flat_map(|r| {
                        r.clients
                            .iter()
                            .map(|c| c.connection.gameplay_diagnostics())
                    })
                    .collect::<Vec<_>>(),
            );
            drain_at = now;
            for room in &mut rooms {
                room.agreement.measure(false)?;
            }
            measurement_end_unix_ms = bomberman_net::diagnostics::unix_ms();
        }
        if final_tick.is_some() {
            if now.saturating_sub(last_drain_repeat) >= 20 {
                last_drain_repeat = now;
                for room in &rooms {
                    room.clients[0].retransmit_committed()?;
                }
            }
            // A stable pause lets all ledgers finish without dropping a tail.
            if rooms
                .iter()
                .all(|r| r.clients[0].replay.as_ref().unwrap().simulation.paused)
                && (rooms.iter().all(|r| r.agreement.pending_ticks() == 0)
                    || (first_failure.is_some() && now > drain_at + 1000))
            {
                break;
            }
            if !pause_requested && rooms.iter().all(|room| !room.clients[0].is_starting()) {
                for room in &mut rooms {
                    room.clients[0].action(Action::Pause(true))?;
                }
                pause_requested = true;
            }
        }
        let work = iteration.elapsed();
        let wait = Duration::from_millis(5).checked_sub(work);
        let sleeping = profile.as_ref().map(|_| Instant::now());
        if let Some(wait) = wait {
            thread::sleep(wait);
        }
        if let Some(profile) = &mut profile {
            profile.iteration(
                now,
                work,
                wait.unwrap_or_default(),
                sleeping.unwrap().elapsed(),
            );
        }
    }
    for room in &mut rooms {
        if let Some(failure) = room.agreement.finish()? {
            first_failure.get_or_insert(failure);
        }
    }
    if let Some(profile) = &mut profile {
        profile.failure.clone_from(&first_failure);
    }
    let lifetime_traffic = rooms
        .iter()
        .flat_map(|r| {
            r.clients
                .iter()
                .map(|c| c.connection.gameplay_diagnostics())
        })
        .collect::<Vec<_>>();
    let final_traffic = measurement_end_traffic.context("measurement did not finish")?;
    let initial_traffic = initial_traffic.context("measurement never began")?;
    let sum = |f: fn(&bomberman_net::diagnostics::TrafficCounters) -> u64| {
        final_traffic
            .iter()
            .zip(&initial_traffic)
            .map(|(a, b)| f(&a.traffic).saturating_sub(f(&b.traffic)))
            .sum::<u64>()
    };
    let evictions = sum(|t| t.queue_evictions);
    let failures = sum(|t| t.send_failures + t.queue_rejections);
    let lifetime_failures = lifetime_traffic
        .iter()
        .map(|d| d.traffic.queue_evictions + d.traffic.queue_rejections + d.traffic.send_failures)
        .sum::<u64>();
    let passed =
        first_failure.is_none() && evictions == 0 && failures == 0 && lifetime_failures == 0;
    let failure_path = report.with_extension("failure.json");
    if let Some(error) = &first_failure {
        trace.fail(error.clone());
        trace.save(&failure_path)?;
    }
    let mut commit_to_apply = TimingHistogram::default();
    for room in &rooms {
        commit_to_apply.merge(&room.agreement.completed.as_ref().unwrap().confirmation_ms);
    }
    let result = json!({"passed":passed,"version":bomberman_net::APP_VERSION,"protocol":bomberman_net::PROTOCOL_VERSION,"players_per_room":players,"rooms":count,"warmup_seconds":warmup,"measurement_seconds":duration,"seed":seed,"measurement_start_unix_ms":measurement_unix_ms,"measurement_end_unix_ms":measurement_end_unix_ms,
        "audit_execution":"bounded background worker; all samples drained before final result",
        "canonical_ticks_compared":rooms.iter().map(|r|r.agreement.compared_ticks()).sum::<u64>(),"sampled_render_frames":frames_sampled,
        "render_frames_checked":rooms.iter().flat_map(|r|r.render_guards.iter()).map(|g|g.frames).sum::<u64>(),
        "explained_render_boundaries":rooms.iter().flat_map(|r|r.render_guards.iter()).map(|g|g.explained_boundaries).sum::<u64>(),
        "first_failure":first_failure,"failure_trace":if passed {None} else {Some(failure_path.file_name().unwrap().to_string_lossy())},
        "application_rtt_ms":rtts.summary(),"host_commit_to_client_apply_ms":commit_to_apply.summary(),"owner_sample_to_confirmation_ms":input_confirmation.summary(),
        "latency_distributions":{"application_rtt_ms":rtts.distribution(),"host_commit_to_client_apply_ms":commit_to_apply.distribution(),"owner_sample_to_confirmation_ms":input_confirmation.distribution(),"generator_scheduling_delay_ms":delays.distribution()},
        "achieved_committed_ticks_per_second":rooms.iter().map(|r|r.agreement.compared_ticks()).sum::<u64>().saturating_sub(measured_compared_at) as f64 / duration as f64,"rematches":rooms.iter().map(|r|r.rematches).sum::<u64>(),"shared_outcomes":outcomes,
        "generator_scheduling_delay_ms":delays.summary(),"received_packets_per_second":sum(|t|t.received_packets) as f64 / duration as f64,"sent_packets_per_second":sum(|t|t.sent_packets) as f64 / duration as f64,
        "lifetime_application_failures":lifetime_failures,"queue_evictions":evictions,"send_failures_and_queue_rejections":failures,"intentional_supersessions":sum(|t|t.superseded_packets),"client_diagnostics":final_traffic,
        "limitations":["Headless production session display state; physical display and audio are outside this measurement.","Latency histograms cover the full measurement at 1ms resolution; RTT samples are cached application-probe observations. Forwarding uses separate relay timestamps."]});
    if let Some(parent) = report.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&report, serde_json::to_vec_pretty(&result)?)?;
    println!(
        "{}",
        json!({"passed":passed,"protocol":bomberman_net::PROTOCOL_VERSION,"canonical_ticks_compared":result["canonical_ticks_compared"]})
    );
    ensure!(
        passed,
        "gameplay correctness/load gate failed (see report and bounded failure trace)"
    );
    Ok(())
}

#[cfg(test)]
mod driver_tests {
    use super::*;
    use bomberman_core::Simulation;
    use bomberman_net::session::Replay;

    fn samples() -> Vec<AuditSample> {
        let mut replay = Replay::new(Simulation::new(config(2), 1997).unwrap());
        (0..2)
            .map(|_| {
                let (frame, events) = replay
                    .advance(vec![PlayerInput::default(); 2], vec![])
                    .unwrap();
                AuditSample {
                    generation: 1,
                    replay: replay.clone(),
                    frame,
                    events,
                    observed_ms: replay.tick * u64::from(STEP_MS),
                }
            })
            .collect()
    }

    #[test]
    fn audit_worker_compares_every_tick_with_delayed_and_reordered_peers() {
        let samples = samples();
        let mut worker = AgreementWorker::new(2);
        worker.measure(true).unwrap();
        for sample in &samples {
            worker.record(0, sample.clone()).unwrap();
        }
        for sample in samples.iter().rev() {
            worker.record(1, sample.clone()).unwrap();
        }
        worker.measure(false).unwrap();
        assert_eq!(worker.finish().unwrap(), None);
        assert_eq!(worker.compared_ticks(), 2);
        assert_eq!(worker.pending_ticks(), 0);
        assert_eq!(
            worker.completed.as_ref().unwrap().confirmation_ms.summary()["samples"],
            2
        );
    }

    #[test]
    fn audit_worker_reports_a_late_state_mismatch_during_the_final_drain() {
        let samples = samples();
        let mut worker = AgreementWorker::new(2);
        worker.record(0, samples[0].clone()).unwrap();
        worker.record(1, samples[0].clone()).unwrap();
        worker.record(0, samples[1].clone()).unwrap();
        let mut broken = samples[1].clone();
        broken.replay.simulation.players[0].kills += 1;
        worker.record(1, broken).unwrap();
        let failure = worker.finish().unwrap().unwrap();
        assert!(failure.contains("players[0].kills"), "{failure}");
        assert_eq!(worker.compared_ticks(), 1);
    }

    #[test]
    fn audit_worker_never_passes_with_a_missing_peer_tick() {
        let samples = samples();
        let mut worker = AgreementWorker::new(2);
        worker.record(0, samples[0].clone()).unwrap();
        let failure = worker.finish().unwrap().unwrap();
        assert!(failure.contains("missing committed ticks"), "{failure}");
        assert_eq!(worker.compared_ticks(), 0);
        assert_eq!(worker.pending_ticks(), 1);
    }
}
