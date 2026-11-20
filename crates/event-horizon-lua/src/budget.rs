//! The time budget of a Lua state: one interrupt per state, reading a stack of deadlines.
//!
//! A call that must not run for long goes through `within` (or `within_with`). The state gets one
//! interrupt, installed on first use and kept for the state's life. It reads the deadline in force,
//! which is the earliest deadline on a stack of nested calls, and fails once that deadline has
//! passed. The failure is repeated at every interrupt point, so a `pcall` in the mod cannot swallow
//! it. Outside any call the deadline is none, and the interrupt only costs one atomic read.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mlua::{Error, Lua, VmState};

/// The message a call stops with when `within` gives none.
const DEFAULT_MESSAGE: &str = "the mod ran over its time budget";

/// The deadline of a state with no call running.
const NONE: u64 = u64::MAX;

/// The latest deadline a budget can hold: one tick short of `NONE`.
const LATEST: u64 = u64::MAX - 1;

/// The budget of one state: its base instant, the deadline in force, and the stack of calls.
struct Budget {
    base: Instant,
    deadline: AtomicU64,
    frames: Mutex<Vec<Frame>>,
}

/// One nested call: the deadline in force for it (never later than its caller's) and the message
/// the interrupt fails with once that deadline passes.
#[derive(Clone, Copy)]
struct Frame {
    deadline: u64,
    message: &'static str,
}

impl Budget {
    /// Nanoseconds since the base instant, capped below `NONE`.
    fn now(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_nanos())
            .unwrap_or(LATEST)
            .min(LATEST)
    }

    fn frames(&self) -> MutexGuard<'_, Vec<Frame>> {
        self.frames.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publishes the deadline of the top frame, or none when no call runs.
    fn publish(&self, frames: &[Frame]) {
        let deadline = frames.last().map_or(NONE, |frame| frame.deadline);
        self.deadline.store(deadline, Ordering::SeqCst);
    }
}

/// Pops the frame of one call when it ends, also when the call panics.
struct Guard {
    budget: Arc<Budget>,
    depth: usize,
    /// The deadline this call asked for (its own, which an outer call may have shortened).
    own: u64,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut frames = self.budget.frames();
        frames.truncate(self.depth);
        self.budget.publish(&frames);
    }
}

/// Runs `f`, a call into `lua`, with `limit` as its time budget. The call stops with the default
/// message once the budget is spent. See `within_with`.
pub fn within<T>(lua: &Lua, limit: Duration, f: impl FnOnce() -> T) -> T {
    within_with(lua, limit, DEFAULT_MESSAGE, f)
}

/// Runs `f`, a call into `lua`, with `limit` as its time budget, and fails the call with `message`
/// once that budget is spent. If a budget is already running on this state (a nested call), the
/// deadline in force is the earlier of the two, so a nested call never extends its caller. When
/// `f` returns, the caller's deadline is restored.
pub fn within_with<T>(
    lua: &Lua,
    limit: Duration,
    message: &'static str,
    f: impl FnOnce() -> T,
) -> T {
    let budget = install(lua);
    let _guard = enter(&budget, limit, message);
    f()
}

/// Runs `f` as `within_with`, and also says whether the call's own deadline had passed by the time `f`
/// returned. A call that failed with the deadline passed was stopped by its time budget, as opposed to
/// failing for another reason; the caller uses this to tell the two apart.
pub fn within_with_expiry<T>(
    lua: &Lua,
    limit: Duration,
    message: &'static str,
    f: impl FnOnce() -> T,
) -> (T, bool) {
    let budget = install(lua);
    let guard = enter(&budget, limit, message);
    let out = f();
    let expired = budget.now() >= guard.own;
    (out, expired)
}

/// The state's budget, installing its one interrupt on first use.
fn install(lua: &Lua) -> Arc<Budget> {
    if let Some(existing) = lua.app_data_ref::<Arc<Budget>>() {
        return Arc::clone(&existing);
    }
    let budget = Arc::new(Budget {
        base: Instant::now(),
        deadline: AtomicU64::new(NONE),
        frames: Mutex::new(Vec::new()),
    });
    lua.set_app_data(Arc::clone(&budget));
    let interrupt = Arc::clone(&budget);
    lua.set_interrupt(move |_| check(&interrupt));
    budget
}

/// Pushes the frame of a call that may run for `limit` and returns the guard that pops it.
fn enter(budget: &Arc<Budget>, limit: Duration, message: &'static str) -> Guard {
    let mut frames = budget.frames();
    let depth = frames.len();
    let limit = u64::try_from(limit.as_nanos()).unwrap_or(LATEST);
    let own = budget.now().saturating_add(limit).min(LATEST);
    let frame = match frames.last().copied() {
        Some(outer) if outer.deadline <= own => outer,
        _ => Frame {
            deadline: own,
            message,
        },
    };
    frames.push(frame);
    budget.publish(&frames);
    Guard {
        budget: Arc::clone(budget),
        depth,
        own,
    }
}

