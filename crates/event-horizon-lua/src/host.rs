//! The Lua mods as one built-in game mod.
//!
//! The game reads `Hooks::subscribed` once, at install time, and then calls only the hook methods
//! that a mod subscribed to. `LuaHost` is that one mod: it holds every loaded `LuaMod` and, when a
//! hook fires, runs each handler the mods registered for it. A mod's failure stays in that mod:
//! its error is logged, its handler is stopped by a time budget, and the others still run.

use std::cmp::Reverse;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use horizon_client::save::level_mods::{self, ModRecord};
use mlua::{Error, Function, Lua, Table, Value};

use crate::budget;
use crate::declare::{self, ListenerOptions};
use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::new_event;
use crate::generated::hooks::{hook_id, is_emitted};
use crate::ratelimit::{RateLimit, Verdict};
use crate::state;

/// The largest `main.luau` that is read.
pub const MAX_SOURCE: u64 = 1024 * 1024;
/// The lines kept per mod.
pub const MAX_LOG_LINES: usize = 100;
/// The characters kept per log line.
pub const MAX_LOG_LINE: usize = 1000;
/// The time one handler may run for one hook call. A handler that runs longer is stopped, so one
/// mod cannot hold up the server's tick or the handlers of the other mods.
pub const HANDLER_BUDGET: Duration = Duration::from_millis(50);
/// The log lines one mod may write at once.
pub const LOG_BURST: u32 = 20;
/// The log lines one mod may add each second, after its burst.
pub const LOG_PER_SECOND: u32 = 20;

const LATE_ON: &str = "horizon.on can only be used while the mod loads";
pub(crate) const LOG_TARGET: &str = "event_horizon_lua";

/// One loaded Lua mod: its own sandboxed state, the hooks it declared, its log lines, and the turn
/// that serialises the threads calling into its state.
pub struct LuaMod {
    id: String,
    version: String,
    side: Side,
    hooks: Vec<String>,
    options: Vec<(String, ListenerOptions)>,
    lua: Lua,
    log: Arc<Mutex<Vec<String>>>,
    turn: Turn,
}

/// A per-mod turn: one thread at a time runs a mod's handlers. The same thread may enter again
/// (a call that re-enters the mod does not wait for itself), and another thread waits until the
/// turn is free. std has no such lock, so it is built from a mutex, its holder and a condvar.
#[derive(Default)]
pub(crate) struct Turn {
    holder: Mutex<Option<(ThreadId, usize)>>,
    free: Condvar,
}

/// Holds a mod's turn; the turn is given back when the guard drops, also when the thread panics.
pub(crate) struct TurnGuard<'a> {
    turn: &'a Turn,
}

impl Turn {
    pub(crate) fn enter(&self) -> TurnGuard<'_> {
        let me = thread::current().id();
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match *holder {
                None => {
                    *holder = Some((me, 1));
                    break;
                }
                Some((who, depth)) if who == me => {
                    *holder = Some((me, depth + 1));
                    break;
                }
                Some(_) => {
                    holder = self
                        .free
                        .wait(holder)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        TurnGuard { turn: self }
    }
}

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        let mut holder = self
            .turn
            .holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((who, depth)) = *holder {
            *holder = (depth > 1).then_some((who, depth - 1));
        }
        self.turn.free.notify_all();
    }
}

