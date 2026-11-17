//! The Lua mods as one built-in game mod.
//!
//! The game reads `Hooks::subscribed` once, at install time, and then calls only the hook methods
//! that a mod subscribed to. `LuaHost` is that one mod: it holds every loaded `LuaMod` and, when a
//! hook fires, runs each handler the mods registered for it. A mod's failure stays in that mod:
//! its error is logged, its handler is stopped by a time budget, and the others still run.

use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use horizon_client::save::level_mods::{self, ModRecord};
use mlua::{Error, Function, Lua, Table, Value, VmState};

use crate::declare;
use crate::discover::{Discovered, ENTRY, Side};
use crate::state;

/// The hook a Lua mod may use for each server tick.
pub const SERVER_TICK: &str = "server_tick";
/// The largest `main.luau` that is read.
pub const MAX_SOURCE: u64 = 1024 * 1024;
/// The lines kept per mod.
pub const MAX_LOG_LINES: usize = 100;
/// The characters kept per log line.
pub const MAX_LOG_LINE: usize = 1000;
/// The time one handler may run for one hook call. A handler that runs longer is stopped, so one
/// mod cannot hold up the server's tick or the handlers of the other mods.
pub const HANDLER_BUDGET: Duration = Duration::from_millis(50);

const LATE_ON: &str = "horizon.on can only be used while the mod loads";
const LOG_TARGET: &str = "event_horizon_lua";

/// The game's hook id for a hook name a Lua mod may use. Only `server_tick` for now.
pub fn hook_id(name: &str) -> Option<mod_api::HookId> {
    match name {
        SERVER_TICK => Some(mod_api::HookId::ServerTick),
        _ => None,
    }
}

/// One loaded Lua mod: its own sandboxed state, the hooks it declared, and its log lines.
pub struct LuaMod {
    id: String,
    version: String,
    side: Side,
    hooks: Vec<String>,
    lua: Lua,
    log: Arc<Mutex<Vec<String>>>,
}

impl LuaMod {
    /// Loads the mod in `found`: reads its `main.luau`, declares its hooks, and installs the
    /// runtime `horizon` table in place of the declaring one. Any failure is an error.
    pub fn load(found: &Discovered) -> Result<LuaMod, String> {
        let source = read_source(&found.dir)?;
        let lua = state::new_state().map_err(|e| e.to_string())?;
        let chunk_name = format!("@{}", found.dir.join(ENTRY).display());
        let declaration = declare::declare(&lua, &source, &chunk_name, &|n| hook_id(n).is_some())?;

        let log = Arc::new(Mutex::new(Vec::new()));
        let runtime =
            runtime_table(&lua, &found.id, Arc::clone(&log)).map_err(|e| e.to_string())?;
        declare::environment(&lua)
            .and_then(|env| env.raw_set("horizon", runtime))
            .map_err(|e| e.to_string())?;

        Ok(LuaMod {
            id: found.id.clone(),
            version: found.version.clone(),
            side: found.side,
            hooks: declaration.hooks,
            lua,
            log,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn side(&self) -> Side {
        self.side
    }

    pub fn hooks(&self) -> &[String] {
        &self.hooks
    }

    /// The lines `horizon.log` wrote, oldest first.
    pub fn log_lines(&self) -> Vec<String> {
        lock(&self.log).clone()
    }
}

/// `existing` (the code mods already recorded, in their order), then one record for each Lua mod
/// that runs on the server (`Server` or `Both`), in load order. An id already in the list is not
/// added again, so merging the same mods twice changes nothing. A client-only mod does not
/// affect a world, so it is not recorded.
pub fn merged_records(existing: Option<Vec<ModRecord>>, mods: &[LuaMod]) -> Vec<ModRecord> {
    let mut records = existing.unwrap_or_default();
    for m in mods
        .iter()
        .filter(|m| matches!(m.side, Side::Server | Side::Both))
    {
        if !records.iter().any(|r| r.id == m.id) {
            records.push(ModRecord {
                id: m.id.clone(),
                version: m.version.clone(),
            });
        }
    }
    records
}

/// The one `Hooks` implementation the game installs: it fans each hook out to the Lua mods.
pub struct LuaHost {
    mods: Vec<LuaMod>,
    subscribed: &'static [Option<mod_api::HookId>],
}

impl LuaHost {
    pub fn new(mods: Vec<LuaMod>) -> LuaHost {
        let mut seen = Vec::<usize>::new();
        let mut subscribed = Vec::<Option<mod_api::HookId>>::new();
        for name in mods.iter().flat_map(|m| m.hooks.iter()) {
            let Some(id) = hook_id(name) else { continue };
            if !seen.contains(&(id as usize)) {
                seen.push(id as usize);
                subscribed.push(Some(id));
            }
        }
        LuaHost {
            mods,
            subscribed: Box::leak(subscribed.into_boxed_slice()),
        }
    }

    /// Adds the server-side Lua mods to the code mods the world records (`level_mods::set_loaded`),
    /// so a world saved with them remembers them. Nothing changes when there are no such mods
    /// and the list was never set.
    pub fn record_loaded(&self) {
        let existing = level_mods::loaded();
        let merged = merged_records(existing.clone(), &self.mods);
        if existing.is_none() && merged.is_empty() {
            return;
        }
        level_mods::set_loaded(Some(merged));
    }

    /// Calls every handler of `hook` in every mod that declared it. A client-only mod is skipped
    /// for the server hooks. A failing handler is logged and does not stop the others.
    pub fn fire(&self, hook: &str) {
        for m in &self.mods {
            if hook == SERVER_TICK && m.side == Side::Client {
                continue;
            }
            let handlers = match declare::handlers(&m.lua, hook) {
                Ok(handlers) => handlers,
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id);
                    continue;
                }
            };
            for handler in &handlers {
                if let Err(err) = call_handler(&m.lua, handler) {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id);
                }
            }
        }
    }
}

