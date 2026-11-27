//! The raw NBT merge under the `nbt` power (C5): `merge_nbt` and `merge_data` gated under `nbt`, the
//! player refusal and the protected keys. The world double answers the entity kind and the two merges;
//! every test loads its own mod, whose handler logs what each call did.

use std::collections::HashMap;
use std::path::Path;

use mlua::Lua;
use mod_api::__private::game::{BackendMutRefOf, BackendRefOf};
use mod_api::prelude::{
    BlockEntitiesBackendMut, EntitiesBackend, EntitiesBackendMut, PlayersBackend, PlayersBackendMut,
};
use mod_api::{
    Backend, BackendMut, BackendMutRef, BackendRef, BlockEntityError, BlockEntityRef, ClientHost,
    Compound, DimensionKey, EntityRef, EntityTypeKey, Host, LevelRef, PlayerDataRefusal, PlayerRef,
    ServerHost, WorldEpoch,
};

use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::{Event, new_event};
use crate::handles::Token;
use crate::host::LuaHost;
use crate::host::LuaMod;
use crate::hostcall::TurnHost;
use crate::manifest::Power;

const HOOK: &str = "block_receive_game";
/// The player's entity id, and the player's uuid.
const PLAYER_ENTITY: u32 = 2;
const PLAYER_UUID: u128 = 7;
/// A non-player entity: its kind is a pig, so a raw merge into it is allowed.
const PIG_ENTITY: u32 = 3;

/// The test world: one persistent compound per player, and the counts of what the host was asked.
#[derive(Default)]
struct World {
    data: HashMap<u128, Compound>,
    /// Persistent-data writes the host accepted.
    writes: u32,
    /// Raw entity merges the host was asked for.
    merges: u32,
    /// Raw block entity merges the host was asked for.
    block_merges: u32,
}

fn level() -> LevelRef {
    LevelRef::new(WorldEpoch::new(1), DimensionKey::OVERWORLD)
}

fn entity(id: u32) -> EntityRef {
    EntityRef::new(WorldEpoch::new(1), id)
}

fn player() -> PlayerRef {
    mod_api::__private::game::player_ref(entity(PLAYER_ENTITY), PLAYER_UUID)
}

fn block() -> BlockEntityRef {
    mod_api::__private::game::block_entity_ref(level(), [1, 2, 3], 0)
}

/// The entity kind of an id: the player's, or a pig's. Both resolve in the vanilla registry.
fn kind_of(id: u32) -> Option<EntityTypeKey> {
    let name = if id == PLAYER_ENTITY {
        "minecraft:player"
    } else {
        "minecraft:pig"
    };
    mod_api::__private::resolved_entity_type_key(name)
}

impl Host for World {
    fn backend(&self) -> Option<BackendRef<'_>> {
        Some(BackendRefOf::of(self))
    }
}
impl Backend for World {}
impl mod_api::prelude::PlayersClientBackend for World {}
mod_api::impl_host_areas!(World);
mod_api::impl_backend_areas!(World, except(entities, players));

impl PlayersBackend for World {
    fn player_persistent_data(&self, player: PlayerRef) -> Option<Compound> {
        Some(self.data.get(&player.uuid()).cloned().unwrap_or_default())
    }
}

impl EntitiesBackend for World {
    fn entity_kind(&self, entity: EntityRef) -> Option<EntityTypeKey> {
        kind_of(entity.id())
    }
}

impl ServerHost for World {
    fn backend_mut(&mut self) -> Option<BackendMutRef<'_>> {
        Some(BackendMutRefOf::of(self))
    }
}
impl BackendMut for World {}
mod_api::impl_server_host_areas!(World);
mod_api::impl_backend_mut_areas!(World, except(entities, players, block_entities));

impl PlayersBackendMut for World {
    fn player_set_persistent_data(
        &mut self,
        player: PlayerRef,
        data: Compound,
    ) -> Result<(), PlayerDataRefusal> {
        self.writes += 1;
        self.data.insert(player.uuid(), data);
        Ok(())
    }
}

