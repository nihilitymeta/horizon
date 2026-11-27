//! The `loot_drops` hook's context and drops in Lua (#66 chunk A): what a handler sees of the
//! context, and what it does to the drops. The host is the vocab's fake host with the items the
//! tests name in its item table. The block test's host also names block states through the test
//! registry, since the fake host names none.

use std::path::Path;

use mod_api::testing::FakeHost;
use mod_api::{BackendMutRef, BackendRef, BlockState, Host, Identifier, ItemStack, ServerHost};

use crate::discover::{Discovered, ENTRY, Side};
use crate::host::{LuaHost, LuaMod};

/// The fake host with `minecraft:stick` in its item table, as the game's load gives it. Build it
/// before the stacks the test names: the registry installs its own tables.
fn stick_host() -> FakeHost {
    let mut host = FakeHost::new();
    host.registries
        .resolve_items(["minecraft:stick"].map(|id| Identifier::parse(id).unwrap()))
        .unwrap();
    host
}

/// The fake host, with block states named through the test registry: a `loot_drops` context's
/// block state converts through the host, and the fake host names none.
struct Stated {
    fake: FakeHost,
}

impl Host for Stated {
    fn backend(&self) -> Option<BackendRef<'_>> {
        Host::backend(&self.fake)
    }
}
mod_api::impl_host_areas!(Stated, except(block_states, items));
impl mod_api::prelude::ItemsHost for Stated {
    fn item_key(&self, id: &Identifier) -> Option<mod_api::ItemKey> {
        mod_api::prelude::ItemsHost::item_key(&self.fake, id)
    }
    fn max_stack_size(&self, item: mod_api::ItemKey) -> Option<u32> {
        mod_api::prelude::ItemsHost::max_stack_size(&self.fake, item)
    }
}
impl mod_api::prelude::BlockStatesHost for Stated {
    fn block_state_string(&self, state: BlockState) -> Option<String> {
        use crate::stacks::StateHost;
        crate::stacks::GlobalHost.state_text(&state)
    }
    fn block_state_id(&self, name: &str) -> Option<BlockState> {
        use crate::stacks::StateHost;
        crate::stacks::GlobalHost.state_parse(name)
    }
}
impl ServerHost for Stated {
    fn backend_mut(&mut self) -> Option<BackendMutRef<'_>> {
        ServerHost::backend_mut(&mut self.fake)
    }
}
mod_api::impl_server_host_areas!(Stated);

/// Loads one mod `id` whose main.luau is `source`, from its own folder under `root`.
fn load_one(root: &Path, id: &str, source: &str) -> LuaMod {
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

/// The registry's `minecraft:stone` state. The registry must be installed first (`block_state_tests::registry`).
fn stone() -> BlockState {
    let block = mod_api::__private::block_key("minecraft:stone").unwrap();
    BlockState::of(mod_api::__private::block_definitions().unwrap(), block, &[]).unwrap()
}

/// A stick, in the stick host's item table.
fn stick(count: u32) -> ItemStack {
    ItemStack::new(
        mod_api::__private::item_key("minecraft:stick").unwrap(),
        count,
    )
    .unwrap()
}

/// Fires `loot_drops` at one mod whose main.luau is `source`, with `context` and one stick in the
/// drops, on `server`. Returns the mod's log lines and the drops after the hook.
fn fire(
    source: &str,
    context: &mod_api::LootDrops<'_>,
    server: &mut dyn ServerHost,
    drops: Vec<ItemStack>,
) -> (Vec<String>, Vec<(String, u32)>) {
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load_one(root.path(), "mymod", source)]);
    let mut drops = drops;
    let mut event = mod_api::LootDropsEvent {
        context,
        drops: &mut drops,
    };
    // The game is in its world phase when a hook fires; the loaded mods leave the thread in Packages.
    let _phase = mod_api::__private::PhaseOwner::scope(mod_api::__private::ModPhase::World);
    mod_api::Hooks::loot_drops(&host, server, &mut event);
    let after = drops
        .iter()
        .map(|stack| {
            (
                mod_api::__private::item_key_id(stack.item())
                    .unwrap()
                    .to_owned(),
                stack.count(),
            )
        })
        .collect();
    (host.mods()[0].log_lines(), after)
}

