//! A per-handler profiler for scripted mods (issue 53, dev tooling).
//!
//! For every mod x hook, the runtime counts hook firings, total time, max
//! time and budget overruns (a firing the budget stopped) since the last
//! reset. One firing runs all of a mod's handlers for the hook as one
//! batch, so it is one `calls` however many handlers the mod has. The
//! timing covers the same span the budget already times: the whole batch
//! per hook in `run_handlers_with` (`Budget::run`), frame hooks (`hud`,
//! `gui_layer_post`) included.
//!
//! Cost when nobody reads: two `Instant` reads per hook call and, after
//! the call, one short lock of a plain map. No lock is ever held while Lua
//! runs: the update happens once the call is back. A steady-state update
//! finds its (mod, hook) entry and allocates nothing; the keys are copied
//! in only on a pair's first firing. A single `Mutex` guards the map (the
//! degenerate sharded map the brief allows): hook calls are brief and the
//! lock is only taken after the call, so contention stays a short critical
//! section per hook firing.
//!
//! Reads (`profile_report`, `mc.profile`) sort on the way out, so the hot
//! path never sorts.
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// One row of the profile report: one mod's one hook since the last reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    /// The mod's id, as its manifest says.
    pub r#mod: String,
    /// The hook's name, as `horizon.on` takes it.
    pub hook: String,
    /// Hook firings recorded (one per firing with a handler, however many
    /// handlers the mod has for the hook).
    pub calls: u64,
    /// Total time of the hook firings, in nanoseconds.
    pub total_ns: u64,
    /// The slowest single hook firing, in nanoseconds.
    pub max_ns: u64,
    /// Firings the budget stopped (`ran over its time budget`).
    pub overruns: u64,
}

/// The counters one map entry holds.
#[derive(Debug, Default)]
struct Counters {
    calls: u64,
    total_ns: u64,
    max_ns: u64,
    overruns: u64,
}

/// The counters by mod id, then hook name, in no order; sorted on read. The
/// nesting lets a steady-state lookup take two `&str`s and allocate nothing.
static PROFILE: LazyLock<Mutex<HashMap<String, HashMap<String, Counters>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Record one hook firing of `hook` in the mod `mod_id`, taking `elapsed`
/// and whether the budget stopped it (`overrun`). Called once the handlers
/// are back, never while Lua runs.
pub(crate) fn note(mod_id: &str, hook: &str, elapsed: Duration, overrun: bool) {
    let mut profile = match PROFILE.lock() {
        Ok(profile) => profile,
        // A profiler failure never breaks a hook: the call already ran.
        Err(_) => return,
    };
    let ns = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
    // The steady state: both keys are there already, nothing is copied.
    if let Some(counters) = profile
        .get_mut(mod_id)
        .and_then(|hooks| hooks.get_mut(hook))
    {
        counters.add(ns, overrun);
        return;
    }
    // The first firing of this (mod, hook) pair: its keys are copied in once.
    profile
        .entry(mod_id.to_owned())
        .or_default()
        .entry(hook.to_owned())
        .or_default()
        .add(ns, overrun);
}

impl Counters {
    fn add(&mut self, ns: u64, overrun: bool) {
        self.calls += 1;
        self.total_ns = self.total_ns.saturating_add(ns);
        self.max_ns = self.max_ns.max(ns);
        if overrun {
            self.overruns += 1;
        }
    }
}

/// Every recorded (mod, hook) row since the last reset, sorted by mod id
/// then hook name. The sorted view `mc.profile_rows()` and a later
/// `horizon-ctl` command read.
pub fn profile_report() -> Vec<ProfileRow> {
    let mut rows: Vec<ProfileRow> = match PROFILE.lock() {
        Ok(profile) => profile
            .iter()
            .flat_map(|(mod_id, hooks)| {
                hooks.iter().map(move |(hook, c)| ProfileRow {
                    r#mod: mod_id.clone(),
                    hook: hook.clone(),
                    calls: c.calls,
                    total_ns: c.total_ns,
                    max_ns: c.max_ns,
                    overruns: c.overruns,
                })
            })
            .collect(),
        Err(_) => return Vec::new(),
    };
    rows.sort_by(|a, b| (&a.r#mod, &a.hook).cmp(&(&b.r#mod, &b.hook)));
    rows
}

/// Drop every recorded row: the next report starts from zero.
pub fn profile_reset() {
    if let Ok(mut profile) = PROFILE.lock() {
        profile.clear();
    }
}

/// Serialises the tests that read or reset the shared map: `cargo test`
/// runs them on threads of one process, and a reset in one would wipe the
/// rows another reads. Every test that reads the report takes it.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// The guard of `TEST_LOCK` (a poisoned lock still serialises).
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `note` accumulates per (mod, hook), `profile_report` sorts by mod
    /// then hook, and `profile_reset` drops every row. Unique ids keep the
    /// shared map to this test alone.
    #[test]
    fn note_accumulates_report_sorts_and_reset_clears() {
        let _lock = test_lock();
        note(
            "profiler_unit_beta",
            "server_tick",
            Duration::from_millis(3),
            false,
        );
        note(
            "profiler_unit_beta",
            "server_tick",
            Duration::from_millis(5),
            true,
        );
        note(
            "profiler_unit_alpha",
            "mob_spawned",
            Duration::from_millis(1),
            false,
        );
        let rows = profile_report();
        let beta = rows
            .iter()
            .find(|r| r.r#mod == "profiler_unit_beta" && r.hook == "server_tick")
            .expect("the beta row");
        assert_eq!(beta.calls, 2);
        assert_eq!(
            beta.total_ns,
            Duration::from_millis(8).as_nanos() as u64,
            "both calls add up: {beta:?}"
        );
        assert_eq!(beta.max_ns, Duration::from_millis(5).as_nanos() as u64);
        assert_eq!(beta.overruns, 1);
        let order: Vec<(&str, &str)> = rows
            .iter()
            .filter(|r| r.r#mod.starts_with("profiler_unit_"))
            .map(|r| (r.r#mod.as_str(), r.hook.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                ("profiler_unit_alpha", "mob_spawned"),
                ("profiler_unit_beta", "server_tick")
            ],
            "sorted by mod then hook: {order:?}"
        );
        profile_reset();
        assert!(
            profile_report()
                .iter()
                .all(|r| !r.r#mod.starts_with("profiler_unit_")),
            "reset drops every row"
        );
    }
}
