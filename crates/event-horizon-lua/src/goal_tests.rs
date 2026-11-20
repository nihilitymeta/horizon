//! Mob goals (#74, B1): what a mod declares in the `goals` table of an entity part, what the game's
//! registration gets, what each goal call answers, and the shared time budget. The budget tests move a
//! clock by hand, so they never sleep; the one real-time test runs 1000 goals on the system clock.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mod_api::testing::FakeHost;
use mod_api::{
    EntityRef, Goal, GoalContext, GoalCxBackend, GoalFlags, GoalRegistrar, GoalRegistrarBackend,
    LegacyRandom, LivingRef, MobRef, MoveToPathError, PathNode, WorldEpoch,
};

use crate::declare::{self, GoalRecord};
use crate::discover::{Discovered, ENTRY, Side};
use crate::goal::add_declared_on;
use crate::goal_budget::{Budget, Clock, GOAL_RESERVE, GOAL_WINDOW, GOAL_WINDOW_BUDGET};
use crate::host::{LuaEntityPart, LuaMod, LuaPart};
use crate::state::new_state;

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

/// The budget on a clock the test moves.
fn budget_on(clock: &Arc<ManualClock>) -> Arc<Budget> {
    Arc::new(Budget::new(clock.clone()))
}

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

/// One goal table: `name`, `priority`, and the handler bodies as `(name, body)` pairs.
fn goal(name: &str, priority: u8, handlers: &[(&str, &str)]) -> String {
    let mut out = format!("{{name = \"{name}\", priority = {priority}");
    for (hook, body) in handlers {
        out.push_str(&format!(", {hook} = function(e) {body} end"));
    }
    out.push('}');
    out
}

/// The mod `mymod`'s goal records, from its declaration.
fn records(source: &str) -> Vec<GoalRecord> {
    declared(source).unwrap().goals
}

/// The game's view of a mob being built: the goal selector's backend, which keeps what was added.
#[derive(Default)]
struct Registrar {
    goals: Vec<(i32, Box<dyn Goal>)>,
}

impl GoalRegistrarBackend for Registrar {
    fn add_goal(&mut self, priority: i32, goal: Box<dyn Goal>) {
        self.goals.push((priority, goal));
    }
    fn add_target_goal(&mut self, _priority: i32, _goal: Box<dyn Goal>) {}
    fn mob_id(&self) -> &str {
        "minecraft:zombie"
    }
}

/// The game's view of a goal's tick: one mob, the fake host, and no writes (every write is refused
/// here, and the Lua goal's own writes raise before they reach it).
struct Cx {
    host: FakeHost,
    mob: MobRef,
    random: LegacyRandom,
    /// The `GoalContext` operations the backend received, in order.
    calls: Vec<String>,
    /// Whether `move_to` panics (a backend that breaks).
    panics: bool,
}

impl Cx {
    fn new() -> Cx {
        Cx {
            host: FakeHost::new(),
            mob: MobRef::new(EntityRef::new(WorldEpoch::new(1), 7)),
            random: LegacyRandom::new(1),
            calls: Vec::new(),
            panics: false,
        }
    }

    /// A backend whose `move_to` panics.
    fn panicking() -> Cx {
        Cx {
            panics: true,
            ..Cx::new()
        }
    }
}

impl GoalCxBackend for Cx {
    fn host(&self) -> &dyn mod_api::Host {
        &self.host
    }
    fn mob(&self) -> MobRef {
        self.mob
    }
    fn random(&mut self) -> &mut LegacyRandom {
        &mut self.random
    }
    fn move_to(&mut self, at: [f64; 3], speed: f64) -> bool {
        if self.panics {
            panic!("the backend broke");
        }
        self.calls.push(format!("move_to {at:?} {speed}"));
        false
    }
    fn move_to_path(&mut self, _path: Vec<PathNode>, _speed: f64) -> Result<(), MoveToPathError> {
        Err(MoveToPathError::Empty)
    }
    fn stop_navigation(&mut self) {
        self.calls.push("stop_navigation".to_owned());
    }
    fn look_at(&mut self, at: [f64; 3]) {
        self.calls.push(format!("look_at {at:?}"));
    }
    fn set_target(&mut self, target: Option<LivingRef>) -> bool {
        self.calls
            .push(format!("set_target {:?}", target.map(EntityRef::from)));
        false
    }
    fn jump(&mut self) {
        self.calls.push("jump".to_owned());
    }
}

/// Sets this test's goal turn deadline (`None`: the real `GOAL_TURN_BUDGET`). Tests that check what a
/// turn answers or applies use a generous one, so a loaded machine cannot time them out; the deadline
/// tests and the tests that loop on purpose use the real one.
fn turn_deadline(limit: Option<Duration>) {
    crate::mob_turn::TURN_BUDGET_OVERRIDE.with(|cell| cell.set(limit));
}

/// The goals of one mob, built from the mod's entity part `index` (the part's `register_goals`).
/// Its turns get a generous deadline (see `turn_deadline`).
fn mob_goals(mods: &Arc<[LuaMod]>, budget: Arc<Budget>) -> Registrar {
    turn_deadline(Some(Duration::from_secs(2)));
    let mut registrar = Registrar::default();
    let mut goals = GoalRegistrar::new(&mut registrar);
    add_declared_on(mods, 0, "minecraft:zombie", &mut goals, budget);
    registrar
}

