//! Development-only counterpart to tools/compare_roulette.py.
use bomberman_core::{DeterministicRng, RoulettePhase, RouletteWheel};

fn add(hash: &mut u64, value: i32) {
    for byte in value.to_le_bytes() {
        *hash = (*hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
}
fn state(hash: &mut u64, wheel: &RouletteWheel) {
    for value in [
        wheel.direction,
        wheel.wheel_angle,
        wheel.pointer_angle,
        wheel.wheel_speed,
        wheel.pointer_speed,
        match wheel.phase {
            RoulettePhase::Spinning => 0,
            RoulettePhase::Stopping => 1,
            RoulettePhase::Settled => 2,
        },
        wheel.reward().map_or(-1, |p| i32::from(p.id())),
    ] {
        add(hash, value);
    }
}
fn main() {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    let mut frame_count = 0;
    for seed in 0..128 {
        let delay = [0, 1, 17, 100][seed as usize % 4];
        let mut rng = DeterministicRng::new(seed);
        let mut wheel = RouletteWheel::new(&mut rng);
        add(&mut hash, seed as i32);
        add(&mut hash, delay);
        add(&mut hash, rng.state as i32);
        state(&mut hash, &wheel);
        for _ in 0..delay {
            let frame = wheel.step_frame();
            state(&mut hash, &wheel);
            add(&mut hash, frame.pointer_ticks as i32);
            frame_count += 1;
        }
        wheel.stop();
        for _ in 0..2000 {
            let frame = wheel.step_frame();
            state(&mut hash, &wheel);
            add(&mut hash, frame.pointer_ticks as i32);
            frame_count += 1;
            if wheel.is_settled() {
                break;
            }
        }
        assert!(wheel.is_settled());
    }
    println!(
        "{}",
        serde_json::json!({"fixtures":128,"frames":frame_count,"fingerprint":format!("{hash:#x}")})
    );
}
