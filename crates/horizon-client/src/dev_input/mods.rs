//! The scripted mods' status: what the developer tools show of each of them
//! (docs/dev-tools.md), and the reloads the tools ask for. The game fills
//! the store when a mod loads, reloads or fails, and the windows' half reads
//! it; the store lives here, with the rest of the tools' shared state, so
//! the game and the windows never reach into each other.

use serde_json::Value;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

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

/// How many times each mod's status was set: once per load and per reload,
/// failed or not. A reload is applied when its mod's count moves.
static WRITES: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());

fn writes(id: &str) -> u32 {
    WRITES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(i, _)| i == id)
        .map_or(0, |(_, n)| *n)
}

/// Whether each mod declares the `registries` capability (its `registry.lua`
/// needs a restart to change). Read when the game sets the mod's status, on
/// the game's thread, so the windows' reload replies never read the package.
static REGISTRIES: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

fn remember_registries(id: &str, declares: bool) {
    let mut r = REGISTRIES.lock().unwrap_or_else(|e| e.into_inner());
    match r.iter_mut().find(|(i, _)| i == id) {
        Some((_, d)) => *d = declares,
        None => r.push((id.to_owned(), declares)),
    }
}

fn registries_of(id: &str) -> bool {
    REGISTRIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|(i, d)| i == id && *d)
}

fn declares_registries(source: &str) -> bool {
    horizon_mods::Package::read(std::path::Path::new(source)).is_ok_and(|p| {
        p.manifest
            .capabilities
            .contains(&horizon_mods::Capability::Registries)
    })
}

/// The status of a mod, kept in the store (the game, when a mod loads,
/// reloads or fails).
pub fn set_status(status: ModStatus) {
    remember_registries(&status.id, declares_registries(&status.source));
    {
        let mut w = WRITES.lock().unwrap_or_else(|e| e.into_inner());
        match w.iter_mut().find(|(i, _)| *i == status.id) {
            Some((_, n)) => *n += 1,
            None => w.push((status.id.clone(), 1)),
        }
    }
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

/// The ids a `reload-mods [ID]` reloads.
pub fn reload_targets(id: Option<&str>) -> Result<Vec<String>, String> {
    let all = status();
    match id {
        Some(want) if all.iter().any(|m| m.id == want) => Ok(vec![want.to_owned()]),
        Some(want) => Err(format!("unknown mod {want}")),
        None => Ok(all.iter().map(|m| m.id.clone()).collect()),
    }
}

/// The `mod-state` reply for the mod `id`: its `world_data` table as JSON
/// (null when the game in place holds none), and `missing` naming the saved
/// tables this chunk does not read from the window thread.
pub fn mod_state_reply(id: &str, world_data: Option<Value>, missing: &[String]) -> Value {
    let mut reply = serde_json::json!({"ok": true, "id": id});
    reply["world_data"] = world_data.unwrap_or(Value::Null);
    reply["missing"] = Value::Array(missing.iter().map(|m| Value::String(m.clone())).collect());
    reply
}

/// The `reload-mods` reply for these statuses, in load order.
pub fn reload_reply(all: &[ModStatus]) -> Value {
    let mods = all
        .iter()
        .map(|m| {
            let mut v = serde_json::json!({"id": m.id, "ok": m.error.is_none()});
            v["reloads"] = m.reloads.into();
            if let Some(e) = &m.error {
                v["error"] = Value::String(e.clone());
            }
            if registries_of(&m.id) {
                v["note"] = Value::String("registry.lua changes need a restart".into());
            }
            v
        })
        .collect::<Vec<_>>();
    serde_json::json!({"ok": true, "mods": mods})
}

struct Pending {
    ids: Vec<String>,
    /// Each target's `writes` when the reload was asked.
    before: Vec<(String, u32)>,
    deadline: Instant,
    reply: Sender<String>,
}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// Whether every target's status was set since `before` (a load or a reload
/// finished, failed or not).
fn applied(ids: &[String], before: &[(String, u32)]) -> bool {
    ids.iter().all(|id| {
        before
            .iter()
            .find(|(i, _)| i == id)
            .is_some_and(|(_, n)| writes(id) != *n)
    })
}

/// Hold the reply until every target changes or 10 s pass (`Ok(None)` deferred).
pub fn start_reload_wait(id: Option<&str>, reply: Sender<String>) -> Result<Option<Value>, String> {
    if PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return Err("a reload is already running".to_owned());
    }
    let ids = reload_targets(id)?;
    let before = ids.iter().map(|id| (id.clone(), writes(id))).collect();
    request_reload(id);
    let deadline = Instant::now() + Duration::from_secs(10);
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
        ids,
        before,
        deadline,
        reply,
    });
    Ok(None)
}

