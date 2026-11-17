//! Event Horizon's Lua runtime: one built-in host mod that runs .hlua mods (sandboxed Luau) on the mod API
#![forbid(unsafe_code)]
pub mod package;
pub mod state;
