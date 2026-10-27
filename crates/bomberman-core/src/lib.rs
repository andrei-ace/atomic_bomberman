//! Headless, integer-coordinate battle simulation.
//!
//! Values documented in VALUELST.RES are preserved in their original units:
//! 20 reference frames/second, hundredths of a pixel, and a 40-frame fuse.
//! The original AI callbacks and RNG are independently compared with x86.
//! Movement and hazard integration remain a reconstruction; isolated callback
//! equivalence does not establish whole-match parity. See the project ledger.

pub mod ai;
mod clock;
mod model;
pub mod original_ai;
pub mod original_ai_adapter;
mod rng;
pub mod roulette;
mod scheme;
mod simulation;
mod sound;
mod sound_slots;

pub use clock::{FrameClock, MAX_FRAME_MS};
pub use model::*;
pub use rng::DeterministicRng;
pub use roulette::{RouletteFrame, RoulettePhase, RouletteWheel};
pub use scheme::*;
pub use simulation::{GoldParticle, PhysicsState, Simulation};
pub use sound::SoundSelector;

#[cfg(test)]
mod tests;
