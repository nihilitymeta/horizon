//! Brain behaviors (#74, B2a): what a mod declares in the `behaviors` table of an entity part, what the
//! brain's registration gets, and the refusals. The handler bodies are B2a stubs (see `behavior.rs`), so
//! these tests pin the declaration and the registration, not the answers.

use std::path::Path;
use std::sync::Arc;

use mod_api::{
    BehaviorRefusal, BehaviorRegistrar, BehaviorRegistrarBackend, EntityPart, RegisteredBehavior,
};

use crate::declare::{self, BehaviorRecord, behavior_handler_key, goal_handler_key};
use crate::discover::{Discovered, ENTRY, Side};
use crate::host::{
    LuaEntityPart, LuaMod, LuaPart, tests::captured_with_prefix, tests::install_capture,
};
use crate::state::new_state;

/// Loads one mod `id` whose main.luau is `source`, from its own folder under `root`. The source's
/// `mymod:` names are the mod's own, so they are read in the namespace `id`.
fn load_one(root: &Path, id: &str, source: &str) -> LuaMod {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let source = source.replace("mymod:", &format!("{id}:"));
    std::fs::write(dir.join(ENTRY), source).unwrap();
    LuaMod::load(&Discovered {
        dir,
        id: id.to_owned(),
        version: "1".to_owned(),
        side: Side::Server,
    })
    .unwrap()
}

/// The declaration of `source` as the mod `mymod`, or the refusal.
fn declared(source: &str) -> Result<declare::Declaration, String> {
    let lua = new_state().unwrap();
    declare::declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &|_| true)
}

/// The mod `mymod`'s behavior records, from its declaration.
fn records(source: &str) -> Vec<BehaviorRecord> {
    declared(source).unwrap().behaviors
}

/// One behavior table: `name`, `activity`, `priority`, and the handler bodies as `(hook, body)` pairs.
fn behavior(name: &str, activity: &str, priority: u8, handlers: &[(&str, &str)]) -> String {
    let mut out = format!("{{name = \"{name}\", activity = \"{activity}\", priority = {priority}");
    for (hook, body) in handlers {
        out.push_str(&format!(", {hook} = function(e) {body} end"));
    }
    out.push('}');
    out
}

/// The source of a villager part (`minecraft:villager`) with `behaviors`.
fn villager(behaviors: &[String]) -> String {
    format!(
        "horizon.entity_part(\"minecraft:villager\", {{behaviors = {{{}}}}})",
        behaviors.join(", ")
    )
}

/// The game's view of a brain being built: the behaviors the registrar received, and the refusal (if
/// any) every add gets.
struct Brain {
    added: Vec<RegisteredBehavior>,
    refusal: Option<BehaviorRefusal>,
}

impl Brain {
    fn new() -> Brain {
        Brain {
            added: Vec::new(),
            refusal: None,
        }
    }

    fn refusing(refusal: BehaviorRefusal) -> Brain {
        Brain {
            added: Vec::new(),
            refusal: Some(refusal),
        }
    }
}

impl BehaviorRegistrarBackend for Brain {
    fn add_behavior(&mut self, registration: RegisteredBehavior) -> Result<(), BehaviorRefusal> {
        match self.refusal {
            Some(refusal) => Err(refusal),
            None => {
                self.added.push(registration);
                Ok(())
            }
        }
    }
    fn mob_id(&self) -> &str {
        "minecraft:villager"
    }
}

/// Builds a brain of `mob` from the mod's entity part `index`: its `register_behaviors`, on `brain`.
fn build(mods: &Arc<[LuaMod]>, index: usize, mob: &str, brain: &mut Brain) {
    let part: LuaEntityPart = LuaPart::new(mods.clone(), index, 0).unwrap();
    let mut registrar = BehaviorRegistrar::new(brain, mob, "mymod", "part");
    part.register_behaviors(&mut registrar);
}

// --- Declaration -----------------------------------------------------------------------------