impl LuaMod {
    /// Loads the mod in `found`: reads its `main.luau`, declares its hooks, and installs the
    /// runtime `horizon` table in place of the declaring one. Any failure is an error.
    pub fn load(found: &Discovered) -> Result<LuaMod, String> {
        let source = read_source(&found.dir)?;
        let lua = state::new_state().map_err(|e| e.to_string())?;
        let chunk_name = format!("@{}", found.dir.join(ENTRY).display());
        let declaration = declare::declare(&lua, &source, &chunk_name, &|n| is_emitted(n))?;

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
            options: declaration.options,
            lua,
            log,
            turn: Turn::default(),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The mod's sandboxed state. Callers hold `turn()` while they use it.
    pub(crate) fn lua(&self) -> &Lua {
        &self.lua
    }

    /// The turn that serialises the threads calling into this mod's state.
    pub(crate) fn turn(&self) -> &Turn {
        &self.turn
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

    /// The options this mod declared for `hook`, or the defaults when it did not declare the hook.
    pub fn options(&self, hook: &str) -> ListenerOptions {
        self.options
            .iter()
            .find(|(name, _)| name == hook)
            .map(|(_, options)| *options)
            .unwrap_or_default()
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
///
/// The game sees this host as ONE listener per hook, so `listeners` gives the game the highest
/// priority any Lua mod asked for and whether any of them receives cancelled events. The order
/// between the Lua mods is the host's own job: `order` holds, for each hook a mod declared, the
/// mods in the order they run.
pub struct LuaHost {
    mods: Vec<LuaMod>,
    subscribed: &'static [Option<mod_api::HookId>],
    listeners: &'static [mod_api::Listener],
    order: Vec<(String, Vec<usize>)>,
    /// One line per folder or mod that did not load, from `builtin`.
    report: Vec<String>,
    /// Set once the report is written, so a second `register` writes nothing.
    reported: AtomicBool,
}

impl LuaHost {
    pub fn new(mods: Vec<LuaMod>) -> LuaHost {
        // The hooks in the order the mods first declare them. Each one's mods run by priority,
        // highest first; ties keep load order.
        let mut names = Vec::<String>::new();
        for name in mods.iter().flat_map(|m| m.hooks.iter()) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        let mut order = Vec::new();
        let mut subscribed = Vec::<Option<mod_api::HookId>>::new();
        let mut listeners = Vec::<mod_api::Listener>::new();
        for name in names {
            let declared: Vec<usize> = (0..mods.len())
                .filter(|&i| mods[i].hooks.contains(&name))
                .collect();
            let priority_of = |i: usize| mods[i].options(&name).priority;
            let mut run = declared.clone();
            run.sort_by_key(|&i| (Reverse(priority_of(i)), i));
            if let Some(id) = hook_id(&name) {
                subscribed.push(Some(id));
                listeners.push(mod_api::Listener {
                    hook: Some(id),
                    priority: declared.iter().map(|&i| priority_of(i)).max().unwrap_or(0),
                    receive_cancelled: declared
                        .iter()
                        .any(|&i| mods[i].options(&name).receive_cancelled),
                });
            }
            order.push((name, run));
        }
        LuaHost {
            mods,
            subscribed: Box::leak(subscribed.into_boxed_slice()),
            listeners: Box::leak(listeners.into_boxed_slice()),
            order,
            report: Vec::new(),
            reported: AtomicBool::new(false),
        }
    }

    /// The lines about the folders and mods that did not load, written by `register`.
    pub fn with_report(mut self, lines: Vec<String>) -> LuaHost {
        self.report = lines;
        self
    }

    pub fn report_lines(&self) -> &[String] {
        &self.report
    }

    /// The loaded Lua mods, in load order.
    pub(crate) fn mods(&self) -> &[LuaMod] {
        &self.mods
    }

    /// The indexes into `mods()` of the mods that declared `hook`, in the order they run. Empty
    /// for a hook no mod declared.
    pub(crate) fn run_order(&self, hook: &str) -> &[usize] {
        match self.order.iter().find(|(name, _)| name == hook) {
            Some((_, run)) => run,
            None => &[],
        }
    }

    /// Writes the loaded and refused counts and one line per refusal. The game's logger is up by
    /// `register`, unlike when the mods are read, so this is where the player sees them.
    fn report(&self) {
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        log::info!(
            "Lua mods: {} loaded, {} refused",
            self.mods.len(),
            self.report.len()
        );
        for line in &self.report {
            log::warn!("Lua mod refused: {line}");
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
        self.dispatch(
            hook,
            &|lua| new_event(lua, lua.create_table()?, &[]),
            &mut |_, _| true,
        );
    }
}

impl LuaHost {
    /// The hook ids the game subscribes to, read by the generated `Hooks::subscribed`.
    pub(crate) fn subscribed_slice(&self) -> &'static [Option<mod_api::HookId>] {
        self.subscribed
    }

    /// The listeners the game reads, generated `Hooks::listeners`: one per subscribed hook, in the
    /// same order as `subscribed_slice`.
    pub(crate) fn listeners_slice(&self) -> &'static [mod_api::Listener] {
        self.listeners
    }

    /// What `register` does: writes the report and records the loaded mods. The generated
    /// `Hooks::register` calls it.
    pub(crate) fn on_register(&self) {
        self.report();
        self.record_loaded();
    }
}

/// Runs one handler under `HANDLER_BUDGET`. The budget keeps failing after the deadline, so a
/// `pcall` inside the handler cannot swallow the stop.
pub(crate) fn call_handler(lua: &Lua, handler: &Function, event: &Table) -> mlua::Result<()> {
    budget::within_with(
        lua,
        HANDLER_BUDGET,
        "the handler ran over its time budget",
        || handler.call::<()>(event.clone()),
    )
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
    let limit = Mutex::new(RateLimit::new(LOG_BURST, LOG_PER_SECOND));
    horizon.raw_set(
        "log",
        lua.create_function(move |_, message: Value| -> mlua::Result<()> {
            let Value::String(message) = message else {
                return Err(Error::runtime("horizon.log needs the message as a string"));
            };
            // A line over the rate is dropped without an error: the mod is not told.
            let verdict = limit
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .allow(Instant::now());
            match verdict {
                Verdict::Dropped => return Ok(()),
                Verdict::AllowedAfter(n) => {
                    log::warn!(target: LOG_TARGET, "{id}: {n} log lines were dropped");
                }
                Verdict::Allowed => {}
            }
            let line: String = message
                .to_string_lossy()
                .chars()
                .take(MAX_LOG_LINE)
                .collect();
            keep_line(&log, line.clone());
            log::info!(target: LOG_TARGET, "{id}: {line}");
            Ok(())
        })?,
    )?;
    horizon.set_readonly(true);
    Ok(horizon)
}

/// Reads a mod's `main.luau`, reading at most `MAX_SOURCE + 1` bytes. A symlink is refused.
///
/// The file is opened, then the opened file and the path are checked, and only then read, so a
/// path swapped for a symlink or another file in between is refused (see `same_file`). The path
/// is also checked before the open, so a FIFO is not opened in the normal case: opening one
/// blocks until a writer appears.
fn read_source(dir: &Path) -> Result<String, String> {
    let dir_meta = std::fs::symlink_metadata(dir).map_err(|_| format!("no {ENTRY}"))?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Err("mod folder is not a plain folder".to_string());
    }
    let path = dir.join(ENTRY);
    let before = std::fs::symlink_metadata(&path).map_err(|_| format!("no {ENTRY}"))?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err(format!("{ENTRY} is not a plain file"));
    }
    let file = std::fs::File::open(&path).map_err(|_| format!("{ENTRY} cannot be read"))?;
    let opened = file
        .metadata()
        .map_err(|_| format!("{ENTRY} cannot be read"))?;
    if !opened.is_file() {
        return Err(format!("{ENTRY} is not a plain file"));
    }
    if !same_file(&file, &path) {
        return Err(format!("{ENTRY} changed while it was opened"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{ENTRY} cannot be read"))?;
    if bytes.len() as u64 > MAX_SOURCE {
        return Err(format!("{ENTRY} is over {} KiB", MAX_SOURCE / 1024));
    }
    String::from_utf8(bytes).map_err(|_| format!("{ENTRY} is not UTF-8"))
}

/// Whether `path` still names the file that `opened` was opened from. A path that is now a
/// symlink, or not a plain file, never does.
fn same_file(opened: &std::fs::File, path: &Path) -> bool {
    let Ok(now) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if now.file_type().is_symlink() || !now.is_file() {
        return false;
    }
    same_inode(opened, &now)
}

/// Whether the opened file and the path's current metadata are the same device and inode.
#[cfg(unix)]
fn same_inode(opened: &std::fs::File, now: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    opened
        .metadata()
        .is_ok_and(|o| o.dev() == now.dev() && o.ino() == now.ino())
}

/// Other platforms have no inode to compare: the symlink and plain-file checks are all there is.
#[cfg(not(unix))]
fn same_inode(_opened: &std::fs::File, _now: &std::fs::Metadata) -> bool {
    true
}

/// Appends `line` to a mod's buffer, dropping the oldest lines past `MAX_LOG_LINES`.
fn keep_line(log: &Mutex<Vec<String>>, line: String) {
    let mut lines = lock(log);
    lines.push(line);
    if lines.len() > MAX_LOG_LINES {
        let excess = lines.len() - MAX_LOG_LINES;
        lines.drain(..excess);
    }
}

/// The log buffer's lock. A poisoned lock still holds the lines, so it is taken anyway.
fn lock(log: &Mutex<Vec<String>>) -> MutexGuard<'_, Vec<String>> {
    log.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Instant;

    /// A server hook the generated impl has, with a cancel answer: the tests fire it by name.
    const HOOK: &str = "block_receive_game";

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
            "horizon.on(\"block_receive_game\", function() horizon.log(\"tick\") end)",
            Side::Both,
        );
        let loaded = LuaMod::load(&found).unwrap();
        assert_eq!(loaded.hooks(), ["block_receive_game"]);
        assert_eq!(loaded.id(), "mod_a");
        assert_eq!(loaded.side(), Side::Both);
    }

    #[test]
    fn the_host_subscribes_to_the_union_once() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", source, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", source, Side::Server)).unwrap();
        let host = LuaHost::new(vec![a, b]);
        let subscribed = mod_api::Hooks::subscribed(&host);
        assert_eq!(subscribed.len(), 1);
        assert_eq!(
            subscribed[0].map(|id| id as usize),
            Some(mod_api::HookId::BlockReceiveGame as usize)
        );
    }

    #[test]
    fn fire_runs_the_handlers() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() horizon.log(\"tick\") end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(HOOK);
        assert_eq!(lock(&log).clone(), ["tick"]);
        host.fire(HOOK);
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
    fn a_hook_without_a_generated_method_is_refused() {
        // `play_sound` writes back (`sound: &mut Option<SoundPlay>`), so it has no generated method.
        let err = load_one("horizon.on(\"play_sound\", function() end)")
            .err()
            .unwrap();
        assert!(err.contains("unknown hook 'play_sound'"), "got {err}");
    }

    #[test]
    fn a_server_tick_listener_is_declared_now_that_the_phase_converts() {
        let loaded = load_one("horizon.on(\"server_tick\", function() end)").unwrap();
        assert_eq!(loaded.hooks(), ["server_tick"]);
    }

    /// The host argument of a server hook: it has no state the Lua hooks read.
    struct NoHost;
    impl mod_api::Host for NoHost {}
    mod_api::impl_host_areas!(NoHost);
    impl mod_api::ServerHost for NoHost {}
    mod_api::impl_server_host_areas!(NoHost);

    /// A block the game received, as the generated `block_receive_game` gets it.
    fn received(cancelled: bool) -> mod_api::BlockReceiveGameEvent<'static> {
        mod_api::BlockReceiveGameEvent {
            dimension: "minecraft:overworld",
            pos: [1, 64, 2],
            block: "minecraft:stone",
            event: "minecraft:block_event",
            cancelled,
        }
    }

    #[test]
    fn a_generated_cancel_hook_returns_cancel_when_a_mod_cancels() {
        let m =
            load_one("horizon.on(\"block_receive_game\", function(event) event.cancel = true end)")
                .unwrap();
        let host = LuaHost::new(vec![m]);
        let mut event = received(false);
        let flow = mod_api::Hooks::block_receive_game(&host, &mut NoHost, &mut event);
        assert!(matches!(flow, mod_api::Flow::Cancel));
    }

    #[test]
    fn a_generated_cancel_hook_returns_continue_when_no_mod_cancels() {
        let m = load_one(
            "horizon.on(\"block_receive_game\", function(event) horizon.log(event.block) end)",
        )
        .unwrap();
        let host = LuaHost::new(vec![m]);
        let mut event = received(false);
        let flow = mod_api::Hooks::block_receive_game(&host, &mut NoHost, &mut event);
        assert!(matches!(flow, mod_api::Flow::Continue));
        assert_eq!(host.mods()[0].log_lines(), ["minecraft:stone"]);
    }

    #[test]
    fn a_client_mod_is_not_fired_for_a_server_hook() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() horizon.log(\"tick\") end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Client);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(HOOK);
        assert!(lock(&log).is_empty());
    }

    #[test]
    fn a_failing_handler_does_not_stop_the_others() {
        let root = tempfile::tempdir().unwrap();
        let failing = "horizon.on(\"block_receive_game\", function() error(\"boom\") end)";
        let logging =
            "horizon.on(\"block_receive_game\", function() horizon.log(\"still here\") end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", failing, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", logging, Side::Both)).unwrap();
        let b_log = Arc::clone(&b.log);
        let host = LuaHost::new(vec![a, b]);
        host.fire(HOOK);
        assert_eq!(lock(&b_log).clone(), ["still here"]);
    }

    #[test]
    fn a_runaway_handler_is_stopped_and_the_others_run() {
        let root = tempfile::tempdir().unwrap();
        let runaway = "horizon.on(\"block_receive_game\", function() while true do end end)";
        let logging = "horizon.on(\"block_receive_game\", function() horizon.log(\"ran\") end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", runaway, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", logging, Side::Both)).unwrap();
        let b_log = Arc::clone(&b.log);
        let host = LuaHost::new(vec![a, b]);
        let started = Instant::now();
        host.fire(HOOK);
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
        let source = "horizon.on(\"block_receive_game\", function()\n\
                      local ok = pcall(function() return horizon.world end)\n\
                      horizon.log(tostring(ok))\n\
                      end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(HOOK);
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
    fn the_log_keeps_the_newest_lines() {
        // The rate limit lets a burst through, so the cap is checked on the buffer itself.
        let log = Mutex::new(Vec::new());
        for i in 1..=MAX_LOG_LINES + 49 {
            keep_line(&log, format!("line {i}"));
        }
        let lines = lock(&log).clone();
        assert_eq!(lines.len(), MAX_LOG_LINES);
        assert_eq!(lines[0], "line 50");
        assert_eq!(
            lines[MAX_LOG_LINES - 1],
            format!("line {}", MAX_LOG_LINES + 49)
        );
    }

    #[test]
    fn a_long_line_is_cut() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function()\n\
                      horizon.log(string.rep(\"x\", 2000))\n\
                      end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let host = LuaHost::new(vec![LuaMod::load(&found).unwrap()]);
        host.fire(HOOK);
        let lines = host.mods[0].log_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].chars().count(), MAX_LOG_LINE);
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

    #[cfg(unix)]
    #[test]
    fn a_symlinked_main_luau_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside.luau");
        std::fs::write(
            &outside,
            "horizon.on(\"block_receive_game\", function() end)",
        )
        .unwrap();
        let dir = root.path().join("mod_a");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(ENTRY)).unwrap();
        let err = read_source(&dir).err().unwrap();
        assert!(err.contains("not a plain file"), "got {err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_mod_folder_is_refused_at_load() {
        let root = tempfile::tempdir().unwrap();
        let real = mod_folder(
            root.path(),
            "real",
            "horizon.on(\"block_receive_game\", function() end)",
            Side::Both,
        );
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(&real.dir, &link).unwrap();
        let found = Discovered {
            dir: link,
            id: "linked".into(),
            version: "1".into(),
            side: Side::Both,
        };
        let err = LuaMod::load(&found).err().unwrap();
        assert!(err.contains("not a plain folder"), "got {err}");
    }

    #[test]
    fn a_file_swapped_after_open_is_detected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(ENTRY);
        std::fs::write(&path, "first").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        assert!(same_file(&file, &path));

        // The open handle keeps the old inode alive, so the new file cannot reuse its number.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "second").unwrap();
        assert!(!same_file(&file, &path));

        #[cfg(unix)]
        {
            let outside = root.path().join("outside.luau");
            std::fs::write(&outside, "third").unwrap();
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert!(!same_file(&file, &path));
        }
    }

    #[test]
    fn pcall_cannot_swallow_a_handlers_budget() {
        // The interrupt keeps failing after the deadline, so catching one error does not help.
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() while true do pcall(function() while true do end end) end end)";
        let m = LuaMod::load(&mod_folder(root.path(), "stubborn", source, Side::Both)).unwrap();
        let host = LuaHost::new(vec![m]);
        let started = Instant::now();
        host.fire(HOOK);
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

    /// Adds a handler for `name` to the mod, as a second hook the mod could declare. Only the
    /// game's own hooks can be declared through `horizon.on`, so the test registers the hook here.
    #[test]
    fn two_threads_calling_one_state_each_keep_their_own_limit() {
        let root = tempfile::tempdir().unwrap();
        let runaway = "horizon.on(\"block_receive_game\", function() while true do end end)";
        // The other hook counts in a Lua global rather than logging: log lines are rate limited.
        let source =
            format!("{runaway}\nhorizon.on(\"animal_tame\", function() n = (n or 0) + 1 end)");
        let m = LuaMod::load(&mod_folder(root.path(), "mod_a", &source, Side::Both)).unwrap();
        let host = LuaHost::new(vec![m]);
        let started = Instant::now();
        thread::scope(|s| {
            let a = s.spawn(|| {
                let began = Instant::now();
                host.fire(HOOK);
                began.elapsed()
            });
            s.spawn(|| {
                for _ in 0..50 {
                    host.fire("animal_tame");
                }
            });
            let a_elapsed = a.join().unwrap();
            assert!(
                a_elapsed < HANDLER_BUDGET * 5,
                "the runaway was not stopped in time: {a_elapsed:?}"
            );
        });
        assert!(started.elapsed() < Duration::from_secs(10));
        let env = declare::environment(&host.mods[0].lua).unwrap();
        let others: i64 = env.get("n").unwrap();
        assert_eq!(others, 50, "every other call must have run");
    }

    #[test]
    fn the_turn_lock_is_reentrant_and_exclusive() {
        let root = tempfile::tempdir().unwrap();
        let m = lua_mod(root.path(), "mod_a", "", Side::Both);
        let turn = Arc::new(m.turn);
        // Re-entry on one thread must not deadlock: a watchdog fails the test if it does.
        let (done_tx, done_rx) = mpsc::channel();
        let reentrant = Arc::clone(&turn);
        thread::spawn(move || {
            let _outer = reentrant.enter();
            let _inner = reentrant.enter();
            let _deepest = reentrant.enter();
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("re-entering the turn on one thread deadlocked");

        // Another thread waits while the turn is held, and gets it once it is free.
        let holder = turn.enter();
        let (entered_tx, entered_rx) = mpsc::channel();
        let waiter = Arc::clone(&turn);
        let handle = thread::spawn(move || {
            let _turn = waiter.enter();
            let _ = entered_tx.send(());
        });
        assert!(
            entered_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "a second thread entered a held turn"
        );
        drop(holder);
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the waiting thread never got the free turn");
        handle.join().unwrap();
    }

    #[test]
    fn a_panic_inside_a_turn_releases_it() {
        let root = tempfile::tempdir().unwrap();
        let m = lua_mod(root.path(), "mod_a", "", Side::Both);
        let turn = Arc::new(m.turn);
        let panicking = Arc::clone(&turn);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _turn = panicking.enter();
            panic!("a handler panicked");
        }));
        assert!(caught.is_err());
        // Another thread enters at once only if the panic gave the turn back.
        let (entered_tx, entered_rx) = mpsc::channel();
        let after = Arc::clone(&turn);
        thread::spawn(move || {
            let _turn = after.enter();
            let _ = entered_tx.send(());
        });
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the panicking thread kept the turn");
    }

    /// Every line the process logs, for the tests that check what is reported.
    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct Capture;

    static CAPTURE: Capture = Capture;

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            lock_captured().push(record.args().to_string());
        }

        fn flush(&self) {}
    }

    fn lock_captured() -> MutexGuard<'static, Vec<String>> {
        CAPTURED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Installs the capturing logger once for the process.
    fn install_capture() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = log::set_logger(&CAPTURE);
            log::set_max_level(log::LevelFilter::Info);
        });
    }

    fn captured_with_prefix(prefix: &str) -> Vec<String> {
        install_capture();
        lock_captured()
            .iter()
            .filter(|line| line.starts_with(prefix))
            .cloned()
            .collect()
    }

    #[test]
    fn a_tight_loop_cannot_flood_the_log() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function()\n\
                      for i = 1, 10000 do horizon.log(\"line \" .. i) end\n\
                      end)";
        let found = mod_folder(root.path(), "flood_mod", source, Side::Both);
        let host = LuaHost::new(vec![LuaMod::load(&found).unwrap()]);
        host.fire(HOOK);
        let lines = host.mods[0].log_lines();
        assert!(
            lines.len() >= LOG_BURST as usize,
            "the loop did not run: {} lines",
            lines.len()
        );
        assert!(
            lines.len() <= LOG_BURST as usize + 3,
            "{} lines got through",
            lines.len()
        );
        // A dropped line is not an error: the handler ran to its end.
        assert!(captured_with_prefix("flood_mod: server_tick").is_empty());
    }

    #[test]
    fn the_report_is_emitted_once_at_register() {
        install_capture();
        let host = LuaHost::new(Vec::new()).with_report(vec!["report_mod: nope".into()]);
        host.report();
        host.report();
        assert_eq!(
            captured_with_prefix("Lua mods: "),
            ["Lua mods: 0 loaded, 1 refused"]
        );
        assert_eq!(
            captured_with_prefix("Lua mod refused: "),
            ["Lua mod refused: report_mod: nope"]
        );
    }

    /// A mod `id` that declares `hook` with `options` (a Lua table literal, or "" for none).
    fn ordered_mod(root: &Path, id: &str, hook: &str, options: &str) -> LuaMod {
        let source = if options.is_empty() {
            format!("horizon.on(\"{hook}\", function() end)")
        } else {
            format!("horizon.on(\"{hook}\", function() end, {options})")
        };
        let found = mod_folder(root, id, &source, Side::Both);
        LuaMod::load(&found).unwrap()
    }

    /// The ids of the mods that ran for `HOOK`, in the order they ran.
    fn ran_order(host: &LuaHost) -> Vec<String> {
        let mut ids = Vec::new();
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, lua.create_table()?, &[]),
            &mut |id, _| {
                ids.push(id.to_string());
                true
            },
        );
        ids
    }

    #[test]
    fn mods_run_by_priority_highest_first_and_ties_in_load_order() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            ordered_mod(root.path(), "a", HOOK, "{priority = 0}"),
            ordered_mod(root.path(), "b", HOOK, "{priority = 100}"),
            ordered_mod(root.path(), "c", HOOK, "{priority = 100}"),
        ]);
        assert_eq!(ran_order(&host), ["b", "c", "a"]);
    }

    #[test]
    fn a_negative_priority_runs_last() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            ordered_mod(root.path(), "a", HOOK, "{priority = -5}"),
            ordered_mod(root.path(), "b", HOOK, ""),
            ordered_mod(root.path(), "c", HOOK, "{priority = 1}"),
        ]);
        assert_eq!(ran_order(&host), ["c", "b", "a"]);
    }

    #[test]
    fn a_mod_that_fails_keeps_the_order_and_the_later_mods_run() {
        let root = tempfile::tempdir().unwrap();
        let failing = mod_folder(
            root.path(),
            "a",
            "horizon.on(\"block_receive_game\", function() error(\"boom\") end, {priority = 10})",
            Side::Both,
        );
        let host = LuaHost::new(vec![
            LuaMod::load(&failing).unwrap(),
            ordered_mod(root.path(), "b", HOOK, ""),
        ]);
        assert_eq!(ran_order(&host), ["a", "b"]);
        assert_eq!(host.run_order(HOOK), [0, 1]);
    }

    #[test]
    fn a_mod_with_no_options_for_a_hook_gets_the_defaults() {
        let root = tempfile::tempdir().unwrap();
        let m = ordered_mod(
            root.path(),
            "a",
            HOOK,
            "{priority = 4, receive_cancelled = true}",
        );
        assert_eq!(
            m.options(HOOK),
            ListenerOptions {
                priority: 4,
                receive_cancelled: true
            }
        );
        assert_eq!(m.options("animal_tame"), ListenerOptions::default());
    }

    #[test]
    fn the_game_gets_one_listener_per_subscribed_hook_with_the_max_priority_and_any_receive() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            ordered_mod(root.path(), "a", HOOK, "{priority = 3}"),
            ordered_mod(
                root.path(),
                "b",
                HOOK,
                "{priority = 7, receive_cancelled = true}",
            ),
            ordered_mod(root.path(), "c", "animal_tame", "{priority = -2}"),
        ]);
        let subscribed = mod_api::Hooks::subscribed(&host);
        let listeners = mod_api::Hooks::listeners(&host);
        assert_eq!(listeners.len(), subscribed.len());
        assert_eq!(listeners.len(), 2);
        for (listener, hook) in listeners.iter().zip(subscribed) {
            assert_eq!(
                listener.hook.map(|id| id as usize),
                hook.map(|id| id as usize)
            );
        }
        assert_eq!(listeners[0].priority, 7);
        assert!(listeners[0].receive_cancelled);
        assert_eq!(listeners[1].priority, -2);
        assert!(!listeners[1].receive_cancelled);
    }

    #[test]
    fn a_host_with_no_mods_gives_the_game_no_listeners() {
        let host = LuaHost::new(Vec::new());
        assert!(mod_api::Hooks::listeners(&host).is_empty());
    }
}