/// Asks goal `index` of `registrar` for `can_use` on `cx`.
fn can_use(registrar: &mut Registrar, index: usize, cx: &mut Cx) -> bool {
    let mut context = GoalContext::new(cx);
    registrar.goals[index].1.can_use(&mut context)
}

/// Asks goal `index` of `registrar` for `can_continue` on `cx`.
fn can_continue(registrar: &mut Registrar, index: usize, cx: &mut Cx) -> bool {
    let mut context = GoalContext::new(cx);
    registrar.goals[index].1.can_continue(&mut context)
}

/// Runs goal `index` of `registrar`'s `hook` (`start`, `tick` or `stop`) on `cx`.
fn run(registrar: &mut Registrar, index: usize, cx: &mut Cx, hook: &str) {
    let mut context = GoalContext::new(cx);
    let goal = &mut registrar.goals[index].1;
    match hook {
        "start" => goal.start(&mut context),
        "tick" => goal.tick(&mut context),
        "stop" => goal.stop(&mut context),
        other => panic!("no hook {other}"),
    }
}

/// The mod's log lines, as its goals wrote them.
fn lines(mods: &Arc<[LuaMod]>) -> Vec<String> {
    mods[0].log_lines()
}

/// A mod `id` with one goal `mymod:g` on `minecraft:zombie`, at priority 5, whose handlers are
/// `handlers`.
fn one_goal_mod(root: &Path, id: &str, handlers: &[(&str, &str)]) -> LuaMod {
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}}}}})",
        goal("mymod:g", 5, handlers)
    );
    load_one(root, id, &source)
}

/// `one_goal_mod`, shared as the game shares a mod's records.
fn one_goal(root: &Path, id: &str, handlers: &[(&str, &str)]) -> Arc<[LuaMod]> {
    Arc::from(vec![one_goal_mod(root, id, handlers)])
}

// --- Declaration -----------------------------------------------------------------------------

#[test]
fn a_goal_declaration_keeps_its_name_priority_flags_and_handlers() {
    let found = records(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {\
         {name = \"mymod:chase\", priority = 5, flags = {\"move\", \"look\"}, can_use = function() return true end, tick = function() end},\
         {name = \"mymod:wait\", priority = 0, can_use = function() return false end}}})",
    );
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].entity, "minecraft:zombie");
    assert_eq!(found[0].name, "mymod:chase");
    assert_eq!(found[0].priority, 5);
    assert_eq!(found[0].flags, GoalFlags::MOVE.union(GoalFlags::LOOK));
    assert_eq!(found[0].hooks, ["can_use", "tick"]);
    assert_eq!(found[1].flags, GoalFlags::NONE);
    assert_eq!(found[1].hooks, ["can_use"]);
}

#[test]
fn a_part_with_only_goals_is_a_part() {
    let decl = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"mymod:g\", priority = 1, can_use = function() return true end}}})",
    )
    .unwrap();
    assert_eq!(decl.entity_parts.len(), 1);
    assert_eq!(decl.entity_parts[0].id, "minecraft:zombie");
}

#[test]
fn a_part_with_no_handler_and_no_goal_is_refused() {
    let err = declared("horizon.entity_part(\"minecraft:zombie\", {goals = {}})").unwrap_err();
    assert!(
        err.contains("needs at least one handler or goal"),
        "got {err}"
    );
}

#[test]
fn nine_goals_are_refused_naming_the_limit() {
    let goals: Vec<String> = (0..9)
        .map(|i| goal(&format!("mymod:g{i}"), 1, &[("can_use", "return true")]))
        .collect();
    let err = declared(&format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}}}}})",
        goals.join(",")
    ))
    .unwrap_err();
    assert!(err.contains("at most 8 goals"), "got {err}");
}

#[test]
fn eight_goals_are_the_most_a_part_may_declare() {
    let goals: Vec<String> = (0..8)
        .map(|i| goal(&format!("mymod:g{i}"), 1, &[("can_use", "return true")]))
        .collect();
    let decl = declared(&format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}}}}})",
        goals.join(",")
    ))
    .unwrap();
    assert_eq!(decl.goals.len(), 8);
}

#[test]
fn an_unknown_flag_is_refused_by_name() {
    let err = declared(&format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}}}}})",
        goal("mymod:g", 1, &[("can_use", "return true")])
            .replace("priority = 1", "priority = 1, flags = {\"fly\"}")
    ))
    .unwrap_err();
    assert!(err.contains("unknown goal flag 'fly'"), "got {err}");
}

#[test]
fn a_priority_out_of_range_or_not_whole_is_refused() {
    for bad in ["21", "-1", "1.5", "\"five\""] {
        let source = format!(
            "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{{name = \"mymod:g\", priority = {bad}, can_use = function() return true end}}}}}})"
        );
        assert!(declared(&source).is_err(), "priority {bad} must be refused");
    }
    let err = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"mymod:g\", priority = 21, can_use = function() return true end}}})",
    )
    .unwrap_err();
    assert!(err.contains("from 0 to 20"), "got {err}");
}

