//! Mob goals a mod declares (#74, B1): the `goals` table of an entity part adds one [`LuaGoal`] per
//! declared goal to each mob of that type, at the mob's construction. Declaring a goal runs no Lua;
//! each of the goal's calls is one Lua turn of the goal's own handler, under the shared time budget of
//! `goal_budget`, on the turn runner of `mob_turn`.
//!
//! What a handler gets: one argument `e` with `e.mob`, the mob as a `MobRef`. Its turn reads through
//! the tick's view (the mob itself, the player, the tick's snapshot); its writes raise, because a goal
//! has no server host in its turn (see `QUESTIONS.md` for the gap). A goal keeps its state in its own
//! Lua upvalues, keyed by `e.mob:entity()` if it needs a table per mob: that table is not saved.
//!
//! Answers: `can_use` is true only for a boolean `true`; nil or any other value is false. `can_continue`
//! is kept as a boolean; nil keeps the goal running; a failed turn (an error, a budget, a panic) stops
//! it. A quarantined goal (see `quarantine`) answers `can_use` false, `can_continue` false and runs no
//! other handler. `start`, `tick` and `stop` answer nothing.

use std::cell::Cell;
use std::sync::Arc;

use mod_api::{Goal, GoalContext, GoalFlags, GoalRegistrar};

use crate::declare::{GoalRecord, goal_handler_key};
use crate::dispatch;
use crate::goal_actions::{self, GoalCtx};
use crate::goal_budget::Budget;
use crate::host::{LuaMod, handler_budget, warn_once};
use crate::hostcall::TurnHost;
use crate::mob_turn::{self, Handler, STOP};

/// The message of a goal turn stopped by its own deadline.
const GOAL_TURN_MESSAGE: &str = "the goal's turn ran over its time budget";

/// Adds the goals `mods[index]` declared for the entity type `entity` to the mob being built, on the
/// process's shared budget.
pub(crate) fn add_declared(
    mods: &Arc<[LuaMod]>,
    index: usize,
    entity: &str,
    goals: &mut GoalRegistrar<'_>,
) {
    add_declared_on(mods, index, entity, goals, Budget::shared());
}

/// `add_declared`, on the budget `budget` (the tests give their own, on a clock they move).
pub(crate) fn add_declared_on(
    mods: &Arc<[LuaMod]>,
    index: usize,
    entity: &str,
    goals: &mut GoalRegistrar<'_>,
    budget: Arc<Budget>,
) {
    for (goal, record) in mods[index].goal_records().iter().enumerate() {
        if record.entity == entity {
            goals.add_goal(
                record.priority,
                LuaGoal {
                    mods: Arc::clone(mods),
                    index,
                    goal,
                    name: mob_turn::intern(&record.name),
                    flags: record.flags,
                    budget: Arc::clone(&budget),
                    last: true,
                    skipped: false,
                },
            );
        }
    }
}

/// One goal of a mod on one mob: a handle onto the goal's record in its mod, the mob's last answer to
/// `can_continue`, and whether its last `can_use` was skipped by the budget. Each call is one turn.
pub struct LuaGoal {
    mods: Arc<[LuaMod]>,
    /// The goal's mod, an index into `mods`.
    index: usize,
    /// The goal's record, an index into the mod's goal records.
    goal: usize,
    /// The goal's name in its mod's namespace.
    name: &'static str,
    flags: GoalFlags,
    budget: Arc<Budget>,
    /// The answer `can_continue` last gave (`can_use` true sets it).
    last: bool,
    /// Whether the last `can_use` was skipped by the budget.
    skipped: bool,
}

impl LuaGoal {
    fn module(&self) -> &LuaMod {
        &self.mods[self.index]
    }

    fn record(&self) -> &GoalRecord {
        &self.module().goal_records()[self.goal]
    }

    fn has(&self, hook: &str) -> bool {
        self.record().hooks.iter().any(|name| name == hook)
    }

    /// The handler `hook` of this goal, as the turn runner records it.
    fn handler<'a>(&'a self, hook: &'a str) -> Handler<'a> {
        let record = self.record();
        Handler {
            module: self.module(),
            label: &record.name,
            noun: "goal",
            hook,
            key: goal_handler_key(&record.name, hook),
        }
    }

