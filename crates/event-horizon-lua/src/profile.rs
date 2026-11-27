//! The Lua profiler: per mod and hook, how many handler calls ran, their total and longest time,
//! and how many failed. Recording is off by default and `profile_enable(true)` turns it on (the
//! `/luaprofile` command does). Off, a handler call costs one relaxed atomic load and reads no
//! clock. On, one call reads the clock twice ([`timed`]); no allocation happens unless a mod or a
//! hook is seen for the first time.
//!
//! The store is process-wide and bounded: a mod keeps at most [`MAX_HOOKS_PER_MOD`] hooks, and a
//! hook past that is not recorded.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// The most hooks one mod keeps counters for. A hook past this is not recorded.
pub const MAX_HOOKS_PER_MOD: usize = 256;

/// One row of the profile: a mod's handler calls for one hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModProfile {
    pub mod_id: String,
    pub hook: String,
    pub calls: u64,
    pub total_ns: u64,
    pub max_ns: u64,
    pub errors: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    calls: u64,
    total_ns: u64,
    max_ns: u64,
    errors: u64,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static STORE: Mutex<BTreeMap<String, BTreeMap<String, Stats>>> = Mutex::new(BTreeMap::new());

fn store() -> MutexGuard<'static, BTreeMap<String, BTreeMap<String, Stats>>> {
    STORE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether recording is on.
pub fn profile_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turns recording on or off. The counters are kept across a switch off; `profile_reset` clears them.
pub fn profile_enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Clears every counter. Recording stays as it is.
pub fn profile_reset() {
    store().clear();
}

/// Every row recorded so far, in mod then hook order.
pub fn profile_snapshot() -> Vec<ModProfile> {
    store()
        .iter()
        .flat_map(|(mod_id, hooks)| {
            hooks.iter().map(move |(hook, s)| ModProfile {
                mod_id: mod_id.clone(),
                hook: hook.clone(),
                calls: s.calls,
                total_ns: s.total_ns,
                max_ns: s.max_ns,
                errors: s.errors,
            })
        })
        .collect()
}

/// Runs one handler call of mod `mod_id` for `hook`, and records it when recording is on: its time,
/// and an error when the call returns `Err`. When recording is off this is just `call()`.
pub(crate) fn timed<T>(
    mod_id: &str,
    hook: &str,
    call: impl FnOnce() -> mlua::Result<T>,
) -> mlua::Result<T> {
    if !profile_enabled() {
        return call();
    }
    let started = Instant::now();
    let out = call();
    let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    record(mod_id, hook, ns, out.is_err());
    out
}

fn record(mod_id: &str, hook: &str, ns: u64, error: bool) {
    let mut store = store();
    if !store.contains_key(mod_id) {
        store.insert(mod_id.to_owned(), BTreeMap::new());
    }
    let Some(hooks) = store.get_mut(mod_id) else {
        return;
    };
    if !hooks.contains_key(hook) {
        if hooks.len() >= MAX_HOOKS_PER_MOD {
            return;
        }
        hooks.insert(hook.to_owned(), Stats::default());
    }
    let Some(s) = hooks.get_mut(hook) else {
        return;
    };
    s.calls = s.calls.saturating_add(1);
    s.total_ns = s.total_ns.saturating_add(ns);
    s.max_ns = s.max_ns.max(ns);
    if error {
        s.errors = s.errors.saturating_add(1);
    }
}

#[cfg(test)]
#[path = "profile_tests.rs"]
pub(crate) mod tests;
