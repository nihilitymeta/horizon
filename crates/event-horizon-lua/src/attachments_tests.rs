//! Attachments a Lua mod keeps on entities and block entities (issue 74, C1 and A1): get, set and remove, the name and value
//! limits, the key and total caps, the turn rules, the save round trip and the index's agreement
//! with the stored values. The world double resolves one entity, id 1 of epoch 1, and two block
//! entities of the overworld ([`BLOCK`] and [`OTHER_BLOCK`]); every other handle is gone. Each test loads its own mod, whose handler logs its results as `R <label> ...`.

use std::collections::HashMap;
use std::path::Path;

use horizon_mod_runtime::attachments::AttachmentSync;
use mlua::Lua;
use mod_api::prelude::{
    BlockEntitiesHost, ConfigHost, EntitiesHost, ModStateHost, ModStateServerHost,
};
use mod_api::{
    AttachmentAccess, AttachmentError, AttachmentHost, AttachmentStore, Attachments,
    BlockEntityRef, ClientHost, Compound, DimensionKey, EntityRef, EntityResidency, Holder, Host,
    LevelRef, ServerHost, Tag, WorldEpoch,
};

use crate::attachments::{
    MAX_BLOCK_ENTITY_NEW_KEYS, MAX_COPY_KEYS, MAX_FLAGGED_VALUE_BYTES, MAX_HOLDER_BYTES, MAX_KEYS,
    MAX_NAME_BYTES, MAX_SYNC_KEYS, MAX_SYNC_WRITES, MAX_VALUE_BYTES,
};
use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::new_event;
use crate::handles::Token;
use crate::host::LuaHost;
use crate::host::LuaMod;
use crate::host::tests::{captured_with_prefix, install_capture};
use crate::hostcall::TurnHost;

const HOOK: &str = "block_receive_game";

/// The holders of the test world: one attachment map per holder the store keeps, and the block
/// entities the world holds now, each with its position: a block entity handle is current only
/// while it is in this list.
#[derive(Default)]
struct World {
    maps: HashMap<Holder, Attachments>,
    blocks: Vec<(BlockEntityRef, [i32; 3])>,
}

/// The position of the test world's first block entity.
const BLOCK: [i32; 3] = [1, 2, 3];
/// The position of its second block entity.
const OTHER_BLOCK: [i32; 3] = [4, 5, 6];

/// The overworld of the test world: where its block entities are.
fn block_level() -> LevelRef {
    LevelRef::new(WorldEpoch::new(1), DimensionKey::OVERWORLD)
}

/// A block entity handle of the test world at `pos`, with lifecycle `generation`.
fn block_handle(pos: [i32; 3], generation: u64) -> BlockEntityRef {
    mod_api::__private::game::block_entity_ref(block_level(), pos, generation)
}

impl World {
    /// A world with the resident entity's map, and the two block entities, each with its map.
    fn new() -> World {
        let mut maps = HashMap::new();
        maps.insert(Holder::Entity(1), Attachments::default());
        let mut blocks = Vec::new();
        for pos in [BLOCK, OTHER_BLOCK] {
            maps.insert(Holder::BlockEntity(pos), Attachments::default());
            blocks.push((block_handle(pos, 0), pos));
        }
        World { maps, blocks }
    }
}

/// The entity the world holds: id 1 of epoch 1.
fn resident() -> EntityRef {
    EntityRef::new(WorldEpoch::new(1), 1)
}

impl Host for World {}
mod_api::impl_host_areas!(World, except(mod_state, entities, block_entities, config));

impl ConfigHost for World {
    fn dimension(&self) -> &'static str {
        "minecraft:overworld"
    }
}

impl EntitiesHost for World {
    fn entity_residency(&self, entity: EntityRef) -> EntityResidency {
        if entity == resident() {
            EntityResidency::InPlace
        } else {
            EntityResidency::Gone
        }
    }
    fn level(&self, dimension: &str) -> Option<LevelRef> {
        (dimension == "minecraft:overworld").then(block_level)
    }
}

