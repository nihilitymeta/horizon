//! Brain behaviors (#74, B2): what a mod declares in the `behaviors` table of an entity part, what the
//! brain's registration gets, the answers and actions of a behavior's handlers, and the shared time budget.
//! The budget tests move a clock by hand or step it; the deadline tests loop for real.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mod_api::testing::FakeHost;
use mod_api::{
    Behavior, BehaviorCx, BehaviorCxBackend, BehaviorRefusal, BehaviorRegistrar,
    BehaviorRegistrarBackend, EntityPart, EntityRef, Goal, GoalContext, GoalCxBackend,
    GoalRegistrar, GoalRegistrarBackend, LegacyRandom, LivingRef, MobRef, MoveToPathError,
    PathNode, RegisteredBehavior, WorldEpoch,
};

use crate::declare::{self, BehaviorRecord, behavior_handler_key, goal_handler_key};
use crate::discover::{Discovered, ENTRY, Side};
use crate::goal_budget::{Budget, Clock, GOAL_RESERVE, GOAL_WINDOW, GOAL_WINDOW_BUDGET};
use crate::host::{
    LuaEntityPart, LuaMod, LuaPart, tests::captured_with_prefix, tests::install_capture,
};
use crate::mob_turn::TURN_BUDGET_OVERRIDE;
use crate::quarantine;
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

// --- The handlers' turns (B2b) ---------------------------------------------------------------

/// A clock the test moves by hand.
struct ManualClock(Mutex<Instant>);

impl ManualClock {
    fn new() -> Arc<ManualClock> {
        Arc::new(ManualClock(Mutex::new(Instant::now())))
    }
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}

/// A clock that moves 1 ms on every read, so a turn that reads it once on each side of the handler charges
/// exactly 1 ms to the budget.
struct StepClock(Mutex<Instant>);

impl StepClock {
    fn new() -> Arc<StepClock> {
        Arc::new(StepClock(Mutex::new(Instant::now())))
    }
}

impl Clock for StepClock {
    fn now(&self) -> Instant {
        let mut at = self.0.lock().unwrap();
        let now = *at;
        *at += Duration::from_millis(1);
        now
    }
}

/// The budget on a clock the test moves.
fn budget_on(clock: &Arc<ManualClock>) -> Arc<Budget> {
    Arc::new(Budget::new(clock.clone()))
}

/// Sets this test's turn deadline (`None`: the real `GOAL_TURN_BUDGET`). Tests that check what a handler
/// answers or applies use a generous one, so a loaded machine cannot time them out.
fn turn_deadline(limit: Option<Duration>) {
    TURN_BUDGET_OVERRIDE.with(|cell| cell.set(limit));
}

/// The behaviors of the villager in mod 0, built on `budget`, in declaration order. Their turns get a
/// generous deadline (see `turn_deadline`).
fn brain_on(mods: &Arc<[LuaMod]>, budget: Arc<Budget>) -> Vec<Box<dyn Behavior>> {
    turn_deadline(Some(Duration::from_secs(2)));
    let mut brain = Brain::new();
    let mut registrar = BehaviorRegistrar::new(&mut brain, "minecraft:villager", "mymod", "part");
    crate::behavior::add_declared_on(mods, 0, "minecraft:villager", &mut registrar, budget);
    brain.added.into_iter().map(|r| r.behavior).collect()
}

/// Mod `id` with one behavior `mymod:b` on the villager, at priority 1, with `handlers`.
fn one_behavior(root: &Path, id: &str, handlers: &[(&str, &str)]) -> Arc<[LuaMod]> {
    let source = villager(&[behavior("mymod:b", "idle", 1, handlers)]);
    Arc::from(vec![load_one(root, id, &source)])
}

/// The mod's log lines, as its behaviors wrote them.
fn lines(mods: &Arc<[LuaMod]>) -> Vec<String> {
    mods[0].log_lines()
}

/// The game's view of a behavior's mob: the backend a `BehaviorCx` reads and writes. It records the writes; a
/// `panicking` one breaks on `set_walk_target`.
#[derive(Default)]
struct Cx {
    host: FakeHost,
    /// The writes the backend received, in order.
    calls: Vec<String>,
    panics: bool,
}