/// A mob-death context: a zombie with entity id 7, killed by a player, dropping from its table.
fn mob_context() -> mod_api::LootDrops<'static> {
    mod_api::LootDrops::from(mod_api::__private::built::LootDrops {
        table: "minecraft:entities/zombie",
        dimension: "minecraft:overworld",
        origin: [0.5, 64.0, 0.5],
        block: None,
        entity: Some((7, "minecraft:zombie")),
        player: Some(1),
        damage_source: None,
        weapon: None,
        tool: None,
        explosion: None,
        used: false,
    })
}

#[test]
fn a_block_break_context_reaches_the_handler_with_its_block_and_tool() {
    crate::block_state_tests::registry();
    let mut server = Stated { fake: stick_host() };
    let tool = stick(1);
    let block = stone();
    let context = mod_api::LootDrops::from(mod_api::__private::built::LootDrops {
        table: "minecraft:blocks/stone",
        dimension: "minecraft:overworld",
        origin: [3.5, 64.5, -1.5],
        block: Some(([3, 64, -2], block)),
        entity: None,
        player: None,
        damage_source: None,
        weapon: None,
        tool: Some(&tool),
        explosion: None,
        used: false,
    });
    let (log, _) = fire(
        "horizon.on(\"loot_drops\", function(event) local c = event.context horizon.log(c.table) horizon.log(c.dimension) horizon.log(tostring(c.block.pos.x)) horizon.log(c.block.state.id) horizon.log(c.tool.id) end)",
        &context,
        &mut server,
        vec![stick(1)],
    );
    assert_eq!(
        log,
        [
            "minecraft:blocks/stone",
            "minecraft:overworld",
            "3",
            "minecraft:stone",
            "minecraft:stick"
        ]
    );
}

#[test]
fn a_mob_death_context_reaches_the_handler_with_its_entity_and_player() {
    let mut server = stick_host();
    let (log, _) = fire(
        "horizon.on(\"loot_drops\", function(event) horizon.log(event.context.entity.id .. \" \" .. event.context.entity.type .. \" \" .. tostring(event.context.player ~= nil) .. \" \" .. tostring(event.context.block == nil)) end)",
        &mob_context(),
        &mut server,
        vec![stick(1)],
    );
    assert_eq!(log, ["7 minecraft:zombie true true"]);
}

#[test]
fn the_context_fields_of_a_mob_death_reach_the_handler_with_their_types() {
    let mut server = stick_host();
    let (log, _) = fire(
        "horizon.on(\"loot_drops\", function(event) local c = event.context horizon.log(tostring(c.origin.y == 64) .. \" \" .. tostring(c.used) .. \" \" .. tostring(c.explosion == nil) .. \" \" .. tostring(c.damage_source == nil)) end)",
        &mob_context(),
        &mut server,
        vec![stick(1)],
    );
    assert_eq!(log, ["true false true true"]);
}

#[test]
fn a_death_weapon_reaches_the_handler_as_weapon_item() {
    let mut server = stick_host();
    let weapon = stick(1);
    let context = mod_api::LootDrops::from(mod_api::__private::built::LootDrops {
        table: "minecraft:entities/zombie",
        dimension: "minecraft:overworld",
        origin: [0.5, 64.0, 0.5],
        block: None,
        entity: Some((7, "minecraft:zombie")),
        player: Some(1),
        damage_source: Some(mod_api::HurtSource {
            damage_type: "minecraft:player_attack",
            causing: 1,
            direct: 1,
            position: None,
        }),
        weapon: Some(&weapon),
        tool: None,
        explosion: None,
        used: false,
    });
    let (log, _) = fire(
        "horizon.on(\"loot_drops\", function(event) horizon.log(event.context.weapon_item.id .. \" \" .. tostring(event.context.weapon_item.count)) end)",
        &context,
        &mut server,
        vec![stick(1)],
    );
    assert_eq!(log, ["minecraft:stick 1"]);
}