impl BlockEntitiesHost for World {
    fn block_entity(&self, level: LevelRef, pos: [i32; 3]) -> Option<BlockEntityRef> {
        self.block_entity_identity(level, pos)
    }
    fn block_entity_identity(&self, level: LevelRef, pos: [i32; 3]) -> Option<BlockEntityRef> {
        if level != block_level() {
            return None;
        }
        self.blocks
            .iter()
            .find(|(_, at)| *at == pos)
            .map(|(block, _)| *block)
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
    World {
        maps,
        blocks: world.blocks.clone(),
    }
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
    crate::host::tests::lift_handler_budget();
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

// ---- C2: sync, copy_on_death, the client read (issue 74, C2) ----

/// The uuid of the test player, who is the resident entity's player.
const PLAYER: u128 = 7;

/// A world with the resident entity and its player (uuid [`PLAYER`]), who has the same entity handle.
fn player_world() -> World {
    let mut world = World::new();
    world
        .maps
        .insert(Holder::Player(PLAYER), Attachments::default());
    world
}

/// The player handle of the test world.
fn player() -> mod_api::PlayerRef {
    mod_api::__private::game::player_ref(resident(), PLAYER)
}

/// Runs `body` with `player` as the event's entity, as a server turn of `world`.
fn run_player(world: &mut World, id: &str, body: &str) -> Vec<String> {
    crate::host::tests::lift_handler_budget();
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let host = load(root.path(), id, body);
    let player = player();
    let make = move |lua: &Lua, _: &TurnHost<'_>| {
        let data = lua.create_table()?;
        data.raw_set("entity", lua.create_userdata(Token(player))?)?;
        new_event(lua, data, &["cancelled", "result"])
    };
    host.dispatch_turns(
        HOOK,
        TurnHost::Server(world),
        &|| false,
        &make,
        &mut |_, _, _| true,
    )
    .unwrap();
    let prefix = format!("{id}: R ");
    captured_with_prefix(&prefix)
        .iter()
        .map(|line| line[prefix.len()..].to_owned())
        .collect()
}

/// The envelope the store saves for `key` on the resident entity's map of `world`.
fn envelope(world: &World, holder: Holder, namespace: &str, key: &str) -> Compound {
    world.maps[&holder]
        .entry(namespace, key)
        .unwrap()
        .unwrap()
        .saved()
        .unwrap()
}

/// The synced changes the entity's map has queued since the last drain.
fn drain(world: &mut World, holder: Holder) -> Vec<AttachmentSync> {
    world.maps.get_mut(&holder).unwrap().take_sync_changes()
}

#[test]
fn a_synced_set_is_saved_as_its_value_and_queues_the_send() {
    let mut world = World::new();
    let lines = run(
        &mut world,
        "sy1",
        "horizon.log(\"R set \" .. tostring(horizon.attachment_set(event.entity, \"sy1:charge\", { a = { b = 7 }, names = { \"x\", \"y\" } }, { sync = true })))",
    );
    assert_eq!(lines, ["set true"]);
    let saved = envelope(&world, Holder::Entity(1), "sy1", "charge");
    assert_eq!(
        saved.get(&"value".into()),
        Some(&Tag::Compound(nested_value()))
    );
    assert_eq!(saved.get(&"sync".into()), Some(&Tag::Byte(1)));
    assert!(!saved.contains_key(&"copy_on_death".into()));
    let queued = drain(&mut world, Holder::Entity(1));
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].value, Some(nested_value()));
    assert_eq!(queued[0].name, "charge".into());
}