#[test]
fn an_unknown_goal_key_is_refused_by_name() {
    let err = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"mymod:g\", priority = 1, speed = 2, can_use = function() return true end}}})",
    )
    .unwrap_err();
    assert!(err.contains("unknown goal key 'speed'"), "got {err}");
}

#[test]
fn register_goals_as_a_handler_is_refused_naming_the_goals_table() {
    let err =
        declared("horizon.entity_part(\"minecraft:zombie\", {register_goals = function() end})")
            .unwrap_err();
    assert!(
        err.contains("'register_goals'") && err.contains("goals table"),
        "got {err}"
    );
}

#[test]
fn a_goal_name_must_be_in_the_mods_namespace_and_short_enough() {
    let bare = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"chase\", priority = 1, can_use = function() return true end}}})",
    )
    .unwrap_err();
    assert!(bare.contains("needs a namespaced id"), "got {bare}");
    let foreign = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"other:chase\", priority = 1, can_use = function() return true end}}})",
    )
    .unwrap_err();
    assert!(
        foreign.contains("outside this mod's namespace"),
        "got {foreign}"
    );
    let long = format!("mymod:{}", "a".repeat(60));
    let err = declared(&format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{{name = \"{long}\", priority = 1, can_use = function() return true end}}}}}})"
    ))
    .unwrap_err();
    assert!(err.contains("longer than 64 bytes"), "got {err}");
}

#[test]
fn a_goal_needs_a_name_a_priority_and_can_use() {
    let no_can_use = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"mymod:g\", priority = 1, tick = function() end}}})",
    )
    .unwrap_err();
    assert!(no_can_use.contains("needs can_use"), "got {no_can_use}");
    let no_priority = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"mymod:g\", can_use = function() return true end}}})",
    )
    .unwrap_err();
    assert!(
        no_priority.contains("needs a priority"),
        "got {no_priority}"
    );
}

#[test]
fn a_goal_name_is_declared_once_per_mod() {
    let err = declared(
        "horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"mymod:g\", priority = 1, can_use = function() return true end}}})\n\
         horizon.entity_part(\"minecraft:sheep\", {goals = {{name = \"mymod:g\", priority = 1, can_use = function() return true end}}})",
    )
    .unwrap_err();
    assert!(err.contains("declared twice"), "got {err}");
}

// --- Registration and answers ----------------------------------------------------------------

#[test]
fn register_goals_adds_each_declared_goal_with_its_priority_and_flags() {
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}, {}}}}})",
        goal("mymod:a", 7, &[("can_use", "return true")])
            .replace("priority = 7", "priority = 7, flags = {\"look\"}"),
        goal("mymod:b", 2, &[("can_use", "return true")])
            .replace("priority = 2", "priority = 2, flags = {\"move\", \"jump\"}"),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "mymod", &source)]);
    let registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let seen: Vec<(i32, GoalFlags)> = registrar
        .goals
        .iter()
        .map(|(priority, goal)| (*priority, goal.flags()))
        .collect();
    assert_eq!(
        seen,
        [
            (7, GoalFlags::LOOK),
            (2, GoalFlags::MOVE.union(GoalFlags::JUMP))
        ]
    );
}

#[test]
fn a_mob_of_another_type_gets_none_of_the_goals() {
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![one_goal_mod(
        root.path(),
        "mymod",
        &[("can_use", "return true")],
    )]);
    let mut registrar = Registrar::default();
    let mut goals = GoalRegistrar::new(&mut registrar);
    add_declared_on(
        &mods,
        0,
        "minecraft:sheep",
        &mut goals,
        budget_on(&ManualClock::new()),
    );
    assert!(registrar.goals.is_empty());
}

#[test]
fn can_use_starts_only_on_a_true_answer() {
    let root = tempfile::tempdir().unwrap();
    crate::host::tests::install_capture();
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}, {}, {}, {}}}}})",
        goal("mymod:yes", 1, &[("can_use", "return true")]),
        goal("mymod:no", 1, &[("can_use", "return false")]),
        goal("mymod:silent", 1, &[("can_use", "")]),
        goal("mymod:number", 1, &[("can_use", "return 1")]),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "answers", &source)]);
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    let answers: Vec<bool> = (0..4)
        .map(|i| can_use(&mut registrar, i, &mut cx))
        .collect();
    assert_eq!(answers, [true, false, false, false]);
    assert!(
        !crate::host::tests::captured_with_prefix("answers: can_use").is_empty(),
        "a non-boolean answer is refused with a line"
    );
}

#[test]
fn can_continue_nil_keeps_running_and_false_stops() {
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}, {}}}}})",
        goal(
            "mymod:silent",
            1,
            &[("can_use", "return true"), ("can_continue", "")]
        ),
        goal(
            "mymod:stop",
            1,
            &[("can_use", "return true"), ("can_continue", "return false")]
        ),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "cont", &source)]);
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    assert!(can_use(&mut registrar, 1, &mut cx));
    assert!(
        can_continue(&mut registrar, 0, &mut cx),
        "nil keeps running"
    );
    assert!(!can_continue(&mut registrar, 1, &mut cx), "false stops");
}

#[test]
fn a_goal_without_can_continue_keeps_the_can_use_answer() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(root.path(), "plain", &[("can_use", "return false")]);
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(!can_continue(&mut registrar, 0, &mut cx));
}

