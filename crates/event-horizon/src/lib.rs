//! Event Horizon's program assembly.
#![forbid(unsafe_code)]

use event_horizon_lua::lua_mods::LuaSide;
use event_horizon_lua::lua_mods::registry::{self, SettingView};
use event_horizon_lua::lua_mods::settings::{Change, Kind as LuaKind};
use horizon_client::mod_api::{Hooks, Value};
use horizon_client::mod_settings::{Kind, ModSettings, Provider, SettingEntry, Side};

/// The mod API level this game serves, kept in the program for the launcher to read
/// without running it (`horizon_mods::api_level::MARK`). [`run`] reads both marks, so
/// the linker keeps them in every executable that starts through it.
static MOD_API_LEVEL: [u8; 27] = horizon_mods::api_level::MARK;
/// The feature registry this game answers `--list-mods` with, kept the same way
/// (`horizon_mods::feature_registry::MARK`).
static FEATURE_REGISTRY: [u8; 27] = horizon_mods::feature_registry::MARK;

/// The built-in mods in dispatch order: the extras (zoom, the minimap, the
/// FPS counter, keystrokes), then the developer tools, then scripted mods,
/// then the `/lua` console (the developer tools only).
pub fn builtin_mods(args: impl Iterator<Item = String>) -> Vec<Box<dyn Hooks>> {
    let lua = event_horizon_lua::discover(args);
    install_mod_settings();
    let mut mods = event_horizon_extras::mods();
    mods.extend(event_horizon_dev_tools::mods(!lua.is_empty()));
    mods.extend(lua);
    if horizon_client::dev_input::enabled() {
        // The console needs the developer tools: its window is theirs.
        mods.extend(event_horizon_lua::console::mods());
    }
    mods
}

/// Gives the game's screens the Lua mods' settings (#64): the registry `discover` just filled,
/// and a write's `config_changed` told through the executable's native hook table. Installed once.
fn install_mod_settings() {
    let _ = horizon_client::mod_settings::install(Provider {
        list: list_mod_settings,
        set: set_mod_setting,
        reset: reset_mod_setting,
    });
}

fn list_mod_settings() -> Vec<ModSettings> {
    registry::mod_settings()
        .into_iter()
        .map(|m| ModSettings {
            mod_id: m.mod_id,
            entries: m.entries.into_iter().map(setting_entry).collect(),
        })
        .collect()
}

/// A declared setting as the game's screens take it. Editable is set here; whether this game may
/// write a server-side value is for the screen to gate (the provider cannot see the game's state).
fn setting_entry(view: SettingView) -> SettingEntry {
    let e = view.entry;
    SettingEntry {
        key: e.key,
        kind: match e.kind {
            LuaKind::Bool => Kind::Bool,
            LuaKind::Int => Kind::Int,
            LuaKind::Float => Kind::Float,
            LuaKind::Str => Kind::Text,
            LuaKind::Choice => Kind::Choice,
        },
        side: match e.side {
            LuaSide::Server => Side::Server,
            LuaSide::Client => Side::Client,
        },
        group: e.group,
        order: e.order,
        hidden: e.hidden,
        editable: true,
        min: e.min,
        max: e.max,
        choices: e.choices,
        default: e.default,
        value: view.value,
    }
}

fn lua_side(side: Side) -> LuaSide {
    match side {
        Side::Server => LuaSide::Server,
        Side::Client => LuaSide::Client,
    }
}

fn set_mod_setting(mod_id: &str, side: Side, key: &str, value: Value) -> Result<(), String> {
    registry::set(mod_id, lua_side(side), key, value, &mut notify_changed)
}

fn reset_mod_setting(mod_id: &str, side: Side, key: &str) -> Result<(), String> {
    registry::reset(mod_id, lua_side(side), key, &mut notify_changed)
}

fn notify_changed(c: &Change) {
    horizon_client::native_client_hooks::native_config_changed(&c.mod_id, &c.key, &c.old, &c.new);
}

/// Event Horizon's startup, shared by its own executable and the launcher's generated one
/// for native mods: the marks the launcher reads, `--check-mod`, `--list-mods`, then the
/// game with `native` first and the built-in mods behind them. `native` comes first so
/// its ids (`mod_api::name_mods`, which the generated main installs first) still name
/// the right entries; the built-in mods are discovered at run time, so no static id can
/// name them.
pub fn run(native: Vec<Box<dyn Hooks>>) {
    event_horizon_websocket::install();
    std::hint::black_box(&MOD_API_LEVEL);
    std::hint::black_box(&FEATURE_REGISTRY);

    // `--check-mod <path> [--dev-mod <library>]... [--resources <folder>]`:
    // load a scripted mod headlessly (and with `--resources`, layer its
    // content as the game would), report, and leave; no window, no world.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--check-mod") {
        let Some(path) = args.get(i + 1) else {
            eprintln!("--check-mod <path to a mod folder or .hmod>");
            std::process::exit(2);
        };
        let dev: Vec<std::path::PathBuf> = args
            .windows(2)
            .filter(|w| w[0] == "--dev-mod")
            .map(|w| std::path::PathBuf::from(&w[1]))
            .collect();
        let resources = args
            .windows(2)
            .find(|w| w[0] == "--resources")
            .map(|w| std::path::PathBuf::from(&w[1]));
        match event_horizon_lua::check_mod(std::path::Path::new(path), &dev, resources.as_deref()) {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    // `--list-mods` asks for the build's feature registry and nothing else
    // (`main_with` prints it and returns): the launcher runs it at every
    // version selection, so no mod is discovered or loaded for it, no
    // `mods` folder scanned, and the developer tools stay off.
    if list_mods_only(&args) {
        horizon_client::entry::main_with(Default::default());
        return;
    }
    let mut mods = native;
    mods.extend(builtin_mods(args.into_iter()));
    horizon_client::entry::main_with(mods.into());
}

/// Whether the command line is a `--list-mods` query, as `main_with` reads
/// it: among the game's arguments, before any `--dedicated`.
fn list_mods_only(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| *a != "--dedicated")
        .any(|a| a == "--list-mods")
}

#[cfg(test)]
mod tests {
    use super::list_mods_only;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn list_mods_is_answered_before_any_mod_is_discovered() {
        // The launcher's query, alone or with the game's other arguments.
        assert!(list_mods_only(&args(&["--list-mods"])));
        assert!(list_mods_only(&args(&[
            "--game-dir",
            "x",
            "--list-mods",
            "--dev-tools"
        ])));
        // A plain start, a mod check and a server start discover their mods.
        assert!(!list_mods_only(&args(&[])));
        assert!(!list_mods_only(&args(&["--dev-tools", "--dev-mod", "m"])));
        assert!(!list_mods_only(&args(&["--dedicated", "--list-mods"])));
    }
}
