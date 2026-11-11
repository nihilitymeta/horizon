//! The Lua console's runtime (docs/dev-tools.md): a Luau state on the
//! server thread that runs what the window queued (`dev_input::console`)
//! once a server tick, after the levels ticked, with an `mc` table bound to
//! the host for the chunk's duration (`Lua::scope`): vanilla commands as
//! the world's owner, entity data as `/data` reads and writes it, selectors,
//! the tools' windows. Tick handlers (`mc.on_tick`) run each tick the same
//! way. A script cannot keep the host: the scoped functions die with the
//! scope, and `mc` is bound afresh each run.
//!
//! The console is a built-in mod ([`Console`]): the engine's own code only
//! keeps the `/lua` command and the window's completions, and the console
//! reads and writes the world through the engine's generic host helpers
//! (`select_ids`, `entity_data_in_place`, `set_entity_data`,
//! `entity_name_in_place`, `run_command_as_owner`, `level_dimension`).
use crate::lua_mods;
use event_horizon_observe::{self as observe, View};
use horizon_client::data_cmd::{self, Op};
use horizon_client::dev_input::console::{self, Kind, SCRIPTS_DIR, STOP_TICKS, TICKING};
use horizon_client::mod_api::{
    Hooks, Host, ServerTickEvent, ServerTickPhase, WorldLifecycleEvent, WorldPhase,
};
use horizon_client::nbt_path::NbtPath;
use horizon_world::nbt::{Compound, JavaString, Tag, parse_snbt};
use mlua::{Function, Lua, MultiValue, Table, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

thread_local! {
    /// The server thread's state, made on first use.
    static VM: RefCell<Option<Lua>> = const { RefCell::new(None) };
    /// The entities a selector found, by id, so a number names one later.
    static KNOWN: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
    /// When the chunk running now must have finished: the interrupt stops it after.
    static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The registry key of the tick handlers: a sequence of functions.
const TICKERS: &str = "mc_tickers";
/// The world closed (`world_closed`): the next server tick drops the tick
/// handlers and the ids a selector found, which named that world's entities.
static RESET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Memory a script may take.
const MEMORY: usize = 64 << 20;
/// Wall-clock time one chunk (a line, a script, a tick's handlers) may take:
/// the server thread runs it, so an endless loop would freeze the game.
const BUDGET: Duration = Duration::from_secs(2);

/// The console as a built-in mod: it runs the window's lines and the tick
/// handlers once a server tick, and drops its state when the world closes.
pub struct Console;

impl Hooks for Console {
    /// Once a server tick, after every level ticked, on a world that runs:
    /// what the window queued, then the tick handlers.
    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        if matches!(
            event.phase,
            ServerTickPhase::ServerTickEnd {
                world_running: true
            }
        ) {
            tick(host);
        }
    }
    /// The world closed: the console window closes, nothing of it asks for
    /// the pointer any more, and the next server tick (of the next world)
    /// drops the tick handlers and the ids a selector found, which named
    /// this world's entities. The output and the history stay: what was
    /// printed is still worth reading.
    fn world_lifecycle(&self, _host: &mut dyn Host, event: &mut WorldLifecycleEvent<'_>) {
        if event.phase == WorldPhase::WorldCloseFinal {
            world_closed();
        }
    }
}

/// The console's mod, for the program crate to install with the others.
pub fn mods() -> Vec<Box<dyn Hooks>> {
    vec![Box::new(Console)]
}

fn vm() -> mlua::Result<Lua> {
    VM.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(lua) = slot.as_ref() {
            return Ok(lua.clone());
        }
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY)?;
        lua.set_interrupt(|_| {
            let over = DEADLINE.with(|d| d.get().is_some_and(|at| Instant::now() > at));
            if over {
                Err(mlua::Error::runtime(format!(
                    "ran over {BUDGET:?}: stopped"
                )))
            } else {
                Ok(mlua::VmState::Continue)
            }
        });
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
                    items.push(lua_to_snbt(
                        &t.raw_get::<Value>(i).map_err(|e| e.to_string())?,
                    )?);
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

/// The entity ids a Lua argument names: a selector string, or an entity id
/// a selector found before (a player's is its network entity id).
fn targets(host: &mut dyn Host, v: &Value) -> Result<Vec<u32>, String> {
    match v {
        Value::Integer(id) => {
            let id = u32::try_from(*id).map_err(|_| format!("bad entity id {id}"))?;
            if KNOWN.with(|k| k.borrow().contains(&id)) {
                Ok(vec![id])
            } else {
                Err(format!(
                    "unknown entity id {id}: find it with mc.select first"
                ))
            }
        }
        Value::Number(n) => targets(host, &Value::Integer(*n as i64)),
        Value::String(s) => {
            let ids = host.select_ids(&s.to_string_lossy());
            KNOWN.with(|k| {
                let mut k = k.borrow_mut();
                for id in &ids {
                    k.insert(*id);
                }
            });
            Ok(ids)
        }
        other => Err(format!(
            "{} is not an entity id or a selector",
            other.type_name()
        )),
    }
}

/// The first entity named, with its data.
fn first_data(host: &mut dyn Host, v: &Value) -> Result<Tag, String> {
    let id = *targets(host, v)?
        .first()
        .ok_or_else(|| "no entity found".to_string())?;
    let data = host
        .entity_data_in_place(id)
        .map(Tag::Compound)
        .ok_or_else(|| "the entity is gone".to_string())?;
    Ok(data)
}

/// The tag a path names in the data, or the data itself.
fn tag_of(data: &Tag, path: Option<String>) -> Result<Tag, String> {
    match path {
        None => Ok(data.clone()),
        Some(p) => {
            let p = NbtPath::parse(&p).map_err(|e| format!("{e:?}"))?;
            data_cmd::single_tag(&p, data).map_err(|e| e.message.plain())
        }
    }
}

/// Containers nested deeper than this are refused: a reply a reader can parse,
/// and a recursion that cannot overflow the stack.
const MAX_LEVELS: usize = 64;

/// A saved tag as JSON, exactly: a byte, short, int or long is its number
/// (a byte is not a flag here). A float or double is its number widened to
/// f64 (a float 0.1 prints as 0.10000000149011612); NaN and the infinities
/// have no JSON form and become null. Arrays and lists are arrays, compounds
/// objects. Nesting past `MAX_LEVELS` is an error.
/// `tag` at `level`: the top-level tag is level 1, its children level 2.
fn tag_at(tag: &Tag, level: usize) -> Result<serde_json::Value, String> {
    use serde_json::{Number, Value as Json};
    let float = |x: f64| Number::from_f64(x).map_or(Json::Null, Json::Number);
    Ok(match tag {
        Tag::End => Json::Null,
        Tag::Byte(v) => Json::from(*v),
        Tag::Short(v) => Json::from(*v),
        Tag::Int(v) => Json::from(*v),
        Tag::Long(v) => Json::from(*v),
        Tag::Float(bits) => float(f64::from(f32::from_bits(*bits))),
        Tag::Double(bits) => float(f64::from_bits(*bits)),
        Tag::String(s) => Json::String(s.to_string_lossy()),
        Tag::ByteArray(v) => Json::Array(v.iter().map(|b| Json::from(*b as i8)).collect()),
        Tag::IntArray(v) => Json::Array(v.iter().map(|n| Json::from(*n)).collect()),
        Tag::LongArray(v) => Json::Array(v.iter().map(|n| Json::from(*n)).collect()),
        Tag::List(items) => {
            check_level(level)?;
            Json::Array(
                items
                    .iter()
                    .map(|t| tag_at(t, level + 1))
                    .collect::<Result<_, _>>()?,
            )
        }
        Tag::Compound(c) => compound_at(c, level)?,
    })
}

fn check_level(level: usize) -> Result<(), String> {
    if level > MAX_LEVELS {
        Err(format!("nests deeper than {MAX_LEVELS} levels"))
    } else {
        Ok(())
    }
}

fn compound_at(c: &Compound, level: usize) -> Result<serde_json::Value, String> {
    check_level(level)?;
    let mut object = serde_json::Map::new();
    for (k, v) in c.iter() {
        object.insert(k.to_string_lossy(), tag_at(v, level + 1)?);
    }
    Ok(serde_json::Value::Object(object))
}

fn compound_json(c: &Compound) -> Result<serde_json::Value, String> {
    compound_at(c, 1)
}

/// What `mc.mod_state_json` prints: `{"world_data": ..., "players": {uuid: ...}}`.
/// A table too deep is an error that names it.
fn mod_state_json(
    world: &Compound,
    players: &[(String, Compound)],
) -> Result<serde_json::Value, String> {
    let world_data = compound_json(world).map_err(|e| format!("world_data {e}"))?;
    let mut by_uuid = serde_json::Map::new();
    for (uuid, data) in players {
        let data = compound_json(data).map_err(|e| format!("a player's data {e}"))?;
        by_uuid.insert(uuid.clone(), data);
    }
    Ok(serde_json::json!({
        "world_data": world_data,
        "players": by_uuid,
    }))
}

/// A loaded scripted mod's saved data (`lua_mods::saved_data`). An id that
/// names no loaded mod is refused before anything is read.
fn mod_saved(host: &mut dyn Host, id: &str) -> Result<(Compound, Vec<(String, Compound)>), String> {
    if !horizon_client::dev_input::mods::status()
        .iter()
        .any(|m| m.id == id)
    {
        return Err(format!("unknown mod {id}"));
    }
    lua_mods::saved_data(host, id)
}

/// Run `code` with `mc` bound to the host. Errors go to the window.
/// One profiler row's numbers as a Lua table: `calls`, `total_ms`,
/// `max_ms` (fractional milliseconds) and `overruns`.
fn profile_stats(lua: &Lua, row: &crate::profiler::ProfileRow) -> mlua::Result<Table> {
    let stats = lua.create_table()?;
    stats.set("calls", row.calls)?;
    stats.set("total_ms", row.total_ns as f64 / 1_000_000.0)?;
    stats.set("max_ms", row.max_ns as f64 / 1_000_000.0)?;
    stats.set("overruns", row.overruns)?;
    Ok(stats)
}

fn lua_run(host: &mut dyn Host, lua: &Lua, label: &str, code: &str) {
    console::push(Kind::Echo, label);
    let host = RefCell::new(host);
    DEADLINE.with(|d| d.set(Some(Instant::now() + BUDGET)));
    let result = lua.scope(|scope| {
        let mc = lua.create_table()?;
        mc.set(
            "cmd",
            scope.create_function_mut(|lua, line: String| {
                let mut h = host.borrow_mut();
                let replies = h.run_command_as_owner(&line);
                let ok = !replies
                    .iter()
                    .any(|r| r.starts_with("Unknown or incomplete"));
                Ok((ok, lua.create_sequence_from(replies)?))
            })?,
        )?;
        mc.set(
            "select",
            scope.create_function_mut(|lua, v: Value| {
                let mut h = host.borrow_mut();
                let found = targets(&mut **h, &v).map_err(lua_err)?;
                lua.create_sequence_from(found)
            })?,
        )?;
        mc.set(
            "get",
            scope.create_function_mut(|lua, (v, path): (Value, Option<String>)| {
                let mut h = host.borrow_mut();
                let data = first_data(&mut **h, &v).map_err(lua_err)?;
                let tag = tag_of(&data, path).map_err(lua_err)?;
                tag_to_lua(lua, &tag)
            })?,
        )?;
        mc.set(
            "snbt",
            scope.create_function_mut(|_, (v, path): (Value, Option<String>)| {
                let mut h = host.borrow_mut();
                let data = first_data(&mut **h, &v).map_err(lua_err)?;
                let tag = tag_of(&data, path).map_err(lua_err)?;
                Ok(horizon_world::snbt_text::to_snbt(&tag))
            })?,
        )?;
        mc.set(
            "set",
            scope.create_function_mut(|_, (v, path, value): (Value, String, Value)| {
                let mut h = host.borrow_mut();
                let value = snbt_of(&value).map_err(lua_err)?;
                let path = NbtPath::parse(&path).map_err(|e| lua_err(format!("{e:?}")))?;
                let mut changed = 0usize;
                for id in targets(&mut **h, &v).map_err(lua_err)? {
                    let Some(data) = h.entity_data_in_place(id) else {
                        continue;
                    };
                    let mut data = Tag::Compound(data);
                    let Ok(n) =
                        data_cmd::modify(Op::Set, &mut data, &path, std::slice::from_ref(&value))
                    else {
                        continue;
                    };
                    if n > 0
                        && let Tag::Compound(compound) = &data
                        && h.set_entity_data(id, compound.clone())
                    {
                        changed += n;
                    }
                }
                Ok(changed)
            })?,
        )?;
        mc.set(
            "merge",
            scope.create_function_mut(|_, (v, value): (Value, Value)| {
                let mut h = host.borrow_mut();
                let value = snbt_of(&value).map_err(lua_err)?;
                let mut changed = 0usize;
                for id in targets(&mut **h, &v).map_err(lua_err)? {
                    let Some(data) = h.entity_data_in_place(id) else {
                        continue;
                    };
                    let old = Tag::Compound(data);
                    let new = data_cmd::merged(&old, &value);
                    if new == old {
                        continue;
                    }
                    let Tag::Compound(compound) = &new else {
                        continue;
                    };
                    if h.set_entity_data(id, compound.clone()) {
                        changed += 1;
                    }
                }
                Ok(changed)
            })?,
        )?;
        mc.set(
            "pos",
            scope.create_function_mut(|_, v: Value| {
                let mut h = host.borrow_mut();
                let data = first_data(&mut **h, &v).map_err(lua_err)?;
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
                let mut h = host.borrow_mut();
                let id = *targets(&mut **h, &v)
                    .map_err(lua_err)?
                    .first()
                    .ok_or_else(|| lua_err("no entity found"))?;
                Ok(h.entity_name_in_place(id)
                    .unwrap_or_else(|| "gone".to_string()))
            })?,
        )?;
        mc.set(
            "inspect",
            scope.create_function_mut(|_, (v, view): (Value, Option<String>)| {
                let mut h = host.borrow_mut();
                let ids = targets(&mut **h, &v).map_err(lua_err)?;
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
                let dim = h.level_dimension();
                remember(&ids, &dim);
                observe::add(&ids, view);
                Ok(ids.len())
            })?,
        )?;
        mc.set(
            "watch",
            scope.create_function_mut(|_, (v, path): (Value, String)| {
                let mut h = host.borrow_mut();
                NbtPath::parse(&path).map_err(|e| lua_err(format!("{e:?}")))?;
                let ids = targets(&mut **h, &v).map_err(lua_err)?;
                let dim = h.level_dimension();
                remember(&ids, &dim);
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
                // A name in the folder, not a path: the console reaches no other file.
                let mut parts = std::path::Path::new(&file).components();
                let plain = matches!(parts.next(), Some(std::path::Component::Normal(_)))
                    && parts.next().is_none();
                if !plain {
                    return Err(lua_err(format!(
                        "{file}: a script is named by its file name in {SCRIPTS_DIR}/"
                    )));
                }
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
            "mods",
            lua.create_function(|lua, ()| {
                let list = lua.create_table()?;
                for m in horizon_client::dev_input::mods::status() {
                    let t = lua.create_table()?;
                    t.set("id", m.id)?;
                    t.set("side", m.side)?;
                    t.set("source", m.source)?;
                    t.set("scripts", m.scripts)?;
                    t.set("reloads", m.reloads)?;
                    let handlers = lua.create_table()?;
                    for (name, n) in m.handlers {
                        handlers.set(name, n)?;
                    }
                    t.set("handlers", handlers)?;
                    t.set("goals", lua.create_sequence_from(m.goals)?)?;
                    t.set("error", m.error)?;
                    list.push(t)?;
                }
                Ok(list)
            })?,
        )?;
        mc.set(
            "mod_state",
            scope.create_function_mut(|lua, id: String| {
                // The saved tables as Lua tables: `world_data`, and `players`
                // keyed by uuid text. A Lua number is a double, so a long past
                // 2^53 loses digits here; `mc.mod_state_json` keeps them.
                let mut h = host.borrow_mut();
                let (world, players) = mod_saved(&mut **h, &id).map_err(lua_err)?;
                let t = lua.create_table()?;
                t.set("world_data", tag_to_lua(lua, &Tag::Compound(world))?)?;
                let by_uuid = lua.create_table()?;
                for (uuid, data) in players {
                    by_uuid.set(uuid, tag_to_lua(lua, &Tag::Compound(data))?)?;
                }
                t.set("players", by_uuid)?;
                Ok(t)
            })?,
        )?;
        mc.set(
            "mod_state_json",
            scope.create_function_mut(|_, id: String| {
                // What `horizon-ctl mod-state` prints: the same data as JSON text.
                let mut h = host.borrow_mut();
                let (world, players) = mod_saved(&mut **h, &id).map_err(lua_err)?;
                Ok(mod_state_json(&world, &players)
                    .map_err(lua_err)?
                    .to_string())
            })?,
        )?;
        mc.set(
            "emit_mod_state",
            scope.create_function_mut(|_, id: String| {
                // The one line `horizon-ctl mod-state` reads: marked, so the
                // run's other lines (a prints, a hook's log) are not taken for it.
                let mut h = host.borrow_mut();
                let (world, players) = mod_saved(&mut **h, &id).map_err(lua_err)?;
                let json = mod_state_json(&world, &players).map_err(lua_err)?;
                console::push(Kind::Out, format!("{}{json}", console::MOD_STATE_MARK));
                Ok(())
            })?,
        )?;
        mc.set(
            "reload_mods",
            lua.create_function(|_, (id, side): (Option<String>, Option<String>)| {
                let targets = match horizon_client::dev_input::mods::reload_targets(
                    id.as_deref(),
                    side.as_deref(),
                ) {
                    Ok(v) => v,
                    Err(e) => return Err(mlua::Error::runtime(e)),
                };
                if targets.is_empty() {
                    // A side with no loaded state at all (a dedicated
                    // server has no client states): an error, not silence.
                    if let Some(s) = side.as_deref()
                        && !s.is_empty()
                        && s != "both"
                    {
                        let has = horizon_client::dev_input::mods::status()
                            .iter()
                            .any(|m| m.side == s);
                        if !has {
                            return Err(mlua::Error::runtime(format!("no {s} state is loaded")));
                        }
                    }
                    // No targets for a side-less request: behave as before
                    // (request_reload(None, None) just bumps the generation).
                    horizon_client::dev_input::mods::request_reload(id.as_deref(), side.as_deref());
                    return Ok(());
                }
                horizon_client::dev_input::mods::request_reload(id.as_deref(), side.as_deref());
                Ok(())
            })?,
        )?;
        mc.set(
            "profile",
            lua.create_function(|lua, ()| {
                // The profiler's rows keyed by mod id and side then hook name,
                // for lookups (`pairs` over it has no order: see
                // `profile_rows`). A server state keeps the bare `<id>` key
                // (unchanged for existing scripts); a client state is
                // `<id>/client`. Each hook's table carries `calls`,
                // `total_ms`, `max_ms` and `overruns` since the last
                // `mc.profile_reset()`.
                let out = lua.create_table()?;
                for row in crate::profiler::profile_report() {
                    let key = if row.side == "client" {
                        format!("{}/client", row.r#mod)
                    } else {
                        row.r#mod.clone()
                    };
                    let by_hook: Table = match out.get::<Value>(key.as_str())? {
                        Value::Table(t) => t,
                        _ => {
                            let t = lua.create_table()?;
                            out.set(key.as_str(), t.clone())?;
                            t
                        }
                    };
                    by_hook.set(row.hook.as_str(), profile_stats(lua, &row)?)?;
                }
                Ok(out)
            })?,
        )?;
        mc.set(
            "profile_rows",
            lua.create_function(|lua, ()| {
                // The same rows as an array sorted by mod id, side, then hook
                // name, each with `mod`, `side` and `hook` beside its numbers.
                let list = lua.create_table()?;
                for row in crate::profiler::profile_report() {
                    let stats = profile_stats(lua, &row)?;
                    stats.set("mod", row.r#mod.as_str())?;
                    stats.set("side", row.side)?;
                    stats.set("hook", row.hook.as_str())?;
                    list.push(stats)?;
                }
                Ok(list)
            })?,
        )?;
        mc.set(
            "profile_reset",
            lua.create_function(|_, ()| {
                crate::profiler::profile_reset();
                Ok(())
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
    DEADLINE.with(|d| d.set(None));
    if let Err(e) = result {
        console::push(Kind::Err, e.to_string());
    }
}

/// Remember the entities the console named, so the tick reads their data:
/// those with an id here, read in `dimension`.
fn remember(ids: &[u32], dimension: &str) {
    let entries: Vec<(u32, Option<String>)> = ids
        .iter()
        .map(|id| (*id, Some(dimension.to_owned())))
        .collect();
    observe::remember(&entries);
}

/// Once a server tick, after every level ticked: what the window queued,
/// then the tick handlers.
fn tick(host: &mut dyn Host) {
    if RESET.swap(false, Ordering::Relaxed) {
        // The world the handlers and the known ids belonged to is gone.
        KNOWN.with(|k| k.borrow_mut().clear());
        if TICKING.swap(false, Ordering::Relaxed)
            && let Ok(lua) = vm()
            && let Ok(t) = lua.create_table()
        {
            let _ = lua.set_named_registry_value(TICKERS, t);
            console::push(Kind::Out, "tick handlers stopped: the world closed");
        }
    }
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
        lua_run(host, &lua, &r.label, &r.code);
    }
    if TICKING.load(Ordering::Relaxed) {
        // The handlers call `mc`, which the scope of a run binds: run
        // them as one chunk that calls each.
        lua_run_tickers(host, &lua);
    }
}

fn lua_run_tickers(host: &mut dyn Host, lua: &Lua) {
    // A tick handler's error aborts the chunk through `exec`, so mlua's own
    // call handler attaches the traceback with the handler's frames.
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
    lua_run(host, lua, "", code);
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

/// The world closed: the console window closes, nothing of it asks for the
/// pointer any more, and the next server tick (of the next world) drops the
/// tick handlers and the ids a selector found, which named this world's
/// entities. The output and the history stay: what was printed is still
/// worth reading.
fn world_closed() {
    console::OPEN.store(false, Ordering::Relaxed);
    console::WANT_POINTER.store(false, Ordering::Relaxed);
    RESET.store(true, Ordering::Relaxed);
}

/// An empty compound, for scripts that build data up.
#[allow(dead_code)]
fn empty() -> Tag {
    Tag::Compound(Compound::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Lua value becomes SNBT the way `/data modify` takes it: an integer
    /// an int, another number a double, a list a list, a table a compound.
    #[test]
    fn a_lua_value_becomes_snbt() {
        let lua = Lua::new();
        let value = lua
            .load(r#"return {Health = 20, Name = "pig", Pos = {1, 2, 3}, Air = 1.5}"#)
            .eval::<Value>()
            .unwrap();
        let text = lua_to_snbt(&value).unwrap();
        // The keys of a Lua table are in no order of their own, so the
        // compound is read back and compared as one.
        let (tag, _) = parse_snbt(&text).unwrap();
        let Tag::Compound(compound) = tag else {
            panic!("a table is a compound: {text}");
        };
        assert_eq!(
            compound.get(&JavaString::from("Health")),
            Some(&Tag::Int(20))
        );
        assert_eq!(
            compound.get(&JavaString::from("Name")),
            Some(&Tag::String("pig".into()))
        );
        assert_eq!(
            compound.get(&JavaString::from("Air")),
            Some(&Tag::Double(1.5f64.to_bits()))
        );
        assert_eq!(
            compound.get(&JavaString::from("Pos")),
            Some(&Tag::List(vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)]))
        );
        // A boolean is a byte, a nil has no SNBT.
        let yes = lua.load("return true").eval::<Value>().unwrap();
        assert_eq!(lua_to_snbt(&yes).unwrap(), "1b");
        let nothing = lua.load("return nil").eval::<Value>().unwrap();
        assert!(lua_to_snbt(&nothing).is_err());
    }

    /// A value given as SNBT text is read as it stands, so the types of the
    /// text are kept.
    #[test]
    fn snbt_text_is_read_as_it_stands() {
        let lua = Lua::new();
        let value = lua.load(r#"return "20.0f""#).eval::<Value>().unwrap();
        let tag = snbt_of(&value).unwrap();
        assert!(matches!(tag, Tag::Float(_)), "{tag:?}");
        // A Lua table is converted first.
        let value = lua.load("return {a = 1}").eval::<Value>().unwrap();
        let tag = snbt_of(&value).unwrap();
        assert_eq!(
            tag,
            Tag::Compound(Compound::from_iter([(JavaString::from("a"), Tag::Int(1))]))
        );
    }

    /// A host that answers the console's calls with fixed values, so a
    /// script can be run without a world.
    #[derive(Default)]
    struct TestHost {
        /// The data the script wrote.
        written: std::cell::RefCell<Vec<(u32, Compound)>>,
    }

    impl Host for TestHost {
        fn select_ids(&mut self, selector: &str) -> Vec<u32> {
            if selector == "@e[type=pig]" {
                vec![7]
            } else {
                Vec::new()
            }
        }
        fn entity_data_in_place(&mut self, id: u32) -> Option<Compound> {
            if id != 7 {
                return None;
            }
            let mut data = Compound::new();
            data.insert(JavaString::from("Health"), Tag::Float(20.0f32.to_bits()));
            Some(data)
        }
        fn set_entity_data(&mut self, id: u32, tag: Compound) -> bool {
            self.written.borrow_mut().push((id, tag));
            true
        }
        fn entity_name_in_place(&mut self, id: u32) -> Option<String> {
            (id == 7).then(|| "pig".to_string())
        }
        fn run_command_as_owner(&mut self, line: &str) -> Vec<String> {
            vec![format!("ran {line}")]
        }
        fn level_dimension(&self) -> String {
            "minecraft:overworld".to_string()
        }
    }

    /// A script reads and writes the world through the host: the ids a
    /// selector finds, an entity's data, and the data it writes back.
    #[test]
    fn a_script_reads_and_writes_the_world_through_the_host() {
        let mut host = TestHost::default();
        let lua = vm().expect("a state");
        console::clear();
        lua_run(
            &mut host,
            &lua,
            "the console's test",
            r#"
                print("id", mc.select("@e[type=pig]")[1])
                print("health", mc.get("@e[type=pig]", "Health"))
                print("name", mc.name("@e[type=pig]"))
                print("changed", mc.set("@e[type=pig]", "Health", "5.0f"))
            "#,
        );
        let text: Vec<String> = console::lines().into_iter().map(|(_, l)| l).collect();
        assert!(
            text.iter().any(|l| l == "id\t7"),
            "the selector's id: {text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("20")),
            "the entity's health: {text:?}"
        );
        assert!(
            text.iter().any(|l| l == "name\tpig"),
            "the entity's name: {text:?}"
        );
        assert!(
            text.iter().any(|l| l == "changed\t1"),
            "the number of values changed: {text:?}"
        );
        // The data the script wrote went back to the host, with the new
        // health in it.
        let written = host.written.borrow();
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(written[0].0, 7);
        assert_eq!(
            written[0].1.get(&JavaString::from("Health")),
            Some(&Tag::Float(5.0f32.to_bits()))
        );
    }

    /// An id no selector found is refused: a script cannot name an entity
    /// the world never showed it.
    #[test]
    fn an_id_no_selector_found_is_refused() {
        let mut host = TestHost::default();
        let lua = vm().expect("a state");
        console::clear();
        lua_run(
            &mut host,
            &lua,
            "the console's test",
            r#"print(mc.get(99, "Health"))"#,
        );
        let text: Vec<String> = console::lines().into_iter().map(|(_, l)| l).collect();
        assert!(
            text.iter().any(|l| l.contains("unknown entity id 99")),
            "the id is refused: {text:?}"
        );
    }

    /// The profiler's rows in the console, from a fresh state: the Lua side
    /// stores what it read in globals, and the test reads them back.
    fn profile_in_lua(code: &str) -> Lua {
        let mut host = TestHost::default();
        let lua = vm().expect("a state");
        lua_run(&mut host, &lua, "the console's test", code);
        lua
    }

    /// `mc.profile_rows()` is the report as an array sorted by mod, side,
    /// then hook (a `pairs` over `mc.profile()` has no order to test), and
    /// `mc.profile_reset()` clears the rows.
    #[test]
    fn profile_rows_are_sorted_and_reset_clears_them() {
        let _lock = crate::profiler::test_lock();
        let d = std::time::Duration::from_millis(1);
        crate::profiler::note("console_order_b", "server", "a_hook", d, false);
        crate::profiler::note("console_order_a", "server", "z_hook", d, false);
        crate::profiler::note("console_order_a", "server", "b_hook", d, false);
        let lua = profile_in_lua(
            r#"
            local names = {}
            for _, row in ipairs(mc.profile_rows()) do
              if string.sub(row.mod, 1, 14) == "console_order_" then
                names[#names + 1] = row.mod .. "/" .. row.hook
              end
            end
            ORDER = table.concat(names, ",")
            mc.profile_reset()
            "#,
        );
        let order: String = lua.globals().get("ORDER").unwrap();
        assert_eq!(
            order,
            "console_order_a/b_hook,console_order_a/z_hook,console_order_b/a_hook"
        );
        assert!(
            crate::profiler::profile_report()
                .iter()
                .all(|r| !r.r#mod.starts_with("console_order_")),
            "the console's reset clears the rows"
        );
    }

    /// The console splits the two states of one mod: `mc.profile_rows()`
    /// sorts by side and carries it, and the nested `mc.profile()` keys a
    /// client state as `<id>/client` while a server-only mod keeps the bare
    /// `<id>` key.
    #[test]
    fn profile_splits_one_mod_by_side_and_keeps_server_key() {
        let _lock = crate::profiler::test_lock();
        let d = std::time::Duration::from_millis(1);
        crate::profiler::note("console_side_both", "server", "can_eat", d, false);
        crate::profiler::note("console_side_both", "client", "can_eat", d, false);
        crate::profiler::note(
            "console_side_server_only",
            "server",
            "mob_spawned",
            d,
            false,
        );
        let lua = profile_in_lua(
            r#"
            local seen = {}
            for _, row in ipairs(mc.profile_rows()) do
              if string.sub(row.mod, 1, 13) == "console_side_" then
                seen[#seen + 1] = row.mod .. "/" .. row.side .. "/" .. row.hook
              end
            end
            ORDER = table.concat(seen, ",")
            local both = mc.profile()
            HAS_SERVER = both["console_side_both"] ~= nil
            HAS_CLIENT = both["console_side_both/client"] ~= nil
            SERVER_ONLY = both["console_side_server_only"] ~= nil
            SERVER_ONLY_CLIENT = both["console_side_server_only/client"] ~= nil
            mc.profile_reset()
            "#,
        );
        let order: String = lua.globals().get("ORDER").unwrap();
        assert_eq!(
            order,
            "console_side_both/client/can_eat,console_side_both/server/can_eat,console_side_server_only/server/mob_spawned",
            "rows sorted by mod, side, hook: {order}"
        );
        assert!(lua.globals().get::<bool>("HAS_SERVER").unwrap());
        assert!(lua.globals().get::<bool>("HAS_CLIENT").unwrap());
        assert!(lua.globals().get::<bool>("SERVER_ONLY").unwrap());
        assert!(
            !lua.globals().get::<bool>("SERVER_ONLY_CLIENT").unwrap(),
            "a server-only mod keeps the bare key and has no /client sibling"
        );
        assert!(
            crate::profiler::profile_report()
                .iter()
                .all(|r| !r.r#mod.starts_with("console_side_")),
            "reset clears the per-side rows"
        );
    }

    /// `mc.profile()` and `mc.profile_rows()` report the numbers of
    /// `profile_report()`, and times are fractional milliseconds: a handler
    /// under 1 ms is not shown as 0. The spin is real work of at least 12
    /// ms, so the floors below are independent of the code under test.
    #[test]
    fn profile_in_the_console_has_fractional_milliseconds() {
        let _lock = crate::profiler::test_lock();
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_millis(12) {
            std::hint::spin_loop();
        }
        crate::profiler::note(
            "console_fraction",
            "server",
            "mob_spawned",
            started.elapsed(),
            false,
        );
        crate::profiler::note(
            "console_fraction",
            "server",
            "mob_spawned",
            std::time::Duration::from_micros(400),
            true,
        );
        let lua = profile_in_lua(
            r#"
            for _, row in ipairs(mc.profile_rows()) do
              if row.mod == "console_fraction" then
                FLAT = {row.calls, row.total_ms, row.max_ms, row.overruns}
              end
            end
            local s = mc.profile()["console_fraction"]["mob_spawned"]
            NESTED = {s.calls, s.total_ms, s.max_ms, s.overruns}
            "#,
        );
        let read = |name: &str| -> Vec<f64> {
            let t: mlua::Table = lua.globals().get(name).unwrap();
            (1..=4).map(|i| t.get::<f64>(i).unwrap()).collect()
        };
        let flat = read("FLAT");
        assert_eq!(flat, read("NESTED"), "both forms report the same row");
        // Two calls, one of them stopped by the budget.
        assert_eq!(flat[0], 2.0, "{flat:?}");
        assert_eq!(flat[3], 1.0, "{flat:?}");
        // total_ms is the 12 ms spin plus the 0.4 ms call; max_ms is the spin.
        assert!(flat[1] >= 12.4, "total_ms is in milliseconds: {flat:?}");
        assert!(flat[2] >= 12.0, "max_ms is the slowest firing: {flat:?}");
        assert!(
            flat[2] < flat[1],
            "the second call adds to the total: {flat:?}"
        );
    }

    fn compound(snbt: &str) -> Compound {
        match parse_snbt(snbt).expect("valid SNBT").0 {
            Tag::Compound(c) => c,
            _ => panic!("{snbt} is not a compound"),
        }
    }

    /// Numbers stay numbers and longs stay exact (2^53 + 1 has no double);
    /// a byte is its number, not a flag; empty lists are arrays.
    #[test]
    fn saved_data_prints_as_json_with_its_numbers_exact() {
        let data = compound(
            "{count:9007199254740993L,flag:1b,ratio:0.5f,items:[I;1,2],\
             names:[\"a\"],empty:[],nested:{level:-7s},lists:[{x:3b}]}",
        );
        assert_eq!(
            compound_json(&data).unwrap(),
            serde_json::json!({
                "count": 9007199254740993_i64,
                "flag": 1,
                "ratio": 0.5,
                "items": [1, 2],
                "names": ["a"],
                "empty": [],
                "nested": {"level": -7},
                "lists": [{"x": 3}],
            })
        );
    }

    #[test]
    fn a_double_with_no_json_form_is_null() {
        let inf = Tag::Double(f64::INFINITY.to_bits());
        assert_eq!(tag_at(&inf, 1).unwrap(), serde_json::Value::Null);
        assert_eq!(
            tag_at(&Tag::Double(2.5f64.to_bits()), 1).unwrap(),
            serde_json::json!(2.5)
        );
    }

    /// The reply body: the world's table, and each online player's by uuid;
    /// nothing saved reads as empty objects.
    #[test]
    fn mod_state_json_is_the_world_then_each_player_by_uuid() {
        let uuid = "00000000-0000-0000-0000-000000000001";
        assert_eq!(
            mod_state_json(
                &compound("{counter:42}"),
                &[(uuid.to_owned(), compound("{coins:3b}"))]
            )
            .unwrap(),
            serde_json::json!({
                "world_data": {"counter": 42},
                "players": {uuid: {"coins": 3}},
            })
        );
        assert_eq!(
            mod_state_json(&Compound::default(), &[]).unwrap(),
            serde_json::json!({"world_data": {}, "players": {}})
        );
    }

    /// `mc.reload_mods(name)` for a mod with no status row is a Lua error
    /// naming it, not a queued request nobody drains.
    #[test]
    fn reload_mods_refuses_a_ghost() {
        let mut host = TestHost::default();
        let lua = vm().expect("a state");
        console::clear();
        lua_run(
            &mut host,
            &lua,
            "the console's test",
            r#"mc.reload_mods("ghost_ps7a")"#,
        );
        let text: Vec<String> = console::lines().into_iter().map(|(_, l)| l).collect();
        assert!(
            text.iter().any(|l| l.contains("unknown mod ghost_ps7a")),
            "ghost is refused: {text:?}"
        );
    }

    #[test]
    fn reload_mods_names_the_missing_side() {
        horizon_client::dev_input::mods::clear_all_for_test();
        horizon_client::dev_input::mods::set_status(horizon_client::dev_input::mods::ModStatus {
            id: "client_only".to_owned(),
            side: "client".to_owned(),
            ..Default::default()
        });
        let mut host = TestHost::default();
        let lua = vm().expect("a state");
        console::clear();
        lua_run(
            &mut host,
            &lua,
            "the console's test",
            r#"mc.reload_mods("client_only", "server")"#,
        );
        let text: Vec<String> = console::lines().into_iter().map(|(_, l)| l).collect();
        assert!(
            text.iter()
                .any(|l| l.contains("mod client_only has no server state")),
            "missing side is named: {text:?}"
        );
    }

    #[test]
    fn reload_mods_no_side_state_is_an_error() {
        horizon_client::dev_input::mods::clear_all_for_test();
        horizon_client::dev_input::mods::set_status(horizon_client::dev_input::mods::ModStatus {
            id: "dedicated_mod".to_owned(),
            side: "server".to_owned(),
            ..Default::default()
        });
        let mut host = TestHost::default();
        let lua = vm().expect("a state");
        console::clear();
        lua_run(
            &mut host,
            &lua,
            "the console's test",
            r#"mc.reload_mods(nil, "client")"#,
        );
        let text: Vec<String> = console::lines().into_iter().map(|(_, l)| l).collect();
        assert!(
            text.iter().any(|l| l.contains("no client state is loaded")),
            "no client state error: {text:?}"
        );
    }

    /// An id that names no loaded mod is refused before any read: the test
    /// host panics on every read it has not been given.
    #[test]
    fn mod_state_refuses_an_id_no_loaded_mod_has() {
        let mut host = TestHost::default();
        assert_eq!(
            mod_saved(&mut host, "no_such_mod_here").unwrap_err(),
            "unknown mod no_such_mod_here"
        );
    }

    /// A world nested past 64 containers is refused, not a stack overflow
    /// or a reply no reader parses; 64 is still fine.
    #[test]
    fn a_table_nested_past_64_levels_is_refused() {
        let mut deep = "{x:1}".to_owned();
        for _ in 0..99 {
            deep = format!("{{a:{deep}}}");
        }
        assert_eq!(
            mod_state_json(&compound(&deep), &[]).unwrap_err(),
            "world_data nests deeper than 64 levels"
        );
        let mut at_limit = "{x:1}".to_owned();
        for _ in 0..63 {
            at_limit = format!("{{a:{at_limit}}}");
        }
        assert!(mod_state_json(&compound(&at_limit), &[]).is_ok());
    }

    /// A host that holds saved data for the console's `mc.mod_state`: the
    /// world's attachment, one attachment per player, and the roster (one
    /// player in the Overworld, one in the Nether, both listed).
    struct SavedHost {
        world: horizon_client::mod_api::Attachments,
        players: Vec<(u128, horizon_client::mod_api::Attachments)>,
        roster: Vec<horizon_client::mod_api::NativeServerPlayer>,
    }

    impl Host for SavedHost {
        fn attachments(
            &mut self,
            holder: horizon_client::mod_api::Holder,
        ) -> horizon_client::mod_api::AttachmentResult<&mut horizon_client::mod_api::Attachments>
        {
            use horizon_client::mod_api::{AttachmentError, Holder};
            match holder {
                Holder::Level => Ok(&mut self.world),
                Holder::Player(uuid) => self
                    .players
                    .iter_mut()
                    .find(|(u, _)| *u == uuid)
                    .map(|(_, a)| a)
                    .ok_or(AttachmentError::MissingHolder),
                _ => Err(AttachmentError::MissingHolder),
            }
        }
        fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
            self.roster.clone()
        }
    }

    /// `data` saved under the mod `id`'s attachment `name`, as the Lua API
    /// stores it.
    fn saved(id: &str, name: &'static str, data: Compound) -> horizon_client::mod_api::Attachments {
        let ty = horizon_client::mod_api::AttachmentType {
            namespace: horizon_client::mobs::native_goal::intern(id),
            name,
            default: Compound::default,
            codec: Some(horizon_client::mod_api::AttachmentCodec {
                encode: |c| Ok(c.clone()),
                decode: |c| Ok(c.clone()),
            }),
            copy_on_death: false,
            sync: false,
        };
        let mut attachments = horizon_client::mod_api::Attachments::default();
        attachments.set(&ty, data).expect("the value is stored");
        attachments
    }

    const PROBE: &str = "console_probe_mod";
    const OWNER: &str = "00000000-0000-0000-0000-000000000001";
    const NETHER: &str = "00000000-0000-0000-0000-000000000002";

    fn saved_host() -> SavedHost {
        let mut world = Compound::new();
        world.insert(JavaString::from("counter"), Tag::Int(42));
        world.insert(JavaString::from("bytes"), Tag::ByteArray(vec![0xFF, 0x01]));
        world.insert(JavaString::from("big"), Tag::Long(9_007_199_254_740_993));
        world.insert(JavaString::from("nan"), Tag::Double(f64::NAN.to_bits()));
        world.insert(JavaString::from("tenth"), Tag::Float(0.1f32.to_bits()));
        let player = |dimension: &str, uuid: u128, name: &str| {
            horizon_client::test_events::native_server_player(
                uuid,
                name.to_owned(),
                4,
                dimension.to_owned(),
                [0.0; 3],
                0,
                20.0,
                0.0,
                0.0,
                uuid as i32,
                String::new(),
            )
        };
        SavedHost {
            world: saved(PROBE, "world", world),
            players: vec![
                (1, saved(PROBE, "player", compound("{coins:3b}"))),
                (2, horizon_client::mod_api::Attachments::default()),
            ],
            roster: vec![
                player("minecraft:overworld", 1, "Owner"),
                player("minecraft:the_nether", 2, "Guest"),
            ],
        }
    }

    fn console_lines() -> Vec<String> {
        console::lines().into_iter().map(|(_, l)| l).collect()
    }

    /// The Lua functions run in the console VM against populated attachments:
    /// the table a script reads, then the marked JSON line after `print` is
    /// gone (a previous `/lua` snippet may have rebound it).
    #[test]
    fn mod_state_runs_in_the_console_vm_against_saved_data() {
        horizon_client::dev_input::mods::set_status(horizon_client::dev_input::mods::ModStatus {
            id: PROBE.to_owned(),
            ..Default::default()
        });
        let mut host = saved_host();
        let lua = vm().expect("a state");
        console::clear();
        lua_run(
            &mut host,
            &lua,
            "the table",
            &format!(
                r#"
                local s = mc.mod_state("{PROBE}")
                print(s.world_data.counter, s.world_data.bytes[1], s.players["{OWNER}"].coins, next(s.players["{NETHER}"]) == nil)
                "#
            ),
        );
        assert!(
            console_lines().contains(&"42\t-1\t3\ttrue".to_owned()),
            "the table's values: {:?}",
            console_lines()
        );
        lua_run(&mut host, &lua, "silence print", "print = nil");
        console::clear();
        lua_run(
            &mut host,
            &lua,
            "emit",
            &format!(r#"mc.emit_mod_state("{PROBE}")"#),
        );
        let marked: Vec<String> = console::lines()
            .into_iter()
            .filter_map(|(k, l)| {
                (k == Kind::Out)
                    .then(|| l.strip_prefix("\u{1}MODSTATE ").map(str::to_owned))
                    .flatten()
            })
            .collect();
        assert_eq!(marked.len(), 1, "{:?}", console_lines());
        let got: serde_json::Value = serde_json::from_str(&marked[0]).expect("JSON");
        assert_eq!(
            got,
            serde_json::json!({
                "world_data": {
                    "counter": 42,
                    "bytes": [-1, 1],
                    "big": 9007199254740993_i64,
                    "nan": null,
                    "tenth": 0.10000000149011612_f64,
                },
                "players": {OWNER: {"coins": 3}, NETHER: {}},
            })
        );
    }
}