#[test]
fn the_handlers_get_e_mob_and_start_tick_and_stop_run() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "hooks",
        &[
            (
                "can_use",
                "horizon.log(\"use \" .. type(e.mob)) return true",
            ),
            ("start", "horizon.log(\"start \" .. type(e.mob))"),
            ("tick", "horizon.log(\"tick \" .. type(e.mob))"),
            ("stop", "horizon.log(\"stop \" .. type(e.mob))"),
        ],
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    run(&mut registrar, 0, &mut cx, "start");
    run(&mut registrar, 0, &mut cx, "tick");
    run(&mut registrar, 0, &mut cx, "stop");
    assert_eq!(
        lines(&mods),
        [
            "use userdata",
            "start userdata",
            "tick userdata",
            "stop userdata"
        ]
    );
}

#[test]
fn a_failing_can_use_is_false_and_a_failing_can_continue_stops() {
    let root = tempfile::tempdir().unwrap();
    crate::host::tests::install_capture();
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}, {}}}}})",
        goal("mymod:bad_use", 1, &[("can_use", "error(\"boom\")")]),
        goal(
            "mymod:bad_cont",
            1,
            &[
                ("can_use", "return true"),
                ("can_continue", "error(\"boom\")")
            ]
        ),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "failing", &source)]);
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(!can_use(&mut registrar, 0, &mut cx));
    assert!(can_use(&mut registrar, 1, &mut cx));
    assert!(!can_continue(&mut registrar, 1, &mut cx));
}

#[test]
fn a_quarantined_goal_answers_no_and_stops_and_runs_nothing() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "quarantined_goal",
        &[
            ("can_use", "horizon.log(\"use\") return true"),
            ("can_continue", "horizon.log(\"cont\") return true"),
            ("tick", "horizon.log(\"tick\")"),
        ],
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    // Retried: a turn on a loaded machine can miss its 1 ms deadline once (see the deadline tests).
    let mut answered = false;
    for _ in 0..5 {
        answered = can_use(&mut registrar, 0, &mut cx);
        if answered {
            break;
        }
    }
    assert!(answered);
    crate::quarantine::quarantine("quarantined_goal", "can_use");
    crate::quarantine::quarantine("quarantined_goal", "can_continue");
    crate::quarantine::quarantine("quarantined_goal", "tick");
    let before = lines(&mods).len();
    assert!(!can_use(&mut registrar, 0, &mut cx));
    assert!(!can_continue(&mut registrar, 0, &mut cx));
    run(&mut registrar, 0, &mut cx, "tick");
    assert_eq!(
        lines(&mods).len(),
        before,
        "no quarantined handler runs again"
    );
}

#[test]
fn a_write_from_a_goal_turn_is_refused() {
    let root = tempfile::tempdir().unwrap();
    crate::host::tests::install_capture();
    let mods = one_goal(
        root.path(),
        "writer",
        &[("can_use", "e.mob:jump() return true")],
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(
        !can_use(&mut registrar, 0, &mut cx),
        "the write raises, so the turn fails"
    );
}

// --- The shared time budget ------------------------------------------------------------------

#[test]
fn a_spent_budget_stops_can_use_without_running_lua() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "spent",
        &[("can_use", "horizon.log(\"ran\") return true")],
    );
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut registrar = mob_goals(&mods, budget.clone());
    budget.charge(GOAL_WINDOW_BUDGET);
    let mut cx = Cx::new();
    assert!(!can_use(&mut registrar, 0, &mut cx));
    assert!(lines(&mods).is_empty(), "no Lua ran");
}

#[test]
fn a_spent_budget_keeps_can_continue_at_its_last_answer_and_never_stops_a_running_goal() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "continuing",
        &[
            ("can_use", "return true"),
            ("can_continue", "horizon.log(\"ran\") return false"),
        ],
    );
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut registrar = mob_goals(&mods, budget.clone());
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    budget.charge(GOAL_WINDOW_BUDGET);
    assert!(
        can_continue(&mut registrar, 0, &mut cx),
        "the last answer, true"
    );
    assert!(lines(&mods).is_empty());
}

#[test]
fn a_spent_budget_skips_tick_but_stop_always_runs_and_is_charged() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "release",
        &[
            ("can_use", "return true"),
            ("tick", "horizon.log(\"tick\")"),
            ("stop", "horizon.log(\"stop\")"),
        ],
    );
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut registrar = mob_goals(&mods, budget.clone());
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    budget.charge(GOAL_WINDOW_BUDGET);
    run(&mut registrar, 0, &mut cx, "tick");
    run(&mut registrar, 0, &mut cx, "stop");
    assert_eq!(lines(&mods), ["stop"]);
    assert!(budget.spent() >= GOAL_WINDOW_BUDGET);
}

#[test]
fn the_next_window_gives_the_budget_back() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(root.path(), "next_window", &[("can_use", "return true")]);
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut registrar = mob_goals(&mods, budget.clone());
    let mut cx = Cx::new();
    budget.charge(GOAL_WINDOW_BUDGET);
    assert!(!can_use(&mut registrar, 0, &mut cx));
    clock.advance(GOAL_WINDOW);
    assert!(can_use(&mut registrar, 0, &mut cx));
}