#[test]
fn a_context_without_a_death_weapon_reaches_the_handler_as_nil_weapon_item() {
    let mut server = stick_host();
    let (log, _) = fire(
        "horizon.on(\"loot_drops\", function(event) horizon.log(tostring(event.context.weapon_item == nil)) end)",
        &mob_context(),
        &mut server,
        vec![stick(1)],
    );
    assert_eq!(log, ["true"]);
}

#[test]
fn a_stack_the_handler_appends_reaches_the_drops() {
    let mut server = stick_host();
    let (_, drops) = fire(
        "horizon.on(\"loot_drops\", function(event) local d = {} for i, s in ipairs(event.drops) do d[i] = s end d[#d + 1] = {id = \"minecraft:stick\", count = 4} event.drops = d end)",
        &mob_context(),
        &mut server,
        vec![stick(1)],
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
fn a_wrong_typed_drops_value_is_refused_and_the_previous_list_is_kept() {
    let mut server = stick_host();
    let (_, drops) = fire(
        "horizon.on(\"loot_drops\", function(event) event.drops = 5 end)",
        &mob_context(),
        &mut server,
        vec![stick(1)],
    );
    assert_eq!(drops, [("minecraft:stick".to_owned(), 1)]);
}

/// A load reader with no registries, tags or settings: the data-load context of these tests.
struct NoLoad;

impl mod_api::LoadReader for NoLoad {
    fn registry(&self, _name: &str) -> Option<std::sync::Arc<[String]>> {
        None
    }
    fn fluid_tags(&self) -> std::sync::Arc<[(String, std::sync::Arc<[String]>)]> {
        std::sync::Arc::from(Vec::new())
    }
    fn setting(&self, _mod_id: &str, _key: &str) -> Option<String> {
        None
    }
}

/// The simple dungeon chest's table as the data packs load it: one pool with one stick.
fn chest_table() -> serde_json::Value {
    serde_json::json!({
        "type": "minecraft:chest",
        "pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:stick"}]}]
    })
}

/// Fires `loot_table_load` for table `id` at one mod whose main.luau is `source`. Returns the mod's
/// log lines; the table is changed in place, as the hook's output.
fn fire_table(source: &str, id: &str, table: &mut serde_json::Value) -> Vec<String> {
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load_one(root.path(), "mymod", source)]);
    let load = NoLoad;
    let context = mod_api::OwningLoad::new(mod_api::DataLoadOrigin::ServerDataLoad, &load);
    let mut event = mod_api::LootTableLoadEvent { id, table };
    let _phase = mod_api::__private::PhaseOwner::scope(mod_api::__private::ModPhase::World);
    mod_api::Hooks::loot_table_load(&host, &context, &mut event);
    host.mods()[0].log_lines()
}

#[test]
fn an_entry_a_handler_adds_to_a_loaded_table_reaches_the_table() {
    // The event's tables are read-only: a change is a new table assigned to `event.table`.
    let mut table = chest_table();
    fire_table(
        "horizon.on(\"loot_table_load\", function(event) if event.id == \"minecraft:chests/simple_dungeon\" then local old = event.table.pools[1] local entries = {} for i, e in ipairs(old.entries) do entries[i] = e end entries[#entries + 1] = {type = \"minecraft:item\", name = \"minecraft:stone\"} event.table = {type = event.table.type, pools = {{rolls = old.rolls, entries = entries}}} end end)",
        "minecraft:chests/simple_dungeon",
        &mut table,
    );
    assert_eq!(
        table["pools"][0]["entries"],
        serde_json::json!([
            {"type": "minecraft:item", "name": "minecraft:stick"},
            {"type": "minecraft:item", "name": "minecraft:stone"}
        ])
    );
}

#[test]
fn an_in_place_edit_of_a_loaded_table_is_refused_and_the_table_is_kept() {
    let mut table = chest_table();
    fire_table(
        "horizon.on(\"loot_table_load\", function(event) event.table.pools[1].rolls = 9 end)",
        "minecraft:chests/simple_dungeon",
        &mut table,
    );
    assert_eq!(table, chest_table());
}

