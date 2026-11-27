//! Fluid parts (#74, P4): what a Lua `fluid_rules` handler sees of a world fluid's flow, what its
//! answers do, and how a refused or failing answer is kept. The level is an empty double: the hook
//! reads nothing from it.

use std::path::Path;
use std::sync::Arc;

use mod_api::{BlockPos, FluidPart, FluidRulesEvent, LevelDimension, LevelHost};

use crate::discover::{Discovered, ENTRY, Side};
use crate::host::{LuaFluidPart, LuaMod, LuaPart};

/// A level that knows nothing: `fluid_rules` asks the level for nothing.
struct Nothing;

impl LevelHost for Nothing {}

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

/// The fluid part `index` of the mod at `mod_index` in `mods`.
fn part(mods: &Arc<[LuaMod]>, mod_index: usize) -> LuaFluidPart {
    LuaPart::new(mods.clone(), mod_index, 0).unwrap()
}

/// The event the game asks for a flow step of `mymod:oil` at the given numbers.
fn event<'a>(fluid: &'a str, tick_delay: i32, drop_off: i32) -> FluidRulesEvent<'a> {
    FluidRulesEvent {
        dimension: LevelDimension::Overworld,
        pos: BlockPos::new(1, 64, 1),
        fluid,
        tick_delay,
        drop_off,
        slope_find_distance: 4,
        converts_to_source: false,
    }
}

/// A mod that declares `mymod:oil` (drop-off 3) and a `fluid_rules` handler with `body`.
fn oil_mod(root: &Path, body: &str) -> Arc<[LuaMod]> {
    let source = format!(
        "horizon.world_fluid{{id = \"mymod:oil\", drop_off = 3}}\n\
         horizon.fluid_part(\"mymod:oil\", {{fluid_rules = function(event) {body} end}})"
    );
    Arc::from(vec![load_one(root, "mymod", &source)])
}

#[test]
fn a_handler_sees_the_scalars_and_its_answers_arrive() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = oil_mod(
        root.path(),
        "horizon.log(event.fluid .. \" \" .. event.tick_delay .. \" \" .. event.drop_off .. \" \" .. tostring(event.converts_to_source)) \
         event.tick_delay = 2 event.drop_off = 4 event.slope_find_distance = 1 event.converts_to_source = true",
    );
    let mut answer = event("mymod:oil", 5, 3);
    part(&mods, 0).fluid_rules(&Nothing, &mut answer);
    assert_eq!(mods[0].log_lines(), ["mymod:oil 5 3 false"]);
    assert_eq!(answer.tick_delay, 2);
    assert_eq!(answer.drop_off, 4);
    assert_eq!(answer.slope_find_distance, 1);
    assert!(answer.converts_to_source);
}

#[test]
fn an_answer_outside_what_the_world_fluid_accepts_keeps_the_previous_value() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = oil_mod(
        root.path(),
        "event.drop_off = 8 event.tick_delay = 0 event.slope_find_distance = 4",
    );
    let mut answer = event("mymod:oil", 5, 3);
    part(&mods, 0).fluid_rules(&Nothing, &mut answer);
    assert_eq!(answer.drop_off, 3);
    assert_eq!(answer.tick_delay, 5);
    assert_eq!(answer.slope_find_distance, 4);
    let refused =
        crate::host::tests::captured_with_prefix("mymod: fluid_rules: output `drop_off` refused");
    assert_eq!(refused.len(), 1, "got {refused:?}");
    assert_eq!(
        crate::host::tests::captured_with_prefix("mymod: fluid_rules: output `tick_delay` refused")
            .len(),
        1
    );
}

#[test]
fn a_failing_fluid_handler_voids_its_turn() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = oil_mod(root.path(), "event.tick_delay = 9 error(\"boom\")");
    let mut answer = event("mymod:oil", 5, 3);
    part(&mods, 0).fluid_rules(&Nothing, &mut answer);
    assert_eq!(answer.tick_delay, 5);
}

#[test]
fn another_mods_fluid_part_does_not_run_for_this_fluid() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mine = load_one(
        root.path(),
        "mymod",
        "horizon.world_fluid{id = \"mymod:oil\"}\n\
         horizon.fluid_part(\"mymod:oil\", {fluid_rules = function(event) event.converts_to_source = true end})",
    );
    let other = load_one(
        root.path(),
        "other",
        "horizon.world_fluid{id = \"other:tar\"}\n\
         horizon.fluid_part(\"other:tar\", {fluid_rules = function(event) horizon.log(\"ran\") event.tick_delay = 1 end})",
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![mine, other]);
    let mut answer = event("mymod:oil", 5, 3);
    part(&mods, 0).fluid_rules(&Nothing, &mut answer);
    assert!(answer.converts_to_source);
    assert_eq!(answer.tick_delay, 5);
    assert!(mods[1].log_lines().is_empty());
}
