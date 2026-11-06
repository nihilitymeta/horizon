//! Event Horizon's developer tools: what the game observes of the entities it
//! watches, and how a server and a guest's client say it to each other
//! (docs/dev-tools.md). The types the windows show ([`View`],
//! [`BrainSnapshot`], [`BlockSnapshot`]), the process-wide store that keeps
//! them ([`add`], [`snapshots`]), the bounded name interner
//! ([`received_name`]) and the wire codec ([`wire`]). Nothing here changes an
//! entity: the game fills the store ([`observe_entity`]), the windows read
//! it.
mod names;
mod snbt;
mod store;
pub mod wire;

pub use names::received_name;
pub use store::*;