#[test]
fn a_table_a_handler_assigns_replaces_the_loaded_table() {
    let mut table = chest_table();
    fire_table(
        "horizon.on(\"loot_table_load\", function(event) event.table = {type = \"minecraft:chest\", pools = {{rolls = 2, entries = {}}}} end)",
        "minecraft:chests/simple_dungeon",
        &mut table,
    );
    assert_eq!(
        table,
        serde_json::json!({"type": "minecraft:chest", "pools": [{"rolls": 2, "entries": []}]})
    );
}

#[test]
fn a_wrong_typed_table_is_refused_and_the_loaded_table_is_kept() {
    let mut table = chest_table();
    fire_table(
        "horizon.on(\"loot_table_load\", function(event) event.table = 5 end)",
        "minecraft:chests/simple_dungeon",
        &mut table,
    );
    assert_eq!(table, chest_table());
}

#[test]
fn a_table_with_a_null_in_an_array_is_not_delivered() {
    let mut table = serde_json::json!({"pools": [null]});
    let log = fire_table(
        "horizon.on(\"loot_table_load\", function(event) horizon.log(\"ran\") end)",
        "minecraft:chests/simple_dungeon",
        &mut table,
    );
    assert!(log.is_empty());
    assert_eq!(table, serde_json::json!({"pools": [null]}));
}

#[test]
fn a_copy_with_one_number_changed_keeps_its_empty_array_and_object() {
    // `conditions` is `[]` and `predicate` is `{}` as the JSON gave them: the copy keeps both.
    let mut table = serde_json::json!({
        "type": "minecraft:chest",
        "pools": [{"rolls": 1, "conditions": [], "predicate": {}, "entries": [{"type": "minecraft:item", "name": "minecraft:stick"}]}]
    });
    fire_table(
        "horizon.on(\"loot_table_load\", function(event) local copy = {} for k, v in pairs(event.table) do copy[k] = v end local pool = {} for k, v in pairs(event.table.pools[1]) do pool[k] = v end pool.rolls = 3 copy.pools = {pool} event.table = copy end)",
        "minecraft:chests/simple_dungeon",
        &mut table,
    );
    assert_eq!(table["pools"][0]["rolls"], serde_json::json!(3));
    assert_eq!(table["pools"][0]["conditions"], serde_json::json!([]));
    assert_eq!(table["pools"][0]["predicate"], serde_json::json!({}));
}

/// The tag lookup the vocab's test listeners use: a condition's event carries one, and a Lua
/// handler does not read it in this slice.
struct NoTags;
impl mod_api::Tags for NoTags {}

/// Fires `loot_condition` for `kind` with `data` and `ctx` at one mod whose main.luau is `source`.
/// Returns the mod's log lines and the answer (`holds`), which starts `false`.
fn fire_condition(
    source: &str,
    kind: &str,
    data: &serde_json::Value,
    ctx: &mod_api::LootContext,
) -> (Vec<String>, bool) {
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load_one(root.path(), "mymod", source)]);
    let tags = NoTags;
    let mut event = mod_api::LootConditionEvent {
        kind,
        data,
        ctx,
        holds: false,
        tags: &tags,
    };
    let _phase = mod_api::__private::PhaseOwner::scope(mod_api::__private::ModPhase::World);
    mod_api::Hooks::loot_condition(&host, &mut event);
    (host.mods()[0].log_lines(), event.holds)
}

/// A loot context with a seed of 7 and a luck of 2.5, the rest as an empty roll has them.
fn luck_context() -> mod_api::LootContext {
    let mut ctx = mod_api::LootContext::new(7);
    ctx.luck = 2.5;
    ctx
}

#[test]
fn a_condition_a_handler_claims_reaches_it_with_its_kind_data_and_context() {
    // The handler sets `holds` only when every value it reads is the one the test gave: a wrong
    // kind, data, or context field leaves the answer false.
    let (_, holds) = fire_condition(
        "horizon.on(\"loot_condition\", function(event) if event.kind ~= \"mymod:is_night\" then return end if event.data.flag == 3 and event.ctx.luck == 2.5 and event.ctx.seed == \"7\" then event.holds = true end end)",
        "mymod:is_night",
        &serde_json::json!({"flag": 3}),
        &luck_context(),
    );
    assert!(holds);
}