#[test]
fn a_goal_skipped_before_gets_the_reserve_first_in_the_next_window() {
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{}, {}}}}})",
        goal(
            "mymod:fresh",
            1,
            &[("can_use", "horizon.log(\"fresh\") return true")]
        ),
        goal(
            "mymod:late",
            1,
            &[("can_use", "horizon.log(\"late\") return true")]
        ),
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "fair", &source)]);
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut fresh_mob = mob_goals(&mods, budget.clone());
    let mut cx = Cx::new();
    // Window one: the budget is spent, so the late goal (index 1) is skipped and remembers it.
    budget.charge(GOAL_WINDOW_BUDGET);
    assert!(!can_use(&mut fresh_mob, 1, &mut cx));
    // Window two: less than the reserve is left. A goal never skipped waits; the skipped one runs.
    clock.advance(GOAL_WINDOW);
    budget.charge(GOAL_WINDOW_BUDGET - GOAL_RESERVE + Duration::from_micros(1));
    assert!(
        !can_use(&mut fresh_mob, 0, &mut cx),
        "never skipped: waits for the reserve"
    );
    assert!(
        can_use(&mut fresh_mob, 1, &mut cx),
        "skipped before: uses the reserve"
    );
    assert_eq!(lines(&mods), ["late"]);
}

#[test]
fn an_overrun_is_warned_once_per_mod_not_per_skip() {
    let root = tempfile::tempdir().unwrap();
    crate::host::tests::install_capture();
    let mods = one_goal(root.path(), "warned_budget", &[("can_use", "return true")]);
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut registrar = mob_goals(&mods, budget.clone());
    let mut cx = Cx::new();
    budget.charge(GOAL_WINDOW_BUDGET);
    for _ in 0..5 {
        assert!(!can_use(&mut registrar, 0, &mut cx));
    }
    assert_eq!(
        crate::host::tests::captured_with_prefix(
            "warned_budget: goals: over the shared time budget"
        ),
        ["warned_budget: goals: over the shared time budget; some goals were skipped"]
    );
}

#[test]
fn a_thousand_goals_in_one_window_stay_within_the_budget_on_the_real_clock() {
    let root = tempfile::tempdir().unwrap();
    // Some tens of microseconds of Lua per turn, so the window's budget runs out part way.
    let mods = one_goal(
        root.path(),
        "thousand",
        &[(
            "can_use",
            "local s = 0 for i = 1, 2000 do s = s + i end return s > 0",
        )],
    );
    // The shared budget may hold spend from other tests in this process, so the real-clock check
    // runs on its own budget, on the system clock.
    let own = Arc::new(Budget::new(Arc::new(crate::goal_budget::SystemClock)));
    let mut worst_spent = Duration::ZERO;
    let mut worst_turn = Duration::ZERO;
    let mut admitted = 0_usize;
    let mut refused = 0_usize;
    let mut cx = Cx::new();
    for _ in 0..1000 {
        let mut registrar = mob_goals(&mods, own.clone());
        let started = Instant::now();
        if can_use(&mut registrar, 0, &mut cx) {
            admitted += 1;
            worst_turn = worst_turn.max(started.elapsed());
        } else {
            refused += 1;
        }
        worst_spent = worst_spent.max(own.spent());
    }
    let bound = GOAL_WINDOW_BUDGET + worst_turn + Duration::from_millis(1);
    eprintln!(
        "goal budget, real clock: admitted {admitted}, refused {refused}, worst window spend {worst_spent:?}, worst turn {worst_turn:?}, bound {bound:?}"
    );
    assert!(
        admitted > 0 && refused > 0,
        "the loop should both run and hit the budget"
    );
    assert!(
        worst_spent <= bound,
        "spent {worst_spent:?} in one window, over the bound {bound:?}"
    );
}

// --- Quarantine on a panic, at the dispatch level --------------------------------------------

#[test]
fn a_panicking_goal_build_fails_the_turn_and_quarantines_the_hook() {
    let root = tempfile::tempdir().unwrap();
    crate::host::tests::install_capture();
    let mods = one_goal(root.path(), "panic_goal", &[("can_use", "return true")]);
    let failed = std::cell::Cell::new(false);
    let build = |_: &mlua::Lua,
                 _: &crate::hostcall::TurnHost<'_>|
     -> mlua::Result<crate::dispatch::Event> { panic!("the goal build broke") };
    crate::dispatch::replace_in_reporting(
        &mods,
        &[0],
        "can_use",
        &declare::goal_handler_key("panic_goal:g", "can_use"),
        crate::hostcall::TurnHost::None,
        &build,
        &mut |_, _, _| true,
        &failed,
    )
    .unwrap();
    assert!(
        failed.get(),
        "the turn failed, so a can_continue would stop"
    );
    assert!(crate::quarantine::is_quarantined("panic_goal", "can_use"));
}

/// The part of a loaded entity part, for the generated `register_goals` route: the part's
/// `declared_goals` adds the goals of its type.
#[test]
fn the_entity_part_registers_its_declared_goals() {
    let root = tempfile::tempdir().unwrap();
    let mods: Arc<[LuaMod]> = Arc::from(vec![one_goal_mod(
        root.path(),
        "part_goals",
        &[("can_use", "return true")],
    )]);
    let part: LuaEntityPart = LuaPart::new(mods.clone(), 0, 0).unwrap();
    let mut registrar = Registrar::default();
    let mut goals = GoalRegistrar::new(&mut registrar);
    mod_api::EntityPart::register_goals(&part, &mut goals);
    assert_eq!(registrar.goals.len(), 1);
    assert_eq!(registrar.goals[0].0, 5);
}