#[test]
fn the_index_keeps_its_keys_flags_and_is_never_synced() {
    let mut world = World::new();
    run(
        &mut world,
        "ix",
        "horizon.attachment_set(event.entity, \"ix:s\", { v = 1 }, { sync = true })\n\
         horizon.attachment_set(event.entity, \"ix:p\", { v = 2 })",
    );
    let index = envelope(&world, Holder::Entity(1), "ix", "#index");
    // The index's own envelope carries no policy: it is never synced or copied.
    assert!(!index.contains_key(&"sync".into()));
    assert!(!index.contains_key(&"copy_on_death".into()));
    // Its value says which keys are synced: the synced key has the flag, the plain key its size only.
    let Some(Tag::Compound(keys)) = index.get(&"value".into()) else {
        panic!("{index:?}")
    };
    let Some(Tag::Compound(s)) = keys.get(&"s".into()) else {
        panic!("{keys:?}")
    };
    assert_eq!(s.get(&"sync".into()), Some(&Tag::Byte(1)));
    assert_eq!(keys.get(&"p".into()), Some(&Tag::Int(9)));
    // The index is never queued for the clients.
    let queued = drain(&mut world, Holder::Entity(1));
    assert!(
        queued.iter().all(|change| change.name != "#index".into()),
        "{queued:?}"
    );
}

#[test]
fn a_synced_value_of_4_kib_is_accepted_and_one_byte_more_is_refused() {
    // { c = "x" * N } is 7 + N bytes: N = 4089 is exactly 4096.
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         try(\"fits\", function() horizon.attachment_set(event.entity, \"sz:c\", {{ c = string.rep(\"x\", 4089) }}, {{ sync = true }}) end)\n\
         try(\"over\", function() horizon.attachment_set(event.entity, \"sz:c\", {{ c = string.rep(\"x\", 4090) }}, {{ sync = true }}) end)\n\
         try(\"plain\", function() horizon.attachment_set(event.entity, \"sz:p\", {{ c = string.rep(\"x\", 4090) }}) end)"
    );
    let lines = run(&mut world, "sz", &body);
    assert_eq!(refused(&lines, "fits"), "fits true nil");
    assert!(
        refused(&lines, "over").contains(&format!(
            "the synced value is 4097 bytes, more than {MAX_FLAGGED_VALUE_BYTES}"
        )),
        "{lines:?}"
    );
    // The same size without sync is an ordinary value, well under the 64 KiB cap.
    assert_eq!(refused(&lines, "plain"), "plain true nil");
    assert!(
        world.maps[&Holder::Entity(1)]
            .entry("sz", "c")
            .unwrap()
            .unwrap()
            .saved()
            .is_some()
    );
}

#[test]
fn the_fifth_synced_key_is_refused_and_resyncing_a_synced_key_is_not_a_new_one() {
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         for i = 1, 4 do horizon.attachment_set(event.entity, \"sk:k\" .. i, {{ v = i }}, {{ sync = true }}) end\n\
         try(\"k5\", function() horizon.attachment_set(event.entity, \"sk:k5\", {{ v = 5 }}, {{ sync = true }}) end)\n\
         try(\"again\", function() horizon.attachment_set(event.entity, \"sk:k2\", {{ v = 22 }}, {{ sync = true }}) end)\n\
         try(\"plain5\", function() horizon.attachment_set(event.entity, \"sk:k5\", {{ v = 5 }}) end)\n\
         horizon.log(\"R value \" .. horizon.attachment_get(event.entity, \"sk:k2\").v)"
    );
    let lines = run(&mut world, "sk", &body);
    assert!(
        refused(&lines, "k5").contains(&format!("at most {MAX_SYNC_KEYS} synced attachments")),
        "{lines:?}"
    );
    assert_eq!(refused(&lines, "again"), "again true nil");
    // A fifth key without sync is an ordinary key: 5 of 16 names.
    assert_eq!(refused(&lines, "plain5"), "plain5 true nil");
    assert!(lines.contains(&"value 22".to_owned()), "{lines:?}");
}

