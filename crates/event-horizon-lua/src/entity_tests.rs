//! Entity parts (#74, P5): what a Lua `is_shearable` handler sees of a shear attempt, what its answer
//! does, and how a refused, failing or panicking handler is kept. The host is the vocab's fake host,
//! with `minecraft:shears` in its item table; the entity is an id the fake host need not hold.

use std::path::Path;
use std::sync::Arc;

use mlua::Lua;
use mod_api::testing::FakeHost;
use mod_api::{EntityPart, EntityRef, Identifier, ItemStack, WorldEpoch};

use crate::discover::{Discovered, ENTRY, Side};
use crate::host::{LuaEntityPart, LuaMod, LuaPart};

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

/// A mod whose entity part on `minecraft:sheep` has `is_shearable = function(event) BODY end`.
fn sheep_mod(root: &Path, id: &str, body: &str) -> LuaMod {
    load_one(
        root,
        id,
        &format!(
            "horizon.entity_part(\"minecraft:sheep\", {{is_shearable = function(event) {body} end}})"
        ),
    )
}

/// The entity part of the mod at `index` in `mods`.
fn part(mods: &Arc<[LuaMod]>, index: usize) -> LuaEntityPart {
    LuaPart::new(mods.clone(), index, 0).unwrap()
}

/// The fake host, holding `minecraft:shears` in its item table as the game's load does.
fn shears_host() -> FakeHost {
    let mut host = FakeHost::new();
    host.registries
        .resolve_items([Identifier::parse("minecraft:shears").unwrap()])
        .unwrap();
    host
}

/// The shears a player holds, in the fake host's item table.
fn shears() -> ItemStack {
    ItemStack::new(mod_api::__private::item_key("minecraft:shears").unwrap(), 1).unwrap()
}

/// The sheep the game asks about.
fn sheep() -> EntityRef {
    EntityRef::new(WorldEpoch::new(1), 7)
}

/// What the game asks: the part's `is_shearable` for the sheep, with no player (a dispenser's shears).
fn ask(part: &LuaEntityPart, host: &FakeHost) -> Option<bool> {
    part.is_shearable(host, sheep(), None, &shears())
}

#[test]
fn a_handler_sees_the_arguments_and_its_answer_arrives() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![sheep_mod(
        root.path(),
        "mymod",
        "horizon.log(event.shears.id .. \" \" .. type(event.entity) .. \" \" .. tostring(event.player)) return false",
    )]);
    assert_eq!(ask(&part(&mods, 0), &shears_host()), Some(false));
    assert_eq!(mods[0].log_lines(), ["minecraft:shears userdata nil"]);
}

#[test]
fn a_true_answer_arrives_and_no_answer_leaves_the_decision_to_the_game() {
    let root = tempfile::tempdir().unwrap();
    let yes: Arc<[LuaMod]> = Arc::from(vec![sheep_mod(root.path(), "yes", "return true")]);
    assert_eq!(ask(&part(&yes, 0), &shears_host()), Some(true));
    let silent: Arc<[LuaMod]> = Arc::from(vec![sheep_mod(root.path(), "silent", "")]);
    assert_eq!(ask(&part(&silent, 0), &shears_host()), None);
}

#[test]
fn an_answer_that_is_not_a_boolean_is_refused_and_leaves_no_opinion() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![sheep_mod(root.path(), "mymod", "return 1")]);
    assert_eq!(ask(&part(&mods, 0), &shears_host()), None);
}

#[test]
fn a_failing_handler_voids_its_turn() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![sheep_mod(root.path(), "mymod", "error(\"boom\")")]);
    assert_eq!(ask(&part(&mods, 0), &shears_host()), None);
}

#[test]
fn another_mods_part_on_the_same_entity_is_not_run_by_this_one() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mine = sheep_mod(root.path(), "mymod", "return true");
    let other = sheep_mod(root.path(), "other", "horizon.log(\"ran\") return false");
    let mods: Arc<[LuaMod]> = Arc::from(vec![mine, other]);
    assert_eq!(ask(&part(&mods, 0), &shears_host()), Some(true));
    assert!(mods[1].log_lines().is_empty());
}

#[test]
fn an_entity_part_has_no_hook_subscription_and_its_handler_runs_by_name() {
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![sheep_mod(root.path(), "mymod", "return true")]);
    assert!(part(&mods, 0).subscribed().is_empty());
}

#[test]
fn a_panic_in_an_entity_part_build_ends_the_turn_and_quarantines_the_hook() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![sheep_mod(root.path(), "panic_mod", "return true")]);
    let build = |_: &Lua,
                 _: &crate::hostcall::TurnHost<'_>|
     -> mlua::Result<crate::dispatch::Event> { panic!("the part build broke") };
    let mut answered = 0;
    part(&mods, 0)
        .dispatch_replace(
            "is_shearable",
            crate::hostcall::TurnHost::None,
            &build,
            &mut |_, _, _| {
                answered += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(answered, 0);
    assert_eq!(
        crate::host::tests::captured_with_prefix(
            "panic_mod: is_shearable: the game's host panicked"
        ),
        [
            "panic_mod: is_shearable: the game's host panicked: the part build broke (this mod no longer receives this hook)"
        ]
    );
    assert!(!mods[0].turn().held_here());
}
