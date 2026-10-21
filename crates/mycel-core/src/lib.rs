//! # mycel-core
//!
//! Peer-to-peer networking core for Mycel, a chat app where one friend's
//! computer hosts the server and nobody in between can switch it off.
//!
//! * [`identity`] – a device's persistent keypair (no accounts).
//! * [`invite`] – shareable invite codes that tell members how to reach a host.
//! * [`protocol`] – the messages members and hosts exchange.
//! * [`node`] – the libp2p node: connectivity, relays, hole punching, chat.

pub mod identity;
pub mod invite;
pub mod node;
pub mod protocol;

pub use invite::Invite;
pub use node::{start, ConnectionPath, NodeConfig, NodeEvent, NodeHandle, Role};
