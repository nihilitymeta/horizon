//! Generator of the Lua host's bindings and type definitions from the mod API's surface.json
#![forbid(unsafe_code)]

pub mod emit_hooks;
pub mod emit_impl;
pub mod handles;
pub mod ir;
pub mod trait_sigs;
pub mod types;
