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

/// Run `code` with `mc` bound to the host. Errors go to the window.
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
            "reload_mods",
            lua.create_function(|_, id: Option<String>| {
                horizon_client::dev_input::mods::request_reload(id.as_deref());
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
}