#[test]
fn copy_on_death_is_accepted_on_a_player_and_refused_on_an_entity() {
    let mut world = player_world();
    let lines = run_player(
        &mut world,
        "cd",
        &format!(
            "{TRY}\
             try(\"player\", function() horizon.attachment_set(event.entity, \"cd:gear\", {{ v = 1 }}, {{ copy_on_death = true }}) end)"
        ),
    );
    assert_eq!(refused(&lines, "player"), "player true nil");
    let saved = envelope(&world, Holder::Player(PLAYER), "cd", "gear");
    assert_eq!(saved.get(&"copy_on_death".into()), Some(&Tag::Byte(1)));
    let entity_lines = run(
        &mut world,
        "cd2",
        &format!(
            "{TRY}\
             try(\"entity\", function() horizon.attachment_set(event.entity, \"cd2:gear\", {{ v = 1 }}, {{ copy_on_death = true }}) end)"
        ),
    );
    assert!(
        refused(&entity_lines, "entity")
            .contains("copy_on_death needs a player holder: an entity does not respawn"),
        "{entity_lines:?}"
    );
}

#[test]
fn the_fifth_copied_key_and_an_oversized_copied_value_are_refused() {
    let mut world = player_world();
    let body = format!(
        "{TRY}\
         for i = 1, 4 do horizon.attachment_set(event.entity, \"cp:k\" .. i, {{ v = i }}, {{ copy_on_death = true }}) end\n\
         try(\"k5\", function() horizon.attachment_set(event.entity, \"cp:k5\", {{ v = 5 }}, {{ copy_on_death = true }}) end)\n\
         try(\"big\", function() horizon.attachment_set(event.entity, \"cp:big\", {{ c = string.rep(\"x\", 4090) }}, {{ copy_on_death = true }}) end)"
    );
    let lines = run_player(&mut world, "cp", &body);
    assert!(
        refused(&lines, "k5").contains(&format!(
            "at most {MAX_COPY_KEYS} copied-on-death attachments"
        )),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "big").contains(&format!(
            "the copied value is 4097 bytes, more than {MAX_FLAGGED_VALUE_BYTES}"
        )),
        "{lines:?}"
    );
}

#[test]
fn a_mod_makes_16_synced_writes_per_turn_and_the_next_turn_is_fresh() {
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         for i = 1, 16 do horizon.attachment_set(event.entity, \"rc:k\" .. (i % 4), {{ v = i }}, {{ sync = true }}) end\n\
         try(\"seventeenth\", function() horizon.attachment_set(event.entity, \"rc:k1\", {{ v = 0 }}, {{ sync = true }}) end)\n\
         try(\"plain\", function() horizon.attachment_set(event.entity, \"rc:p\", {{ v = 0 }}) end)"
    );
    let lines = run(&mut world, "rc", &body);
    assert!(
        refused(&lines, "seventeenth").contains(&format!(
            "a mod may make {MAX_SYNC_WRITES} synced writes in one turn"
        )),
        "{lines:?}"
    );
    // Plain writes are not counted against the synced cap.
    assert_eq!(refused(&lines, "plain"), "plain true nil");
    let next = run(
        &mut world,
        "rc",
        &format!(
            "{TRY}try(\"next\", function() horizon.attachment_set(event.entity, \"rc:k1\", {{ v = 0 }}, {{ sync = true }}) end)"
        ),
    );
    assert_eq!(refused(&next, "next"), "next true nil");
}

#[test]
fn unknown_option_keys_and_non_boolean_values_are_refused_and_change_nothing() {
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         try(\"typo\", function() horizon.attachment_set(event.entity, \"op:k\", {{ v = 1 }}, {{ sycn = true }}) end)\n\
         try(\"number\", function() horizon.attachment_set(event.entity, \"op:k\", {{ v = 1 }}, {{ sync = 1 }}) end)\n\
         try(\"opts\", function() horizon.attachment_set(event.entity, \"op:k\", {{ v = 1 }}, 5) end)\n\
         try(\"index\", function() horizon.attachment_set(event.entity, \"op:k\", {{ v = 1 }}, {{ [1] = true }}) end)"
    );
    let lines = run(&mut world, "op", &body);
    assert!(
        refused(&lines, "typo").contains("the option 'sycn' is unknown"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "number").contains("the option 'sync' must be true or false, not integer"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "opts").contains("the options must be a table, not integer"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "index").contains("the options key must be a name, not integer"),
        "{lines:?}"
    );
    assert!(world.maps[&Holder::Entity(1)].to_nbt().is_empty());
}