impl Cx {
    fn panicking() -> Cx {
        Cx {
            panics: true,
            ..Cx::default()
        }
    }
}

impl BehaviorCxBackend for Cx {
    fn host(&self) -> &dyn mod_api::Host {
        &self.host
    }
    fn mob(&self) -> MobRef {
        MobRef::new(EntityRef::new(WorldEpoch::new(1), 7))
    }
    fn walk_target(&self) -> Option<mod_api::BrainWalkTarget> {
        None
    }
    fn look_target(&self) -> Option<[f64; 3]> {
        None
    }
    fn attack_target(&self) -> Option<LivingRef> {
        None
    }
    fn set_brain_target(&mut self, target: Option<LivingRef>) -> bool {
        self.calls.push(format!(
            "set_brain_target {:?}",
            target.map(EntityRef::from)
        ));
        true
    }
    fn set_walk_target(&mut self, at: [i32; 3], speed: f64, close: i32) {
        if self.panics {
            panic!("the backend broke");
        }
        self.calls
            .push(format!("set_walk_target {at:?} {speed} {close}"));
    }
}

/// Asks `behavior` for `check_extra_start_conditions` on `cx`.
fn check(behavior: &mut dyn Behavior, cx: &mut Cx) -> bool {
    behavior.check_extra_start_conditions(&mut BehaviorCx::new(cx))
}

/// Asks `behavior` for `can_still_use` on `cx`.
fn still(behavior: &mut dyn Behavior, cx: &mut Cx) -> bool {
    behavior.can_still_use(&mut BehaviorCx::new(cx))
}

/// Runs `behavior`'s `hook` (`start`, `tick` or `stop`) on `cx`.
fn run(behavior: &mut dyn Behavior, cx: &mut Cx, hook: &str) {
    let mut context = BehaviorCx::new(cx);
    match hook {
        "start" => behavior.start(&mut context),
        "tick" => behavior.tick(&mut context),
        "stop" => behavior.stop(&mut context),
        other => panic!("no hook {other}"),
    }
}

// --- Answers ---------------------------------------------------------------------------------

#[test]
fn only_a_true_check_starts_the_behavior() {
    let root = tempfile::tempdir().unwrap();
    let source = villager(&[
        behavior(
            "mymod:yes",
            "idle",
            1,
            &[("check_extra_start_conditions", "return true")],
        ),
        behavior(
            "mymod:no",
            "idle",
            2,
            &[("check_extra_start_conditions", "return false")],
        ),
        behavior(
            "mymod:nil",
            "idle",
            3,
            &[("check_extra_start_conditions", "return nil")],
        ),
        behavior(
            "mymod:text",
            "idle",
            4,
            &[("check_extra_start_conditions", "return \"yes\"")],
        ),
        behavior(
            "mymod:none",
            "idle",
            5,
            &[("check_extra_start_conditions", "")],
        ),
    ]);
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "ans", &source)]);
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    let answers: Vec<bool> = brain
        .iter_mut()
        .map(|b| check(b.as_mut(), &mut cx))
        .collect();
    assert_eq!(answers, [true, false, false, false, false]);
}

#[test]
fn can_still_use_keeps_on_nil_and_stops_on_false_a_failure_or_no_handler() {
    let root = tempfile::tempdir().unwrap();
    let source = villager(&[
        behavior("mymod:nil", "idle", 1, &[("can_still_use", "return nil")]),
        behavior("mymod:off", "idle", 2, &[("can_still_use", "return false")]),
        behavior(
            "mymod:boom",
            "idle",
            3,
            &[("can_still_use", "error(\"boom\")")],
        ),
        behavior("mymod:idle", "idle", 4, &[("start", "")]),
    ]);
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "still", &source)]);
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    let answers: Vec<bool> = brain
        .iter_mut()
        .map(|b| still(b.as_mut(), &mut cx))
        .collect();
    assert_eq!(answers, [true, false, false, false]);
}

