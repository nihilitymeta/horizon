//! The time budget of mod goals (#74): every goal turn of every mob shares one budget of Lua time per
//! window, so a mod's goals cannot take more of a server tick than the budget allows.
//!
//! The host has no tick counter, so the window is a fixed span of wall-clock time: it starts at the
//! first goal turn after the previous window ended, and it lasts [`GOAL_WINDOW`]. Each goal turn is
//! timed; the turns spend the window's [`GOAL_WINDOW_BUDGET`], and once it is spent:
//!
//! - `can_use` answers false without running Lua (the goal does not start);
//! - `can_continue` gives its last answer without running Lua (a running goal is never stopped by
//!   the budget);
//! - `tick` is skipped;
//! - `start` is not budget-gated: it only follows a `can_use` that answered true;
//! - `stop` always runs, so a goal can release its locks. Its time is charged, and it is exempt from
//!   the cap.
//!
//! Fairness: while less than [`GOAL_RESERVE`] is left, only a goal that was skipped in the last
//! turn it asked for may start (`can_use` runs for it); the others are skipped. So a goal skipped in
//! one window gets the reserve in the next one before the goals that were never skipped.
//!
//! Each goal turn also has its own deadline, [`GOAL_TURN_BUDGET`], which is shorter than the window's
//! budget: a handler that loops is stopped at 1 ms, not at the 50 ms handler budget. So the worst case
//! for one window is [`GOAL_WINDOW_BUDGET`] plus one turn's [`GOAL_TURN_BUDGET`] (a turn that starts
//! just inside the window's budget runs to its deadline). A turn stopped by its deadline is a budget
//! failure: its queued actions are dropped, and [`GOAL_STRIKES`] of them in a row quarantine that goal
//! handler (goal.rs). `stop` is not on the 1 ms deadline: it runs under the handler budget.
//!
//! The clock is injected ([`Clock`]), so the window logic is tested with a fake one. [`Budget::shared`]
//! is the process's budget, on the system clock.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// The Lua time all goal turns of all mobs may take in one window.
pub const GOAL_WINDOW_BUDGET: Duration = Duration::from_millis(4);
/// The length of a window of wall-clock time.
pub const GOAL_WINDOW: Duration = Duration::from_millis(50);
/// The deadline of one goal turn: its `can_use`, `can_continue`, `tick` or `start` handler, on the
/// interrupt of the mod's Lua state (a nested call may shorten it, never lengthen it).
pub const GOAL_TURN_BUDGET: Duration = Duration::from_millis(1);
/// The budget failures in a row after which a goal handler is quarantined.
pub const GOAL_STRIKES: u32 = 3;
/// The part of the budget kept back for goals that were skipped in their last turn: a `can_use` runs
/// when at least this much is left, or when its goal was skipped.
pub const GOAL_RESERVE: Duration = Duration::from_millis(1);

/// Where the budget reads the time from.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// The system's monotonic clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// The window's state: when it started and how much Lua time its turns have spent.
#[derive(Debug, Default)]
struct Window {
    start: Option<Instant>,
    spent: Duration,
}

/// The shared budget of goal turns. Every goal turn asks it before it runs and reports its time after.
pub struct Budget {
    clock: Arc<dyn Clock>,
    window: Mutex<Window>,
}

