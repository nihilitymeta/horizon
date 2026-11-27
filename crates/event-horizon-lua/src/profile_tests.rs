//! Tests for the Lua profiler: what the counters hold after handler calls, reset, the default
//! (off), the bound on hooks per mod, and the `mc.profile()` rows. The store is process-wide, so
//! every test holds `LOCK` and leaves recording off.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use mlua::{Lua, Value};

use super::*;
use crate::console::profile_table;
use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::new_event;
use crate::host::{LuaHost, LuaMod};

/// Serialises the tests of this file, since they share the store and the switch.
pub(crate) static LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn guard() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Loads a mod named `id` whose main.luau is `source`, on both sides.
fn load(root: &std::path::Path, id: &str, source: &str) -> LuaMod {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ENTRY), source).unwrap();
    LuaMod::load(&Discovered {
        dir,
        id: id.to_string(),
        version: "1".to_string(),
        side: Side::Both,
    })
    .unwrap()
}

/// The row of mod `id` for `hook`, if it was recorded.
fn row(id: &str, hook: &str) -> Option<ModProfile> {
    profile_snapshot()
        .into_iter()
        .find(|r| r.mod_id == id && r.hook == hook)
}

/// Fires `hook` once for `host`, with an empty event.
fn fire(host: &LuaHost, hook: &str) {
    host.dispatch(
        hook,
        &|lua| new_event(lua, lua.create_table()?, &[]),
        &mut |_, _| true,
    );
}

#[test]
fn timed_calls_count_time_max_and_errors() {
    let _g = guard();
    profile_reset();
    profile_enable(true);
    let slow = timed("profile_timed", "hook_slow", || {
        std::thread::sleep(Duration::from_millis(5));
        Ok::<(), mlua::Error>(())
    });
    let failed = timed("profile_timed", "hook_slow", || {
        Err::<(), mlua::Error>(mlua::Error::runtime("boom"))
    });
    profile_enable(false);
    assert!(slow.is_ok());
    assert!(failed.is_err());
    let r = row("profile_timed", "hook_slow").expect("the hook is recorded");
    assert_eq!(r.calls, 2);
    assert_eq!(r.errors, 1);
    assert!(r.total_ns >= 5_000_000, "total {} ns", r.total_ns);
    assert!(r.max_ns >= 5_000_000, "max {} ns", r.max_ns);
    assert!(r.max_ns <= r.total_ns, "max is one of the calls' times");
    profile_reset();
}

#[test]
fn dispatch_counts_a_failing_handler_as_an_error() {
    let _g = guard();
    profile_reset();
    profile_enable(true);
    let root = tempfile::tempdir().unwrap();
    let source = "horizon.on(\"block_receive_game\", function(event) error(\"boom\") end)";
    let host = LuaHost::new(vec![load(root.path(), "profile_dispatch", source)]);
    fire(&host, "block_receive_game");
    fire(&host, "block_receive_game");
    profile_enable(false);
    let r = row("profile_dispatch", "block_receive_game").expect("the handler is recorded");
    assert_eq!(r.calls, 2);
    assert_eq!(r.errors, 2);
    profile_reset();
}

#[test]
fn reset_clears_every_counter() {
    let _g = guard();
    profile_reset();
    profile_enable(true);
    let _ = timed("profile_reset_mod", "hook_a", || Ok::<(), mlua::Error>(()));
    profile_enable(false);
    assert!(row("profile_reset_mod", "hook_a").is_some());
    profile_reset();
    assert!(row("profile_reset_mod", "hook_a").is_none());
}

#[test]
fn recording_is_off_by_default_and_records_nothing() {
    let _g = guard();
    profile_reset();
    profile_enable(false);
    assert!(!profile_enabled());
    let root = tempfile::tempdir().unwrap();
    let source = "horizon.on(\"block_receive_game\", function(event) error(\"boom\") end)";
    let host = LuaHost::new(vec![load(root.path(), "profile_off", source)]);
    fire(&host, "block_receive_game");
    assert!(row("profile_off", "block_receive_game").is_none());
    assert!(profile_snapshot().iter().all(|r| r.mod_id != "profile_off"));
}

#[test]
fn a_mod_keeps_at_most_256_hooks() {
    let _g = guard();
    profile_reset();
    profile_enable(true);
    for i in 0..300 {
        let hook = format!("hook_{i}");
        let _ = timed("profile_bound", &hook, || Ok::<(), mlua::Error>(()));
    }
    // A call on a hook already kept still counts once the mod is full.
    let _ = timed("profile_bound", "hook_0", || Ok::<(), mlua::Error>(()));
    profile_enable(false);
    let rows: Vec<ModProfile> = profile_snapshot()
        .into_iter()
        .filter(|r| r.mod_id == "profile_bound")
        .collect();
    assert_eq!(rows.len(), MAX_HOOKS_PER_MOD);
    assert_eq!(row("profile_bound", "hook_0").map(|r| r.calls), Some(2));
    assert!(row("profile_bound", "hook_299").is_none());
    profile_reset();
}

#[test]
fn mc_profile_returns_the_rows_as_a_lua_list() {
    let _g = guard();
    profile_reset();
    profile_enable(true);
    let _ = timed("profile_lua", "hook_x", || {
        Err::<(), mlua::Error>(mlua::Error::runtime("boom"))
    });
    profile_enable(false);
    let lua = Lua::new();
    let rows = profile_table(&lua).unwrap();
    let first: mlua::Table = rows.get(1).unwrap();
    assert_eq!(first.get::<String>("mod").unwrap(), "profile_lua");
    assert_eq!(first.get::<String>("hook").unwrap(), "hook_x");
    assert_eq!(first.get::<u64>("calls").unwrap(), 1);
    assert_eq!(first.get::<u64>("errors").unwrap(), 1);
    assert!(matches!(
        first.get::<Value>("total_ms").unwrap(),
        Value::Number(_)
    ));
    assert!(matches!(
        first.get::<Value>("max_ms").unwrap(),
        Value::Number(_)
    ));
    profile_reset();
}