/// A handler that answers `holds` only when `ctx.seed` is exactly `seed` as a Lua string.
fn seed_condition_source(seed: &str) -> String {
    format!(
        "horizon.on(\"loot_condition\", function(event) if event.kind == \"mymod:is_night\" and type(event.ctx.seed) == \"string\" and event.ctx.seed == \"{seed}\" then event.holds = true end end)"
    )
}

#[test]
fn a_condition_with_a_seed_past_2_to_the_53_is_delivered_as_its_exact_decimal_string() {
    // A roll's seed is a full i64; a Lua number cannot hold it, so it arrives as its decimal text.
    let seed = -5_299_148_278_949_906_928_i64;
    let (log, holds) = fire_condition(
        &seed_condition_source(&seed.to_string()),
        "mymod:is_night",
        &serde_json::json!({}),
        &mod_api::LootContext::new(seed),
    );
    assert!(holds, "log: {log:?}");
    assert!(log.is_empty(), "log: {log:?}");
}

#[test]
fn a_condition_with_the_largest_seed_is_delivered_as_its_exact_decimal_string() {
    let seed = i64::MAX;
    let (log, holds) = fire_condition(
        &seed_condition_source(&seed.to_string()),
        "mymod:is_night",
        &serde_json::json!({}),
        &mod_api::LootContext::new(seed),
    );
    assert!(holds, "log: {log:?}");
    assert!(log.is_empty(), "log: {log:?}");
}

#[test]
fn a_condition_another_handler_claims_leaves_holds_false() {
    let (_, holds) = fire_condition(
        "horizon.on(\"loot_condition\", function(event) if event.kind ~= \"mymod:other\" then return end event.holds = true end)",
        "mymod:is_night",
        &serde_json::json!({}),
        &luck_context(),
    );
    assert!(!holds);
}

#[test]
fn a_wrong_typed_condition_answer_is_refused_and_holds_stays_false() {
    let (_, holds) = fire_condition(
        "horizon.on(\"loot_condition\", function(event) if event.kind == \"mymod:is_night\" then event.holds = \"yes\" end end)",
        "mymod:is_night",
        &serde_json::json!({}),
        &luck_context(),
    );
    assert!(!holds);
}

/// A Lua value of `source`, evaluated in `lua`.
fn lua_value(lua: &mlua::Lua, source: &str) -> mlua::Value {
    lua.load(source)
        .eval::<mlua::Value>()
        .expect("the test's Lua evaluates")
}

/// The stack a handler sees (its fields are the engine's, written as Lua), with an enchantment, a
/// potion, a renamed item and a removed component.
fn loot_stack_with_everything() -> mod_api::world::snapshots::LootStack {
    let mut stack = mod_api::world::snapshots::LootStack::new(
        mod_api::__private::item_key("minecraft:stick").unwrap(),
        2,
    );
    stack.damage = 3;
    stack.enchantments = vec![("minecraft:sharpness".to_owned(), 3)];
    stack.potion = Some("minecraft:swiftness".to_owned());
    stack.map = true;
    stack.ominous_amplifier = Some(1);
    stack.stew_effects = vec![("minecraft:night_vision".to_owned(), 100)];
    stack.names = vec![(
        "minecraft:custom_name".to_owned(),
        mod_api::Text::literal("Gold"),
    )];
    stack.components = vec![
        ("minecraft:custom_name".to_owned(), None),
        ("minecraft:damage".to_owned(), Some(mod_api::Tag::Int(4))),
    ];
    stack
}

#[test]
fn a_loot_stack_written_to_lua_reads_back_unchanged() {
    let _host = stick_host();
    let lua = mlua::Lua::new();
    let stack = loot_stack_with_everything();
    let value = <mod_api::world::snapshots::LootStack as event_horizon_lua_convert::scalar::ToLuaValue>::to_lua_value(&stack, &lua)
        .unwrap();
    assert_eq!(
        event_horizon_lua_convert::loot::loot_stack_from_lua(&value),
        Ok(stack)
    );
}

