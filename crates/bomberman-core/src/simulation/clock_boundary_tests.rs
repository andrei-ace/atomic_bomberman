use super::*;

#[test]
fn result_dwell_counts_as_wall_time_but_not_battle_frames_or_new_round_time() {
    let mut config = GameConfig {
        countdown_ms: 1000,
        round_over_ms: 0,
        random_starts: false,
        ..Default::default()
    };
    config.players.truncate(2);
    let mut sim = Simulation::new(config, 1997).unwrap();
    sim.step(50, &[]);
    let frame = sim.clock.frames;
    sim.force_draw();
    sim.step(6000, &[]);
    assert_eq!(sim.round, 2);
    assert_eq!(sim.clock.frames, frame);
    sim.step(50, &[]);
    assert_eq!(sim.clock.frames, frame + 1);
    assert_eq!(sim.clock.elapsed_ms, 150);
    assert_eq!(sim.clock.raw_elapsed_ms, 6050);
    assert_eq!(sim.wall_elapsed_ms(), 6100);
    assert_eq!(sim.remaining_ms, Some(149_950));
    assert_eq!(sim.phase, RoundPhase::Countdown { remaining_ms: 850 });
}
