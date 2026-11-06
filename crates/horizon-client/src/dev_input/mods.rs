//! The scripted mods' status: what the developer tools show of each of them
//! (docs/dev-tools.md), and the reloads the tools ask for. The game fills
//! the store when a mod loads, reloads or fails, and the windows' half reads
//! it; the store lives here, with the rest of the tools' shared state, so
//! the game and the windows never reach into each other.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// What the developer tools show of one scripted mod (docs/dev-tools.md).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModStatus {
    /// The mod's id, or its path's file name when it did not load.
    pub id: String,
    /// The `.hmod` or folder.
    pub source: String,
    /// Scripts read from it.
    pub scripts: usize,
    /// Reloads done (`/reload`, the console).
    pub reloads: u32,
    /// Hook names with the count of handlers registered for each, sorted.
    pub handlers: Vec<(String, usize)>,
    /// Goal names (`horizon.goal`), sorted.
    pub goals: Vec<String>,
    /// The last error: a load or reload failure, or a handler's error, until the next reload.
    pub error: Option<String>,
}

/// Every scripted mod's status, in load order.
static STATUS: Mutex<Vec<ModStatus>> = Mutex::new(Vec::new());
/// Reloads asked for from the developer tools: by mod id, or everyone with a
/// new generation of `RELOAD_ALL`. Served on the server thread at the start
/// of a tick.
static RELOAD: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RELOAD_ALL: AtomicU32 = AtomicU32::new(0);

/// The scripted mods and what is known of them, for the developer tools.
pub fn status() -> Vec<ModStatus> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Ask the mod `id` (None: every mod) to read its scripts again and run
/// `init.lua` afresh at the next server tick, as `/reload` makes it do.
pub fn request_reload(id: Option<&str>) {
    match id {
        Some(id) => RELOAD
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(id.to_owned()),
        None => {
            RELOAD_ALL.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The status of a mod, kept in the store (the game, when a mod loads,
/// reloads or fails).
pub fn set_status(status: ModStatus) {
    let mut all = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    match all.iter_mut().find(|s| s.id == status.id) {
        Some(s) => *s = status,
        None => all.push(status),
    }
}

/// The last error of the mod `id`, kept in its status.
pub fn set_error(id: &str, error: String) {
    if let Some(s) = STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter_mut()
        .find(|s| s.id == id)
    {
        s.error = Some(error);
    }
}

/// The generation of "reload every mod" the store is at.
pub fn reload_generation() -> u32 {
    RELOAD_ALL.load(Ordering::Relaxed)
}

/// Whether a reload was asked for the mod `id` since `seen` last asked: the
/// generation of "reload every mod", or the mod's own name in the list.
pub fn reload_asked(id: &str, seen: &AtomicU32) -> bool {
    let all = RELOAD_ALL.load(Ordering::Relaxed);
    let mut asked = seen.swap(all, Ordering::Relaxed) != all;
    let mut list = RELOAD.lock().unwrap_or_else(|e| e.into_inner());
    if list.iter().any(|i| i == id) {
        list.retain(|i| i != id);
        asked = true;
    }
    asked
}
