//! Hand-written tests for the round-3 converters: the cases the generated
//! smoke tests cannot tell apart on their own. Each rule from the settled
//! shapes gets its telling inputs here (both `Option` paths, two-element
//! and empty sequences, both bool directions, every enum variant plus a
//! wrong name, the NBT edges, text read/write, out and mutable stacks,
//! byte-strings with `0x00` and bytes above 127), over the fakes in
//! `generated_hooks::test_support` (`FakeHost` names id 7 stone and every
//! other id after itself).

use super::test_support::{FakeHost, FakeLevel, boxed, loaded, no_error};
use crate::generated_runtime::{
    LuaStruct, placement_answer, props_of, split_state_name, struct_vec_from_lua,
    struct_vec_to_lua, text_from_parts,
};
use horizon_client::mod_api::{
    BlockStateId, Compound, Corner, Direction, Hand, ItemStack, NativeConfigKind,
    NativeFluidAction, Tag, dispatch_custom_block_fluids, dispatch_item_after_vertical_collision,
    dispatch_net_from_client, dispatch_prepare_anvil, dispatch_state_for_placement,
};
use horizon_world::consumable::UseAnimation;
use mlua::{Lua, Value};

fn lua() -> Lua {
    Lua::new()
}

fn s(lua: &Lua, s: &str) -> Value {
    lua.create_string(s).map(Value::String).unwrap()
}

/// Id 7 is stone; every id above 7 names itself, and both split into an id
/// string plus a property table.
#[test]
fn state_names_split_id_7_and_above() {
    let host = FakeHost::empty();
    assert_eq!(
        horizon_client::mod_api::Host::state_name(&host, BlockStateId(7)),
        "minecraft:stone"
    );
    assert_eq!(
        horizon_client::mod_api::Host::state_name(&host, BlockStateId(42)),
        "test:state42"
    );
    let lua = lua();
    let (id, props) = split_state_name(&lua, "minecraft:stone").unwrap();
    assert_eq!(id.as_str(), "minecraft:stone");
    assert_eq!(props.len().unwrap(), 0);
    let (id, props) = split_state_name(&lua, "test:mod[facing=north,open=true]").unwrap();
    assert_eq!(id.as_str(), "test:mod");
    assert_eq!(props.get::<String>("facing").unwrap(), "north");
    assert_eq!(props.get::<String>("open").unwrap(), "true");
    let plain = props_of(&lua, vec![("facing".into(), "north".into())]).unwrap();
    assert_eq!(plain.get::<String>("facing").unwrap(), "north");
}

/// Every `Option` shape reads `nil` as none and a value as some.
#[test]
fn options_read_nil_and_values() {
    let lua = lua();
    assert_eq!(
        Option::<ItemStack>::from_value(&lua, Value::Nil, "stack").unwrap(),
        None
    );
    let some = Option::<ItemStack>::from_value(
        &lua,
        Value::Table(item_table(&lua, "test:stack", 1, 0)),
        "stack",
    )
    .unwrap();
    assert_eq!(some.unwrap().count, 1);
    // Text reads a string or a component table; two Nils are not text.
    let text = text_from_parts(&lua, s(&lua, "hi"), Value::Nil, "motd").unwrap();
    assert_eq!(text.plain(), "hi");
    let component = lua
        .create_table_from([("text", "hi")])
        .map(Value::Table)
        .unwrap();
    // A component table wins: the vanilla source round-trips even though the
    // spans stay empty (plain() reads spans only).
    let text = text_from_parts(&lua, Value::Nil, component.clone(), "motd").unwrap();
    assert_eq!(text.plain(), "");
    assert_eq!(
        text.component().get("text").and_then(|v| v.as_str()),
        Some("hi")
    );
    text_from_parts(&lua, Value::Nil, Value::Nil, "motd").unwrap_err();
    // A sequence of two and the empty sequence.
    let two = struct_vec_to_lua(&lua, &[(7, 8), (1, 2)]).unwrap();
    assert_eq!(two.len().unwrap(), 2);
    let back: Vec<(i32, i32)> = struct_vec_from_lua(&lua, &two, "seq").unwrap();
    assert_eq!(back, vec![(7, 8), (1, 2)]);
    let empty = lua.create_table().unwrap();
    let back: Vec<(i32, i32)> = struct_vec_from_lua(&lua, &empty, "seq").unwrap();
    assert!(back.is_empty());
}

fn item_table(lua: &Lua, id: &str, count: i32, damage: i32) -> mlua::Table {
    let t = lua.create_table().unwrap();
    t.set("id", id).unwrap();
    t.set("count", count).unwrap();
    t.set("damage", damage).unwrap();
    t
}

/// Both bool directions survive a `&mut bool` round trip.
#[test]
fn bools_write_true_and_false() {
    for (start, set) in [(false, true), (true, false)] {
        let (m, id, _dir) = loaded(
            "gen_bool_dir",
            &format!(
                "horizon.on(\"custom_block_fluids\", function(event) event.custom = {} end)",
                if set { "true" } else { "false" }
            ),
        );
        let mods = boxed(m);
        let mut custom = start;
        dispatch_custom_block_fluids(&FakeHost::empty(), &mods, "test:block", &mut custom);
        assert_eq!(custom, set);
        no_error(&id);
    }
}

