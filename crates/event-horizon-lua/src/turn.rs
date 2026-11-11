//! Per-state turn lock: one thread at a time runs Lua for a state.
//! A `both` hook from a worker thread has no caller side, so its firings
//! compete with the game threads; the state serialises them here.

use std::panic::Location;
use std::sync::Mutex;
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

/// How long a thread waits for the turn before it gives up.
pub const TURN_TIMEOUT: Duration = Duration::from_secs(5);

struct Inner {
    owner: Option<ThreadId>,
    depth: u32,
    /// Diagnostic snapshot of the outermost holder (thread name, since,
    /// take site). Read briefly by `held_by` after a timeout.
    holder_name: Option<String>,
    since: Option<Instant>,
    site: Option<String>,
    /// The hook (or op) the outermost holder names, when given.
    holder_hook: Option<String>,
}

/// A re-entrant lock owned by one `LuaMod` (one side of one mod).
/// Different states have different `ReentrantTurn`s and never block
/// each other; the same thread re-enters freely.
pub struct ReentrantTurn {
    inner: Mutex<Inner>,
}

/// The turn held for one entry; released when dropped, even on panic.
pub struct Guard<'a> {
    turn: &'a ReentrantTurn,
    owner: ThreadId,
}

impl ReentrantTurn {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                owner: None,
                depth: 0,
                holder_name: None,
                since: None,
                site: None,
                holder_hook: None,
            }),
        }
    }

    /// Takes the turn, waiting at most `timeout`. `None` on timeout.
    /// The same thread re-enters at once; another thread waits for
    /// the outermost entry to end.
    #[track_caller]
    pub fn try_acquire_timeout(&self, timeout: Duration) -> Option<Guard<'_>> {
        self.try_acquire_timeout_label(timeout, None)
    }

    /// Non-blocking `try_lock`-style take: `Some` without waiting
    /// when the turn is free (or the caller already holds it),
    /// `None` at once when another thread holds it. Frame hooks
    /// (`hud`, `gui_layer_post`) run on the Render thread and must
    /// never wait a frame's worth of time for the state.
    #[track_caller]
    pub fn try_acquire(&self, label: Option<&str>) -> Option<Guard<'_>> {
        self.try_acquire_timeout_label(Duration::ZERO, label)
    }

    /// `try_acquire_timeout` with a diagnostic label (the hook or op)
    /// recorded while held.
    #[track_caller]
    pub fn try_acquire_timeout_label(
        &self,
        timeout: Duration,
        label: Option<&str>,
    ) -> Option<Guard<'_>> {
        let deadline = Instant::now() + timeout;
        let current = thread::current().id();
        let caller = Location::caller();
        loop {
            {
                let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
                match inner.owner {
                    None => {
                        inner.owner = Some(current);
                        inner.depth = 1;
                        inner.holder_name =
                            Some(thread::current().name().unwrap_or("<unnamed>").to_owned());
                        inner.since = Some(Instant::now());
                        inner.site = Some(format!("{}:{}", caller.file(), caller.line()));
                        inner.holder_hook = label.map(str::to_owned);
                        return Some(Guard {
                            turn: self,
                            owner: current,
                        });
                    }
                    Some(id) if id == current => {
                        inner.depth = inner.depth.saturating_add(1);
                        return Some(Guard {
                            turn: self,
                            owner: current,
                        });
                    }
                    Some(_) => {}
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A one-line description of who holds the turn, for a timeout
    /// error: `held by thread "<name>" since <ms> ms at <file:line>`
    /// plus the re-entrant depth and hook. `None` when the turn is
    /// free. The snapshot is read under the inner mutex and the clock
    /// is read after it is released, so a waiter never holds the lock long.
    pub fn held_by(&self) -> Option<String> {
        let (name, since, site, depth, hook) = {
            let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            (
                inner.holder_name.clone(),
                inner.since,
                inner.site.clone(),
                inner.depth,
                inner.holder_hook.clone(),
            )
        };
        let name = name?;
        let elapsed_ms = since.map(|s| s.elapsed().as_millis()).unwrap_or(0);
        let site = site.unwrap_or_else(|| "<unknown>".to_owned());
        let hook = hook.map(|h| format!(" for \"{h}\"")).unwrap_or_default();
        Some(format!(
            "held by thread \"{name}\" since {elapsed_ms} ms at {site}{hook} (depth {depth})"
        ))
    }
}

impl Default for ReentrantTurn {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let mut inner = self.turn.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.owner == Some(self.owner) {
            inner.depth = inner.depth.saturating_sub(1);
            if inner.depth == 0 {
                inner.owner = None;
                inner.holder_name = None;
                inner.since = None;
                inner.site = None;
                inner.holder_hook = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn reentrancy_same_thread() {
        let t = ReentrantTurn::new();
        let g1 = t.try_acquire_timeout(Duration::from_millis(100)).unwrap();
        let g2 = t.try_acquire_timeout(Duration::from_millis(10)).unwrap();
        drop(g1);
        // outer still held, inner released: still held
        let g3 = t.try_acquire_timeout(Duration::from_millis(10)).unwrap();
        drop(g2);
        drop(g3);
        // now free
        let _g = t.try_acquire_timeout(Duration::from_millis(10)).unwrap();
    }

    #[test]
    fn two_threads_serialise() {
        let t = Arc::new(ReentrantTurn::new());
        let g = t.try_acquire_timeout(Duration::from_millis(100)).unwrap();
        let t2 = t.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let started = Instant::now();
            let g2 = t2.try_acquire_timeout(Duration::from_secs(2)).unwrap();
            tx.send(started.elapsed()).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            drop(g2);
        });
        std::thread::sleep(Duration::from_millis(50));
        drop(g);
        let waited = rx.recv().unwrap();
        handle.join().unwrap();
        // the worker waited for the main thread's guard
        assert!(waited >= Duration::from_millis(40), "{waited:?}");
    }

    #[test]
    fn timeout_after_bound() {
        let t = Arc::new(ReentrantTurn::new());
        let _g = t.try_acquire_timeout(Duration::from_millis(100)).unwrap();
        let t2 = t.clone();
        let started = Instant::now();
        let inner =
            std::thread::spawn(move || t2.try_acquire_timeout(Duration::from_millis(60)).is_some());
        // holder still holds
        let got = inner.join().unwrap();
        assert!(!got, "should time out while held");
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn drop_on_panic_releases() {
        let t = Arc::new(ReentrantTurn::new());
        let t2 = t.clone();
        let _ = std::panic::catch_unwind(move || {
            let _g = t2.try_acquire_timeout(Duration::from_millis(100)).unwrap();
            panic!("trip");
        });
        // after unwind the guard dropped, so the turn is free
        let g = t.try_acquire_timeout(Duration::from_millis(100)).unwrap();
        drop(g);
    }

    #[test]
    fn distinct_turns_do_not_block() {
        let a = Arc::new(ReentrantTurn::new());
        let b = Arc::new(ReentrantTurn::new());
        let _ga = a.try_acquire_timeout(Duration::from_millis(100)).unwrap();
        // b is free at once
        let started = Instant::now();
        let _gb = b.try_acquire_timeout(Duration::from_millis(100)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn atomic_counter_under_contention() {
        let t = Arc::new(ReentrantTurn::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let t2 = t.clone();
            let c2 = counter.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    let _g = t2.try_acquire_timeout(Duration::from_secs(5)).unwrap();
                    let v = c2.load(Ordering::Relaxed);
                    std::thread::yield_now();
                    c2.store(v + 1, Ordering::Relaxed);
                    drop(_g);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::Relaxed), 200);
    }

    #[test]
    fn try_acquire_is_immediate() {
        let t = Arc::new(ReentrantTurn::new());
        let t_holder = t.clone();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _g = t_holder
                .try_acquire_timeout(Duration::from_millis(100))
                .unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        rx.recv().unwrap();
        let started = Instant::now();
        assert!(
            t.try_acquire(Some("hud")).is_none(),
            "busy turn is skipped at once"
        );
        assert!(
            started.elapsed() < Duration::from_millis(20),
            "immediate take does not wait: {:?}",
            started.elapsed()
        );
        holder.join().unwrap();
        // Free turn is taken at once, and same thread re-enters.
        let _g = t.try_acquire(Some("hud")).expect("free turn takes at once");
        let _g2 = t.try_acquire(Some("hud")).expect("same thread re-enters");
    }

    #[test]
    fn timeout_names_the_holder_thread() {
        let t = Arc::new(ReentrantTurn::new());
        let t_holder = t.clone();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::Builder::new()
            .name("holder-thread".to_owned())
            .spawn(move || {
                let _g = t_holder
                    .try_acquire_timeout(Duration::from_millis(100))
                    .unwrap();
                tx.send(()).unwrap();
                // Hold until the waiter has timed out.
                std::thread::sleep(Duration::from_millis(150));
            })
            .unwrap();
        // Wait until the holder really holds it.
        rx.recv().unwrap();
        let t_waiter = t.clone();
        let got = std::thread::Builder::new()
            .name("waiter-thread".to_owned())
            .spawn(move || {
                t_waiter
                    .try_acquire_timeout(Duration::from_millis(60))
                    .is_some()
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(!got, "waiter should time out");
        let note = t.held_by().expect("held_by names the holder while held");
        assert!(
            note.contains("holder-thread"),
            "note names the holder thread: {note}"
        );
        assert!(
            note.contains("held by thread \"holder-thread\" since "),
            "note format: {note}"
        );
        assert!(!note.contains("waiter-thread"), "note: {note}");
        holder.join().unwrap();
        // After release the note is gone and the turn is free.
        assert!(t.held_by().is_none(), "free turn has no holder");
        let _g = t.try_acquire_timeout(Duration::from_millis(100)).unwrap();
    }
}
