//! The developer console's Lua runner: a typed chunk runs in a sandbox of its own, with a captured
//! `print`, a time budget and `mc.cmd`.
//!
//! Deliberate deviation from the plan's "one VM per mod per side": the console is not a mod, so it
//! has no mod's state. It has one VM, made once by [`state::new_state`] and shared by every run, and
//! no chunk it runs reaches a mod's state or the other way round.
//!
//! `mc.profile()` returns the Lua profiler's rows (see [`profile_table`]).
//!
//! `mc.cmd(line)` runs a server command through `ServerHost::run_command_with_output`, which runs it
//! as the server console (level 4), and returns the command's feedback lines. This runner does NOT
//! check who may call it: the caller (the `/lua` command, chunk 5e-b) must gate on the strict owner
//! before it calls [`ConsoleVm::run_chunk`]. `mc.cmd` is bound to the host only for the length of one
//! run (a `Lua::scope`), so a function kept from a run fails when called later instead of reaching a
//! host that is gone.
//!
//! This module sits in the platform layer and imports no dev-tools, observe or debug crate.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::Duration;

use horizon_client::crash::contain;
use mlua::{Error as LuaError, Function, Lua, MultiValue, Value};
use mod_api::ServerHost;

use crate::budget;
use crate::profile;
use crate::state::new_state;

/// The time one run may take on the server thread. A run past it stops with an error.
pub const CHUNK_BUDGET: Duration = Duration::from_millis(500);

/// The most lines a run keeps from `print`. Older lines drop first.
const MAX_LINES: usize = 256;
/// The most bytes of printed text a run keeps in all.
const MAX_BYTES: usize = 16 * 1024;
/// The most bytes one printed line keeps. A longer line is cut and ends in an ellipsis.
const MAX_LINE_BYTES: usize = 4 * 1024;

/// The error a run gives while another run holds the console's VM.
const BUSY: &str = "the console is busy";
/// The message a run stopped by its budget fails with.
const OVER_BUDGET: &str = "the chunk ran over its time budget";
/// The start of the traceback mlua appends to a runtime error; the runner reports the message only.
const TRACEBACK: &str = "\nstack traceback:";

/// What one run gives back: the lines `print` wrote, the error (with the chunk's label and line),
/// and the chunk's returned value as text.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub lines: Vec<String>,
    pub error: Option<String>,
    pub value: Option<String>,
}

impl RunResult {
    fn failed(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::default()
        }
    }
}

/// The printed lines of the current run, kept within [`MAX_LINES`] and [`MAX_BYTES`].
#[derive(Default)]
struct Printed {
    lines: VecDeque<String>,
    bytes: usize,
}

impl Printed {
    fn push(&mut self, line: &str) {
        let line = cut(line, MAX_LINE_BYTES);
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES || self.bytes > MAX_BYTES {
            match self.lines.pop_front() {
                Some(old) => self.bytes -= old.len(),
                None => break,
            }
        }
    }

    fn take(&mut self) -> Vec<String> {
        self.bytes = 0;
        self.lines.drain(..).collect()
    }
}

/// Cuts `text` to at most `max` bytes at a character boundary, ending in an ellipsis when it cuts.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max.saturating_sub("...".len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// The console's VM: one sandboxed Luau state and the buffer its `print` writes to.
pub struct ConsoleVm {
    /// The state, or why it could not be made. A `Lua` is not `Sync`, so it sits behind a mutex.
    lua: Result<Mutex<Lua>, String>,
    printed: Arc<Mutex<Printed>>,
}

/// The process's console VM, made on first use.
static CONSOLE: OnceLock<ConsoleVm> = OnceLock::new();

/// The console's VM. The first call makes it; later calls share it.
pub fn console() -> &'static ConsoleVm {
    CONSOLE.get_or_init(ConsoleVm::start)
}

impl ConsoleVm {
    fn start() -> Self {
        let printed = Arc::new(Mutex::new(Printed::default()));
        let lua = match setup(Arc::clone(&printed)) {
            Ok(lua) => Ok(Mutex::new(lua)),
            Err(err) => Err(message(&err)),
        };
        Self { lua, printed }
    }