#[test]
fn turning_sync_off_queues_the_clients_removal_and_keeps_the_plain_value() {
    let mut world = World::new();
    run(
        &mut world,
        "fl",
        "horizon.attachment_set(event.entity, \"fl:k\", { v = 1 }, { sync = true })",
    );
    drain(&mut world, Holder::Entity(1));
    let lines = run(
        &mut world,
        "fl",
        "horizon.attachment_set(event.entity, \"fl:k\", { v = 2 })\n\
         horizon.log(\"R kept \" .. horizon.attachment_get(event.entity, \"fl:k\").v)",
    );
    assert_eq!(lines, ["kept 2"]);
    // The clients get a removal, never the unsynced value.
    let queued = drain(&mut world, Holder::Entity(1));
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].value, None);
    let saved = envelope(&world, Holder::Entity(1), "fl", "k");
    assert!(!saved.contains_key(&"sync".into()));
}

#[test]
fn removing_a_synced_value_queues_the_clients_removal() {
    let mut world = World::new();
    run(
        &mut world,
        "rs",
        "horizon.attachment_set(event.entity, \"rs:k\", { v = 1 }, { sync = true })",
    );
    drain(&mut world, Holder::Entity(1));
    let lines = run(
        &mut world,
        "rs",
        "horizon.log(\"R removed \" .. tostring(horizon.attachment_remove(event.entity, \"rs:k\")))",
    );
    assert_eq!(lines, ["removed true"]);
    let queued = drain(&mut world, Holder::Entity(1));
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].name, "k".into());
    assert_eq!(queued[0].value, None);
}

#[test]
fn a_synced_value_removed_then_set_unsynced_in_one_pass_sends_no_value() {
    // The defect this pins: a removal, then an unsynced set of the same key, in one pass.
    let mut world = World::new();
    run(
        &mut world,
        "pr",
        "horizon.attachment_set(event.entity, \"pr:k\", { v = 1 }, { sync = true })\n\
         horizon.attachment_remove(event.entity, \"pr:k\")\n\
         horizon.attachment_set(event.entity, \"pr:k\", { v = 2 })",
    );
    let queued = drain(&mut world, Holder::Entity(1));
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].value, None);
}

#[test]
fn a_client_turn_reads_a_synced_value_and_cannot_set_or_remove_it() {
    let mut world = World::new();
    run(
        &mut world,
        "cl",
        "horizon.attachment_set(event.entity, \"cl:k\", { v = 9 }, { sync = true })",
    );
    let lines = run_on(
        TurnHost::Client(&mut world),
        "cl",
        &format!(
            "{TRY}\
             horizon.log(\"R got \" .. horizon.attachment_get(event.entity, \"cl:k\").v)\n\
             horizon.log(\"R missing \" .. tostring(horizon.attachment_get(event.entity, \"cl:none\")))\n\
             try(\"set\", function() horizon.attachment_set(event.entity, \"cl:k\", {{ v = 1 }}) end)\n\
             try(\"remove\", function() horizon.attachment_remove(event.entity, \"cl:k\") end)"
        ),
        resident(),
    );
    assert!(lines.contains(&"got 9".to_owned()), "{lines:?}");
    assert!(lines.contains(&"missing nil".to_owned()), "{lines:?}");
    assert!(
        refused(&lines, "set").contains("not available in this hook"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "remove").contains("not available in this hook"),
        "{lines:?}"
    );
    // The client's refused calls changed nothing on the server's copy.
    assert_eq!(
        envelope(&world, Holder::Entity(1), "cl", "k").get(&"value".into()),
        Some(&Tag::Compound({
            let mut v = Compound::new();
            v.insert("v".into(), Tag::Int(9));
            v
        }))
    );
}

// ---- A1: block entity holders (issue 74, A1) ----