/// Every variant of every name enum converts, and a wrong name errors
/// listing the valid ones.
#[test]
fn enums_convert_every_variant() {
    let lua = lua();
    for (name, hand) in [("main", Hand::Main), ("off", Hand::Off)] {
        assert_eq!(Hand::from_value(&lua, s(&lua, name), "hand").unwrap(), hand);
        let v = Hand::to_value(&lua, &hand).unwrap();
        assert_eq!(v.as_string().unwrap().to_str().unwrap(), name);
    }
    for (name, dir) in [
        ("down", Direction::Down),
        ("up", Direction::Up),
        ("north", Direction::North),
        ("south", Direction::South),
        ("west", Direction::West),
        ("east", Direction::East),
    ] {
        assert_eq!(
            Direction::from_value(&lua, s(&lua, name), "facing").unwrap(),
            dir
        );
    }
    for (name, corner) in [
        ("top_left", Corner::TopLeft),
        ("top_right", Corner::TopRight),
        ("bottom_left", Corner::BottomLeft),
        ("bottom_right", Corner::BottomRight),
    ] {
        assert_eq!(
            Corner::from_value(&lua, s(&lua, name), "corner").unwrap(),
            corner
        );
    }
    assert_eq!(
        NativeFluidAction::from_value(&lua, s(&lua, "simulate"), "action").unwrap(),
        NativeFluidAction::Simulate
    );
    assert_eq!(
        NativeFluidAction::from_value(&lua, s(&lua, "execute"), "action").unwrap(),
        NativeFluidAction::Execute
    );
    for (name, kind) in [
        ("client", NativeConfigKind::Client),
        ("common", NativeConfigKind::Common),
        ("server", NativeConfigKind::Server),
    ] {
        assert_eq!(
            NativeConfigKind::from_value(&lua, s(&lua, name), "kind").unwrap(),
            kind
        );
    }
    let animations = [
        "none",
        "eat",
        "drink",
        "block",
        "bow",
        "trident",
        "crossbow",
        "spyglass",
        "toot_horn",
        "brush",
        "bundle",
        "spear",
    ];
    assert_eq!(animations.len(), 12);
    for name in animations {
        UseAnimation::from_value(&lua, s(&lua, name), "animation").unwrap();
    }
    let err = Hand::from_value(&lua, s(&lua, "middle"), "hand").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("middle"), "names the bad value: {text}");
    assert!(
        text.contains("main") && text.contains("off"),
        "lists valid: {text}"
    );
}

/// An NBT round trip keeps each number's type; a new whole number is an
/// int, a fraction a double, and a key set to nil is removed.
#[test]
fn nbt_compound_roundtrip_keeps_types() {
    let lua = lua();
    let mut seed = Compound::new();
    seed.insert("kept_int".into(), Tag::Int(3));
    seed.insert("kept_double".into(), Tag::Double(1.5f64.to_bits()));
    seed.insert("name".into(), Tag::String("pig".into()));
    let table = crate::lua_mods::nbt::compound_to_table(&lua, &seed).unwrap();
    // New whole numbers are ints, fractions doubles.
    table.set("whole", 4).unwrap();
    table.set("fraction", 4.5).unwrap();
    table.set("kept_int", Value::Nil).unwrap();
    let back = crate::lua_mods::nbt::table_to_compound(&table, Some(&seed)).unwrap();
    assert_eq!(
        back.get(&"kept_double".into()),
        Some(&Tag::Double(1.5f64.to_bits()))
    );
    assert_eq!(back.get(&"name".into()), Some(&Tag::String("pig".into())));
    assert_eq!(back.get(&"whole".into()), Some(&Tag::Int(4)));
    assert_eq!(
        back.get(&"fraction".into()),
        Some(&Tag::Double(4.5f64.to_bits()))
    );
    assert!(!back.contains_key(&"kept_int".into()));
    // Untouched numbers keep their type through the like-compound.
    let again = crate::lua_mods::nbt::table_to_compound(&table, Some(&back)).unwrap();
    assert_eq!(again.get(&"whole".into()), Some(&Tag::Int(4)));
    // A nest past the limit is an error, not a stack overflow.
    let mut deep = lua.create_table().unwrap();
    for _ in 0..200 {
        let outer = lua.create_table().unwrap();
        outer.set("inner", deep).unwrap();
        deep = outer;
    }
    let err = crate::lua_mods::nbt::table_to_compound(&deep, None).unwrap_err();
    assert!(err.to_string().contains("deeper"), "names the depth: {err}");
}

