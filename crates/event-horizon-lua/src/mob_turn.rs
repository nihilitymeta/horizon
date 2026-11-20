//! The turn runner of a mob's declared handlers (#74): what the goals of an entity part (goal.rs) run
//! their Lua turns on, and what brain behaviors will too. The pieces both use:
//!
//! - [`turn_budget`]: the deadline of one turn (`GOAL_TURN_BUDGET`, or a test's own on its thread);
//! - [`intern`]: a record's name as the `&'static str` a game trait wants, leaked once per name;
//! - [`Handler`]: one handler of one record on one mod, with its strikes and quarantine. A turn that
//!   overruns its deadline is a budget failure: [`GOAL_STRIKES`] of them in a row quarantine the handler,
//!   and a `stop` is never counted;
//! - [`run_turn`]: one turn on its deadline, which settles the strikes and drops the actions of a budget
//!   failure. The caller applies the actions that are left;
//! - [`event`]: the `e` table a handler gets (`mob`, and `ctx` made for the call);
//! - [`queue`]: one `e.ctx` action queued for its call, with the call's room (`MAX_ACTIONS`).
//!
//! The window budget itself is `goal_budget::Budget`, shared by every declared handler of every mob.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use mlua::{AnyUserData, Error, Lua};
use mod_api::MobRef;

use crate::budget;
use crate::dispatch::{self, Event};
use crate::goal_actions::MAX_ACTIONS;
use crate::goal_budget::{GOAL_STRIKES, GOAL_TURN_BUDGET};
use crate::handles;
use crate::host::{LuaMod, warn_once};
use crate::hostcall::{TurnHost, refused};
use crate::quarantine;

/// The hook a handler runs under the handler budget, not the turn deadline: never counted as a strike.
pub(crate) const STOP: &str = "stop";

/// The names this process has seen, each leaked once: a trait names its items with `&'static str`, and
/// the names are the mods' declared records, so the leak is bounded by the records declared.
static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// The budget failures in a row of each handler, by (mod, handler key).
static STRIKES: Mutex<BTreeMap<(String, String), u32>> = Mutex::new(BTreeMap::new());

/// The deadline a turn runs under: `GOAL_TURN_BUDGET`, or in tests a value the test set on its own
/// thread, so tests that check results are not timed against the wall clock.
pub(crate) fn turn_budget() -> Duration {
    #[cfg(test)]
    {
        if let Some(limit) = TURN_BUDGET_OVERRIDE.with(Cell::get) {
            return limit;
        }
    }
    GOAL_TURN_BUDGET
}

#[cfg(test)]
thread_local! {
    /// A test's own turn deadline, on its thread; unset in production.
    pub(crate) static TURN_BUDGET_OVERRIDE: Cell<Option<Duration>> = const { Cell::new(None) };
}

/// The name as a `&'static str`, leaked once per distinct name.
pub(crate) fn intern(name: &str) -> &'static str {
    let mut names = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(found) = names.iter().find(|known| **known == name) {
        return found;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(leaked);
    leaked
}

/// One handler of one declared record (a goal or a behavior) on one mod: what a turn of it is recorded
/// against. `label` is the record's name, `noun` what the record is (named in the quarantine warning),
/// `hook` the handler's hook and `key` its key in the mod's handler table.
pub(crate) struct Handler<'a> {
    pub(crate) module: &'a LuaMod,
    pub(crate) label: &'a str,
    pub(crate) noun: &'static str,
    pub(crate) hook: &'a str,
    pub(crate) key: String,
}

impl Handler<'_> {
    /// Whether this handler is quarantined for its mod: after `GOAL_STRIKES` budget failures in a row.
    pub(crate) fn quarantined(&self) -> bool {
        quarantine::is_quarantined(self.module.id(), &self.key)
    }

    /// Records how a turn of this handler ended: a budget failure (the turn failed with its deadline
    /// passed) adds a strike, and any other turn clears them. Returns whether the turn's actions are
    /// dropped: a budget failure drops them, and so does a `stop` stopped by its budget (which is not
    /// counted). The third strike quarantines the handler.
    pub(crate) fn settle(&self, failed: bool, expired: bool) -> bool {
        if self.hook == STOP {
            return failed && expired;
        }
        let id = self.module.id();
        let mut strikes = STRIKES.lock().unwrap_or_else(PoisonError::into_inner);
        let count = strikes
            .entry((id.to_owned(), self.key.clone()))
            .or_insert(0);
        if !(failed && expired) {
            *count = 0;
            return false;
        }
        *count += 1;
        if *count >= GOAL_STRIKES {
            quarantine::quarantine(id, &self.key);
            warn_once(
                id,
                &format!(
                    "{id}: {}: {} went over its {GOAL_TURN_BUDGET:?} turn budget {GOAL_STRIKES} times in a row; the {} is quarantined",
                    self.label, self.hook, self.noun
                ),
            );
        }
        true
    }
}

/// Runs one turn of `handler` on the deadline `limit` (`message` names a turn the deadline stops). `body`
/// makes the dispatch call, queuing its actions into the vector it is given. A turn that fails (an error,
/// a panic, or a budget failure) sets `failed`, and its error is logged. The strikes are settled, and the
/// actions of a dropped turn are cleared. Returns whether they were dropped; the caller applies the rest.
pub(crate) fn run_turn<A>(
    handler: &Handler<'_>,
    limit: Duration,
    message: &'static str,
    failed: &Cell<bool>,
    actions: &mut Vec<A>,
    body: impl FnOnce(&mut Vec<A>) -> Result<(), String>,
) -> bool {
    let (opened, expired) =
        budget::within_with_expiry(handler.module.lua(), limit, message, || body(actions));
    if opened.is_err() {
        failed.set(true);
    }
    refused(opened);
    let dropped = handler.settle(failed.get(), expired);
    if dropped {
        actions.clear();
    }
    dropped
}

/// The `e` table a handler gets: `mob` (the MobRef) and `ctx`, made for the call by `ctx` (the call's
/// number is read from the turn, so a kept `ctx` can tell it is stale).
pub(crate) fn event(
    mob: MobRef,
    ctx: fn(&Lua, u64) -> mlua::Result<AnyUserData>,
) -> impl Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event> {
    move |lua, turn| {
        let data = lua.create_table()?;
        data.raw_set("mob", handles::family_out(lua, mob)?)?;
        let call = turn.mob_call().unwrap_or(0);
        data.raw_set("ctx", ctx(lua, call)?)?;
        dispatch::new_event(lua, data, &[])
    }
}

/// Queues one `e.ctx` action of the call `call` into its turn's `actions`. The turn must be that call
/// (else the `ctx` is stale and `unavailable` is raised), and the call must have room (`MAX_ACTIONS`).
pub(crate) fn queue<A>(
    actions: &mut Vec<A>,
    current: u64,
    call: u64,
    action: A,
    unavailable: &str,
) -> mlua::Result<()> {
    if current != call {
        return Err(Error::runtime(unavailable.to_owned()));
    }
    if actions.len() >= MAX_ACTIONS {
        return Err(Error::runtime(format!(
            "a call may make at most {MAX_ACTIONS} actions; this one is over"
        )));
    }
    actions.push(action);
    Ok(())
}
