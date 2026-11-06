//! The developer tools' windows on a guest's own screen (docs/dev-tools.md,
//! "A guest's windows"): what the server and a Horizon client with the
//! windows say to each other. The codec lives in the `event-horizon-observe`
//! crate; this module keeps the path the game's callers use.
pub use event_horizon_observe::wire::*;
