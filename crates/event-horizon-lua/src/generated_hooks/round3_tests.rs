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
use horizon_client::item_data::StackData;
use horizon_client::mod_api::{
    BlockStateId, Compound, Corner, Direction, Hand, ItemStack, ModContainer, ModEnergyBuffer,
    NativeConfigKind, NativeFluidAction, NativeScreen, Tag, Text, dispatch_broadcast_message,
    dispatch_console_line, dispatch_custom_block_fluids, dispatch_item_after_vertical_collision,
    dispatch_menu_input_closed, dispatch_mouse_scrolling, dispatch_net_from_client,
    dispatch_player_login, dispatch_prepare_anvil, dispatch_screen, dispatch_shader_params,
    dispatch_state_for_placement,
};
use horizon_world::consumable::UseAnimation;
use mlua::{Lua, Value};
use std::time::Duration;

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
    // Every name converts to its own variant (the converter's last arm is
    // a catch-all, so converting without checking the variant proves
    // nothing).
    let animations = [
        ("none", UseAnimation::None),
        ("eat", UseAnimation::Eat),
        ("drink", UseAnimation::Drink),
        ("block", UseAnimation::Block),
        ("bow", UseAnimation::Bow),
        ("trident", UseAnimation::Trident),
        ("crossbow", UseAnimation::Crossbow),
        ("spyglass", UseAnimation::Spyglass),
        ("toot_horn", UseAnimation::TootHorn),
        ("brush", UseAnimation::Brush),
        ("bundle", UseAnimation::Bundle),
        ("spear", UseAnimation::Spear),
    ];
    assert_eq!(animations.len(), 12);
    for (name, expected) in animations {
        assert_eq!(
            UseAnimation::from_value(&lua, s(&lua, name), "animation").unwrap(),
            expected,
            "wrong variant for {name:?}",
        );
        let back = UseAnimation::to_value(&lua, &expected).unwrap();
        assert_eq!(back.as_string().unwrap().to_str().unwrap(), name);
    }
    let err = Hand::from_value(&lua, s(&lua, "middle"), "hand").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("middle"), "names the bad value: {text}");
    assert!(
        text.contains("main") && text.contains("off"),
        "lists valid: {text}"
    );
}

