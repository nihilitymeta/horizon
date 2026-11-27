//! Entity parts (#74, P5): what a Lua `is_shearable` handler sees of a shear attempt, what its answer
//! does, and how a refused, failing or panicking handler is kept. The host is the vocab's fake host,
//! with `minecraft:shears` in its item table; the entity is an id the fake host need not hold.

use std::path::Path;
use std::sync::Arc;

use mlua::Lua;
use mod_api::testing::FakeHost;
use mod_api::{
    EntityPart, EntityRef, Identifier, ItemStack, SpawnReason, Tag, UnspawnedMobRef, WorldEpoch,
};

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

/// The fake host, holding the items the tests name (`minecraft:shears` and the drops) in its item
/// table as the game's load does.
fn shears_host() -> FakeHost {
    let mut host = FakeHost::new();
    host.registries
        .resolve_items(
            [
                "minecraft:shears",
                "minecraft:white_wool",
                "minecraft:ender_pearl",
            ]
            .map(|id| Identifier::parse(id).unwrap()),
        )
        .unwrap();
    host.registries.item_component(
        "minecraft:ender_pearl",
        "minecraft:max_stack_size",
        Tag::Int(16),
    );
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

/// A mod whose entity part on `minecraft:sheep` has `HOOK = function(event) BODY end`, for one of the
/// plain handlers (`spawn_cluster_size`, `on_sheared`).
fn plain_mod(root: &Path, id: &str, hook: &str, body: &str) -> LuaMod {
    load_one(
        root,
        id,
        &format!(
            "horizon.entity_part(\"minecraft:sheep\", {{{hook} = function(event) {body} end}})"
        ),
    )
}

/// The mob the game asks about: a spawn group's mob just spawned, in no level yet.
fn mob() -> UnspawnedMobRef {
    UnspawnedMobRef::new(WorldEpoch::new(1), 9)
}

/// What the game asks: the part's `spawn_cluster_size` for a natural spawn with no spawner, where the
/// vanilla limit is 4.
fn ask_cluster(part: &LuaEntityPart, host: &FakeHost) -> Option<u32> {
    part.spawn_cluster_size(host, mob(), SpawnReason::Natural, None, 4)
}

#[test]
fn a_cluster_handler_sees_the_arguments_and_its_answer_arrives() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "mymod",
        "spawn_cluster_size",
        "horizon.log(event.reason .. \" \" .. tostring(event.spawner) .. \" \" .. event.vanilla .. \" \" .. type(event.mob)) return 3",
    )]);
    assert_eq!(ask_cluster(&part(&mods, 0), &FakeHost::new()), Some(3));
    assert_eq!(mods[0].log_lines(), ["natural nil 4 userdata"]);
}

#[test]
fn a_cluster_handler_with_no_answer_leaves_the_size_to_vanilla() {
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "silent",
        "spawn_cluster_size",
        "",
    )]);
    assert_eq!(ask_cluster(&part(&mods, 0), &FakeHost::new()), None);
}

#[test]
fn a_cluster_answer_above_the_bound_is_refused_with_one_warning() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "mymod",
        "spawn_cluster_size",
        "return 65",
    )]);
    assert_eq!(ask_cluster(&part(&mods, 0), &FakeHost::new()), None);
    assert_eq!(
        crate::host::tests::captured_with_prefix("mymod: spawn_cluster_size"),
        [
            "mymod: spawn_cluster_size: answer refused, the mod gives no answer: 65 is outside 0 to 64"
        ]
    );
}

#[test]
fn a_cluster_answer_that_is_not_an_integer_is_refused() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "text_mod",
        "spawn_cluster_size",
        "return \"three\"",
    )]);
    assert_eq!(ask_cluster(&part(&mods, 0), &FakeHost::new()), None);
    assert_eq!(
        crate::host::tests::captured_with_prefix("text_mod: spawn_cluster_size: answer refused")
            .len(),
        1
    );
}

