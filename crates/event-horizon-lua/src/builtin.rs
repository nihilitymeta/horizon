//! The built-in mod that runs the Lua mods in `<game_dir>/lua-mods/`: the one entry the game starts
//! with, and the package the loader checks for it beside the executable.

use crate::assets;
use crate::discover::{self, ScannedMod};
use crate::host::{LuaHost, LuaMod, both_side_records, collect_load_notices};
use crate::package;
use crate::provider::LuaProvider;
use crate::settings::{Role, Schema, Settings};
use mod_api::__private::native::BuiltMod;
use mod_api::ModEntry;
use mod_api::Side;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// The dev paths the environment names: `EH_LUA_MODS`, a list of paths separated by the system's
/// path separator. Empty entries are dropped; the scan refuses more than `MAX_DEV_PATHS`.
pub fn env_dev_paths(value: Option<&OsStr>) -> Vec<PathBuf> {
    value
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

/// Scans `<game_dir>/lua-mods/` and `dev_paths`, and loads each mod that passes. A mod that fails
/// is refused and the others still load. A missing folder is fine: the entry then subscribes to
/// nothing. `dedicated` is the game's role: on a dedicated server a client-only mod is not loaded.
pub fn builtin(game_dir: &Path, dev_paths: &[PathBuf], dedicated: bool) -> Loaded {
    let (mods, refused, disabled, notices, skipped, packs) =
        load_mods(game_dir, dev_paths, dedicated);
    let loaded = mods.len();
    // The both-side Lua mods are announced on `horizon:mod_list` and recorded in a world beside the
    // compiled mods. This runs before the game builds its announcement (`content_overlay`).
    let _previous = horizon_mod_package::runtime::set_declared(
        both_side_records(&mods)
            .into_iter()
            .map(|m| (m.id, m.version))
            .collect(),
    );
    // The content packs of the mods that loaded (`assets::publish`): the game selects them at its
    // startup and at each reload.
    let _previous = horizon_mod_package::runtime::set_content_packs(packs);
    let host = LuaHost::new(mods)
        .with_report(refused.clone())
        .with_disabled(disabled.clone(), notices)
        .with_skipped(skipped);
    // The Mods screen reads the mods through this provider. The first registration wins, so a
    // second `builtin` in the same process (tests) is refused and changes nothing.
    let provider: &'static LuaProvider = Box::leak(Box::new(LuaProvider::new(
        host.shared_mods(),
        game_dir,
        disabled,
        &refused,
    )));
    let _ = horizon_hooks::mod_entries::register(provider);
    Loaded {
        entry: ModEntry::new(package::ID, Box::new(host)),
        loaded,
        refused,
    }
}

/// The settings of one scanned mod: its schema from the manifest, and its values from
/// `<game_dir>/config/<id>/settings.toml`. The schema was checked when the package was read, so a
/// refusal here is a failed check of the same text. The role is the server's for every process:
/// `Role::PureClient` is not passed by any caller yet, so a server-side setting stays editable.
fn load_settings(game_dir: &Path, scanned: &ScannedMod) -> Result<Settings, String> {
    let schema =
        Schema::parse(&scanned.package.manifest().settings).map_err(|fault| fault.reason())?;
    Ok(Settings::load(
        game_dir,
        &scanned.found.id,
        schema,
        Role::ServerRuns,
    ))
}

/// Lets the built-in package declare content in `m`'s namespace (its id). The game reads a
/// package's namespaces when it starts, so a Lua mod's namespace, found in its folder, is granted
/// here, before the game dispatches the `blocks` and `items` hooks.
fn grant_namespace(m: &LuaMod) -> Result<(), String> {
    mod_api::__private::grant_namespace(package::ID, m.id())
        .map_err(|reason| format!("cannot declare content in its namespace: {reason}"))
}