// --- The goal actions (e.ctx) ----------------------------------------------------------------

/// A goal `mymod:g` on the zombie, with `can_use` = `body` (its last statement returns the answer).
fn acting(root: &Path, id: &str, body: &str) -> Arc<[LuaMod]> {
    one_goal(root, id, &[("can_use", body)])
}

#[test]
fn each_action_reaches_the_backend_with_its_converted_arguments() {
    let root = tempfile::tempdir().unwrap();
    let mods = acting(
        root.path(),
        "acts",
        "e.ctx:move_to(1, 2, 3, 0.5) e.ctx:look_at(4, 5, 6) e.ctx:stop_navigation() \
         e.ctx:set_target(e.mob) e.ctx:set_target(nil) e.ctx:jump() return true",
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    assert_eq!(
        cx.calls,
        [
            "move_to [1.0, 2.0, 3.0] 0.5",
            "look_at [4.0, 5.0, 6.0]",
            "stop_navigation",
            "set_target Some(EntityRef { epoch: WorldEpoch(1), id: 7 })",
            "set_target None",
            "jump",
        ]
    );
}

#[test]
fn an_action_is_refused_when_it_is_not_a_valid_number_or_target() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = acting(
        root.path(),
        "refuse",
        "local function try(f) local ok, err = pcall(f) if not ok then horizon.log(tostring(err)) end end \
         try(function() e.ctx:move_to(0/0, 0, 0, 1) end) \
         try(function() e.ctx:move_to(0, math.huge, 0, 1) end) \
         try(function() e.ctx:move_to(0, 0, 0, 0/0) end) \
         try(function() e.ctx:move_to(0, 0, 0, 3.5) end) \
         try(function() e.ctx:move_to(0, 0, 0, -1) end) \
         try(function() e.ctx:look_at(0, 0, -math.huge) end) \
         try(function() e.ctx:set_target(\"a mob\") end) \
         e.ctx:move_to(1, 1, 1, 3.0) return true",
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    let logged = lines(&mods);
    assert_eq!(logged.len(), 7, "got {logged:?}");
    assert!(
        logged[0].contains("argument 1: expected finite number, got NaN"),
        "got {}",
        logged[0]
    );
    assert!(
        logged[1].contains("argument 2: expected finite number, got inf"),
        "got {}",
        logged[1]
    );
    assert!(
        logged[2].contains("argument 4: expected finite number, got NaN"),
        "got {}",
        logged[2]
    );
    assert!(
        logged[3].contains("speed 3.5 is outside 0.0 to 3"),
        "got {}",
        logged[3]
    );
    assert!(
        logged[4].contains("speed -1 is outside 0.0 to 3"),
        "got {}",
        logged[4]
    );
    assert!(
        logged[5].contains("argument 3: expected finite number, got -inf"),
        "got {}",
        logged[5]
    );
    assert!(
        logged[6].contains("argument 1: must be a MobRef, an EntityRef or nil"),
        "got {}",
        logged[6]
    );
    assert_eq!(
        cx.calls,
        ["move_to [1.0, 1.0, 1.0] 3"],
        "only the valid action reaches the backend"
    );
}

#[test]
fn the_65th_action_of_a_call_is_refused_and_the_64_before_it_stay_applied() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = acting(
        root.path(),
        "many",
        "for i = 1, 65 do e.ctx:jump() end return true",
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(
        !can_use(&mut registrar, 0, &mut cx),
        "the error fails the turn"
    );
    assert_eq!(cx.calls.len(), 64);
    assert_eq!(
        crate::host::tests::captured_with_prefix("many: can_use: a handler failed"),
        ["many: can_use: a handler failed, so this mod gives no answer"]
    );
    assert!(!crate::host::tests::captured_with_prefix("many: can_use").is_empty());
}

#[test]
fn a_kept_ctx_used_in_a_later_call_raises() {
    let root = tempfile::tempdir().unwrap();
    let source = "local saved\n\
         horizon.entity_part(\"minecraft:zombie\", {goals = {\
         {name = \"mymod:keep\", priority = 1, can_use = function(e) saved = e.ctx return true end},\
         {name = \"mymod:use\", priority = 2, can_use = function(e) local ok, err = pcall(function() saved:jump() end) horizon.log(tostring(ok) .. \" \" .. tostring(err)) return true end}}})";
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "kept", source)]);
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    assert!(can_use(&mut registrar, 1, &mut cx));
    assert!(cx.calls.is_empty(), "the stale ctx applies nothing");
    let logged = lines(&mods);
    assert_eq!(logged.len(), 1);
    assert!(logged[0].starts_with("false "), "got {}", logged[0]);
    assert!(
        logged[0].contains("not available outside this goal call"),
        "got {}",
        logged[0]
    );
}

