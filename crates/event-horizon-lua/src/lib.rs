//! Event Horizon's Lua runtime and developer windows: scripted mods and
//! their mob goals (docs/lua-bindings.md), a [`Hooks`] implementation the
//! program crate installs like a trait mod's, and the egui windows the
//! developer tools draw over the world (docs/dev-tools.md).
//!
//! [`Hooks`]: horizon_client::mod_api::Hooks
#![forbid(unsafe_code)]

pub mod dev_tools;
pub mod lua_mods;

pub use lua_mods::{LUA_HOOK_NAMES, check_mod, discover};