#[test]
fn every_handler_gets_e_with_the_mob_and_the_ctx() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "args",
        &[
            (
                "check_extra_start_conditions",
                "horizon.log(\"check \" .. type(e.mob) .. \" \" .. type(e.ctx)) return true",
            ),
            (
                "start",
                "horizon.log(\"start \" .. type(e.mob) .. \" \" .. type(e.ctx))",
            ),
            (
                "can_still_use",
                "horizon.log(\"still \" .. type(e.mob) .. \" \" .. type(e.ctx)) return true",
            ),
            (
                "tick",
                "horizon.log(\"tick \" .. type(e.mob) .. \" \" .. type(e.ctx))",
            ),
            (
                "stop",
                "horizon.log(\"stop \" .. type(e.mob) .. \" \" .. type(e.ctx))",
            ),
        ],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    assert!(check(brain[0].as_mut(), &mut cx));
    run(brain[0].as_mut(), &mut cx, "start");
    assert!(still(brain[0].as_mut(), &mut cx));
    run(brain[0].as_mut(), &mut cx, "tick");
    run(brain[0].as_mut(), &mut cx, "stop");
    assert_eq!(
        lines(&mods),
        [
            "check userdata userdata",
            "start userdata userdata",
            "still userdata userdata",
            "tick userdata userdata",
            "stop userdata userdata",
        ]
    );
}

#[test]
fn a_check_that_errors_is_false_and_keeps_the_actions_made_before_the_error() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "errs",
        &[(
            "check_extra_start_conditions",
            "e.ctx:set_walk_target(1, 2, 3, 0.5, 4) error(\"boom\") return true",
        )],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    assert!(!check(brain[0].as_mut(), &mut cx));
    assert_eq!(cx.calls, ["set_walk_target [1, 2, 3] 0.5 4"]);
}

// --- The actions (e.ctx) ---------------------------------------------------------------------

#[test]
fn each_action_reaches_the_backend_with_its_converted_arguments() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "acts",
        &[(
            "check_extra_start_conditions",
            "e.ctx:set_walk_target(1, 2, 3, 0.5, 4) e.ctx:set_brain_target(e.mob) e.ctx:set_brain_target(nil) return true",
        )],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    assert!(check(brain[0].as_mut(), &mut cx));
    assert_eq!(
        cx.calls,
        [
            "set_walk_target [1, 2, 3] 0.5 4",
            "set_brain_target Some(EntityRef { epoch: WorldEpoch(1), id: 7 })",
            "set_brain_target None",
        ]
    );
}

#[test]
fn an_invalid_action_is_refused_by_name_and_nothing_is_made() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "refuse",
        &[(
            "check_extra_start_conditions",
            "local function try(f) local ok, err = pcall(f) if not ok then horizon.log(tostring(err)) end end \
             try(function() e.ctx:set_walk_target(0/0, 0, 0, 1, 0) end) \
             try(function() e.ctx:set_walk_target(1.5, 0, 0, 1, 0) end) \
             try(function() e.ctx:set_walk_target(0, 0, 0, 3.5, 0) end) \
             try(function() e.ctx:set_walk_target(0, 0, 0, 1, 65) end) \
             try(function() e.ctx:set_walk_target(0, 0, 0, 1, 1.5) end) \
             try(function() e.ctx:set_brain_target(\"a mob\") end) \
             e.ctx:set_walk_target(1, 1, 1, 3.0, 64) return true",
        )],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    assert!(check(brain[0].as_mut(), &mut cx));
    let logged = lines(&mods);
    assert_eq!(logged.len(), 6, "got {logged:?}");
    let expected = [
        "expected finite number, got NaN",
        "x 1.5 is not a whole block coordinate",
        "speed 3.5 is outside 0.0 to 3",
        "close 65 is outside 0 to 64",
        "close 1.5 is not a whole block coordinate",
        "must be a MobRef, an EntityRef or nil",
    ];
    for (line, want) in logged.iter().zip(expected) {
        assert!(line.contains(want), "{line} should say {want}");
    }
    assert_eq!(cx.calls, ["set_walk_target [1, 1, 1] 3 64"]);
}