/// An out stack arrives as a table and stays nil without an answer; a
/// mutable stack edits in place.
#[test]
fn out_stack_and_mutable_stack() {
    let (m, id, _dir) = loaded(
        "gen_out_stack",
        "horizon.on(\"prepare_anvil\", function(event) event.result = { id = \"test:edited\", count = 2, damage = 1 } end)",
    );
    let mods = boxed(m);
    let mut cost = 1;
    let mut result = None;
    dispatch_prepare_anvil(
        &mut FakeHost::empty(),
        &mods,
        7,
        "test:player",
        [1, 2, 3],
        &mut cost,
        &mut result,
    );
    let stack = result.expect("an answer");
    assert_eq!(stack.item.as_ref(), "test:edited");
    assert_eq!((stack.count, stack.damage), (2, 1));
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_no_answer",
        "horizon.on(\"prepare_anvil\", function(event) end)",
    );
    let mods = boxed(m);
    let mut cost = 1;
    let mut result = None;
    dispatch_prepare_anvil(
        &mut FakeHost::empty(),
        &mods,
        7,
        "test:player",
        [1, 2, 3],
        &mut cost,
        &mut result,
    );
    assert_eq!(result, None);
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_mut_stack",
        "horizon.on(\"item_after_vertical_collision\", function(event) event.stack.count = 9 end)",
    );
    let mods = boxed(m);
    let mut stack = ItemStack {
        item: "test:fall".into(),
        count: 1,
        damage: 0,
        extra: None,
    };
    let mut discard = false;
    dispatch_item_after_vertical_collision(
        &mut FakeHost::empty(),
        &mods,
        "minecraft:overworld",
        true,
        7,
        BlockStateId(7),
        [1, 2, 3],
        [1.0, 2.0, 3.0],
        &mut stack,
        &mut discard,
    );
    assert_eq!(stack.count, 9);
    no_error(&id);
}

/// Byte strings cross with zero bytes and bytes above 127 intact.
#[test]
fn bytes_roundtrip_zero_and_high() {
    let lua = lua();
    let bytes = [116u8, 0, 159, 255];
    let s = lua.create_string(bytes).unwrap();
    assert_eq!(s.as_bytes(), bytes);
    let (m, id, _dir) = loaded(
        "gen_bytes",
        "horizon.on(\"net_from_client\", function(event) assert(#event.data == 3 and event.data:byte(2) == 0, \"bytes\") end)",
    );
    let mods = boxed(m);
    dispatch_net_from_client(
        &mut FakeHost::empty(),
        &mods,
        7,
        "test:channel",
        &[116u8, 0, 159],
    );
    no_error(&id);
}

/// A level that grants one property, to cover the success path the shared
/// fake (which grants none) cannot reach.
struct PropLevel;
impl horizon_client::mod_api::LevelHost for PropLevel {
    fn block_state(&self, _: [i32; 3]) -> BlockStateId {
        BlockStateId(7)
    }
    fn is_client_side(&self) -> bool {
        false
    }
    fn state_with_property(
        &self,
        state: BlockStateId,
        key: &str,
        value: &str,
    ) -> Option<BlockStateId> {
        (state == BlockStateId(7) && key == "facing" && value == "north").then_some(BlockStateId(8))
    }
    fn state_block(&self, state: BlockStateId) -> Option<String> {
        (state == BlockStateId(7) || state == BlockStateId(8)).then(|| "minecraft:stone".to_owned())
    }
    fn state_properties(&self, _: BlockStateId) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// A placement answer applies the own block's properties; anything else
/// errors.
#[test]
fn placement_answer_applies_own_props() {
    let level = FakeLevel;
    // No properties: the event's own state answers.
    let id = placement_answer(&level, BlockStateId(7), "minecraft:stone", "answer").unwrap();
    assert_eq!(id, BlockStateId(7));
    // A granted property moves to the new state.
    let id = placement_answer(
        &PropLevel,
        BlockStateId(7),
        "minecraft:stone[facing=north]",
        "answer",
    )
    .unwrap();
    assert_eq!(id, BlockStateId(8));
    // Another block's name is refused, as is a bad property.
    placement_answer(&level, BlockStateId(7), "minecraft:dirt", "answer").unwrap_err();
    placement_answer(&level, BlockStateId(7), "minecraft:stone[bad=1]", "answer").unwrap_err();
    placement_answer(&level, BlockStateId(7), "minecraft:stone[bad]", "answer").unwrap_err();
}

/// An unset placement answer leaves the decision to the next mod.
#[test]
fn placement_answer_nil_passes_through() {
    let (m, id, _dir) = loaded(
        "gen_no_place",
        "horizon.on(\"state_for_placement\", function(event) end)",
    );
    let mods = boxed(m);
    let mut answer = None;
    dispatch_state_for_placement(
        &FakeLevel,
        &mods,
        horizon_client::mod_api::LevelDimension::Overworld,
        BlockStateId(7),
        &horizon_client::mod_api::PlacementContext {
            pos: horizon_client::mod_api::BlockPos::new(3, 4, 5),
            hit_pos: horizon_client::mod_api::BlockPos::new(3, 4, 5),
            face: Direction::North,
            hit: Default::default(),
            inside: true,
            yaw: 92.0,
            pitch: 0.0,
            sneaking: false,
            item: "test:item2".into(),
            replace_clicked: true,
        },
        horizon_client::mod_api::FluidState::EMPTY,
        &mut answer,
    );
    assert_eq!(answer, None);
    no_error(&id);
}