/// An NBT round trip through the generated write path keeps each number's
/// type; a new whole number is an int, a fraction a double, and a key set
/// to nil is removed.
#[test]
fn nbt_compound_roundtrip_keeps_types() {
    let lua = lua();
    let mut seed = Compound::new();
    seed.insert("kept_int".into(), Tag::Int(3));
    seed.insert("kept_double".into(), Tag::Double(1.5f64.to_bits()));
    seed.insert("name".into(), Tag::String("pig".into()));
    // The write path the hooks use: `Compound::from_value` with no
    // like-compound.
    let Value::Table(table) = Compound::to_value(&lua, &seed).unwrap() else {
        panic!("a compound seeds as a table");
    };
    // New whole numbers are ints, fractions doubles.
    table.set("whole", 4).unwrap();
    table.set("fraction", 4.5).unwrap();
    table.set("kept_int", Value::Nil).unwrap();
    let back = Compound::from_value(&lua, Value::Table(table), "tag").unwrap();
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
    // Untouched numbers keep their type on the way back through.
    let Value::Table(table) = Compound::to_value(&lua, &back).unwrap() else {
        panic!("a compound seeds as a table");
    };
    let again = Compound::from_value(&lua, Value::Table(table), "tag").unwrap();
    assert_eq!(again.get(&"whole".into()), Some(&Tag::Int(4)));
    // A nest past the limit is an error, not a stack overflow.
    let mut deep = lua.create_table().unwrap();
    for _ in 0..200 {
        let outer = lua.create_table().unwrap();
        outer.set("inner", deep).unwrap();
        deep = outer;
    }
    let err = Compound::from_value(&lua, Value::Table(deep), "tag").unwrap_err();
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

/// Byte strings cross the hook with zero bytes and bytes above 127
/// intact, including the 159 the old test never checked.
#[test]
fn bytes_roundtrip_zero_and_high() {
    let lua = lua();
    let bytes = [116u8, 0, 159, 255];
    let s = lua.create_string(bytes).unwrap();
    assert_eq!(s.as_bytes(), bytes);
    let (m, id, _dir) = loaded(
        "gen_bytes",
        "horizon.on(\"net_from_client\", function(event) assert(#event.data == 3, \"len\") assert(event.data:byte(1) == 116, \"first\") assert(event.data:byte(2) == 0, \"zero\") assert(event.data:byte(3) == 159, \"high\") end)",
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
    // State 8 carries one property, so a seed test can read a non-empty
    // property table (the shared fake grants none).
    fn state_properties(&self, state: BlockStateId) -> Vec<(String, String)> {
        if state == BlockStateId(8) {
            vec![("facing".into(), "north".into())]
        } else {
            Vec::new()
        }
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

/// A non-empty property table seeds through `gen_seed`: the state name
/// and its `state_props` reach the handler together.
#[test]
fn state_props_seed_nonempty_through_gen_seed() {
    let (m, id, _dir) = loaded(
        "gen_state_props",
        "horizon.on(\"state_for_placement\", function(event) assert(event.state == \"minecraft:stone\", \"state\") assert(event.state_props.facing == \"north\", \"props\") end)",
    );
    let mods = boxed(m);
    let mut answer = None;
    dispatch_state_for_placement(
        &PropLevel,
        &mods,
        horizon_client::mod_api::LevelDimension::Overworld,
        BlockStateId(8),
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

/// A text write through a real hook: a string-only change wins while the
/// seeded component sits untouched, a component-only change wins, and an
/// untouched event keeps the native text.
#[test]
fn text_string_and_component_writes() {
    let seed = || Text::empty().with_component(serde_json::json!({ "text": "hello" }));
    let (m, id, _dir) = loaded(
        "gen_text_string",
        "horizon.on(\"broadcast_message\", function(event) event.line = \"bye\" horizon.cancel() end)",
    );
    let mods = boxed(m);
    let mut line = seed();
    let mut cancel = false;
    dispatch_broadcast_message(&FakeHost::empty(), &mods, &mut line, &mut cancel);
    assert!(cancel, "cancel not cancelled");
    assert_eq!(line, Text::literal("bye"));
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_text_component",
        "horizon.on(\"broadcast_message\", function(event) event.line_component = { text = \"comp\" } horizon.cancel() end)",
    );
    let mods = boxed(m);
    let mut line = seed();
    let mut cancel = false;
    dispatch_broadcast_message(&FakeHost::empty(), &mods, &mut line, &mut cancel);
    assert!(cancel, "cancel not cancelled");
    // `Text` equality compares spans only (both empty here), so the
    // component JSON itself is the assertion.
    assert_eq!(line.component(), serde_json::json!({ "text": "comp" }));
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_text_untouched",
        "horizon.on(\"broadcast_message\", function(event) horizon.cancel() end)",
    );
    let mods = boxed(m);
    let mut line = seed();
    let mut cancel = false;
    dispatch_broadcast_message(&FakeHost::empty(), &mods, &mut line, &mut cancel);
    assert!(cancel, "cancel not cancelled");
    assert_eq!(line, seed());
    no_error(&id);
}

/// A stack write that only changes `count` keeps the native components; the
/// script clears them with `extra = nil`, and a fresh table without `extra`
/// is a plain stack.
#[test]
fn item_stack_count_only_keeps_extra() {
    let extra = || StackData {
        enchantments: vec![(std::sync::Arc::from("test:ench"), 1)],
        ..Default::default()
    };
    let stack = || ItemStack {
        item: "test:fall".into(),
        count: 1,
        damage: 0,
        extra: Some(Box::new(extra())),
    };
    let (m, id, _dir) = loaded(
        "gen_extra_kept",
        "horizon.on(\"item_after_vertical_collision\", function(event) event.stack.count = 9 end)",
    );
    let mods = boxed(m);
    let mut kept = stack();
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
        &mut kept,
        &mut discard,
    );
    assert_eq!(kept.count, 9);
    assert_eq!(kept.extra.as_deref(), Some(&extra()));
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_extra_cleared",
        "horizon.on(\"item_after_vertical_collision\", function(event) event.stack.extra = nil end)",
    );
    let mods = boxed(m);
    let mut cleared = stack();
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
        &mut cleared,
        &mut discard,
    );
    assert_eq!(cleared.count, 1);
    assert_eq!(cleared.extra, None);
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_extra_fresh",
        "horizon.on(\"item_after_vertical_collision\", function(event) event.stack = { id = \"test:new\", count = 2, damage = 0 } end)",
    );
    let mods = boxed(m);
    let mut fresh = stack();
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
        &mut fresh,
        &mut discard,
    );
    assert_eq!(fresh.item.as_ref(), "test:new");
    assert_eq!((fresh.count, fresh.damage), (2, 0));
    assert_eq!(fresh.extra, None);
    no_error(&id);
}

