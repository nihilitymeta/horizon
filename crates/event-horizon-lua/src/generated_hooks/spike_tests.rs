//! The spike round 1 tests for its 17 hooks, kept verbatim: hand-picked
//! samples (`ruby:drum`, `Alex`, `81000`) the model-derived tests do not
//! replace. The model-derived tests in `mod.rs` skip these hooks.
use super::test_support::{FakeHost, FakeLevel, boxed, drop_dir, loaded, no_error};
use horizon_client::mod_api::{
    BlockPos, BlockStateId, ItemStack, LevelDimension, dispatch_anvil_damaged,
    dispatch_arrow_loose, dispatch_bell_ring, dispatch_block_burn, dispatch_block_cook,
    dispatch_block_lock_check, dispatch_chunk_load, dispatch_component_types,
    dispatch_datapack_sync, dispatch_is_bonemeal_success, dispatch_item_fluid_tank,
    dispatch_level_tick_pre, dispatch_player_container_close, dispatch_prepare_anvil,
    dispatch_screen_click, dispatch_screen_tooltip, dispatch_tooltip_component,
};
#[test]
fn generated_level_tick_pre() {
    const LUA: &str = r#"horizon.on("level_tick_pre", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
end)"#;
    let (m, id, dir) = loaded("gen_level_tick_pre", LUA);
    let mods = boxed(m);
    dispatch_level_tick_pre(&mut FakeHost::empty(), &mods, "minecraft:overworld");
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_chunk_load() {
    const LUA: &str = r#"horizon.on("chunk_load", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.chunk.x == 3 and event.chunk.z == -5, "chunk")
  assert(event.new == true, "new")
end)"#;
    let (m, id, dir) = loaded("gen_chunk_load", LUA);
    let mods = boxed(m);
    dispatch_chunk_load(
        &mut FakeHost::empty(),
        &mods,
        "minecraft:overworld",
        [3, -5],
        true,
    );
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_block_cook() {
    const LUA: &str = r#"horizon.on("block_cook", function(event)
  assert(event.pos.x == 1 and event.pos.y == 2 and event.pos.z == 3, "pos")
  assert(event.item == "minecraft:wheat", "item")
  assert(event.result == "minecraft:bread", "result")
end)"#;
    let (m, id, dir) = loaded("gen_block_cook", LUA);
    let mods = boxed(m);
    dispatch_block_cook(
        &FakeHost::empty(),
        &mods,
        [1, 2, 3],
        "minecraft:wheat",
        "minecraft:bread",
    );
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_anvil_damaged() {
    const LUA: &str = r#"horizon.on("anvil_damaged", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.pos.x == 4 and event.pos.y == 64 and event.pos.z == -2, "pos")
  assert(event.from == "minecraft:damaged_anvil", "from")
  assert(event.to == "minecraft:chipped_anvil", "to")
end)"#;
    let (m, id, dir) = loaded("gen_anvil_damaged", LUA);
    let mods = boxed(m);
    dispatch_anvil_damaged(
        &mut FakeHost::empty(),
        &mods,
        "minecraft:overworld",
        [4, 64, -2],
        "minecraft:damaged_anvil",
        "minecraft:chipped_anvil",
    );
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_player_container_close() {
    const LUA: &str = r#"horizon.on("player_container_close", function(event)
  assert(event.player.uuid == "00000000-0000-0000-0000-000000000007", "uuid")
  assert(event.player.name == "Alex", "name")
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.menu == "minecraft:chest", "menu")
  assert(event.pos == nil, "pos")