#[test]
fn the_65th_action_is_refused_and_the_64_before_it_stay_applied() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "cap",
        &[(
            "check_extra_start_conditions",
            "local function try(f) local ok, err = pcall(f) if not ok then horizon.log(tostring(err)) end end \
             for i = 1, 65 do try(function() e.ctx:set_brain_target(nil) end) end return true",
        )],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    assert!(check(brain[0].as_mut(), &mut cx));
    let logged = lines(&mods);
    assert_eq!(logged.len(), 1, "got {logged:?}");
    assert!(
        logged[0].contains("at most 64 actions"),
        "got {}",
        logged[0]
    );
    assert_eq!(cx.calls.len(), 64);
}

#[test]
fn a_kept_ctx_used_in_a_later_call_raises() {
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "local saved\n{}",
        villager(&[
            behavior(
                "mymod:keep",
                "idle",
                1,
                &[("check_extra_start_conditions", "saved = e.ctx return true")]
            ),
            behavior(
                "mymod:use",
                "idle",
                2,
                &[(
                    "tick",
                    "local ok, err = pcall(function() saved:set_brain_target(nil) end) horizon.log(tostring(ok) .. \" \" .. tostring(err))",
                )],
            ),
        ])
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "kept", &source)]);
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::default();
    assert!(check(brain[0].as_mut(), &mut cx));
    run(brain[1].as_mut(), &mut cx, "tick");
    assert!(cx.calls.is_empty(), "the stale ctx applies nothing");
    let logged = lines(&mods);
    assert_eq!(logged.len(), 1);
    assert!(logged[0].starts_with("false "), "got {}", logged[0]);
    assert!(
        logged[0].contains("not available outside this behavior call"),
        "got {}",
        logged[0]
    );
}

#[test]
fn a_panicking_backend_is_contained_and_quarantines_the_hook() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "panic_beh",
        &[(
            "check_extra_start_conditions",
            "e.ctx:set_walk_target(0, 0, 0, 1, 0) e.ctx:set_brain_target(nil) return true",
        )],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::panicking();
    let _ = check(brain[0].as_mut(), &mut cx);
    assert!(
        cx.calls.is_empty(),
        "the panicking action ends the call's actions"
    );
    assert!(quarantine::is_quarantined(
        "panic_beh",
        "check_extra_start_conditions"
    ));
    assert_eq!(
        captured_with_prefix("panic_beh: check_extra_start_conditions: the game's host panicked")
            .len(),
        1
    );
}

// --- The shared budget ------------------------------------------------------------------------

#[test]
fn a_spent_budget_refuses_a_check_without_running_lua() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "spent",
        &[(
            "check_extra_start_conditions",
            "horizon.log(\"check\") return true",
        )],
    );
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut brain = brain_on(&mods, budget.clone());
    budget.charge(GOAL_WINDOW_BUDGET);
    assert!(!check(brain[0].as_mut(), &mut Cx::default()));
    assert!(lines(&mods).is_empty(), "the refused check runs no Lua");
}

#[test]
fn a_spent_budget_keeps_can_still_use_and_skips_tick_but_stop_always_runs() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "gated",
        &[
            ("check_extra_start_conditions", "return true"),
            ("can_still_use", "horizon.log(\"still\") return false"),
            ("tick", "horizon.log(\"tick\")"),
            ("stop", "horizon.log(\"stop\")"),
        ],
    );
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut brain = brain_on(&mods, budget.clone());
    let mut cx = Cx::default();
    assert!(check(brain[0].as_mut(), &mut cx));
    budget.charge(GOAL_WINDOW_BUDGET);
    assert!(
        still(brain[0].as_mut(), &mut cx),
        "keeps its last answer, true"
    );
    run(brain[0].as_mut(), &mut cx, "tick");
    run(brain[0].as_mut(), &mut cx, "stop");
    assert_eq!(
        lines(&mods),
        ["stop"],
        "the spent budget runs no still or tick"
    );
}