/// The mods under `game_dir` that load, one line per folder or mod that does not, the ids the
/// enabled file turned off, the warnings about that file, one line per client-only mod a
/// dedicated server skips, and the content packs of the loaded mods. Nothing is logged here: this
/// runs before the game's logger is up, and `LuaHost::register` reports them. A refusal line
/// carries the translation key after the English reason; a skip line carries
/// `discover::SKIPPED_CLIENT_ONLY`. A mod whose content is refused still loads: its notice is kept
/// with the mod (`LuaMod::push_notice`), and it contributes no pack.
#[allow(clippy::type_complexity)]
fn load_mods(
    game_dir: &Path,
    dev_paths: &[PathBuf],
    dedicated: bool,
) -> (
    Vec<LuaMod>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<PathBuf>,
) {
    let scan = discover::scan_on(game_dir, dev_paths, dedicated);
    let skipped: Vec<String> = scan
        .skipped
        .iter()
        .map(|d| {
            format!(
                "{}: not loaded on a dedicated server, it is client-only ({})",
                d.id,
                discover::SKIPPED_CLIENT_ONLY
            )
        })
        .collect();
    let mut refused: Vec<String> = scan
        .refused
        .iter()
        .map(|r| {
            let name = r
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{name}: {} ({})", r.fault.reason(), r.fault.key())
        })
        .collect();
    let disabled: Vec<String> = scan.disabled.iter().map(|d| d.id.clone()).collect();
    let mut mods: Vec<LuaMod> = Vec::new();
    let mut packs: Vec<PathBuf> = Vec::new();
    for scanned in &scan.loaded {
        // The settings are read before the mod declares, so `horizon.setting` sees the file's values.
        // Their warnings are kept for `LuaHost::report`, as the declaration's are.
        let (settings, settings_notices) =
            collect_load_notices(|| load_settings(game_dir, scanned));
        let settings = match settings {
            Ok(settings) => Arc::new(settings),
            Err(reason) => {
                refused.push(format!("{}: {reason}", scanned.found.id));
                continue;
            }
        };
        match LuaMod::load_package(&scanned.found, &scanned.package, settings, settings_notices) {
            Ok(mut m) => match grant_namespace(&m) {
                Ok(()) => {
                    match assets::publish(game_dir, &scanned.package) {
                        Ok(Some(pack)) => packs.push(pack),
                        Ok(None) => {}
                        Err(error) => m.push_notice(
                            &scanned.found.id,
                            format!(
                                "{}: its assets, data and lang are not loaded: {} ({})",
                                scanned.found.id,
                                error.reason(),
                                assets::KEY
                            ),
                        ),
                    }
                    mods.push(m);
                }
                Err(reason) => refused.push(format!("{}: {reason}", scanned.found.id)),
            },
            Err(reason) => refused.push(format!("{}: {reason}", scanned.found.id)),
        }
    }
    (mods, refused, disabled, scan.notices, skipped, packs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mods and refusals of a scan with no dev paths (the loader's pair, without the notes).
    fn load_mods_pair(root: &Path) -> (Vec<LuaMod>, Vec<String>) {
        let (mods, refused, _, _, _, _) = load_mods(root, &[], false);
        (mods, refused)
    }

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

    /// Writes `<root>/lua-mods/<name>.hlua`, a zip archive mod with `id`, `version` and `side`.
    fn archive_on(root: &Path, name: &str, id: &str, version: &str, side: &str) {
        use std::io::Write;
        let dir = root.join(discover::DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join(format!("{name}.hlua"))).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let manifest = format!("id = \"{id}\"\nversion = \"{version}\"\nside = \"{side}\"\n");
        writer
            .start_file(discover::MANIFEST, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer
            .start_file(discover::ENTRY, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"return {}\n").unwrap();
        writer.finish().unwrap();
    }

    /// The records the announcement and the saved worlds take from a scan of `root`.
    fn announced(root: &Path) -> Vec<(String, String)> {
        let (mods, _) = load_mods_pair(root);
        both_side_records(&mods)
            .into_iter()
            .map(|m| (m.id, m.version))
            .collect()
    }

    #[test]
    fn only_loaded_both_side_mods_are_announced_in_load_order() {
        let root = tempfile::tempdir().unwrap();
        mod_folder_on(root.path(), "alpha", "alpha_mod", "both", "return {}\n");
        mod_folder_on(root.path(), "beta", "beta_mod", "client", "return {}\n");
        mod_folder_on(root.path(), "gamma", "gamma_mod", "server", "return {}\n");
        mod_folder_on(root.path(), "delta", "delta_mod", "both", "return {}\n");
        mod_folder_on(
            root.path(),
            "epsilon",
            "epsilon_mod",
            "sideways",
            "return {}\n",
        );
        archive_on(root.path(), "zeta", "zeta_mod", "4.2.0", "both");
        archive_on(root.path(), "eta", "eta_mod", "5.0.0", "client");
        let (_, refused, disabled, _, _, _) = load_mods(root.path(), &[], false);
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(disabled.is_empty());
        assert_eq!(
            announced(root.path()),
            [
                ("alpha_mod".to_string(), "1.0.0".to_string()),
                ("delta_mod".to_string(), "1.0.0".to_string()),
                ("zeta_mod".to_string(), "4.2.0".to_string()),
            ]
        );
    }

    #[test]
    fn a_mod_turned_off_in_enabled_toml_is_not_announced_and_turning_it_on_adds_it_back() {
        let root = tempfile::tempdir().unwrap();
        mod_folder_on(root.path(), "alpha", "alpha_mod", "both", "return {}\n");
        mod_folder_on(root.path(), "delta", "delta_mod", "both", "return {}\n");
        let enabled = root.path().join(discover::DIR).join(crate::enabled::FILE);
        std::fs::write(&enabled, "[mods]\ndelta_mod = false\n").unwrap();
        let ids: Vec<_> = announced(root.path()).into_iter().map(|m| m.0).collect();
        assert_eq!(ids, ["alpha_mod"]);
        std::fs::write(&enabled, "[mods]\ndelta_mod = true\n").unwrap();
        let ids: Vec<_> = announced(root.path()).into_iter().map(|m| m.0).collect();
        assert_eq!(ids, ["alpha_mod", "delta_mod"]);
    }

    #[test]
    fn no_lua_mods_announce_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(announced(root.path()).is_empty());
        mod_folder_on(root.path(), "beta", "beta_mod", "client", "return {}\n");
        assert!(announced(root.path()).is_empty());
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
        let built = builtin(root.path(), &[], false);
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
        let built = builtin(root.path(), &[], false);
        assert_eq!(built.loaded, 1);
        assert_eq!(built.refused.len(), 1);
        assert!(built.refused[0].starts_with("bad_mod: "));
        assert!(built.refused[0].len() > "bad_mod: ".len());
    }

    #[test]
    fn a_missing_lua_mods_folder_is_fine() {
        let root = tempfile::tempdir().unwrap();
        let built = builtin(root.path(), &[], false);
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
        let (mods, refused) = load_mods_pair(root.path());
        assert!(mods.is_empty());
        assert_eq!(refused.len(), 1);
        let host = LuaHost::new(mods).with_report(refused.clone());
        assert_eq!(host.report_lines(), refused.as_slice());
    }

    #[test]
    fn refused_folders_from_the_scan_are_reported() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(discover::DIR).join("no_manifest")).unwrap();
        let built = builtin(root.path(), &[], false);
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
        fluids: Vec<(String, mod_api::WorldFluid)>,
        fluid_parts: Vec<crate::host::LuaFluidPart>,
        entity_parts: Vec<crate::host::LuaEntityPart>,
        menu_parts: Vec<crate::host::LuaMenuPart>,
    }

    impl crate::host::DeclaredKeys for RecordingKeys {
        fn menu_part(&mut self, id: &str, part: crate::host::LuaMenuPart) -> Result<(), String> {
            self.asked.push(format!("menu part {id}"));
            self.menu_parts.push(part);
            Ok(())
        }

        fn entity_part(
            &mut self,
            id: &str,
            part: crate::host::LuaEntityPart,
        ) -> Result<(), String> {
            self.asked.push(format!("entity part {id}"));
            self.entity_parts.push(part);
            Ok(())
        }

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

        fn world_fluid(&mut self, id: &str, fluid: &mod_api::WorldFluid) -> Result<(), String> {
            self.asked.push(format!("world fluid {id}"));
            self.fluids.push((id.to_owned(), *fluid));
            self.answer(id)
        }

        fn fluid_part(&mut self, id: &str, part: crate::host::LuaFluidPart) -> Result<(), String> {
            self.asked.push(format!("fluid part {id}"));
            self.fluid_parts.push(part);
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
        let (mods, refused) = load_mods_pair(root.path());
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
        let (mods, refused) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
        let (mods, refused) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
    fn declared_commands_are_answered_for_every_server_mod_in_order() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_first",
            "first_mod",
            "horizon.command{usage = \"ban <who:word>\", level = 2}\n\
             horizon.command{usage = \"hush\"}\n",
        );
        mod_folder_on(
            root.path(),
            "b_client",
            "client_mod",
            "client",
            "horizon.command{usage = \"unseen\"}\n",
        );
        mod_folder_on(
            root.path(),
            "c_server",
            "server_mod",
            "server",
            "horizon.command{usage = \"srv <n:integer>\", level = 4}\n",
        );
        let (mods, refused) = load_mods_pair(root.path());
        assert!(refused.is_empty(), "got {refused:?}");
        let host = LuaHost::new(mods);
        let mut commands = Vec::new();
        host.declared_commands(&mut commands);
        let got: Vec<(&str, u8)> = commands
            .iter()
            .map(|c| (c.usage.as_str(), c.level))
            .collect();
        assert_eq!(
            got,
            [("ban <who:word>", 2), ("hush", 0), ("srv <n:integer>", 4)]
        );
        assert!(subscribes_to(&host, "register_commands"));
    }

    #[test]
    fn a_client_only_mod_with_commands_does_not_subscribe_to_the_server_hook() {
        let root = tempfile::tempdir().unwrap();
        mod_folder_on(
            root.path(),
            "a_client",
            "client_mod",
            "client",
            "horizon.command{usage = \"unseen\"}\n",
        );
        let (mods, _) = load_mods_pair(root.path());
        let host = LuaHost::new(mods);
        let mut commands = Vec::new();
        host.declared_commands(&mut commands);
        assert!(commands.is_empty());
        assert!(!subscribes_to(&host, "register_commands"));
    }

    #[test]
    fn a_command_without_a_run_mod_command_handler_is_warned_once_at_load() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(
            root.path(),
            "a_nohandler",
            "nohandler_mod",
            "horizon.command{usage = \"lonely\"}\n",
        );
        crate::host::tests::install_capture();
        let (mods, _) = load_mods_pair(root.path());
        let host = LuaHost::new(mods);
        host.report();
        let lines = crate::host::tests::captured_with_prefix("nohandler_mod: declares commands");
        assert_eq!(lines.len(), 1, "got {lines:?}");
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
        let (mods, _) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
        let built = builtin(root.path(), &[], false);
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
        let (mods, _) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
    fn a_world_fluid_is_requested_before_its_fluid_part() {
        let (_, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.world_fluid{id = \"first_mod:oil\"}\n\
             horizon.fluid_part(\"first_mod:oil\", {fluid_rules = function() end})",
        )]);
        assert_eq!(
            keys.asked,
            ["world fluid first_mod:oil", "fluid part first_mod:oil"]
        );
        assert_eq!(keys.fluids.len(), 1);
        assert_eq!(keys.fluid_parts.len(), 1);
    }

    #[test]
    fn a_menu_part_is_attached_to_the_menu_it_names_in_any_namespace() {
        let (_, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.menu_part(\"other:press_menu\", {menu_still_valid = function() end})",
        )]);
        assert_eq!(keys.asked, ["menu part other:press_menu"]);
        assert_eq!(keys.menu_parts.len(), 1);
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
        let (mods, _) = load_mods_pair(root.path());
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
        let (mods, _) = load_mods_pair(root.path());
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
        let built = builtin(root.path(), &[], false);
        assert_eq!(built.loaded, 1);
        assert_eq!(built.refused.len(), 1);
        // The package reader refuses the id before the mod has one, so the line names the folder.
        assert!(
            built.refused[0].starts_with("a_reserved: "),
            "got {:?}",
            built.refused
        );
        assert!(built.refused[0].contains("reserved"));
        assert!(built.refused[0].ends_with("(lua_mod.refused.reserved_id)"));
    }

    /// A level double for the hooks that take one; the container hook reads nothing from it.
    struct Level;
    impl mod_api::LevelHost for Level {}

    /// A loop context with no registries or settings, for the item hooks' `context` argument.
    struct NoReader;
    impl mod_api::LoopReader for NoReader {
        fn registry(&self, _name: &str) -> Option<std::sync::Arc<[String]>> {
            None
        }
        fn setting(&self, _mod_id: &str, _key: &str) -> Option<String> {
            None
        }
    }

    /// The thread's item table with `minecraft:stick` in it, as the game's load gives it. The
    /// returned host keeps the table installed while it lives.
    fn stick_table() -> mod_api::testing::FakeHost {
        let mut host = mod_api::testing::FakeHost::new();
        host.registries
            .resolve_items([mod_api::Identifier::parse("minecraft:stick").unwrap()])
            .unwrap();
        host
    }

    #[test]
    fn global_item_part_grindstone_handler_sees_the_stack_and_its_answer_is_taken() {
        use mod_api::ItemPart;
        let (host, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.item{id = \"first_mod:wand\", template = \"minecraft:stick\"}\n\
             horizon.item_part(\"first_mod:wand\", {can_grindstone_repair = function(e) horizon.log(e.stack.id .. \" \" .. e.stack.count) e.can_repair = false end})",
        )]);
        let _table = stick_table();
        let stick =
            mod_api::ItemStack::new(mod_api::__private::item_key("minecraft:stick").unwrap(), 3)
                .unwrap();
        let context = mod_api::OwningLoop::new(mod_api::LoopSide::Server, &NoReader);
        let mut event = mod_api::CanGrindstoneRepairEvent {
            stack: &stick,
            can_repair: true,
        };
        keys.item_parts[0].can_grindstone_repair(&context, &mut event);
        assert!(!event.can_repair);
        assert_eq!(host.mods()[0].log_lines(), ["minecraft:stick 3"]);
    }

    /// The drops a `container_loot_filled` handler leaves on a list of one stick.
    fn loot_after(handler: &str) -> Vec<(String, u32)> {
        let (host, _keys) = requested(&[(
            "a_first",
            "first_mod",
            &format!("horizon.on(\"container_loot_filled\", function(e) {handler} end)"),
        )]);
        let _table = stick_table();
        let context =
            mod_api::ContainerLootFilled::from(mod_api::__private::built::ContainerLootFilled {
                table: "test:chest",
                dimension: "minecraft:overworld",
                origin: [0.5, 64.5, 0.5],
                block: Some([0, 64, 0]),
                entity: None,
                opener: None,
            });
        let mut drops = vec![
            mod_api::ItemStack::new(mod_api::__private::item_key("minecraft:stick").unwrap(), 1)
                .unwrap(),
        ];
        let mut event = mod_api::ContainerLootFilledEvent {
            context: &context,
            drops: &mut drops,
        };
        let level = Level;
        mod_api::Hooks::container_loot_filled(&host, &level, &mut event);
        drops
            .iter()
            .map(|stack| {
                (
                    mod_api::__private::item_key_id(stack.item())
                        .unwrap()
                        .to_owned(),
                    stack.count(),
                )
            })
            .collect()
    }

    #[test]
    fn container_loot_handler_adds_a_stack_and_it_reaches_the_output() {
        let drops = loot_after(
            "e.drops = {{id = \"minecraft:stick\", count = 1}, {id = \"minecraft:stick\", count = 4}}",
        );
        assert_eq!(
            drops,
            [
                ("minecraft:stick".to_owned(), 1),
                ("minecraft:stick".to_owned(), 4)
            ]
        );
    }

    #[test]
    fn container_loot_handler_with_a_bad_stack_is_refused_whole_and_the_previous_list_is_kept() {
        let drops = loot_after(
            "e.drops = {{id = \"minecraft:stick\", count = 5}, {id = \"minecraft:stick\", count = 0}}",
        );
        assert_eq!(drops, [("minecraft:stick".to_owned(), 1)]);
    }

    /// Runs `can_survive` on a part with event data `x`, `y`, `z`, and reads its answer.
    fn survives_with_fields(part: &crate::host::LuaBlockPart) -> Option<bool> {
        let mut found = None;
        let build = |lua: &mlua::Lua, _: &crate::hostcall::TurnHost<'_>| {
            let data = lua.create_table()?;
            data.raw_set("x", 1)?;
            data.raw_set("y", 2)?;
            data.raw_set("z", 3)?;
            crate::dispatch::new_event(lua, data, &[])
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

    /// Mean nanoseconds per call of `f` over `n` calls; `f` must answer true every time.
    fn ns_per_call(n: u32, mut f: impl FnMut() -> bool) -> f64 {
        let start = std::time::Instant::now();
        for _ in 0..n {
            assert!(
                std::hint::black_box(f()),
                "the handler's answer did not arrive"
            );
        }
        start.elapsed().as_nanos() as f64 / f64::from(n)
    }

    /// Cost of one block part delivery (#74, report 74-parts-explore2 section B). Prints a table of
    /// ns per call; it asserts only that the answers arrive, never a time.
    /// Run: `heavy cargo nextest run -p event-horizon-lua --run-ignored only hot_path_bench --nocapture`
    #[test]
    #[ignore]
    fn hot_path_bench() {
        const N: u32 = 200_000;
        let (_host, keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.block_part(\"first_mod:lamp\", {can_survive = function() return true end})",
        )]);
        let (_fields_host, fields_keys) = requested(&[(
            "a_first",
            "first_mod",
            "horizon.block_part(\"first_mod:lamp\", {can_survive = function(event) return event.x + event.y + event.z == 6 end})",
        )]);
        let idle = LuaHost::new(Vec::new());

        let subscribed = ns_per_call(N, || survives(&keys.parts[0]) == Some(true));
        let unsubscribed = ns_per_call(N, || !idle.has_handlers("can_survive"));
        let three_fields = ns_per_call(N, || {
            survives_with_fields(&fields_keys.parts[0]) == Some(true)
        });

        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
        eprintln!("\nhot path bench (#74): profile {profile}, N = {N}, nproc = {cpus}");
        eprintln!("{:<58} {:>12}", "case", "ns/call");
        eprintln!(
            "{:<58} {:>12.1}",
            "1. one mod subscribed, can_survive returns true", subscribed
        );
        eprintln!(
            "{:<58} {:>12.1}",
            "2. no subscribed mod (has_handlers gate only)", unsubscribed
        );
        eprintln!(
            "{:<58} {:>12.1}",
            "3. one mod subscribed, handler reads three fields", three_fields
        );
    }

    /// A folder mod `name` with `manifest` text and the given script.
    fn folder_with(root: &Path, name: &str, manifest: &str, script: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(discover::MANIFEST), manifest).unwrap();
        std::fs::write(dir.join(discover::ENTRY), script).unwrap();
        dir
    }

    /// The skip lines of a scan of `root` with `dedicated` set, and the loaded mod ids.
    fn skipped_and_loaded(root: &Path, dedicated: bool) -> (Vec<String>, Vec<String>) {
        let (mods, _, _, _, skipped, _) = load_mods(root, &[], dedicated);
        (skipped, mods.iter().map(|m| m.id().to_string()).collect())
    }

    #[test]
    fn on_a_dedicated_server_a_client_only_mod_is_skipped_with_its_key_and_never_loads() {
        let root = tempfile::tempdir().unwrap();
        mod_folder_on(
            root.path(),
            "a_client",
            "client_mod",
            "client",
            "horizon.block{id = \"client_mod:lamp\", template = \"minecraft:stone\"}\n",
        );
        mod_folder_on(
            root.path(),
            "b_server",
            "server_mod",
            "server",
            "return {}\n",
        );
        mod_folder_on(root.path(), "c_both", "both_mod", "both", "return {}\n");
        let (mods, refused, _, _, skipped, _) = load_mods(root.path(), &[], true);
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(
            skipped,
            [
                "client_mod: not loaded on a dedicated server, it is client-only (lua_mod.skipped.client_only_on_server)"
            ]
        );
        let mut loaded: Vec<&str> = mods.iter().map(|m| m.id()).collect();
        loaded.sort();
        assert_eq!(loaded, ["both_mod", "server_mod"]);
        // Not announced, and its block is not declared.
        let announced: Vec<String> = both_side_records(&mods).into_iter().map(|m| m.id).collect();
        assert_eq!(announced, ["both_mod"]);
        assert!(!answered_blocks(&LuaHost::new(mods)).contains(&"client_mod:lamp".to_string()));
    }

    #[test]
    fn off_a_dedicated_server_every_side_loads_and_nothing_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        mod_folder_on(
            root.path(),
            "a_client",
            "client_mod",
            "client",
            "return {}\n",
        );
        mod_folder_on(
            root.path(),
            "b_server",
            "server_mod",
            "server",
            "return {}\n",
        );
        mod_folder_on(root.path(), "c_both", "both_mod", "both", "return {}\n");
        let (skipped, loaded) = skipped_and_loaded(root.path(), false);
        assert!(skipped.is_empty());
        let mut sorted = loaded.clone();
        sorted.sort();
        assert_eq!(sorted, ["both_mod", "client_mod", "server_mod"]);
    }

    #[test]
    fn a_dev_client_only_mod_obeys_the_same_rule_on_a_dedicated_server() {
        let root = tempfile::tempdir().unwrap();
        let dev_root = tempfile::tempdir().unwrap();
        let dev = folder_with(
            dev_root.path(),
            "dev",
            "id = \"dev_client\"\nversion = \"1.0.0\"\nside = \"client\"\n",
            "return {}\n",
        );
        let (mods, _, _, _, skipped, _) = load_mods(root.path(), std::slice::from_ref(&dev), true);
        assert!(mods.is_empty());
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].starts_with("dev_client: "));
        let (mods, _, _, _, skipped, _) = load_mods(root.path(), std::slice::from_ref(&dev), false);
        assert_eq!(mods.len(), 1);
        assert!(skipped.is_empty());
    }

    #[test]
    fn a_mod_that_needs_a_skipped_client_only_mod_is_refused_as_disabled() {
        let root = tempfile::tempdir().unwrap();
        mod_folder_on(root.path(), "a_client", "base_mod", "client", "return {}\n");
        folder_with(
            &root.path().join(discover::DIR),
            "b_needs",
            "id = \"needs_mod\"\nversion = \"1.0.0\"\nside = \"both\"\n[dependencies]\nbase_mod = \"1\"\n",
            "return {}\n",
        );
        let (mods, refused, _, _, skipped, _) = load_mods(root.path(), &[], true);
        assert!(
            mods.is_empty(),
            "the base is skipped and its dependent refused: nothing loads"
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].starts_with("b_needs: "));
        assert!(refused[0].ends_with("(lua_mod.refused.dependency_disabled)"));
    }

    #[test]
    fn a_settings_file_is_read_when_the_mod_loads_and_the_host_shares_its_values() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(discover::DIR).join("tuned");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(discover::MANIFEST),
            "id = \"tuned_mod\"\nversion = \"1.0.0\"\nside = \"both\"\n\n\
             [[settings]]\nkey = \"level\"\ntype = \"slider\"\nlabel = \"Level\"\n\
             min = 0\nmax = 10\nstep = 1\ninteger = true\ndefault = 1\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(discover::ENTRY),
            "horizon.on(\"block_receive_game\", function()\n\
             horizon.log(\"level \" .. horizon.setting(\"level\"))\nend)\n",
        )
        .unwrap();
        let config = root
            .path()
            .join(crate::settings::CONFIG_DIR)
            .join("tuned_mod");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(crate::settings::FILE), "level = 6\n").unwrap();

        let (mods, refused, _, _, _, _) = load_mods(root.path(), &[], false);
        assert!(refused.is_empty(), "{refused:?}");
        let host = LuaHost::new(mods);
        let shared = host.settings("tuned_mod").expect("loaded");
        assert!(Arc::ptr_eq(&shared, host.mods()[0].settings()));
        assert_eq!(shared.get("level"), Some(crate::settings::Value::Int(6)));
        host.fire("block_receive_game");
        assert_eq!(host.mods()[0].log_lines(), ["level 6"]);
    }

    /// Writes `<root>/lua-mods/<name>/`: a manifest for `id` with `side`, a script, and `files`.
    fn mod_with_files(root: &Path, name: &str, id: &str, side: &str, files: &[(&str, &[u8])]) {
        let dir = root.join(discover::DIR).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(discover::MANIFEST),
            format!("id = \"{id}\"\nversion = \"1.0.0\"\nside = \"{side}\"\nname = \"{id}\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join(discover::ENTRY), "return {}\n").unwrap();
        for (rel, bytes) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }

    #[test]
    fn a_mods_content_is_published_as_a_pack_and_a_refused_one_still_loads() {
        let root = tempfile::tempdir().unwrap();
        mod_with_files(
            root.path(),
            "good",
            "good",
            "both",
            &[("assets/good/textures/a.png", b"a")],
        );
        mod_with_files(
            root.path(),
            "bad",
            "bad",
            "both",
            &[("assets/bad/textures/Upper.png", b"b")],
        );
        let (mods, refused, _, _, _, packs) = load_mods(root.path(), &[], false);
        assert_eq!(refused, Vec::<String>::new());
        assert_eq!(mods.len(), 2, "a refused content must not refuse the mod");
        assert_eq!(packs.len(), 1, "{packs:?}");
        assert_eq!(
            packs[0].parent(),
            Some(
                root.path()
                    .join(".horizon-cache")
                    .join("lua-mods")
                    .as_path()
            )
        );
        assert!(
            packs[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("lua_good-")
        );
        let bad = mods.iter().find(|m| m.id() == "bad").unwrap();
        let notices = bad.notices();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices[0].0, "bad");
        assert!(
            notices[0].1.contains("(lua_mod.content.refused)"),
            "{notices:?}"
        );
        assert!(
            mods.iter()
                .find(|m| m.id() == "good")
                .unwrap()
                .notices()
                .is_empty()
        );
    }

    #[test]
    fn a_disabled_mod_and_a_skipped_client_only_mod_publish_nothing() {
        let root = tempfile::tempdir().unwrap();
        mod_with_files(
            root.path(),
            "off",
            "off",
            "both",
            &[("assets/off/a.png", b"a")],
        );
        mod_with_files(
            root.path(),
            "client",
            "client_mod",
            "client",
            &[("assets/client_mod/a.png", b"a")],
        );
        crate::enabled::write(root.path(), "off", false).unwrap();
        let (mods, _, disabled, _, skipped, packs) = load_mods(root.path(), &[], true);
        assert!(mods.is_empty());
        assert_eq!(disabled, ["off"]);
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(packs.is_empty(), "{packs:?}");
    }
}