end)"#;
    let (m, id, dir) = loaded("gen_player_container_close", LUA);
    let mods = boxed(m);
    dispatch_player_container_close(
        &mut FakeHost::empty(),
        &mods,
        7,
        "Alex",
        "minecraft:overworld",
        "minecraft:chest",
        None,
    );
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_block_burn() {
    const LUA: &str = r#"horizon.on("block_burn", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.pos.x == 1 and event.pos.y == 2 and event.pos.z == 3, "pos")
  assert(event.block == "minecraft:stone", "block")
  horizon.cancel()
end)"#;
    let (m, id, dir) = loaded("gen_block_burn", LUA);
    let mods = boxed(m);
    let mut cancel = false;
    dispatch_block_burn(
        &FakeLevel,
        &mods,
        LevelDimension::Overworld,
        BlockPos::new(1, 2, 3),
        &mut cancel,
    );
    assert!(cancel, "a cancel takes the burn");
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_bell_ring() {
    const LUA: &str = r#"horizon.on("bell_ring", function(event)
  assert(event.player == "00000000-0000-0000-0000-000000000007", "player")
  assert(event.block == "minecraft:stone", "block")
  assert(event.pos.x == 0 and event.pos.y == 64 and event.pos.z == 0, "pos")
  horizon.cancel()
end)"#;
    let (m, id, dir) = loaded("gen_bell_ring", LUA);
    let mods = boxed(m);
    let mut cancel = false;
    dispatch_bell_ring(
        &FakeLevel,
        &mods,
        LevelDimension::Overworld,
        Some(7),
        BlockPos::new(0, 64, 0),
        &mut cancel,
    );
    assert!(cancel, "a cancel stills the bell");
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_arrow_loose() {
    const LUA: &str = r#"horizon.on("arrow_loose", function(event)
  assert(event.stack.id == "minecraft:bow", "item")
  assert(event.has_ammo == true, "ammo")
  assert(event.charge == 0, "charge")
  event.charge = 3
  horizon.cancel()
end)"#;
    let (m, id, dir) = loaded("gen_arrow_loose", LUA);
    let mods = boxed(m);
    let stack = ItemStack {
        item: "minecraft:bow".into(),
        count: 1,
        damage: 0,
        extra: None,
    };
    let mut charge = 0;
    let mut canceled = false;
    dispatch_arrow_loose(
        &mut FakeHost::empty(),
        &mods,
        &stack,
        true,
        &mut charge,
        &mut canceled,
    );
    assert!(canceled, "a cancel holds the arrow");
    assert_eq!(charge, 3, "the edited charge is written back");
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_block_lock_check() {
    const LUA: &str = r#"horizon.on("block_lock_check", function(event)
  assert(event.player.name == "Alex", "player")
  assert(event.pos.x == 5 and event.pos.y == 6 and event.pos.z == 7, "pos")
  horizon.cancel()
end)"#;
    let (m, id, dir) = loaded("gen_block_lock_check", LUA);
    let mods = boxed(m);
    let mut locked = false;
    dispatch_block_lock_check(
        &mut FakeHost::empty(),
        &mods,
        7,
        "Alex",
        [5, 6, 7],
        &mut locked,
    );
    assert!(locked, "a cancel keeps the lock");
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_component_types() {
    const LUA: &str = r#"horizon.on("component_types", function(event)
  assert(event.types[1] == "minecraft:existing", "seed")
  table.insert(event.types, "ruby:part")
end)"#;
    let (m, id, dir) = loaded("gen_component_types", LUA);
    let mods = boxed(m);
    let mut types = vec!["minecraft:existing".to_owned()];
    dispatch_component_types(&mods, &mut types);
    assert_eq!(types, ["minecraft:existing", "ruby:part"]);
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_datapack_sync() {
    const LUA: &str = r#"horizon.on("datapack_sync", function(event)
  assert(event.recipe_types[1] == "minecraft:crafting", "seed")
  event.recipe_types[1] = "ruby:pressing"
end)"#;
    let (m, id, dir) = loaded("gen_datapack_sync", LUA);
    let mods = boxed(m);
    let mut types = vec!["minecraft:crafting".to_owned()];
    dispatch_datapack_sync(&mut FakeHost::empty(), &mods, &mut types);
    assert_eq!(types, ["ruby:pressing"]);
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_screen_tooltip() {
    const LUA: &str = r#"horizon.on("screen_tooltip", function(event)
  assert(event.block == "ruby:gauge", "block")
  assert(event.data[1] == 4 and event.data[2] == 9, "data")
  assert(event.at.x == 1.5 and event.at.y == 2.5, "at")
  assert(event.lines[1] == "old", "lines")
  event.lines[1] = "new"
  table.insert(event.lines, "added")
end)"#;
    let (m, id, dir) = loaded("gen_screen_tooltip", LUA);
    let mods = boxed(m);
    let mut lines = vec!["old".to_owned()];
    dispatch_screen_tooltip(
        &FakeHost::empty(),
        &mods,
        "ruby:gauge",
        &[4, 9],
        [1.5, 2.5],
        &mut lines,
    );
    assert_eq!(lines, ["new", "added"]);
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_is_bonemeal_success() {
    const LUA: &str = r#"horizon.on("is_bonemeal_success", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.block == "minecraft:stone", "block")
  assert(event.success == false, "seed")
  event.success = true
end)"#;
    let (m, id, dir) = loaded("gen_is_bonemeal_success", LUA);
    let mods = boxed(m);
    let mut success = false;
    dispatch_is_bonemeal_success(
        &mut FakeLevel,
        &mods,
        LevelDimension::Overworld,
        BlockPos::new(1, 2, 3),
        BlockStateId(7),
        &mut success,
    );
    assert!(success, "the grown flag is written back");
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_item_fluid_tank() {
    const LUA: &str = r#"horizon.on("item_fluid_tank", function(event)
  assert(event.item == "ruby:drum", "item")
  assert(event.capacity == nil, "seed")
  event.capacity = 81000
end)"#;
    let (m, id, dir) = loaded("gen_item_fluid_tank", LUA);
    let mods = boxed(m);
    let mut capacity = None;
    dispatch_item_fluid_tank(&FakeHost::empty(), &mods, "ruby:drum", &mut capacity);
    assert_eq!(capacity, Some(81000));
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_tooltip_component() {
    const LUA: &str = r#"horizon.on("tooltip_component", function(event)
  assert(event.stack.id == "ruby:gauge", "item")
  assert(event.size == nil, "seed")
  event.size = { x = 16, y = 8 }
end)"#;
    let (m, id, dir) = loaded("gen_tooltip_component", LUA);
    let mods = boxed(m);
    let stack = ItemStack {
        item: "ruby:gauge".into(),
        count: 1,
        damage: 0,
        extra: None,
    };
    let mut size = None;
    dispatch_tooltip_component(&FakeHost::empty(), &mods, &stack, &mut size);
    assert_eq!(size, Some([16.0, 8.0]));
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_screen_click() {
    const LUA: &str = r#"horizon.on("screen_click", function(event)
  assert(event.block == "ruby:gauge", "block")
  assert(event.data[1] == 4, "data")
  assert(event.right == true, "button kind")
  assert(event.button == nil, "seed")
  event.button = 2
end)"#;
    let (m, id, dir) = loaded("gen_screen_click", LUA);
    let mods = boxed(m);
    let mut button = None;
    dispatch_screen_click(
        &FakeHost::empty(),
        &mods,
        "ruby:gauge",
        &[4],
        [1.5, 2.5],
        true,
        &mut button,
    );
    assert_eq!(button, Some(2));
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_prepare_anvil() {
    const LUA: &str = r#"horizon.on("prepare_anvil", function(event)
  assert(event.player.name == "Alex", "player")
  assert(event.cost == 5, "cost")
  assert(event.result == nil, "seed")
  event.result = { id = "minecraft:diamond_sword", count = 1 }
  event.cost = 9
end)"#;
    let (m, id, dir) = loaded("gen_prepare_anvil", LUA);
    let mods = boxed(m);
    let mut result = None;
    let mut cost = 5;
    dispatch_prepare_anvil(
        &mut FakeHost::empty(),
        &mods,
        7,
        "Alex",
        [5, 6, 7],
        &mut cost,
        &mut result,
    );
    assert_eq!(cost, 9, "the edited cost is written back");
    let result = result.expect("a replaced result");
    assert_eq!(&*result.item, "minecraft:diamond_sword");
    assert_eq!(result.count, 1);
    no_error(&id);
    drop_dir(dir);
}
#[test]
fn generated_no_handler_does_no_lua_work() {
    const LUA: &str = "-- nothing registered";
    let (m, id, dir) = loaded("gen_no_handler", LUA);
    let mods = boxed(m);
    dispatch_level_tick_pre(&mut FakeHost::empty(), &mods, "minecraft:overworld");
    dispatch_chunk_load(
        &mut FakeHost::empty(),
        &mods,
        "minecraft:overworld",
        [0, 0],
        false,
    );
    dispatch_block_cook(
        &FakeHost::empty(),
        &mods,
        [0, 0, 0],
        "minecraft:wheat",
        "minecraft:bread",
    );
    dispatch_anvil_damaged(
        &mut FakeHost::empty(),
        &mods,
        "minecraft:overworld",
        [0, 0, 0],
        "minecraft:anvil",
        "minecraft:chipped_anvil",
    );
    dispatch_player_container_close(
        &mut FakeHost::empty(),
        &mods,
        7,
        "Alex",
        "minecraft:overworld",
        "minecraft:chest",
        None,
    );
    let mut cancel = false;
    dispatch_block_burn(
        &FakeLevel,
        &mods,
        LevelDimension::Overworld,
        BlockPos::new(0, 0, 0),
        &mut cancel,
    );
    assert!(!cancel);
    dispatch_bell_ring(
        &FakeLevel,
        &mods,
        LevelDimension::Overworld,
        None,
        BlockPos::new(0, 0, 0),
        &mut cancel,
    );
    assert!(!cancel);
    let stack = ItemStack {
        item: "minecraft:bow".into(),
        count: 1,
        damage: 0,
        extra: None,
    };
    let mut charge = 0;
    let mut canceled = false;
    dispatch_arrow_loose(
        &mut FakeHost::empty(),
        &mods,
        &stack,
        true,
        &mut charge,
        &mut canceled,
    );
    assert!(!canceled && charge == 0);
    let mut locked = false;
    dispatch_block_lock_check(
        &mut FakeHost::empty(),
        &mods,
        7,
        "Alex",
        [0, 0, 0],
        &mut locked,
    );
    assert!(!locked);
    let mut types = vec!["minecraft:existing".to_owned()];
    dispatch_component_types(&mods, &mut types);
    assert_eq!(types, ["minecraft:existing"]);
    let mut recipes = vec!["minecraft:crafting".to_owned()];
    dispatch_datapack_sync(&mut FakeHost::empty(), &mods, &mut recipes);
    assert_eq!(recipes, ["minecraft:crafting"]);
    let mut lines = vec!["old".to_owned()];
    dispatch_screen_tooltip(
        &FakeHost::empty(),
        &mods,
        "ruby:gauge",
        &[4],
        [0.0, 0.0],
        &mut lines,
    );
    assert_eq!(lines, ["old"]);
    let mut success = false;
    dispatch_is_bonemeal_success(
        &mut FakeLevel,
        &mods,
        LevelDimension::Overworld,
        BlockPos::new(0, 0, 0),
        BlockStateId(7),
        &mut success,
    );
    assert!(!success);
    let mut capacity = None;
    dispatch_item_fluid_tank(&FakeHost::empty(), &mods, "ruby:drum", &mut capacity);
    assert_eq!(capacity, None);
    let mut size = None;
    dispatch_tooltip_component(&FakeHost::empty(), &mods, &stack, &mut size);
    assert_eq!(size, None);
    let mut button = None;
    dispatch_screen_click(
        &FakeHost::empty(),
        &mods,
        "ruby:gauge",
        &[4],
        [0.0, 0.0],
        true,
        &mut button,
    );
    assert_eq!(button, None);
    let mut result = None;
    let mut cost = 5;
    dispatch_prepare_anvil(
        &mut FakeHost::empty(),
        &mods,
        7,
        "Alex",
        [0, 0, 0],
        &mut cost,
        &mut result,
    );
    assert!(result.is_none() && cost == 5);
    no_error(&id);
    drop_dir(dir);
}