#[test]
fn stop_is_charged_to_the_budget_even_when_the_budget_is_spent() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(root.path(), "charged", &[("stop", "horizon.log(\"stop\")")]);
    let clock = StepClock::new();
    let budget = Arc::new(Budget::new(clock.clone()));
    let mut brain = brain_on(&mods, budget.clone());
    budget.charge(GOAL_WINDOW_BUDGET);
    let before = budget.spent();
    run(brain[0].as_mut(), &mut Cx::default(), "stop");
    assert!(budget.spent() > before, "stop's time is charged");
    assert_eq!(lines(&mods), ["stop"]);
}

#[test]
fn the_next_window_gives_the_budget_back() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "window",
        &[("check_extra_start_conditions", "return true")],
    );
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut brain = brain_on(&mods, budget.clone());
    budget.charge(GOAL_WINDOW_BUDGET);
    assert!(!check(brain[0].as_mut(), &mut Cx::default()));
    clock.advance(GOAL_WINDOW);
    assert!(check(brain[0].as_mut(), &mut Cx::default()));
}

#[test]
fn a_check_skipped_before_gets_the_reserve_first_in_the_next_window() {
    let root = tempfile::tempdir().unwrap();
    let source = villager(&[
        behavior(
            "mymod:fresh",
            "idle",
            1,
            &[(
                "check_extra_start_conditions",
                "horizon.log(\"fresh\") return true",
            )],
        ),
        behavior(
            "mymod:late",
            "idle",
            2,
            &[(
                "check_extra_start_conditions",
                "horizon.log(\"late\") return true",
            )],
        ),
    ]);
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "fair", &source)]);
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut brain = brain_on(&mods, budget.clone());
    let mut cx = Cx::default();
    // Window one: the budget is spent, so the late behavior is skipped and remembers it.
    budget.charge(GOAL_WINDOW_BUDGET);
    assert!(!check(brain[1].as_mut(), &mut cx));
    // Window two: less than the reserve is left. A behavior never skipped waits; the skipped one runs.
    clock.advance(GOAL_WINDOW);
    budget.charge(GOAL_WINDOW_BUDGET - GOAL_RESERVE + Duration::from_micros(1));
    assert!(
        !check(brain[0].as_mut(), &mut cx),
        "never skipped: waits for the reserve"
    );
    assert!(
        check(brain[1].as_mut(), &mut cx),
        "skipped before: uses the reserve"
    );
    assert_eq!(lines(&mods), ["late"]);
}

/// The game's view of a goal's mob: the backend a `GoalContext` reads and writes. Goals make no writes here.
struct GoalCx {
    host: FakeHost,
    random: LegacyRandom,
}

impl GoalCxBackend for GoalCx {
    fn host(&self) -> &dyn mod_api::Host {
        &self.host
    }
    fn mob(&self) -> MobRef {
        MobRef::new(EntityRef::new(WorldEpoch::new(1), 9))
    }
    fn random(&mut self) -> &mut LegacyRandom {
        &mut self.random
    }
    fn move_to(&mut self, _at: [f64; 3], _speed: f64) -> bool {
        false
    }
    fn move_to_path(&mut self, _path: Vec<PathNode>, _speed: f64) -> Result<(), MoveToPathError> {
        Err(MoveToPathError::Empty)
    }
    fn stop_navigation(&mut self) {}
    fn look_at(&mut self, _at: [f64; 3]) {}
    fn set_target(&mut self, _target: Option<LivingRef>) -> bool {
        false
    }
    fn jump(&mut self) {}
}

/// The game's view of a zombie's goal registrar: it keeps the goals added.
#[derive(Default)]
struct GoalSink {
    goals: Vec<Box<dyn Goal>>,
}

impl GoalRegistrarBackend for GoalSink {
    fn add_goal(&mut self, _priority: i32, goal: Box<dyn Goal>) {
        self.goals.push(goal);
    }
    fn add_target_goal(&mut self, _priority: i32, _goal: Box<dyn Goal>) {}
    fn mob_id(&self) -> &str {
        "minecraft:zombie"
    }
}

