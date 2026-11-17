//! The built-in mod that runs the Lua mods in `<game_dir>/lua-mods/`: the one entry the game starts
//! with, and the package the loader checks for it beside the executable.

use crate::discover;
use crate::host::{LuaHost, LuaMod};
use crate::package;
use mod_api::__private::native::BuiltMod;
use mod_api::ModEntry;
use mod_api::Side;
use std::path::Path;

/// The built-in mod the loader must find a package for.
pub const BUILT: &[BuiltMod] = &[BuiltMod {
    id: package::ID,
    version: package::VERSION,
    side: Side::Both,
}];

/// What the game's Lua mods came to: the entry to start with, how many loaded, and one line per
/// folder or mod that did not load.
pub struct Loaded {
    pub entry: ModEntry,
    pub loaded: usize,
    pub refused: Vec<String>,
}

/// Scans `<game_dir>/lua-mods/` and loads each mod that passes. A mod that fails is refused and
/// the others still load. A missing folder is fine: the entry then subscribes to nothing.
pub fn builtin(game_dir: &Path) -> Loaded {
    let (mods, refused) = load_mods(game_dir);
    let loaded = mods.len();
    Loaded {
        entry: ModEntry::new(
            package::ID,
            Box::new(LuaHost::new(mods).with_report(refused.clone())),
        ),
        loaded,
        refused,
    }
}

/// The mods under `game_dir` that load, and one line per folder or mod that does not. Nothing is
/// logged here: this runs before the game's logger is up, and `LuaHost::register` reports them.
fn load_mods(game_dir: &Path) -> (Vec<LuaMod>, Vec<String>) {
    let found = discover::scan(game_dir);
    let mut refused: Vec<String> = found
        .refused
        .iter()
        .map(|r| {
            let name = r
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{name}: {}", r.reason)
        })
        .collect();
    let mut mods: Vec<LuaMod> = Vec::new();
    for found_mod in &found.mods {
        match LuaMod::load(found_mod) {
            Ok(m) => mods.push(m),
            Err(reason) => refused.push(format!("{}: {reason}", found_mod.id)),
        }
    }
    (mods, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `<root>/lua-mods/<name>/` with a manifest for `id` and the given script.
    fn mod_folder(root: &Path, name: &str, id: &str, script: &str) {
        let dir = root.join(discover::DIR).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(discover::MANIFEST),
            format!("id = \"{id}\"\nversion = \"1.0.0\"\nside = \"both\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join(discover::ENTRY), script).unwrap();
    }

    #[test]
    fn built_matches_the_package() {
        assert_eq!(BUILT.len(), 1);
        assert_eq!(BUILT[0].id, package::ID);
        assert_eq!(BUILT[0].version, package::VERSION);
    }

    #[test]
    fn the_loader_accepts_the_package() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("horizon-native-mods").join(package::ID);
        package::write_to(&dir).unwrap();
        let runtime =
            horizon_mod_package::runtime::Runtime::load(&root.path().join("event-horizon"), BUILT);
        assert!(runtime.is_ok());
    }

    #[test]
    fn valid_mods_are_loaded() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "alpha",
            "alpha_mod",
            // Lua `server_tick` is unavailable until the generator converts `ServerTickPhase`;
            // this fixture uses `block_receive_game`, which the generated impl has.
            "horizon.on(\"block_receive_game\", function() end)\n",
        );
        let built = builtin(root.path());
        assert_eq!(built.loaded, 1);
        assert!(built.refused.is_empty());
    }

    #[test]
    fn a_bad_mod_is_refused_and_the_others_still_load() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_bad",
            "bad_mod",
            "horizon.on(\"nope\", function() end)\n",
        );
        mod_folder(
            root.path(),
            "b_good",
            "good_mod",
            // Lua `server_tick` is unavailable until the generator converts `ServerTickPhase`;
            // this fixture uses `block_receive_game`, which the generated impl has.
            "horizon.on(\"block_receive_game\", function() end)\n",
        );
        let built = builtin(root.path());
        assert_eq!(built.loaded, 1);
        assert_eq!(built.refused.len(), 1);
        assert!(built.refused[0].starts_with("bad_mod: "));
        assert!(built.refused[0].len() > "bad_mod: ".len());
    }

    #[test]
    fn a_missing_lua_mods_folder_is_fine() {
        let root = tempfile::tempdir().unwrap();
        let built = builtin(root.path());
        assert_eq!(built.loaded, 0);
        assert!(built.refused.is_empty());
    }

    #[test]
    fn the_refusals_are_handed_to_the_host() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_bad",
            "bad_mod",
            "horizon.on(\"nope\", function() end)\n",
        );
        let (mods, refused) = load_mods(root.path());
        assert!(mods.is_empty());
        assert_eq!(refused.len(), 1);
        let host = LuaHost::new(mods).with_report(refused.clone());
        assert_eq!(host.report_lines(), refused.as_slice());
    }

    #[test]
    fn refused_folders_from_the_scan_are_reported() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(discover::DIR).join("no_manifest")).unwrap();
        let built = builtin(root.path());
        assert_eq!(built.loaded, 0);
        assert_eq!(built.refused.len(), 1);
        assert!(built.refused[0].starts_with("no_manifest: "));
    }
}
