//! Shared Linux/macOS fixture. CI compares its digest across architectures.
use bomberman_core::{GameConfig, PlayerInput, Simulation};
use bomberman_net::{session::Replay, Action};
fn main() {
    let mut replay = Replay::new(Simulation::new(GameConfig::default(), 1997).unwrap());
    for tick in 0..1000 {
        let actions = match tick {
            20 => vec![Action::Pause(true)],
            30 => vec![Action::Pause(false)],
            _ => vec![],
        };
        replay
            .advance(
                vec![
                    PlayerInput {
                        bomb: tick % 40 == 0,
                        ..Default::default()
                    };
                    10
                ],
                actions,
            )
            .unwrap();
    }
    for byte in replay.hash() {
        print!("{byte:02x}");
    }
    println!();
}
