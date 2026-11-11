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
    /// The side this status is for: "server" or "client", or "" when it has
    /// no side (a native mod, or a load that failed before any state ran).
    pub side: String,
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

/// Every scripted mod's status, in load order, one row per (id, side).
static STATUS: Mutex<Vec<ModStatus>> = Mutex::new(Vec::new());
/// Reloads asked for from the developer tools: one entry per (id, side) to
/// run, or everyone with a new generation of `RELOAD_ALL`. Served on the
/// server thread at the start of a tick.
static RELOAD: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());
static RELOAD_ALL: AtomicU32 = AtomicU32::new(0);

/// The scripted mods and what is known of them, for the developer tools.
pub fn status() -> Vec<ModStatus> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The status of the mod `id` in the side `side`, when one is kept.
pub fn status_of(id: &str, side: &str) -> Option<ModStatus> {
    STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|s| s.id == id && s.side == side)
        .cloned()
}

/// "both" asks for every side, so it is the same as no side at all.
fn norm_side(side: Option<&str>) -> Option<String> {
    match side {
        None | Some("") | Some("both") => None,
        Some(s) => Some(s.to_owned()),
    }
}

/// Ask the mod `id` (None: every mod) in the side `side` (None: both sides)
/// to read its scripts again and run `init.lua` afresh at the next tick of
/// that side, as `/reload` makes it do. Returns whether anything was queued:
/// a request matching no status row (an unknown id, or a side the mod has
/// no state for) queues nothing, so nobody has to drain it.
pub fn request_reload(id: Option<&str>, side: Option<&str>) -> bool {
    let side = norm_side(side);
    match (id, side) {
        (None, None) => {
            RELOAD_ALL.fetch_add(1, Ordering::Relaxed);
            true
        }
        (None, Some(s)) => {
            let rows = status();
            let matched: Vec<_> = rows.iter().filter(|r| r.side == s).collect();
            if matched.is_empty() {
                return false;
            }
            let mut list = RELOAD.lock().unwrap_or_else(|e| e.into_inner());
            for r in matched {
                let entry = (r.id.clone(), Some(s.clone()));
                if !list.contains(&entry) {
                    list.push(entry);
                }
            }
            true
        }
        (Some(id), None) => {
            let rows = status();
            let mut sides: Vec<String> = Vec::new();
            for r in rows.iter().filter(|r| r.id == id) {
                if !sides.contains(&r.side) {
                    sides.push(r.side.clone());
                }
            }
            // A row with no side has no state to reload.
            let live: Vec<String> = sides.into_iter().filter(|s| !s.is_empty()).collect();
            if live.is_empty() {
                return false;
            }
            let mut list = RELOAD.lock().unwrap_or_else(|e| e.into_inner());
            for s in live {
                let entry = (id.to_owned(), Some(s));
                if !list.contains(&entry) {
                    list.push(entry);
                }
            }
            true
        }
        (Some(id), Some(s)) => {
            if !status().iter().any(|r| r.id == id && r.side == s) {
                return false;
            }
            let mut list = RELOAD.lock().unwrap_or_else(|e| e.into_inner());
            let entry = (id.to_owned(), Some(s));
            if !list.contains(&entry) {
                list.push(entry);
            }
            true
        }
    }
}

/// How many times each mod's status was set, per (id, side): once per load
/// and per reload, failed or not. A reload is applied when its state's count
/// moves.
static WRITES: Mutex<Vec<((String, String), u32)>> = Mutex::new(Vec::new());

fn writes(id: &str, side: &str) -> u32 {
    WRITES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|((i, s), _)| i == id && s == side)
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
/// reloads or fails), one row per (id, side) in load order.
pub fn set_status(status: ModStatus) {
    remember_registries(&status.id, declares_registries(&status.source));
    {
        let mut w = WRITES.lock().unwrap_or_else(|e| e.into_inner());
        match w
            .iter_mut()
            .find(|((i, s), _)| *i == status.id && *s == status.side)
        {
            Some((_, n)) => *n += 1,
            None => w.push(((status.id.clone(), status.side.clone()), 1)),
        }
    }
    let mut all = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    match all
        .iter_mut()
        .find(|s| s.id == status.id && s.side == status.side)
    {
        Some(s) => *s = status,
        None => all.push(status),
    }
}