/// A replaced screen reads the `false` sentinel as the `None` slot in the
/// middle: three elements, not one. (Seeded holes read as nil instead; see
/// `menu_input_closed_none_slot_reads_nil`. `false` stays accepted in both.)
#[test]
fn screen_none_slot_reads_false_sentinel() {
    let (m, id, _dir) = loaded(
        "gen_screen_none",
        "horizon.on(\"screen\", function(event) event.screen = { width = 100.0, height = 50.0, slots = { { x = 1.0, y = 2.0 }, false, { x = 3.0, y = 4.0 } }, inventory_y = 10.0, inventory_x = 20.0, own_labels = true, own_background = false } end)",
    );
    let firing_mods = boxed(m);
    let mut screen: Option<NativeScreen> = None;
    dispatch_screen(&FakeHost::empty(), &firing_mods, "test:block", &mut screen);
    assert_eq!(
        screen,
        Some(NativeScreen {
            width: 100.0,
            height: 50.0,
            slots: vec![Some([1.0, 2.0]), None, Some([3.0, 4.0])],
            inventory_y: 10.0,
            inventory_x: 20.0,
            own_labels: true,
            own_background: false,
        })
    );
    no_error(&id);
}

/// A short shader write errors naming the count instead of padding with
/// zeros, and the native values stay.
#[test]
fn shader_params_short_write_errors() {
    let (m, id, _dir) = loaded(
        "gen_shader_short",
        "horizon.on(\"shader_params\", function(event) event.params = { { x = 1.0, y = 2.0, z = 3.0, w = 4.0 } } end)",
    );
    let firing_mods = boxed(m);
    let mut params = [[2.5, 2.5, 3.5, 4.5]; 8];
    dispatch_shader_params(&FakeHost::empty(), &firing_mods, 1.5, &mut params);
    assert_eq!(params, [[2.5, 2.5, 3.5, 4.5]; 8]);
    let err = horizon_client::dev_input::mods::status()
        .into_iter()
        .find(|s| s.id == id)
        .and_then(|s| s.error)
        .expect("a short write errors");
    assert!(err.contains("eight"), "names the count: {err}");
}

/// A scroll delta that `f32` cannot hold crosses the hook exactly.
#[test]
fn scroll_keeps_full_precision() {
    let (m, id, _dir) = loaded(
        "gen_scroll_precise",
        "horizon.on(\"mouse_scrolling\", function(event) assert(event.scroll.x == 1.1, \"scroll x\") assert(event.scroll.y == 2.5, \"scroll y\") horizon.cancel() end)",
    );
    let firing_mods = boxed(m);
    let mut canceled = false;
    dispatch_mouse_scrolling(
        &mut FakeHost::empty(),
        &firing_mods,
        [1.1, 2.5],
        [4, -6],
        [true, false, false],
        [4.5, 5.5],
        11,
        &mut canceled,
    );
    assert!(canceled, "canceled not cancelled");
    no_error(&id);
}

/// `Duration` reads finite seconds, clamps negatives, and errors on
/// non-finite or huge numbers instead of panicking.
#[test]
fn duration_rejects_nonfinite_and_huge() {
    let lua = lua();
    assert_eq!(
        Duration::from_value(&lua, Value::Number(1.5), "elapsed").unwrap(),
        Duration::new(1, 500_000_000)
    );
    assert_eq!(
        Duration::from_value(&lua, Value::Integer(3), "elapsed").unwrap(),
        Duration::new(3, 0)
    );
    assert_eq!(
        Duration::from_value(&lua, Value::Number(-2.0), "elapsed").unwrap(),
        Duration::new(0, 0)
    );
    for bad in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 1e20] {
        Duration::from_value(&lua, Value::Number(bad), "elapsed").unwrap_err();
    }
}

