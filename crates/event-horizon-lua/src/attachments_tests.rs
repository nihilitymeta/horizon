//! Attachments a Lua mod keeps on entities (issue 74, C1): get, set and remove, the name and value
//! limits, the key and total caps, the turn rules, the save round trip and the index's agreement
//! with the stored values. The world double resolves one entity, id 1 of epoch 1, and every other
//! handle is gone. Each test loads its own mod, whose handler logs its results as `R <label> ...`.

use std::collections::HashMap;
use std::path::Path;

use mlua::Lua;
use mod_api::prelude::{EntitiesHost, ModStateHost, ModStateServerHost};
use mod_api::{
    AttachmentAccess, AttachmentError, AttachmentHost, AttachmentStore, Attachments, ClientHost,
    Compound, EntityRef, EntityResidency, Holder, Host, ServerHost, Tag, WorldEpoch,
};

use crate::attachments::{MAX_HOLDER_BYTES, MAX_KEYS, MAX_NAME_BYTES, MAX_VALUE_BYTES};
use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::new_event;
use crate::handles::Token;
use crate::host::LuaHost;
use crate::host::LuaMod;
use crate::host::tests::{captured_with_prefix, install_capture};
use crate::hostcall::TurnHost;

const HOOK: &str = "block_receive_game";

/// The holders of the test world: one attachment map per holder the store keeps.
#[derive(Default)]
struct World {
    maps: HashMap<Holder, Attachments>,
}

impl World {
    /// A world with the resident entity's map.
    fn new() -> World {
        let mut maps = HashMap::new();
        maps.insert(Holder::Entity(1), Attachments::default());
        World { maps }
    }
}

/// The entity the world holds: id 1 of epoch 1.
fn resident() -> EntityRef {
    EntityRef::new(WorldEpoch::new(1), 1)
}

impl Host for World {}
mod_api::impl_host_areas!(World, except(mod_state, entities));

impl EntitiesHost for World {
    fn entity_residency(&self, entity: EntityRef) -> EntityResidency {
        if entity == resident() {
            EntityResidency::InPlace
        } else {
            EntityResidency::Gone
        }
    }
}

impl ModStateHost for World {
    fn attachment_query(&self, _: &str, _: &str) -> mod_api::AttachmentBatch {
        horizon_mod_runtime::attachments::AttachmentBatch::empty().into()
    }
    fn attachments(&mut self) -> &mut dyn AttachmentStore {
        self
    }
}

impl AttachmentHost for World {
    fn attachments(
        &mut self,
        holder: Holder,
        _: AttachmentAccess,
    ) -> Result<&mut Attachments, AttachmentError> {
        self.maps
            .get_mut(&holder)
            .ok_or(AttachmentError::MissingHolder)
    }
}

impl ServerHost for World {}
mod_api::impl_server_host_areas!(World, except(mod_state));
impl ModStateServerHost for World {}

impl ClientHost for World {}
mod_api::impl_client_host_areas!(World);

/// The world as a save writes it and reads it back: every map through `to_nbt` and `from_nbt`.
fn reload(world: &World) -> World {
    let maps = world
        .maps
        .iter()
        .map(|(holder, map)| (*holder, Attachments::from_nbt(map.to_nbt())))
        .collect();
    World { maps }
}

/// Loads a server-and-client mod `id` whose `block_receive_game` handler runs `body` on `event`.
fn load(root: &Path, id: &str, body: &str) -> LuaHost {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let source = format!("horizon.on(\"{HOOK}\", function(event)\n{body}\nend)\n");
    std::fs::write(dir.join(ENTRY), source).unwrap();
    let module = LuaMod::load(&Discovered {
        dir,
        id: id.to_owned(),
        version: "1".to_owned(),
        side: Side::Both,
    })
    .unwrap();
    LuaHost::new(vec![module])
}

/// The event for one fire: the entity handle the mod reads as `event.entity`.
fn event_for(
    entity: EntityRef,
) -> impl Fn(&Lua, &TurnHost<'_>) -> mlua::Result<crate::dispatch::Event> {
    move |lua: &Lua, _: &TurnHost<'_>| {
        let data = lua.create_table()?;
        data.raw_set("entity", lua.create_userdata(Token(entity))?)?;
        new_event(lua, data, &["cancelled", "result"])
    }
}

