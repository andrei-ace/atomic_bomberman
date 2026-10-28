//! Wire protocol, private invitations, bounded transport and deterministic replay.
//! This crate never initializes graphics, audio or input devices.
pub mod broker;
pub mod discovery;
pub mod invitation;
pub mod protocol;
pub mod session;
pub mod transport;
pub use invitation::{Invitation, ServerProfile};
pub use protocol::*;