#[test]
fn a_panic_in_a_cluster_handler_ends_the_turn_and_quarantines_the_hook() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "panic_mod",
        "spawn_cluster_size",
        "return 2",
    )]);
    let build = |_: &Lua,
                 _: &crate::hostcall::TurnHost<'_>|
     -> mlua::Result<crate::dispatch::Event> { panic!("the part build broke") };
    let mut answered = 0;
    part(&mods, 0)
        .dispatch_replace(
            "spawn_cluster_size",
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
            "panic_mod: spawn_cluster_size: the game's host panicked"
        )
        .len(),
        1
    );
    assert!(!mods[0].turn().held_here());
}

/// What the game asks on a player's shears: the part's `on_sheared` for the sheep, on a host the
/// test holds, with no player.
fn shear(part: &LuaEntityPart, host: &mut FakeHost) -> Vec<ItemStack> {
    part.on_sheared(host, sheep(), None, &shears())
}

/// The counts of a list of drops, in order.
fn counts(drops: &[ItemStack]) -> Vec<u32> {
    drops.iter().map(ItemStack::count).collect()
}

/// A drop table, as a mod writes one: `count` of `minecraft:white_wool`.
const WOOL: &str = "{id = \"minecraft:white_wool\", count = ";

#[test]
fn two_drops_reach_the_output_in_order() {
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "mymod",
        "on_sheared",
        &format!("return {{{WOOL}2}}, {WOOL}3}}}}"),
    )]);
    let mut host = shears_host();
    assert_eq!(counts(&shear(&part(&mods, 0), &mut host)), [2, 3]);
}

#[test]
fn a_silent_shear_drops_nothing() {
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(root.path(), "silent", "on_sheared", "")]);
    let mut host = shears_host();
    assert!(shear(&part(&mods, 0), &mut host).is_empty());
}

#[test]
fn seventeen_drops_are_refused_whole() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "mymod",
        "on_sheared",
        &format!("local t = {{}} for i = 1, 17 do t[i] = {WOOL}1}} end return t"),
    )]);
    let mut host = shears_host();
    assert!(shear(&part(&mods, 0), &mut host).is_empty());
}

#[test]
fn a_bad_stack_among_good_ones_refuses_the_whole_list() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "mymod",
        "on_sheared",
        &format!("return {{{WOOL}1}}, {{id = \"minecraft:not_an_item\", count = 1}}}}"),
    )]);
    let mut host = shears_host();
    assert!(shear(&part(&mods, 0), &mut host).is_empty());
}

#[test]
fn a_count_above_the_items_maximum_is_refused() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "mymod",
        "on_sheared",
        "return {{id = \"minecraft:ender_pearl\", count = 17}}",
    )]);
    let mut host = shears_host();
    assert!(shear(&part(&mods, 0), &mut host).is_empty());
}

#[test]
fn drops_above_256_items_in_all_are_refused_and_256_is_not() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let five = plain_mod(
        root.path(),
        "five",
        "on_sheared",
        &format!("local t = {{}} for i = 1, 5 do t[i] = {WOOL}64}} end return t"),
    );
    let four = plain_mod(
        root.path(),
        "four",
        "on_sheared",
        &format!("local t = {{}} for i = 1, 4 do t[i] = {WOOL}64}} end return t"),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![five, four]);
    let mut host = shears_host();
    assert!(shear(&part(&mods, 0), &mut host).is_empty());
    assert_eq!(counts(&shear(&part(&mods, 1), &mut host)), [64, 64, 64, 64]);
}

#[test]
fn a_failing_shear_handler_voids_its_turn() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![plain_mod(
        root.path(),
        "mymod",
        "on_sheared",
        &format!("error(\"boom\") return {{{WOOL}1}}}}"),
    )]);
    let mut host = shears_host();
    assert!(shear(&part(&mods, 0), &mut host).is_empty());
}

#[test]
fn a_shear_runs_only_the_part_the_game_asked() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mine = plain_mod(
        root.path(),
        "mymod",
        "on_sheared",
        &format!("return {{{WOOL}1}}}}"),
    );
    let other = plain_mod(
        root.path(),
        "other",
        "on_sheared",
        "horizon.log(\"ran\") return {}",
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![mine, other]);
    let mut host = shears_host();
    assert_eq!(counts(&shear(&part(&mods, 0), &mut host)), [1]);
    assert!(mods[1].log_lines().is_empty());
}