/// Runs `body` once on `turn`, with `entity` as the event's entity, and returns the mod's `R` lines
/// without the id and the `R ` marker.
fn run_on(turn: TurnHost<'_>, id: &str, body: &str, entity: EntityRef) -> Vec<String> {
    // The capture must be in place before the mod logs, or its first lines are lost.
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let host = load(root.path(), id, body);
    host.dispatch_turns(HOOK, turn, &|| false, &event_for(entity), &mut |_, _, _| {
        true
    })
    .unwrap();
    let prefix = format!("{id}: R ");
    captured_with_prefix(&prefix)
        .iter()
        .map(|line| line[prefix.len()..].to_owned())
        .collect()
}

/// Runs `body` on the resident entity of `world`.
fn run(world: &mut World, id: &str, body: &str) -> Vec<String> {
    run_on(TurnHost::Server(world), id, body, resident())
}

/// The value a set of a nested table stores, as the save reads it: `{ a = { b = 7 }, names = {"x","y"} }`.
fn nested_value() -> Compound {
    let mut inner = Compound::new();
    inner.insert("b".into(), Tag::Int(7));
    let mut value = Compound::new();
    value.insert("a".into(), Tag::Compound(inner));
    value.insert(
        "names".into(),
        Tag::List(vec![Tag::String("x".into()), Tag::String("y".into())]),
    );
    value
}

/// The lines a refused call logs: `label ok err`, with `err` the Lua error text.
fn refused(lines: &[String], label: &str) -> String {
    let prefix = format!("{label} ");
    lines
        .iter()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no line for {label}: {lines:?}"))
        .clone()
}

/// The Lua helper every body starts with: `try(label, f)` logs whether `f` raised, and why.
const TRY: &str = "local function try(label, f)\n\
    local ok, err = pcall(f)\n\
    horizon.log(\"R \" .. label .. \" \" .. tostring(ok) .. \" \" .. tostring(err))\n\
end\n";

/// A table whose NBT is exactly `size` bytes of entries: two strings of 30000 bytes, and a third of
/// `5517 + extra` bytes, which is 65536 at `extra` 0 (1 + 2 * 30006 + 5523 for the three entries).
const BIG: &str = "local function big(extra)\n\
    return { a = string.rep(\"x\", 30000), b = string.rep(\"x\", 30000), c = string.rep(\"x\", 5517 + extra) }\n\
end\n";

#[test]
fn a_nested_value_reads_back_and_a_missing_name_is_nil() {
    let mut world = World::new();
    let lines = run(
        &mut world,
        "rt1",
        "local v = { a = { b = 7 }, names = { \"x\", \"y\" } }\n\
         horizon.log(\"R set \" .. tostring(horizon.attachment_set(event.entity, \"rt1:charge\", v)))\n\
         local got = horizon.attachment_get(event.entity, \"rt1:charge\")\n\
         horizon.log(\"R get \" .. got.a.b .. \" \" .. got.names[2])\n\
         horizon.log(\"R missing \" .. tostring(horizon.attachment_get(event.entity, \"rt1:none\")))",
    );
    assert_eq!(lines, ["set true", "get 7 y", "missing nil"]);
    let saved = AttachmentStore::saved(&mut reload(&world), Holder::Entity(1), "rt1", "charge");
    assert_eq!(saved.unwrap(), Some(nested_value()));
}

#[test]
fn remove_says_true_once_then_false_and_the_save_drops_it() {
    let mut world = World::new();
    let lines = run(
        &mut world,
        "rm1",
        "horizon.attachment_set(event.entity, \"rm1:k\", { v = 1 })\n\
         horizon.log(\"R first \" .. tostring(horizon.attachment_remove(event.entity, \"rm1:k\")))\n\
         horizon.log(\"R again \" .. tostring(horizon.attachment_remove(event.entity, \"rm1:k\")))\n\
         horizon.log(\"R gone \" .. tostring(horizon.attachment_get(event.entity, \"rm1:k\")))",
    );
    assert_eq!(lines, ["first true", "again false", "gone nil"]);
    let mut saved = reload(&world);
    assert_eq!(
        AttachmentStore::saved(&mut saved, Holder::Entity(1), "rm1", "k").unwrap(),
        None
    );
    // The index drops with the last key, so the namespace holds nothing for the holder.
    assert_eq!(
        AttachmentStore::saved(&mut saved, Holder::Entity(1), "rm1", "#index").unwrap(),
        None
    );
}

