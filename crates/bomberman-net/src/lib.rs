//! Wire protocol, private invitations, bounded transport and deterministic replay.
//! This crate never initializes graphics, audio or input devices.
pub mod broker;
pub mod discovery;
pub mod gameplay;
pub mod interfaces;
pub mod invitation;
pub mod prediction;
pub mod protocol;
pub mod quic;
pub mod session;
pub mod transport;
pub use gameplay::*;
pub use invitation::{Invitation, ServerProfile};
pub use protocol::*;
pub use quic::GameplayOffer;
