//! Brain behaviors a mod declares (#74, B2): the `behaviors` table of an entity part adds one [`LuaBehavior`]
//! per declared behavior to each brain of that type, when the brain is built (the generated
//! `register_behaviors` calls `add_declared`). Declaring a behavior runs no Lua, and building a brain runs
//! none either: a `LuaBehavior` is a record until the brain asks it something.
//!
//! What a handler gets: `e` with `e.mob` (the MobRef) and `e.ctx` (BehaviorCtx: `set_walk_target` and
//! `set_brain_target`, see behavior_actions.rs). Its reads go through the mob's handle, as a goal's do.
//!
//! Answers: `check_extra_start_conditions` is true only for a boolean `true`; nil or any other value is false,
//! and a behavior with no such handler starts (the trait's default). `can_still_use` keeps the behavior running
//! on nil, stops it on false or a failed turn, and answers false when it has no handler (vanilla's default).
//! `start`, `tick` and `stop` answer nothing. A quarantined handler answers as a failed one.
//!
//! Budget (the same `Budget` as goals, goal_budget.rs): `check_extra_start_conditions` is gated like a goal's
//! `can_use` (its reserve rule and skipped flag), `can_still_use` like `can_continue` (it keeps its last
//! answer while the budget is spent), `tick` is skipped while the budget is spent, `start` is not gated (it
//! follows a check that answered true), and `stop` always runs, charged and exempt. Each turn runs under
//! `GOAL_TURN_BUDGET` (1 ms) except `stop`, which runs under the handler budget. Budget failures and strikes
//! are settled by `mob_turn::Handler`, keyed `behavior <name> <hook>`.

use std::cell::Cell;
use std::sync::Arc;

use mod_api::{Behavior, BehaviorCx, BehaviorRegistrar};

use crate::behavior_actions::{self, BehaviorCtx};
use crate::declare::{BehaviorRecord, behavior_handler_key};
use crate::dispatch;
use crate::goal_actions;
use crate::goal_budget::Budget;
use crate::host::{LuaMod, handler_budget, warn_once};
use crate::hostcall::TurnHost;
use crate::mob_turn::{self, Handler, STOP};

/// The message of a behavior turn stopped by its own deadline.
const BEHAVIOR_TURN_MESSAGE: &str = "the behavior's turn ran over its time budget";

const CHECK: &str = "check_extra_start_conditions";
const START: &str = "start";
const STILL: &str = "can_still_use";
const TICK: &str = "tick";

/// Adds the behaviors `mods[index]` declared for the entity type `entity` to the brain being built, on the
/// process's shared budget.
pub(crate) fn add_declared(
    mods: &Arc<[LuaMod]>,
    index: usize,
    entity: &str,
    behaviors: &mut BehaviorRegistrar<'_>,
) {
    add_declared_on(mods, index, entity, behaviors, Budget::shared());
}

/// `add_declared`, on the budget `budget` (the tests give their own, on a clock they move). A refusal from the
/// registrar (the type has no brain, the activity is not one of the brain's, or the duration is not one
/// vanilla draws) is logged once per mod, since the registrar asks every non-brain type at construction too.
pub(crate) fn add_declared_on(
    mods: &Arc<[LuaMod]>,
    index: usize,
    entity: &str,
    behaviors: &mut BehaviorRegistrar<'_>,
    budget: Arc<Budget>,
) {
    let module = &mods[index];
    for (behavior, record) in module.behavior_records().iter().enumerate() {
        if record.entity != entity {
            continue;
        }
        let added = behaviors.add(
            record.activity,
            record.priority,
            record.min_duration,
            record.max_duration,
            &[],
            LuaBehavior {
                mods: Arc::clone(mods),
                index,
                behavior,
                name: mob_turn::intern(&record.name),
                budget: Arc::clone(&budget),
                last: false,
                skipped: false,
            },
        );
        if let Err(refusal) = added {
            warn_once(
                module.id(),
                &format!("{}: behavior {}: {refusal}", module.id(), record.name),
            );
        }
    }
}

/// One behavior of a mod on one brain: a handle onto its record in its mod, the brain's last answer to
/// `can_still_use`, and whether its last check was skipped by the budget. Each call is one turn.
pub struct LuaBehavior {
    mods: Arc<[LuaMod]>,
    /// The behavior's mod, an index into `mods`.
    index: usize,
    /// The behavior's record, an index into the mod's behavior records.
    behavior: usize,
    name: &'static str,
    budget: Arc<Budget>,
    /// The answer `can_still_use` last gave (a check that answered true sets it).
    last: bool,
    /// Whether the last check was skipped by the budget.
    skipped: bool,
}

impl LuaBehavior {
    fn module(&self) -> &LuaMod {
        &self.mods[self.index]
    }

    fn record(&self) -> &BehaviorRecord {
        &self.module().behavior_records()[self.behavior]
    }