impl mod_api::Hooks for LuaHost {
    fn subscribed(&self) -> &'static [Option<mod_api::HookId>] {
        self.subscribed
    }

    fn register(&self, _r: &mut dyn mod_api::Registrar, _cx: &mod_api::RegistrationContext) {
        self.record_loaded();
    }

    fn server_tick(
        &self,
        _host: &mut dyn mod_api::ServerHost,
        _event: &mut mod_api::ServerTickEvent<'_>,
    ) {
        self.fire(SERVER_TICK);
    }
}

/// Runs one handler under `HANDLER_BUDGET`. The interrupt keeps failing after the deadline, so a
/// `pcall` inside the handler cannot swallow the stop.
fn call_handler(lua: &Lua, handler: &Function) -> mlua::Result<()> {
    let deadline = Instant::now() + HANDLER_BUDGET;
    lua.set_interrupt(move |_| {
        if Instant::now() >= deadline {
            Err(Error::runtime("the handler ran over its time budget"))
        } else {
            Ok(VmState::Continue)
        }
    });
    let result = handler.call::<()>(());
    lua.remove_interrupt();
    result
}

/// The `horizon` table a mod's handlers see: `log`, and nothing else yet. Any other field errors,
/// and `on` only refuses, because hooks are declared while the mod loads.
fn runtime_table(lua: &Lua, id: &str, log: Arc<Mutex<Vec<String>>>) -> mlua::Result<Table> {
    let horizon = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set(
        "__index",
        lua.create_function(|_, (_, field): (Value, Value)| -> mlua::Result<Value> {
            let name = match field {
                Value::String(s) => s.to_string_lossy(),
                other => other.type_name().to_string(),
            };
            Err(Error::runtime(format!(
                "horizon.{name} is not available yet"
            )))
        })?,
    )?;
    horizon.set_metatable(Some(meta))?;
    horizon.raw_set(
        "on",
        lua.create_function(|_, _: mlua::MultiValue| -> mlua::Result<()> {
            Err(Error::runtime(LATE_ON))
        })?,
    )?;
    let id = id.to_string();
    horizon.raw_set(
        "log",
        lua.create_function(move |_, message: Value| -> mlua::Result<()> {
            let Value::String(message) = message else {
                return Err(Error::runtime("horizon.log needs the message as a string"));
            };
            let line: String = message
                .to_string_lossy()
                .chars()
                .take(MAX_LOG_LINE)
                .collect();
            {
                let mut lines = lock(&log);
                lines.push(line.clone());
                if lines.len() > MAX_LOG_LINES {
                    let excess = lines.len() - MAX_LOG_LINES;
                    lines.drain(..excess);
                }
            }
            log::info!(target: LOG_TARGET, "{id}: {line}");
            Ok(())
        })?,
    )?;
    horizon.set_readonly(true);
    Ok(horizon)
}

/// Reads a mod's `main.luau`, reading at most `MAX_SOURCE + 1` bytes. A symlink is refused.
fn read_source(dir: &Path) -> Result<String, String> {
    let path = dir.join(ENTRY);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| format!("no {ENTRY}"))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(format!("{ENTRY} is not a plain file"));
    }
    let file = std::fs::File::open(&path).map_err(|_| format!("{ENTRY} cannot be read"))?;
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{ENTRY} cannot be read"))?;
    if bytes.len() as u64 > MAX_SOURCE {
        return Err(format!("{ENTRY} is over {} KiB", MAX_SOURCE / 1024));
    }
    String::from_utf8(bytes).map_err(|_| format!("{ENTRY} is not UTF-8"))
}

