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

    /// Records the keys asked for and the parts attached, and refuses the requests for `refuse` (a
    /// block or item id).
    #[derive(Default)]
    struct RecordingKeys {
        asked: Vec<String>,
        refuse: Option<String>,
        parts: Vec<crate::host::LuaBlockPart>,
        item_parts: Vec<crate::host::LuaItemPart>,
        block_entity_types: Vec<(String, Vec<String>)>,
        block_entity_parts: Vec<crate::host::LuaBlockEntityPart>,
        /// Refuses the block entity part of this type id (the type's own request is answered).
        refuse_part: Option<String>,
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

        fn block_part(&mut self, id: &str, part: crate::host::LuaBlockPart) -> Result<(), String> {
            self.asked.push(format!("part {id}"));
            self.parts.push(part);
            self.answer(id)
        }

        fn item_part(&mut self, id: &str, part: crate::host::LuaItemPart) -> Result<(), String> {
            self.asked.push(format!("item part {id}"));
            self.item_parts.push(part);
            self.answer(id)
        }

        fn block_entity_type(&mut self, id: &str, blocks: &[String]) -> Result<(), String> {
            self.asked.push(format!("block entity type {id}"));
            self.block_entity_types
                .push((id.to_owned(), blocks.to_vec()));
            self.answer(id)
        }

        fn block_entity_part(
            &mut self,
            id: &str,
            part: crate::host::LuaBlockEntityPart,
        ) -> Result<(), String> {
            self.asked.push(format!("block entity part {id}"));
            self.block_entity_parts.push(part);
            if self.refuse_part.as_deref() == Some(id) {
                return Err(format!("the game refused the part of {id}"));
            }
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

    /// A mod folder that declares its manifest's `side`, with `script` as its main.luau.
    fn mod_folder_on(root: &Path, name: &str, id: &str, side: &str, script: &str) {
        let dir = root.join(discover::DIR).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(discover::MANIFEST),
            format!("id = \"{id}\"\nversion = \"1.0.0\"\nside = \"{side}\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join(discover::ENTRY), script).unwrap();
    }

    #[test]
    fn declared_client_commands_are_answered_for_every_client_mod_in_order() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_first",
            "first_mod",
            "horizon.client_command{usage = \"hello <who:word>\"}\n\
             horizon.client_command{usage = \"mode <m:word>\"}\n",
        );
        mod_folder(
            root.path(),
            "b_none",
            "empty_mod",
            "horizon.on(\"server_tick\", function() end)\n",
        );
        mod_folder_on(
            root.path(),
            "c_server",
            "server_mod",
            "server",
            "horizon.client_command{usage = \"unseen\"}\n",
        );
        mod_folder(
            root.path(),
            "d_second",
            "second_mod",
            "horizon.client_command{usage = \"hi\"}\n",
        );
        let (mods, refused) = load_mods(root.path());
        assert!(refused.is_empty(), "got {refused:?}");
        let host = LuaHost::new(mods);
        let mut commands = Vec::new();
        host.declared_client_commands(&mut commands);
        let usages: Vec<(&str, u8)> = commands
            .iter()
            .map(|c| (c.usage.as_str(), c.level))
            .collect();
        assert_eq!(
            usages,
            [("hello <who:word>", 0), ("mode <m:word>", 0), ("hi", 0)]
        );
        assert!(subscribes_to(&host, "register_client_commands"));
    }

    #[test]
    fn a_mod_without_client_commands_does_not_subscribe_to_the_client_hook() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_none",
            "empty_mod",
            "horizon.on(\"server_tick\", function() end)\n",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let mut commands = Vec::new();
        host.declared_client_commands(&mut commands);
        assert!(commands.is_empty());
        assert!(!subscribes_to(&host, "register_client_commands"));
    }

    /// The configs the host answers `register_configs` with, as (mod id, file, kind, default).
    fn answered_configs(host: &LuaHost) -> Vec<(String, String, mod_api::ConfigKind, String)> {
        let mut out = Vec::new();
        host.declared_configs(&mut out);
        out.into_iter()
            .map(|c| (c.mod_id, c.file, c.kind, c.default))
            .collect()
    }

    #[test]
    fn declared_configs_are_answered_in_load_order_under_each_host_id() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_first",
            "first_mod",
            "horizon.config{kind = \"common\", default = \"a = 1\\n\"}\n\
             horizon.config{kind = \"server\", default = \"\", file = \"first_mod/machines.toml\"}\n",
        );
        mod_folder(
            root.path(),
            "b_second",
            "second_mod",
            "horizon.config{kind = \"common\", default = \"b\"}\n",
        );
        let (mods, refused) = load_mods(root.path());
        assert!(refused.is_empty(), "got {refused:?}");
        let host = LuaHost::new(mods);
        use mod_api::ConfigKind::{Common, Server};
        assert_eq!(
            answered_configs(&host),
            [
                (
                    "first_mod".into(),
                    "first_mod-common.toml".into(),
                    Common,
                    "a = 1\n".into()
                ),
                (
                    "first_mod".into(),
                    "first_mod/machines.toml".into(),
                    Server,
                    String::new()
                ),
                (
                    "second_mod".into(),
                    "second_mod-common.toml".into(),
                    Common,
                    "b".into()
                ),
            ]
        );
        assert!(subscribes_to(&host, "register_configs"));
    }

    #[test]
    fn a_config_is_answered_only_where_its_mod_runs() {
        let root = tempfile::tempdir().unwrap();
        mod_folder_on(
            root.path(),
            "a_client",
            "client_mod",
            "client",
            "horizon.config{kind = \"server\", default = \"\"}\n\
             horizon.config{kind = \"client\", default = \"\"}\n",
        );
        mod_folder_on(
            root.path(),
            "b_server",
            "server_mod",
            "server",
            "horizon.config{kind = \"client\", default = \"\"}\n",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let files: Vec<String> = answered_configs(&host).into_iter().map(|c| c.1).collect();
        assert_eq!(files, ["client_mod-client.toml"]);
    }

    #[test]
    fn a_mod_without_configs_does_not_subscribe_to_the_config_hook() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_none",
            "empty_mod",
            "horizon.on(\"server_tick\", function() end)\n",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        assert!(answered_configs(&host).is_empty());
        assert!(!subscribes_to(&host, "register_configs"));
    }

    #[test]
    fn a_config_hook_reaches_only_the_mod_that_owns_the_file() {
        let root = tempfile::tempdir().unwrap();
        for (name, id) in [("a_first", "mod_a"), ("b_second", "mod_b")] {
            mod_folder(
                root.path(),
                name,
                id,
                "horizon.on(\"config_loading\", function(e) horizon.log(\"saw \" .. e.config.file) end)\n",
            );
        }
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let build = |lua: &mlua::Lua, _: &crate::hostcall::TurnHost<'_>| {
            let config = lua.create_table()?;
            config.raw_set("mod_id", "mod_b")?;
            config.raw_set("file", "mod_b-common.toml")?;
            config.raw_set("text", "answer = 42\n")?;
            let data = lua.create_table()?;
            data.raw_set("config", config)?;
            crate::dispatch::new_event(lua, data, &[])
        };
        host.dispatch_turns_owned(
            "config_loading",
            "mod_b",
            crate::hostcall::TurnHost::None,
            &|| false,
            &build,
            &mut |_, _, _| true,
        )
        .unwrap();
        let seen = |id: &str| {
            host.mods()
                .iter()
                .find(|m| m.id() == id)
                .unwrap()
                .log_lines()
        };
        assert!(seen("mod_a").is_empty(), "mod_a saw {:?}", seen("mod_a"));
        assert_eq!(seen("mod_b"), ["saw mod_b-common.toml"]);
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

    /// The host of the mods in `folders` (`(folder, id, script)`), with its keys requested.
    fn requested(folders: &[(&str, &str, &str)]) -> (LuaHost, RecordingKeys) {
        let root = tempfile::tempdir().unwrap();
        for (folder, id, script) in folders {
            mod_folder(root.path(), folder, id, script);
        }
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let mut keys = RecordingKeys::default();
        host.request_keys(&mut keys);
        (host, keys)
    }

    /// Runs the part's `can_survive` handler: its answer, as the generated method would pass it on.
    fn survives(part: &crate::host::LuaBlockPart) -> Option<bool> {
        let mut found = None;
        let build = |lua: &mlua::Lua, _: &crate::hostcall::TurnHost<'_>| {
            crate::dispatch::new_event(lua, lua.create_table()?, &[])
        };
        let mut answer = |id: &str, value: mlua::Value, _: &crate::hostcall::TurnHost<'_>| {
            found = crate::dispatch::answer_of::<bool>(id, "can_survive", &value);
            true
        };
        part.dispatch_replace(
            "can_survive",
            crate::hostcall::TurnHost::None,
            &build,
            &mut answer,
        )
        .unwrap();
        found
    }

    #[test]
    fn parts_with_one_handler_set_share_one_subscription() {
        let (_, keys) = requested(&[
            (
                "a_first",
                "first_mod",
                "horizon.block_part(\"first_mod:lamp\", {can_survive = function() end})\n\
                horizon.block_part(\"first_mod:pipe\", {can_survive = function() end})",
            ),
            (
                "b_second",
                "second_mod",
                "horizon.block_part(\"second_mod:x\", {can_survive = function() end, may_place_on = function() end})",
            ),
        ]);
        assert_eq!(keys.parts.len(), 3);
        assert!(std::ptr::eq(
            keys.parts[0].subscribed(),
            keys.parts[1].subscribed()
        ));
        assert!(!std::ptr::eq(
            keys.parts[0].subscribed(),
            keys.parts[2].subscribed()
        ));
        assert_eq!(keys.parts[2].subscribed().len(), 2);
    }

    #[test]
    fn a_part_runs_only_its_own_mods_handler() {
        let (host, keys) = requested(&[
            (
                "a_first",
                "first_mod",
                "horizon.block_part(\"first_mod:lamp\", {can_survive = function() horizon.log(\"lamp answered\") return true end})",
            ),
            (
                "b_second",
                "second_mod",
                "horizon.block_part(\"second_mod:x\", {can_survive = function() horizon.log(\"second ran\") return false end})",
            ),
        ]);
        assert_eq!(survives(&keys.parts[0]), Some(true));
        assert_eq!(host.mods()[0].log_lines(), ["lamp answered"]);
        assert!(host.mods()[1].log_lines().is_empty());
    }

    #[test]
    fn a_failing_part_handler_voids_its_answer() {
        let (_, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.block_part(\"first_mod:lamp\", {can_survive = function() error(\"boom\") end})",
        )]);
        assert_eq!(survives(&keys.parts[0]), None);
    }

    /// Runs the item part's `use_item` handler: its answer, as the generated method would pass it on.
    fn use_item_answer(part: &crate::host::LuaItemPart) -> Option<mod_api::InteractionResult> {
        let mut found = None;
        let build = |lua: &mlua::Lua, _: &crate::hostcall::TurnHost<'_>| {
            crate::dispatch::new_event(lua, lua.create_table()?, &[])
        };
        let mut answer = |id: &str, value: mlua::Value, _: &crate::hostcall::TurnHost<'_>| {
            found =
                crate::dispatch::answer_of::<mod_api::InteractionResult>(id, "use_item", &value);
            true
        };
        part.dispatch_replace(
            "use_item",
            crate::hostcall::TurnHost::None,
            &build,
            &mut answer,
        )
        .unwrap();
        found
    }

    /// A mod with one block, one block entity type on it and that type's part (`on_chunk_unloaded`
    /// logs a line), as `first_mod`.
    const PRESS: &str = "horizon.block{id = \"first_mod:press_block\", template = \"minecraft:stone\"}\n\
        horizon.block_entity_type{id = \"first_mod:press\", blocks = {\"first_mod:press_block\"}}\n\
        horizon.block_entity_part(\"first_mod:press\", {on_chunk_unloaded = function() horizon.log(\"unloaded\") end})\n";

    /// Runs the block entity part's `on_chunk_unloaded` turn with no host (as the generated method
    /// does for a hook that gets none), and returns how many times its turn committed.
    fn chunk_unloaded_commits(part: &crate::host::LuaBlockEntityPart) -> usize {
        let mut committed = 0;
        let build = |lua: &mlua::Lua, _: &crate::hostcall::TurnHost<'_>| {
            crate::dispatch::new_event(lua, lua.create_table()?, &[])
        };
        part.dispatch_turns(
            "on_chunk_unloaded",
            crate::hostcall::TurnHost::None,
            &|| false,
            &build,
            &mut |_, _, _| {
                committed += 1;
                true
            },
        )
        .unwrap();
        committed
    }

    #[test]
    fn a_block_entity_type_is_requested_before_its_part() {
        let (_, keys) = requested(&[("a_first", "first_mod", PRESS)]);
        assert_eq!(
            keys.asked,
            [
                "block first_mod:press_block",
                "block entity type first_mod:press",
                "block entity part first_mod:press",
            ]
        );
        assert_eq!(
            keys.block_entity_types,
            [(
                "first_mod:press".to_owned(),
                vec!["first_mod:press_block".to_owned()]
            )]
        );
    }

    #[test]
    fn a_block_entity_part_is_a_block_part_that_subscribes_to_no_block_hook() {
        use mod_api::{BlockEntityPart, BlockPart};
        let (_, keys) = requested(&[("a_first", "first_mod", PRESS)]);
        let part = &keys.block_entity_parts[0];
        assert!(part.block_subscribed().is_empty());
        assert_eq!(part.block_entity_subscribed().len(), 1);
    }

    #[test]
    fn a_block_entity_part_runs_only_its_own_mods_handler() {
        let (host, keys) = requested(&[
            ("a_first", "first_mod", PRESS),
            (
                "b_second",
                "second_mod",
                "horizon.block{id = \"second_mod:x\", template = \"minecraft:stone\"}\n\
                 horizon.block_entity_type{id = \"second_mod:press\", blocks = {\"second_mod:x\"}}\n\
                 horizon.block_entity_part(\"second_mod:press\", {on_chunk_unloaded = function() horizon.log(\"second ran\") end})",
            ),
        ]);
        assert_eq!(keys.block_entity_parts.len(), 2);
        assert_eq!(chunk_unloaded_commits(&keys.block_entity_parts[0]), 1);
        assert_eq!(host.mods()[0].log_lines(), ["unloaded"]);
        assert!(host.mods()[1].log_lines().is_empty());
    }

    #[test]
    fn a_failing_block_entity_part_handler_voids_its_turn() {
        let (_, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.block{id = \"first_mod:press_block\", template = \"minecraft:stone\"}\n\
             horizon.block_entity_type{id = \"first_mod:press\", blocks = {\"first_mod:press_block\"}}\n\
             horizon.block_entity_part(\"first_mod:press\", {on_chunk_unloaded = function() error(\"boom\") end})",
        )]);
        assert_eq!(chunk_unloaded_commits(&keys.block_entity_parts[0]), 0);
    }

    #[test]
    fn a_refused_block_entity_type_refuses_only_its_mod() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(root.path(), "a_first", "first_mod", PRESS);
        mod_folder(
            root.path(),
            "b_second",
            "second_mod",
            "horizon.block{id = \"second_mod:x\", template = \"minecraft:stone\"}\n\
             horizon.block_entity_type{id = \"second_mod:press\", blocks = {\"second_mod:x\"}}\n\
             horizon.block_entity_part(\"second_mod:press\", {on_chunk_unloaded = function() end})",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let mut keys = RecordingKeys {
            refuse: Some("first_mod:press".to_owned()),
            ..RecordingKeys::default()
        };
        host.request_keys(&mut keys);
        assert_eq!(answered_blocks(&host), ["second_mod:x"]);
        assert!(
            !keys
                .asked
                .contains(&"block entity part first_mod:press".to_owned())
        );
        assert_eq!(keys.block_entity_parts.len(), 1);
    }

    #[test]
    fn a_refused_block_entity_part_refuses_only_its_mod() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(root.path(), "a_first", "first_mod", PRESS);
        mod_folder(
            root.path(),
            "b_second",
            "second_mod",
            "horizon.block{id = \"second_mod:x\", template = \"minecraft:stone\"}\n\
             horizon.block_entity_type{id = \"second_mod:press\", blocks = {\"second_mod:x\"}}\n\
             horizon.block_entity_part(\"second_mod:press\", {on_chunk_unloaded = function() end})",
        );
        let (mods, _) = load_mods(root.path());
        let host = LuaHost::new(mods);
        let mut keys = RecordingKeys {
            refuse_part: Some("first_mod:press".to_owned()),
            ..RecordingKeys::default()
        };
        host.request_keys(&mut keys);
        assert_eq!(answered_blocks(&host), ["second_mod:x"]);
        assert_eq!(keys.block_entity_parts.len(), 2);
    }

    #[test]
    fn a_block_part_an_item_part_and_a_block_entity_part_of_one_mod_all_work() {
        let (host, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.block{id = \"first_mod:press_block\", template = \"minecraft:stone\"}\n\
             horizon.item{id = \"first_mod:wand\", template = \"minecraft:stick\"}\n\
             horizon.block_entity_type{id = \"first_mod:press\", blocks = {\"first_mod:press_block\"}}\n\
             horizon.block_part(\"first_mod:press_block\", {can_survive = function() return true end})\n\
             horizon.item_part(\"first_mod:wand\", {use_item = function() return \"success\" end})\n\
             horizon.block_entity_part(\"first_mod:press\", {on_chunk_unloaded = function() horizon.log(\"unloaded\") end})",
        )]);
        assert_eq!(keys.parts.len(), 1);
        assert_eq!(keys.item_parts.len(), 1);
        assert_eq!(keys.block_entity_parts.len(), 1);
        assert!(matches!(
            use_item_answer(&keys.item_parts[0]),
            Some(mod_api::InteractionResult::Success)
        ));
        assert_eq!(survives(&keys.parts[0]), Some(true));
        assert_eq!(chunk_unloaded_commits(&keys.block_entity_parts[0]), 1);
        assert_eq!(host.mods()[0].log_lines(), ["unloaded"]);
    }

    #[test]
    fn an_item_part_answers_from_its_own_handler_and_the_others_are_not_called() {
        let (host, keys) = requested(&[
            (
                "a_first",
                "first_mod",
                "horizon.item_part(\"first_mod:wand\", {use_item = function(e) horizon.log(\"wand used\") return \"success\" end})",
            ),
            (
                "b_second",
                "second_mod",
                "horizon.item_part(\"second_mod:x\", {use_item = function() horizon.log(\"second ran\") return \"fail\" end})",
            ),
        ]);
        assert_eq!(keys.item_parts.len(), 2);
        assert!(matches!(
            use_item_answer(&keys.item_parts[0]),
            Some(mod_api::InteractionResult::Success)
        ));
        assert_eq!(host.mods()[0].log_lines(), ["wand used"]);
        assert!(host.mods()[1].log_lines().is_empty());
    }

    #[test]
    fn a_failing_item_part_handler_voids_its_answer() {
        let (_, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.item_part(\"first_mod:wand\", {use_item = function() error(\"boom\") end})",
        )]);
        assert!(use_item_answer(&keys.item_parts[0]).is_none());
    }

    #[test]
    fn item_parts_with_one_handler_set_share_one_subscription() {
        let (_, keys) = requested(&[
            (
                "a_first",
                "first_mod",
                "horizon.item_part(\"first_mod:wand\", {use_item = function() end})\n\
                 horizon.item_part(\"first_mod:rod\", {use_item = function() end})",
            ),
            (
                "b_second",
                "second_mod",
                "horizon.item_part(\"second_mod:x\", {use_item = function() end, hurt_enemy = function() end})",
            ),
        ]);
        assert!(std::ptr::eq(
            keys.item_parts[0].subscribed(),
            keys.item_parts[1].subscribed()
        ));
        assert!(!std::ptr::eq(
            keys.item_parts[0].subscribed(),
            keys.item_parts[2].subscribed()
        ));
        assert_eq!(keys.item_parts[2].subscribed().len(), 2);
    }

    #[test]
    fn a_block_part_and_a_hook_handler_of_one_mod_both_work() {
        let (host, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.on(\"server_tick\", function() horizon.log(\"tick\") end)\n\
             horizon.block_part(\"first_mod:lamp\", {can_survive = function() return false end})",
        )]);
        assert_eq!(host.subscribed_slice().len(), 1);
        assert_eq!(survives(&keys.parts[0]), Some(false));
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
