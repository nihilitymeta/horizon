//! The Lua console's runtime (docs/dev-tools.md): a Luau state on the
//! server thread that runs what the window queued (`dev_tools::console`)
//! once a server tick, after the levels ticked, with an `mc` table bound to
//! this `Game` for the chunk's duration (`Lua::scope`): vanilla commands as
//! the world's owner, entity data as `/data` reads and writes it, selectors,
//! the tools' windows. Tick handlers (`mc.on_tick`) run each tick the same
//! way. A script cannot keep a reference to the game: the scoped functions
//! die with the scope, and `mc` is bound afresh each run.
use super::{Game, Target, data::DataRef, inspect::id_of};
use crate::dev_tools::console::{self, Kind, SCRIPTS_DIR, STOP_TICKS, TICKING};
use horizon_client::{
    command::{Reader, selector},
    data_cmd::{self, Op},
    mobs::observe::{self, View},
    nbt_path::NbtPath,
};
use horizon_world::nbt::{Compound, JavaString, Tag, parse_snbt};
use mlua::{Function, Lua, MultiValue, Table, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

thread_local! {
    /// The server thread's state, made on first use.
    static VM: RefCell<Option<Lua>> = const { RefCell::new(None) };
    /// The entities a selector found, by id, so a number names one later.
    static KNOWN: RefCell<HashMap<u32, Target>> = RefCell::new(HashMap::new());
}

/// The registry key of the tick handlers: a sequence of functions.
const TICKERS: &str = "mc_tickers";
/// Memory a script may take.
const MEMORY: usize = 64 << 20;

fn vm() -> mlua::Result<Lua> {
    VM.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(lua) = slot.as_ref() {
            return Ok(lua.clone());
        }
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY)?;
        lua.set_named_registry_value(TICKERS, lua.create_table()?)?;
        lua.globals().set(
            "print",
            lua.create_function(|_, args: MultiValue| {
                let line: Vec<String> = args
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.to_string_lossy(),
                        other => format!("{other:#?}"),
                    })
                    .collect();
                console::push(Kind::Out, line.join("\t"));
                Ok(())
            })?,
        )?;
        *slot = Some(lua.clone());
        Ok(lua)
    })
}

