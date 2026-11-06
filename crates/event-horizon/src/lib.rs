//! Event Horizon's program assembly.
#![forbid(unsafe_code)]

use horizon_client::mod_api::Hooks;

/// The built-in mods in dispatch order: developer tools, then scripted mods.
pub fn builtin_mods(args: impl Iterator<Item = String>) -> Vec<Box<dyn Hooks>> {
    let lua = event_horizon_lua::discover(args);
    let mut mods = horizon_client::dev_tools::mods(!lua.is_empty());
    mods.extend(lua);
    mods
}