/// The interrupt: continue while the deadline in force is ahead, and fail once it has passed.
fn check(budget: &Budget) -> mlua::Result<VmState> {
    let deadline = budget.deadline.load(Ordering::SeqCst);
    if deadline == NONE || budget.now() < deadline {
        return Ok(VmState::Continue);
    }
    let message = budget
        .frames()
        .last()
        .map_or(DEFAULT_MESSAGE, |f| f.message);
    Err(Error::runtime(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::new_state;

    /// The most a test lets a stopped call take. Far past every budget used here, so a call that
    /// was not stopped fails the test instead of hanging it.
    const SLACK: Duration = Duration::from_secs(5);

    #[test]
    fn a_runaway_is_stopped_at_its_deadline() {
        let lua = new_state().unwrap();
        let limit = Duration::from_millis(100);
        let started = Instant::now();
        let result = within(&lua, limit, || lua.load("while true do end").exec());
        let elapsed = started.elapsed();
        assert!(result.is_err(), "the loop must be stopped");
        assert!(elapsed >= limit, "stopped early: {elapsed:?}");
        assert!(elapsed < SLACK, "not stopped in time: {elapsed:?}");
    }

    #[test]
    fn the_state_is_usable_after_and_costs_nothing_when_idle() {
        let lua = new_state().unwrap();
        let _ = within(&lua, Duration::from_millis(50), || {
            lua.load("while true do end").exec()
        });
        let sum: i64 = lua.load("return 1 + 1").eval().unwrap();
        assert_eq!(sum, 2);
        // Outside any call the interrupt is installed but has no deadline, so a long loop runs.
        let started = Instant::now();
        let count: i64 = lua
            .load("local n = 0 for _ = 1, 5000000 do n = n + 1 end return n")
            .eval()
            .unwrap();
        assert_eq!(count, 5_000_000);
        assert!(started.elapsed() < SLACK);
    }

    #[test]
    fn a_nested_call_cannot_extend_its_caller() {
        let lua = new_state().unwrap();
        let outer = Duration::from_millis(100);
        let started = Instant::now();
        let result = within(&lua, outer, || {
            let inner = lua
                .create_function(|lua, ()| {
                    within(lua, Duration::from_secs(10), || {
                        lua.load("while true do end").exec()
                    })
                })
                .unwrap();
            inner.call::<()>(())
        });
        let elapsed = started.elapsed();
        assert!(result.is_err(), "the inner loop must be stopped");
        assert!(elapsed >= outer, "stopped early: {elapsed:?}");
        assert!(
            elapsed < Duration::from_secs(2),
            "the inner call ran past its caller's deadline: {elapsed:?}"
        );
    }

    #[test]
    fn a_nested_call_may_shorten_and_the_outer_budget_is_restored() {
        let lua = new_state().unwrap();
        let outer = Duration::from_millis(400);
        let inner_limit = Duration::from_millis(50);
        let started = Instant::now();
        let result = within(&lua, outer, || {
            let inner = lua
                .create_function(move |lua, ()| {
                    within(lua, inner_limit, || lua.load("while true do end").exec())
                })
                .unwrap();
            // The inner call is stopped at its own 50 ms; the outer call then keeps running.
            assert!(inner.call::<()>(()).is_err());
            lua.load("while true do end").exec()
        });
        let elapsed = started.elapsed();
        assert!(result.is_err(), "the outer loop must be stopped");
        assert!(
            elapsed >= outer,
            "the outer deadline was not restored after the inner call: {elapsed:?}"
        );
        assert!(elapsed < SLACK, "not stopped in time: {elapsed:?}");
    }

    #[test]
    fn pcall_cannot_swallow_the_budget_through_within() {
        let lua = new_state().unwrap();
        let started = Instant::now();
        let result = within(&lua, Duration::from_millis(100), || {
            lua.load("while true do pcall(function() while true do end end) end")
                .exec()
        });
        assert!(result.is_err(), "a pcall must not swallow the stop");
        assert!(started.elapsed() < SLACK);
    }

    #[test]
    fn expiry_tells_a_stopped_call_from_one_that_failed_early() {
        let lua = new_state().unwrap();
        let limit = Duration::from_millis(20);
        let (stopped, expired) = within_with_expiry(&lua, limit, "stopped", || {
            lua.load("while true do end").exec().is_err()
        });
        assert!(stopped && expired, "the loop is stopped by its deadline");
        let (failed, expired) = within_with_expiry(&lua, Duration::from_secs(5), "late", || {
            lua.load("error('early')").exec().is_err()
        });
        assert!(
            failed && !expired,
            "an ordinary error well before the deadline"
        );
    }

    #[test]
    fn the_failure_carries_the_message_of_the_call_that_ran_over() {
        let lua = new_state().unwrap();
        let err = within_with(
            &lua,
            Duration::from_millis(50),
            "the test call ran over",
            || lua.load("while true do end").exec(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("the test call ran over"),
            "got {err}"
        );
    }
}