/// Runs `body` on `turn` with `block` as the event's `block`, and returns the mod's `R` lines.
fn run_block_on(turn: TurnHost<'_>, id: &str, body: &str, block: BlockEntityRef) -> Vec<String> {
    crate::host::tests::lift_handler_budget();
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let host = load(root.path(), id, body);
    let make = move |lua: &Lua, _: &TurnHost<'_>| {
        let data = lua.create_table()?;
        data.raw_set("block", lua.create_userdata(Token(block))?)?;
        new_event(lua, data, &["cancelled", "result"])
    };
    host.dispatch_turns(HOOK, turn, &|| false, &make, &mut |_, _, _| true)
        .unwrap();
    let prefix = format!("{id}: R ");
    captured_with_prefix(&prefix)
        .iter()
        .map(|line| line[prefix.len()..].to_owned())
        .collect()
}

/// Runs `body` as a server turn of `world`, on the current block entity at [`BLOCK`].
fn run_block(world: &mut World, id: &str, body: &str) -> Vec<String> {
    run_block_on(TurnHost::Server(world), id, body, block_handle(BLOCK, 0))
}

#[test]
fn a_block_entity_value_is_kept_read_back_saved_and_removed() {
    let mut world = World::new();
    let lines = run_block(
        &mut world,
        "bl1",
        "local v = { a = { b = 7 }, names = { \"x\", \"y\" } }\n\
         horizon.log(\"R set \" .. tostring(horizon.attachment_set(event.block, \"bl1:charge\", v)))\n\
         local got = horizon.attachment_get(event.block, \"bl1:charge\")\n\
         horizon.log(\"R get \" .. got.a.b .. \" \" .. got.names[2])\n\
         horizon.log(\"R missing \" .. tostring(horizon.attachment_get(event.block, \"bl1:none\")))",
    );
    assert_eq!(lines, ["set true", "get 7 y", "missing nil"]);
    // The saved form is the value as set, on the block entity's own map.
    let mut saved = reload(&world);
    assert_eq!(
        AttachmentStore::saved(&mut saved, Holder::BlockEntity(BLOCK), "bl1", "charge").unwrap(),
        Some(nested_value())
    );
    // A later turn reads it back from the reloaded world.
    let again = run_block(
        &mut saved,
        "bl1",
        "horizon.log(\"R again \" .. horizon.attachment_get(event.block, \"bl1:charge\").a.b)",
    );
    assert!(again.contains(&"again 7".to_owned()), "{again:?}");
    let removed = run_block(
        &mut world,
        "bl1",
        "horizon.log(\"R removed \" .. tostring(horizon.attachment_remove(event.block, \"bl1:charge\")))",
    );
    assert!(removed.contains(&"removed true".to_owned()), "{removed:?}");
    // The last key gone, the index goes with it: the holder's map saves nothing for the mod.
    let mut gone = reload(&world);
    assert_eq!(
        AttachmentStore::saved(&mut gone, Holder::BlockEntity(BLOCK), "bl1", "charge").unwrap(),
        None
    );
    assert_eq!(
        AttachmentStore::saved(&mut gone, Holder::BlockEntity(BLOCK), "bl1", "#index").unwrap(),
        None
    );
}

#[test]
fn a_block_entity_index_keeps_its_size_and_no_policy() {
    let mut world = World::new();
    run_block(
        &mut world,
        "bi",
        "horizon.attachment_set(event.block, \"bi:p\", { v = 2 })",
    );
    // The index is saved with no policy, and its entry for the plain key is the size alone.
    let index = envelope(&world, Holder::BlockEntity(BLOCK), "bi", "#index");
    assert!(!index.contains_key(&"sync".into()));
    assert!(!index.contains_key(&"copy_on_death".into()));
    let Some(Tag::Compound(keys)) = index.get(&"value".into()) else {
        panic!("{index:?}")
    };
    // { v = 2 } is 1 + (3 + 1 + 4) bytes: the root's end tag, then the entry.
    assert_eq!(keys.get(&"p".into()), Some(&Tag::Int(9)));
    let value = envelope(&world, Holder::BlockEntity(BLOCK), "bi", "p");
    assert!(!value.contains_key(&"sync".into()));
    assert!(!value.contains_key(&"copy_on_death".into()));
}