impl Budget {
    /// A budget with no window open, on `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Budget {
            clock,
            window: Mutex::new(Window::default()),
        }
    }

    /// The process's budget, on the system clock: the one every mod's goals share.
    pub fn shared() -> Arc<Budget> {
        static SHARED: OnceLock<Arc<Budget>> = OnceLock::new();
        Arc::clone(SHARED.get_or_init(|| Arc::new(Budget::new(Arc::new(SystemClock)))))
    }

    /// The window that `now` falls in, opened at `now` if the last one ended.
    fn roll(window: &mut Window, now: Instant) {
        match window.start {
            Some(start) if now.saturating_duration_since(start) < GOAL_WINDOW => {}
            _ => {
                *window = Window {
                    start: Some(now),
                    spent: Duration::ZERO,
                }
            }
        }
    }

    fn with_window<T>(&self, f: impl FnOnce(&mut Window, Instant) -> T) -> T {
        let now = self.clock.now();
        let mut window = self.window.lock().unwrap_or_else(PoisonError::into_inner);
        Self::roll(&mut window, now);
        f(&mut window, now)
    }

    /// The Lua time the current window has spent so far.
    #[cfg(test)]
    pub fn spent(&self) -> Duration {
        self.with_window(|window, _| window.spent)
    }

    /// Whether a `can_use` turn may run: some budget is left, and, when less than [`GOAL_RESERVE`]
    /// is left, its goal was skipped in its last turn (`skipped`).
    pub fn may_start(&self, skipped: bool) -> bool {
        self.with_window(|window, _| {
            let left = GOAL_WINDOW_BUDGET.saturating_sub(window.spent);
            !left.is_zero() && (left >= GOAL_RESERVE || skipped)
        })
    }

    /// Whether a turn of a running goal (`tick`, `can_continue`) may run: some budget is left.
    pub fn may_run(&self) -> bool {
        self.with_window(|window, _| window.spent < GOAL_WINDOW_BUDGET)
    }

    /// Reports `used` Lua time for a turn. It counts against the window the turn is in.
    pub fn charge(&self, used: Duration) {
        self.with_window(|window, _| window.spent += used);
    }

    /// The clock the budget reads, for the time a turn takes.
    pub fn now(&self) -> Instant {
        self.clock.now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock the test moves by hand.
    struct Manual(Mutex<Instant>);

    impl Manual {
        fn at(start: Instant) -> Arc<Manual> {
            Arc::new(Manual(Mutex::new(start)))
        }
        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    impl Clock for Manual {
        fn now(&self) -> Instant {
            *self.0.lock().unwrap()
        }
    }

    fn budget_at(start: Instant) -> (Budget, Arc<Manual>) {
        let clock = Manual::at(start);
        (Budget::new(clock.clone()), clock)
    }

    #[test]
    fn a_fresh_budget_has_the_whole_window_to_spend() {
        let (budget, _) = budget_at(Instant::now());
        assert!(budget.may_start(false));
        assert!(budget.may_run());
        assert_eq!(budget.spent(), Duration::ZERO);
    }

    #[test]
    fn a_spent_window_refuses_every_turn_but_stop_is_not_asked() {
        let (budget, _) = budget_at(Instant::now());
        budget.charge(Duration::from_millis(4));
        assert!(!budget.may_start(false));
        assert!(!budget.may_start(true));
        assert!(!budget.may_run());
    }

    #[test]
    fn the_next_window_starts_when_the_last_one_has_ended_and_resets_the_spend() {
        let base = Instant::now();
        let (budget, clock) = budget_at(base);
        budget.charge(Duration::from_millis(4));
        clock.advance(GOAL_WINDOW - Duration::from_millis(1));
        assert!(!budget.may_run(), "the window is still open");
        clock.advance(Duration::from_millis(1));
        assert!(budget.may_run(), "the window ended: its spend is gone");
        assert_eq!(budget.spent(), Duration::ZERO);
    }

    #[test]
    fn a_window_opens_at_the_first_turn_not_at_a_fixed_tick() {
        let base = Instant::now();
        let (budget, clock) = budget_at(base);
        clock.advance(Duration::from_secs(10));
        budget.charge(Duration::from_millis(1));
        // The window opened at the first call after the clock moved, so it lasts until 50 ms after
        // that, not until 50 ms after `base`.
        clock.advance(GOAL_WINDOW - Duration::from_millis(1));
        assert_eq!(budget.spent(), Duration::from_millis(1));
    }

    #[test]
    fn below_the_reserve_only_a_skipped_goal_may_start() {
        let (budget, _) = budget_at(Instant::now());
        budget.charge(GOAL_WINDOW_BUDGET - GOAL_RESERVE + Duration::from_micros(1));
        assert!(!budget.may_start(false), "a goal not skipped waits");
        assert!(
            budget.may_start(true),
            "a goal skipped before may use the reserve"
        );
        assert!(budget.may_run(), "a running goal still gets its turn");
    }

    #[test]
    fn at_the_reserve_every_goal_may_start() {
        let (budget, _) = budget_at(Instant::now());
        budget.charge(GOAL_WINDOW_BUDGET - GOAL_RESERVE);
        assert!(budget.may_start(false));
    }

    #[test]
    fn a_charge_counts_against_the_window_it_is_made_in() {
        let base = Instant::now();
        let (budget, clock) = budget_at(base);
        budget.charge(Duration::from_millis(2));
        clock.advance(Duration::from_millis(30));
        budget.charge(Duration::from_millis(1));
        assert_eq!(budget.spent(), Duration::from_millis(3));
    }
}
