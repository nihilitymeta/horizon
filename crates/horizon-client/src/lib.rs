//! Gameplay and startup, above the reusable client library.
// No `#![recursion_limit]` or other inner attribute horizon-mixins doesn't know here: the
// launcher's horizon-mixins then rejects every native mod targeting this crate (#8169).
#![forbid(unsafe_code)]
extern crate self as horizon_client;
pub use horizon_client_lib::*;
pub mod dev_input;
pub mod entry;
pub mod game;
pub mod mod_api;
/// Constructors for the mod API's `#[non_exhaustive]` event and player structs
/// (whos-carmen/horizon#28): the structs say fields may be added, so only their
/// defining crate can build them, and the Lua runtime's tests moved out of it
/// into `event-horizon-lua`. The `test-support` feature (as `horizon-items`'s)
/// turns them on for a dev-dependency; players' builds never see them.
#[cfg(any(test, feature = "test-support"))]
pub mod test_events;