#[test]
fn a_block_entity_refuses_sync_and_copy_on_death_by_name_and_stores_nothing_for_them() {
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         try(\"sync\", function() horizon.attachment_set(event.block, \"sy:k\", {{ v = 1 }}, {{ sync = true }}) end)\n\
         try(\"copy\", function() horizon.attachment_set(event.block, \"sy:k\", {{ v = 1 }}, {{ copy_on_death = true }}) end)\n\
         try(\"plain\", function() horizon.attachment_set(event.block, \"sy:k\", {{ v = 1 }}) end)"
    );
    let lines = run_block(&mut world, "sy", &body);
    assert!(
        refused(&lines, "sync")
            .contains("sync does not apply to block entities: their clients need chunk-level sync"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "copy").contains(
            "copy_on_death does not apply to block entities: a block entity does not respawn"
        ),
        "{lines:?}"
    );
    assert_eq!(refused(&lines, "plain"), "plain true nil");
    // Only the plain value was kept: nothing was queued for the clients, and it has no policy.
    assert!(drain(&mut world, Holder::BlockEntity(BLOCK)).is_empty());
    let kept = envelope(&world, Holder::BlockEntity(BLOCK), "sy", "k");
    assert!(!kept.contains_key(&"sync".into()));
    assert!(!kept.contains_key(&"copy_on_death".into()));
}

#[test]
fn a_block_entity_name_must_be_the_mods_own() {
    let mut world = World::new();
    let body = format!(
        "{TRY}\
         try(\"other\", function() horizon.attachment_set(event.block, \"other:x\", {{ v = 1 }}) end)\n\
         try(\"vanilla\", function() horizon.attachment_get(event.block, \"minecraft:x\") end)"
    );
    let lines = run_block(&mut world, "fb", &body);
    assert!(
        refused(&lines, "other").contains("is outside this mod's namespace 'fb'"),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "vanilla").contains("is outside this mod's namespace 'fb'"),
        "{lines:?}"
    );
    assert!(world.maps[&Holder::BlockEntity(BLOCK)].to_nbt().is_empty());
}

#[test]
fn a_stale_or_other_level_block_entity_handle_is_refused_and_changes_nothing() {
    let mut world = World::new();
    // Each run has its own mod id: the log lines are kept per id, so one id per run keeps them apart.
    let run = |world: &mut World, id: &str, handle: BlockEntityRef| {
        let body = format!(
            "{TRY}\
             try(\"set\", function() horizon.attachment_set(event.block, \"{id}:k\", {{ v = 1 }}) end)\n\
             try(\"get\", function() horizon.attachment_get(event.block, \"{id}:k\") end)"
        );
        run_block_on(TurnHost::Server(world), id, &body, handle)
    };
    // A new generation at the same position: the block entity was replaced.
    let replaced = run(&mut world, "st1", block_handle(BLOCK, 9));
    // No block entity at the position at all.
    let nothing = run(&mut world, "st2", block_handle([9, 9, 9], 0));
    // The same position in another dimension: the host's level is the overworld.
    let nether = mod_api::__private::game::block_entity_ref(
        LevelRef::new(WorldEpoch::new(1), DimensionKey::NETHER),
        BLOCK,
        0,
    );
    let other = run(&mut world, "st3", nether);
    for lines in [&replaced, &nothing] {
        assert!(
            refused(lines, "set").contains("the holder is gone, or its handle is stale"),
            "{lines:?}"
        );
        assert!(
            refused(lines, "get").contains("the holder is gone, or its handle is stale"),
            "{lines:?}"
        );
    }
    assert!(
        refused(&other, "set").contains("the holder is in another level"),
        "{other:?}"
    );
    assert!(world.maps[&Holder::BlockEntity(BLOCK)].to_nbt().is_empty());
}

