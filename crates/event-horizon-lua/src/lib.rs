//! Event Horizon's Lua runtime: scripted mods and their mob goals
//! (docs/lua-bindings.md) and the `/lua` console (docs/dev-tools.md), each
//! a [`Hooks`] implementation the program crate installs like a trait mod's.
//! The developer windows live in `event-horizon-dev-tools`
//! (docs/dev-tools.md).
//!
//! [`Hooks`]: horizon_client::mod_api::Hooks
#![forbid(unsafe_code)]

pub mod console;
pub mod lua_mods;

pub use lua_mods::{LUA_HOOK_NAMES, check_mod, discover};
