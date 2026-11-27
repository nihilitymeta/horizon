//! Block states in the block parts (#74): what a Lua handler sees of a state, what a state answer
//! may be, and what a state the level cannot convert does to its hook. The level is a double that
//! knows two states of `minecraft:oak_stairs` (`facing` east and north) and of `minecraft:stone`,
//! built from the mod API's own block definitions.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use mlua::Lua;
use mod_api::{
    BlockEntityPart, BlockPart, BlockPos, BlockState, IsValidBonemealTargetEvent, LevelDimension,
    LevelHost, LevelRandomHost, LevelSide, OnBlockStateChangeEvent, PistonPushReactionEvent,
    PreRemoveSideEffectsEvent, PushReaction, RemoveBlockEvent, StateConstraint,
};

use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch;
use crate::host::{LuaBlockEntityPart, LuaBlockPart, LuaMod, LuaPart};
use crate::hostcall::TurnHost;

/// A level that converts the states it was given, by their text, and no others.
struct Level {
    known: Vec<(String, BlockState)>,
}

impl LevelHost for Level {
    fn state_string(&self, state: BlockState) -> Option<String> {
        self.known
            .iter()
            .find(|(_, known)| *known == state)
            .map(|(text, _)| text.clone())
    }

    fn block_state_id(&self, name: &str) -> Option<BlockState> {
        self.known
            .iter()
            .find(|(text, _)| text == name)
            .map(|(_, state)| *state)
    }
}

impl LevelRandomHost for Level {}

/// The state `name` with `facing` set (none for a block without the property), from the registry.
fn state(name: &str, facing: Option<&str>) -> BlockState {
    let metadata = mod_api::__private::block_definitions().unwrap();
    let block = mod_api::__private::block_key(name).unwrap();
    let constraints: Vec<StateConstraint> = facing
        .map(|value| StateConstraint::new("facing", value))
        .into_iter()
        .collect();
    BlockState::of(metadata, block, &constraints).unwrap()
}

/// Installs the test registry once per process: stone, oak stairs with `facing` east or north, and
/// a two-property block `minecraft:probe_slab` (`facing` east or north, `half` bottom or top, four
/// states in that order). It also installs the log capture, so a test sees the warnings its hook
/// raises. The global tables are a process-wide once-cell: a test that needs them uninstalled runs
/// in its own process (nextest does this).
pub(crate) fn registry() {
    crate::host::tests::install_capture();
    let empty = BTreeMap::new();
    let east = BTreeMap::from([("facing".to_owned(), "east".to_owned())]);
    let north = BTreeMap::from([("facing".to_owned(), "north".to_owned())]);
    let probe = |facing: &str, half: &str| {
        BTreeMap::from([
            ("facing".to_owned(), facing.to_owned()),
            ("half".to_owned(), half.to_owned()),
        ])
    };
    let probe_states = [
        probe("east", "bottom"),
        probe("east", "top"),
        probe("north", "bottom"),
        probe("north", "top"),
    ];
    let probe_refs: Vec<_> = probe_states.iter().collect();
    mod_api::__private::part_tests::install_parts_fixture(&[]).unwrap();
    mod_api::__private::resolve_block_keys(
        [
            "minecraft:stone",
            "minecraft:oak_stairs",
            "minecraft:probe_slab",
        ],
        |name| {
            let result = match name {
                "minecraft:oak_stairs" => {
                    horizon_mod_runtime::block_definitions::BlockDefinition::new(
                        &[&east, &north],
                        0,
                    )
                }
                "minecraft:probe_slab" => {
                    horizon_mod_runtime::block_definitions::BlockDefinition::new(&probe_refs, 0)
                }
                _ => horizon_mod_runtime::block_definitions::BlockDefinition::new(&[&empty], 0),
            };
            result.map_err(|error| format!("{error:?}"))
        },
    )
    .unwrap();
}