/// Send the held reply once every target applied it (or after 10 s, a timeout).
pub fn poll_reload_wait() {
    poll_reload_wait_at(Instant::now());
}

/// `poll_reload_wait` at the time `now`: the deadline is the pending reload's,
/// so a test can pass a later time instead of waiting.
fn poll_reload_wait_at(now: Instant) {
    let ready = PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|p| applied(&p.ids, &p.before) || now >= p.deadline);
    if !ready {
        return;
    }
    let Some(p) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let all = status()
        .into_iter()
        .filter(|m| p.ids.contains(&m.id))
        .collect::<Vec<_>>();
    let reply = if applied(&p.ids, &p.before) {
        reload_reply(&all)
    } else {
        serde_json::json!({"error": "timed out after 10 s waiting for the reload", "kind": "timeout"})
    };
    let _ = p.reply.send(reply.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mod_status(id: &str, reloads: u32, error: Option<&str>) -> ModStatus {
        ModStatus {
            id: id.to_owned(),
            reloads,
            error: error.map(str::to_owned),
            ..Default::default()
        }
    }
    #[test]
    fn mod_state_reply_carries_world_data_and_missing() {
        let reply = mod_state_reply("my_mod", Some(serde_json::json!({"count": 1})), &[]);
        assert_eq!(
            reply,
            serde_json::json!({"ok": true, "id": "my_mod", "world_data": {"count": 1}, "missing": []})
        );
        let reply = mod_state_reply(
            "my_mod",
            None,
            &["world_data is not synced to this window".to_owned()],
        );
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["id"], "my_mod");
        assert!(reply["world_data"].is_null());
        assert_eq!(
            reply["missing"],
            serde_json::json!(["world_data is not synced to this window"])
        );
    }

    #[test]
    fn a_registry_mod_reload_reply_notes_the_restart_from_its_kept_flag() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        remember_registries("reg_mod", true);
        remember_registries("plain_mod", false);
        let reply = reload_reply(&[
            mod_status("reg_mod", 1, None),
            mod_status("plain_mod", 1, None),
        ]);
        assert_eq!(
            reply["mods"][0]["note"],
            "registry.lua changes need a restart"
        );
        assert!(reply["mods"][1].get("note").is_none());
        REGISTRIES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    #[test]
    fn a_reload_that_fails_the_same_way_is_applied() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        set_status(mod_status("same", 0, Some("init.lua: boom")));
        let (reply, got) = std::sync::mpsc::channel();
        start_reload_wait(Some("same"), reply).unwrap();
        // The reload runs and fails with the same text: neither the count
        // nor the error text changes, but the attempt is applied.
        set_status(mod_status("same", 0, Some("init.lua: boom")));
        poll_reload_wait();
        let v: Value = serde_json::from_str(&got.try_recv().unwrap()).unwrap();
        assert_eq!(v["mods"][0]["ok"], false);
        assert_eq!(v["mods"][0]["error"], "init.lua: boom");
        STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    #[test]
    fn a_reload_nobody_applies_times_out_after_ten_seconds() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        set_status(mod_status("slow", 0, None));
        let (reply, got) = std::sync::mpsc::channel();
        start_reload_wait(Some("slow"), reply).unwrap();
        poll_reload_wait_at(Instant::now());
        assert!(got.try_recv().is_err());
        poll_reload_wait_at(Instant::now() + Duration::from_secs(11));
        let v: Value = serde_json::from_str(&got.try_recv().unwrap()).unwrap();
        assert_eq!(v["kind"], "timeout");
        assert!(v["error"].as_str().unwrap().contains("10 s"));
        STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    #[test]
    fn reload_targets_and_reply() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_status(mod_status("only", 0, None));
        let ids = reload_targets(Some("only")).unwrap();
        assert_eq!(ids, vec!["only"]);
        let err = reload_targets(Some("missing")).unwrap_err();
        assert!(err.contains("missing"));
        STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let good = mod_status("good", 2, None);
        let bad = mod_status("bad", 1, Some("init.lua: boom"));
        let reply = reload_reply(&[good, bad]);
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["mods"][0]["ok"], true);
        assert_eq!(reply["mods"][1]["ok"], false);
        assert_eq!(reply["mods"][0]["reloads"], 2);
        assert_eq!(reply["mods"][1]["error"], "init.lua: boom");
    }
}