#[test]
fn a_ctx_used_by_a_hook_that_is_not_a_goal_raises() {
    let root = tempfile::tempdir().unwrap();
    let source = "local saved\n\
         horizon.entity_part(\"minecraft:zombie\", {goals = {\
         {name = \"mymod:keep\", priority = 1, can_use = function(e) saved = e.ctx return true end}},\
         is_shearable = function(e) local ok, err = pcall(function() saved:jump() end) horizon.log(tostring(ok) .. \" \" .. tostring(err)) return nil end})";
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "hooked", source)]);
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    // Each turn is timed against GOAL_TURN_BUDGET in wall clock, so a turn on a loaded machine can miss
    // it once: retry until one turn answers, so the test checks the hook, not the scheduler.
    let mut answered = false;
    for _ in 0..5 {
        answered = can_use(&mut registrar, 0, &mut cx);
        if answered {
            break;
        }
    }
    assert!(answered);
    // The is_shearable hook runs in a turn that is not a goal call, with the same mod's state.
    let key = declare::part_handler_key(
        crate::declare::PartKind::Entity,
        "minecraft:zombie",
        "is_shearable",
    );
    let host = FakeHost::new();
    let build = |lua: &mlua::Lua, _: &crate::hostcall::TurnHost<'_>| {
        crate::dispatch::new_event(lua, lua.create_table()?, &[])
    };
    crate::dispatch::replace_in(
        &mods,
        &[0],
        "is_shearable",
        &key,
        crate::hostcall::TurnHost::Read(&host),
        &build,
        &mut |_, _, _| true,
    )
    .unwrap();
    assert!(cx.calls.is_empty());
    let logged = lines(&mods);
    assert_eq!(logged.len(), 1, "got {logged:?}");
    assert!(
        logged[0].contains("not available outside this goal call"),
        "got {}",
        logged[0]
    );
}

#[test]
fn a_handler_error_keeps_the_actions_already_made() {
    let root = tempfile::tempdir().unwrap();
    let mods = acting(root.path(), "errs", "e.ctx:jump() error(\"boom\")");
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    assert!(!can_use(&mut registrar, 0, &mut cx));
    assert_eq!(
        cx.calls,
        ["jump"],
        "the action made before the error stays applied"
    );
}

#[test]
fn a_panicking_backend_is_contained_and_quarantines_the_hook() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = acting(
        root.path(),
        "panic_actions",
        "e.ctx:move_to(0, 0, 0, 1) e.ctx:jump() return true",
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::panicking();
    let _ = can_use(&mut registrar, 0, &mut cx);
    assert!(
        cx.calls.is_empty(),
        "the panicking action ends the call's actions"
    );
    assert!(crate::quarantine::is_quarantined(
        "panic_actions",
        "can_use"
    ));
    assert_eq!(
        crate::host::tests::captured_with_prefix(
            "panic_actions: can_use: the game's host panicked"
        )
        .len(),
        1
    );
}

#[test]
fn an_exhausted_budget_stops_a_tick_and_its_actions_with_it() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "budget_acts",
        &[("can_use", "return true"), ("tick", "e.ctx:jump()")],
    );
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let mut registrar = mob_goals(&mods, budget.clone());
    let mut cx = Cx::new();
    assert!(can_use(&mut registrar, 0, &mut cx));
    budget.charge(GOAL_WINDOW_BUDGET);
    run(&mut registrar, 0, &mut cx, "tick");
    assert!(
        cx.calls.is_empty(),
        "the tick is skipped, so its action is never made"
    );
}

#[test]
fn a_goal_reports_its_namespaced_name() {
    let root = tempfile::tempdir().unwrap();
    let mods = acting(root.path(), "named", "return true");
    let registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    assert_eq!(registrar.goals[0].1.name(), "named:g");
}

// --- Goal turn deadlines (real interrupts: the fake clock cannot stop Lua) -------------------

#[test]
fn an_infinite_loop_in_can_use_is_stopped_at_the_goal_turn_deadline() {
    let root = tempfile::tempdir().unwrap();
    let mods = acting(
        root.path(),
        "spin",
        "e.ctx:jump() while true do end return true",
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    turn_deadline(None);
    let mut cx = Cx::new();
    let started = Instant::now();
    let answer = can_use(&mut registrar, 0, &mut cx);
    let elapsed = started.elapsed();
    eprintln!("goal turn deadline: an infinite can_use returned after {elapsed:?}");
    assert!(!answer, "the stopped turn answers no");
    assert!(elapsed < Duration::from_millis(20), "took {elapsed:?}");
    assert!(
        cx.calls.is_empty(),
        "the stopped turn's action is dropped, not applied"
    );
}

#[test]
fn three_budget_failures_in_a_row_quarantine_the_goal_handler() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = acting(
        root.path(),
        "strike",
        "horizon.log(\"run\") while true do end return true",
    );
    let clock = ManualClock::new();
    let mut registrar = mob_goals(&mods, budget_on(&clock));
    turn_deadline(None);
    let mut cx = Cx::new();
    for _ in 0..3 {
        clock.advance(GOAL_WINDOW);
        assert!(!can_use(&mut registrar, 0, &mut cx));
    }
    assert!(crate::quarantine::is_quarantined(
        "strike",
        "goal strike:g can_use"
    ));
    clock.advance(GOAL_WINDOW);
    assert!(!can_use(&mut registrar, 0, &mut cx));
    assert_eq!(
        lines(&mods),
        ["run", "run", "run"],
        "a quarantined goal runs no Lua"
    );
    assert_eq!(
        crate::host::tests::captured_with_prefix("strike: strike:g: can_use went over").len(),
        1,
        "one warning, when the goal is quarantined"
    );
}

