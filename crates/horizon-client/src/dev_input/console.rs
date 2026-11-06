//! The Lua console's shared state: the lines it shows, the code it runs and
//! the completions of its input line (docs/dev-tools.md). The window (the
//! windows' half, the `dev_tools` console) draws it, and the game's own code
//! (`game/commands/inspect.rs`) answers its completion requests. Both
//! read and write these queues; neither draws the other.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where scripts live, under the game folder (the working directory).
pub const SCRIPTS_DIR: &str = "dev-tools/scripts";
/// Output lines kept.
const KEEP: usize = 1000;

/// Whether the window is open (`/lua` toggles it).
pub static OPEN: AtomicBool = AtomicBool::new(false);
/// `/lua` just opened the window: the tools take the pointer and the line
/// takes the keyboard.
pub static WANT_POINTER: AtomicBool = AtomicBool::new(false);
static FOCUS_INPUT: AtomicBool = AtomicBool::new(false);

/// Open the window from a command: with the pointer and the focus.
pub fn open_from_command() {
    OPEN.store(true, Ordering::Relaxed);
    WANT_POINTER.store(true, Ordering::Relaxed);
    FOCUS_INPUT.store(true, Ordering::Relaxed);
}

/// Whether the window's input line is to be focused now (a command opened
/// the console): the window takes it and forgets.
pub fn take_focus() -> bool {
    FOCUS_INPUT.swap(false, Ordering::Relaxed)
}
/// A click on "stop callbacks": the tick handlers are dropped.
pub static STOP_TICKS: AtomicBool = AtomicBool::new(false);

/// A chunk to run: what the window shows for it, and the code.
#[derive(Clone, Debug)]
pub struct Request {
    pub label: String,
    pub code: String,
}

/// What a line of the output is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What was run, echoed.
    Echo,
    /// `print` and results.
    Out,
    /// An error.
    Err,
}

static INPUT: Mutex<Vec<Request>> = Mutex::new(Vec::new());
static OUTPUT: Mutex<VecDeque<(Kind, String)>> = Mutex::new(VecDeque::new());
/// Whether the server thread has a VM running tick callbacks (shown in the window).
pub static TICKING: AtomicBool = AtomicBool::new(false);

/// Completions for the input line: the byte range they replace and the
/// candidates (text, tooltip), for the line and cursor they were made for,
/// and a hint shown above them (the call's signature and an example).
#[derive(Clone, Debug, Default)]
pub struct Completions {
    pub text: String,
    pub cursor: usize,
    pub range: (usize, usize),
    pub items: Vec<(String, Option<String>)>,
    pub hint: Option<String>,
}

/// Where the cursor is in a call: `mc.watch(` is `name`, the argument
/// being typed is `arg` (0 first), and `string` is the string literal
/// the cursor is inside, if any: where its content starts, and the
/// content up to the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallContext {
    pub name: String,
    pub arg: usize,
    pub string: Option<(usize, String)>,
}

/// The innermost open call at the cursor.
pub fn call_context(text: &str, cursor: usize) -> Option<CallContext> {
    let cursor = cursor.min(text.len());
    let head = &text[..cursor];
    let mut frames: Vec<(usize, usize)> = Vec::new(); // (index of '(', args so far)
    let mut quote: Option<(char, usize)> = None;
    let mut escaped = false;
    for (i, c) in head.char_indices() {
        match quote {
            Some((q, _)) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some((c, i + 1)),
                '(' => frames.push((i, 0)),
                ')' => {
                    frames.pop();
                }
                ',' => {
                    if let Some(f) = frames.last_mut() {
                        f.1 += 1;
                    }
                }
                _ => {}
            },
        }
    }
    let &(paren, arg) = frames.last()?;
    let name_start = head[..paren]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '.')
        .last()
        .map_or(paren, |(i, _)| i);
    let name = head[name_start..paren].to_string();
    if name.is_empty() {
        return None;
    }
    Some(CallContext {
        name,
        arg,
        string: quote.map(|(_, start)| (start, head[start..].to_string())),
    })
}