/// The text a level writes for each state: the registry's states of the test.
fn known_level(with_stairs: bool) -> Level {
    let mut known = vec![("minecraft:stone".to_owned(), state("minecraft:stone", None))];
    if with_stairs {
        known.push((
            "minecraft:oak_stairs[facing=east]".to_owned(),
            state("minecraft:oak_stairs", Some("east")),
        ));
        known.push((
            "minecraft:oak_stairs[facing=north]".to_owned(),
            state("minecraft:oak_stairs", Some("north")),
        ));
    }
    Level { known }
}

/// Loads one mod whose main.luau is `source` and whose block part is on `minecraft:oak_stairs`.
fn load(root: &Path, source: &str) -> Arc<[LuaMod]> {
    let dir = root.join("first_mod");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ENTRY), source).unwrap();
    let found = Discovered {
        dir,
        id: "first_mod".to_owned(),
        version: "1".to_owned(),
        side: Side::Both,
    };
    Arc::from(vec![LuaMod::load(&found).unwrap()])
}

/// The block part of the one mod loaded by [`load`].
fn part(mods: &Arc<[LuaMod]>) -> LuaBlockPart {
    LuaPart::new(mods.clone(), 0, 0).unwrap()
}

fn pos() -> BlockPos {
    BlockPos::new(1, 64, 1)
}

#[test]
fn on_block_state_change_sees_old_and_new_as_text_and_a_failing_handler_voids_its_turn() {
    registry();
    let root = tempfile::tempdir().unwrap();
    let mods = load(
        root.path(),
        r#"horizon.block_part("first_mod:stairs", {on_block_state_change = function(event)
             horizon.log(event.state.id .. " " .. event.old.props.facing .. " " .. event.new.props.facing)
           end})"#,
    );
    let level = known_level(true);
    let mut event = OnBlockStateChangeEvent {
        dimension: LevelDimension::Overworld,
        pos: pos(),
        state: state("minecraft:oak_stairs", Some("north")),
        old: state("minecraft:oak_stairs", Some("east")),
        new: state("minecraft:oak_stairs", Some("north")),
        predicted: false,
    };
    part(&mods).on_block_state_change(LevelSide::Server(&level), &mut event);
    assert_eq!(mods[0].log_lines(), ["minecraft:oak_stairs east north"]);

    // A handler that raises voids its turn: nothing is committed.
    let failing = load(
        root.path(),
        r#"horizon.block_part("first_mod:stairs", {on_block_state_change = function() error("boom") end})"#,
    );
    let build = |lua: &Lua, _: &TurnHost<'_>| dispatch::new_event(lua, lua.create_table()?, &[]);
    let mut committed = 0;
    part(&failing)
        .dispatch_turns(
            "on_block_state_change",
            TurnHost::None,
            &|| false,
            &build,
            &mut |_, _, _| {
                committed += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(committed, 0);
}

#[test]
fn is_valid_bonemeal_target_is_not_delivered_when_the_level_cannot_convert_the_state() {
    registry();
    let root = tempfile::tempdir().unwrap();
    let mods = load(
        root.path(),
        r#"horizon.block_part("first_mod:stairs", {is_valid_bonemeal_target = function() horizon.log("ran") return true end})"#,
    );
    // The level knows stone only, so the stairs state has no text here.
    let level = known_level(false);
    let mut event = IsValidBonemealTargetEvent {
        dimension: LevelDimension::Overworld,
        pos: pos(),
        state: state("minecraft:oak_stairs", Some("east")),
    };
    let answer = part(&mods).is_valid_bonemeal_target(LevelSide::Server(&level), &mut event);
    assert_eq!(answer, None);
    assert!(mods[0].log_lines().is_empty());
    assert_eq!(
        crate::host::tests::captured_with_prefix("is_valid_bonemeal_target: not delivered").len(),
        1
    );
}

#[test]
fn pre_remove_side_effects_is_not_delivered_when_the_level_cannot_convert_the_state() {
    registry();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("first_mod");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(ENTRY),
        r#"horizon.block{id = "first_mod:stairs_block", template = "minecraft:stone"}
           horizon.block_entity_type{id = "first_mod:press", blocks = {"first_mod:stairs_block"}}
           horizon.block_entity_part("first_mod:press", {pre_remove_side_effects = function() horizon.log("ran") end})"#,
    )
    .unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![
        LuaMod::load(&Discovered {
            dir,
            id: "first_mod".to_owned(),
            version: "1".to_owned(),
            side: Side::Both,
        })
        .unwrap(),
    ]);
    let entity = mod_api::ModBlockEntity::new("first_mod:press".to_owned(), Default::default());
    let part: LuaBlockEntityPart = LuaPart::new(mods.clone(), 0, 0).unwrap();
    // The level knows stone only, so the stairs state has no text here.
    let mut level = known_level(false);
    let mut drops = Vec::new();
    let mut event = PreRemoveSideEffectsEvent {
        pos: pos(),
        state: state("minecraft:oak_stairs", Some("east")),
        entity: &entity,
        drops: &mut drops,
    };
    part.pre_remove_side_effects(&mut level, &mut event);
    assert!(drops.is_empty());
    assert!(mods[0].log_lines().is_empty());
    assert_eq!(
        crate::host::tests::captured_with_prefix("pre_remove_side_effects: not delivered").len(),
        1
    );
}