/// Fixed-length sequences error on the wrong length instead of padding or
/// truncating.
#[test]
fn fixed_sequences_reject_wrong_length() {
    let lua = lua();
    let seq = |values: &[bool]| {
        let t = lua.create_table().unwrap();
        for (i, v) in values.iter().enumerate() {
            t.set(i + 1, *v).unwrap();
        }
        t
    };
    let err =
        <[bool; 3] as LuaStruct>::from_value(&lua, Value::Table(seq(&[true, false])), "buttons")
            .unwrap_err();
    assert!(err.to_string().contains("three"), "names the count: {err}");
    let err = <[bool; 3] as LuaStruct>::from_value(
        &lua,
        Value::Table(seq(&[true, false, true, false])),
        "buttons",
    )
    .unwrap_err();
    assert!(err.to_string().contains("three"), "names the count: {err}");

    // Five face lists, not six.
    let faces = lua.create_table().unwrap();
    for i in 1..=5 {
        let face = lua.create_table().unwrap();
        face.set(1, 0).unwrap();
        faces.set(i, face).unwrap();
    }
    let container = lua.create_table().unwrap();
    container.set("slots", 9).unwrap();
    container.set("faces", faces).unwrap();
    let inputs = lua.create_table().unwrap();
    inputs.set(1, 0).unwrap();
    container.set("inputs", inputs).unwrap();
    container
        .set("outputs", lua.create_table().unwrap())
        .unwrap();
    let err = ModContainer::from_value(&lua, Value::Table(container), "container").unwrap_err();
    assert!(err.to_string().contains("six"), "names the count: {err}");

    // Five face flags, not six.
    let buffer = lua.create_table().unwrap();
    buffer.set("capacity", 1000).unwrap();
    buffer.set("max_insert", 100).unwrap();
    buffer.set("max_extract", 100).unwrap();
    buffer
        .set("insert_faces", seq(&[true, false, false, false, false]))
        .unwrap();
    buffer
        .set(
            "extract_faces",
            seq(&[false, false, false, false, false, false]),
        )
        .unwrap();
    let err = ModEnergyBuffer::from_value(&lua, Value::Table(buffer), "buffer").unwrap_err();
    assert!(err.to_string().contains("six"), "names the count: {err}");

    // Seven shader slots, not eight.
    let params = lua.create_table().unwrap();
    for i in 1..=7 {
        let slot = lua.create_table().unwrap();
        slot.set("x", 1.0).unwrap();
        slot.set("y", 2.0).unwrap();
        slot.set("z", 3.0).unwrap();
        slot.set("w", 4.0).unwrap();
        params.set(i, slot).unwrap();
    }
    let err =
        <[[f32; 4]; 8] as LuaStruct>::from_value(&lua, Value::Table(params), "params").unwrap_err();
    assert!(err.to_string().contains("eight"), "names the count: {err}");
}

/// Setting a non-optional field to nil keeps the native value.
#[test]
fn nil_on_required_fields_keeps_native() {
    let (m, id, _dir) = loaded(
        "gen_nil_stack",
        "horizon.on(\"item_after_vertical_collision\", function(event) event.stack = nil end)",
    );
    let mods = boxed(m);
    let mut stack = ItemStack {
        item: "test:fall".into(),
        count: 4,
        damage: 2,
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
    assert_eq!(stack.count, 4);
    assert_eq!(stack.damage, 2);
    assert_eq!(stack.item.as_ref(), "test:fall");
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_nil_bool",
        "horizon.on(\"custom_block_fluids\", function(event) event.custom = nil end)",
    );
    let mods = boxed(m);
    let mut custom = true;
    dispatch_custom_block_fluids(&FakeHost::empty(), &mods, "test:block", &mut custom);
    assert!(custom, "nil keeps the native value");
    no_error(&id);
}