/// The `mc` functions, their signatures and an example each.
pub const SIGNATURES: &[(&str, &str, &str)] = &[
    (
        "mc.cmd",
        "mc.cmd(line) -> ok, lines",
        "mc.cmd(\"time set day\")",
    ),
    (
        "mc.select",
        "mc.select(selector) -> {id, ...}",
        "mc.select(\"@e[type=pig,distance=..10]\")",
    ),
    (
        "mc.get",
        "mc.get(entity[, path]) -> table or value",
        "mc.get(\"@e[type=pig,limit=1]\", \"Pos\")",
    ),
    (
        "mc.snbt",
        "mc.snbt(entity[, path]) -> SNBT text",
        "mc.snbt(\"@e[type=pig,limit=1]\", \"Health\")",
    ),
    (
        "mc.set",
        "mc.set(entity, path, value) -> changed",
        "mc.set(\"@e[type=pig,limit=1]\", \"Health\", \"20.0f\")",
    ),
    (
        "mc.merge",
        "mc.merge(entity, value) -> changed",
        "mc.merge(\"@e[type=pig,limit=1]\", \"{NoAI:1b}\")",
    ),
    (
        "mc.pos",
        "mc.pos(entity) -> x, y, z",
        "local x, y, z = mc.pos(\"@p\")",
    ),
    (
        "mc.name",
        "mc.name(entity) -> string",
        "mc.name(\"@e[type=pig,limit=1]\")",
    ),
    (
        "mc.inspect",
        "mc.inspect(entity[, view])  view: \"data\" \"goals\" \"senses\" \"nav\"",
        "mc.inspect(\"@e[type=pig,limit=1]\", \"nav\")",
    ),
    (
        "mc.watch",
        "mc.watch(entity, path)  a chart of a numeric path",
        "mc.watch(\"@e[type=pig,limit=1]\", \"Health\")",
    ),
    (
        "mc.on_tick",
        "mc.on_tick(function() ... end)  every server tick",
        "mc.on_tick(function() print(mc.pos(\"@p\")) end)",
    ),
    ("mc.off_tick", "mc.off_tick()", "mc.off_tick()"),
    (
        "mc.run",
        "mc.run(file)  a script of the scripts folder",
        "mc.run(\"hello.lua\")",
    ),
    (
        "mc.scripts_dir",
        "mc.scripts_dir() -> path",
        "print(mc.scripts_dir())",
    ),
    (
        "mc.log",
        "mc.log(message)  the window and the game log",
        "mc.log(\"hi\")",
    ),
    (
        "mc.mods",
        "mc.mods() -> {{id, source, scripts, reloads, handlers, goals, error}, ...}",
        "for _, m in ipairs(mc.mods()) do print(m.id, m.error) end",
    ),
    (
        "mc.reload_mods",
        "mc.reload_mods([id])  the scripted mods read their scripts again next tick",
        "mc.reload_mods(\"horizon_parasite_mod\")",
    ),
    (
        "print",
        "print(...)  the window",
        "print(mc.snbt(\"@p\", \"Pos\"))",
    ),
];

/// The functions whose first argument names an entity: a selector string
/// there completes as the chat box completes `/tp`'s.
pub const ENTITY_FIRST: &[&str] = &[
    "mc.select",
    "mc.get",
    "mc.snbt",
    "mc.set",
    "mc.merge",
    "mc.pos",
    "mc.name",
    "mc.inspect",
    "mc.watch",
];
/// The functions whose second argument is an NBT path.
pub const PATH_SECOND: &[&str] = &["mc.get", "mc.snbt", "mc.set", "mc.watch"];
/// NBT keys most entities have, offered for a path argument.
const PATH_KEYS: &[&str] = &[
    "Health",
    "Pos",
    "Motion",
    "Rotation",
    "NoAI",
    "NoGravity",
    "Silent",
    "Invulnerable",
    "CustomName",
    "CustomNameVisible",
    "Glowing",
    "Fire",
    "Air",
    "OnGround",
    "FallDistance",
    "PersistenceRequired",
    "Age",
    "InLove",
    "Brain",
    "attributes",
    "Tags",
    "UUID",
    "HurtTime",
    "DeathTime",
    "AbsorptionAmount",
    "LeftHanded",
    "CanPickUpLoot",
    "equipment",
    "active_effects",
    "Pos[0]",
    "Pos[1]",
    "Pos[2]",
];

/// The hint for a call: its signature and example.
pub fn signature_hint(name: &str) -> Option<String> {
    SIGNATURES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, sig, example)| format!("{sig}\n   e.g. {example}"))
}

static COMPLETION_REQUEST: Mutex<Option<(String, usize)>> = Mutex::new(None);
static COMPLETIONS: Mutex<Option<Completions>> = Mutex::new(None);

/// The window asks for completions of `text` at byte `cursor`; the window
/// thread's game answers with [`set_completions`].
pub fn request_completions(text: &str, cursor: usize) {
    let known = COMPLETIONS.lock().unwrap_or_else(|e| e.into_inner());
    if known
        .as_ref()
        .is_some_and(|c| c.text == text && c.cursor == cursor)
    {
        return;
    }
    *COMPLETION_REQUEST.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((text.to_string(), cursor));
}

/// The pending request, if any (the window thread's game).
pub fn take_completion_request() -> Option<(String, usize)> {
    COMPLETION_REQUEST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

pub fn set_completions(c: Completions) {
    *COMPLETIONS.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
}

/// The completions made for exactly this line and cursor, if any.
pub fn completions_for(text: &str, cursor: usize) -> Option<Completions> {
    COMPLETIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|c| c.text == text && c.cursor == cursor)
        .cloned()
}

