//! Original Gold Bomberman twinkle pool (0x420d4e / 0x420e39).
//! Emission consumes the same Watcom RNG as gameplay, even without rendering.
use super::*;

pub const GOLD_PARTICLE_COUNT: usize = 100;
pub const GOLD_PARTICLE_STATES: u32 = 13;
pub const GOLD_TWINKLE_SECONDS: i32 = 5; // VALUELST.RES #1010; zero is indefinite.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GoldParticle {
    pub active: bool,
    pub position: Position,
    pub frame: u32,
    pub render_frame: u32,
    pub render_visible: bool,
}

impl GoldParticle {
    /// Native 0x420d4e: inspect the first free slot, apply the elapsed-second
    /// limit, then one 5/6 probability draw and two integer offset draws.
    pub fn emit(
        particles: &mut [Self],
        position: Position,
        elapsed_seconds: i32,
        duration_seconds: i32,
        rng: &mut DeterministicRng,
    ) {
        let Some(slot) = particles
            .iter_mut()
            .take(GOLD_PARTICLE_COUNT)
            .find(|p| !p.active)
        else {
            return;
        };
        if duration_seconds != 0 && elapsed_seconds >= duration_seconds {
            return;
        }
        if rng.range(6) == 0 {
            return;
        }
        *slot = Self {
            active: true,
            position: Position {
                x: position.x + (rng.range(40) as i32 - 20) * 100,
                y: position.y + (rng.range(50) as i32 - 48) * 100,
            },
            ..Self::default()
        };
    }

    /// Native 0x420e39: queue the old frame, advance once per global update,
    /// then expire strictly above the sequence length. Frame 13 wraps to 0 in
    /// the source lookup, so the original effect has fourteen queued frames.
    pub fn advance_all(particles: &mut [Self], frame: u64, last_frame: &mut u64) {
        for particle in particles.iter_mut().take(GOLD_PARTICLE_COUNT) {
            particle.render_visible = particle.active;
            if particle.active {
                particle.render_frame = particle.frame;
                if *last_frame != frame {
                    particle.frame = particle.frame.wrapping_add(1);
                    if particle.frame > GOLD_PARTICLE_STATES {
                        particle.active = false;
                    }
                }
            }
        }
        *last_frame = frame;
    }
}

impl Simulation {
    pub(super) fn gold_player_update(&mut self, player: usize) {
        if !self.config.gold_bomberman {
            return;
        }
        let Some(reward) = &self.gold_reward else {
            return;
        };
        let Some(actor) = self.players.get(player) else {
            return;
        };
        if !actor.alive && actor.presentation.death_finished {
            return;
        }
        let eligible = if self.config.team_play {
            self.players
                .get(reward.player)
                .is_some_and(|winner| winner.config.team == actor.config.team)
        } else {
            player == reward.player
        };
        if !eligible {
            return;
        }
        let elapsed_seconds = self.round_elapsed_seconds().min(i32::MAX as u32) as i32;
        GoldParticle::emit(
            &mut self.gold_particles,
            actor.position,
            elapsed_seconds,
            GOLD_TWINKLE_SECONDS,
            &mut self.rng,
        );
    }

    pub(super) fn gold_particle_update(&mut self) {
        GoldParticle::advance_all(
            &mut self.gold_particles,
            self.clock.frames,
            &mut self.gold_last_frame,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pool_and_elapsed_limit_consume_no_randomness() {
        let mut rng = DeterministicRng::new(1997);
        let seed = rng.state;
        let mut particles = vec![
            GoldParticle {
                active: true,
                ..GoldParticle::default()
            };
            100
        ];
        GoldParticle::emit(&mut particles, Position::default(), 0, 5, &mut rng);
        assert_eq!(rng.state, seed);
        particles[7].active = false;
        GoldParticle::emit(&mut particles, Position::default(), 5, 5, &mut rng);
        assert_eq!(rng.state, seed);
    }

    #[test]
    fn effect_queues_the_last_wrapped_frame_then_reuses_the_slot() {
        let mut particles = vec![GoldParticle {
            active: true,
            ..GoldParticle::default()
        }];
        let mut last = 0;
        for frame in 1..=14 {
            GoldParticle::advance_all(&mut particles, frame, &mut last);
            assert!(particles[0].render_visible);
            assert_eq!(particles[0].render_frame, (frame - 1) as u32);
        }
        assert!(!particles[0].active);
        GoldParticle::advance_all(&mut particles, 15, &mut last);
        assert!(!particles[0].render_visible);
    }

    #[test]
    fn repeated_particle_pass_in_the_same_native_frame_does_not_advance() {
        let mut particles = vec![GoldParticle {
            active: true,
            ..GoldParticle::default()
        }];
        let mut last = 30;
        GoldParticle::advance_all(&mut particles, 30, &mut last);
        assert_eq!(particles[0].frame, 0);
        GoldParticle::advance_all(&mut particles, 31, &mut last);
        assert_eq!(particles[0].frame, 1);
        GoldParticle::advance_all(&mut particles, 31, &mut last);
        assert_eq!(particles[0].frame, 1);
    }

    #[test]
    fn simulation_uses_frozen_hud_seconds_and_keeps_dying_gold_players_eligible() {
        let mut sim = Simulation::new(GameConfig::default(), 1997).unwrap();
        sim.gold_reward = Some(GoldReward {
            player: 0,
            powerup: Powerup::Bomb,
        });
        let initial = sim.config.round_time_ms.unwrap();
        // The original elapsed HUD value rounds up the partial remaining second.
        sim.remaining_ms = Some(initial - 4000);
        sim.rng = DeterministicRng::new(1);
        sim.players[0].alive = false;
        sim.gold_player_update(0);
        assert!(sim.gold_particles[0].active);
        sim.gold_particles.fill(GoldParticle::default());
        sim.rng = DeterministicRng::new(1);
        sim.remaining_ms = Some(initial - 4001);
        sim.gold_player_update(0);
        assert_eq!(sim.rng.state, 1);
        sim.remaining_ms = Some(initial);
        sim.players[0].presentation.death_finished = true;
        sim.gold_player_update(0);
        assert_eq!(sim.rng.state, 1);
    }
}