/// Forget every status (a test's own mods, so the next test starts empty).
#[cfg(test)]
pub(crate) fn clear_for_tests() {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Cross-crate test helper: forget statuses, reloads, writes and the pending
/// reload, so a console test can simulate a dedicated server (no client
/// states) without its rows depending on test order.
pub fn clear_all_for_test() {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    RELOAD.lock().unwrap_or_else(|e| e.into_inner()).clear();
    WRITES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    REGISTRIES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
    RELOAD_ALL.store(0, Ordering::Relaxed);
}

/// The last error of the mod `id` in the side `side`, kept in its status.
pub fn set_error(id: &str, side: &str, error: String) {
    if let Some(s) = STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter_mut()
        .find(|s| s.id == id && s.side == side)
    {
        s.error = Some(error);
    }
}

/// The generation of "reload every mod" the store is at.
pub fn reload_generation() -> u32 {
    RELOAD_ALL.load(Ordering::Relaxed)
}

/// Whether a reload was asked for the mod `id` in the side `side` since
/// `seen` last asked: the generation of "reload every mod", or the mod's
/// entry for this side in the list.
pub fn reload_asked(id: &str, side: &str, seen: &AtomicU32) -> bool {
    let all = RELOAD_ALL.load(Ordering::Relaxed);
    let mut asked = seen.swap(all, Ordering::Relaxed) != all;
    let mut list = RELOAD.lock().unwrap_or_else(|e| e.into_inner());
    let mut kept: Vec<(String, Option<String>)> = Vec::new();
    for (i, s) in list.iter() {
        let matches = i == id && (s.is_none() || s.as_deref() == Some(side));
        if !matches {
            kept.push((i.clone(), s.clone()));
            continue;
        }
        asked = true;
        if s.is_none() {
            let other = match side {
                "server" => "client",
                "client" => "server",
                _ => continue,
            };
            if status().iter().any(|m| m.id == id && m.side == other) {
                kept.push((id.to_owned(), Some(other.to_owned())));
            }
        }
    }
    *list = kept;
    asked
}

/// The (id, side) states a `reload-mods [ID]` reloads.
pub fn reload_targets(
    id: Option<&str>,
    side: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let side = norm_side(side);
    let all = status();
    match (id, side) {
        (Some(want), Some(s)) => {
            let found: Vec<(String, String)> = all
                .iter()
                .filter(|m| m.id == want && m.side == s)
                .map(|m| (m.id.clone(), m.side.clone()))
                .collect();
            if found.is_empty() {
                if all.iter().any(|m| m.id == want) {
                    Err(format!("mod {want} has no {s} state"))
                } else {
                    Err(format!("unknown mod {want}"))
                }
            } else {
                Ok(found)
            }
        }
        (Some(want), None) => {
            let found: Vec<(String, String)> = all
                .iter()
                .filter(|m| m.id == want)
                .map(|m| (m.id.clone(), m.side.clone()))
                .collect();
            if found.is_empty() {
                Err(format!("unknown mod {want}"))
            } else {
                Ok(found)
            }
        }
        (None, Some(s)) => Ok(all
            .iter()
            .filter(|m| m.side == s)
            .map(|m| (m.id.clone(), m.side.clone()))
            .collect()),
        (None, None) => Ok(all.iter().map(|m| (m.id.clone(), m.side.clone())).collect()),
    }
}

/// The `mod-state` reply for the mod `id`: the saved data the server thread
/// printed (`world_data`, `players`), beside `ok` and `id`.
pub fn mod_state_reply(id: &str, data: serde_json::Map<String, Value>) -> Value {
    let mut reply = data;
    reply.insert("ok".to_owned(), Value::Bool(true));
    reply.insert("id".to_owned(), Value::String(id.to_owned()));
    Value::Object(reply)
}

/// The `reload-mods` reply for these statuses, in load order.
pub fn reload_reply(all: &[ModStatus]) -> Value {
    let mods = all
        .iter()
        .map(|m| {
            let mut v = serde_json::json!({"id": m.id, "side": m.side, "ok": m.error.is_none()});
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
    targets: Vec<(String, String)>,
    /// Each target's `writes` when the reload was asked.
    before: Vec<((String, String), u32)>,
    deadline: Instant,
    reply: Sender<String>,
}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// Whether every target's status was set since `before` (a load or a reload
/// finished, failed or not).
fn applied(targets: &[(String, String)], before: &[((String, String), u32)]) -> bool {
    targets.iter().all(|(id, side)| {
        before
            .iter()
            .find(|(k, _)| k.0 == *id && k.1 == *side)
            .is_some_and(|(_, n)| writes(id, side) != *n)
    })
}

/// Hold the reply until every target changes or 10 s pass (`Ok(None)` deferred).
pub fn start_reload_wait(
    id: Option<&str>,
    side: Option<&str>,
    reply: Sender<String>,
) -> Result<Option<Value>, String> {
    if PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return Err("a reload is already running".to_owned());
    }
    let targets = reload_targets(id, side)?;
    let before = targets
        .iter()
        .map(|(id, side)| ((id.clone(), side.clone()), writes(id, side)))
        .collect();
    request_reload(id, side);
    let deadline = Instant::now() + Duration::from_secs(10);
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
        targets,
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
        .is_some_and(|p| applied(&p.targets, &p.before) || now >= p.deadline);
    if !ready {
        return;
    }
    let Some(p) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let all = status()
        .into_iter()
        .filter(|m| p.targets.contains(&(m.id.clone(), m.side.clone())))
        .collect::<Vec<_>>();
    let reply = if applied(&p.targets, &p.before) {
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
    fn mod_status_side(id: &str, side: &str, reloads: u32) -> ModStatus {
        ModStatus {
            id: id.to_owned(),
            side: side.to_owned(),
            reloads,
            ..Default::default()
        }
    }
    fn clear_store() {
        STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        RELOAD.lock().unwrap_or_else(|e| e.into_inner()).clear();
        WRITES.lock().unwrap_or_else(|e| e.into_inner()).clear();
        REGISTRIES.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
        RELOAD_ALL.store(0, Ordering::Relaxed);
    }
    #[test]
    fn mod_state_reply_is_the_saved_data_beside_ok_and_id() {
        let data =
            serde_json::json!({"world_data": {"counter": 42}, "players": {"u": {"coins": 3}}});
        let reply = mod_state_reply("my_mod", data.as_object().unwrap().clone());
        assert_eq!(
            reply,
            serde_json::json!({
                "ok": true,
                "id": "my_mod",
                "world_data": {"counter": 42},
                "players": {"u": {"coins": 3}},
            })
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
        clear_store();
        set_status(mod_status("same", 0, Some("init.lua: boom")));
        let (reply, got) = std::sync::mpsc::channel();
        start_reload_wait(Some("same"), None, reply).unwrap();
        // The reload runs and fails with the same text: neither the count
        // nor the error text changes, but the attempt is applied.
        set_status(mod_status("same", 0, Some("init.lua: boom")));
        poll_reload_wait();
        let v: Value = serde_json::from_str(&got.try_recv().unwrap()).unwrap();
        assert_eq!(v["mods"][0]["ok"], false);
        assert_eq!(v["mods"][0]["error"], "init.lua: boom");
        clear_store();
    }

    #[test]
    fn a_reload_nobody_applies_times_out_after_ten_seconds() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status("slow", 0, None));
        let (reply, got) = std::sync::mpsc::channel();
        start_reload_wait(Some("slow"), None, reply).unwrap();
        poll_reload_wait_at(Instant::now());
        assert!(got.try_recv().is_err());
        poll_reload_wait_at(Instant::now() + Duration::from_secs(11));
        let v: Value = serde_json::from_str(&got.try_recv().unwrap()).unwrap();
        assert_eq!(v["kind"], "timeout");
        assert!(v["error"].as_str().unwrap().contains("10 s"));
        clear_store();
    }

    #[test]
    fn reload_targets_and_reply() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status("only", 0, None));
        let ids = reload_targets(Some("only"), None).unwrap();
        assert_eq!(ids, vec![("only".to_owned(), String::new())]);
        let err = reload_targets(Some("missing"), None).unwrap_err();
        assert!(err.contains("missing"));
        clear_store();
        let good = mod_status("good", 2, None);
        let bad = mod_status("bad", 1, Some("init.lua: boom"));
        let reply = reload_reply(&[good, bad]);
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["mods"][0]["ok"], true);
        assert_eq!(reply["mods"][1]["ok"], false);
        assert_eq!(reply["mods"][0]["reloads"], 2);
        assert_eq!(reply["mods"][1]["error"], "init.lua: boom");
    }

    #[test]
    fn two_sides_have_two_rows_and_update_keeps_the_other() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("dual", "server", 0));
        set_status(mod_status_side("dual", "client", 0));
        let rows = status();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "dual");
        assert_eq!(rows[0].side, "server");
        assert_eq!(rows[1].id, "dual");
        assert_eq!(rows[1].side, "client");
        set_error("dual", "client", "client boom".to_owned());
        assert_eq!(
            status_of("dual", "server").unwrap().error,
            None,
            "server row keeps its own error"
        );
        assert_eq!(
            status_of("dual", "client").unwrap().error,
            Some("client boom".to_owned())
        );
        set_status(mod_status_side("dual", "server", 1));
        assert_eq!(status().len(), 2, "update in place, not a new row");
        assert_eq!(status_of("dual", "server").unwrap().reloads, 1);
        assert_eq!(
            status_of("dual", "client").unwrap().error,
            Some("client boom".to_owned()),
            "client row untouched by server update"
        );
        clear_store();
    }

    #[test]
    fn a_client_reload_is_seen_only_by_the_client() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("solo", "server", 0));
        set_status(mod_status_side("solo", "client", 0));
        let seen_server = AtomicU32::new(reload_generation());
        let seen_client = AtomicU32::new(reload_generation());
        request_reload(Some("solo"), Some("client"));
        assert!(reload_asked("solo", "client", &seen_client));
        assert!(!reload_asked("solo", "client", &seen_client));
        assert!(!reload_asked("solo", "server", &seen_server));
        clear_store();
    }

    #[test]
    fn a_no_side_reload_is_seen_by_both_sides() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("both", "server", 0));
        set_status(mod_status_side("both", "client", 0));
        let seen_server = AtomicU32::new(reload_generation());
        let seen_client = AtomicU32::new(reload_generation());
        request_reload(Some("both"), None);
        assert!(reload_asked("both", "server", &seen_server));
        assert!(!reload_asked("both", "server", &seen_server));
        assert!(reload_asked("both", "client", &seen_client));
        assert!(!reload_asked("both", "client", &seen_client));
        clear_store();
    }

    #[test]
    fn a_reload_all_is_seen_by_both_sides() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("allm", "server", 0));
        set_status(mod_status_side("allm", "client", 0));
        let seen_server = AtomicU32::new(reload_generation());
        let seen_client = AtomicU32::new(reload_generation());
        request_reload(None, None);
        assert!(reload_asked("allm", "server", &seen_server));
        assert!(!reload_asked("allm", "server", &seen_server));
        assert!(reload_asked("allm", "client", &seen_client));
        assert!(!reload_asked("allm", "client", &seen_client));
        clear_store();
    }

    #[test]
    fn a_no_side_reload_is_applied_only_when_both_states_move() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("paired", "server", 0));
        set_status(mod_status_side("paired", "client", 0));
        let (reply, got) = std::sync::mpsc::channel();
        start_reload_wait(Some("paired"), None, reply).unwrap();
        set_status(mod_status_side("paired", "server", 1));
        poll_reload_wait();
        assert!(
            got.try_recv().is_err(),
            "only the server moved: not applied yet"
        );
        set_status(mod_status_side("paired", "client", 1));
        poll_reload_wait();
        let v: Value = serde_json::from_str(&got.try_recv().unwrap()).unwrap();
        assert_eq!(v["mods"].as_array().unwrap().len(), 2);
        assert!(
            v["mods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == "paired" && m["side"] == "server")
        );
        assert!(
            v["mods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == "paired" && m["side"] == "client")
        );
        clear_store();
    }

    #[test]
    fn two_states_have_one_reply_row_per_side() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("dual", "server", 2));
        set_status(mod_status_side("dual", "client", 3));
        set_status(mod_status_side("only_client", "client", 1));
        assert_eq!(
            reload_targets(Some("dual"), Some("client")).unwrap(),
            vec![("dual".to_owned(), "client".to_owned())]
        );
        assert_eq!(
            reload_targets(Some("dual"), Some("both")).unwrap(),
            vec![
                ("dual".to_owned(), "server".to_owned()),
                ("dual".to_owned(), "client".to_owned())
            ]
        );
        let reply = reload_reply(&status());
        let rows = reply["mods"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .any(|m| m["id"] == "dual" && m["side"] == "server" && m["reloads"] == 2)
        );
        assert!(
            rows.iter()
                .any(|m| m["id"] == "dual" && m["side"] == "client" && m["reloads"] == 3)
        );
        assert_eq!(
            reload_targets(Some("only_client"), Some("server")).unwrap_err(),
            "mod only_client has no server state"
        );
        assert_eq!(
            reload_targets(Some("ghost"), Some("client")).unwrap_err(),
            "unknown mod ghost"
        );
        clear_store();
    }

    #[test]
    fn a_ghost_reload_is_not_queued() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("real", "server", 0));
        set_status(mod_status_side("real", "client", 0));
        set_status(mod_status_side("only_server", "server", 0));
        // An unknown id queues nothing: nobody ever drains it.
        assert!(!request_reload(Some("ghost"), Some("client")));
        let seen = AtomicU32::new(reload_generation());
        assert!(
            !reload_asked("ghost", "client", &seen),
            "ghost leaves RELOAD unchanged"
        );
        assert!(!request_reload(Some("ghost"), None));
        let seen = AtomicU32::new(reload_generation());
        assert!(!reload_asked("ghost", "server", &seen));
        assert!(!reload_asked("ghost", "client", &seen));
        // A side the mod has no state for queues nothing.
        assert!(!request_reload(Some("only_server"), Some("client")));
        let seen = AtomicU32::new(reload_generation());
        assert!(
            !reload_asked("only_server", "client", &seen),
            "a side with no state queues nothing"
        );
        // A real request queues.
        assert!(request_reload(Some("real"), Some("server")));
        let seen = AtomicU32::new(reload_generation());
        assert!(reload_asked("real", "server", &seen));
        assert!(!reload_asked("real", "server", &seen));
        clear_store();
    }

    #[test]
    fn repeated_requests_do_not_duplicate_the_reload_list() {
        let _g = super::super::console::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        clear_store();
        set_status(mod_status_side("m", "server", 0));
        set_status(mod_status_side("m", "client", 0));
        // Three identical requests leave one entry.
        assert!(request_reload(Some("m"), Some("client")));
        assert!(request_reload(Some("m"), Some("client")));
        assert!(request_reload(Some("m"), Some("client")));
        assert_eq!(
            RELOAD.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1,
            "duplicate (id, side) is not queued twice"
        );
        let seen = AtomicU32::new(reload_generation());
        assert!(reload_asked("m", "client", &seen));
        assert!(!reload_asked("m", "client", &seen));
        // The list is drained once.
        assert_eq!(
            RELOAD.lock().unwrap_or_else(|e| e.into_inner()).len(),
            0,
            "reload_asked drains the single entry"
        );
        clear_store();
    }
}