/// Finding 1: editing the seeded component table in place applies. The
/// snapshot the write-back compares against is a frozen value, not an alias
/// of the live table, so `line_component.color = "red"` is a change.
#[test]
fn text_in_place_component_edit_applies() {
    let seed = || Text::empty().with_component(serde_json::json!({ "text": "hello" }));
    let (m, id, _dir) = loaded(
        "gen_text_in_place",
        "horizon.on(\"broadcast_message\", function(event) event.line_component.color = \"red\" horizon.cancel() end)",
    );
    let mods = boxed(m);
    let mut line = seed();
    let mut cancel = false;
    dispatch_broadcast_message(&FakeHost::empty(), &mods, &mut line, &mut cancel);
    assert!(cancel, "cancel not cancelled");
    // `Text` equality compares spans only (both empty here), so the
    // component JSON itself is the assertion.
    assert_eq!(
        line.component(),
        serde_json::json!({ "text": "hello", "color": "red" })
    );
    no_error(&id);
}

/// Finding 2: setting only the plain side of an optional text to nil clears
/// it. The seeded component table still sitting there is not a write.
#[test]
fn opt_text_plain_nil_clears() {
    let (m, id, _dir) = loaded(
        "gen_reason_nil",
        "horizon.on(\"player_login\", function(event) event.reason = nil horizon.cancel() end)",
    );
    let firing_mods = boxed(m);
    let mut reason = Some(Text::literal("test:reason"));
    let mut cancel = false;
    dispatch_player_login(
        &mut FakeHost::empty(),
        &firing_mods,
        7,
        "test:name",
        &mut reason,
        &mut cancel,
    );
    assert!(cancel, "cancel not cancelled");
    assert_eq!(reason, None);
    no_error(&id);
}

/// Finding 3: a script-written `__len` is not trusted. It is not read at
/// all, so even 2^62 (an integer-tagged literal, like the finding's `1e15`)
/// cannot reserve absurd capacity.
#[test]
fn seq_script_len_key_ignored() {
    let (m, id, _dir) = loaded(
        "gen_menu_huge_len",
        "horizon.on(\"menu_input_closed\", function(event) event.slots.__len = 4611686018427387904 end)",
    );
    let mods = boxed(m);
    let mut slots = vec![
        Some(ItemStack {
            item: "test:a".into(),
            count: 1,
            damage: 0,
            extra: None,
        }),
        None,
    ];
    dispatch_menu_input_closed(&mut FakeHost::empty(), &mods, 7, 8, &mut slots);
    assert_eq!(slots.len(), 2);
    assert_eq!(slots[0].as_ref().unwrap().count, 1);
    assert_eq!(slots[1], None);
    no_error(&id);
}

/// Finding 3: deleting the last slot in place shrinks the list; `false`
/// still pins an explicit trailing hole.
#[test]
fn seq_trailing_nil_shrinks() {
    let stack = |item: &str| {
        Some(ItemStack {
            item: item.into(),
            count: 1,
            damage: 0,
            extra: None,
        })
    };
    let (m, id, _dir) = loaded(
        "gen_menu_shrink",
        "horizon.on(\"menu_input_closed\", function(event) event.slots[3] = nil end)",
    );
    let mods = boxed(m);
    let mut slots = vec![stack("test:a"), stack("test:b"), stack("test:c")];
    dispatch_menu_input_closed(&mut FakeHost::empty(), &mods, 7, 8, &mut slots);
    assert_eq!(slots.len(), 2);
    no_error(&id);

    let (m, id, _dir) = loaded(
        "gen_menu_false_hole",
        "horizon.on(\"menu_input_closed\", function(event) event.slots[3] = false end)",
    );
    let mods = boxed(m);
    let mut slots = vec![stack("test:a"), stack("test:b"), stack("test:c")];
    dispatch_menu_input_closed(&mut FakeHost::empty(), &mods, 7, 8, &mut slots);
    assert_eq!(slots.len(), 3);
    assert_eq!(slots[2], None);
    no_error(&id);
}

