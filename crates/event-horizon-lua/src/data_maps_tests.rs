//! Data maps a Lua mod declares, and `horizon.data_map` reads (issue 74): each limit refuses its own
//! declaration with a warning and the mod still loads; a good declaration is read back by key; a
//! read outside a turn raises. The double backend answers what the test hand-writes.

use std::path::Path;

use mod_api::__private::game::BackendRefOf;
use mod_api::prelude::DataMapsBackend;
use mod_api::{Backend, BackendRef, DataMapError, Host, Tag};

use crate::data_maps::{
    MAX_DATA_MAP_TYPES, MAX_DATA_PACK_FILES, MAX_JSON_BYTES, MAX_JSON_DEPTH, MAX_JSON_ENTRIES,
    check_json,
};
use crate::declare::environment;
use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::new_event;
use crate::host::LuaHost;
use crate::host::LuaMod;
use crate::host::tests::{captured_with_prefix, install_capture};
use crate::hostcall::TurnHost;

/// A backend whose only data map is an `…:emc` map for items: an apple is worth 5, nothing else has
/// a value. Every other read answers no value.
struct Maps;

impl DataMapsBackend for Maps {
    fn data_map_value(
        &self,
        registry: &str,
        entry: &str,
        map: &str,
    ) -> Result<Option<Tag>, DataMapError> {
        Ok(match (registry, entry, map) {
            ("minecraft:item", "minecraft:apple", map) if map.ends_with(":emc") => {
                Some(Tag::Int(5))
            }
            _ => None,
        })
    }

    fn data_map_entries(
        &self,
        _registry: &str,
        _map: &str,
    ) -> Result<Vec<(String, Tag)>, DataMapError> {
        Ok(Vec::new())
    }
}

impl Host for Maps {
    fn backend(&self) -> Option<BackendRef<'_>> {
        Some(BackendRefOf::of(self))
    }
}
impl Backend for Maps {}
mod_api::impl_host_areas!(Maps);
mod_api::impl_backend_areas!(Maps, except(data_maps));

/// Loads one server mod `id` whose main.luau is `source`, from its own folder under `root`.
fn load(root: &Path, id: &str, source: &str) -> LuaMod {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ENTRY), source).unwrap();
    LuaMod::load(&Discovered {
        dir,
        id: id.to_owned(),
        version: "1".to_owned(),
        side: Side::Server,
    })
    .unwrap()
}

/// The types and files the host answers its data map hooks with, for one mod.
fn answered(host: &LuaHost) -> (Vec<mod_api::DataMapType>, Vec<(String, Vec<u8>)>) {
    let mut types = Vec::new();
    let mut files = Vec::new();
    host.declared_data_map_types(&mut types);
    host.declared_data_pack_files(&mut files);
    (types, files)
}

/// Runs the mod's `data_maps_updated` handlers with the double backend as the turn's host.
fn fire_read(host: &LuaHost) {
    host.dispatch_turns(
        "data_maps_updated",
        TurnHost::Read(&Maps),
        &|| false,
        &|lua, _| new_event(lua, lua.create_table()?, &[]),
        &mut |_, _, _| true,
    )
    .unwrap();
}

/// The mod's warnings that start with `prefix`.
fn warnings(prefix: &str) -> Vec<String> {
    captured_with_prefix(prefix)
}

#[test]
fn a_declared_type_and_file_are_read_back_by_key() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let m = load(
        root.path(),
        "dm_read",
        "horizon.data_map_type{ id = \"dm_read:emc\", registry = \"minecraft:item\" }\n\
         horizon.data_pack_file{ key = \"data_maps/dm_read:item/emc\", json = '{\"values\": {\"minecraft:apple\": 5}}' }\n\
         horizon.on(\"data_maps_updated\", function()\n\
           horizon.log(tostring(horizon.data_map(\"minecraft:item\", \"minecraft:apple\", \"dm_read:emc\")))\n\
           horizon.log(tostring(horizon.data_map(\"minecraft:item\", \"minecraft:stone\", \"dm_read:emc\")))\n\
         end)",
    );
    let host = LuaHost::new(vec![m]);
    let (types, files) = answered(&host);
    assert_eq!(
        types,
        vec![mod_api::DataMapType {
            id: "dm_read:emc".to_owned(),
            registry: "minecraft:item".to_owned(),
        }]
    );
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, "data_maps/dm_read:item/emc");
    assert_eq!(
        files[0].1,
        br#"{"values": {"minecraft:apple": 5}}"#.to_vec()
    );
    fire_read(&host);
    // The double answers an apple's value (5) and no value for stone (nil).
    assert_eq!(host.mods()[0].log_lines(), ["5", "nil"]);
}