    /// Runs the `can_use` or `can_continue` handler as one replace turn on the goal's own deadline, then
    /// applies the actions the call queued (none, when the turn was dropped). Says whether the turn
    /// failed (so a caller can tell a failure from a handler that returned nothing).
    fn ask(&self, hook: &str, cx: &mut GoalContext<'_>) -> (Option<bool>, bool) {
        let handler = self.handler(hook);
        if handler.quarantined() {
            return (None, true);
        }
        let mob = cx.mob();
        let call = goal_actions::next_call();
        let mut actions = Vec::new();
        let failed = Cell::new(false);
        let mut answer = None;
        let build = mob_turn::event(mob, GoalCtx::userdata);
        let dropped = mob_turn::run_turn(
            &handler,
            mob_turn::turn_budget(),
            GOAL_TURN_MESSAGE,
            &failed,
            &mut actions,
            |actions| {
                dispatch::replace_in_reporting(
                    &self.mods,
                    &[self.index],
                    hook,
                    &handler.key,
                    TurnHost::Goal {
                        host: cx.host(),
                        actions,
                        call,
                    },
                    &build,
                    &mut |id, value, _turn| match dispatch::answer_of::<bool>(id, hook, &value) {
                        Some(found) => {
                            answer = Some(found);
                            true
                        }
                        None => false,
                    },
                    &failed,
                )
            },
        );
        if dropped {
            answer = None;
        }
        goal_actions::apply(cx, actions, self.module().id(), hook);
        (answer, failed.get())
    }

    /// Runs the `tick`, `start` or `stop` handler as one turn, applies the actions it queued, and
    /// charges the time to the budget. `tick` and `start` run on the goal's own deadline and count
    /// strikes; `stop` runs whatever the quarantine, under the handler budget, and a `stop` stopped by
    /// that budget is dropped with its actions (no strike: the goal is leaving anyway).
    fn run(&self, hook: &str, cx: &mut GoalContext<'_>) {
        if !self.has(hook) {
            return;
        }
        let handler = self.handler(hook);
        if hook != STOP && handler.quarantined() {
            return;
        }
        let mob = cx.mob();
        let call = goal_actions::next_call();
        let mut actions = Vec::new();
        let failed = Cell::new(false);
        let started = self.budget.now();
        let build = mob_turn::event(mob, GoalCtx::userdata);
        let (limit, message) = if hook == STOP {
            (handler_budget(), "the handler ran over its time budget")
        } else {
            (mob_turn::turn_budget(), GOAL_TURN_MESSAGE)
        };
        mob_turn::run_turn(&handler, limit, message, &failed, &mut actions, |actions| {
            dispatch::turns_in_reporting(
                &self.mods,
                &[self.index],
                hook,
                &handler.key,
                TurnHost::Goal {
                    host: cx.host(),
                    actions,
                    call,
                },
                &|_| true,
                &|| false,
                &build,
                &mut |_, _, _| true,
                &failed,
            )
        });
        goal_actions::apply(cx, actions, self.module().id(), hook);
        self.budget
            .charge(self.budget.now().saturating_duration_since(started));
    }
}

impl Goal for LuaGoal {
    fn flags(&self) -> GoalFlags {
        self.flags
    }

    fn can_use(&mut self, cx: &mut GoalContext<'_>) -> bool {
        if !self.budget.may_start(self.skipped) {
            self.skipped = true;
            warn_once(
                self.module().id(),
                &format!(
                    "{}: goals: over the shared time budget; some goals were skipped",
                    self.module().id()
                ),
            );
            return false;
        }
        self.skipped = false;
        let started = self.budget.now();
        let (answer, _failed) = self.ask("can_use", cx);
        self.budget
            .charge(self.budget.now().saturating_duration_since(started));
        self.last = answer.unwrap_or(false);
        self.last
    }

    fn can_continue(&mut self, cx: &mut GoalContext<'_>) -> bool {
        if !self.has("can_continue") {
            // No handler of its own: the trait's default, can_use's answer.
            return self.can_use(cx);
        }
        // A panic quarantines the raw hook name (contained in hostcall), so this guard checks that name.
        if crate::quarantine::is_quarantined(self.module().id(), "can_continue") {
            return false;
        }
        if !self.budget.may_run() {
            return self.last;
        }
        let started = self.budget.now();
        let (answer, failed) = self.ask("can_continue", cx);
        self.budget
            .charge(self.budget.now().saturating_duration_since(started));
        // A nil answer keeps the goal running; a failed turn stops it, so a broken goal releases its flags.
        self.last = answer.unwrap_or(!failed);
        self.last
    }

    fn start(&mut self, cx: &mut GoalContext<'_>) {
        // Not budget-gated: it only follows a `can_use` that answered true.
        self.run("start", cx);
    }

    fn tick(&mut self, cx: &mut GoalContext<'_>) {
        if self.budget.may_run() {
            self.run("tick", cx);
        }
    }

    fn stop(&mut self, cx: &mut GoalContext<'_>) {
        // Always runs, so a goal can release its locks; its time is charged but never refused.
        self.run(STOP, cx);
    }

    fn name(&self) -> &'static str {
        self.name
    }
}
