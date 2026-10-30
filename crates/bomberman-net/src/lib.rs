//! Wire protocol, private invitations, bounded transport and deterministic replay.
//! This crate never initializes graphics, audio or input devices.
pub mod authority;
pub mod broker;
pub mod diagnostics;
pub mod discovery;
pub mod gameplay;
pub mod interfaces;
pub mod invitation;
mod lan_socket;
pub mod prediction;
pub mod protocol;
pub mod quic;
pub mod session;
pub mod transport;
pub mod verification;
#[cfg(windows)]
pub mod windows;
pub use gameplay::*;
pub use invitation::{Invitation, ServerProfile};
pub use protocol::*;
pub use quic::GameplayOffer;