#[test]
fn a_behavior_and_a_goal_spend_one_budget() {
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}}}}})\n{}",
        goal_source("mymod:g", "return true"),
        villager(&[behavior(
            "mymod:b",
            "idle",
            1,
            &[(
                "check_extra_start_conditions",
                "horizon.log(\"check\") return true"
            )],
        )]),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "shared", &source)]);
    // One budget object for both: the goal's turns spend it, and the behavior is asked against the same one.
    let budget = Arc::new(Budget::new(StepClock::new()));
    turn_deadline(Some(Duration::from_secs(2)));
    let mut sink = GoalSink::default();
    crate::goal::add_declared_on(
        &mods,
        0,
        "minecraft:zombie",
        &mut GoalRegistrar::new(&mut sink),
        budget.clone(),
    );
    let mut goal_cx = GoalCx {
        host: FakeHost::new(),
        random: LegacyRandom::new(1),
    };
    for _ in 0..4 {
        let mut context = GoalContext::new(&mut goal_cx);
        assert!(sink.goals[0].can_use(&mut context));
    }
    assert!(
        budget.spent() >= GOAL_WINDOW_BUDGET,
        "the goal's turns spent the window"
    );
    let mut brain = brain_on(&mods, budget.clone());
    assert!(
        !check(brain[0].as_mut(), &mut Cx::default()),
        "the window is spent by the goal"
    );
    assert!(!lines(&mods).iter().any(|line| line == "check"));
}

/// A goal table `name` with `can_use` = `body`, as goal_tests writes them.
fn goal_source(name: &str, body: &str) -> String {
    format!("{{name = \"{name}\", priority = 1, can_use = function(e) {body} end}}")
}

// --- Turn deadlines (real interrupts) ---------------------------------------------------------

#[test]
fn an_infinite_loop_in_check_is_stopped_at_the_turn_deadline() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "beh_loop",
        &[(
            "check_extra_start_conditions",
            "e.ctx:set_brain_target(nil) while true do end return true",
        )],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    turn_deadline(None);
    let mut cx = Cx::default();
    let started = Instant::now();
    let answer = check(brain[0].as_mut(), &mut cx);
    let elapsed = started.elapsed();
    eprintln!(
        "behavior turn deadline: an infinite check_extra_start_conditions returned after {elapsed:?}"
    );
    assert!(!answer, "the stopped turn answers no");
    assert!(elapsed < Duration::from_millis(20), "took {elapsed:?}");
    assert!(
        cx.calls.is_empty(),
        "the stopped turn's action is dropped, not applied"
    );
}

#[test]
fn three_budget_failures_in_a_row_quarantine_the_behavior_handler() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "beh_strike",
        &[(
            "check_extra_start_conditions",
            "horizon.log(\"run\") while true do end return true",
        )],
    );
    let clock = ManualClock::new();
    let mut brain = brain_on(&mods, budget_on(&clock));
    turn_deadline(None);
    for _ in 0..3 {
        clock.advance(GOAL_WINDOW);
        assert!(!check(brain[0].as_mut(), &mut Cx::default()));
    }
    assert!(quarantine::is_quarantined(
        "beh_strike",
        &behavior_handler_key("beh_strike:b", "check_extra_start_conditions")
    ));
    let before = lines(&mods).len();
    assert!(!check(brain[0].as_mut(), &mut Cx::default()));
    assert_eq!(
        lines(&mods).len(),
        before,
        "the quarantined handler runs no more"
    );
}

#[test]
fn a_stop_over_the_handler_budget_is_dropped_and_never_a_strike() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_behavior(
        root.path(),
        "beh_stop",
        &[("stop", "e.ctx:set_brain_target(nil) while true do end")],
    );
    let mut brain = brain_on(&mods, budget_on(&ManualClock::new()));
    turn_deadline(None);
    let mut cx = Cx::default();
    run(brain[0].as_mut(), &mut cx, "stop");
    assert!(
        cx.calls.is_empty(),
        "the stopped stop's actions are dropped"
    );
    assert!(!quarantine::is_quarantined(
        "beh_stop",
        &behavior_handler_key("beh_stop:b", "stop")
    ));
}