#[test]
fn a_client_turn_cannot_read_a_block_entity_and_a_server_turn_can() {
    let mut world = World::new();
    run_block(
        &mut world,
        "cr",
        "horizon.attachment_set(event.block, \"cr:k\", { v = 4 })",
    );
    let body =
        format!("{TRY}try(\"get\", function() horizon.attachment_get(event.block, \"cr:k\") end)");
    let lines = run_block_on(
        TurnHost::Client(&mut world),
        "cr",
        &body,
        block_handle(BLOCK, 0),
    );
    assert!(
        refused(&lines, "get").contains("a client turn cannot read a block entity's attachments"),
        "{lines:?}"
    );
}

#[test]
fn a_block_entity_keeps_the_key_and_value_caps_of_an_entity() {
    let mut world = World::new();
    let body = format!(
        "{TRY}{BIG}\
         for i = 1, {MAX_KEYS} do horizon.attachment_set(event.block, \"bc:k\" .. i, {{ v = i }}) end\n\
         try(\"k17\", function() horizon.attachment_set(event.block, \"bc:k17\", {{ v = 17 }}) end)\n\
         try(\"over\", function() horizon.attachment_set(event.block, \"bc:k1\", big(1)) end)\n\
         try(\"fits\", function() horizon.attachment_set(event.block, \"bc:k1\", big(0)) end)"
    );
    let lines = run_block(&mut world, "bc", &body);
    assert!(
        refused(&lines, "k17").contains(&format!("at most {MAX_KEYS} attachments")),
        "{lines:?}"
    );
    assert!(
        refused(&lines, "over").contains(&format!(
            "the value is {} bytes, more than {MAX_VALUE_BYTES}",
            MAX_VALUE_BYTES + 1
        )),
        "{lines:?}"
    );
    assert_eq!(refused(&lines, "fits"), "fits true nil");
}

#[test]
fn a_block_entity_holds_at_most_256_kib_of_values_per_mod() {
    let mut world = World::new();
    let body = format!(
        "{TRY}{BIG}\
         for i = 1, 4 do horizon.attachment_set(event.block, \"bt:b\" .. i, big(0)) end\n\
         try(\"fifth\", function() horizon.attachment_set(event.block, \"bt:b5\", {{ v = 1 }}) end)"
    );
    let lines = run_block(&mut world, "bt", &body);
    assert!(
        refused(&lines, "fifth")
            .contains("the holder's values would be 262153 bytes, more than 262144"),
        "{lines:?}"
    );
}

#[test]
fn a_mod_creates_at_most_the_block_entity_key_guard_and_then_only_overwrites() {
    let mut world = World::new();
    // The guard's count is the keys a mod created in the process: MAX_BLOCK_ENTITY_NEW_KEYS of them
    // on one block entity, all allowed.
    let first = run_block(
        &mut world,
        "rg",
        &format!(
            "for i = 1, {MAX_BLOCK_ENTITY_NEW_KEYS} do horizon.attachment_set(event.block, \"rg:k\" .. i, {{ v = i }}) end"
        ),
    );
    assert!(first.is_empty(), "{first:?}");
    // An overwrite of an existing key is not a new key: it is allowed past the guard.
    let again = run_block(
        &mut world,
        "rg",
        &format!(
            "{TRY}try(\"again\", function() horizon.attachment_set(event.block, \"rg:k1\", {{ v = 9 }}) end)"
        ),
    );
    assert_eq!(refused(&again, "again"), "again true nil");
    // The next new key, on the other block entity, is refused by name.
    let other = run_block_on(
        TurnHost::Server(&mut world),
        "rg",
        &format!(
            "{TRY}try(\"new\", function() horizon.attachment_set(event.block, \"rg:new\", {{ v = 1 }}) end)"
        ),
        block_handle(OTHER_BLOCK, 0),
    );
    assert!(
        refused(&other, "new").contains(&format!(
            "at most {MAX_BLOCK_ENTITY_NEW_KEYS} block-entity attachment keys in one run of the game"
        )),
        "{other:?}"
    );
    assert!(
        world.maps[&Holder::BlockEntity(OTHER_BLOCK)]
            .to_nbt()
            .is_empty(),
        "a refused new key stores nothing"
    );
}
