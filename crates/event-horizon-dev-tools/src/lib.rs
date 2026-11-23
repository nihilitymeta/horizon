//! Event Horizon's developer tools, as a built-in mod: an egui window drawn over the world, on
//! with `HORIZON_DEV_TOOLS=1` or `--dev-tools`. Off, nothing here is registered or loaded.
//!
//! Chunk 5c-a is the skeleton: one empty window. Chunk 5c-c adds the commands (`commands`); chunk
//! 5c-b2 adds the input (`input`: the key frees the pointer, and egui takes the mouse and keys
//! while it is free); 5e-c adds the Lua console window (`console`: its state, and the server tick
//! that runs what it queues).
#![forbid(unsafe_code)]

mod commands;
mod console;
mod input;
mod inspect;
mod overlay;
pub mod package;
mod windows;

use mod_api::__private::native::BuiltMod;
use mod_api::{
    Flow, Hooks, ModEntry, PlayerCommandEvent, RegisterCommandsEvent, RunModCommandEvent,
    ServerHost, ServerTickEvent, ServerTickPhase, Side, WorldLifecycleEvent, WorldPhase,
};
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
    input::install();
    horizon_client_input::keybinds::show_dev_tools_key(true);
    vec![ModEntry::new(package::ID, Box::new(DevTools))]
}

/// The developer tools' hooks: their commands. The window is drawn by its callback.
#[derive(Default)]
struct DevTools;

#[mod_api::horizon_mod]
impl Hooks for DevTools {
    fn register_commands(&self, _: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        event.commands.extend(
            commands::USAGES
                .iter()
                .map(|usage| mod_api::Command::new(usage)),
        );
    }

    /// The window's own player's commands, before the tree: the owner's only. Any other player's
    /// command goes on to the tree, which refuses it.
    fn player_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut PlayerCommandEvent<'_>,
    ) -> Flow {
        match commands::player_command(host, event.player, event.command) {
            Some(replies) => {
                event.replies.extend(replies);
                Flow::Cancel
            }
            None => Flow::Continue,
        }
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        for line in commands::run_tree_command(host, event.source, event.command) {
            if let Err(e) = host.commands().feedback(line) {
                log::warn!("a developer tools reply was refused: {e:?}");
            }
        }
        Flow::Cancel
    }

    /// Once a server tick, after every level ticked, while the world runs: the windows' requests
    /// and the watched entities' data (`inspect`), and the console's queued sources.
    fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>) {
        if matches!(
            event.phase,
            ServerTickPhase::ServerTickEnd {
                world_running: true
            }
        ) {
            inspect::tick(host);
            console::tick_host(host);
        }
    }

    /// The world closed: the tools' own state starts again (the store is reset by the engine).
    fn world_lifecycle(&self, _host: &mut dyn ServerHost, event: &mut WorldLifecycleEvent<'_>) {
        if event.phase == WorldPhase::WorldCloseFinal {
            inspect::world_closed();
        }
    }
}

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