impl EntitiesBackendMut for World {
    fn entity_merge_nbt(&mut self, _: EntityRef, _: Compound) -> bool {
        self.merges += 1;
        true
    }
}

impl BlockEntitiesBackendMut for World {
    fn block_entity_merge_data(
        &mut self,
        _: BlockEntityRef,
        _: Compound,
    ) -> Result<bool, BlockEntityError> {
        self.block_merges += 1;
        Ok(true)
    }
}

impl ClientHost for World {}
mod_api::impl_client_host_areas!(World);

/// Loads mod `id` whose hook runs `body` with `event.player`, `event.entity` and `event.block`.
fn load(root: &Path, id: &str, body: &str, powers: &[Power]) -> LuaHost {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let source = format!("horizon.on(\"{HOOK}\", function(event)\n{body}\nend)\n");
    std::fs::write(dir.join(ENTRY), source).unwrap();
    let mut module = LuaMod::load(&Discovered {
        dir,
        id: id.to_owned(),
        version: "1".to_owned(),
        side: Side::Server,
    })
    .unwrap();
    module.set_powers(powers.to_vec());
    LuaHost::new(vec![module])
}

/// The event of one fire: the player, the player's entity, a pig and a block entity handle.
fn build() -> impl Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event> {
    move |lua: &Lua, _: &TurnHost<'_>| {
        let data = lua.create_table()?;
        data.raw_set("player", lua.create_userdata(Token(player()))?)?;
        data.raw_set("entity", lua.create_userdata(Token(entity(PLAYER_ENTITY)))?)?;
        data.raw_set("pig", lua.create_userdata(Token(entity(PIG_ENTITY)))?)?;
        data.raw_set("block", lua.create_userdata(Token(block()))?)?;
        new_event(lua, data, &["cancelled", "result"])
    }
}

/// Fires the hook once on `world` and returns the mod's log lines. The entity-type registry is loaded
/// first, as the game loads it, so the player's kind resolves (a repeat load keeps the first table).
fn run(world: &mut World, host: &LuaHost) -> Vec<String> {
    let _ = mod_api::testing::FakeHost::new().resolve_entity_types();
    crate::host::tests::lift_handler_budget();
    host.dispatch_turns(
        HOOK,
        TurnHost::Server(world),
        &|| false,
        &build(),
        &mut |_, _, _| true,
    )
    .unwrap();
    host.mods()[0].log_lines()
}

/// The fixed body every test shares: `pcall` the call, and log whether it passed and why not.
fn logged(call: &str) -> String {
    format!(
        "local ok, err = pcall(function() {call} end)\nhorizon.log(tostring(ok) .. \" \" .. tostring(err))"
    )
}

// --- the raw merge under nbt ------------------------------------------------------------------

/// Runs mod `id`, whose hook calls `call` (`event.pig` is a pig, `event.entity` the player), with `powers`.
fn merge(root: &Path, id: &str, call: &str, powers: &[Power]) -> (Vec<String>, World) {
    let host = load(root, id, &logged(call), powers);
    let mut world = World::default();
    let lines = run(&mut world, &host);
    (lines, world)
}

#[test]
fn a_raw_merge_needs_the_nbt_power_and_is_refused_without_it() {
    let root = tempfile::tempdir().unwrap();
    let call = "event.pig:merge_nbt({ Health = 5 })";
    let (lines, world) = merge(root.path(), "nbt_no_power", call, &[]);
    assert_eq!(world.merges, 0, "the host is not called");
    assert!(lines[0].starts_with("false "), "{lines:?}");
    let (lines, world) = merge(root.path(), "nbt_power", call, &[Power::Nbt]);
    assert_eq!(
        world.merges, 1,
        "with the power, the clean merge reaches the host once"
    );
    assert_eq!(lines, ["true nil"]);
}