    fn has(&self, hook: &str) -> bool {
        self.record().hooks.iter().any(|name| name == hook)
    }

    /// The handler `hook` of this behavior, as the turn runner records it.
    fn handler<'a>(&'a self, hook: &'a str) -> Handler<'a> {
        let record = self.record();
        Handler {
            module: self.module(),
            label: &record.name,
            noun: "behavior",
            hook,
            key: behavior_handler_key(&record.name, hook),
        }
    }

    /// Runs the `check_extra_start_conditions` or `can_still_use` handler as one replace turn on the
    /// behavior's own deadline, then applies the actions it queued (none, when the turn was dropped). Says
    /// whether the turn failed.
    fn ask(&self, hook: &str, cx: &mut BehaviorCx<'_>) -> (Option<bool>, bool) {
        let handler = self.handler(hook);
        if handler.quarantined() {
            return (None, true);
        }
        let mob = cx.mob();
        let call = goal_actions::next_call();
        let mut actions = Vec::new();
        let failed = Cell::new(false);
        let mut answer = None;
        let build = mob_turn::event(mob, BehaviorCtx::userdata);
        let dropped = mob_turn::run_turn(
            &handler,
            mob_turn::turn_budget(),
            BEHAVIOR_TURN_MESSAGE,
            &failed,
            &mut actions,
            |actions| {
                dispatch::replace_in_reporting(
                    &self.mods,
                    &[self.index],
                    hook,
                    &handler.key,
                    TurnHost::Behavior {
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
        behavior_actions::apply(cx, actions, self.module().id(), hook);
        (answer, failed.get())
    }

    /// Runs the `start`, `tick` or `stop` handler as one turn, applies the actions it queued, and charges the
    /// time to the budget. `start` and `tick` run on the behavior's own deadline and count strikes; `stop`
    /// runs whatever the quarantine, under the handler budget, and a `stop` stopped by that budget is dropped.
    fn run(&self, hook: &str, cx: &mut BehaviorCx<'_>) {
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
        let build = mob_turn::event(mob, BehaviorCtx::userdata);
        let (limit, message) = if hook == STOP {
            (handler_budget(), "the handler ran over its time budget")
        } else {
            (mob_turn::turn_budget(), BEHAVIOR_TURN_MESSAGE)
        };
        mob_turn::run_turn(&handler, limit, message, &failed, &mut actions, |actions| {
            dispatch::turns_in_reporting(
                &self.mods,
                &[self.index],
                hook,
                &handler.key,
                TurnHost::Behavior {
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
        behavior_actions::apply(cx, actions, self.module().id(), hook);
        self.budget
            .charge(self.budget.now().saturating_duration_since(started));
    }
}

impl Behavior for LuaBehavior {
    fn check_extra_start_conditions(&mut self, cx: &mut BehaviorCx<'_>) -> bool {
        if !self.has(CHECK) {
            // No handler of its own: the trait's default, which starts the behavior.
            return true;
        }
        if !self.budget.may_start(self.skipped) {
            self.skipped = true;
            warn_once(
                self.module().id(),
                &format!(
                    "{}: behaviors: over the shared time budget; some behaviors were skipped",
                    self.module().id()
                ),
            );
            return false;
        }
        self.skipped = false;
        let started = self.budget.now();
        let (answer, _failed) = self.ask(CHECK, cx);
        self.budget
            .charge(self.budget.now().saturating_duration_since(started));
        self.last = answer.unwrap_or(false);
        self.last
    }

    fn start(&mut self, cx: &mut BehaviorCx<'_>) {
        // Not budget-gated: it only follows a check that answered true.
        self.run(START, cx);
    }

    fn can_still_use(&mut self, cx: &mut BehaviorCx<'_>) -> bool {
        if !self.has(STILL) {
            // No handler: vanilla's default, which lives out the brain tick that started it.
            return false;
        }
        // A panic quarantines the raw hook name (contained in hostcall), so this guard checks that name.
        if crate::quarantine::is_quarantined(self.module().id(), STILL) {
            return false;
        }
        if !self.budget.may_run() {
            return self.last;
        }
        let started = self.budget.now();
        let (answer, failed) = self.ask(STILL, cx);
        self.budget
            .charge(self.budget.now().saturating_duration_since(started));
        // A nil answer keeps the behavior running; a failed turn stops it.
        self.last = answer.unwrap_or(!failed);
        self.last
    }

    fn tick(&mut self, cx: &mut BehaviorCx<'_>) {
        if self.budget.may_run() {
            self.run(TICK, cx);
        }
    }

    fn stop(&mut self, cx: &mut BehaviorCx<'_>) {
        // Always runs, so a behavior can release what it holds; its time is charged but never refused.
        self.run(STOP, cx);
    }

    fn name(&self) -> &'static str {
        self.name
    }
}
