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

/// Lets the built-in package declare content in `m`'s namespace (its id). The game reads a
/// package's namespaces when it starts, so a Lua mod's namespace, found in its folder, is granted
/// here, before the game dispatches the `blocks` and `items` hooks.
fn grant_namespace(m: &LuaMod) -> Result<(), String> {
    mod_api::__private::grant_namespace(package::ID, m.id())
        .map_err(|reason| format!("cannot declare content in its namespace: {reason}"))
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
            Ok(m) => match grant_namespace(&m) {
                Ok(()) => mods.push(m),
                Err(reason) => refused.push(format!("{}: {reason}", found_mod.id)),
            },
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
            "horizon.on(\"server_tick\", function() end)\n",
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
            "horizon.on(\"server_tick\", function() end)\n",
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

    /// Whether the game subscribes `host` to `hook`. `HookId` has no `PartialEq`, so the ids are
    /// compared by their discriminant.
    fn subscribes_to(host: &LuaHost, hook: &str) -> bool {
        let wanted = crate::generated::hooks::hook_id(hook).expect("the hook exists");
        host.subscribed_slice()
            .iter()
            .flatten()
            .any(|id| *id as usize == wanted as usize)
    }

    /// The block ids the host answers `blocks` with, in order.
    fn answered_blocks(host: &LuaHost) -> Vec<String> {
        let mut out = Vec::new();
        host.declared_blocks(&mut out);
        out.into_iter().map(|block| block.id).collect()
    }

    /// Records the keys asked for, and refuses the requests for `refuse` (a block or item id).
    #[derive(Default)]
    struct RecordingKeys {
        asked: Vec<String>,
        refuse: Option<String>,
    }

    impl crate::host::DeclaredKeys for RecordingKeys {
        fn block_key(&mut self, id: &str) -> Result<(), String> {
            self.asked.push(format!("block {id}"));
            self.answer(id)
        }

        fn item_key(&mut self, id: &str) -> Result<(), String> {
            self.asked.push(format!("item {id}"));
            self.answer(id)
        }
    }

    impl RecordingKeys {
        fn answer(&self, id: &str) -> Result<(), String> {
            if self.refuse.as_deref() == Some(id) {
                Err(format!("the game refused {id}"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn declared_blocks_and_items_are_answered_for_every_mod_in_order() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_first",
            "first_mod",
            "horizon.block{id = \"first_mod:lamp\", template = \"minecraft:stone\"}\n\
             horizon.block{id = \"first_mod:pipe\", template = \"minecraft:stone\"}\n\
             horizon.item{id = \"first_mod:wand\", template = \"minecraft:stick\"}\n",
        );
        mod_folder(
            root.path(),
            "b_none",
            "empty_mod",
            "horizon.on(\"server_tick\", function() end)\n",
        );
        mod_folder(
            root.path(),
            "c_second",
            "second_mod",
            "horizon.block{id = \"second_mod:cube\", template = \"minecraft:stone\"}\n",
        );
        let (mods, refused) = load_mods(root.path());
        assert!(refused.is_empty(), "got {refused:?}");
        let host = LuaHost::new(mods);
        assert_eq!(
            answered_blocks(&host),
            ["first_mod:lamp", "first_mod:pipe", "second_mod:cube"]
        );
        let mut items = Vec::new();
        host.declared_items(&mut items);
        let items: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(items, ["first_mod:wand"]);
    }

    #[test]
    fn a_mod_that_declares_nothing_contributes_nothing_and_no_subscription() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_none",
            "empty_mod",
            "horizon.on(\"server_tick\", function() end)\n",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        assert!(answered_blocks(&host).is_empty());
        assert!(!subscribes_to(&host, "blocks"));
    }

    #[test]
    fn declaring_a_block_subscribes_the_host_to_the_blocks_hook() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_block",
            "block_mod",
            "horizon.block{id = \"block_mod:lamp\", template = \"minecraft:stone\"}\n",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        assert!(subscribes_to(&host, "blocks"));
        assert!(!subscribes_to(&host, "items"));
    }

    #[test]
    fn a_bad_declaration_refuses_only_its_mod() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_bad",
            "bad_mod",
            "horizon.block{id = \"minecraft:lamp\", template = \"minecraft:stone\"}\n",
        );
        mod_folder(
            root.path(),
            "b_good",
            "good_mod",
            "horizon.block{id = \"good_mod:lamp\", template = \"minecraft:stone\"}\n",
        );
        let built = builtin(root.path());
        assert_eq!(built.loaded, 1);
        assert_eq!(built.refused.len(), 1);
        assert!(
            built.refused[0].starts_with("bad_mod: "),
            "got {:?}",
            built.refused
        );
        assert!(built.refused[0].contains("outside this mod's namespace"));
    }

    #[test]
    fn key_requests_are_made_in_order_for_each_declared_block_and_item() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_first",
            "first_mod",
            "horizon.block{id = \"first_mod:lamp\", template = \"minecraft:stone\"}\n\
             horizon.item{id = \"first_mod:wand\", template = \"minecraft:stick\"}\n\
             horizon.block{id = \"first_mod:pipe\", template = \"minecraft:stone\"}\n",
        );
        mod_folder(
            root.path(),
            "b_second",
            "second_mod",
            "horizon.block{id = \"second_mod:cube\", template = \"minecraft:stone\"}\n",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let mut keys = RecordingKeys::default();
        host.request_keys(&mut keys);
        // Each mod's blocks, then its items: the order the game sees the requests in.
        assert_eq!(
            keys.asked,
            [
                "block first_mod:lamp",
                "block first_mod:pipe",
                "item first_mod:wand",
                "block second_mod:cube",
            ]
        );
        assert_eq!(answered_blocks(&host).len(), 3);
    }

    #[test]
    fn a_refused_key_request_refuses_only_its_mod() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_first",
            "first_mod",
            "horizon.block{id = \"first_mod:lamp\", template = \"minecraft:stone\"}\n",
        );
        mod_folder(
            root.path(),
            "b_second",
            "second_mod",
            "horizon.block{id = \"second_mod:cube\", template = \"minecraft:stone\"}\n",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let mut keys = RecordingKeys {
            refuse: Some("first_mod:lamp".to_owned()),
            ..RecordingKeys::default()
        };
        host.request_keys(&mut keys);
        assert_eq!(answered_blocks(&host), ["second_mod:cube"]);
    }

    #[test]
    fn a_mod_whose_namespace_is_reserved_is_refused_and_the_others_load() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_reserved",
            "minecraft",
            "horizon.block{id = \"minecraft:lamp\", template = \"minecraft:stone\"}\n",
        );
        mod_folder(
            root.path(),
            "b_good",
            "granted_mod",
            "horizon.block{id = \"granted_mod:lamp\", template = \"minecraft:stone\"}\n",
        );
        let built = builtin(root.path());
        assert_eq!(built.loaded, 1);
        assert_eq!(built.refused.len(), 1);
        assert!(
            built.refused[0].starts_with("minecraft: "),
            "got {:?}",
            built.refused
        );
        assert!(built.refused[0].contains("reserved"));
    }
}