/// The `mc` table and the Lua names worth offering, with what they do.
const LUA_NAMES: &[(&str, &str)] = &[
    ("mc.cmd(", "run a vanilla command as the owner: ok, lines"),
    ("mc.select(", "entity ids a selector finds"),
    ("mc.get(", "entity data as a table; a path narrows it"),
    ("mc.snbt(", "entity data as SNBT text; a path narrows it"),
    ("mc.set(", "/data modify … set value: entity, path, value"),
    ("mc.merge(", "/data merge entity: entity, value"),
    ("mc.pos(", "x, y, z of an entity"),
    ("mc.name(", "an entity's display name"),
    (
        "mc.inspect(",
        "open a window: entity, \"data\"|\"goals\"|\"senses\"|\"nav\"",
    ),
    ("mc.watch(", "open a /watch chart: entity, path"),
    ("mc.on_tick(", "run a function every server tick"),
    ("mc.off_tick()", "stop the tick functions"),
    ("mc.run(", "run a script of the scripts folder"),
    ("mc.scripts_dir()", "the scripts folder"),
    ("mc.log(", "to the window and the game log"),
    (
        "mc.mods()",
        "the scripted mods: id, source, scripts, reloads, handlers, goals, error",
    ),
    (
        "mc.reload_mods(",
        "reload the scripted mods' scripts next tick: all, or one by id",
    ),
    ("print(", "to the window"),
    ("string.format(", ""),
    ("tostring(", ""),
    ("tonumber(", ""),
    ("ipairs(", ""),
    ("pairs(", ""),
    ("local ", ""),
    ("function", ""),
    ("return", ""),
    ("end", ""),
    ("then", ""),
    ("for ", ""),
    ("if ", ""),
    ("while ", ""),
    ("not ", ""),
    ("and ", ""),
    ("or ", ""),
    ("nil", ""),
    ("true", ""),
    ("false", ""),
];

/// Completions of a Lua name at the cursor: the identifier before it
/// (letters, digits, `_`, `.`) against [`LUA_NAMES`]; inside a call, its
/// signature as the hint, and for a string argument the values it takes
/// (an `mc.inspect` view, an NBT key). Entity arguments complete on the
/// window's game, which has the command tree.
pub fn lua_completions(text: &str, cursor: usize) -> Completions {
    let cursor = cursor.min(text.len());
    let call = call_context(text, cursor);
    let hint = call.as_ref().and_then(|c| signature_hint(&c.name));
    if let Some(c) = &call
        && let Some((start, partial)) = &c.string
    {
        let candidates: Vec<&str> = if c.name == "mc.inspect" && c.arg == 1 {
            vec!["data", "goals", "senses", "nav"]
        } else if PATH_SECOND.contains(&c.name.as_str()) && c.arg == 1 {
            PATH_KEYS.to_vec()
        } else {
            Vec::new()
        };
        let items = candidates
            .into_iter()
            .filter(|k| k.starts_with(partial.as_str()) && *k != partial.as_str())
            .map(|k| (k.to_string(), None))
            .collect();
        return Completions {
            text: text.to_string(),
            cursor,
            range: (*start, cursor),
            items,
            hint,
        };
    }
    let before = &text[..cursor];
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '.')
        .last()
        .map_or(cursor, |(i, _)| i);
    let prefix = &before[start..];
    let items = if prefix.is_empty() {
        Vec::new()
    } else {
        LUA_NAMES
            .iter()
            .filter(|(name, _)| name.starts_with(prefix) && *name != prefix)
            .map(|(name, what)| {
                (
                    name.to_string(),
                    (!what.is_empty()).then(|| what.to_string()),
                )
            })
            .collect()
    };
    Completions {
        text: text.to_string(),
        cursor,
        range: (start, cursor),
        items,
        hint,
    }
}

/// When the cursor is inside the string of an `mc.cmd("…")` call: where
/// the command text starts (after the quote and a slash, if any) and the
/// command typed so far.
pub fn command_context(text: &str, cursor: usize) -> Option<(usize, String)> {
    let c = call_context(text, cursor)?;
    if c.name != "mc.cmd" || c.arg != 0 {
        return None;
    }
    let (start, partial) = c.string?;
    let start = if partial.starts_with('/') {
        start + 1
    } else {
        start
    };
    Some((start, text[start..cursor.min(text.len())].to_string()))
}

/// When the cursor is inside the string that names an entity (the first
/// argument of [`ENTITY_FIRST`]): where it starts and what is typed.
pub fn entity_context(text: &str, cursor: usize) -> Option<(usize, String)> {
    let c = call_context(text, cursor)?;
    if !ENTITY_FIRST.contains(&c.name.as_str()) || c.arg != 0 {
        return None;
    }
    c.string
}