#[test]
fn a_raw_merge_into_a_player_is_refused_even_with_the_power() {
    let root = tempfile::tempdir().unwrap();
    let (lines, world) = merge(
        root.path(),
        "nbt_player",
        "event.entity:merge_nbt({ Health = 5 })",
        &[Power::Nbt],
    );
    assert_eq!(world.merges, 0);
    assert!(
        lines[0].contains("horizon: merge_nbt refused: players cannot be changed with raw NBT"),
        "{lines:?}"
    );
}

#[test]
fn each_protected_key_refuses_a_raw_merge_and_none_reaches_the_host() {
    let root = tempfile::tempdir().unwrap();
    for (n, key) in [
        "UUID",
        "id",
        "Pos",
        "Passengers",
        "Inventory",
        "EnderItems",
        "horizon:attachments",
    ]
    .iter()
    .enumerate()
    {
        let id = format!("nbt_key_{n}");
        let (lines, world) = merge(
            root.path(),
            &id,
            &format!("event.pig:merge_nbt({{ [\"{key}\"] = 1, Health = 5 }})"),
            &[Power::Nbt],
        );
        assert_eq!(world.merges, 0, "{key}: host not called");
        assert!(
            lines[0].contains(&format!(
                "horizon: merge_nbt refused: key {key} cannot be merged"
            )),
            "{key}: {lines:?}"
        );
    }
}

#[test]
fn the_protected_keys_are_case_sensitive_and_top_level_only() {
    let root = tempfile::tempdir().unwrap();
    let (_, world) = merge(
        root.path(),
        "nbt_case",
        "event.pig:merge_nbt({ uuid = 1, ID = 2, Health = { id = 3 } })",
        &[Power::Nbt],
    );
    assert_eq!(
        world.merges, 1,
        "lowercase uuid, ID and a nested id are not refused"
    );
}

#[test]
fn a_block_entity_merge_needs_the_nbt_power_and_refuses_a_protected_key() {
    let root = tempfile::tempdir().unwrap();
    let (lines, world) = merge(
        root.path(),
        "nbt_be_no_power",
        "event.block:merge_data({ Items = {} })",
        &[],
    );
    assert_eq!(world.block_merges, 0);
    assert!(lines[0].starts_with("false "), "{lines:?}");
    let (lines, world) = merge(
        root.path(),
        "nbt_be_power",
        "event.block:merge_data({ Items = {} })",
        &[Power::Nbt],
    );
    assert_eq!(world.block_merges, 1);
    assert_eq!(lines, ["true nil"]);
    let (lines, world) = merge(
        root.path(),
        "nbt_be_key",
        "event.block:merge_data({ Pos = {} })",
        &[Power::Nbt],
    );
    assert_eq!(world.block_merges, 0);
    assert!(
        lines[0].contains("horizon: merge_data refused: key Pos cannot be merged"),
        "{lines:?}"
    );
}

#[test]
fn a_block_entity_merge_refuses_its_position_keys() {
    let root = tempfile::tempdir().unwrap();
    for (name, key) in [("nbt_be_x", "x"), ("nbt_be_y", "y"), ("nbt_be_z", "z")] {
        let (lines, world) = merge(
            root.path(),
            name,
            &format!("event.block:merge_data({{ {key} = 5 }})"),
            &[Power::Nbt],
        );
        assert_eq!(world.block_merges, 0, "{key}");
        assert!(
            lines[0].contains(&format!("key {key} cannot be merged")),
            "{lines:?}"
        );
    }
}

#[test]
fn an_empty_merge_changes_nothing_and_is_still_a_merge() {
    // A merge adds and changes keys, it cannot remove one: an empty table is a merge that changes
    // nothing, and the host is still asked (it answers whether the data changed).
    let root = tempfile::tempdir().unwrap();
    let (lines, world) = merge(
        root.path(),
        "nbt_empty",
        "event.pig:merge_nbt({})",
        &[Power::Nbt],
    );
    assert_eq!(world.merges, 1);
    assert_eq!(lines, ["true nil"]);
}