/// The log buffer's lock. A poisoned lock still holds the lines, so it is taken anyway.
fn lock(log: &Mutex<Vec<String>>) -> MutexGuard<'_, Vec<String>> {
    log.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A mod folder with `source` as its main.luau; returns what discovery would have reported.
    fn mod_folder(root: &Path, name: &str, source: &str, side: Side) -> Discovered {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), source).unwrap();
        Discovered {
            dir,
            id: name.to_string(),
            version: "1".to_string(),
            side,
        }
    }

    fn load_one(source: &str) -> Result<LuaMod, String> {
        let root = tempfile::tempdir().unwrap();
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        LuaMod::load(&found)
    }

    #[test]
    fn a_loaded_mod_declares_its_hooks() {
        let root = tempfile::tempdir().unwrap();
        let found = mod_folder(
            root.path(),
            "mod_a",
            "horizon.on(\"server_tick\", function() horizon.log(\"tick\") end)",
            Side::Both,
        );
        let loaded = LuaMod::load(&found).unwrap();
        assert_eq!(loaded.hooks(), ["server_tick"]);
        assert_eq!(loaded.id(), "mod_a");
        assert_eq!(loaded.side(), Side::Both);
    }

    #[test]
    fn the_host_subscribes_to_the_union_once() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"server_tick\", function() end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", source, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", source, Side::Server)).unwrap();
        let host = LuaHost::new(vec![a, b]);
        let subscribed = mod_api::Hooks::subscribed(&host);
        assert_eq!(subscribed.len(), 1);
        assert_eq!(
            subscribed[0].map(|id| id as usize),
            Some(mod_api::HookId::ServerTick as usize)
        );
    }

    #[test]
    fn fire_runs_the_handlers() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"server_tick\", function() horizon.log(\"tick\") end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(SERVER_TICK);
        assert_eq!(lock(&log).clone(), ["tick"]);
        host.fire(SERVER_TICK);
        assert_eq!(lock(&log).clone(), ["tick", "tick"]);
    }

    #[test]
    fn an_unknown_hook_in_main_luau_refuses_the_mod() {
        let err = load_one("horizon.on(\"nope\", function() end)")
            .err()
            .unwrap();
        assert!(err.contains("unknown hook 'nope'"), "got {err}");
    }

    #[test]
    fn a_client_mod_is_not_fired_for_a_server_hook() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"server_tick\", function() horizon.log(\"tick\") end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Client);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(SERVER_TICK);
        assert!(lock(&log).is_empty());
    }

    #[test]
    fn a_failing_handler_does_not_stop_the_others() {
        let root = tempfile::tempdir().unwrap();
        let failing = "horizon.on(\"server_tick\", function() error(\"boom\") end)";
        let logging = "horizon.on(\"server_tick\", function() horizon.log(\"still here\") end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", failing, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", logging, Side::Both)).unwrap();
        let b_log = Arc::clone(&b.log);
        let host = LuaHost::new(vec![a, b]);
        host.fire(SERVER_TICK);
        assert_eq!(lock(&b_log).clone(), ["still here"]);
    }

    #[test]
    fn a_runaway_handler_is_stopped_and_the_others_run() {
        let root = tempfile::tempdir().unwrap();
        let runaway = "horizon.on(\"server_tick\", function() while true do end end)";
        let logging = "horizon.on(\"server_tick\", function() horizon.log(\"ran\") end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", runaway, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", logging, Side::Both)).unwrap();
        let b_log = Arc::clone(&b.log);
        let host = LuaHost::new(vec![a, b]);
        let started = Instant::now();
        host.fire(SERVER_TICK);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(lock(&b_log).clone(), ["ran"]);
    }

    #[test]
    fn horizon_log_is_not_available_while_the_mod_loads() {
        let err = load_one("horizon.log(\"x\")").err().unwrap();
        assert!(
            err.contains("not available while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn a_runtime_field_that_does_not_exist_errors() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"server_tick\", function()\n\
                      local ok = pcall(function() return horizon.world end)\n\
                      horizon.log(tostring(ok))\n\
                      end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(SERVER_TICK);
        assert_eq!(lock(&log).clone(), ["false"]);
    }

    #[test]
    fn an_oversized_source_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = "-".repeat(MAX_SOURCE as usize + 1);
        let found = mod_folder(root.path(), "mod_a", &source, Side::Both);
        let err = LuaMod::load(&found).err().unwrap();
        assert!(err.contains("over"), "got {err}");
    }

    #[test]
    fn the_log_keeps_the_newest_lines_and_cuts_long_ones() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"server_tick\", function()\n\
                      for i = 1, 149 do horizon.log(\"line \" .. i) end\n\
                      horizon.log(string.rep(\"x\", 2000))\n\
                      end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let loaded = LuaMod::load(&found).unwrap();
        let host = LuaHost::new(vec![loaded]);
        host.fire(SERVER_TICK);
        let lines = host.mods[0].log_lines();
        assert_eq!(lines.len(), MAX_LOG_LINES);
        assert_eq!(lines[0], "line 51");
        assert_eq!(lines[MAX_LOG_LINES - 1].chars().count(), MAX_LOG_LINE);
    }

    #[test]
    fn a_missing_main_luau_is_an_error() {
        let found = Discovered {
            dir: PathBuf::from("/nonexistent/eh-lua-mod"),
            id: "mod_a".into(),
            version: "1".into(),
            side: Side::Both,
        };
        assert!(LuaMod::load(&found).is_err());
    }

    #[test]
    fn pcall_cannot_swallow_a_handlers_budget() {
        // The interrupt keeps failing after the deadline, so catching one error does not help.
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"server_tick\", function() while true do pcall(function() while true do end end) end end)";
        let m = LuaMod::load(&mod_folder(root.path(), "stubborn", source, Side::Both)).unwrap();
        let host = LuaHost::new(vec![m]);
        let started = Instant::now();
        host.fire(SERVER_TICK);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A loaded mod with `id`, `version` and `side`, with no hooks.
    fn lua_mod(root: &Path, id: &str, version: &str, side: Side) -> LuaMod {
        let mut found = mod_folder(root, id, "", side);
        found.version = version.to_string();
        LuaMod::load(&found).unwrap()
    }

    fn record(id: &str, version: &str) -> ModRecord {
        ModRecord {
            id: id.into(),
            version: version.into(),
        }
    }

    #[test]
    fn server_and_both_mods_are_added_and_client_mods_are_not() {
        let root = tempfile::tempdir().unwrap();
        let mods = [
            lua_mod(root.path(), "client_mod", "1.0.0", Side::Client),
            lua_mod(root.path(), "server_mod", "2.0.0", Side::Server),
            lua_mod(root.path(), "both_mod", "3.0.0", Side::Both),
        ];
        assert_eq!(
            merged_records(None, &mods),
            [record("server_mod", "2.0.0"), record("both_mod", "3.0.0")]
        );
    }

    #[test]
    fn existing_records_come_first_and_ids_are_not_repeated() {
        let root = tempfile::tempdir().unwrap();
        let mods = [
            lua_mod(root.path(), "alpha_mod", "1.0.0", Side::Both),
            lua_mod(root.path(), "beta_mod", "2.0.0", Side::Server),
        ];
        let existing = Some(vec![
            record("native_mod", "0.1.0"),
            record("alpha_mod", "1.0.0"),
        ]);
        assert_eq!(
            merged_records(existing, &mods),
            [
                record("native_mod", "0.1.0"),
                record("alpha_mod", "1.0.0"),
                record("beta_mod", "2.0.0"),
            ]
        );
    }

    #[test]
    fn merging_twice_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let mods = [
            lua_mod(root.path(), "alpha_mod", "1.0.0", Side::Both),
            lua_mod(root.path(), "beta_mod", "2.0.0", Side::Server),
        ];
        let first = merged_records(None, &mods);
        assert_eq!(merged_records(Some(first.clone()), &mods), first);
    }

    /// The loaded list is the process's, so this runs under nextest (one process per test), where
    /// no other test sees it.
    #[test]
    fn record_loaded_updates_the_process_list() {
        let root = tempfile::tempdir().unwrap();
        let previous = level_mods::set_loaded(None);
        let host = LuaHost::new(vec![lua_mod(root.path(), "both_mod", "3.0.0", Side::Both)]);
        host.record_loaded();
        let loaded = level_mods::loaded().unwrap();
        assert!(loaded.contains(&record("both_mod", "3.0.0")));
        level_mods::set_loaded(previous);
    }

    #[test]
    fn a_second_record_loaded_is_a_no_op() {
        let root = tempfile::tempdir().unwrap();
        let previous = level_mods::set_loaded(None);
        let host = LuaHost::new(vec![lua_mod(root.path(), "both_mod", "3.0.0", Side::Both)]);
        host.record_loaded();
        host.record_loaded();
        let loaded = level_mods::loaded().unwrap();
        assert_eq!(loaded.iter().filter(|m| m.id == "both_mod").count(), 1);
        level_mods::set_loaded(previous);
    }
}