#[test]
fn a_behavior_declaration_keeps_its_name_activity_priority_durations_and_hooks() {
    let source = villager(&[behavior(
        "mymod:farm",
        "work",
        7,
        &[
            ("start", ""),
            ("check_extra_start_conditions", "return true"),
            ("can_still_use", "return false"),
        ],
    )
    .replace(
        "priority = 7",
        "priority = 7, min_duration = 10, max_duration = 30",
    )]);
    let found = records(&source);
    assert_eq!(found.len(), 1);
    let record = &found[0];
    assert_eq!(record.entity, "minecraft:villager");
    assert_eq!(record.name, "mymod:farm");
    assert_eq!(record.activity, mod_api::Activity::Work);
    assert_eq!(record.priority, 7);
    assert_eq!((record.min_duration, record.max_duration), (10, 30));
    assert_eq!(
        record.hooks,
        ["can_still_use", "check_extra_start_conditions", "start"]
    );
}

#[test]
fn a_behavior_without_durations_gets_the_default_window() {
    let found = records(&villager(&[behavior(
        "mymod:idle",
        "idle",
        1,
        &[("start", "")],
    )]));
    assert_eq!((found[0].min_duration, found[0].max_duration), (40, 80));
}

#[test]
fn a_part_with_only_behaviors_is_a_part() {
    let declaration = declared(&villager(&[behavior(
        "mymod:a",
        "idle",
        1,
        &[("tick", "")],
    )]))
    .unwrap();
    assert_eq!(declaration.entity_parts.len(), 1);
    assert_eq!(declaration.behaviors.len(), 1);
}

#[test]
fn eight_behaviors_are_the_most_a_part_may_declare() {
    let eight: Vec<String> = (0..8)
        .map(|n| behavior(&format!("mymod:b{n}"), "idle", 1, &[("tick", "")]))
        .collect();
    assert_eq!(records(&villager(&eight)).len(), 8);
    let mut nine = eight;
    nine.push(behavior("mymod:b8", "idle", 1, &[("tick", "")]));
    let err = declared(&villager(&nine)).unwrap_err();
    assert!(err.contains("at most 8 behaviors"), "got {err}");
}

#[test]
fn an_unknown_activity_is_refused_by_name() {
    let err = declared(&villager(&[behavior(
        "mymod:a",
        "dance",
        1,
        &[("tick", "")],
    )]))
    .unwrap_err();
    assert!(
        err.contains("activity 'dance' is not a brain activity"),
        "got {err}"
    );
}

#[test]
fn an_unknown_behavior_key_is_refused_by_name() {
    let source = villager(&[behavior("mymod:a", "idle", 1, &[("tick", "")])
        .replace("priority = 1", "priority = 1, entry = {}")]);
    let err = declared(&source).unwrap_err();
    assert!(err.contains("unknown behavior key 'entry'"), "got {err}");
}

#[test]
fn a_behavior_name_must_be_in_the_mods_namespace_and_short_enough() {
    let other = declared(&villager(&[behavior(
        "other:a",
        "idle",
        1,
        &[("tick", "")],
    )]))
    .unwrap_err();
    assert!(
        other.contains("outside this mod's namespace 'mymod'"),
        "got {other}"
    );
    let long = format!("mymod:{}", "a".repeat(60));
    let err = declared(&villager(&[behavior(&long, "idle", 1, &[("tick", "")])])).unwrap_err();
    assert!(err.contains("longer than 64 bytes"), "got {err}");
}

#[test]
fn a_behavior_name_is_declared_once_per_mod() {
    let twice = villager(&[
        behavior("mymod:a", "idle", 1, &[("tick", "")]),
        behavior("mymod:a", "work", 2, &[("tick", "")]),
    ]);
    let err = declared(&twice).unwrap_err();
    assert!(
        err.contains("the behavior 'mymod:a' is declared twice"),
        "got {err}"
    );
}