/// The lines run so far, newest last, kept across runs in
/// `dev-tools/console-history.txt` under the game folder.
static HISTORY: Mutex<Option<Vec<String>>> = Mutex::new(None);
/// Where the history is kept.
pub const HISTORY_FILE: &str = "dev-tools/console-history.txt";
/// Lines of history kept.
const HISTORY_KEEP: usize = 500;

fn with_history<R>(f: impl FnOnce(&mut Vec<String>) -> R) -> R {
    let mut slot = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let history = slot.get_or_insert_with(|| {
        std::fs::read_to_string(HISTORY_FILE)
            .map(|text| {
                let lines: Vec<String> = text.lines().map(str::to_string).collect();
                lines[lines.len().saturating_sub(HISTORY_KEEP)..].to_vec()
            })
            .unwrap_or_default()
    });
    f(history)
}

/// A line that was run: the newest of the history, unless it repeats the
/// last one.
pub fn remember(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    with_history(|h| {
        if h.last().is_some_and(|l| l == line) {
            return;
        }
        h.push(line.to_string());
        if h.len() > HISTORY_KEEP {
            h.remove(0);
        }
        if let Some(dir) = Path::new(HISTORY_FILE).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(HISTORY_FILE, h.join("\n") + "\n");
    });
}

/// The history, oldest first.
pub fn history() -> Vec<String> {
    with_history(|h| h.clone())
}

/// Queue `code` to run on the server thread.
pub fn submit(label: impl Into<String>, code: impl Into<String>) {
    INPUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Request {
            label: label.into(),
            code: code.into(),
        });
}

/// The requests queued since the last take (the server thread's).
pub fn take_requests() -> Vec<Request> {
    std::mem::take(&mut *INPUT.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A line for the window.
pub fn push(kind: Kind, text: impl Into<String>) {
    let mut out = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    for line in text.into().lines() {
        out.push_back((kind, line.to_string()));
    }
    while out.len() > KEEP {
        out.pop_front();
    }
}

/// Everything shown.
pub fn lines() -> Vec<(Kind, String)> {
    OUTPUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

/// Forget the output.
pub fn clear() {
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_inside_an_mc_cmd_string_is_a_command_context() {
        assert_eq!(
            command_context("mc.cmd(\"tp @e\")", 11),
            Some((8, "tp ".to_string()))
        );
        // A leading slash is skipped; the quote kind does not matter.
        assert_eq!(
            command_context("mc.cmd('/kill', 1)", 12),
            Some((9, "kil".to_string()))
        );
        // A closed string, another call, or no string: nothing.
        assert_eq!(command_context("mc.cmd(\"tp\") ", 13), None);
        assert_eq!(command_context("print(\"tp\")", 9), None);
        assert_eq!(command_context("mc.cmd(", 7), None);
        // An escaped quote stays inside the string.
        assert_eq!(
            command_context("mc.cmd(\"say \\\"hi\\\" x", 20),
            Some((8, "say \\\"hi\\\" x".to_string()))
        );
    }

    #[test]
    fn the_innermost_call_and_its_argument_are_found() {
        let c = call_context("mc.watch(\"@e[type=pig]\", \"He", 28).unwrap();
        assert_eq!(c.name, "mc.watch");
        assert_eq!(c.arg, 1);
        assert_eq!(c.string, Some((26, "He".to_string())));
        // Nested: the inner call wins; a closed one is forgotten.
        let c = call_context("print(mc.pos(\"@p", 16).unwrap();
        assert_eq!((c.name.as_str(), c.arg), ("mc.pos", 0));
        let c = call_context("print(mc.pos(\"@p\"), x", 21).unwrap();
        assert_eq!((c.name.as_str(), c.arg, c.string), ("print", 1, None));
        assert_eq!(call_context("x = 1", 5), None);
        assert_eq!(
            entity_context("mc.get(\"@e[ty", 13),
            Some((8, "@e[ty".to_string()))
        );
        assert_eq!(entity_context("mc.get(\"@p\", \"Po", 16), None);
        let c = lua_completions("mc.get(\"@p\", \"Po", 16);
        assert_eq!(
            c.items.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["Pos", "Pos[0]", "Pos[1]", "Pos[2]"]
        );
        assert!(c.hint.unwrap().starts_with("mc.get(entity[, path])"));
    }

    #[test]
    fn lua_names_complete_by_prefix() {
        let c = lua_completions("x = mc.se", 9);
        assert_eq!(c.range, (4, 9));
        let names: Vec<&str> = c.items.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["mc.select(", "mc.set("]);
        // Nothing to complete without a name, and an exact name is not offered again.
        assert!(lua_completions("x = ", 4).items.is_empty());
        assert!(lua_completions("end", 3).items.is_empty());
    }
}