#[test]
fn a_handler_that_changes_the_count_gets_that_count_and_nothing_else() {
    let _host = stick_host();
    let lua = mlua::Lua::new();
    let stack = loot_stack_with_everything();
    let value = <mod_api::world::snapshots::LootStack as event_horizon_lua_convert::scalar::ToLuaValue>::to_lua_value(&stack, &lua)
        .unwrap();
    let mlua::Value::Table(table) = &value else {
        panic!("a loot stack is a table")
    };
    table.raw_set("count", 29).unwrap();
    let mut expected = stack;
    expected.count = 29;
    assert_eq!(
        event_horizon_lua_convert::loot::loot_stack_from_lua(&value),
        Ok(expected)
    );
}

#[test]
fn an_unknown_item_a_fractional_count_and_an_exploration_map_are_refused_by_name() {
    let _host = stick_host();
    let lua = mlua::Lua::new();
    let unknown = lua_value(&lua, "return {item = 'minecraft:nope', count = 1}");
    let err = event_horizon_lua_convert::loot::loot_stack_from_lua(&unknown).unwrap_err();
    assert!(err.contains("minecraft:nope"), "{err}");
    let fraction = lua_value(&lua, "return {item = 'minecraft:stick', count = 1.5}");
    let err = event_horizon_lua_convert::loot::loot_stack_from_lua(&fraction).unwrap_err();
    assert!(err.contains("count"), "{err}");
    let explore = lua_value(
        &lua,
        "return {item = 'minecraft:stick', count = 1, exploration = {destination = 'x'}}",
    );
    let err = event_horizon_lua_convert::loot::loot_stack_from_lua(&explore).unwrap_err();
    assert_eq!(err, "exploration maps are read-only");
}

#[test]
fn an_unknown_key_is_refused_by_name() {
    let _host = stick_host();
    let lua = mlua::Lua::new();
    let typo = lua_value(
        &lua,
        "return {item = 'minecraft:stick', count = 1, colour = 'red'}",
    );
    let err = event_horizon_lua_convert::loot::loot_stack_from_lua(&typo).unwrap_err();
    assert!(err.contains("colour"), "{err}");
}

#[test]
fn a_list_longer_than_256_entries_is_refused() {
    let _host = stick_host();
    let lua = mlua::Lua::new();
    let pairs = vec!["{'minecraft:sharpness', 1}"; 257].join(",");
    let source =
        format!("return {{item = 'minecraft:stick', count = 1, enchantments = {{{pairs}}}}}");
    let err = event_horizon_lua_convert::loot::loot_stack_from_lua(&lua_value(&lua, &source))
        .unwrap_err();
    assert!(err.contains("256"), "{err}");
}

/// Fires `loot_function` for `kind` with `data`, `ctx` and `stack` at one mod whose main.luau is
/// `source`. Returns the mod's log lines and the stack after the hook (the roll's stack, which the
/// answer replaces when the handler assigns `event.stack`).
fn fire_function(
    source: &str,
    kind: &str,
    data: &serde_json::Value,
    ctx: &mod_api::LootContext,
    stack: mod_api::world::snapshots::LootStack,
) -> (Vec<String>, mod_api::world::snapshots::LootStack) {
    let root = tempfile::tempdir().unwrap();
    let host = LuaHost::new(vec![load_one(root.path(), "mymod", source)]);
    let tags = NoTags;
    let mut stack = stack;
    {
        let mut event = mod_api::LootFunctionEvent {
            kind,
            data,
            ctx,
            tags: &tags,
            stack: &mut stack,
        };
        let _phase = mod_api::__private::PhaseOwner::scope(mod_api::__private::ModPhase::World);
        mod_api::Hooks::loot_function(&host, &mut event);
    }
    (host.mods()[0].log_lines(), stack)
}

#[test]
fn a_function_a_handler_claims_sees_its_kind_data_context_and_count_and_writes_the_stack() {
    // The handler copies every field of the stack it reads, then changes the count: the copy is a
    // new table, so the frozen one the event holds is never written. A wrong kind, data, context
    // or count leaves the stack as it was.
    let _host = stick_host();
    let stack = loot_stack_with_everything();
    let mut expected = stack.clone();
    expected.count = 29;
    let (_, after) = fire_function(
        "horizon.on(\"loot_function\", function(event) if event.kind ~= \"mymod:double\" then return end if event.data.factor == 3 and event.ctx.seed == \"7\" and event.stack.count == 2 then local s = {} for k, v in pairs(event.stack) do s[k] = v end s.count = 29 event.stack = s end end)",
        "mymod:double",
        &serde_json::json!({"factor": 3}),
        &luck_context(),
        stack,
    );
    assert_eq!(after, expected);
}