#[test]
fn a_behavior_needs_an_activity_a_priority_and_a_handler() {
    let no_activity = declared(&villager(&[
        "{name = \"mymod:a\", priority = 1, tick = function(e) end}".to_owned(),
    ]))
    .unwrap_err();
    assert!(
        no_activity.contains("needs an activity"),
        "got {no_activity}"
    );
    let no_priority = declared(&villager(&[
        "{name = \"mymod:a\", activity = \"idle\", tick = function(e) end}".to_owned(),
    ]))
    .unwrap_err();
    assert!(
        no_priority.contains("needs a priority"),
        "got {no_priority}"
    );
    let no_handler = declared(&villager(&[
        "{name = \"mymod:a\", activity = \"idle\", priority = 1}".to_owned(),
    ]))
    .unwrap_err();
    assert!(no_handler.contains("needs a handler"), "got {no_handler}");
}

#[test]
fn a_behavior_priority_out_of_range_or_not_whole_is_refused() {
    let high = declared(&villager(&[behavior(
        "mymod:a",
        "idle",
        21,
        &[("tick", "")],
    )]));
    assert!(high.unwrap_err().contains("from 0 to 20, not 21"));
    let fraction = declared(&villager(&[
        "{name = \"mymod:a\", activity = \"idle\", priority = 1.5, tick = function(e) end}"
            .to_owned(),
    ]));
    assert!(
        fraction
            .unwrap_err()
            .contains("a behavior's priority is a whole number")
    );
}

#[test]
fn a_duration_window_vanilla_does_not_draw_is_refused() {
    let negative = behavior("mymod:a", "idle", 1, &[("tick", "")]).replace(
        "priority = 1",
        "priority = 1, min_duration = -1, max_duration = 5",
    );
    let err = declared(&villager(&[negative])).unwrap_err();
    assert!(err.contains("is not one vanilla draws"), "got {err}");
    let empty = behavior("mymod:a", "idle", 1, &[("tick", "")]).replace(
        "priority = 1",
        "priority = 1, min_duration = 5, max_duration = 4",
    );
    let err = declared(&villager(&[empty])).unwrap_err();
    assert!(err.contains("(5, 4)"), "got {err}");
}

#[test]
fn register_behaviors_as_a_handler_is_refused_naming_the_behaviors_table() {
    let err = declared(
        "horizon.entity_part(\"minecraft:villager\", {register_behaviors = function() end})",
    )
    .unwrap_err();
    assert!(
        err.contains("'register_behaviors'") && err.contains("behaviors table"),
        "got {err}"
    );
}

#[test]
fn behaviors_on_a_block_part_are_refused() {
    let err = declared("horizon.block_part(\"minecraft:stone\", {behaviors = {}})").unwrap_err();
    assert!(err.contains("'behaviors'"), "got {err}");
}

#[test]
fn a_behavior_handler_key_is_its_own_shape_and_never_a_goal_key() {
    assert_eq!(
        behavior_handler_key("mymod:b", "tick"),
        "behavior mymod:b tick"
    );
    assert_ne!(
        behavior_handler_key("mymod:x", "tick"),
        goal_handler_key("mymod:x", "tick"),
        "a behavior and a goal of one name never share a handler key"
    );
    // A quarantine of the goal's key does not reach the behavior's key of the same name.
    crate::quarantine::quarantine("behavior_key_shape", &goal_handler_key("mymod:x", "tick"));
    assert!(!crate::quarantine::is_quarantined(
        "behavior_key_shape",
        &behavior_handler_key("mymod:x", "tick")
    ));
}

// --- Registration ----------------------------------------------------------------------------

#[test]
fn register_behaviors_adds_each_declared_behavior_with_its_activity_priority_and_durations() {
    let root = tempfile::tempdir().unwrap();
    let source = villager(&[
        behavior("mymod:farm", "work", 5, &[("tick", "")]).replace(
            "priority = 5",
            "priority = 5, min_duration = 10, max_duration = 30",
        ),
        behavior("mymod:rest", "rest", 2, &[("start", "")]),
    ]);
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "reg", &source)]);
    let mut brain = Brain::new();
    build(&mods, 0, "minecraft:villager", &mut brain);
    let seen: Vec<(&str, mod_api::Activity, i32, i32, i32)> = brain
        .added
        .iter()
        .map(|r| (r.behavior.name(), r.activity, r.priority, r.min, r.max))
        .collect();
    assert_eq!(
        seen,
        [
            ("reg:farm", mod_api::Activity::Work, 5, 10, 30),
            ("reg:rest", mod_api::Activity::Rest, 2, 40, 80),
        ]
    );
    assert_eq!(
        brain.added[0].part, "part",
        "the registrar names the part it was asked for"
    );
}