    /// Runs `source` as a chunk named `label`. An expression (`1+1`) gives its value, as a REPL
    /// does: the source is tried as `return <source>` first, then as a statement. Never waits: a run
    /// while another holds the VM gives the error "the console is busy". Never panics: a Lua error,
    /// a budget stop, a memory failure and a panic in the host each come back as `error`.
    pub fn run_chunk(&self, host: &mut dyn ServerHost, label: &str, source: &str) -> RunResult {
        let lua = match &self.lua {
            Ok(lua) => lua,
            Err(err) => {
                return RunResult::failed(format!("the console could not start: {err}"));
            }
        };
        let guard = match lua.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return RunResult::failed(BUSY.to_owned()),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        lock(&self.printed).take();
        let outcome = contain(|| execute(&guard, host, label, source));
        let lines = lock(&self.printed).take();
        match outcome {
            Ok(Ok(value)) => RunResult {
                lines,
                error: None,
                value,
            },
            Ok(Err(error)) => RunResult {
                lines,
                error: Some(error),
                value: None,
            },
            Err(panic) => RunResult {
                lines,
                error: Some(format!("the game's host panicked: {panic}")),
                value: None,
            },
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The VM as it runs: the libraries of [`new_state`], a `print` that writes to `printed`, and the
/// `mc` table, whose `cmd` is added for each run.
fn setup(printed: Arc<Mutex<Printed>>) -> mlua::Result<Lua> {
    let lua = new_state()?;
    let print = lua.create_function(move |lua, args: MultiValue| {
        let tostring: Function = lua.globals().get("tostring")?;
        let mut parts = Vec::with_capacity(args.len());
        for arg in args {
            parts.push(tostring.call::<String>(arg)?);
        }
        lock(&printed).push(&parts.join("\t"));
        Ok(())
    })?;
    lua.globals().raw_set("print", print)?;
    let mc = lua.create_table()?;
    mc.raw_set(
        "profile",
        lua.create_function(|lua, ()| profile_table(lua))?,
    )?;
    lua.globals().raw_set("mc", mc)?;
    Ok(lua)
}

/// `mc.profile()`: the profiler's rows as a list of tables `{mod, hook, calls, total_ms, max_ms,
/// errors}`, the most total time first (the rows `/luaprofile` shows, all of them). The times are
/// milliseconds.
pub fn profile_table(lua: &Lua) -> mlua::Result<mlua::Table> {
    let mut rows = profile::profile_snapshot();
    rows.sort_by_key(|row| std::cmp::Reverse(row.total_ns));
    let out = lua.create_table()?;
    for (i, row) in rows.iter().enumerate() {
        let t = lua.create_table()?;
        t.raw_set("mod", row.mod_id.as_str())?;
        t.raw_set("hook", row.hook.as_str())?;
        t.raw_set("calls", row.calls)?;
        t.raw_set("total_ms", row.total_ns as f64 / 1_000_000.0)?;
        t.raw_set("max_ms", row.max_ns as f64 / 1_000_000.0)?;
        t.raw_set("errors", row.errors)?;
        out.raw_set(i + 1, t)?;
    }
    Ok(out)
}

/// Runs one chunk with `host` bound to `mc.cmd`. The binding and the function are gone when it ends.
fn execute(
    lua: &Lua,
    host: &mut dyn ServerHost,
    label: &str,
    source: &str,
) -> Result<Option<String>, String> {
    let outcome = lua.scope(|scope| {
        let cmd = scope.create_function_mut(move |_, line: String| {
            Ok(host.commands().run_command_with_output(&line).feedback)
        })?;
        let mc: mlua::Table = lua.globals().get("mc")?;
        mc.raw_set("cmd", cmd)?;
        let ran = compile(lua, label, source).and_then(|chunk| {
            budget::within_with(lua, CHUNK_BUDGET, OVER_BUDGET, || chunk.call::<Value>(()))
                .and_then(|value| describe(lua, value))
        });
        mc.raw_set("cmd", Value::Nil)?;
        Ok(ran)
    });
    match outcome {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) | Err(err) => Err(message(&err)),
    }
}

/// The chunk as an expression if it is one, else as a statement. A statement's syntax error is the
/// one reported.
fn compile(lua: &Lua, label: &str, source: &str) -> mlua::Result<Function> {
    match lua
        .load(format!("return {source}"))
        .set_name(format!("={label}"))
        .into_function()
    {
        Ok(function) => Ok(function),
        Err(_) => lua
            .load(source)
            .set_name(format!("={label}"))
            .into_function(),
    }
}

/// The text of a returned value: `None` for nil, `table` and `function` for those, and `tostring`
/// for the rest.
fn describe(lua: &Lua, value: Value) -> mlua::Result<Option<String>> {
    Ok(match value {
        Value::Nil => None,
        Value::Table(_) => Some("table".to_owned()),
        Value::Function(_) => Some("function".to_owned()),
        other => {
            let tostring: Function = lua.globals().get("tostring")?;
            Some(tostring.call::<String>(other)?)
        }
    })
}

/// A Lua error as one line: its message, without the traceback mlua appends to it.
fn message(err: &LuaError) -> String {
    match err {
        LuaError::SyntaxError { message, .. } | LuaError::RuntimeError(message) => message
            .split(TRACEBACK)
            .next()
            .unwrap_or_default()
            .to_owned(),
        LuaError::CallbackError { cause, .. } => message(cause),
        other => other.to_string(),
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