#[test]
fn a_block_state_round_trips_through_remove_block_and_an_unknown_answer_is_refused_whole() {
    registry();
    let root = tempfile::tempdir().unwrap();
    let mods = load(
        root.path(),
        r#"horizon.block_part("first_mod:stairs", {remove_block = function(event)
             horizon.log(event.state.id .. " " .. event.state.props.facing)
             if event.moved_by_piston then return {id = "minecraft:no_such_block", props = {}} end
             return {id = "minecraft:oak_stairs", props = {facing = "north"}}
           end})"#,
    );
    let level = known_level(true);
    let mut event = RemoveBlockEvent {
        dimension: LevelDimension::Overworld,
        pos: pos(),
        state: state("minecraft:oak_stairs", Some("east")),
        moved_by_piston: false,
    };
    let valid = part(&mods).remove_block(&level, &mut event);
    assert_eq!(valid, Some(state("minecraft:oak_stairs", Some("north"))));

    event.moved_by_piston = true;
    let unknown = part(&mods).remove_block(&level, &mut event);
    assert_eq!(unknown, None);
    assert_eq!(
        mods[0].log_lines(),
        ["minecraft:oak_stairs east", "minecraft:oak_stairs east"]
    );
}

#[test]
fn piston_push_reaction_gets_its_state_through_the_global_tables_and_a_bad_answer_keeps_the_last() {
    // The hook has no host parameter, so its state comes from the installed tables as `{id, props}`
    // in name order, and its answer is a reaction name. A refused answer leaves the reaction unset.
    registry();
    let root = tempfile::tempdir().unwrap();
    let mods = load(
        root.path(),
        r#"horizon.block_part("first_mod:stairs", {piston_push_reaction = function(event)
             horizon.log(event.state.id .. " " .. event.state.props.facing .. " " .. event.state.props.half)
             if event.state.props.half == "top" then return "not_a_reaction" end
             return "push"
           end})"#,
    );
    let mut event = PistonPushReactionEvent {
        state: BlockState::of(
            mod_api::__private::block_definitions().unwrap(),
            mod_api::__private::block_key("minecraft:probe_slab").unwrap(),
            &[
                StateConstraint::new("facing", "east"),
                StateConstraint::new("half", "bottom"),
            ],
        )
        .unwrap(),
    };
    let answer = part(&mods).piston_push_reaction(&mut event);
    assert_eq!(answer, Some(PushReaction::Push));
    assert_eq!(mods[0].log_lines(), ["minecraft:probe_slab east bottom"]);

    event.state = BlockState::of(
        mod_api::__private::block_definitions().unwrap(),
        mod_api::__private::block_key("minecraft:probe_slab").unwrap(),
        &[
            StateConstraint::new("facing", "north"),
            StateConstraint::new("half", "top"),
        ],
    )
    .unwrap();
    let refused = part(&mods).piston_push_reaction(&mut event);
    assert_eq!(refused, None);
}