#[test]
fn a_brain_built_runs_no_handler_and_a_declared_behavior_does_not_start() {
    let root = tempfile::tempdir().unwrap();
    let source = villager(&[behavior(
        "mymod:b",
        "idle",
        1,
        &[(
            "check_extra_start_conditions",
            "horizon.log(\"ran\") return true",
        )],
    )]);
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "stub", &source)]);
    let mut brain = Brain::new();
    build(&mods, 0, "minecraft:villager", &mut brain);
    assert!(
        mods[0].log_lines().is_empty(),
        "building a brain runs no Lua"
    );
    // B2a stub: the registered behavior answers false, so it never starts.
    let mut behavior = brain.added.pop().unwrap().behavior;
    let mut cx = StubCx::default();
    let mut bcx = mod_api::BehaviorCx::new(&mut cx);
    assert!(!behavior.check_extra_start_conditions(&mut bcx));
    assert!(!behavior.can_still_use(&mut bcx));
}

/// A backend for the B2a stubs: a behavior's `BehaviorCx` reads nothing and writes nothing here.
#[derive(Default)]
struct StubCx {
    host: mod_api::testing::FakeHost,
}

impl mod_api::BehaviorCxBackend for StubCx {
    fn host(&self) -> &dyn mod_api::Host {
        &self.host
    }
    fn mob(&self) -> mod_api::MobRef {
        mod_api::MobRef::new(mod_api::EntityRef::new(mod_api::WorldEpoch::new(1), 7))
    }
    fn walk_target(&self) -> Option<mod_api::BrainWalkTarget> {
        None
    }
    fn look_target(&self) -> Option<[f64; 3]> {
        None
    }
    fn attack_target(&self) -> Option<mod_api::LivingRef> {
        None
    }
    fn set_brain_target(&mut self, _target: Option<mod_api::LivingRef>) -> bool {
        false
    }
    fn set_walk_target(&mut self, _at: [i32; 3], _speed: f64, _close: i32) {}
}

#[test]
fn a_mob_of_another_type_gets_only_its_own_parts_behaviors() {
    let root = tempfile::tempdir().unwrap();
    // A zombie part with one behavior, and a villager part with another: each part registers its own.
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{behaviors = {{{}}}}})\n{}",
        behavior("mymod:zed", "idle", 1, &[("tick", "")]),
        villager(&[behavior("mymod:vil", "work", 2, &[("tick", "")])]),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "other", &source)]);
    let mut villagers = Brain::new();
    let villager_part: LuaEntityPart = LuaPart::new(mods.clone(), 0, 1).unwrap();
    let mut registrar =
        BehaviorRegistrar::new(&mut villagers, "minecraft:villager", "mymod", "part");
    villager_part.register_behaviors(&mut registrar);
    let names: Vec<&str> = villagers.added.iter().map(|r| r.behavior.name()).collect();
    assert_eq!(names, ["other:vil"]);
}

#[test]
fn a_refused_add_is_logged_once_per_mod_not_per_mob() {
    install_capture();
    let root = tempfile::tempdir().unwrap();
    let source = villager(&[behavior("mymod:b", "idle", 1, &[("tick", "")])]);
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "refused", &source)]);
    for _ in 0..3 {
        let mut brain = Brain::refusing(BehaviorRefusal::NoBrain);
        build(&mods, 0, "minecraft:villager", &mut brain);
        assert!(brain.added.is_empty());
    }
    let logged = captured_with_prefix("refused: behavior refused:b");
    assert_eq!(logged.len(), 1, "got {logged:?}");
    assert!(
        logged[0].contains("the entity type has no brain"),
        "got {}",
        logged[0]
    );
}
