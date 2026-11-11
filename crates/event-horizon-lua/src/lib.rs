//! Event Horizon's Lua runtime: scripted mods and their mob goals
//! (docs/lua-bindings.md) and the `/lua` console (docs/dev-tools.md), each
//! a [`Hooks`] implementation the program crate installs like a trait mod's.
//! The developer windows live in `event-horizon-dev-tools`
//! (docs/dev-tools.md).
//!
//! [`Hooks`]: horizon_client::mod_api::Hooks
#![forbid(unsafe_code)]

pub mod console;
#[macro_use]
mod generated_runtime;
#[macro_use]
mod generated_hooks;
pub mod hud_hooks;
pub mod lua_mods;
pub mod profiler;

pub use lua_mods::{check_mod, discover};
pub use profiler::{ProfileRow, profile_report, profile_reset};