/// A tag as a Lua value: compounds as tables, lists and arrays as sequences.
fn tag_to_lua(lua: &Lua, tag: &Tag) -> mlua::Result<Value> {
    Ok(match tag {
        Tag::End => Value::Nil,
        Tag::Byte(v) => Value::Integer(i64::from(*v)),
        Tag::Short(v) => Value::Integer(i64::from(*v)),
        Tag::Int(v) => Value::Integer(i64::from(*v)),
        Tag::Long(v) => Value::Integer(*v),
        Tag::Float(bits) => Value::Number(f64::from(f32::from_bits(*bits))),
        Tag::Double(bits) => Value::Number(f64::from_bits(*bits)),
        Tag::String(s) => Value::String(lua.create_string(s.to_string_lossy())?),
        Tag::ByteArray(v) => {
            let t = lua.create_table()?;
            for (i, b) in v.iter().enumerate() {
                t.set(i + 1, i64::from(*b as i8))?;
            }
            Value::Table(t)
        }
        Tag::IntArray(v) => Value::Table(lua.create_sequence_from(v.iter().copied())?),
        Tag::LongArray(v) => Value::Table(lua.create_sequence_from(v.iter().copied())?),
        Tag::List(items) => {
            let t = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                t.set(i + 1, tag_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        Tag::Compound(c) => {
            let t = lua.create_table()?;
            for (k, v) in c {
                t.set(k.to_string_lossy(), tag_to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}

/// A Lua value as SNBT text, for `mc.set` and `mc.merge` given a table or
/// a number rather than SNBT: integers are ints, other numbers doubles,
/// sequences lists, other tables compounds, booleans bytes.
fn lua_to_snbt(v: &Value) -> Result<String, String> {
    Ok(match v {
        Value::Nil => return Err("nil has no SNBT".into()),
        Value::Boolean(b) => if *b { "1b" } else { "0b" }.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => format!("{n}d"),
        Value::String(s) => horizon_world::snbt_text::quote_and_escape(&s.to_string_lossy()),
        Value::Table(t) => {
            let len = t.raw_len();
            if len > 0 {
                let mut items = Vec::new();
                for i in 1..=len {
                    items.push(lua_to_snbt(&t.raw_get::<Value>(i).map_err(|e| e.to_string())?)?);
                }
                format!("[{}]", items.join(","))
            } else {
                let mut entries = Vec::new();
                for pair in t.clone().pairs::<String, Value>() {
                    let (k, v) = pair.map_err(|e| e.to_string())?;
                    entries.push(format!(
                        "{}:{}",
                        horizon_world::snbt_text::quote_and_escape(&k),
                        lua_to_snbt(&v)?
                    ));
                }
                format!("{{{}}}", entries.join(","))
            }
        }
        other => return Err(format!("{} has no SNBT", other.type_name())),
    })
}

/// The SNBT of a value given as SNBT text or as a Lua value.
fn snbt_of(v: &Value) -> Result<Tag, String> {
    let text = match v {
        Value::String(s) => s.to_string_lossy(),
        other => lua_to_snbt(other)?,
    };
    parse_snbt(&text).map(|(tag, _)| tag)
}

fn lua_err(e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(e.to_string())
}

impl Game<'_> {
    /// The entities a Lua argument names: a selector string, or an entity
    /// id a selector found before (players have none).
    fn lua_targets(&mut self, v: &Value) -> Result<Vec<Target>, String> {
        match v {
            Value::Integer(id) => {
                let id = u32::try_from(*id).map_err(|_| format!("bad entity id {id}"))?;
                KNOWN
                    .with(|k| k.borrow().get(&id).copied())
                    .map(|t| vec![t])
                    .ok_or_else(|| format!("unknown entity id {id}: find it with mc.select first"))
            }
            Value::Number(n) => self.lua_targets(&Value::Integer(*n as i64)),
            Value::String(s) => {
                let text = s.to_string_lossy();
                let mut reader = Reader::new(&text);
                let sel = selector::parse(&mut reader, &self.ui.registries, true)
                    .map_err(|e| format!("{text}: {}", e.message.plain()))?;
                let found = self.select(&sel);
                KNOWN.with(|k| {
                    let mut k = k.borrow_mut();
                    for &t in &found {
                        if let Some(id) = id_of(t) {
                            k.insert(id, t);
                        }
                    }
                });
                Ok(found)
            }
            other => Err(format!("{} is not an entity id or a selector", other.type_name())),
        }
    }

    /// The first entity named, with its data.
    fn lua_first_data(&mut self, v: &Value) -> Result<(Target, Tag), String> {
        let t = *self
            .lua_targets(v)?
            .first()
            .ok_or_else(|| "no entity found".to_string())?;
        let data = self
            .in_target_level(t, |g| g.data_of(&DataRef::Entity(t)).ok())
            .flatten()
            .ok_or_else(|| "the entity is gone".to_string())?;
        Ok((t, data))
    }

    /// Run `code` with `mc` bound to this game. Errors go to the window.
    fn lua_run(&mut self, lua: &Lua, label: &str, code: &str) {
        console::push(Kind::Echo, label);
        let game = RefCell::new(self);
        let result = lua.scope(|scope| {
            let mc = lua.create_table()?;
            mc.set(
                "cmd",
                scope.create_function_mut(|lua, line: String| {
                    let mut g = game.borrow_mut();
                    let replies = g.owner_console_command(&line);
                    let ok = !replies.iter().any(|r| r.starts_with("Unknown or incomplete"));
                    Ok((ok, lua.create_sequence_from(replies)?))
                })?,
            )?;
            mc.set(
                "select",
                scope.create_function_mut(|lua, v: Value| {
                    let mut g = game.borrow_mut();
                    let found = g.lua_targets(&v).map_err(lua_err)?;
                    let ids: Vec<u32> = found.iter().filter_map(|t| id_of(*t)).collect();
                    lua.create_sequence_from(ids)
                })?,
            )?;
            mc.set(
                "get",
                scope.create_function_mut(|lua, (v, path): (Value, Option<String>)| {
                    let mut g = game.borrow_mut();
                    let (_, data) = g.lua_first_data(&v).map_err(lua_err)?;
                    let tag = match path {
                        None => data,
                        Some(p) => {
                            let p = NbtPath::parse(&p).map_err(|e| lua_err(format!("{e:?}")))?;
                            data_cmd::single_tag(&p, &data)
                                .map_err(|e| lua_err(e.message.plain()))?
                        }
                    };
                    tag_to_lua(lua, &tag)
                })?,
            )?;
            mc.set(
                "snbt",
                scope.create_function_mut(|_, (v, path): (Value, Option<String>)| {
                    let mut g = game.borrow_mut();
                    let (_, data) = g.lua_first_data(&v).map_err(lua_err)?;
                    let tag = match path {
                        None => data,
                        Some(p) => {
                            let p = NbtPath::parse(&p).map_err(|e| lua_err(format!("{e:?}")))?;
                            data_cmd::single_tag(&p, &data)
                                .map_err(|e| lua_err(e.message.plain()))?
                        }
                    };
                    Ok(horizon_world::snbt_text::to_snbt(&tag))
                })?,
            )?;
            mc.set(
                "set",
                scope.create_function_mut(|_, (v, path, value): (Value, String, Value)| {
                    let mut g = game.borrow_mut();
                    let value = snbt_of(&value).map_err(lua_err)?;
                    let path = NbtPath::parse(&path).map_err(|e| lua_err(format!("{e:?}")))?;
                    let mut changed = 0usize;
                    for t in g.lua_targets(&v).map_err(lua_err)? {
                        let r = DataRef::Entity(t);
                        let done = g.in_target_level(t, |g| {
                            let mut data = g.data_of(&r).ok()?;
                            let n = data_cmd::modify(Op::Set, &mut data, &path, std::slice::from_ref(&value))
                                .ok()?;
                            if n > 0 {
                                g.set_data_of(&r, data).ok()?;
                            }
                            Some(n)
                        });
                        changed += done.flatten().unwrap_or(0);
                    }
                    Ok(changed)
                })?,
            )?;
            mc.set(
                "merge",
                scope.create_function_mut(|_, (v, value): (Value, Value)| {
                    let mut g = game.borrow_mut();
                    let value = snbt_of(&value).map_err(lua_err)?;
                    let mut changed = 0usize;
                    for t in g.lua_targets(&v).map_err(lua_err)? {
                        let r = DataRef::Entity(t);
                        let done = g.in_target_level(t, |g| {
                            let old = g.data_of(&r).ok()?;
                            let new = data_cmd::merged(&old, &value);
                            if new == old {
                                return Some(0);
                            }
                            g.set_data_of(&r, new).ok()?;
                            Some(1)
                        });
                        changed += done.flatten().unwrap_or(0);
                    }
                    Ok(changed)
                })?,
            )?;
            mc.set(
                "pos",
                scope.create_function_mut(|_, v: Value| {
                    let mut g = game.borrow_mut();
                    let (_, data) = g.lua_first_data(&v).map_err(lua_err)?;
                    let Tag::Compound(c) = &data else {
                        return Err(lua_err("no data"));
                    };
                    match c.get(&JavaString::from("Pos")) {
                        Some(Tag::List(p)) if p.len() == 3 => {
                            let n = |t: &Tag| data_cmd::numeric(t).unwrap_or(0.0);
                            Ok((n(&p[0]), n(&p[1]), n(&p[2])))
                        }
                        _ => Err(lua_err("no Pos")),
                    }
                })?,
            )?;
            mc.set(
                "name",
                scope.create_function_mut(|_, v: Value| {
                    let mut g = game.borrow_mut();
                    let t = *g
                        .lua_targets(&v)
                        .map_err(lua_err)?
                        .first()
                        .ok_or_else(|| lua_err("no entity found"))?;
                    Ok(g.in_target_level(t, |g| g.target_name(t).plain())
                        .unwrap_or_else(|| "gone".to_string()))
                })?,
            )?;
            mc.set(
                "inspect",
                scope.create_function_mut(|_, (v, view): (Value, Option<String>)| {
                    let mut g = game.borrow_mut();
                    let ids: Vec<u32> = g
                        .lua_targets(&v)
                        .map_err(lua_err)?
                        .iter()
                        .filter_map(|t| id_of(*t))
                        .collect();
                    let view = match view.as_deref() {
                        None | Some("data") | Some("inspect") => View::Data,
                        Some("goals") => View::Goals,
                        Some("senses") => View::Senses,
                        Some("nav") => View::Nav,
                        Some(other) => {
                            return Err(lua_err(format!(
                                "{other}: a view is data, goals, senses or nav"
                            )));
                        }
                    };
                    super::inspect::remember_ids(&ids);
                    observe::add(&ids, view);
                    Ok(ids.len())
                })?,
            )?;
            mc.set(
                "watch",
                scope.create_function_mut(|_, (v, path): (Value, String)| {
                    let mut g = game.borrow_mut();
                    NbtPath::parse(&path).map_err(|e| lua_err(format!("{e:?}")))?;
                    let ids: Vec<u32> = g
                        .lua_targets(&v)
                        .map_err(lua_err)?
                        .iter()
                        .filter_map(|t| id_of(*t))
                        .collect();
                    super::inspect::remember_ids(&ids);
                    observe::add_path(&ids, &path);
                    Ok(ids.len())
                })?,
            )?;
            mc.set(
                "on_tick",
                lua.create_function(|lua, f: Function| {
                    let tickers: Table = lua.named_registry_value(TICKERS)?;
                    tickers.push(f)?;
                    TICKING.store(true, Ordering::Relaxed);
                    Ok(())
                })?,
            )?;
            mc.set(
                "off_tick",
                lua.create_function(|lua, ()| {
                    lua.set_named_registry_value(TICKERS, lua.create_table()?)?;
                    TICKING.store(false, Ordering::Relaxed);
                    Ok(())
                })?,
            )?;
            mc.set(
                "run",
                lua.create_function(|lua, file: String| {
                    let path = std::path::Path::new(SCRIPTS_DIR).join(&file);
                    let code = std::fs::read_to_string(&path)
                        .map_err(|e| lua_err(format!("{}: {e}", path.display())))?;
                    lua.load(&code).set_name(file).exec()
                })?,
            )?;
            mc.set(
                "scripts_dir",
                lua.create_function(|_, ()| {
                    Ok(std::path::absolute(SCRIPTS_DIR)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| SCRIPTS_DIR.to_string()))
                })?,
            )?;
            mc.set(
                "log",
                lua.create_function(|_, msg: String| {
                    log::info!("[lua console] {msg}");
                    console::push(Kind::Out, msg);
                    Ok(())
                })?,
            )?;
            lua.globals().set("mc", mc)?;
            lua.load(code).set_name(label).exec()
        });
        if let Err(e) = result {
            console::push(Kind::Err, e.to_string());
        }
    }

    /// Once a frame on the window's game: the completions the console asked
    /// for. Inside `mc.cmd("…")` they are the chat box's own, from the
    /// tree the server sent; elsewhere the `mc` names.
    pub(in crate::game) fn dev_console_complete(&mut self) {
        let Some((text, cursor)) = console::take_completion_request() else {
            return;
        };
        let tree_completions = |start: usize, command: &str, skip: usize| {
            // `skip` bytes of `command` are a prefix the line does not hold.
            let tree = crate::game::chat::chat_tree(&self.remote);
            let parse = tree.parse(command, &self.ui.registries);
            let (range, list) = tree.client_suggestions(&parse, command.len(), &self.ui.registries);
            let map = |i: usize| start + i.saturating_sub(skip);
            console::Completions {
                text: text.clone(),
                cursor,
                range: (map(range.0), map(range.1)),
                items: list.into_iter().map(|s| (s.text, s.tooltip)).collect(),
                hint: console::call_context(&text, cursor)
                    .and_then(|c| console::signature_hint(&c.name)),
            }
        };
        let completions = if let Some((start, command)) = console::command_context(&text, cursor)
        {
            tree_completions(start, &command, 0)
        } else if let Some((start, partial)) = console::entity_context(&text, cursor) {
            // An entity argument completes as /tp's target does.
            tree_completions(start, &format!("tp {partial}"), 3)
        } else {
            console::lua_completions(&text, cursor)
        };
        console::set_completions(completions);
    }

    /// `/lua`: the console window; `/lua <code>` runs a line in it.
    pub(super) fn cmd_lua(&mut self, code: Option<String>) -> super::R<i32> {
        match code {
            None => {
                let open = !console::OPEN.load(Ordering::Relaxed);
                if open {
                    console::open_from_command();
                } else {
                    console::OPEN.store(false, Ordering::Relaxed);
                }
                self.feedback(horizon_client::text::Text::literal(if open {
                    "Lua console open"
                } else {
                    "Lua console closed"
                }));
                Ok(1)
            }
            Some(code) => {
                console::open_from_command();
                console::submit(format!("> {code}"), code);
                Ok(1)
            }
        }
    }

    /// Once a server tick, after every level ticked: what the window
    /// queued, then the tick handlers.
    pub(in crate::game) fn dev_console_tick(&mut self) {
        let requests = console::take_requests();
        if STOP_TICKS.swap(false, Ordering::Relaxed)
            && let Ok(lua) = vm()
            && let Ok(t) = lua.create_table()
        {
            let _ = lua.set_named_registry_value(TICKERS, t);
            TICKING.store(false, Ordering::Relaxed);
            console::push(Kind::Out, "tick handlers stopped");
        }
        if requests.is_empty() && !TICKING.load(Ordering::Relaxed) {
            return;
        }
        let lua = match vm() {
            Ok(lua) => lua,
            Err(e) => {
                console::push(Kind::Err, format!("Lua: {e}"));
                return;
            }
        };
        for r in requests {
            self.lua_run(&lua, &r.label, &r.code);
        }
        if TICKING.load(Ordering::Relaxed) {
            // The handlers call `mc`, which the scope of a run binds: run
            // them as one chunk that calls each.
            self.lua_run_tickers(&lua);
        }
    }

    fn lua_run_tickers(&mut self, lua: &Lua) {
        let code = "for _, f in ipairs(mc_tickers_now) do f() end";
        let tickers: Table = match lua.named_registry_value(TICKERS) {
            Ok(t) => t,
            Err(_) => return,
        };
        if tickers.raw_len() == 0 {
            TICKING.store(false, Ordering::Relaxed);
            return;
        }
        if lua.globals().set("mc_tickers_now", tickers).is_err() {
            return;
        }
        let before = console::lines().len();
        self.lua_run(lua, "", code);
        // An error in a handler stops them all: a tick loop that fails
        // every tick would flood the window.
        let failed = console::lines()
            .iter()
            .skip(before)
            .any(|(k, _)| *k == Kind::Err);
        if failed && let Ok(t) = lua.create_table() {
            let _ = lua.set_named_registry_value(TICKERS, t);
            TICKING.store(false, Ordering::Relaxed);
            console::push(Kind::Err, "tick handlers stopped after the error");
        }
    }
}

/// An empty compound, for scripts that build data up.
#[allow(dead_code)]
fn empty() -> Tag {
    Tag::Compound(Compound::new())
}
