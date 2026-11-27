//! Quarantine (issue 74): a mod whose host call panicked for a hook stops receiving that hook for the
//! rest of the process. The mod's other hooks and every other mod are not affected, and nothing is
//! disabled globally. A panic in a per-tick hook would otherwise cost a backtrace and a log entry on
//! every event.
//!
//! The set is process-wide and bounded by [`MAX_QUARANTINED`] pairs. A panic past the cap is still
//! contained and logged, but its pair is not added, so that mod keeps receiving the hook.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The most (mod, hook) pairs the process quarantines.
pub const MAX_QUARANTINED: usize = 1024;

/// A bounded set of (mod id, hook) pairs.
#[derive(Default)]
pub struct Set {
    hooks: BTreeMap<String, BTreeSet<String>>,
    len: usize,
}

impl Set {
    /// Adds the pair when the set has room and does not hold it yet. Returns whether it was added.
    pub fn insert(&mut self, id: &str, hook: &str) -> bool {
        if self.len >= MAX_QUARANTINED || self.contains(id, hook) {
            return false;
        }
        self.hooks
            .entry(id.to_owned())
            .or_default()
            .insert(hook.to_owned());
        self.len += 1;
        true
    }

    /// Whether the pair is in the set.
    pub fn contains(&self, id: &str, hook: &str) -> bool {
        self.hooks.get(id).is_some_and(|hooks| hooks.contains(hook))
    }
}

/// The process's set, and its size, which lets the common case (nothing quarantined) skip the lock.
static SET: Mutex<Set> = Mutex::new(Set {
    hooks: BTreeMap::new(),
    len: 0,
});
static COUNT: AtomicUsize = AtomicUsize::new(0);

fn set() -> std::sync::MutexGuard<'static, Set> {
    SET.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Quarantines `hook` for mod `id`. Returns whether the pair was added: false when the cap is full
/// or the pair was already there.
pub fn quarantine(id: &str, hook: &str) -> bool {
    let added = set().insert(id, hook);
    if added {
        COUNT.fetch_add(1, Ordering::SeqCst);
    }
    added
}

/// Whether mod `id` is quarantined for `hook`.
pub fn is_quarantined(id: &str, hook: &str) -> bool {
    COUNT.load(Ordering::SeqCst) != 0 && set().contains(id, hook)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_holds_1024_pairs_and_the_1025th_is_not_added() {
        let mut set = Set::default();
        for n in 0..MAX_QUARANTINED {
            assert!(
                set.insert(&format!("qcap_{n}"), "block_receive_game"),
                "pair {n}"
            );
        }
        assert!(!set.insert("qcap_1024", "block_receive_game"));
        assert!(!set.contains("qcap_1024", "block_receive_game"));
        assert!(set.contains("qcap_0", "block_receive_game"));
        assert!(
            !set.contains("qcap_0", "server_tick"),
            "only the pair is quarantined"
        );
        assert!(
            !set.insert("qcap_0", "block_receive_game"),
            "a pair is added once"
        );
    }
}
