//! Event Horizon's program assembly.
#![forbid(unsafe_code)]

use horizon_client::mod_api::Hooks;

/// The mod API level this game serves, kept in the program for the launcher to read
/// without running it (`horizon_mods::api_level::MARK`). [`run`] reads both marks, so
/// the linker keeps them in every executable that starts through it.
static MOD_API_LEVEL: [u8; 27] = horizon_mods::api_level::MARK;
/// The feature registry this game answers `--list-mods` with, kept the same way
/// (`horizon_mods::feature_registry::MARK`).
static FEATURE_REGISTRY: [u8; 27] = horizon_mods::feature_registry::MARK;

/// The built-in mods in dispatch order: the extras (zoom, the minimap, the
/// FPS counter, keystrokes), then the developer tools, then scripted mods.
pub fn builtin_mods(args: impl Iterator<Item = String>) -> Vec<Box<dyn Hooks>> {
    let lua = event_horizon_lua::discover(args);
    let mut mods = event_horizon_extras::mods();
    mods.extend(event_horizon_dev_tools::mods(!lua.is_empty()));
    mods.extend(lua);
    mods
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