#[test]
fn names_must_be_the_mods_own_and_in_the_charset() {
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         local long = \"nm1:\" .. string.rep(\"a\", 252)\n\
         try(\"ok256\", function() horizon.attachment_set(event.entity, long, {{ v = 1 }}) end)\n\
         try(\"long257\", function() horizon.attachment_set(event.entity, long .. \"a\", {{ v = 1 }}) end)\n\
         try(\"other\", function() horizon.attachment_set(event.entity, \"other:x\", {{ v = 1 }}) end)\n\
         try(\"vanilla\", function() horizon.attachment_set(event.entity, \"minecraft:x\", {{ v = 1 }}) end)\n\
         try(\"read_foreign\", function() horizon.attachment_get(event.entity, \"other:x\") end)\n\
         try(\"charset\", function() horizon.attachment_set(event.entity, \"nm1:Bad\", {{ v = 1 }}) end)\n\
         try(\"bare\", function() horizon.attachment_set(event.entity, \"nm1\", {{ v = 1 }}) end)"
    );
    let lines = run(&mut world, "nm1", &body);
    assert_eq!(refused(&lines, "ok256"), "ok256 true nil");
    assert!(
        refused(&lines, "long257").contains(&format!(
            "the name is {} bytes, more than {MAX_NAME_BYTES}",
            MAX_NAME_BYTES + 1
        )),
        "{lines:?}"
    );
    for label in ["other", "vanilla", "read_foreign"] {
        assert!(
            refused(&lines, label).contains("is outside this mod's namespace 'nm1'"),
            "{lines:?}"
        );
    }
    assert!(
        refused(&lines, "charset").contains("invalid path"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "bare").contains("needs a namespaced name"),
        "{lines:?}"
    );
    // The foreign writes changed nothing: only the 256-byte name is held.
    let mut saved = reload(&world);
    assert!(
        AttachmentStore::saved(&mut saved, Holder::Entity(1), "other", "x")
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_value_over_64_kib_or_not_a_table_is_refused_and_the_old_value_stays() {
    let mut world = World::new();
    let body = format!(
        "{TRY}{BIG}\
         try(\"fits\", function() horizon.attachment_set(event.entity, \"cap:big\", big(0)) end)\n\
         try(\"over\", function() horizon.attachment_set(event.entity, \"cap:big\", big(1)) end)\n\
         horizon.log(\"R kept \" .. #horizon.attachment_get(event.entity, \"cap:big\").c)\n\
         try(\"number\", function() horizon.attachment_set(event.entity, \"cap:n\", 5) end)\n\
         try(\"string\", function() horizon.attachment_set(event.entity, \"cap:n\", \"text\") end)\n\
         try(\"deep\", function() horizon.attachment_set(event.entity, \"cap:s\", {{ s = string.rep(\"x\", 32768) }}) end)"
    );
    let lines = run(&mut world, "cap", &body);
    assert_eq!(refused(&lines, "fits"), "fits true nil");
    assert!(
        refused(&lines, "over").contains(&format!(
            "the value is {} bytes, more than {MAX_VALUE_BYTES}",
            MAX_VALUE_BYTES + 1
        )),
        "{lines:?}"
    );
    assert!(lines.contains(&"kept 5517".to_owned()), "{lines:?}");
    assert!(
        refused(&lines, "number").contains("the value must be a table"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "string").contains("the value must be a table"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "deep").contains("the value is refused"),
        "{lines:?}"
    );
}

#[test]
fn a_holder_keeps_at_most_16_names_and_refuses_the_17th() {
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         for i = 1, 16 do horizon.attachment_set(event.entity, \"keys:k\" .. i, {{ v = i }}) end\n\
         try(\"k17\", function() horizon.attachment_set(event.entity, \"keys:k17\", {{ v = 17 }}) end)\n\
         try(\"overwrite\", function() horizon.attachment_set(event.entity, \"keys:k1\", {{ v = 99 }}) end)\n\
         horizon.log(\"R count \" .. horizon.attachment_get(event.entity, \"keys:k1\").v)"
    );
    let lines = run(&mut world, "keys", &body);
    assert!(
        refused(&lines, "k17").contains(&format!("at most {MAX_KEYS} attachments")),
        "{lines:?}"
    );
    assert_eq!(refused(&lines, "overwrite"), "overwrite true nil");
    assert!(lines.contains(&"count 99".to_owned()), "{lines:?}");
}

#[test]
fn a_holder_keeps_at_most_256_kib_of_values_per_mod() {
    let mut world = World::new();
    let body = format!(
        "{TRY}{BIG}\
         for i = 1, 4 do horizon.attachment_set(event.entity, \"tot:b\" .. i, big(0)) end\n\
         try(\"fifth\", function() horizon.attachment_set(event.entity, \"tot:b5\", {{ v = 1 }}) end)"
    );
    let lines = run(&mut world, "tot", &body);
    // Four values of 65536 bytes fill the cap; the fifth, 9 bytes, would make 262153.
    assert_eq!(MAX_HOLDER_BYTES, 4 * MAX_VALUE_BYTES);
    assert!(
        refused(&lines, "fifth")
            .contains("the holder's values would be 262153 bytes, more than 262144"),
        "{lines:?}"
    );
}

#[test]
fn only_a_server_turn_reaches_the_store_and_a_stale_holder_is_refused() {
    let body = "horizon.log(\"R set \" .. tostring(horizon.attachment_set(event.entity, \"tu1:k\", { v = 1 })))";
    let mut world = World::new();
    let client = run_on(TurnHost::Client(&mut world), "tu1", body, resident());
    assert!(
        client.iter().all(|line| !line.starts_with("set")),
        "a client turn must not run the set: {client:?}"
    );
    let world = World::new();
    let read = run_on(
        TurnHost::Read(&world),
        "tu2",
        &body.replace("tu1", "tu2"),
        resident(),
    );
    assert!(read.is_empty(), "{read:?}");
    let mut world = World::new();
    let none = run_on(
        TurnHost::None,
        "tu3",
        &body.replace("tu1", "tu3"),
        resident(),
    );
    assert!(none.is_empty(), "{none:?}");
    // A handle of another world epoch is gone, and its write changes nothing.
    let stale = EntityRef::new(WorldEpoch::new(2), 1);
    let stale_lines = run_on(
        TurnHost::Server(&mut world),
        "tu4",
        &format!(
            "{TRY}try(\"stale\", function() horizon.attachment_set(event.entity, \"tu4:k\", {{ v = 1 }}) end)"
        ),
        stale,
    );
    assert!(
        refused(&stale_lines, "stale").contains("the holder is gone, or its handle is stale"),
        "{stale_lines:?}"
    );
    assert!(world.maps[&Holder::Entity(1)].to_nbt().is_empty());
}

#[test]
fn values_and_the_key_count_survive_a_save_and_a_reload() {
    let mut world = World::new();
    run(
        &mut world,
        "rl",
        "horizon.attachment_set(event.entity, \"rl:keep\", { v = 5 })",
    );
    let mut reloaded = reload(&world);
    let mut body = String::from(TRY);
    body.push_str(
        "horizon.log(\"R kept \" .. horizon.attachment_get(event.entity, \"rl:keep\").v)\n",
    );
    body.push_str(
        "for i = 1, 15 do horizon.attachment_set(event.entity, \"rl:n\" .. i, { v = i }) end\n",
    );
    body.push_str("try(\"n16\", function() horizon.attachment_set(event.entity, \"rl:n16\", { v = 16 }) end)\n");
    let lines = run(&mut reloaded, "rl", &body);
    assert!(lines.contains(&"kept 5".to_owned()), "{lines:?}");
    // The reloaded index already counts the saved key: 1 + 15 is the limit, the 17th refused.
    assert!(
        refused(&lines, "n16").contains(&format!("at most {MAX_KEYS} attachments")),
        "{lines:?}"
    );
}
