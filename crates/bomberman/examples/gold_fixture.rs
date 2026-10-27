//! JSON-lines development bridge for the isolated original Gold sparkle probe.
use bomberman_core::{DeterministicRng, GoldParticle, Position};
use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead};

#[derive(Deserialize)]
struct Fixture {
    seed: u64,
    last_frame: u64,
    initial: Vec<[i32; 4]>,
    steps: Vec<Step>,
}
#[derive(Deserialize)]
struct Step {
    frame: u64,
    elapsed_seconds: i32,
    duration_seconds: i32,
    positions: Vec<[i32; 2]>,
}

fn main() {
    for line in io::stdin().lock().lines() {
        let fixture: Fixture =
            serde_json::from_str(&line.expect("fixture line")).expect("parse fixture");
        let mut particles: Vec<_> = fixture
            .initial
            .into_iter()
            .map(|[active, x, y, frame]| GoldParticle {
                active: active != 0,
                position: Position {
                    x: x * 100,
                    y: y * 100,
                },
                frame: frame as u32,
                ..GoldParticle::default()
            })
            .collect();
        let mut rng = DeterministicRng::new(fixture.seed);
        let mut last_frame = fixture.last_frame;
        let mut steps = Vec::new();
        for step in fixture.steps {
            for [x, y] in step.positions {
                GoldParticle::emit(
                    &mut particles,
                    Position {
                        x: x * 100,
                        y: y * 100,
                    },
                    step.elapsed_seconds,
                    step.duration_seconds,
                    &mut rng,
                );
            }
            GoldParticle::advance_all(&mut particles, step.frame, &mut last_frame);
            let pool: Vec<_> = particles
                .iter()
                .map(|p| {
                    [
                        i32::from(p.active),
                        p.position.x / 100,
                        p.position.y / 100,
                        p.frame as i32,
                    ]
                })
                .collect();
            let drawn: Vec<_> = particles
                .iter()
                .filter(|p| p.render_visible)
                .map(|p| {
                    [
                        p.position.x / 100,
                        p.position.y / 100,
                        p.render_frame as i32,
                    ]
                })
                .collect();
            steps.push(json!({"pool":pool,"drawn":drawn,"seed":rng.state,"last_frame":last_frame}));
        }
        println!("{}", json!({"steps":steps}));
    }
}