#[test]
fn a_success_between_budget_failures_resets_the_strikes() {
    let root = tempfile::tempdir().unwrap();
    let source = "local n = 0\n\
        horizon.entity_part(\"minecraft:zombie\", {goals = {{name = \"reset:g\", priority = 1, \
        can_use = function(e) n = n + 1 if n == 1 or n == 2 or n == 4 or n == 5 then while true do end end return true end}}})";
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "reset", source)]);
    let clock = ManualClock::new();
    let mut registrar = mob_goals(&mods, budget_on(&clock));
    turn_deadline(None);
    let mut cx = Cx::new();
    let mut answers = Vec::new();
    for _ in 0..6 {
        clock.advance(GOAL_WINDOW);
        answers.push(can_use(&mut registrar, 0, &mut cx));
    }
    assert_eq!(answers, [false, false, true, false, false, true]);
    assert!(!crate::quarantine::is_quarantined(
        "reset",
        "goal reset:g can_use"
    ));
}

#[test]
fn a_quarantined_running_goal_gets_its_stop_exactly_once() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "stopq",
        &[
            (
                "can_use",
                "horizon.log(\"use\") while true do end return true",
            ),
            ("stop", "horizon.log(\"stop\")"),
        ],
    );
    let clock = ManualClock::new();
    let mut registrar = mob_goals(&mods, budget_on(&clock));
    turn_deadline(None);
    let mut cx = Cx::new();
    for _ in 0..3 {
        clock.advance(GOAL_WINDOW);
        assert!(!can_use(&mut registrar, 0, &mut cx));
    }
    assert!(
        !can_continue(&mut registrar, 0, &mut cx),
        "the quarantined goal stops"
    );
    run(&mut registrar, 0, &mut cx, "stop");
    let logged = lines(&mods);
    assert_eq!(
        logged.iter().filter(|line| *line == "stop").count(),
        1,
        "got {logged:?}"
    );
    // A turn that misses its deadline before its first log line adds no "use" line, but still a strike.
    let uses = logged.iter().filter(|line| *line == "use").count();
    assert!((1..=3).contains(&uses), "got {logged:?}");
}

#[test]
fn a_stop_that_runs_over_the_handler_budget_is_dropped_without_a_strike() {
    let root = tempfile::tempdir().unwrap();
    let mods = one_goal(
        root.path(),
        "stopt",
        &[
            ("can_use", "return true"),
            ("stop", "e.ctx:jump() while true do end"),
        ],
    );
    let mut registrar = mob_goals(&mods, budget_on(&ManualClock::new()));
    let mut cx = Cx::new();
    let started = Instant::now();
    run(&mut registrar, 0, &mut cx, "stop");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "stopped by the handler budget: {elapsed:?}"
    );
    assert!(
        cx.calls.is_empty(),
        "a stop stopped by its budget drops its actions"
    );
    assert!(!crate::quarantine::is_quarantined(
        "stopt",
        "goal stopt:g stop"
    ));
}

#[test]
fn a_looping_goal_over_many_mobs_is_quarantined_after_its_strikes() {
    let root = tempfile::tempdir().unwrap();
    let mods = acting(root.path(), "loopers", "while true do end");
    let clock = ManualClock::new();
    let budget = budget_on(&clock);
    let started = Instant::now();
    let mut answered_true = 0;
    for _ in 0..50 {
        let mut registrar = mob_goals(&mods, budget.clone());
        turn_deadline(None);
        let mut cx = Cx::new();
        if can_use(&mut registrar, 0, &mut cx) {
            answered_true += 1;
        }
    }
    let elapsed = started.elapsed();
    eprintln!("goal turn deadline: 50 mobs, one looping goal: {elapsed:?}");
    assert_eq!(answered_true, 0);
    assert!(crate::quarantine::is_quarantined(
        "loopers",
        "goal loopers:g can_use"
    ));
    assert!(elapsed < Duration::from_millis(100), "took {elapsed:?}");
}

#[test]
fn a_normal_goal_beside_a_looping_one_keeps_working() {
    let root = tempfile::tempdir().unwrap();
    let source = "horizon.entity_part(\"minecraft:zombie\", {goals = {\
        {name = \"sib:bad\", priority = 1, can_use = function(e) while true do end end},\
        {name = \"sib:good\", priority = 2, can_use = function(e) return true end}}})";
    let mods: Arc<[LuaMod]> = Arc::from(vec![load_one(root.path(), "sib", source)]);
    let clock = ManualClock::new();
    let mut registrar = mob_goals(&mods, budget_on(&clock));
    turn_deadline(None);
    let mut cx = Cx::new();
    assert!(!can_use(&mut registrar, 0, &mut cx));
    assert!(
        can_use(&mut registrar, 1, &mut cx),
        "the good goal still starts in the same window"
    );
    assert!(!crate::quarantine::is_quarantined(
        "sib",
        "goal sib:good can_use"
    ));
}
