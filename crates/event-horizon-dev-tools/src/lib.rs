//! Event Horizon's developer tools as one built-in native mod
//! (docs/dev-tools.md): the egui windows drawn over the world
//! ([`dev_tools`]), their server half ([`inspect`], fed through the
//! shared `event_horizon_observe` store: the engine's two packet feeds and
//! the commands talk to it, never to this crate directly) and their
//! commands ([`commands`], registered in the server's tree as a mod's).
#![forbid(unsafe_code)]

pub mod commands;
pub mod dev_tools;
mod inspect;

pub use dev_tools::mods;
pub use inspect::world_closed;