#[test]
fn a_read_outside_a_turn_raises() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let m = load(
        root.path(),
        "dm_outside",
        "horizon.on(\"data_maps_updated\", function() keep = horizon.data_map end)",
    );
    let host = LuaHost::new(vec![m]);
    fire_read(&host);
    let env = environment(host.mods()[0].lua()).unwrap();
    let kept: mlua::Function = env.get("keep").unwrap();
    let err = kept
        .call::<mlua::Value>(("minecraft:item", "minecraft:apple", "dm_outside:emc"))
        .unwrap_err();
    assert!(
        err.to_string().contains("no host call outside a hook"),
        "got {err}"
    );
}

#[test]
fn a_read_while_the_mod_loads_raises() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("dm_loading");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(ENTRY),
        "horizon.data_map(\"minecraft:item\", \"minecraft:apple\", \"dm_loading:emc\")",
    )
    .unwrap();
    let err = LuaMod::load(&Discovered {
        dir,
        id: "dm_loading".to_owned(),
        version: "1".to_owned(),
        side: Side::Server,
    })
    .err()
    .expect("a read while the mod loads must refuse the mod");
    assert!(
        err.contains("not available while the mod loads"),
        "got {err}"
    );
}

#[test]
fn an_oversize_file_is_refused_and_the_mod_still_loads() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    // One byte over the cap, then exactly the cap; the type declared after both still stands.
    let source = format!(
        "horizon.data_pack_file{{ key = \"data_maps/dm_big:item/big\", json = \"[\" .. string.rep(\" \", {}) .. \"]\" }}\n\
         horizon.data_pack_file{{ key = \"data_maps/dm_big:item/fits\", json = \"[\" .. string.rep(\" \", {}) .. \"]\" }}\n\
         horizon.data_map_type{{ id = \"dm_big:emc\", registry = \"minecraft:item\" }}",
        MAX_JSON_BYTES - 1,
        MAX_JSON_BYTES - 2,
    );
    let m = load(root.path(), "dm_big", &source);
    let host = LuaHost::new(vec![m]);
    let (types, files) = answered(&host);
    assert_eq!(types.len(), 1, "the type declared after the file stands");
    let keys: Vec<&str> = files.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys,
        ["data_maps/dm_big:item/fits"],
        "the cap itself is allowed"
    );
    let lines = warnings("dm_big: data pack file 'data_maps/dm_big:item/big' refused");
    assert_eq!(lines.len(), 1, "got {lines:?}");
    assert!(
        lines[0].ends_with(&format!(
            "its JSON is {} bytes, more than {MAX_JSON_BYTES}",
            MAX_JSON_BYTES + 1
        )),
        "got {}",
        lines[0]
    );
}

#[test]
fn a_mod_may_ship_64_files_and_not_a_65th() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "for i = 1, {} do horizon.data_pack_file{{ key = \"data_maps/dm_many:item/f\" .. i, json = \"{{}}\" }} end",
        MAX_DATA_PACK_FILES + 1
    );
    let host = LuaHost::new(vec![load(root.path(), "dm_many", &source)]);
    let (_, files) = answered(&host);
    assert_eq!(files.len(), MAX_DATA_PACK_FILES);
    assert_eq!(
        warnings("dm_many: data pack file 'data_maps/dm_many:item/f65' refused"),
        [
            "dm_many: data pack file 'data_maps/dm_many:item/f65' refused: a mod may ship at most 64 data pack files"
        ]
    );
}

#[test]
fn json_nested_33_deep_is_refused_and_32_is_not() {
    install_capture();
    let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    // The check reads the bytes: a 33-deep document never reaches the parser, so no recursion.
    let err = check_json(&deep(MAX_JSON_DEPTH + 1)).unwrap_err();
    assert_eq!(err, "its JSON nests deeper than 32 levels");
    assert!(check_json(&deep(MAX_JSON_DEPTH)).is_ok());
    // A bracket inside a string is not nesting.
    assert!(check_json(&format!("[\"{}\"]", "[".repeat(40))).is_ok());
    let root = tempfile::tempdir().unwrap();
    let source = "horizon.data_pack_file{ key = \"data_maps/dm_deep:item/deep\", json = string.rep(\"[\", 33) .. string.rep(\"]\", 33) }";
    let host = LuaHost::new(vec![load(root.path(), "dm_deep", source)]);
    assert!(answered(&host).1.is_empty());
    assert_eq!(
        warnings("dm_deep: data pack file 'data_maps/dm_deep:item/deep' refused").len(),
        1
    );
}