/// A handler that replaces the stack's count with 29 only when `ctx.seed` is exactly `seed`.
fn seed_function_source(seed: &str) -> String {
    format!(
        "horizon.on(\"loot_function\", function(event) if event.kind ~= \"mymod:double\" then return end if type(event.ctx.seed) == \"string\" and event.ctx.seed == \"{seed}\" then local s = {{}} for k, v in pairs(event.stack) do s[k] = v end s.count = 29 event.stack = s end end)"
    )
}

#[test]
fn a_function_with_a_seed_past_2_to_the_53_runs_and_its_stack_arrives() {
    let _host = stick_host();
    let seed = -5_299_148_278_949_906_928_i64;
    let stack = loot_stack_with_everything();
    let mut expected = stack.clone();
    expected.count = 29;
    let (log, after) = fire_function(
        &seed_function_source(&seed.to_string()),
        "mymod:double",
        &serde_json::json!({}),
        &mod_api::LootContext::new(seed),
        stack,
    );
    assert_eq!(after, expected, "log: {log:?}");
    assert!(log.is_empty(), "log: {log:?}");
}

#[test]
fn a_function_with_the_largest_seed_runs_and_its_stack_arrives() {
    let _host = stick_host();
    let seed = i64::MAX;
    let stack = loot_stack_with_everything();
    let mut expected = stack.clone();
    expected.count = 29;
    let (log, after) = fire_function(
        &seed_function_source(&seed.to_string()),
        "mymod:double",
        &serde_json::json!({}),
        &mod_api::LootContext::new(seed),
        stack,
    );
    assert_eq!(after, expected, "log: {log:?}");
    assert!(log.is_empty(), "log: {log:?}");
}

#[test]
fn a_function_another_handler_claims_leaves_the_stack_unchanged() {
    let _host = stick_host();
    let stack = loot_stack_with_everything();
    let (_, after) = fire_function(
        "horizon.on(\"loot_function\", function(event) if event.kind ~= \"mymod:other\" then return end event.stack = {item = \"minecraft:stick\", count = 1} end)",
        "mymod:double",
        &serde_json::json!({}),
        &luck_context(),
        stack.clone(),
    );
    assert_eq!(after, stack);
}

#[test]
fn an_unknown_item_or_a_number_written_as_the_stack_is_refused_and_the_stack_is_unchanged() {
    let _host = stick_host();
    let stack = loot_stack_with_everything();
    let (_, after) = fire_function(
        "horizon.on(\"loot_function\", function(event) if event.kind == \"mymod:bad\" then event.stack = {item = \"minecraft:nope\", count = 1} end end)",
        "mymod:bad",
        &serde_json::json!({}),
        &luck_context(),
        stack.clone(),
    );
    assert_eq!(after, stack);
    let (_, after) = fire_function(
        "horizon.on(\"loot_function\", function(event) if event.kind == \"mymod:bad\" then event.stack = 5 end end)",
        "mymod:bad",
        &serde_json::json!({}),
        &luck_context(),
        stack.clone(),
    );
    assert_eq!(after, stack);
}

#[test]
fn an_in_place_edit_of_the_stack_without_assigning_it_is_an_error_and_the_stack_is_unchanged() {
    // `event.stack` is the event's frozen data, not the answer: the answer is what the handler
    // assigns to `event.stack`. So `event.stack.count = 29` writes the frozen table, which is
    // read-only, the handler errors, and nothing is answered.
    let _host = stick_host();
    let stack = loot_stack_with_everything();
    // The handler's error is reported by the host, not in the mod's own log lines.
    let (_log, after) = fire_function(
        "horizon.on(\"loot_function\", function(event) if event.kind == \"mymod:double\" then event.stack.count = 29 end end)",
        "mymod:double",
        &serde_json::json!({}),
        &luck_context(),
        stack.clone(),
    );
    assert_eq!(after, stack);
}
