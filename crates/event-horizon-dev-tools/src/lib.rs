//! Event Horizon's developer tools, as a built-in mod: an egui window drawn over the world, on
//! with `HORIZON_DEV_TOOLS=1` or `--dev-tools`. Off, nothing here is registered or loaded.
//!
//! Chunk 5c-a is the skeleton: one empty window. Input, commands and the console come later.
#![forbid(unsafe_code)]

mod overlay;
pub mod package;

use mod_api::__private::native::BuiltMod;
use mod_api::{Hooks, ModEntry, Side};
use std::ffi::{OsStr, OsString};

/// The environment variable that turns the tools on, with the value `1`.
pub const ENV: &str = "HORIZON_DEV_TOOLS";
/// The launch argument that turns the tools on, as a launcher passes it.
pub const FLAG: &str = "--dev-tools";

/// The built-in mod the loader must find a package for, when the tools are on.
pub const BUILT: &[BuiltMod] = &[BuiltMod {
    id: package::ID,
    version: package::VERSION,
    side: Side::Both,
}];

/// Whether the environment's `HORIZON_DEV_TOOLS` is `1`, or the launch arguments name `--dev-tools`.
pub fn requested(env: Option<&OsStr>, args: impl IntoIterator<Item = OsString>) -> bool {
    env == Some(OsStr::new("1")) || args.into_iter().any(|a| a.as_os_str() == OsStr::new(FLAG))
}

/// The tools' mod entry, and the window's callback added to the world's draw. Call this only
/// when [`requested`] said yes: the callback is never added otherwise.
pub fn mods() -> Vec<ModEntry> {
    overlay::install();
    vec![ModEntry::new(package::ID, Box::new(DevTools))]
}

/// The developer tools' hooks: none yet, the window is drawn by its callback.
#[derive(Default)]
struct DevTools;

#[mod_api::horizon_mod]
impl Hooks for DevTools {}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn the_env_value_one_turns_the_tools_on() {
        assert!(requested(Some(OsStr::new("1")), []));
        assert!(!requested(Some(OsStr::new("0")), []));
        assert!(!requested(None, []));
    }

    #[test]
    fn the_flag_turns_the_tools_on_beside_or_without_the_env() {
        assert!(requested(None, [os("game"), os(FLAG)]));
        assert!(requested(Some(OsStr::new("1")), [os(FLAG)]));
        assert!(!requested(None, [os("--dev-tools-off"), os("--world")]));
    }

    #[test]
    fn the_built_in_mod_is_the_tools_package() {
        assert_eq!(BUILT.len(), 1);
        assert_eq!(BUILT[0].id, package::ID);
        assert_eq!(BUILT[0].version, package::VERSION);
    }
}