#[test]
fn entries_past_4096_are_refused_and_4096_are_not() {
    let entries = |n: usize| format!("[{}0]", "0,".repeat(n - 1));
    let err = check_json(&entries(MAX_JSON_ENTRIES + 1)).unwrap_err();
    assert_eq!(err, "its JSON has 4097 entries, more than 4096");
    assert!(check_json(&entries(MAX_JSON_ENTRIES)).is_ok());
}

#[test]
fn a_bad_registry_id_refuses_its_type_with_a_warning() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load(
        root.path(),
        "dm_reg",
        "horizon.data_map_type{ id = \"dm_reg:emc\", registry = \"Minecraft:Item\" }\n\
         horizon.data_map_type{ id = \"dm_reg:ok\", registry = \"minecraft:item\" }",
    )]);
    let (types, _) = answered(&host);
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].id, "dm_reg:ok");
    let lines = warnings("dm_reg: data map type 'dm_reg:emc' refused");
    assert_eq!(lines.len(), 1, "got {lines:?}");
    assert!(lines[0].contains("has an invalid path"), "got {}", lines[0]);
}

#[test]
fn a_type_outside_the_mods_namespace_is_refused() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load(
        root.path(),
        "dm_ns",
        "horizon.data_map_type{ id = \"other:emc\", registry = \"minecraft:item\" }\n\
         horizon.data_map_type{ id = \"minecraft:emc\", registry = \"minecraft:item\" }",
    )]);
    assert!(answered(&host).0.is_empty());
    let lines = warnings("dm_ns: data map type 'other:emc' refused");
    assert!(
        lines[0].ends_with("is outside this mod's namespace 'dm_ns'"),
        "got {lines:?}"
    );
    assert_eq!(
        warnings("dm_ns: data map type 'minecraft:emc' refused").len(),
        1
    );
}

#[test]
fn a_file_under_another_mods_data_is_refused() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load(
        root.path(),
        "dm_other",
        "horizon.data_pack_file{ key = \"data_maps/other:item/emc\", json = \"{}\" }\n\
         horizon.data_pack_file{ key = \"recipe/dm_other:thing\", json = \"{}\" }\n\
         horizon.data_pack_file{ key = \"data_maps/dm_other:item/../emc\", json = \"{}\" }",
    )]);
    assert!(answered(&host).1.is_empty());
    assert_eq!(
        warnings("dm_other: data pack file 'data_maps/other:item/emc' refused")[0],
        "dm_other: data pack file 'data_maps/other:item/emc' refused: is outside this mod's namespace 'dm_other'"
    );
    assert_eq!(
        warnings("dm_other: data pack file 'recipe/dm_other:thing' refused").len(),
        1
    );
    assert_eq!(
        warnings("dm_other: data pack file 'data_maps/dm_other:item/../emc' refused").len(),
        1
    );
}

#[test]
fn a_duplicate_type_id_and_too_many_types_are_refused() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "horizon.data_map_type{{ id = \"dm_dup:emc\", registry = \"minecraft:item\" }}\n\
         horizon.data_map_type{{ id = \"dm_dup:emc\", registry = \"minecraft:block\" }}\n\
         for i = 1, {MAX_DATA_MAP_TYPES} do horizon.data_map_type{{ id = \"dm_dup:t\" .. i, registry = \"minecraft:item\" }} end"
    );
    let host = LuaHost::new(vec![load(root.path(), "dm_dup", &source)]);
    let (types, _) = answered(&host);
    assert_eq!(types.len(), MAX_DATA_MAP_TYPES);
    assert_eq!(types[0].id, "dm_dup:emc");
    assert_eq!(
        warnings("dm_dup: data map type 'dm_dup:emc' refused")[0],
        "dm_dup: data map type 'dm_dup:emc' refused: is declared twice"
    );
    assert_eq!(
        warnings("dm_dup: data map type 'dm_dup:t16' refused")[0],
        "dm_dup: data map type 'dm_dup:t16' refused: a mod may declare at most 16 data map types"
    );
}

#[test]
fn a_mod_with_a_refused_declaration_keeps_its_handlers() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load(
        root.path(),
        "dm_keep",
        "horizon.data_map_type{ id = \"dm_keep:emc\", registry = \"no good:item\" }\n\
         horizon.on(\"data_maps_updated\", function() horizon.log(\"still here\") end)",
    )]);
    fire_read(&host);
    assert_eq!(host.mods()[0].log_lines(), ["still here"]);
}