/// Finding 4: the write-back keeps no snapshot keys on the event table, so
/// `pairs` never sees them, and a forged one cannot confuse the write-back.
#[test]
fn text_snapshot_keys_hidden() {
    let seed = || Text::empty().with_component(serde_json::json!({ "text": "hello" }));
    let (m, id, _dir) = loaded(
        "gen_text_hidden_snap",
        "horizon.on(\"broadcast_message\", function(event) for k, _ in pairs(event) do assert(k ~= \"\\0line#text\" and k ~= \"\\0line#component\", \"snapshot key visible\") end event[\"\\0line#text\"] = \"forged\" event[\"\\0line#component\"] = { text = \"evil\" } event.line = \"bye\" horizon.cancel() end)",
    );
    let mods = boxed(m);
    let mut line = seed();
    let mut cancel = false;
    dispatch_broadcast_message(&FakeHost::empty(), &mods, &mut line, &mut cancel);
    assert!(cancel, "cancel not cancelled");
    assert_eq!(line, Text::literal("bye"));
    no_error(&id);
}

/// Finding 5: a seeded `None` slot reads as nil, and a nil hole or trailing
/// nil in a seeded list reads back as `None` in place.
#[test]
fn menu_input_closed_none_slot_reads_nil() {
    let (m, id, _dir) = loaded(
        "gen_menu_nil",
        "horizon.on(\"menu_input_closed\", function(event) assert(event.slots[2] == nil, \"hole seeds as nil\") event.slots[1] = nil event.slots[2] = { id = \"test:new\", count = 1, damage = 0 } end)",
    );
    let mods = boxed(m);
    let mut slots = vec![
        Some(ItemStack {
            item: "test:a".into(),
            count: 1,
            damage: 0,
            extra: None,
        }),
        None,
        Some(ItemStack {
            item: "test:b".into(),
            count: 2,
            damage: 0,
            extra: None,
        }),
    ];
    dispatch_menu_input_closed(&mut FakeHost::empty(), &mods, 7, 8, &mut slots);
    assert_eq!(
        format!("{:?}", slots),
        format!(
            "{:?}",
            vec![
                None,
                Some(ItemStack {
                    item: "test:new".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                Some(ItemStack {
                    item: "test:b".into(),
                    count: 2,
                    damage: 0,
                    extra: None,
                }),
            ]
        )
    );
    no_error(&id);
}

/// Finding 6: unmodelled NBT keeps its types across an in-place round trip:
/// a byte array stays a byte array, an empty list stays an empty list.
#[test]
fn extra_unmodelled_arrays_survive() {
    let extra = || StackData {
        other: vec![
            ("mymod:blob".to_owned(), Tag::ByteArray(vec![1, 2, 3])),
            ("mymod:empty".to_owned(), Tag::List(vec![])),
        ],
        ..Default::default()
    };
    let (m, id, _dir) = loaded(
        "gen_extra_arrays",
        "horizon.on(\"item_after_vertical_collision\", function(event) event.stack.count = 9 end)",
    );
    let mods = boxed(m);
    let mut stack = ItemStack {
        item: "test:fall".into(),
        count: 1,
        damage: 0,
        extra: Some(Box::new(extra())),
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
    assert_eq!(stack.extra.as_deref(), Some(&extra()));
    no_error(&id);
}

/// Finding 1 scope check: nested text (tooltip lines, `console_line`
/// replies) never went through the snapshot alias: the write-back reads the
/// live table, so an in-place component edit already applies.
#[test]
fn console_line_reply_in_place_edit_applies() {
    let (m, id, _dir) = loaded(
        "gen_console_in_place",
        "horizon.on(\"console_line\", function(event) event.replies[1].component.color = \"red\" horizon.cancel() end)",
    );
    let firing_mods = boxed(m);
    let mut replies = vec![Text::literal("test:line2"), Text::literal("test:line3")];
    let mut cancel = false;
    dispatch_console_line(
        &mut FakeHost::empty(),
        &firing_mods,
        "test:source",
        "test:line",
        &mut replies,
        &mut cancel,
    );
    assert!(cancel, "cancel not cancelled");
    assert_eq!(
        replies[0].component().get("color").and_then(|v| v.as_str()),
        Some("red")
    );
    no_error(&id);
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
