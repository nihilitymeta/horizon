//! Lua scripts as mods (docs/lua-bindings.md). A content mod that carries
//! `scripts/init.lua` beside its `data/` and `assets/` is also a code mod: the
//! game runs the script in a Luau state of its own, and the script registers
//! functions for the hook points (`horizon.on("server_tick", fn)`), the same
//! events trait mods and mixin bundles hear. Nothing is compiled and nothing
//! patches the game, so two scripted mods never conflict in the build.
//!
//! The runtime is a [`Hooks`] implementation like a trait mod's, so the
//! generated dispatch calls it in order with the other mods. A script cannot
//! hold the host: a hook's `horizon.*` calls that act on the game are queued
//! and run by the runtime once the handler returns, in order, with the host
//! the hook was given.
//!
//! Scripts travel inside the `.hmod` under capability `scripts`, or load from
//! `--dev-mod` folders. Mob behaviour is written as goals ([`goals`]).
//!
//! With the developer tools on (docs/dev-tools.md), each mod's `horizon.log`,
//! `print` and errors also reach the Lua console's window, which lists the
//! mods ([`horizon_client::dev_input::mods::status`]) and reloads one on demand
//! ([`horizon_client::dev_input::mods::request_reload`]).
use horizon_client::dev_input::mods::{
    ModStatus, reload_asked, reload_generation, set_error, set_status,
};
use horizon_client::mod_api::HookSide;
use horizon_client::mod_api::ItemStack;
use horizon_client::mod_api::ModSide;
use horizon_client::mod_api::NativeCommand;
use horizon_client::mod_api::{
    AttachmentCodec, AttachmentType, BlockStateId, Compound, Flow, Holder, Hooks, Host,
    LevelDimension, LevelHost, LevelWriteHost, MobView,
};
use horizon_mods::{Capability, Content, Package};
use mlua::{Function, Lua, MultiValue, Table, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
#[path = "turn.rs"]
pub(crate) mod turn;

/// A line for the developer tools' console, when they are on.
fn to_console(error: bool, line: String) {
    if horizon_client::dev_input::enabled() {
        use horizon_client::dev_input::console::{Kind, push};
        push(if error { Kind::Err } else { Kind::Out }, line);
    }
}

pub mod goals;
pub mod json;
pub mod nbt;

/// Wall-clock time one handler or one goal function may take before Luau's
/// interrupt stops it with an error: the server thread runs them, so an
/// endless loop would otherwise freeze the game.
pub const HANDLER_BUDGET: Duration = Duration::from_millis(250);
/// Wall-clock time one handler of a hook that fires every frame (`hud`,
/// `gui_layer_post`) may take: a 250 ms stall would freeze rendering, so a
/// frame hook gets a slice of a frame instead (see `hud_hooks`). Scaled by
/// the handler count like `HANDLER_BUDGET`.
pub(crate) const FRAME_BUDGET: Duration = Duration::from_millis(8);
/// Consecutive frame-budget overruns after which a frame hook's handlers
/// are disabled (see `hud_hooks`): `/reload` registers them again.
pub(crate) const FRAME_OVERRUNS_UNTIL_DISABLED: u32 = 3;
/// The same for `init.lua` and the modules it requires, at load and reload.
const LOAD_BUDGET: Duration = Duration::from_secs(5);
/// Memory one mod's state may take.
const MEMORY: usize = 64 << 20;

/// The deadline of the chunk running in one mod's state, shared with the
/// state's interrupt: nanoseconds since the first budget was made, 0 for
/// none. A chunk runs inside [`Budget::run`].
#[derive(Clone, Debug, Default)]
pub struct Budget(Arc<AtomicU64>);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

impl Budget {
    /// Makes the state stop a chunk that runs past its deadline: the error
    /// is raised at every safepoint (a loop's back edge, a call, a return)
    /// from then on until the call ends, so a `pcall` around a slow loop
    /// catches one error and the next instruction raises another; nothing
    /// Lua does after the deadline runs for long. What runs after the
    /// chunk (restoring the host slot, taking the queue) is Rust's.
    fn install(&self, lua: &Lua) {
        let budget = self.clone();
        lua.set_interrupt(move |_| {
            let deadline = budget.0.load(Ordering::Relaxed);
            if deadline != 0 && epoch().elapsed().as_nanos() as u64 > deadline {
                Err(mlua::Error::runtime(
                    "ran over its time budget and was stopped (an endless loop?)",
                ))
            } else {
                Ok(mlua::VmState::Continue)
            }
        });
    }

    /// Runs `f`, a call into Lua, with `limit` as its budget.
    pub(crate) fn run<R>(&self, limit: Duration, f: impl FnOnce() -> R) -> R {
        let deadline = (epoch().elapsed() + limit).as_nanos() as u64;
        // A call nested in another (a hook fired by a summon answered at
        // once) gives the outer call its deadline back.
        let outer = self.0.swap(deadline.max(1), Ordering::Relaxed);
        let r = f();
        self.0.store(outer, Ordering::Relaxed);
        r
    }
}

/// The game a mod's Lua state runs in. A scripted mod has one state per side
/// it has an entry for: `scripts/server/init.lua` (the server's state) and
/// `scripts/client/init.lua` (the window's). Both read `scripts/shared/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LuaSide {
    Server,
    Client,
}

impl LuaSide {
    /// The side's name: `horizon.side` gives it, and it names the folder.
    pub fn as_str(self) -> &'static str {
        match self {
            LuaSide::Server => "server",
            LuaSide::Client => "client",
        }
    }

    /// The file the side's state starts from.
    pub fn entry(self) -> &'static str {
        match self {
            LuaSide::Server => horizon_mods::content::SERVER_ENTRY,
            LuaSide::Client => horizon_mods::content::CLIENT_ENTRY,
        }
    }

    /// Whether this side's state may register a handler for a hook on
    /// `hook`'s side: its own side's hooks, and the hooks of both.
    pub fn allows(self, hook: HookSide) -> bool {
        matches!(
            (self, hook),
            (LuaSide::Server, HookSide::Server | HookSide::Both)
                | (LuaSide::Client, HookSide::Client | HookSide::Both)
        )
    }
}

/// The folder every side's state reads its modules from, beside the side's own.
const SHARED_DIR: &str = "scripts/shared";

/// The content mod's manifest, which names the mod.
const MANIFEST: &str = "horizon-mod.toml";
/// The Lua global the game provides.
const GLOBAL: &str = "horizon";
/// The `horizon` names a client state keeps: everything else on the
/// table is server-only and raises in a client state.
const CLIENT_API: &[&str] = &[
    "mod_id",
    "mod_version",
    "side",
    "libraries",
    "log",
    "on",
    "after",
    "every",
    "ticks",
    "config",
    "save_config",
    "dev",
    "distance",
    "handled",
    "cancel",
    "require",
    "net_send_to_server",
];
/// The server-only `horizon` names a client state's draw-hook handler
/// (`hud`, `gui_layer_post`) may still call: the reads that answer through
/// the shared host (`Reader::Shared`, `&self` only), and the two queued
/// chat actions (`tell`, `broadcast`) that run after the frame. The
/// saved-table reads and `summon` are listed only so the call reaches
/// their draw-hook refusal (they are never served in a draw hook); every
/// other server-only name (queued actions like `set_block`, declarations
/// like `command`, `goal`, `block_entity`) raises the server-only error
/// even inside a draw hook.
const DRAW_API: &[&str] = &[
    "block",
    "block_property",
    "light",
    "surface_height",
    "time",
    "raycast",
    "player",
    "players",
    "acting_player",
    "mob",
    "mobs",
    "inventory",
    "count_items",
    "carried_items",
    "data",
    "world_data",
    "player_data",
    "entity_data",
    "summon",
    "tell",
    "broadcast",
];
/// The registry key of the handlers table: `name -> { fn, fn, ... }`.
const HANDLERS: &str = "horizon_handlers";
/// The registry key of the handlers that watch one kind of event
/// (`horizon.on(name, fn, kind)`): `name -> kind -> { fn, fn, ... }`.
const KIND_HANDLERS: &str = "horizon_kind_handlers";
/// The registry key of the queue of actions a handler asked for.
const QUEUE: &str = "horizon_queue";
/// The registry key of the function that runs a hook's handlers, as one
/// call into the state: the state is shared by the threads that fire hooks
/// (the server's, the render thread's own mobs), and a call is the unit
/// they take turns at, so no handler sees another thread's actions. The
/// runner sets the host slot; `run_handlers` restores it and takes the
/// queue once the call is back, whether the runner finished or was stopped
/// by the budget mid-way.
const RUNNER: &str = "horizon_runner";
const RUNNER_CODE: &str = r#"
return function(slot, base)
  -- The globals the runner calls, taken from `base` (BASE_CODE, built before
  -- any script ran): a script that later assigns `error`, `xpcall`, `debug`
  -- or any other global cannot change how a failure is caught or reported.
  local xpcall, error = base.xpcall, base.error
  local type, ipairs, traceback, text = base.type, base.ipairs, base.traceback, base.text
  return function(handlers, host, full, ...)
    -- A hook fired from inside a handler (a summon answered at once tells of
    -- the spawn) runs nested: the outer call's host is kept for it. slot[3]
    -- counts the runners in progress: an action outside any is refused.
    local outer_host, outer_full, depth = slot[1], slot[2], slot[3] or 0
    slot[1] = host
    slot[2] = full
    slot[3] = depth + 1
    local failed
    -- A string error gains the handler's traceback here, where its frames
    -- still run; anything else a script throws (a table, a number) gains it
    -- too, with its type named. A Rust error travels as userdata, which
    -- `tostring` would flatten, so it passes through for the host to report.
    local function with_trace(e)
      local t = type(e)
      if t == "string" then
        return e .. "\nstack traceback:" .. traceback("", 2)
      elseif t ~= "userdata" then
        return "(" .. t .. ") " .. text(e) .. "\nstack traceback:" .. traceback("", 2)
      end
      return e
    end
    for _, f in ipairs(handlers) do
      local ok, e = xpcall(f, with_trace, ...)
      if not ok then failed = e break end
    end
    slot[1] = outer_host
    slot[2] = outer_full
    slot[3] = depth
    if failed == nil then return end
    -- A message is returned, not raised: mlua would run a raised error's text
    -- through `__tostring`, which a script may replace. Rust raises it.
    if type(failed) == "string" then return failed end
    error(failed, 0)
  end
end
"#;
/// The registry key of the table of the globals the runtime's chunks use
/// (`BASE_CODE`): built once, before any script runs, and read by the
/// runner, the reads and the timers, so none of them looks a global up while
/// a script may have replaced it. Scripts never see it.
const BASE: &str = "horizon_base";
const BASE_CODE: &str = r#"
local xpcall, error, pcall = xpcall, error, pcall
local type, tostring, ipairs = type, tostring, ipairs
local traceback = debug.traceback
local max, floor, remove = math.max, math.floor, table.remove
-- A value's text for an error message, never failing: a `__tostring` that
-- throws, or returns no string, gives a placeholder instead.
local function text(e)
  -- A string is its own text: `tostring` would run it through a `__tostring`
  -- a script may have replaced on the string metatable.
  if type(e) == "string" then return e end
  local ok, s = pcall(tostring, e)
  if ok and type(s) == "string" then return s end
  return "<unprintable " .. type(e) .. ">"
end
return {
  xpcall = xpcall, error = error, pcall = pcall, type = type, ipairs = ipairs,
  traceback = traceback, text = text, max = max, floor = floor, remove = remove,
}
"#;
/// The registry key of the table holding the call in progress: `[1]` its
/// read function (`horizon.block` and the other reads go through it; nil in
/// a hook without a host), `[2]` whether the host is a full one, `[3]` how
/// many runners are in progress (0: no hook's handlers run, and an action
/// has nowhere to go).
const HOST_SLOT: &str = "horizon_host";
/// The registry key of the timers' own tick handler, so a listing
/// of the handlers can leave it out.
pub(crate) const TIMERS_HANDLER: &str = "horizon_timers";
/// `horizon.block`, `block_property`, `mob`, `mobs`, `players`, `player`,
/// `time` and `raycast`: wrappers over the call's read function, which the
/// runner keeps in the slot while the handlers run; outside a call there is
/// none, and a read is an error.
const READS_CODE: &str = r#"
return function(slot, horizon, base)
  local error, pcall, text = base.error, base.pcall, base.text
  local function read(what, ...)
    local f = slot[1]
    if f == nil then
      -- With no host there is no acting player either: `acting_player`
      -- answers nil where every other read is an error.
      if what == "acting_player" then return nil end
      error("horizon." .. what .. ": reads work inside a hook with a host (not in a hostless hook or a goal)", 3)
    end
    return f(what, ...)
  end
  horizon.block = function(x, y, z) return read("block", x, y, z) end
  horizon.block_property = function(x, y, z, key) return read("block_property", x, y, z, key) end
  horizon.mob = function(id) return read("mob", id) end
  horizon.mobs = function() return read("mobs") end
  horizon.players = function() return read("players") end
  horizon.player = function(who) return read("player", who) end
  horizon.time = function() return read("time") end
  horizon.raycast = function(who, distance) return read("raycast", who, distance) end
  horizon.world_data = function() return read("world_data") end
  horizon.player_data = function(who) return read("player_data", who) end
  horizon.entity_data = function(id) return read("entity_data", id) end
  horizon.acting_player = function() return read("acting_player") end
  horizon.light = function(x, y, z) return read("light", x, y, z) end
  horizon.surface_height = function(x, z, heightmap) return read("surface_height", x, z, heightmap) end
  horizon.inventory = function(who) return read("inventory", who) end
  horizon.count_items = function(who, item) return read("count_items", who, item) end
  horizon.carried_items = function(who) return read("carried_items", who) end
  horizon.data = function(path) return read("data", path) end
  horizon.menu_contents = function(who) return read("menu_contents", who) end
  horizon.menu_cursor = function(who) return read("menu_cursor", who) end
  horizon.dev = { inspect = function(who, what) return read("dev_inspect", who, what) end }
  -- A refused registration (a hook name the runtime does not know) is
  -- raised from the script's own line, as any Lua error is.
  local on = horizon.on
  horizon.on = function(name, f, kind)
    local ok, e = pcall(on, name, f, kind)
    if not ok then error(text(e), 2) end
  end
  -- A summon with a host at hand is made at once and answers the mob's id;
  -- in a level hook it waits for the level's pass (the queued action).
  local queued_summon = horizon.summon
  horizon.summon = function(...)
    if slot[1] ~= nil and slot[2] == true then return read("summon", ...) end
    return queued_summon(...)
  end
end
"#;
/// `horizon.after(ticks, fn)` and `horizon.every(ticks, fn)`: timers run from
/// the state's own tick (the end of a server tick while the world runs, or
/// each client tick), as a handler of their own; a timer that fails is reported to the log (`report`) and,
/// if repeating, kept, and the timers after it run: the handler itself does
/// not fail, so what the other timers asked for and saved stands.
const TIMERS_CODE: &str = r#"
return function(horizon, report, base)
  -- The globals the timers use, from `base` (BASE_CODE), as the runner's are.
  local xpcall, type, text, traceback = base.xpcall, base.type, base.text, base.traceback
  local max, floor, remove = base.max, base.floor, base.remove
  local error = base.error
  local timers, tick = {}, 0
  -- A delay is a finite number of ticks: `after` takes zero or more, `every`
  -- one or more. Anything else is a Lua error at the call, so the mod's
  -- scripts fail to load; a NaN or infinite delay would hold a timer that
  -- never fires, until the next reload.
  local function delay(name, ticks, least)
    if type(ticks) ~= "number" or ticks ~= ticks or ticks == 1 / 0 or ticks == -1 / 0 or ticks < least then
      error("horizon." .. name .. ": the delay must be a finite number of ticks, at least " .. least, 3)
    end
    return ticks
  end
  horizon.after = function(ticks, fn)
    timers[#timers + 1] = { due = tick + max(1, floor(delay("after", ticks, 0))), fn = fn }
  end
  horizon.every = function(ticks, fn)
    local n = max(1, floor(delay("every", ticks, 1)))
    timers[#timers + 1] = { due = tick + n, every = n, fn = fn }
  end
  horizon.ticks = function() return tick end
  local function run_timers(event)
    -- A server tick counts only its end phase in a running world;
    -- a client tick has no phase to gate on, and counts outright.
    if event.phase ~= nil and (event.phase ~= "end" or not event.world_running) then return end
    tick = tick + 1
    local i = 1
    while i <= #timers do
      local t = timers[i]
      if t.due <= tick then
        if t.every then t.due = tick + t.every; i = i + 1 else remove(timers, i) end
        -- A string error gains the timer's traceback here, where its
        -- frames still run; anything else a script throws gains it too,
        -- with its type named, as the runner does.
        local function with_trace(e)
          local t = type(e)
          if t == "string" then
            return e .. "\nstack traceback:" .. traceback("", 2)
          elseif t ~= "userdata" then
            return "(" .. t .. ") " .. text(e) .. "\nstack traceback:" .. traceback("", 2)
          end
          return e
        end
        local ok, e = xpcall(t.fn, with_trace)
        if not ok then report(text(e)) end
      else
        i = i + 1
      end
    end
  end
  return run_timers
end
"#;

/// What a hook's handlers may read from while they run: the hook's host.
/// It reaches Lua as a function of the call's scope (`Lua::scope`), gone
/// when the call returns, whatever kept it.
pub(crate) enum Reader<'a> {
    /// A hook without a host (a `none`-host generated arm): reads fail.
    None,
    Host(&'a mut dyn Host),
    /// A draw hook (`hud`, `gui_layer_post`): the hook's `&dyn Host`,
    /// shared with the draw closures. Every `&self` read works through it;
    /// reads that need `&mut` (saved data, the acting player's stacks, a
    /// direct summon) fail with a draw-hook error.
    Shared(&'a dyn Host),
    Level(&'a dyn LevelHost),
}

impl Reader<'_> {
    /// Whether this reader serves a draw hook (`hud`, `gui_layer_post`).
    fn is_draw(&self) -> bool {
        matches!(self, Reader::Shared(_))
    }

    /// The hook's host for a `&self` read, when the reader has one.
    fn host(&self) -> Option<&dyn Host> {
        match self {
            Reader::None | Reader::Level(_) => None,
            Reader::Host(h) => Some(&**h),
            Reader::Shared(s) => Some(*s),
        }
    }
}

/// A saved table a call handed out (`horizon.world_data`,
/// `horizon.player_data`, `horizon.entity_data`): written back when the call
/// ends.
struct Fetched {
    holder: Holder,
    before: Compound,
    table: Table,
}

/// A UUID as written (`8-4-4-4-12`, or 32 hex digits), back.
fn uuid_parse(text: &str) -> Option<u128> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32)
        .then(|| u128::from_str_radix(&hex, 16).ok())
        .flatten()
}

/// A server player as a table: `uuid`, `name`, `dimension`, `position`
/// (`{x, y, z}`), `yaw`, `pitch`, `game_mode`, `health`.
fn player_table_of(
    lua: &Lua,
    p: &horizon_client::mod_api::NativeServerPlayer,
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("uuid", uuid_text(p.uuid))?;
    t.set("name", p.name.as_str())?;
    t.set("dimension", p.dimension.as_str())?;
    t.set("position", goals::vec3(lua, p.position)?)?;
    t.set("yaw", p.yaw)?;
    t.set("pitch", p.pitch)?;
    t.set("game_mode", p.game_mode)?;
    t.set("health", p.health)?;
    Ok(t)
}

/// A server player as a table with their held stack: `player_table_of` plus
/// `held`, the main-hand stack as an `Item` table (nil for an empty hand or
/// a player the host has no stack for, as on a client window). Only the
/// `horizon.player(s)` reads and `horizon.acting_player()` set it: the
/// generated `player` seeds stay bare, so unheard player hooks cost no
/// stack read. `held_stack` takes `&self`, so this works in draw hooks too.
fn player_table_with_held(
    lua: &Lua,
    h: &dyn Host,
    p: &horizon_client::mod_api::NativeServerPlayer,
) -> mlua::Result<Table> {
    let t = player_table_of(lua, p)?;
    t.set(
        "held",
        match h.held_stack(p.uuid) {
            Some(stack) => Value::Table(item_table_of(lua, &stack)?),
            None => Value::Nil,
        },
    )?;
    Ok(t)
}

/// The player a script named: a `player` table (its `uuid`), a UUID or a
/// name of a player of the world.
fn uuid_of(h: &dyn Host, who: &Value) -> Option<u128> {
    let text = match who {
        Value::Table(t) => t.get::<Option<String>>("uuid").ok().flatten()?,
        Value::String(s) => s.to_str().ok()?.to_owned(),
        _ => return None,
    };
    uuid_parse(&text).or_else(|| {
        h.level_players()
            .into_iter()
            .find(|p| p.name == text)
            .map(|p| p.uuid)
    })
}

/// A block state as a command names it: `id[key=value,...]` from a block id
/// (`minecraft:` when it has no namespace) and a table of properties.
fn state_name(id: &str, props: Option<&Table>) -> mlua::Result<String> {
    let mut name = qualify(id);
    if let Some(props) = props {
        let mut pairs = Vec::new();
        for pair in props.pairs::<String, Value>() {
            let (k, v) = pair?;
            let v = match v {
                Value::String(s) => s.to_str()?.to_owned(),
                Value::Integer(i) => i.to_string(),
                Value::Boolean(b) => b.to_string(),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "{k}: a property is a string, a number or a boolean, not a {}",
                        other.type_name()
                    )));
                }
            };
            pairs.push(format!("{k}={v}"));
        }
        if !pairs.is_empty() {
            pairs.sort();
            name.push('[');
            name.push_str(&pairs.join(","));
            name.push(']');
        }
    }
    Ok(name)
}

/// Every plain item in `stacks` with how many, by id, as `carried_items`.
fn carried_in(stacks: &[Option<ItemStack>]) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for s in stacks.iter().flatten() {
        if s.damage != 0 || s.extra.is_some() {
            continue;
        }
        match out.iter_mut().find(|(i, _)| *i == *s.item) {
            Some((_, n)) => *n += s.count,
            None => out.push((s.item.to_string(), s.count)),
        }
    }
    out
}

/// An item stack as a table: `id`, `count`, `damage`, the `extra`
/// components compound when the stack has any (nil when plain), and `plain`
/// (no components beyond count and damage) for the older readers.
pub(crate) fn item_table_of(lua: &Lua, stack: &ItemStack) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", &*stack.item)?;
    t.set("count", stack.count)?;
    t.set("damage", stack.damage)?;
    t.set("extra", crate::generated_runtime::extra_seed(lua, stack)?)?;
    t.set("plain", stack.extra.is_none())?;
    Ok(t)
}

impl LuaMod {
    /// One read (`what`, its arguments) answered from `reader`; a saved table
    /// handed out joins `fetched` for the write-back after the call.
    fn read(
        &self,
        lua: &Lua,
        reader: &mut Reader<'_>,
        fetched: &std::cell::RefCell<Vec<Fetched>>,
        what: &str,
        rest: MultiValue,
    ) -> mlua::Result<Value> {
        use mlua::FromLuaMulti;
        let level_only = || {
            mlua::Error::runtime(format!(
                "horizon.{what}: a level hook (random_tick, tick_block) reads blocks only"
            ))
        };
        let no_host = || mlua::Error::runtime(format!("horizon.{what}: no host"));
        // A read that needs `&mut` from a draw hook (`hud`,
        // `gui_layer_post`): names the helper, as an ordinary handler
        // error (it resets the frame-overrun count, like any error).
        let draw_unavailable = || {
            mlua::Error::runtime(format!(
                "horizon.{what}: not available in draw hooks (hud, gui_layer_post)"
            ))
        };
        Ok(match what {
            "block" => {
                let (x, y, z): (i32, i32, i32) = FromLuaMulti::from_lua_multi(rest, lua)?;
                let id = match reader {
                    Reader::Host(h) => h.block([x, y, z]),
                    Reader::Shared(s) => s.block([x, y, z]),
                    Reader::Level(l) => l
                        .state_block(l.block_state([x, y, z]))
                        .unwrap_or_else(|| "minecraft:air".to_owned()),
                    Reader::None => return Err(no_host()),
                };
                Value::String(lua.create_string(id)?)
            }
            "block_property" => {
                let (x, y, z, key): (i32, i32, i32, String) =
                    FromLuaMulti::from_lua_multi(rest, lua)?;
                let value = match reader {
                    Reader::Host(h) => h.block_property([x, y, z], &key),
                    Reader::Shared(s) => s.block_property([x, y, z], &key),
                    Reader::Level(l) => l
                        .state_properties(l.block_state([x, y, z]))
                        .into_iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v),
                    Reader::None => return Err(no_host()),
                };
                match value {
                    Some(v) => Value::String(lua.create_string(v)?),
                    None => Value::Nil,
                }
            }
            "world_data" | "player_data" | "entity_data" => {
                // Saved tables are written back through `&mut`: a draw hook
                // cannot lend it.
                if reader.is_draw() {
                    return Err(draw_unavailable());
                }
                let Reader::Host(h) = reader else {
                    return Err(level_only());
                };
                let (holder, attachment) = if what == "world_data" {
                    (Holder::Level, &self.world_attachment)
                } else if what == "player_data" {
                    let who: String = FromLuaMulti::from_lua_multi(rest, lua)?;
                    let uuid = uuid_parse(&who).or_else(|| {
                        h.level_players()
                            .into_iter()
                            .find(|p| p.name == who)
                            .map(|p| p.uuid)
                    });
                    let Some(uuid) = uuid else {
                        return Ok(Value::Nil);
                    };
                    (Holder::Player(uuid), &self.player_attachment)
                } else {
                    let id: u32 = FromLuaMulti::from_lua_multi(rest, lua)?;
                    (Holder::Entity(id), &self.entity_attachment)
                };
                // The same table twice in one call, so changes are not lost.
                if let Some(f) = fetched.borrow().iter().find(|f| f.holder == holder) {
                    return Ok(Value::Table(f.table.clone()));
                }
                let before = match attachment.get_existing(&mut **h, holder) {
                    Ok(Some(c)) => c,
                    Ok(None) => Compound::default(),
                    Err(_) => return Ok(Value::Nil),
                };
                let table = nbt::compound_to_table(lua, &before)?;
                fetched.borrow_mut().push(Fetched {
                    holder,
                    before,
                    table: table.clone(),
                });
                Value::Table(table)
            }
            "acting_player" => {
                // Nil where no acting player is in place: a hook with a
                // host but no player swapped in (a death, the
                // dedicated-server console hook, a player command before
                // it runs), a draw hook on a client window, or a level
                // hook. A single-player owner's console command runs as
                // that owner, through `perform_command`. A hostless hook
                // never reaches this arm: its read answers nil up front.
                let Some(h) = reader.host() else {
                    return Ok(Value::Nil);
                };
                match h.acting_player() {
                    Some(p) => Value::Table(player_table_with_held(lua, h, &p)?),
                    None => Value::Nil,
                }
            }
            _ => {
                // A draw hook reads through the shared host: every `&self`
                // read below works through it. The acting player's stacks
                // and a direct summon need `&mut` and fail with a
                // draw-hook error in their own arms.
                let Some(h) = reader.host() else {
                    return Err(level_only());
                };
                match what {
                    "mob" => {
                        let id: u32 = FromLuaMulti::from_lua_multi(rest, lua)?;
                        match h.mob_view(h.dimension(), id) {
                            Some(view) => Value::Table(goals::view_table(lua, &view)?),
                            None => Value::Nil,
                        }
                    }
                    "mobs" => {
                        let out = lua.create_table()?;
                        for view in h.mob_views(h.dimension()) {
                            out.push(goals::view_table(lua, &view)?)?;
                        }
                        Value::Table(out)
                    }
                    // The inspect stick: `who` used it on an entity (its
                    // id) or a block (`{x, y, z, face}`). Only a player who
                    // may use game-master items (creative, permission 2 or
                    // more, as vanilla's debug stick) inspects; the game
                    // opens the window at the end of the tick. Whether the
                    // click was the stick's, which the caller then cancels.
                    "dev_inspect" => {
                        let (who, what): (Value, Value) = FromLuaMulti::from_lua_multi(rest, lua)?;
                        // The acting player first: during a guest's turn the
                        // other players are out of reach, and its permission
                        // is the guest's own.
                        let player = uuid_of(h, &who).and_then(|u| {
                            h.acting_player()
                                .filter(|p| p.uuid == u)
                                .or_else(|| h.level_players().into_iter().find(|p| p.uuid == u))
                        });
                        let Some(p) = player.filter(|p| p.game_mode == 1 && p.permission >= 2)
                        else {
                            return Ok(Value::Boolean(false));
                        };
                        let int = |v: Value| -> mlua::Result<i32> {
                            match v {
                                Value::Integer(i) => i32::try_from(i).map_err(mlua::Error::runtime),
                                Value::Number(n) if n.fract() == 0.0 => Ok(n as i32),
                                other => Err(mlua::Error::runtime(format!(
                                    "horizon.dev.inspect: a whole number, not {}",
                                    other.type_name()
                                ))),
                            }
                        };
                        use horizon_client::mobs::observe::{Request, StickTarget, request};
                        let target = match what {
                            Value::Table(t) => {
                                let face = match t.get::<Value>("face")? {
                                    Value::Table(f) => {
                                        [int(f.get("x")?)?, int(f.get("y")?)?, int(f.get("z")?)?]
                                    }
                                    _ => [0, 1, 0],
                                };
                                StickTarget::Block {
                                    dimension: t
                                        .get::<Option<String>>("dimension")?
                                        .unwrap_or_else(|| p.dimension.clone()),
                                    pos: [int(t.get("x")?)?, int(t.get("y")?)?, int(t.get("z")?)?],
                                    face,
                                }
                            }
                            other => StickTarget::Entity(
                                u32::try_from(int(other)?).map_err(mlua::Error::runtime)?,
                            ),
                        };
                        request(Request::Stick {
                            player: p.uuid,
                            target,
                        });
                        Value::Boolean(true)
                    }
                    "players" => {
                        let out = lua.create_table()?;
                        for p in h.level_players() {
                            out.push(player_table_with_held(lua, h, &p)?)?;
                        }
                        Value::Table(out)
                    }
                    "player" => {
                        let who: String = FromLuaMulti::from_lua_multi(rest, lua)?;
                        let uuid = uuid_parse(&who);
                        match h
                            .level_players()
                            .into_iter()
                            .find(|p| p.name == who || Some(p.uuid) == uuid)
                        {
                            Some(p) => Value::Table(player_table_with_held(lua, h, &p)?),
                            None => Value::Nil,
                        }
                    }
                    "time" => {
                        let (rain, thunder) = h.weather();
                        let day_time = h.day_time();
                        let t = lua.create_table()?;
                        t.set("dimension", h.dimension())?;
                        t.set("day_time", day_time)?;
                        t.set("day", day_time.div_euclid(24000))?;
                        t.set("time_of_day", day_time.rem_euclid(24000))?;
                        t.set("rain", rain)?;
                        t.set("thunder", thunder)?;
                        Value::Table(t)
                    }
                    "light" => {
                        let (x, y, z): (i32, i32, i32) = FromLuaMulti::from_lua_multi(rest, lua)?;
                        let (block, sky) = h.light([x, y, z]);
                        let t = lua.create_table()?;
                        t.set("block", block)?;
                        t.set("sky", sky)?;
                        Value::Table(t)
                    }
                    "surface_height" => {
                        use horizon_client::mod_api::NativeHeightmap;
                        let (x, z, heightmap): (i32, i32, Option<String>) =
                            FromLuaMulti::from_lua_multi(rest, lua)?;
                        let heightmap = match heightmap.as_deref() {
                            None | Some("world_surface") => NativeHeightmap::WorldSurface,
                            Some("motion_blocking") => NativeHeightmap::MotionBlocking,
                            Some("motion_blocking_no_leaves") => {
                                NativeHeightmap::MotionBlockingNoLeaves
                            }
                            Some("ocean_floor") => NativeHeightmap::OceanFloor,
                            Some(other) => {
                                return Err(mlua::Error::runtime(format!(
                                    "horizon.surface_height: no heightmap {other:?} (world_surface, motion_blocking, motion_blocking_no_leaves, ocean_floor)"
                                )));
                            }
                        };
                        match h.surface_heights(h.dimension(), &[(x, z)], heightmap) {
                            Ok(heights) => match heights.first().copied().flatten() {
                                Some(y) => Value::Integer(i64::from(y)),
                                None => Value::Nil,
                            },
                            Err(e) => {
                                log::debug!("[lua:{}] surface_height: {e}", self.id);
                                Value::Nil
                            }
                        }
                    }
                    "inventory" | "count_items" | "carried_items" => {
                        let (who, item): (Value, Option<String>) =
                            FromLuaMulti::from_lua_multi(rest, lua)?;
                        let Some(uuid) = uuid_of(h, &who) else {
                            return Ok(Value::Nil);
                        };
                        // The acting player's stacks are in place (the host's
                        // own slots); another player's are read from the server's
                        // records.
                        let acting = h.acting_player().is_some_and(|p| p.uuid == uuid);
                        // The acting player's stacks live in the host's own
                        // slots: reading them needs `&mut`, which a draw
                        // hook cannot lend. Another player's stacks come
                        // from the server's records and work shared.
                        if acting && reader.is_draw() {
                            return Err(draw_unavailable());
                        }
                        // Another player's stacks read through the shared
                        // host (`player_stacks` takes `&self`); only the
                        // acting player's stacks need the `&mut` host,
                        // which a draw hook cannot lend (refused above).
                        let stacks: Vec<Option<ItemStack>> = if acting {
                            let Reader::Host(host) = reader else {
                                return Err(draw_unavailable());
                            };
                            (0..36)
                                .map(|slot| host.carried_stack(slot).cloned())
                                .collect()
                        } else {
                            match h.player_stacks(uuid) {
                                Some(stacks) => stacks,
                                None => return Ok(Value::Nil),
                            }
                        };
                        match what {
                            "inventory" => {
                                let out = lua.create_table()?;
                                for (i, s) in stacks.iter().enumerate() {
                                    if let Some(s) = s {
                                        out.set(i + 1, item_table_of(lua, s)?)?;
                                    }
                                }
                                Value::Table(out)
                            }
                            "count_items" => {
                                let item = qualify(&item.unwrap_or_default());
                                let n: u32 = carried_in(&stacks)
                                    .into_iter()
                                    .filter(|(i, _)| *i == item)
                                    .map(|(_, n)| n)
                                    .sum();
                                Value::Integer(i64::from(n))
                            }
                            _ => {
                                let out = lua.create_table()?;
                                for (id, n) in carried_in(&stacks) {
                                    out.set(id, n)?;
                                }
                                Value::Table(out)
                            }
                        }
                    }
                    "data" => {
                        let path: String = FromLuaMulti::from_lua_multi(rest, lua)?;
                        match h.data(&path) {
                            Some(bytes) => {
                                match serde_json::from_slice::<serde_json::Value>(&bytes) {
                                    Ok(json) => json::json_to_lua(lua, &json)?,
                                    Err(_) => Value::String(lua.create_string(bytes)?),
                                }
                            }
                            None => Value::Nil,
                        }
                    }
                    "summon" => {
                        let (dimension, kind, x, y, z): (String, String, f64, f64, f64) =
                            FromLuaMulti::from_lua_multi(rest, lua)?;
                        // A direct summon writes: a draw hook names the
                        // helper and fails instead of queueing one.
                        let Reader::Host(h) = reader else {
                            return Err(draw_unavailable());
                        };
                        match h.summon(&dimension, &qualify(&kind), [x, y, z], Default::default()) {
                            Some(id) => Value::Integer(i64::from(id)),
                            None => Value::Nil,
                        }
                    }
                    "raycast" => {
                        let (who, distance): (String, Option<f32>) =
                            FromLuaMulti::from_lua_multi(rest, lua)?;
                        let uuid = uuid_parse(&who).or_else(|| {
                            h.level_players()
                                .into_iter()
                                .find(|p| p.name == who)
                                .map(|p| p.uuid)
                        });
                        let hit =
                            uuid.and_then(|uuid| h.player_raycast(uuid, distance.unwrap_or(64.0)));
                        match hit {
                            Some(hit) => {
                                let t = lua.create_table()?;
                                t.set("id", hit.block.as_str())?;
                                t.set("x", hit.x)?;
                                t.set("y", hit.y)?;
                                t.set("z", hit.z)?;
                                let block = lua.create_table()?;
                                block.set("x", hit.block_x)?;
                                block.set("y", hit.block_y)?;
                                block.set("z", hit.block_z)?;
                                t.set("block", block)?;
                                Value::Table(t)
                            }
                            None => Value::Nil,
                        }
                    }
                    "menu_contents" => {
                        let who: Value = FromLuaMulti::from_lua_multi(rest, lua)?;
                        let Some(uuid) = uuid_of(h, &who) else {
                            return Ok(Value::Nil);
                        };
                        match h.menu_contents(uuid) {
                            Some(stacks) => {
                                Value::Table(crate::generated_runtime::struct_vec_to_lua::<
                                    Option<ItemStack>,
                                >(lua, &stacks)?)
                            }
                            None => Value::Nil,
                        }
                    }
                    "menu_cursor" => {
                        let who: Value = FromLuaMulti::from_lua_multi(rest, lua)?;
                        let Some(uuid) = uuid_of(h, &who) else {
                            return Ok(Value::Nil);
                        };
                        match h.menu_cursor(uuid) {
                            Some(stack) => Value::Table(item_table_of(lua, &stack)?),
                            None => Value::Nil,
                        }
                    }
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "horizon.{other}: no such read"
                        )));
                    }
                }
            }
        })
    }

    /// Writes a saved table a call handed out back to its holder, when it
    /// changed.
    fn store_fetched(&self, host: &mut dyn Host, hook: &str, f: &Fetched) {
        let attachment = match f.holder {
            Holder::Level => &self.world_attachment,
            Holder::Entity(_) => &self.entity_attachment,
            _ => &self.player_attachment,
        };
        match nbt::table_to_compound(&f.table, Some(&f.before)) {
            Ok(after) if after != f.before => {
                if let Err(e) = attachment.set(host, f.holder, after) {
                    log::debug!(
                        "[lua:{}] {hook}: {:?} data not kept: {e:?}",
                        self.id,
                        f.holder
                    );
                }
            }
            Ok(_) => {}
            Err(e) => {
                self.hook_error(hook, e);
            }
        }
    }
}

/// Installs the read wrappers (`READS_CODE`) on the `horizon` table.
fn install_reads(lua: &Lua, api: &Table) -> mlua::Result<()> {
    let slot: Table = lua.named_registry_value(HOST_SLOT)?;
    let base: Table = lua.named_registry_value(BASE)?;
    let install: Function = lua.load(READS_CODE).set_name("horizon").eval()?;
    install.call::<()>((slot, api, base))
}

/// Installs the timers (`TIMERS_CODE`) on the `horizon` table: their handler
/// is the first handler of the state's own tick hook.
/// A usage's words before its arguments (`parasite count` of `parasite count
/// [kind:word]`), joined by single spaces: the command's name.
fn command_words(usage: &str) -> String {
    usage
        .split_whitespace()
        .take_while(|p| !p.starts_with('<') && !p.starts_with('['))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fills `args` from `line` by `usage` (`pay <who:player> <amount:integer>
/// [note:text]`): the words after the usage's leading words go to its
/// arguments in order, `integer` and `number` as numbers, `boolean` as one,
/// `text` as the rest of the line, the other kinds as the word. A required
/// argument (`<...>`) missing, or an integer or number that is not one,
/// refuses the line: the message for the player (translated: the
/// `horizon.lua.command.*` keys), and the handler is not called.
fn command_args(
    lua: &Lua,
    usage: &str,
    line: &str,
    args: &Table,
) -> mlua::Result<Option<horizon_client::mod_api::Text>> {
    use horizon_client::mod_api::Text;
    let usage_line = || Text::tr("horizon.lua.command.usage", &[&usage]);
    let params: Vec<(&str, &str)> = usage
        .split_whitespace()
        .filter_map(|p| {
            let inner = p
                .strip_prefix('<')
                .and_then(|p| p.strip_suffix('>'))
                .or_else(|| p.strip_prefix('[').and_then(|p| p.strip_suffix(']')))?;
            Some(inner.split_once(':').unwrap_or((inner, "word")))
        })
        .collect();
    let literals = usage
        .split_whitespace()
        .take_while(|p| !p.starts_with('<') && !p.starts_with('['))
        .count();
    let required: Vec<&str> = usage
        .split_whitespace()
        .filter_map(|p| p.strip_prefix('<').and_then(|p| p.strip_suffix('>')))
        .map(|inner| inner.split_once(':').map_or(inner, |(n, _)| n))
        .collect();
    let mut words = line.split_whitespace().skip(literals);
    for (name, kind) in &params {
        if *kind == "text" {
            let rest: Vec<&str> = words.by_ref().collect();
            if !rest.is_empty() {
                args.set(*name, rest.join(" "))?;
            } else if required.contains(name) {
                return Ok(Some(usage_line()));
            }
            break;
        }
        let Some(word) = words.next() else {
            if required.contains(name) {
                return Ok(Some(usage_line()));
            }
            break;
        };
        let quoted = format!("{word:?}");
        match *kind {
            "integer" => match word.parse::<i64>() {
                Ok(n) => args.set(*name, n)?,
                Err(_) => {
                    return Ok(Some(Text::tr(
                        "horizon.lua.command.needs_integer",
                        &[name, &quoted],
                    )));
                }
            },
            // `NaN`, `inf` and a value past f64's range parse too: a
            // number a script can use is finite, so they are refused.
            "number" => match word.parse::<f64>().ok().filter(|n| n.is_finite()) {
                Some(n) => args.set(*name, n)?,
                None => {
                    return Ok(Some(Text::tr(
                        "horizon.lua.command.needs_number",
                        &[name, &quoted],
                    )));
                }
            },
            "boolean" => args.set(*name, word == "true")?,
            _ => args.set(*name, word)?,
        }
    }
    let _ = lua;
    Ok(None)
}

fn install_timers(lua: &Lua, api: &Table, id: &str, side: LuaSide) -> mlua::Result<()> {
    let install: Function = lua.load(TIMERS_CODE).set_name("horizon").eval()?;
    let base: Table = lua.named_registry_value(BASE)?;
    let id = id.to_owned();
    // The state's own tick: `server_tick` on the server, `client_tick_post`
    // on the client. A client timer counts client ticks, and vice versa.
    let hook = match side {
        LuaSide::Server => "server_tick",
        LuaSide::Client => "client_tick_post",
    };
    let hook_report = format!("{hook}: timer");
    // A timer's failure reads as a handler's does: the same formatter, the
    // traceback capped to 20 frames.
    let report = lua.create_function(move |_, message: String| {
        let text = handler_error_text(&hook_report, &mlua::Error::runtime(message));
        log::error!("[lua:{id}] {text}");
        Ok(())
    })?;
    let handler: Function = install.call((api, report, base))?;
    // The timers' own handler heads the hook's list, before any script's.
    // It is the runtime's, not the script's, so it is put there here and
    // not through `horizon.on`: the side gate refuses a state that
    // registers a hook of the other side, and a timer is not one.
    let handlers: Table = lua.named_registry_value(HANDLERS)?;
    table_in(lua, &handlers, hook)?.push(handler.clone())?;
    lua.set_named_registry_value(TIMERS_HANDLER, handler)
}
/// The registry key of the modules `require` ran: `name -> value`.
const LOADED: &str = "horizon_loaded";

/// One scripted mod: its folder and its Lua state.
pub struct LuaMod {
    id: String,
    /// The game this state runs in: its folder, entry and `horizon.side`.
    side: LuaSide,
    /// The mod's version, from its manifest (`horizon.mod_version`).
    version: String,
    /// The versions of the mods it depends on, by id (`horizon.libraries`).
    library_versions: Vec<(String, String)>,
    /// The `.hmod` or folder the scripts came from.
    source: PathBuf,
    pub(crate) lua: Lua,
    /// The scripts, by path, as last read from `source`.
    scripts: Scripts,
    /// The mods it depends on (`depends`), whose scripts `require` reaches
    /// after the mod's own: id and source, in the manifest's order.
    libraries: Vec<(String, PathBuf)>,
    /// The libraries' scripts, by path, the first library's winning.
    library_scripts: Scripts,
    /// The goals the scripts defined (`horizon.goal`), by name.
    goals: Goals,
    /// The commands the scripts declared (`horizon.command`), for the
    /// server's tree; their handlers are kind handlers of the
    /// `perform_command` hook.
    commands: Arc<Mutex<Vec<NativeCommand>>>,
    /// The blocks the scripts gave a block entity (`horizon.block_entity`), by block id.
    block_entities: BlockEntities,
    /// Data loads seen (`data_pack_files`): the scripts reload from the second on.
    loads: AtomicU32,
    /// The running chunk's deadline, for the state's interrupt.
    budget: Budget,
    /// The kinds of mob the scripts watch, by hook (`horizon.on(name, fn,
    /// kind)`), so a mob hook of any other kind costs nothing.
    kinds: Kinds,
    /// Consecutive frame-budget overruns, by frame hook: a frame hook's
    /// handler that keeps missing `FRAME_BUDGET` is disabled after
    /// `FRAME_OVERRUNS_UNTIL_DISABLED`, until `/reload`.
    frame_overruns: Mutex<HashMap<String, u32>>,
    /// The mod's saved data on the world (`horizon.world_data()`) and on a
    /// player (`horizon.player_data(who)`): attachments on the level and
    /// the player, under the mod's id.
    world_attachment: AttachmentType<Compound>,
    player_attachment: AttachmentType<Compound>,
    /// The mod's saved data on an entity (`horizon.entity_data(id)`): the
    /// attachment under the mod's id, saved with the entity.
    entity_attachment: AttachmentType<Compound>,
    /// The names the player hooks told, by uuid: the name of a player the
    /// host has no record of at the moment (one dying, whose state is out of
    /// reach then). The generated `player` seeds keep it through
    /// `player_by_uuid`.
    names: Mutex<HashMap<u128, String>>,
    /// The mod's settings file (`horizon.config`): the server state uses
    /// `config/<id>.json` of the game folder, the client state
    /// `config/<id>.client.json`; none when the folder is not known (a test).
    config_path: Option<PathBuf>,
    /// Reloads done, for the status.
    reloads: AtomicU32,
    /// The `RELOAD_ALL` generation served last.
    reload_seen: AtomicU32,
    /// One thread at a time runs Lua for this state.
    turn: turn::ReentrantTurn,
}

/// A mod's goal definitions, shared with the `horizon.goal` function.
type Goals = Arc<Mutex<HashMap<String, goals::GoalDef>>>;
/// The kinds watched by hook, shared with the `horizon.on` function.
type Kinds = Arc<Mutex<HashMap<String, HashSet<String>>>>;

/// A mob kind as the mob hooks name it: `minecraft:zombie`, or a content
/// mod's own id as it is. Item ids too (`hud_hooks` qualifies what it draws).
pub(crate) fn qualify(kind: &str) -> String {
    if kind.contains(':') {
        kind.to_owned()
    } else {
        ["minecraft:", kind].concat()
    }
}

/// The mod's attachment under another name: one per holder kind.
fn mod_attachment_named(id: &str, name: &'static str) -> AttachmentType<Compound> {
    AttachmentType {
        namespace: horizon_client::mobs::native_goal::intern(id),
        name,
        default: Compound::default,
        codec: Some(AttachmentCodec {
            encode: |c| Ok(c.clone()),
            decode: |c| Ok(c.clone()),
        }),
        copy_on_death: false,
        sync: false,
    }
}

/// A scripted mod's saved data as the server thread holds it now, for the
/// console's `mc.mod_state`: the world's table, and each online player's,
/// keyed by uuid text. Read only; a holder with nothing saved reads empty.
/// The attachment names are private to this module, so the console asks here.
pub(crate) fn saved_data(
    host: &mut dyn Host,
    id: &str,
) -> Result<(Compound, Vec<(String, Compound)>), String> {
    let world = mod_attachment_named(id, "world");
    let player = mod_attachment_named(id, "player");
    let world_data = world
        .get_existing(&mut *host, Holder::Level)
        .map_err(|e| format!("{id}: the world data cannot be read ({e:?})"))?
        .unwrap_or_default();
    let mut players = Vec::new();
    for p in host.level_players() {
        let data = player
            .get_existing(&mut *host, Holder::Player(p.uuid))
            .map_err(|e| format!("{id}: a player's data cannot be read ({e:?})"))?
            .unwrap_or_default();
        players.push((uuid_text(p.uuid), data));
    }
    Ok((world_data, players))
}

/// `text` as a UUID is written: `8-4-4-4-12` hex digits.
fn uuid_text(uuid: u128) -> String {
    let hex = format!("{uuid:032x}");
    [
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..],
    ]
    .join("-")
}

/// What a script asked for a block's block entity: whether it ticks, and
/// the saved compound a new one starts with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockEntityDecl {
    pub ticks: bool,
    pub data: Compound,
}
/// A mod's block entity declarations, shared with `horizon.block_entity`.
type BlockEntities = Arc<Mutex<HashMap<String, BlockEntityDecl>>>;

impl std::fmt::Debug for LuaMod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaMod")
            .field("id", &self.id)
            .field("side", &self.side)
            .field("source", &self.source)
            .finish()
    }
}

/// The scripts of a mod, by path (`scripts/init.lua`), read from its
/// `.hmod` or folder, shared with `require`.
type Scripts = Arc<Mutex<HashMap<String, String>>>;

/// The scripted mods the game runs, in order, as the trait mod list
/// `entry::main_with` takes: the installed mods in `horizon-mods/` that are
/// on (`horizon-mods.txt`, as the content loader reads it) and the `--dev-mod`
/// folders of the command line, when they have capability `scripts`. Each
/// mod gives one state per side it has a folder for (`scripts/server/`,
/// `scripts/client/`); a `--dedicated` server gives no client state. One
/// whose scripts fail to load is left out with the error in the log.
pub fn discover(args: impl Iterator<Item = String>) -> Vec<Box<dyn Hooks>> {
    load_states(args)
        .into_iter()
        .map(|m| Box::new(m) as Box<dyn Hooks>)
        .collect()
}

/// The states `discover` runs, in load order (see there), before they are boxed.
fn load_states(args: impl Iterator<Item = String>) -> Vec<LuaMod> {
    let mut dev: Vec<PathBuf> = Vec::new();
    let mut game_dir: Option<PathBuf> = None;
    let mut dedicated = false;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--dedicated" {
            dedicated = true;
            continue;
        }
        let value = |flag: &str, args: &mut std::iter::Peekable<_>| -> Option<String> {
            match arg.strip_prefix(flag) {
                Some(rest) => rest.strip_prefix('=').map(str::to_owned),
                None => None,
            }
            .or_else(|| (arg == flag).then(|| args.next()).flatten())
        };
        if let Some(folder) = value("--game-dir", &mut args) {
            let dir = PathBuf::from(folder);
            game_dir = Some(std::path::absolute(&dir).unwrap_or(dir));
            continue;
        }
        let Some(folder) = value("--dev-mod", &mut args) else {
            continue;
        };
        let dir = PathBuf::from(folder);
        dev.push(std::path::absolute(&dir).unwrap_or(dir));
    }
    let here = std::env::current_dir().unwrap_or_default();
    // The game folder: `--game-dir` when given (the game moves there only after this runs, so a
    // game started elsewhere, as horizon-ctl starts it, still finds its installed mods), else here.
    let root = game_dir.clone().unwrap_or_else(|| here.clone());
    let enabled =
        horizon_client::mod_list::ModList::load(&horizon_client::logging::shared_dir(&root))
            .enabled;
    let mut paths = horizon_mods::find(&root.join(horizon_client::mod_list::DIR));
    paths.retain(|p| !dev.contains(p));
    paths.extend(dev.iter().cloned());
    // Every mod that is on, scripted or not: a dependency may be content only.
    let mut on: Vec<Candidate> = Vec::new();
    for path in &paths {
        let Ok(package) = Package::read(path) else {
            continue;
        };
        let m = package.manifest;
        let scripted = m.capabilities.contains(&Capability::Scripts);
        let entry = [LuaSide::Server, LuaSide::Client]
            .map(LuaSide::entry)
            .into_iter()
            .find(|e| path.join(e).is_file());
        if let Some(entry) = entry.filter(|_| !scripted && dev.contains(path)) {
            log::warn!(
                "Horizon: {} has {entry} but no capability scripts in {MANIFEST}: not run",
                path.display()
            );
        }
        if !(enabled.contains(&m.id) || dev.contains(path)) || on.iter().any(|c| c.id == m.id) {
            continue;
        }
        on.push(Candidate {
            id: m.id,
            version: m.version,
            path: path.clone(),
            scripted,
            depends: m.depends,
        });
    }
    let mut mods: Vec<LuaMod> = Vec::new();
    for step in load_order(&on) {
        let c = &on[step.index];
        if let Some(missing) = step.missing {
            let e = format!("needs {missing}, which is not installed, not on, or not that version");
            log::error!("Horizon: Lua mod {} not loaded: it {e}", c.id);
            to_console(true, format!("[{}] not loaded: {e}", c.id));
            set_status(ModStatus {
                id: c.id.clone(),
                source: c.path.display().to_string(),
                error: Some(e),
                ..Default::default()
            });
            continue;
        }
        if !c.scripted {
            continue;
        }
        let sides = match Package::read(&c.path).and_then(|p| Content::read(&c.path, &p.manifest)) {
            Ok(content) => sides_of(&content, dedicated),
            Err(e) => {
                report_not_loaded(c, None, &e);
                continue;
            }
        };
        let libraries: Vec<(String, PathBuf)> = c
            .depends
            .iter()
            .filter_map(|d| {
                let id = horizon_mods::manifest::parse_dependency(d).ok()?.0;
                on.iter()
                    .find(|o| o.id == id)
                    .map(|o| (o.id.clone(), o.path.clone()))
            })
            .collect();
        // The game folder's `config/`, as the host's `config_dir` names it
        // (the game moves there once its options are read: `--game-dir`).
        let config_dir = game_dir.as_deref().unwrap_or(&here).join("config");
        for side in sides {
            match LuaMod::load_with(&c.path, side, &libraries, Some(&config_dir)) {
                Ok(lua) => {
                    log::info!(
                        "Horizon: Lua mod {} [{}] loaded from {}",
                        lua.id,
                        side.as_str(),
                        c.path.display()
                    );
                    lua.publish(None);
                    mods.push(lua);
                }
                Err(e) => report_not_loaded(c, Some(side), &format!("[{}] {e}", side.as_str())),
            }
        }
    }
    mods
}

/// The sides a scripted mod runs a state on: `Server` when it has
/// `scripts/server/init.lua`, `Client` when it has `scripts/client/init.lua`
/// and the process is not a dedicated server (which has no window).
fn sides_of(content: &Content, dedicated: bool) -> Vec<LuaSide> {
    let mut sides = Vec::new();
    if content.has_server() {
        sides.push(LuaSide::Server);
    }
    if content.has_client() && !dedicated {
        sides.push(LuaSide::Client);
    }
    sides
}

/// A scripted mod whose state did not load: the log, the console and the
/// status row of the mod in the side `side` (None: no side ran yet).
fn report_not_loaded(c: &Candidate, side: Option<LuaSide>, e: &str) {
    log::error!("Horizon: Lua mod in {} not loaded: {e}", c.path.display());
    to_console(true, format!("[{}] not loaded: {e}", c.path.display()));
    set_status(ModStatus {
        id: c.id.clone(),
        side: side.map(|s| s.as_str().to_owned()).unwrap_or_default(),
        source: c.path.display().to_string(),
        error: Some(e.to_owned()),
        ..Default::default()
    });
}

/// Loads the mod at `path` headlessly, as the game would at start, with the
/// libraries it depends on found among `dev` (mod folders or `.hmod`s), and
/// says what went wrong: a script that fails to load (file and line), a hook
/// `horizon.on` does not know, a command or goal that errors when declared.
/// `Ok` carries what loaded: the mod's id and its scripts and the libraries,
/// one line each. With `resources` (the prepared vanilla folder), the mod's
/// content is also layered on it as the game does at start
/// (`mod_content::build`), so a file the game would refuse, which leaves every
/// content mod out of the world, fails the check instead; a content mod
/// without scripts gets only that. For `horizon-client --check-mod <path>
/// [--dev-mod <lib>]... [--resources <folder>]` and `cargo horizon-mod check`.
pub fn check_mod(
    path: &Path,
    dev: &[PathBuf],
    resources: Option<&Path>,
) -> Result<Vec<String>, String> {
    let at = |e: String| format!("{}: {e}", path.display());
    let package = Package::read(path).map_err(at)?;
    let manifest = package.manifest;
    let scripted = manifest.capabilities.contains(&Capability::Scripts);
    if !scripted && resources.is_none() {
        return Err(at(format!(
            "no capability scripts in {MANIFEST}: a content mod, nothing to check here without --resources"
        )));
    }
    // The registry script, as the content loader runs it.
    let content = Content::read(path, &manifest).map_err(at)?;
    let mut lines = Vec::new();
    // Every side the mod has a folder for is loaded, the dedicated one too.
    let sides = sides_of(&content, false);
    if let Some(base) = resources {
        lines.extend(check_content(base, &manifest.id, content).map_err(at)?);
    }
    if !scripted {
        return Ok(lines);
    }
    let mut libraries = Vec::new();
    for dep in &manifest.depends {
        let (id, min) = horizon_mods::manifest::parse_dependency(dep).map_err(at)?;
        let found = dev.iter().find_map(|d| {
            Package::read(d)
                .ok()
                .filter(|p| p.manifest.id == id)
                .map(|p| (d, p))
        });
        match found {
            Some((d, p)) => {
                if let Some(min) = min
                    && !horizon_mods::manifest::version_at_least(&p.manifest.version, min)
                {
                    return Err(at(format!(
                        "depends on {dep}, found {id} {} among the --dev-mod folders",
                        p.manifest.version
                    )));
                }
                libraries.push((id, d.clone()));
            }
            None => {
                return Err(at(format!(
                    "depends on {dep}, not among the --dev-mod folders given"
                )));
            }
        }
    }
    // A settings folder of its own: `horizon.config()` works at load, and
    // what a script saves there is thrown away with it.
    let config = std::env::temp_dir().join(format!("horizon-check-mod-{}", std::process::id()));
    let mut failures = Vec::new();
    for side in sides {
        match LuaMod::load_with(path, side, &libraries, Some(&config)) {
            Ok(m) => {
                lines.push(format!("{}: [{}] scripts loaded", m.id, side.as_str()));
                if let Ok(scripts) = m.scripts.lock() {
                    let mut names: Vec<&String> = scripts.keys().collect();
                    names.sort();
                    for name in names {
                        lines.push(format!("  {name}"));
                    }
                }
            }
            Err(e) => failures.push(at(format!("[{}] {e}", side.as_str()))),
        }
    }
    let _ = std::fs::remove_dir_all(&config);
    if !failures.is_empty() {
        return Err(failures.join("\n"));
    }
    for (id, lib) in &libraries {
        lines.push(format!("  with {id} from {}", lib.display()));
    }
    Ok(lines)
}

/// Layers `content` on the vanilla folder `base` in a scratch folder, as the
/// game's content root does, and answers its warnings, or the error the game
/// would stop on.
fn check_content(base: &Path, id: &str, content: Content) -> Result<Vec<String>, String> {
    let out = std::env::temp_dir().join(format!("horizon-check-content-{}", std::process::id()));
    let mods = [horizon_client::mod_content::ContentMod {
        id: id.to_owned(),
        content,
    }];
    let built = horizon_client::mod_content::build(base, &mods, &out);
    let _ = std::fs::remove_dir_all(&out);
    let warnings = built.map_err(|e| format!("its content would not load in a game: {e}"))?;
    let mut lines = vec![format!("{id}: content layered on {}", base.display())];
    lines.extend(warnings.into_iter().map(|w| format!("  warning: {w}")));
    Ok(lines)
}

/// A mod that is on, as `discover` found it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Candidate {
    id: String,
    version: String,
    path: PathBuf,
    scripted: bool,
    depends: Vec<String>,
}

/// One mod's turn in the load order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    index: usize,
    /// A dependency that is not among the mods, or that could not load
    /// itself: the mod is skipped with this named.
    missing: Option<String>,
}

/// The order to load `on` in: a mod after everything it depends on. A mod
/// whose dependency is missing, failed, or in a cycle gets a `missing` step.
fn load_order(on: &[Candidate]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    let mut done: Vec<bool> = vec![false; on.len()];
    let mut failed: Vec<bool> = vec![false; on.len()];
    loop {
        let mut progressed = false;
        for (i, c) in on.iter().enumerate() {
            if done[i] {
                continue;
            }
            // A dependency is missing when no mod of its id is on, that mod
            // failed, or its version is below the least one asked for.
            let missing = c.depends.iter().find_map(|d| {
                let Ok((id, min)) = horizon_mods::manifest::parse_dependency(d) else {
                    return Some(format!("{d} (not a dependency)"));
                };
                match on.iter().position(|o| o.id == id) {
                    None => Some(id),
                    Some(j) if failed[j] => Some(id),
                    Some(j) => match min {
                        Some(min)
                            if !horizon_mods::manifest::version_at_least(&on[j].version, min) =>
                        {
                            Some(format!("{d} (found {})", on[j].version))
                        }
                        _ => None,
                    },
                }
            });
            if let Some(missing) = missing {
                done[i] = true;
                failed[i] = true;
                progressed = true;
                steps.push(Step {
                    index: i,
                    missing: Some(missing),
                });
                continue;
            }
            let ready = c.depends.iter().all(|d| {
                let id = horizon_mods::manifest::parse_dependency(d)
                    .map(|(id, _)| id)
                    .unwrap_or_default();
                on.iter().position(|o| o.id == id).is_some_and(|j| done[j])
            });
            if ready {
                done[i] = true;
                progressed = true;
                steps.push(Step {
                    index: i,
                    missing: None,
                });
            }
        }
        if done.iter().all(|d| *d) {
            return steps;
        }
        if !progressed {
            // What is left waits on each other: a cycle.
            for (i, c) in on.iter().enumerate() {
                if !done[i] {
                    done[i] = true;
                    failed[i] = true;
                    steps.push(Step {
                        index: i,
                        missing: c.depends.first().cloned(),
                    });
                }
            }
            return steps;
        }
    }
}

impl LuaMod {
    /// Load the mod at `path` (a `.hmod` or a mod folder) as its `side`
    /// state: its id from `horizon-mod.toml`, the side's scripts read once,
    /// the side's entry run.
    pub fn load(
        path: &Path,
        side: LuaSide,
        libraries: &[(String, PathBuf)],
    ) -> Result<Self, String> {
        Self::load_with(path, side, libraries, None)
    }

    /// `load`, with `config_dir` the folder of the mods' settings files
    /// (`config/` of the game folder: `horizon.config` reads `<id>.json`
    /// there on the server, `<id>.client.json` on the client, at load too).
    pub fn load_with(
        path: &Path,
        side: LuaSide,
        libraries: &[(String, PathBuf)],
        config_dir: Option<&Path>,
    ) -> Result<Self, String> {
        let (id, version, scripts) = Self::read_scripts(path, side)?;
        let (library_scripts, library_versions) = Self::read_libraries(libraries, side)?;
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY).map_err(|e| e.to_string())?;
        let budget = Budget::default();
        budget.install(&lua);
        let world_attachment = mod_attachment_named(&id, "world");
        let player_attachment = mod_attachment_named(&id, "player");
        let entity_attachment = mod_attachment_named(&id, "data");
        let config_file = match side {
            LuaSide::Server => format!("{id}.json"),
            LuaSide::Client => format!("{id}.client.json"),
        };
        let config_path = config_dir.map(|d| d.join(config_file));
        let m = LuaMod {
            id,
            side,
            version,
            library_versions,
            source: path.to_owned(),
            lua,
            scripts: Arc::new(Mutex::new(scripts)),
            libraries: libraries.to_vec(),
            library_scripts: Arc::new(Mutex::new(library_scripts)),
            goals: Goals::default(),
            commands: Arc::default(),
            names: Mutex::default(),
            block_entities: BlockEntities::default(),
            loads: AtomicU32::new(0),
            budget,
            kinds: Kinds::default(),
            frame_overruns: Mutex::default(),
            world_attachment,
            player_attachment,
            entity_attachment,
            config_path,
            reloads: AtomicU32::new(0),
            reload_seen: AtomicU32::new(reload_generation()),
            turn: turn::ReentrantTurn::new(),
        };
        m.install_api().map_err(|e| e.to_string())?;
        m.run(side.entry()).map_err(|e| e.to_string())?;
        Ok(m)
    }

    /// Whether the mod's declared Lua hook surface matches this engine's:
    /// absent means no check (existing mods keep loading), present and equal
    /// to the engine's loads, and anything else is a load error naming the
    /// mod and both versions.
    fn check_hook_surface(manifest: &horizon_mods::Manifest) -> Result<(), String> {
        match manifest.lua_hook_surface {
            None => Ok(()),
            Some(pinned) if pinned == crate::generated_hooks::LUA_HOOK_SURFACE_VERSION => Ok(()),
            Some(pinned) => Err(format!(
                "mod {} was built for Lua hook surface {pinned}, this engine has {}",
                manifest.id,
                crate::generated_hooks::LUA_HOOK_SURFACE_VERSION
            )),
        }
    }

    /// The mod's id, version and the scripts of `side` (its folder and
    /// `scripts/shared/`), read from its `.hmod` or folder. The other side's
    /// folder is not read: its files do not reach this state.
    fn read_scripts(
        path: &Path,
        side: LuaSide,
    ) -> Result<(String, String, HashMap<String, String>), String> {
        let at = |e: String| format!("{}: {e}", path.display());
        let package = Package::read(path).map_err(at)?;
        let manifest = package.manifest;
        Self::check_hook_surface(&manifest).map_err(at)?;
        if !manifest.capabilities.contains(&Capability::Scripts) {
            return Err(at(format!("no capability scripts in {MANIFEST}")));
        }
        let content = Content::read(path, &manifest).map_err(at)?;
        let mut reader = content.reader().map_err(at)?;
        let own = match side {
            LuaSide::Server => &content.server_scripts,
            LuaSide::Client => &content.client_scripts,
        };
        let names: Vec<&String> = own.iter().chain(&content.shared_scripts).collect();
        let mut scripts = HashMap::with_capacity(names.len());
        for name in names {
            let bytes = reader.read(name).map_err(at)?;
            let code = String::from_utf8(bytes).map_err(|_| at(format!("{name}: not UTF-8")))?;
            scripts.insert(name.clone(), code);
        }
        Ok((manifest.id, manifest.version, scripts))
    }

    /// The libraries' scripts of `side`, the first library's winning a shared
    /// name, and each library's version by id. A library without scripts
    /// (content only) contributes none.
    fn read_libraries(
        libraries: &[(String, PathBuf)],
        side: LuaSide,
    ) -> Result<(HashMap<String, String>, Vec<(String, String)>), String> {
        let mut out: HashMap<String, String> = HashMap::new();
        let mut versions = Vec::new();
        for (id, path) in libraries {
            let package = Package::read(path).map_err(|e| format!("library {id}: {e}"))?;
            versions.push((id.clone(), package.manifest.version.clone()));
            if !package.manifest.capabilities.contains(&Capability::Scripts) {
                continue;
            }
            let (_, _, scripts) =
                Self::read_scripts(path, side).map_err(|e| format!("library {id}: {e}"))?;
            for (name, code) in scripts {
                out.entry(name).or_insert(code);
            }
        }
        Ok((out, versions))
    }

    /// Reads the scripts again and runs `scripts/init.lua` afresh: the
    /// handlers, the goals, the commands, the block entities and the modules
    /// `require` ran are forgotten; the Lua state and its globals stay. The
    /// new script registers into fresh tables and maps, and on an error
    /// (a script that does not read, or fails anywhere in `init.lua`) the
    /// old ones come back whole: the mod keeps running as it was, with its
    /// old handlers, timers, commands and goals. A goal already on a live
    /// mob keeps the definition it was made with either way, until the
    /// mob's AI is set up again.
    pub fn reload(&self) -> Result<(), String> {
        let (_, _, scripts) = Self::read_scripts(&self.source, self.side)?;
        let (library_scripts, _) = Self::read_libraries(&self.libraries, self.side)?;
        // Hold the per-state turn for the whole reload: the registry
        // tables, timers and `run` touch the host slot and the queue.
        let _guard = self
            .turn
            .try_acquire_timeout(turn::TURN_TIMEOUT)
            .ok_or_else(|| {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                format!("{}: turn timed out ({})", self.id, note)
            })?;
        let lua = &self.lua;
        // What the running mod registered, kept aside while the new script
        // runs: the registry tables, the timers (their handler and the
        // `horizon.after`/`every`/`ticks` closures over their list) and the
        // maps the `horizon` functions fill.
        let api: Table = lua
            .globals()
            .get::<Table>(GLOBAL)
            .map_err(|e| e.to_string())?;
        // Set aside as they are before anything is replaced, so every error
        // from here on (staging included) puts them back.
        let mut registry: Vec<(&str, Value)> = Vec::new();
        let mut timers: Vec<(&str, Value)> = Vec::new();
        let staged = (|| -> mlua::Result<()> {
            for key in [HANDLERS, KIND_HANDLERS, LOADED, TIMERS_HANDLER] {
                registry.push((key, lua.named_registry_value::<Value>(key)?));
            }
            for name in ["after", "every", "ticks"] {
                timers.push((name, api.get::<Value>(name)?));
            }
            lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
            lua.set_named_registry_value(KIND_HANDLERS, lua.create_table()?)?;
            lua.set_named_registry_value(LOADED, lua.create_table()?)?;
            // The timers' handler is the first handler of the state's own
            // tick hook: a new one, over an empty list, for the new script.
            install_timers(lua, &api, &self.id, self.side)
        })();
        // Back as it was: the registry tables and the timers' closures.
        let restore = |registry: Vec<(&str, Value)>, timers: Vec<(&str, Value)>| {
            for (key, value) in registry {
                lua.set_named_registry_value(key, value)?;
            }
            for (name, value) in timers {
                api.set(name, value)?;
            }
            Ok::<(), mlua::Error>(())
        };
        if let Err(e) = staged {
            let restored = restore(registry, timers);
            restored.map_err(|e| e.to_string())?;
            return Err(e.to_string());
        }
        fn swap<T>(m: &Mutex<T>, new: T) -> T {
            let mut guard = m.lock().unwrap_or_else(|p| p.into_inner());
            std::mem::replace(&mut *guard, new)
        }
        let old_commands = swap(&self.commands, Vec::new());
        let old_goals = swap(&self.goals, HashMap::new());
        let old_kinds = swap(&self.kinds, HashMap::new());
        let old_declared = swap(&self.block_entities, HashMap::new());
        let old_scripts = swap(&self.scripts, scripts);
        let old_library_scripts = swap(&self.library_scripts, library_scripts);
        let Err(e) = self.run(self.side.entry()) else {
            return Ok(());
        };
        // Whatever the failed script registered is dropped with the fresh
        // tables; the old ones come back whole.
        let restored = restore(registry, timers);
        swap(&self.commands, old_commands);
        swap(&self.goals, old_goals);
        swap(&self.kinds, old_kinds);
        swap(&self.block_entities, old_declared);
        swap(&self.scripts, old_scripts);
        swap(&self.library_scripts, old_library_scripts);
        restored.map_err(|e| e.to_string())?;
        Err(e.to_string())
    }

    /// The mod's id, as its manifest says.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The side this state runs in.
    pub fn side(&self) -> LuaSide {
        self.side
    }

    /// Reload because of `why`, and tell the log and the console how it went.
    pub(crate) fn reload_now(&self, why: &str) {
        // Take the turn once and hold it across reload and publish: inner
        // takes are re-entrant, so a firing from another thread serialises
        // until the status write lands. If the outer take times out, the
        // reload must not run (and its writes must not be expected).
        let _outer_guard = match self.turn.try_acquire_timeout(turn::TURN_TIMEOUT) {
            Some(g) => g,
            None => {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                log::error!(
                    "Horizon: Lua mod {} reload_now: turn timed out ({})",
                    self.id,
                    note
                );
                return;
            }
        };
        match self.reload() {
            Ok(()) => {
                // Fresh tables, fresh chances: a frame hook disabled for
                // overruns registers again with its count cleared.
                if let Ok(mut overruns) = self.frame_overruns.lock() {
                    overruns.clear();
                }
                self.reloads.fetch_add(1, Ordering::Relaxed);
                log::info!("Horizon: Lua mod {} reloaded ({why})", self.id);
                to_console(false, format!("[{}] reloaded ({why})", self.id));
                self.publish(None);
            }
            Err(e) => {
                log::error!(
                    "Horizon: Lua mod {} not reloaded, still running as before: {e}",
                    self.id
                );
                to_console(
                    true,
                    format!("[{}] not reloaded, running as before: {e}", self.id),
                );
                self.publish(Some(e));
            }
        }
    }

    /// Whether the developer tools asked this mod to reload since last asked.
    fn reload_asked(&self) -> bool {
        reload_asked(&self.id, self.side.as_str(), &self.reload_seen)
    }

    /// The status the developer tools show, with `error` as the last error.
    fn publish(&self, error: Option<String>) {
        let _guard = match self.turn.try_acquire_timeout(turn::TURN_TIMEOUT) {
            Some(g) => g,
            None => {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                log::warn!("[lua:{}] publish: turn timed out ({})", self.id, note);
                return;
            }
        };
        let handlers = (|| -> mlua::Result<Vec<(String, usize)>> {
            let table: Table = self.lua.named_registry_value(HANDLERS)?;
            // The timers' own handler is the runtime's, not the script's.
            let timers: Option<Function> = self.lua.named_registry_value(TIMERS_HANDLER).ok();
            let mut list = Vec::new();
            for pair in table.pairs::<String, Table>() {
                let (name, fns) = pair?;
                let count = fns
                    .sequence_values::<Function>()
                    .filter(|f| f.as_ref().is_ok_and(|f| timers.as_ref() != Some(f)))
                    .count();
                if count > 0 {
                    list.push((name, count));
                }
            }
            list.sort();
            Ok(list)
        })()
        .unwrap_or_default();
        let mut goals: Vec<String> = self
            .goals
            .lock()
            .map(|g| g.keys().cloned().collect())
            .unwrap_or_default();
        goals.sort();
        set_status(ModStatus {
            id: self.id.clone(),
            side: self.side.as_str().to_owned(),
            source: self.source.display().to_string(),
            scripts: self.scripts.lock().map(|s| s.len()).unwrap_or(0),
            reloads: self.reloads.load(Ordering::Relaxed),
            handlers,
            goals,
            error,
        });
    }

    /// An error of a hook or an action: the log, the console, the status.
    pub(crate) fn error(&self, text: String) {
        log::error!("[lua:{}] {text}", self.id);
        to_console(true, format!("[{}] {text}", self.id));
        set_error(&self.id, self.side.as_str(), text);
    }

    /// Something dropped or odd: the log and the console.
    fn warn(&self, text: String) {
        log::warn!("[lua:{}] {text}", self.id);
        to_console(true, format!("[{}] {text}", self.id));
    }

    fn install_api(&self) -> mlua::Result<()> {
        let lua = &self.lua;
        let scripts = self.scripts.clone();
        let library_scripts = self.library_scripts.clone();
        let side = self.side;
        let gate_owner = self.id.clone();
        lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
        lua.set_named_registry_value(KIND_HANDLERS, lua.create_table()?)?;
        lua.set_named_registry_value(QUEUE, lua.create_table()?)?;
        lua.set_named_registry_value(LOADED, lua.create_table()?)?;
        let slot = lua.create_table()?;
        // Entry 4 is the draw-hook flag for the client filter below.
        slot.set(4, false)?;
        lua.set_named_registry_value(HOST_SLOT, &slot)?;
        // The base table first: the runner, the reads and the timers take
        // their globals from it, so none of them reads one a script may
        // have replaced.
        let base: Table = lua.load(BASE_CODE).set_name("horizon").eval()?;
        lua.set_named_registry_value(BASE, &base)?;
        let make_runner: Function = lua.load(RUNNER_CODE).set_name("horizon").eval()?;
        let runner: Function = make_runner.call((&slot, &base))?;
        lua.set_named_registry_value(RUNNER, runner)?;
        let api = lua.create_table()?;
        let id = self.id.clone();
        api.set("mod_id", id.as_str())?;
        api.set("side", side.as_str())?;
        // The manifest's version, and the versions of the mods this one
        // depends on: a library reads its own from `horizon.libraries`.
        api.set("mod_version", self.version.as_str())?;
        let libraries = lua.create_table()?;
        for (lib, version) in &self.library_versions {
            libraries.set(lib.as_str(), version.as_str())?;
        }
        api.set("libraries", libraries)?;
        api.set(
            "log",
            lua.create_function(move |_, msg: String| {
                log::info!("[lua:{id}] {msg}");
                to_console(false, format!("[{id}] {msg}"));
                Ok(())
            })?,
        )?;
        // `print` goes where `horizon.log` goes, not to the terminal.
        let id = self.id.clone();
        lua.globals().set(
            "print",
            lua.create_function(move |_, args: MultiValue| {
                let line: Vec<String> = args
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.to_string_lossy(),
                        other => format!("{other:#?}"),
                    })
                    .collect();
                let msg = line.join("\t");
                log::info!("[lua:{id}] {msg}");
                to_console(false, format!("[{id}] {msg}"));
                Ok(())
            })?,
        )?;
        // `horizon.on(name, fn[, kind])`: a handler of the hook `name`, one
        // of the generated hooks; with `kind`, only for events of that
        // kind (a kind-filtered hook), which keeps a hook that fires for
        // every kind free for the others. A state registers only the hooks
        // of its side and of both sides (`LuaSide::allows`).
        let kinds = self.kinds.clone();
        api.set(
            "on",
            lua.create_function(
                move |lua, (name, f, kind): (String, Function, Option<String>)| {
                    if !crate::generated_hooks::GENERATED_HOOK_NAMES.contains(&name.as_str()) {
                        return Err(mlua::Error::runtime(format!(
                            "horizon.on({name:?}): no such hook; the hooks are {}",
                            {
                                let mut names: Vec<&str> =
                                    crate::generated_hooks::GENERATED_HOOK_NAMES.to_vec();
                                names.sort_unstable();
                                names.join(", ")
                            }
                        )));
                    }
                    if let Some(hook) = crate::generated_hooks::hook_side(&name)
                        && !side.allows(hook)
                    {
                        // The hook's own side: the one its folder is named for.
                        let runs = match hook {
                            HookSide::Client => "client",
                            HookSide::Server | HookSide::Both => "server",
                        };
                        return Err(mlua::Error::runtime(format!(
                            "hook '{name}' runs on the {runs}: register it from scripts/{runs}/ \
                             (this is the {} state of mod {gate_owner})",
                            side.as_str()
                        )));
                    }
                    // The generated hooks without a kind filter answer every
                    // event to every handler: a kind would silently never
                    // fire. Kind-filtered generated hooks register like the
                    // mob hooks.
                    if kind.is_some()
                        && crate::generated_hooks::GENERATED_HOOK_NAMES.contains(&name.as_str())
                        && !crate::generated_hooks::GENERATED_KIND_HOOK_NAMES
                            .contains(&name.as_str())
                    {
                        return Err(mlua::Error::runtime(format!(
                            "horizon.on({name:?}): this hook takes no kind"
                        )));
                    }
                    let list = match kind {
                        None => {
                            let handlers: Table = lua.named_registry_value(HANDLERS)?;
                            table_in(lua, &handlers, &name)?
                        }
                        Some(kind) => {
                            // Command words are not registry ids: a
                            // `perform_command` kind is stored as given
                            // (`parasite count`, not
                            // `minecraft:parasite count`), like
                            // `horizon.command` stores it.
                            let kind = if name == "perform_command" {
                                kind
                            } else {
                                qualify(&kind)
                            };
                            let all: Table = lua.named_registry_value(KIND_HANDLERS)?;
                            let by_kind = table_in(lua, &all, &name)?;
                            kinds
                                .lock()
                                .map_err(|_| mlua::Error::runtime("kinds"))?
                                .entry(name.clone())
                                .or_default()
                                .insert(kind.clone());
                            table_in(lua, &by_kind, &kind)?
                        }
                    };
                    list.push(f)?;
                    Ok(())
                },
            )?,
        )?;
        // `horizon.command(usage, level, fn)`: a command of the mod's, in the
        // server's tree for players of `level` and above; `fn(player, args)`
        // runs it (the `perform_command` hook, by the command's name).
        let commands = self.commands.clone();
        let kinds = self.kinds.clone();
        api.set(
            "command",
            lua.create_function(
                move |lua, (usage, level, f): (String, Option<u8>, Function)| {
                    // The handler's key: the usage's words before its arguments.
                    let name = command_words(&usage);
                    if name.is_empty() {
                        return Err(mlua::Error::runtime(format!(
                            "horizon.command({usage:?}): a usage starts with the command's name"
                        )));
                    }
                    let mut list = commands
                        .lock()
                        .map_err(|_| mlua::Error::runtime("commands"))?;
                    if !list.iter().any(|c| c.usage == usage) {
                        list.push(NativeCommand::op(&usage, level.unwrap_or(0)));
                    }
                    let all: Table = lua.named_registry_value(KIND_HANDLERS)?;
                    let by_kind = table_in(lua, &all, "perform_command")?;
                    kinds
                        .lock()
                        .map_err(|_| mlua::Error::runtime("kinds"))?
                        .entry("perform_command".to_owned())
                        .or_default()
                        .insert(name.clone());
                    table_in(lua, &by_kind, &name)?.push(f)?;
                    Ok(())
                },
            )?,
        )?;
        // Goals: `horizon.goal(name, def)` defines one for `horizon.add_goal`.
        // Its name for F3 and the tools' /goals is `mod_id:name`.
        let goals = self.goals.clone();
        let id = self.id.clone();
        api.set(
            "goal",
            lua.create_function(move |lua, (name, def): (String, Table)| {
                let mut goals = goals.lock().map_err(|_| mlua::Error::runtime("goals"))?;
                // One name for the game's life: a redefinition keeps the first string.
                let leaked: &'static str = match goals.get(name.as_str()) {
                    Some(g) => g.name,
                    None => Box::leak(format!("{id}:{name}").into_boxed_str()),
                };
                let def = goals::GoalDef::parse(lua, leaked, def)?;
                goals.insert(name, def);
                Ok(())
            })?,
        )?;
        // Block entities: `horizon.block_entity(block, { ticks = true, data = {...} })`
        // gives the mod's block one, made with `data` when the block is placed,
        // ticked each server tick when `ticks` (`block_entity_tick`).
        let declared = self.block_entities.clone();
        api.set(
            "block_entity",
            lua.create_function(move |_, (block, def): (String, Option<Table>)| {
                let mut decl = BlockEntityDecl::default();
                if let Some(def) = def {
                    decl.ticks = def.get::<Option<bool>>("ticks")?.unwrap_or(false);
                    if let Some(data) = def.get::<Option<Table>>("data")? {
                        decl.data = nbt::table_to_compound(&data, None)?;
                    }
                }
                declared
                    .lock()
                    .map_err(|_| mlua::Error::runtime("block entities"))?
                    .insert(block, decl);
                Ok(())
            })?,
        )?;
        api.set("distance", lua.create_function(goals::distance)?)?;
        // Actions on the game, run after the handler with the hook's host
        // (or, in `mob_ai_register`, on the mob).
        for action in [
            "broadcast",
            "summon",
            "kill_mob",
            "remove_mob",
            "set_mob_health",
            "push_mob",
            "teleport_mob",
            "take_items",
            "give_items",
            "feedback",
            "tell",
            "action_bar",
            "title",
            "subtitle",
            "title_times",
            "sound",
            "particles",
            "add_goal",
            "add_target_goal",
            "clear_goals",
            "clear_target_goals",
            "set_body_rotation",
            "set_block_property",
            "schedule_tick",
            "handled",
            "cancel",
            "set_block",
            "fill",
            "run_command",
            "give_experience",
            "give_levels",
            "falling_block",
            "drop_item",
            "set_nickname",
            "effect",
            "net_send_to_player",
            "send_custom_payload_to_player",
            "net_send_to_server",
            "open_menu",
            "open_menu_with_input",
            "set_menu_items",
            "close_menu",
        ] {
            let slot = slot.clone();
            api.set(
                action,
                lua.create_function(move |lua, args: MultiValue| {
                    // Outside a hook's handlers nothing would take the
                    // action: it would sit in the queue for the state's life.
                    if slot.raw_get::<Option<i64>>(3)?.unwrap_or(0) == 0 {
                        return Err(mlua::Error::runtime(format!(
                            "horizon.{action}: actions work inside a hook's handler (or a \
                             timer), which the game runs them after; a goal function acts \
                             through its `out` table, and init.lua registers handlers"
                        )));
                    }
                    let queue: Table = lua.named_registry_value(QUEUE)?;
                    let entry = lua.create_table()?;
                    entry.set("action", action)?;
                    entry.set("args", lua.create_sequence_from(args)?)?;
                    queue.push(entry)?;
                    Ok(())
                })?,
            )?;
        }
        // Calls the deleted hooks used to queue, with no consumer left:
        // installing them above would take the call and drop it in
        // `run_action`'s `unknown action` warn, reporting success. Each
        // fails at the call instead, naming the event field that replaces
        // it, so the handler fails through the hook's error path.
        for (action, message) in [
            (
                "set_count",
                "horizon.set_count does nothing: inventory_tick has no stack.",
            ),
            (
                "set_result",
                "horizon.set_result does nothing: set event.result = { id = ..., count = ... } or nil in living_entity_use_item_finish.",
            ),
            (
                "set_duration",
                "horizon.set_duration does nothing: set event.duration = n in living_entity_use_item_start.",
            ),
            (
                "set_distance",
                "horizon.set_distance does nothing: set event.distance = n in living_fall.",
            ),
            (
                "set_multiplier",
                "horizon.set_multiplier does nothing: set event.multiplier = n in living_fall.",
            ),
            (
                "set_strength",
                "horizon.set_strength does nothing: set event.strength = n in living_knockback.",
            ),
            (
                "set_amount",
                "horizon.set_amount does nothing: set event.amount = n in item_damaged, living_damage_pre, living_heal, living_incoming_damage, living_incoming_damage_v2, living_incoming_damage_v3, player_exp_change, player_hurt_by_player or player_xp_change.",
            ),
            (
                "reply",
                "horizon.reply does nothing: set event.replies in player_chat or player_command.",
            ),
        ] {
            let message = message.to_owned();
            api.set(
                action,
                lua.create_function(move |_, _: MultiValue| -> mlua::Result<()> {
                    Err(mlua::Error::runtime(message.clone()))
                })?,
            )?;
        }
        // The reads and the timers over the actions: a summon with a host at
        // hand is a read (it answers the id).
        install_reads(lua, &api)?;
        install_timers(lua, &api, &self.id, self.side)?;
        // `horizon.config()`: the mod's own settings file (`config/<id>.json`
        // on the server, `config/<id>.client.json` on the client) as a table
        // (empty when there is none); `horizon.save_config(t)` writes it.
        // No host needed: at load too.
        let config_path = self.config_path.clone();
        let id = self.id.clone();
        api.set(
            "config",
            lua.create_function(move |lua, ()| {
                let Some(path) = &config_path else {
                    return Err(mlua::Error::runtime("horizon.config: no config folder"));
                };
                let text = match std::fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Value::Table(lua.create_table()?));
                    }
                    Err(e) => {
                        return Err(mlua::Error::runtime(format!("{}: {e}", path.display())));
                    }
                };
                let json: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| mlua::Error::runtime(format!("{}: {e}", path.display())))?;
                match json::json_to_lua(lua, &json)? {
                    Value::Table(t) => Ok(Value::Table(t)),
                    _ => Err(mlua::Error::runtime(format!(
                        "{}: not a JSON object",
                        path.display()
                    ))),
                }
            })?,
        )?;
        let config_path = self.config_path.clone();
        api.set(
            "save_config",
            lua.create_function(move |_, t: Table| {
                let Some(path) = &config_path else {
                    return Err(mlua::Error::runtime(
                        "horizon.save_config: no config folder",
                    ));
                };
                let json = json::lua_to_json(&Value::Table(t))?;
                let text = serde_json::to_string_pretty(&json).map_err(mlua::Error::runtime)?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)
                        .map_err(|e| mlua::Error::runtime(format!("{}: {e}", dir.display())))?;
                }
                std::fs::write(path, text + "\n")
                    .map_err(|e| mlua::Error::runtime(format!("{}: {e}", path.display())))?;
                log::debug!("[lua:{id}] config saved to {}", path.display());
                Ok(())
            })?,
        )?;
        // `require("name")`: a module of this state's side folder, then of
        // `scripts/shared/`, run once.
        api.set(
            "require",
            lua.create_function(move |lua, name: String| {
                // `modkit.entity`, `modkit/entity` or `@modkit/entity` (the
                // form a Luau editor resolves through `.luaurc` aliases) is
                // `scripts/<side>/modkit/entity.lua`, or
                // `scripts/<side>/modkit/entity/init.lua`, or the same under
                // `scripts/shared/`: the mod's own first, then the mods it
                // depends on.
                let name = name.strip_prefix('@').unwrap_or(&name).replace('/', ".");
                let plain = !name.is_empty()
                    && !name.contains("..")
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
                if !plain {
                    return Err(mlua::Error::runtime(format!(
                        "require({name:?}): a module name, with dots or slashes for folders"
                    )));
                }
                // scripts/registry/ holds the registry script's modules,
                // which run once, when the content is read, with `registry`.
                if name == "registry" || name.starts_with("registry.") {
                    return Err(mlua::Error::runtime(format!(
                        "require({name:?}): scripts/registry/ holds the modules of \
                         scripts/registry.lua, which run once when the mod is read; \
                         the mod's scripts require their own folders"
                    )));
                }
                let loaded: Table = lua.named_registry_value(LOADED)?;
                if let Some(v) = loaded.get::<Option<Value>>(name.as_str())? {
                    return Ok(v);
                }
                let path = name.replace('.', "/");
                let keys = [
                    format!("scripts/{}/{path}.lua", side.as_str()),
                    format!("scripts/{}/{path}/init.lua", side.as_str()),
                    format!("{SHARED_DIR}/{path}.lua"),
                    format!("{SHARED_DIR}/{path}/init.lua"),
                ];
                let find = |map: &Scripts| -> Option<(String, String)> {
                    let map = map.lock().ok()?;
                    keys.iter()
                        .find_map(|k| map.get(k).map(|code| (k.clone(), code.clone())))
                };
                let (key, code) = find(&scripts)
                    .or_else(|| find(&library_scripts))
                    .ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "require({name:?}): no {} or {} in the mod or the mods it depends on",
                            keys[0], keys[2]
                        ))
                    })?;
                let v: Value = lua.load(code.as_str()).set_name(&key).eval()?;
                let v = if v.is_nil() { Value::Boolean(true) } else { v };
                loaded.set(name.as_str(), &v)?;
                Ok(v)
            })?,
        )?;
        // A client state keeps only `CLIENT_API`; every other entry raises
        // the server-only error. One pass over the finished table, so a
        // name added later is server-only by default. A draw hook
        // (`hud`, `gui_layer_post`) keeps the shared host, but only for
        // the names in `DRAW_API`: one of those delegates to the real
        // entry, which answers or refuses as a draw hook
        // (`Reader::Shared`); every other server-only name raises the
        // server-only error even inside a draw hook.
        if side == LuaSide::Client {
            let id = self.id.clone();
            let keys: Vec<String> = api
                .pairs::<String, Value>()
                .map(|result| result.map(|(name, _)| name))
                .collect::<mlua::Result<Vec<_>>>()?;
            for key in keys {
                if CLIENT_API.contains(&key.as_str()) {
                    continue;
                }
                let name = key.clone();
                let id = id.clone();
                let slot = slot.clone();
                let original: Value = api.get(key.clone())?;
                let Value::Function(original) = original else {
                    continue;
                };
                let stub = lua.create_function(
                    move |_, args: MultiValue| -> mlua::Result<Value> {
                        let draw = slot.raw_get::<Option<bool>>(4)?.unwrap_or(false);
                        if draw && DRAW_API.contains(&name.as_str()) {
                            original.call(args)
                        } else {
                            Err(mlua::Error::runtime(format!(
                                "horizon.{name} is only available in scripts/server/ (this is the client state of mod {id})"
                            )))
                        }
                    },
                )?;
                api.set(key, stub)?;
            }
        }
        lua.globals().set(GLOBAL, &api)?;
        lua.globals()
            .set("require", api.get::<Function>("require")?)?;
        Ok(())
    }

    /// Runs the script at `name` (the side's entry, `scripts/<side>/init.lua`).
    fn run(&self, name: &str) -> mlua::Result<()> {
        let code = self
            .scripts
            .lock()
            .ok()
            .and_then(|s| s.get(name).cloned())
            .ok_or_else(|| mlua::Error::runtime(format!("{name}: not in the mod")))?;
        // Hold the turn for the load; the budget starts after it.
        let _guard = self
            .turn
            .try_acquire_timeout(turn::TURN_TIMEOUT)
            .ok_or_else(|| {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                mlua::Error::runtime(format!("{name}: turn timed out ({note})"))
            })?;
        self.budget.run(LOAD_BUDGET, || {
            self.lua.load(code.as_str()).set_name(name).exec()
        })
    }

    /// Whether any handler is registered for `hook`: the hooks that fire
    /// often (random ticks) build their arguments only then. The frame
    /// hooks (`hud_hooks`) check it first, so an unregistered frame hook
    /// does no Lua work and no table allocation.
    pub(crate) fn has_handlers(&self, hook: &str) -> bool {
        let _guard = match self
            .turn
            .try_acquire_timeout_label(turn::TURN_TIMEOUT, Some(hook))
        {
            Some(g) => g,
            None => {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                log::error!("[lua:{}] {hook}: turn timed out ({})", self.id, note);
                return false;
            }
        };
        let handlers: mlua::Result<Table> = self.lua.named_registry_value(HANDLERS);
        handlers.is_ok_and(|h| h.get::<Option<Table>>(hook).ok().flatten().is_some())
    }

    /// Whether `hook` has a handler for every kind or one watching `kind`:
    /// the mob hooks fire for every mob, and build their arguments only then.
    fn watches(&self, hook: &str, kind: &str) -> bool {
        self.has_handlers(hook)
            || self
                .kinds
                .lock()
                .ok()
                .is_some_and(|k| k.get(hook).is_some_and(|set| set.contains(kind)))
    }

    /// The handlers of `hook`, in registration order: those for every kind,
    /// then those watching `kind`.
    fn handlers(&self, hook: &str, kind: Option<&str>) -> mlua::Result<Vec<Function>> {
        let _guard = self
            .turn
            .try_acquire_timeout_label(turn::TURN_TIMEOUT, Some(hook))
            .ok_or_else(|| {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                mlua::Error::runtime(format!("{hook}: turn timed out ({note})"))
            })?;
        let lua = &self.lua;
        let mut out = Vec::new();
        let handlers: Table = lua.named_registry_value(HANDLERS)?;
        if let Some(list) = handlers.get::<Option<Table>>(hook)? {
            for f in list.sequence_values::<Function>() {
                out.push(f?);
            }
        }
        if let Some(kind) = kind {
            let all: Table = lua.named_registry_value(KIND_HANDLERS)?;
            if let Some(by_kind) = all.get::<Option<Table>>(hook)?
                && let Some(list) = by_kind.get::<Option<Table>>(kind)?
            {
                for f in list.sequence_values::<Function>() {
                    out.push(f?);
                }
            }
        }
        Ok(out)
    }

    /// `{id, kind}` of an entity, for a hook without a host or a view.
    fn id_kind_table(&self, id: u32, kind: &str) -> mlua::Result<Table> {
        let t = self.lua.create_table()?;
        t.set("id", id)?;
        t.set("kind", kind)?;
        Ok(t)
    }

    /// The acting player's held item as a table (`item`, `count`, `plain`),
    /// or nil for an empty hand.
    fn held_table(&self, host: &dyn Host) -> mlua::Result<Value> {
        let lua = &self.lua;
        Ok(match host.held() {
            Some((item, count, plain)) => {
                let t = lua.create_table()?;
                t.set("item", item)?;
                t.set("count", count)?;
                t.set("plain", plain)?;
                Value::Table(t)
            }
            None => Value::Nil,
        })
    }

    /// The player `uuid` as a table: the world's record when they are on,
    /// else just `uuid` and `name` (the one given, else the one a player
    /// hook told before). The generated `player` seeds call this.
    fn player_by_uuid(
        &self,
        host: &dyn Host,
        uuid: u128,
        name: Option<&str>,
    ) -> mlua::Result<Table> {
        let name = name.filter(|n| !n.is_empty()).map(str::to_owned);
        let mut names = self.names.lock().unwrap();
        if let Some(name) = &name {
            names.insert(uuid, name.clone());
        }
        match host.level_players().into_iter().find(|p| p.uuid == uuid) {
            Some(p) => {
                names.insert(uuid, p.name.clone());
                player_table_of(&self.lua, &p)
            }
            None => {
                let t = self.lua.create_table()?;
                t.set("uuid", uuid_text(uuid))?;
                if let Some(name) = name
                    .as_deref()
                    .or_else(|| names.get(&uuid).map(String::as_str))
                {
                    t.set("name", name)?;
                }
                Ok(t)
            }
        }
    }

    fn hook_error(&self, hook: &str, e: mlua::Error) -> Flow {
        self.error(handler_error_text(hook, &e));
        Flow::Continue
    }

    /// A frame hook's (`hud`, `gui_layer_post`) handlers finished: `overrun`
    /// when the budget stopped them. A handler that errors, or runs clean,
    /// resets its count; one that keeps missing `FRAME_BUDGET` is disabled
    /// after `FRAME_OVERRUNS_UNTIL_DISABLED` consecutive misses, until
    /// `/reload` registers it again. Reports like any handler failure: the
    /// log, the console, and the mod's status row.
    pub(crate) fn note_frame_outcome(&self, hook: &str, overrun: bool) {
        let misses = {
            let mut all = self.frame_overruns.lock().unwrap();
            if !overrun {
                all.remove(hook);
                return;
            }
            let misses = all.entry(hook.to_owned()).or_insert(0);
            *misses += 1;
            if *misses < FRAME_OVERRUNS_UNTIL_DISABLED {
                return;
            }
            *misses
        };
        let _guard = match self.turn.try_acquire_timeout(turn::TURN_TIMEOUT) {
            Some(g) => g,
            None => {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                log::error!(
                    "[lua:{}] {hook}: turn timed out disabling frame hook ({})",
                    self.id,
                    note
                );
                return;
            }
        };
        if let Ok(handlers) = self.lua.named_registry_value::<Table>(HANDLERS) {
            let _ = handlers.set(hook, Value::Nil);
        }
        if let Ok(mut kinds) = self.kinds.lock() {
            kinds.remove(hook);
        }
        self.error(format!(
            "{hook}: disabled after {misses} consecutive overruns of the frame budget; /reload enables it again"
        ));
    }

    /// Call the handlers of `hook` (those for every kind and those watching
    /// `kind`) with `args` and no host to read from, then hand each queued
    /// action (its name and arguments) to `apply`, in order.
    fn call_bare(
        &self,
        hook: &str,
        kind: Option<&str>,
        args: impl mlua::IntoLuaMulti,
        apply: &mut dyn FnMut(&str, &Table) -> mlua::Result<()>,
    ) -> mlua::Result<()> {
        for (action, args) in self.run_handlers(hook, kind, Reader::None, args)? {
            apply(&action, &args)?;
        }
        Ok(())
    }

    /// Call the handlers of `hook` with `args`, reading from `host`, then
    /// hand each queued action to `apply` with the host to act on.
    fn call_kind(
        &self,
        hook: &str,
        kind: Option<&str>,
        host: &mut dyn Host,
        args: impl mlua::IntoLuaMulti,
        apply: &mut dyn FnMut(&mut dyn Host, &str, &Table) -> mlua::Result<()>,
    ) -> mlua::Result<()> {
        let actions = self.run_handlers(hook, kind, Reader::Host(&mut *host), args)?;
        for (action, args) in actions {
            apply(host, &action, &args)?;
        }
        Ok(())
    }

    /// `call_kind` on a level (`random_tick`, `tick_block`): the handlers read
    /// the level's blocks.
    fn call_level(
        &self,
        hook: &str,
        kind: Option<&str>,
        level: &mut dyn LevelWriteHost,
        args: impl mlua::IntoLuaMulti,
        apply: &mut dyn FnMut(&mut dyn LevelWriteHost, &str, &Table) -> mlua::Result<()>,
    ) -> mlua::Result<()> {
        let actions = self.run_handlers(hook, kind, Reader::Level(&*level), args)?;
        for (action, args) in actions {
            apply(level, &action, &args)?;
        }
        Ok(())
    }

    /// Whether `hook` has any handler at all, unfiltered or kind-filtered: a
    /// generated hook resolves its kind only then, so an unregistered hook
    /// does no Lua work and no host reads.
    fn has_any_handlers(&self, hook: &str) -> bool {
        self.has_handlers(hook)
            || self
                .kinds
                .lock()
                .ok()
                .is_some_and(|k| k.get(hook).is_some_and(|s| !s.is_empty()))
    }

    /// Runs the handlers of `hook` (those for every kind and those watching
    /// `kind`) with `args`, `reader` at hand for `horizon.block` and the other
    /// reads, as one call into the state: what they queued, in order.
    fn run_handlers(
        &self,
        hook: &str,
        kind: Option<&str>,
        reader: Reader<'_>,
        args: impl mlua::IntoLuaMulti,
    ) -> mlua::Result<Vec<(String, Table)>> {
        self.run_handlers_with(hook, kind, reader, args, HANDLER_BUDGET)
    }

    /// `run_handlers` with `limit` as one handler's time budget (the frame
    /// hooks in `hud_hooks` pass `FRAME_BUDGET`, for the hooks that fire
    /// every frame).
    pub(crate) fn run_handlers_with(
        &self,
        hook: &str,
        kind: Option<&str>,
        reader: Reader<'_>,
        args: impl mlua::IntoLuaMulti,
        limit: Duration,
    ) -> mlua::Result<Vec<(String, Table)>> {
        // One thread at a time per state; a wait does not count toward the
        // budget and is bounded at 5 s, after which this firing errors.
        let _guard = self
            .turn
            .try_acquire_timeout_label(turn::TURN_TIMEOUT, Some(hook))
            .ok_or_else(|| {
                let note = self
                    .turn
                    .held_by()
                    .unwrap_or_else(|| "unknown holder".to_owned());
                mlua::Error::runtime(format!("{hook}: turn timed out ({note})"))
            })?;
        let lua = &self.lua;
        let list = self.handlers(hook, kind)?;
        if list.is_empty() {
            return Ok(Vec::new());
        }
        let count = list.len() as u32;
        let runner: Function = lua.named_registry_value(RUNNER)?;
        let queue: Table = lua.named_registry_value(QUEUE)?;
        let slot: Table = lua.named_registry_value(HOST_SLOT)?;
        let handlers = lua.create_sequence_from(list)?;
        // What the call finds in the slot and the queue: a hook fired from
        // inside a handler runs nested, and gives both back as they were.
        // Nothing is below `start` unless a runner was stopped before it
        // could take its queue; a top-level call starts clean.
        let outer = (
            slot.get::<Value>(1)?,
            slot.get::<Value>(2)?,
            slot.get::<Option<i64>>(3)?.unwrap_or(0),
        );
        let outer_draw = slot.get::<Option<bool>>(4)?.unwrap_or(false);
        if outer.2 == 0 {
            queue.clear()?;
        }
        let start = queue.raw_len();
        // One call runs every handler; the read function lives for the
        // call's scope only, whatever kept it, and the saved tables it handed
        // out are written back after.
        let reader = std::cell::RefCell::new(reader);
        let is_draw = matches!(&*reader.borrow(), Reader::Shared(_));
        slot.set(4, is_draw)?;
        let fetched: std::cell::RefCell<Vec<Fetched>> = std::cell::RefCell::new(Vec::new());
        // The profiler's stopwatch, set around the budget's own span below:
        // `None` when the call never reached the handlers.
        let elapsed: std::cell::Cell<Option<Duration>> = std::cell::Cell::new(None);
        let result = lua.scope(|scope| {
            let read = match &*reader.borrow() {
                Reader::None => Value::Nil,
                _ => Value::Function(scope.create_function(
                    |lua, (what, rest): (String, MultiValue)| {
                        let mut reader = reader.borrow_mut();
                        self.read(lua, &mut reader, &fetched, &what, rest)
                    },
                )?),
            };
            // A draw hook reads through the shared host: `full`, so its
            // `horizon.summon` takes the direct read (which fails with a
            // draw-hook error) instead of queueing a summon for later.
            // Only the summon wrapper reads the slot's second entry.
            let full = matches!(&*reader.borrow(), Reader::Host(_) | Reader::Shared(_));
            let mut args = args.into_lua_multi(lua)?;
            args.push_front(Value::Boolean(full));
            args.push_front(read);
            args.push_front(Value::Table(handlers));
            // The runner returns a handler's message rather than raising it
            // (see RUNNER_CODE): raised here, with its text intact.
            // The profiler times the same span the budget times: the whole
            // hook firing (every handler of the mod for the hook). The map
            // update waits until the scope is back
            // (below), so no lock is held while Lua runs.
            let started = Instant::now();
            let outcome = self
                .budget
                .run(limit * count, || runner.call::<Value>(args));
            elapsed.set(Some(started.elapsed()));
            match outcome? {
                Value::String(message) => Err(mlua::Error::runtime(message.to_string_lossy())),
                _ => Ok(()),
            }
        });
        if let Some(taken) = elapsed.take() {
            let overrun = matches!(&result, Err(e) if is_overrun(e));
            crate::profiler::note(&self.id, self.side.as_str(), hook, taken, overrun);
        }
        // The runner restores the slot itself when it gets that far; one
        // stopped by the budget (raised at every safepoint past the
        // deadline) may not, so it is restored here in any case.
        slot.set(1, outer.0)?;
        slot.set(2, outer.1)?;
        slot.set(3, outer.2)?;
        slot.set(4, outer_draw)?;
        // What the handlers queued, in order; taken out whether or not
        // they all ran, and handed on only when they did.
        let mut out = Vec::new();
        for i in start + 1..=queue.raw_len() {
            let entry: Table = queue.raw_get(i)?;
            queue.raw_set(i, Value::Nil)?;
            if result.is_ok() {
                out.push((entry.get::<String>("action")?, entry.get::<Table>("args")?));
            }
        }
        // The saved tables the handlers were handed are written back when
        // they all ran, as the actions are taken then: a handler's error
        // drops both, so a cancel and the edit it went with are not split.
        if result.is_ok()
            && let Reader::Host(host) = &mut *reader.borrow_mut()
        {
            for f in fetched.borrow().iter() {
                self.store_fetched(*host, hook, f);
            }
        }
        result?;
        Ok(out)
    }

    pub(crate) fn run_action(
        &self,
        host: &mut dyn Host,
        action: &str,
        args: &Table,
    ) -> mlua::Result<()> {
        match action {
            "broadcast" => {
                let line: String = args.get(1)?;
                host.broadcast(horizon_client::mod_api::Text::literal(line));
            }
            "summon" => {
                let (dimension, kind): (String, String) = (args.get(1)?, args.get(2)?);
                let pos = [
                    args.get::<f64>(3)?,
                    args.get::<f64>(4)?,
                    args.get::<f64>(5)?,
                ];
                let id = host.summon(&dimension, &kind, pos, Default::default());
                log::debug!("[lua:{}] summon {kind} -> {id:?}", self.id);
            }
            "kill_mob" => {
                let dimension: String = args.get(1)?;
                let id: u32 = args.get(2)?;
                host.kill_mob(&dimension, id);
            }
            "tell" | "action_bar" | "title" | "subtitle" => {
                let (who, text): (Value, String) = (args.get(1)?, args.get(2)?);
                if let Some(uuid) = uuid_of(host, &who) {
                    match action {
                        "tell" => {
                            host.tell(uuid, horizon_client::mod_api::Text::literal(text));
                        }
                        "action_bar" => {
                            host.title(uuid, "actionbar", &text);
                        }
                        other => {
                            host.title(uuid, other, &text);
                        }
                    }
                }
            }
            "title_times" => {
                let (who, fade_in, stay, fade_out): (Value, i32, i32, i32) =
                    (args.get(1)?, args.get(2)?, args.get(3)?, args.get(4)?);
                if let Some(uuid) = uuid_of(host, &who) {
                    host.title_times(uuid, fade_in, stay, fade_out);
                }
            }
            "sound" => {
                let name: String = args.get(1)?;
                let pos = [
                    args.get::<f64>(2)?,
                    args.get::<f64>(3)?,
                    args.get::<f64>(4)?,
                ];
                let volume: Option<f32> = args.get(5)?;
                let pitch: Option<f32> = args.get(6)?;
                // Upstream's helper: the level in place, on the master source (0).
                host.play_sound(
                    &qualify(&name),
                    pos,
                    0,
                    volume.unwrap_or(1.0),
                    pitch.unwrap_or(1.0),
                );
            }
            "particles" => {
                let name: String = args.get(1)?;
                let pos = [
                    args.get::<f64>(2)?,
                    args.get::<f64>(3)?,
                    args.get::<f64>(4)?,
                ];
                let count: Option<u32> = args.get(5)?;
                let spread: Option<f64> = args.get(6)?;
                let speed: Option<f32> = args.get(7)?;
                let dimension = host.dimension().to_owned();
                let spread = spread.unwrap_or(0.0);
                host.particles(
                    &dimension,
                    &qualify(&name),
                    pos,
                    count.unwrap_or(1),
                    [spread; 3],
                    speed.unwrap_or(0.0),
                );
            }
            "remove_mob" => {
                let dimension: String = args.get(1)?;
                let id: u32 = args.get(2)?;
                host.remove_mob(&dimension, id);
            }
            "set_mob_health" => {
                let (dimension, id, health): (String, u32, f32) =
                    (args.get(1)?, args.get(2)?, args.get(3)?);
                host.set_mob_health(&dimension, id, health);
            }
            "take_items" | "give_items" => {
                if args.raw_len() >= 3 {
                    // To a named player: `give_items(player, item, count)`.
                    let (who, item, count): (Value, String, u32) =
                        (args.get(1)?, args.get(2)?, args.get(3)?);
                    let item = qualify(&item);
                    let Some(uuid) = uuid_of(host, &who) else {
                        log::debug!("[lua:{}] {action}: no such player", self.id);
                        return Ok(());
                    };
                    if action == "take_items" {
                        host.take_from(uuid, &item, count);
                    } else if let Some(left) = host.give_stack(
                        uuid,
                        ItemStack {
                            item: item.as_str().into(),
                            count,
                            damage: 0,
                            extra: None,
                        },
                    ) {
                        log::debug!(
                            "[lua:{}] give_items: {} of {item} did not fit",
                            self.id,
                            left.count
                        );
                    }
                    return Ok(());
                }
                let (item, count): (String, u32) = (args.get(1)?, args.get(2)?);
                if action == "take_items" {
                    host.take_items(&item, count);
                } else {
                    host.give_items(&item, count);
                }
            }
            "give_experience" | "give_levels" => {
                let (who, amount): (Value, i64) = (args.get(1)?, args.get(2)?);
                let Some(uuid) = uuid_of(host, &who) else {
                    return Ok(());
                };
                if action == "give_experience" {
                    host.give_experience(uuid, amount.clamp(0, i64::from(u32::MAX)) as u32);
                } else {
                    host.give_levels(
                        uuid,
                        amount.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                    );
                }
            }
            "set_nickname" => {
                let (who, name): (Value, Option<String>) = (args.get(1)?, args.get(2)?);
                if let Some(uuid) = uuid_of(host, &who) {
                    host.set_nickname(uuid, name.as_deref());
                }
            }
            "run_command" => {
                let line: String = args.get(1)?;
                let line = line.trim_start_matches('/');
                if !host.run_command(line) {
                    log::debug!("[lua:{}] run_command {line:?}: not run", self.id);
                }
            }
            "set_block" => {
                let (x, y, z, id, props, flags): (
                    i32,
                    i32,
                    i32,
                    String,
                    Option<Table>,
                    Option<i32>,
                ) = (
                    args.get(1)?,
                    args.get(2)?,
                    args.get(3)?,
                    args.get(4)?,
                    args.get(5)?,
                    args.get(6)?,
                );
                let name = state_name(&id, props.as_ref())?;
                let Some(state) = host.state_named(&name) else {
                    return Err(mlua::Error::runtime(format!(
                        "set_block: no block state {name:?}"
                    )));
                };
                let dimension = host.dimension().to_owned();
                host.set_block(&dimension, [x, y, z], state, flags.unwrap_or(UPDATE_ALL));
            }
            "fill" => {
                use horizon_client::mod_api::{NativeBlockEdit, NativeBlockRun};
                let (x1, y1, z1, x2, y2, z2, id, props): (
                    i32,
                    i32,
                    i32,
                    i32,
                    i32,
                    i32,
                    String,
                    Option<Table>,
                ) = (
                    args.get(1)?,
                    args.get(2)?,
                    args.get(3)?,
                    args.get(4)?,
                    args.get(5)?,
                    args.get(6)?,
                    args.get(7)?,
                    args.get(8)?,
                );
                let name = state_name(&id, props.as_ref())?;
                let (x1, x2) = (x1.min(x2), x1.max(x2));
                let (y1, y2) = (y1.min(y2), y1.max(y2));
                let (z1, z2) = (z1.min(z2), z1.max(z2));
                // Widened before the subtraction: the corners are any i32.
                // The product is u128, so a full-world span in both axes
                // reports its honest count (2^64) instead of wrapping past
                // the cap below, and is refused before any allocation.
                let span = |a: i32, b: i32| i64::from(b) - i64::from(a) + 1;
                let columns = (span(x1, x2) as u128) * (span(z1, z2) as u128);
                if columns > FILL_COLUMNS as u128 {
                    return Err(mlua::Error::runtime(format!(
                        "fill: {columns} columns, at most {FILL_COLUMNS}"
                    )));
                }
                let height = span(y1, y2);
                let Ok(height) = u32::try_from(height) else {
                    return Err(mlua::Error::runtime(format!(
                        "fill: a column of {height} blocks"
                    )));
                };
                let mut runs = Vec::with_capacity(columns as usize);
                for x in x1..=x2 {
                    for z in z1..=z2 {
                        runs.push(NativeBlockRun {
                            x,
                            z,
                            y: y1,
                            height,
                            surface: None,
                            block: 0,
                            only: None,
                        });
                    }
                }
                let edit = NativeBlockEdit {
                    palette: vec![name],
                    filters: Vec::new(),
                    runs,
                };
                let dimension = host.dimension().to_owned();
                if let Err(e) = host.edit_blocks(&dimension, &edit) {
                    return Err(mlua::Error::runtime(format!("fill: {e}")));
                }
            }
            "falling_block" => {
                let (x, y, z, id): (f64, f64, f64, String) =
                    (args.get(1)?, args.get(2)?, args.get(3)?, args.get(4)?);
                let velocity = [
                    args.get::<Option<f64>>(5)?.unwrap_or(0.0),
                    args.get::<Option<f64>>(6)?.unwrap_or(0.0),
                    args.get::<Option<f64>>(7)?.unwrap_or(0.0),
                ];
                let name = state_name(&id, None)?;
                let Some(state) = host.state_named(&name) else {
                    return Err(mlua::Error::runtime(format!(
                        "falling_block: no block state {name:?}"
                    )));
                };
                let dimension = host.dimension().to_owned();
                host.spawn_falling_block(
                    &dimension,
                    state,
                    [x, y, z],
                    velocity,
                    Default::default(),
                );
            }
            "drop_item" => {
                let (x, y, z, id, count): (f64, f64, f64, String, Option<u32>) = (
                    args.get(1)?,
                    args.get(2)?,
                    args.get(3)?,
                    args.get(4)?,
                    args.get(5)?,
                );
                // As `/summon minecraft:item ~ ~ ~ {Item: {id, count}}`: an item
                // entity made with no stack is gone at once.
                use horizon_world::nbt::Tag;
                let mut item = Compound::default();
                item.insert("id".into(), Tag::String(qualify(&id).as_str().into()));
                item.insert("count".into(), Tag::Int(count.unwrap_or(1).max(1) as i32));
                let mut nbt = Compound::default();
                nbt.insert("Item".into(), Tag::Compound(item));
                let dimension = host.dimension().to_owned();
                if host
                    .summon(&dimension, "minecraft:item", [x, y, z], nbt)
                    .is_none()
                {
                    log::debug!("[lua:{}] drop_item: no item entity made", self.id);
                }
            }
            "effect" => {
                let (target, name, ticks, amplifier): (Value, String, i32, Option<i32>) =
                    (args.get(1)?, args.get(2)?, args.get(3)?, args.get(4)?);
                let name = qualify(&name);
                let amplifier = amplifier.unwrap_or(0);
                match target {
                    Value::Integer(id) => {
                        let dimension = host.dimension().to_owned();
                        host.mob_effect(&dimension, id as u32, &name, ticks, amplifier);
                    }
                    Value::Number(id) => {
                        let dimension = host.dimension().to_owned();
                        host.mob_effect(&dimension, id as u32, &name, ticks, amplifier);
                    }
                    who => {
                        if let Some(uuid) = uuid_of(host, &who) {
                            host.player_effect(uuid, &name, ticks, amplifier);
                        }
                    }
                }
            }
            "push_mob" | "teleport_mob" => {
                let (dimension, id): (String, u32) = (args.get(1)?, args.get(2)?);
                let v = [
                    args.get::<f64>(3)?,
                    args.get::<f64>(4)?,
                    args.get::<f64>(5)?,
                ];
                if action == "push_mob" {
                    host.push_mob(&dimension, id, v);
                } else {
                    host.teleport_mob(&dimension, id, v);
                }
            }
            "net_send_to_player" | "send_custom_payload_to_player" => {
                let (who, channel, data): (Value, String, Value) =
                    (args.get(1)?, args.get(2)?, args.get(3)?);
                // A Lua string's bytes, NUL and all: what the other side's
                // `event.data` reads, byte for byte.
                let Value::String(data) = &data else {
                    return Err(mlua::Error::runtime(format!(
                        "{action}: data is a string of bytes"
                    )));
                };
                let Some(uuid) = uuid_of(host, &who) else {
                    log::debug!("[lua:{}] {action}: no such player", self.id);
                    return Ok(());
                };
                let bytes = data.as_bytes().to_vec();
                let r = if action == "net_send_to_player" {
                    host.net_send_to_player(uuid, &channel, &bytes)
                } else {
                    host.send_custom_payload_to_player(uuid, &channel, &bytes)
                };
                // A refused send is ordinary (past the cap, a bad channel):
                // report it and carry on, as `run_command` does, so later
                // queued actions still run.
                if let Err(e) = r {
                    self.error(format!("{action}: {e}"));
                }
            }
            "net_send_to_server" => {
                let (channel, data): (String, Value) = (args.get(1)?, args.get(2)?);
                let Value::String(data) = &data else {
                    return Err(mlua::Error::runtime(format!(
                        "{action}: data is a string of bytes"
                    )));
                };
                let bytes = data.as_bytes().to_vec();
                // As above: a refusal is reported, not fatal to the queue.
                if let Err(e) = host.net_send_to_server(&channel, &bytes) {
                    self.error(format!("{action}: {e}"));
                }
            }
            "open_menu" => {
                let (who, rows, title, items): (Value, i64, String, Table) =
                    { (args.get(1)?, args.get(2)?, args.get(3)?, args.get(4)?) };
                let Some(uuid) = uuid_of(host, &who) else {
                    log::debug!("[lua:{}] {action}: no such player", self.id);
                    return Ok(());
                };
                let items: Vec<Option<ItemStack>> =
                    crate::generated_runtime::struct_vec_from_lua(&self.lua, &items, "items")?;
                let _ = host.open_menu(
                    uuid,
                    u8::try_from(rows).unwrap_or(if rows < 0 { 0 } else { u8::MAX }),
                    horizon_client::mod_api::Text::literal(title),
                    items,
                );
            }
            "open_menu_with_input" => {
                let (who, rows, title, items, input): (Value, i64, String, Table, Table) = (
                    args.get(1)?,
                    args.get(2)?,
                    args.get(3)?,
                    args.get(4)?,
                    args.get(5)?,
                );
                let Some(uuid) = uuid_of(host, &who) else {
                    log::debug!("[lua:{}] {action}: no such player", self.id);
                    return Ok(());
                };
                let start: usize = match input.get::<Value>("start")? {
                    Value::Integer(i) => usize::try_from(i).map_err(|_| {
                        mlua::Error::runtime(format!("{action}: input.start is a slot from 0"))
                    })?,
                    Value::Number(n) if n.fract() == 0.0 && n >= 0.0 => n as usize,
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "{action}: input.start is a number, not {}",
                            other.type_name()
                        )));
                    }
                };
                let end: usize = match input.get::<Value>("end")? {
                    Value::Integer(i) => usize::try_from(i).map_err(|_| {
                        mlua::Error::runtime(format!("{action}: input.end is a slot from 0"))
                    })?,
                    Value::Number(n) if n.fract() == 0.0 && n >= 0.0 => n as usize,
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "{action}: input.end is a number, not {}",
                            other.type_name()
                        )));
                    }
                };
                let items: Vec<Option<ItemStack>> =
                    crate::generated_runtime::struct_vec_from_lua(&self.lua, &items, "items")?;
                let _ = host.open_menu_with_input(
                    uuid,
                    u8::try_from(rows).unwrap_or(if rows < 0 { 0 } else { u8::MAX }),
                    horizon_client::mod_api::Text::literal(title),
                    items,
                    start..end,
                );
            }
            "set_menu_items" => {
                let (who, items): (Value, Table) = (args.get(1)?, args.get(2)?);
                let Some(uuid) = uuid_of(host, &who) else {
                    return Ok(());
                };
                let items: Vec<Option<ItemStack>> =
                    crate::generated_runtime::struct_vec_from_lua(&self.lua, &items, "items")?;
                host.set_menu_items(uuid, items);
            }
            "close_menu" => {
                let who: Value = args.get(1)?;
                if let Some(uuid) = uuid_of(host, &who) {
                    host.close_menu(uuid);
                }
            }
            other => self.warn(format!("unknown action {other}")),
        }
        Ok(())
    }

    /// A command handler's queued action: `horizon.feedback(text)` answers
    /// the player whose command it is, as vanilla commands' feedback is;
    /// every other action runs as usual. The generated `perform_command`
    /// arm calls this (hand-written, so the hooks-doc helper test sees the
    /// `host.feedback` call the macro bodies hide).
    fn apply_command_action(
        &self,
        host: &mut dyn Host,
        action: &str,
        args: &Table,
    ) -> mlua::Result<()> {
        match action {
            "feedback" => {
                let text: String = args.get(1)?;
                host.feedback(horizon_client::mod_api::Text::literal(text));
            }
            other => self.run_action(host, other, args)?,
        }
        Ok(())
    }
}

/// `Level.setBlock`'s flag a crop grows with: clients see it, no neighbour update.
const UPDATE_CLIENTS: i32 = 2;
/// `Level.setBlock`'s flags a placed block gets: neighbours told, clients see it.
const UPDATE_ALL: i32 = 3;
/// The most columns one `horizon.fill` makes: 256 by 256 blocks.
const FILL_COLUMNS: i64 = 65536;

pub(crate) fn dimension_id(dimension: LevelDimension) -> &'static str {
    match dimension {
        LevelDimension::Overworld => "minecraft:overworld",
        LevelDimension::Nether => "minecraft:the_nether",
        LevelDimension::End => "minecraft:the_end",
    }
}

impl Hooks for LuaMod {
    fn mod_side(&self) -> Option<ModSide> {
        match self.side {
            LuaSide::Server => Some(ModSide::Server),
            LuaSide::Client => Some(ModSide::Client),
        }
    }

    // Every hook is generated (`generated_hooks`, one method per native
    // hook), each with one event table.
    impl_generated_hooks!();
}

/// A handler stopped by the interrupt, as opposed to one that failed on its
/// own: the budget's message (`Budget::run`).
pub(crate) fn is_overrun(e: &mlua::Error) -> bool {
    e.to_string().contains("ran over its time budget")
}

/// A handler's error for the log, the console and the status row: the hook
/// first, then the error's first line (which names the Lua file and line),
/// then the traceback indented, capped to the first 20 frames with a
/// `... N more` line. mlua decorates the message with its kind
/// (`runtime error: ...`), which stays, so existing matches keep working.
pub(crate) fn handler_error_text(hook: &str, e: &mlua::Error) -> String {
    let text = e.to_string();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").trim_end();
    let mut out = format!("{hook}: {first}");
    // Every non-empty line after the first, in order: the traceback body.
    // Its `stack traceback:` header is shown but not counted, so the cap is
    // on frames.
    let body: Vec<&str> = lines
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    const MAX_FRAMES: usize = 20;
    let is_header = |line: &str| line.trim() == "stack traceback:";
    let frames = body.iter().filter(|line| !is_header(line)).count();
    let mut shown = 0;
    for line in &body {
        if is_header(line) {
            out.push_str("\n  ");
            out.push_str(line.trim());
        } else if shown < MAX_FRAMES {
            out.push_str("\n  ");
            out.push_str(line);
            shown += 1;
        }
    }
    if frames > MAX_FRAMES {
        out.push_str(&format!("\n  ... {} more", frames - MAX_FRAMES));
    }
    out
}

/// The table at `key` in `table`, made when there is none.
fn table_in(lua: &Lua, table: &Table, key: &str) -> mlua::Result<Table> {
    match table.get::<Option<Table>>(key)? {
        Some(t) => Ok(t),
        None => {
            let t = lua.create_table()?;
            table.set(key, &t)?;
            Ok(t)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The engine's `#[non_exhaustive]` events are built by their defining
    // crate's test-support constructors (horizon-client's `test_events`).
    use horizon_client::mod_api::{BlockPos, ServerTickPhase};
    use horizon_client::test_events;

    #[test]
    fn the_hook_surface_check_names_both_versions_on_a_mismatch() {
        use crate::generated_hooks::LUA_HOOK_SURFACE_VERSION;
        let base = "id = \"m\"\nname = \"M\"\nversion = \"1\"\napi = \"0.1\"\n";
        let absent = horizon_mods::Manifest::parse(base).unwrap();
        assert_eq!(LuaMod::check_hook_surface(&absent), Ok(()));
        let equal = horizon_mods::Manifest::parse(&format!(
            "{base}lua_hook_surface = {LUA_HOOK_SURFACE_VERSION}\n"
        ))
        .unwrap();
        assert_eq!(LuaMod::check_hook_surface(&equal), Ok(()));
        for pinned in [
            LUA_HOOK_SURFACE_VERSION.wrapping_add(1),
            LUA_HOOK_SURFACE_VERSION.wrapping_sub(1),
        ] {
            let stale =
                horizon_mods::Manifest::parse(&format!("{base}lua_hook_surface = {pinned}\n"))
                    .unwrap();
            assert_eq!(
                LuaMod::check_hook_surface(&stale),
                Err(format!(
                    "mod m was built for Lua hook surface {pinned}, this engine has {LUA_HOOK_SURFACE_VERSION}"
                )),
                "a higher and a lower surface both fail"
            );
        }
    }

    const MANIFEST_TEXT: &str = r#"
id = "ruby"
name = "Ruby"
version = "0.1.0"
api = "0.2"
side = "both"
capabilities = ["scripts"]
experimental = ["scripts"]
"#;

    fn hmod(name: &str, files: &[(&str, &[u8])]) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("horizon-lua-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ruby.hmod");
        std::fs::write(
            &path,
            Package::write_files(MANIFEST_TEXT, None, files).unwrap(),
        )
        .unwrap();
        (dir, path)
    }

    #[test]
    fn a_scripted_hmod_loads_and_its_scripts_see_each_other() {
        let (dir, path) = hmod(
            "ok",
            &[
                (
                    "scripts/server/init.lua",
                    b"local n = require('util')\nassert(n == 42, 'util')\nassert(require('util') == 42)\nhorizon.on('server_tick', function() end)\nhorizon.log('ready ' .. horizon.mod_id)",
                ),
                ("scripts/server/util.lua", b"return 42"),
            ],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        assert_eq!(m.id(), "ruby");
        let handlers: Table = m.lua.named_registry_value(HANDLERS).unwrap();
        let ticks: Table = handlers.get("server_tick").unwrap();
        // The timers' own handler comes first.
        assert_eq!(ticks.len().unwrap(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_script_defines_goals_and_attaches_them_from_mob_ai_register() {
        let (dir, path) = hmod(
            "goals",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.goal("stalk", {
                  flags = {"move", "look"},
                  can_use = function(mob, state, out) return mob.nearest_player ~= nil end,
                  tick = function(mob, state, out) out.look_at(0, 0, 0) end,
                })
                horizon.on("mob_ai_register", function(event)
                  local mob = event.mob
                  if mob.kind == "minecraft:cow" then
                    horizon.clear_goals()
                    horizon.add_goal(2, "stalk")
                    horizon.add_target_goal(1, "missing")
                  end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // `load` leaves a status row behind, so a later `error`-like lookup
        // sees this mod; the handler then runs against a real mob. The
        // defined goal attaches at its priority, and the missing goal is the
        // handler's error.
        set_status(ModStatus {
            id: m.id().into(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        let mut mob = horizon_client::mobs::Mob::new(
            1,
            horizon_client::mobs::MobType::cow_for_tests(),
            horizon_core::geometry::Vec3::ZERO,
            0.0,
        );
        m.mob_ai_register(&mut test_events::mob_ai_register_event(&mut mob));
        let goals: Vec<(i32, String, bool)> = mob
            .ai
            .goals
            .entries()
            .map(|(p, name, running, _)| (p, name.to_owned(), running))
            .collect();
        assert_eq!(goals, [(2, "ruby:stalk".to_owned(), false)]);
        assert!(mob.ai.targets.entries().next().is_none());
        let def = m.goals.lock().unwrap().get("stalk").cloned().unwrap();
        assert_eq!(
            def.flags,
            horizon_client::mod_api::GOAL_MOVE | horizon_client::mod_api::GOAL_LOOK
        );
        let e = horizon_client::dev_input::mods::status()
            .into_iter()
            .find(|s| s.id == m.id())
            .and_then(|s| s.error)
            .expect("the missing goal is recorded");
        assert!(e.contains("no such goal"), "{e}");
        assert!(e.contains("missing"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_add_goal_calls_without_a_clear_keep_both_goals_in_order() {
        let (dir, path) = hmod(
            "goals-two",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.goal("alpha", { can_use = function() return false end })
                horizon.goal("beta", { can_use = function() return false end })
                horizon.on("mob_ai_register", function(event)
                  local mob = event.mob
                  if mob.kind == "minecraft:cow" then
                    horizon.add_goal(2, "alpha")
                    horizon.add_goal(1, "beta")
                  end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        set_status(ModStatus {
            id: m.id().into(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        let mut mob = horizon_client::mobs::Mob::new(
            1,
            horizon_client::mobs::MobType::cow_for_tests(),
            horizon_core::geometry::Vec3::ZERO,
            0.0,
        );
        m.mob_ai_register(&mut test_events::mob_ai_register_event(&mut mob));
        // Both adds append after the species' own goals, in call order: an
        // arm that replaced the list would leave only the second goal.
        let names: Vec<String> = mob
            .ai
            .goals
            .entries()
            .map(|(_, name, _, _)| name.to_owned())
            .collect();
        let added: Vec<&str> = names
            .iter()
            .filter(|n| *n == "ruby:alpha" || *n == "ruby:beta")
            .map(|n| n.as_str())
            .collect();
        assert_eq!(added, ["ruby:alpha", "ruby:beta"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_script_sets_a_mobs_body_rotation_from_mob_ai_register() {
        let (dir, path) = hmod(
            "rotation",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("mob_ai_register", function(event)
                  local mob = event.mob
                  if mob.kind == "minecraft:cow" then
                    horizon.set_body_rotation("smooth", 7)
                  end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut mob = horizon_client::mobs::Mob::new(
            1,
            horizon_client::mobs::MobType::cow_for_tests(),
            horizon_core::geometry::Vec3::ZERO,
            0.0,
        );
        assert_eq!(
            mob.ai.body.mode,
            horizon_client::mobs::control::BodyRotationMode::Vanilla
        );
        m.mob_ai_register(&mut test_events::mob_ai_register_event(&mut mob));
        assert_eq!(
            mob.ai.body.mode,
            horizon_client::mobs::control::BodyRotationMode::Smooth {
                degrees_per_tick: 7.0
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unknown_body_rotation_mode_is_the_handlers_error() {
        let (dir, path) = hmod(
            "rotation-bad",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("mob_ai_register", function(event)
                  horizon.set_body_rotation("swivel")
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // `load` leaves a status row behind, so a later `error`-like lookup
        // sees this mod; the handler then runs against a real mob. The
        // game's own unknown-mode error must name the accepted modes, and
        // the mob stays vanilla.
        set_status(ModStatus {
            id: m.id().into(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        let mut mob = horizon_client::mobs::Mob::new(
            1,
            horizon_client::mobs::MobType::cow_for_tests(),
            horizon_core::geometry::Vec3::ZERO,
            0.0,
        );
        m.mob_ai_register(&mut test_events::mob_ai_register_event(&mut mob));
        assert_eq!(
            mob.ai.body.mode,
            horizon_client::mobs::control::BodyRotationMode::Vanilla
        );
        let e = horizon_client::dev_input::mods::status()
            .into_iter()
            .find(|s| s.id == m.id())
            .and_then(|s| s.error)
            .expect("the failed set_body_rotation is recorded");
        assert!(e.contains("no such body rotation"), "{e}");
        assert!(e.contains("vanilla"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The hook's recorded error for a mod that just failed.
    fn hook_error_of(m: &LuaMod) -> String {
        horizon_client::dev_input::mods::status()
            .into_iter()
            .find(|s| s.id == m.id())
            .and_then(|s| s.error)
            .expect("the dead action call is the hook's error")
    }

    /// An hmod with its own manifest id (the status store is keyed by mod
    /// id, and every `hmod` is `ruby`): loads it, leaves a status row,
    /// and hands back a host with the wand.
    fn dead_mod(name: &str, script: &[u8]) -> (LuaMod, MobHost, PathBuf) {
        let dir = std::env::temp_dir().join(format!("horizon-lua-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.hmod"));
        let manifest = MANIFEST_TEXT.replacen("id = \"ruby\"", &format!("id = \"{name}\""), 1);
        std::fs::write(
            &path,
            Package::write_files(&manifest, None, &[("scripts/server/init.lua", script)]).unwrap(),
        )
        .unwrap();
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        set_status(ModStatus {
            id: m.id().into(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        let host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        (m, host, dir)
    }

    #[test]
    fn dead_host_actions_error_and_leave_their_hooks_working() {
        let (m, mut host, dir) = dead_mod(
            "dead_host_actions",
            br#"
            horizon.on("inventory_tick", function(event)
              __witness = (__witness or 0) + 1
              horizon.broadcast("tick " .. event.slot)
              if CALL_DEAD then horizon.set_count(3) end
            end)
            horizon.on("living_entity_use_item_finish", function(event)
              if event.stack.id ~= "ruby:wand" then return end
              if CALL_DEAD then horizon.set_result("minecraft:stick", 2) end
              event.result = { id = "minecraft:stick", count = 2 }
            end)
            horizon.on("living_entity_use_item_start", function(event)
              if event.stack.id ~= "ruby:wand" then return end
              if CALL_DEAD then horizon.set_duration(10) end
              event.duration = 10
            end)
            "#,
        );
        let wand = wand();
        let start = || {
            test_events::living_entity_use_item_start_event(
                &wand,
                None,
                horizon_client::mod_api::Hand::Off,
                32,
            )
        };
        // The live paths first: the broadcast runs, the writes land.
        m.lua.globals().set("CALL_DEAD", false).unwrap();
        m.inventory_tick(&mut host, &mut test_events::inventory_tick_event(3, true));
        assert_eq!(*host.lines.lock().unwrap(), ["tick 3"]);
        let mut result = Some(wand.clone());
        m.living_entity_use_item_finish(
            &mut host,
            &mut test_events::living_entity_use_item_finish_event(
                &wand,
                None,
                horizon_client::mod_api::Hand::Main,
                0,
                &mut result,
            ),
        );
        assert_eq!(
            result.as_ref().map(|s| (&*s.item, s.count)),
            Some(("minecraft:stick", 2))
        );
        let mut use_start = start();
        assert_eq!(
            m.living_entity_use_item_start(&mut host, &mut use_start),
            Flow::Continue
        );
        assert_eq!(use_start.duration, 10);
        // Each dead call fails its handler through the hook's error path,
        // naming the replacement; the queued broadcast is dropped with it.
        m.lua.globals().set("CALL_DEAD", true).unwrap();
        m.inventory_tick(&mut host, &mut test_events::inventory_tick_event(4, false));
        let e = hook_error_of(&m);
        assert!(e.contains("horizon.set_count does nothing"), "{e}");
        assert!(e.contains("inventory_tick has no stack"), "{e}");
        let mut result = Some(wand.clone());
        m.living_entity_use_item_finish(
            &mut host,
            &mut test_events::living_entity_use_item_finish_event(
                &wand,
                None,
                horizon_client::mod_api::Hand::Main,
                0,
                &mut result,
            ),
        );
        let e = hook_error_of(&m);
        assert!(e.contains("horizon.set_result does nothing"), "{e}");
        assert!(e.contains("event.result"), "{e}");
        assert_eq!(
            result.as_ref().map(|s| (&*s.item, s.count)),
            Some(("ruby:wand", 3))
        );
        let mut use_start = start();
        assert_eq!(
            m.living_entity_use_item_start(&mut host, &mut use_start),
            Flow::Continue
        );
        let e = hook_error_of(&m);
        assert!(e.contains("horizon.set_duration does nothing"), "{e}");
        assert!(e.contains("event.duration"), "{e}");
        assert_eq!(use_start.duration, 32);
        // After the errors the hooks still tick: the broadcast runs again.
        m.lua.globals().set("CALL_DEAD", false).unwrap();
        m.inventory_tick(&mut host, &mut test_events::inventory_tick_event(5, false));
        assert_eq!(*host.lines.lock().unwrap(), ["tick 3", "tick 5"]);
        assert_eq!(m.lua.globals().get::<f64>("__witness").unwrap(), 3.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dead_hostless_actions_error_and_leave_their_hooks_working() {
        let (m, _host, dir) = dead_mod(
            "dead_hostless_actions",
            br#"
            horizon.on("living_fall", function(event)
              if MODE == "distance" then horizon.set_distance(1) end
              if MODE == "multiplier" then horizon.set_multiplier(1) end
              event.distance = 1.5
            end, "minecraft:cow")
            horizon.on("living_knockback", function(event)
              if MODE == "strength" then horizon.set_strength(1) end
              event.strength = 0.25
            end, "minecraft:cow")
            "#,
        );
        let knock = || {
            test_events::living_knockback_event(
                7,
                "minecraft:cow",
                "minecraft:player_attack",
                5,
                5,
                0.4,
                1.0,
                0.0,
            )
        };
        // The live paths first: both writes land.
        m.lua.globals().set("MODE", "live").unwrap();
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        assert_eq!(m.living_fall(&mut fall), Flow::Continue);
        assert_eq!(fall.distance, 1.5);
        let mut hit = knock();
        assert_eq!(m.living_knockback(&mut hit), Flow::Continue);
        assert_eq!(hit.strength, 0.25);
        // Each dead call fails its handler before the write, naming its
        // field, while the other hook still writes back.
        for (mode, call, field) in [
            (
                "distance",
                "horizon.set_distance does nothing",
                "event.distance",
            ),
            (
                "multiplier",
                "horizon.set_multiplier does nothing",
                "event.multiplier",
            ),
            (
                "strength",
                "horizon.set_strength does nothing",
                "event.strength",
            ),
        ] {
            m.lua.globals().set("MODE", mode).unwrap();
            let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
            assert_eq!(m.living_fall(&mut fall), Flow::Continue);
            let mut hit = knock();
            assert_eq!(m.living_knockback(&mut hit), Flow::Continue);
            let e = hook_error_of(&m);
            assert!(e.contains(call), "{e}");
            assert!(e.contains(field), "{e}");
            if mode == "strength" {
                assert_eq!(hit.strength, 0.4);
                assert_eq!(fall.distance, 1.5);
            } else {
                assert_eq!(fall.distance, 4.0);
                assert_eq!(hit.strength, 0.25);
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dead_player_actions_error_and_leave_their_hooks_working() {
        let (m, mut host, dir) = dead_mod(
            "dead_player_actions",
            br#"
            horizon.on("player_exp_change", function(event)
              __witness = (__witness or 0) + 1
              if CALL_DEAD then horizon.set_amount(10) end
              event.amount = event.amount * 2
            end)
            horizon.on("player_chat", function(event)
              __witness = (__witness or 0) + 1
              if CALL_DEAD then horizon.reply("sneaky") end
              event.replies = { "heard " .. event.message }
            end)
            horizon.on("player_command", function(event)
              __witness = (__witness or 0) + 1
              if CALL_DEAD then horizon.reply("sneaky") end
              if event.command == "shh" then horizon.cancel() end
              horizon.broadcast("ran " .. event.command)
            end)
            "#,
        );
        let steve = 0x123e4567_e89b_12d3_a456_426614174000;
        // The live paths first: the write lands, the line reaches the
        // sender, the cancel stops the command.
        m.lua.globals().set("CALL_DEAD", false).unwrap();
        let mut exp = test_events::player_exp_change_event(steve, "steve", 3);
        m.player_exp_change(&mut host, &mut exp);
        assert_eq!(exp.amount, 6);
        let mut replies = Vec::new();
        assert_eq!(
            m.player_chat(&mut test_events::player_chat_event(
                steve,
                "steve",
                "hello",
                &mut replies
            )),
            Flow::Continue
        );
        assert_eq!(
            replies,
            [horizon_client::mod_api::Text::literal("heard hello")]
        );
        let mut replies = Vec::new();
        let mut cmd = test_events::player_command_event(steve, "steve", "shh", &mut replies);
        assert_eq!(m.player_command(&mut host, &mut cmd), Flow::Cancel);
        assert!(replies.is_empty());
        // Each dead call fails its handler through the hook's error path,
        // naming the event field; the write, the reply and the cancel are
        // dropped with it.
        m.lua.globals().set("CALL_DEAD", true).unwrap();
        let mut exp = test_events::player_exp_change_event(steve, "steve", 3);
        m.player_exp_change(&mut host, &mut exp);
        let e = hook_error_of(&m);
        assert!(e.contains("horizon.set_amount does nothing"), "{e}");
        assert!(e.contains("event.amount"), "{e}");
        assert!(e.contains("player_exp_change"), "{e}");
        assert!(e.contains("living_heal"), "{e}");
        assert!(e.contains("living_incoming_damage"), "{e}");
        assert!(e.contains("item_damaged"), "{e}");
        assert_eq!(exp.amount, 3);
        let mut replies = Vec::new();
        assert_eq!(
            m.player_chat(&mut test_events::player_chat_event(
                steve,
                "steve",
                "hello",
                &mut replies
            )),
            Flow::Continue
        );
        let e = hook_error_of(&m);
        assert!(e.contains("horizon.reply does nothing"), "{e}");
        assert!(e.contains("event.replies"), "{e}");
        assert!(e.contains("player_chat"), "{e}");
        assert!(replies.is_empty());
        let mut replies = Vec::new();
        let mut cmd = test_events::player_command_event(steve, "steve", "shh", &mut replies);
        assert_eq!(m.player_command(&mut host, &mut cmd), Flow::Continue);
        let e = hook_error_of(&m);
        assert!(e.contains("horizon.reply does nothing"), "{e}");
        assert!(e.contains("event.replies"), "{e}");
        assert!(e.contains("player_command"), "{e}");
        assert_eq!(*host.lines.lock().unwrap(), ["ran shh"]);
        // After the errors the hooks still run: the write lands again.
        m.lua.globals().set("CALL_DEAD", false).unwrap();
        let mut exp = test_events::player_exp_change_event(steve, "steve", 4);
        m.player_exp_change(&mut host, &mut exp);
        assert_eq!(exp.amount, 8);
        assert_eq!(m.lua.globals().get::<f64>("__witness").unwrap(), 7.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A level of one block whose writes and scheduled ticks are recorded.
    struct FakeLevel {
        block: &'static str,
        props: Vec<(String, String)>,
        writes: Vec<([i32; 3], String, String, i32)>,
        ticks: Vec<([i32; 3], String, i32)>,
        summons: Vec<(String, [f64; 3])>,
        /// Mod block entities' data by position.
        data: HashMap<[i32; 3], Compound>,
    }
    impl horizon_client::mod_api::LevelHost for FakeLevel {
        fn block_state(&self, _: [i32; 3]) -> BlockStateId {
            BlockStateId(7)
        }
        fn is_client_side(&self) -> bool {
            false
        }
        fn state_with_property(&self, _: BlockStateId, _: &str, _: &str) -> Option<BlockStateId> {
            None
        }
        fn mod_block_entity_data(&self, pos: [i32; 3]) -> Option<Compound> {
            self.data.get(&pos).cloned()
        }
        fn state_block(&self, state: BlockStateId) -> Option<String> {
            (state == BlockStateId(7)).then(|| self.block.to_owned())
        }
        fn state_properties(&self, _: BlockStateId) -> Vec<(String, String)> {
            self.props.clone()
        }
    }
    impl horizon_client::mod_api::LevelRandomHost for FakeLevel {
        fn attachments(
            &mut self,
            _: horizon_client::mod_api::Holder,
        ) -> Result<
            &mut horizon_client::mod_api::Attachments,
            horizon_client::mod_api::AttachmentError,
        > {
            Err(horizon_client::mod_api::AttachmentError::MissingHolder)
        }
        fn random(&mut self) -> Option<&mut horizon_client::mod_api::LegacyRandom> {
            None
        }
    }
    impl horizon_client::mod_api::LevelTicksHost for FakeLevel {
        fn schedule_block_tick(&mut self, pos: [i32; 3], block: &str, delay: i32) -> bool {
            self.ticks.push((pos, block.to_owned(), delay));
            true
        }
        fn schedule_fluid_tick(&mut self, _: [i32; 3], _: &str, _: i32) -> bool {
            false
        }
    }
    impl LevelWriteHost for FakeLevel {
        fn set_block_property(
            &mut self,
            pos: [i32; 3],
            key: &str,
            value: &str,
            flags: i32,
        ) -> bool {
            self.writes
                .push((pos, key.to_owned(), value.to_owned(), flags));
            true
        }
        fn summon(
            &mut self,
            kind: &str,
            pos: [f64; 3],
            _: horizon_client::mod_api::Compound,
        ) -> bool {
            self.summons.push((kind.to_owned(), pos));
            true
        }
        fn set_mod_block_entity_data(&mut self, pos: [i32; 3], data: Compound) -> bool {
            self.data.insert(pos, data);
            true
        }
        fn play_sound(&mut self, _: &str, _: [f64; 3], _: usize, _: f32, _: f32) -> bool {
            false
        }
        fn send_particles(
            &mut self,
            _: &horizon_client::mod_api::ParticleOptions,
            _: Option<BlockStateId>,
            _: [f64; 3],
            _: [f64; 3],
            _: f64,
            _: i32,
        ) -> bool {
            false
        }
    }

    #[test]
    fn a_random_tick_handler_grows_a_block_and_takes_the_tick() {
        let (dir, path) = hmod(
            "ticks",
            &[(
                "scripts/server/init.lua",
                br#"
                LAMPS = 0
                TICKS = 0
                horizon.on("random_tick", function() LAMPS = LAMPS + 1 end, "ruby:lamp")
                horizon.on("random_tick", function(event)
                  TICKS = TICKS + 1
                  if event.state ~= "ruby:hive" then return end
                  if event.state_props.stage == "dormant" then
                    horizon.set_block_property("stage", "growing")
                    horizon.set_block_property("lit", "true", 1)
                    horizon.schedule_tick(40)
                    horizon.handled()
                  elseif event.state_props.stage == "mature" then
                    horizon.summon(event.dimension, "ruby:crawler", event.pos.x + 0.5, event.pos.y + 1, event.pos.z + 0.5)
                  end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut level = FakeLevel {
            block: "ruby:hive",
            props: vec![("stage".into(), "dormant".into())],
            writes: Vec::new(),
            ticks: Vec::new(),
            summons: Vec::new(),
            data: HashMap::new(),
        };
        let mut event = test_events::random_tick_event(
            LevelDimension::Nether,
            BlockPos::new(1, 64, -3),
            BlockStateId(7),
        );
        assert!(matches!(
            m.random_tick(&mut level, &mut event),
            Flow::Cancel
        ));
        // The default flags tell the clients; the explicit ones ride along.
        assert_eq!(
            level.writes,
            [
                ([1, 64, -3], "stage".to_owned(), "growing".to_owned(), 2),
                ([1, 64, -3], "lit".to_owned(), "true".to_owned(), 1),
            ]
        );
        assert_eq!(level.ticks, [([1, 64, -3], "ruby:hive".to_owned(), 40)]);
        // The handler registered for the lamp never heard the hive's tick.
        assert_eq!(m.lua.globals().get::<i32>("LAMPS").unwrap(), 0);
        assert_eq!(m.lua.globals().get::<i32>("TICKS").unwrap(), 1);

        // An unknown state tells nobody, not even unfiltered handlers.
        let mut unknown = test_events::random_tick_event(
            LevelDimension::Nether,
            BlockPos::new(1, 64, -3),
            BlockStateId(9),
        );
        assert!(matches!(
            m.random_tick(&mut level, &mut unknown),
            Flow::Continue
        ));
        assert_eq!(m.lua.globals().get::<i32>("TICKS").unwrap(), 1);
        assert_eq!(level.writes.len(), 2);

        // A mature hive summons (queued on the level, made after the pass)
        // and keeps its tick; another block: nothing happens.
        level.props = vec![("stage".into(), "mature".into())];
        assert!(matches!(
            m.random_tick(&mut level, &mut event),
            Flow::Continue
        ));
        assert_eq!(
            level.summons,
            [("ruby:crawler".to_owned(), [1.5, 65.0, -2.5])]
        );
        level.block = "minecraft:grass_block";
        assert!(matches!(
            m.random_tick(
                &mut level,
                &mut test_events::random_tick_event(
                    LevelDimension::Overworld,
                    BlockPos::new(0, 0, 0),
                    BlockStateId(7)
                )
            ),
            Flow::Continue
        ));
        // The every-block handler still ran for the grass block (its body
        // returned on the block id), but queued nothing.
        assert_eq!(m.lua.globals().get::<i32>("TICKS").unwrap(), 3);
        assert_eq!(level.writes.len(), 2);
        // The sibling hook stays quiet: nothing listens to `tick_block`.
        assert!(matches!(
            m.tick_block(
                &mut level,
                &mut test_events::tick_block_event(
                    LevelDimension::Overworld,
                    BlockPos::new(0, 0, 0),
                    BlockStateId(7)
                )
            ),
            Flow::Continue
        ));
        assert_eq!(m.lua.globals().get::<i32>("TICKS").unwrap(), 3);
        assert_eq!(level.writes.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_tick_block_handler_that_errors_drops_its_write_and_data_edit() {
        use horizon_world::nbt::Tag;
        let (dir, path) = hmod(
            "ticks-error",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("tick_block", function(event)
                  event.data.charge = event.data.charge + 1
                  horizon.set_block_property("stage", "growing")
                end, "ruby:hive")
                horizon.on("tick_block", function(event)
                  error("boom")
                end, "ruby:hive")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // `load` leaves no status row behind on its own; without one the
        // error lookup below sees nothing.
        set_status(ModStatus {
            id: m.id().into(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        let key = |k: &str| horizon_world::nbt::JavaString::from(k);
        let mut data = Compound::new();
        data.insert(key("charge"), Tag::Int(0));
        let mut level = FakeLevel {
            block: "ruby:hive",
            props: Vec::new(),
            writes: Vec::new(),
            ticks: Vec::new(),
            summons: Vec::new(),
            data: HashMap::from([([1, 64, -3], data)]),
        };
        let mut event = test_events::tick_block_event(
            LevelDimension::Overworld,
            BlockPos::new(1, 64, -3),
            BlockStateId(7),
        );
        assert!(matches!(
            m.tick_block(&mut level, &mut event),
            Flow::Continue
        ));
        // The queued write never ran and the data edit never landed: the
        // int is still there, untouched.
        assert!(level.writes.is_empty());
        assert_eq!(
            level.data[&[1, 64, -3]].get(&key("charge")),
            Some(&Tag::Int(0))
        );
        let failed = horizon_client::dev_input::mods::status()
            .into_iter()
            .find(|s| s.id == m.id())
            .and_then(|s| s.error)
            .expect("the erroring handler is recorded");
        assert!(failed.contains("boom"), "{failed}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A host nothing is asked of.
    struct NoHost;
    impl Host for NoHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }
        fn attachments(
            &mut self,
            _: Holder,
        ) -> horizon_client::mod_api::AttachmentResult<&mut horizon_client::mod_api::Attachments>
        {
            Err(horizon_client::mod_api::AttachmentError::MissingHolder)
        }
        fn acting_player(&self) -> Option<horizon_client::mod_api::NativeServerPlayer> {
            None
        }
        fn held(&self) -> Option<(String, u32, bool)> {
            None
        }
        fn mob_view(&self, _: &str, _: u32) -> Option<MobView> {
            None
        }
    }

    /// A host of one living cow, number 7, that keeps its attachments and
    /// records what the mob hooks ask of it.
    struct MobHost {
        attachments: horizon_client::mod_api::Attachments,
        health: Vec<(u32, f32)>,
        lines: Mutex<Vec<String>>,
        pushes: Vec<[f64; 3]>,
        carried: ItemStack,
        world: horizon_client::mod_api::Attachments,
        player: horizon_client::mod_api::Attachments,
    }
    impl Host for MobHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }
        fn attachments(
            &mut self,
            holder: Holder,
        ) -> horizon_client::mod_api::AttachmentResult<&mut horizon_client::mod_api::Attachments>
        {
            match holder {
                Holder::Entity(7) => Ok(&mut self.attachments),
                Holder::Level => Ok(&mut self.world),
                Holder::Player(0x123e4567_e89b_12d3_a456_426614174000) => Ok(&mut self.player),
                _ => Err(horizon_client::mod_api::AttachmentError::MissingHolder),
            }
        }
        fn acting_player(&self) -> Option<horizon_client::mod_api::NativeServerPlayer> {
            Some(test_events::native_server_player(
                0x123e4567_e89b_12d3_a456_426614174000,
                "steve".into(),
                0,
                "minecraft:overworld".into(),
                [1.0, 64.0, 1.0],
                0,
                20.0,
                0.0,
                0.0,
                0,
                String::new(),
            ))
        }
        fn mob_view(&self, dimension: &str, id: u32) -> Option<MobView> {
            use horizon_core::geometry::Vec3;
            (dimension == "minecraft:overworld" && id == 7).then(|| {
                let kind = horizon_client::mobs::MobType::cow_for_tests();
                MobView::of(
                    &horizon_client::mobs::Mob::new(7, kind, Vec3::new(2.0, 64.0, 3.0), 0.0),
                    None,
                )
            })
        }
        fn set_mob_health(&mut self, _: &str, id: u32, health: f32) -> bool {
            self.health.push((id, health));
            true
        }
        fn push_mob(&mut self, _: &str, _: u32, velocity: [f64; 3]) -> bool {
            self.pushes.push(velocity);
            true
        }
        fn held(&self) -> Option<(String, u32, bool)> {
            Some(("ruby:wand".into(), 1, true))
        }
        fn held_stack(&self, uuid: u128) -> Option<ItemStack> {
            (uuid == 0x123e4567_e89b_12d3_a456_426614174000).then(|| self.carried.clone())
        }
        fn take_items(&mut self, item: &str, count: u32) -> u32 {
            self.lines
                .lock()
                .unwrap()
                .push(format!("took {count} {item}"));
            count
        }
        fn give_items(&mut self, item: &str, count: u32) -> u32 {
            self.lines
                .lock()
                .unwrap()
                .push(format!("gave {count} {item}"));
            count
        }
        fn net_send_to_player(
            &mut self,
            player: u128,
            channel: &str,
            data: &[u8],
        ) -> Result<(), String> {
            self.lines
                .lock()
                .unwrap()
                .push(format!("net_send_to_player {player} {channel} {data:?}"));
            Ok(())
        }
        fn send_custom_payload_to_player(
            &mut self,
            player: u128,
            channel: &str,
            data: &[u8],
        ) -> Result<(), String> {
            self.lines.lock().unwrap().push(format!(
                "send_custom_payload_to_player {player} {channel} {data:?}"
            ));
            Ok(())
        }
        fn net_send_to_server(&mut self, channel: &str, data: &[u8]) -> Result<(), String> {
            self.lines
                .lock()
                .unwrap()
                .push(format!("net_send_to_server {channel} {data:?}"));
            Ok(())
        }
        fn carried_stack(&mut self, slot: i32) -> Option<&mut ItemStack> {
            (slot == 3).then_some(&mut self.carried)
        }
        fn block(&self, pos: [i32; 3]) -> String {
            if pos == [1, 2, 3] {
                "minecraft:stone".into()
            } else {
                "minecraft:air".into()
            }
        }
        fn block_property(&self, pos: [i32; 3], key: &str) -> Option<String> {
            (pos == [1, 2, 3] && key == "facing").then(|| "north".to_owned())
        }
        fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
            self.acting_player().into_iter().collect()
        }
        fn day_time(&self) -> i64 {
            30000
        }
        fn weather(&self) -> (f32, f32) {
            (0.5, 0.0)
        }
        fn mob_views(&self, dimension: &str) -> Vec<MobView> {
            self.mob_view(dimension, 7).into_iter().collect()
        }
        fn play_sound(
            &mut self,
            sound: &str,
            pos: [f64; 3],
            _: usize,
            volume: f32,
            pitch: f32,
        ) -> bool {
            self.lines.lock().unwrap().push(format!(
                "sound {sound} at {},{},{} x{volume} p{pitch}",
                pos[0], pos[1], pos[2]
            ));
            true
        }
        fn particles(
            &mut self,
            _: &str,
            name: &str,
            pos: [f64; 3],
            count: u32,
            spread: [f64; 3],
            speed: f32,
        ) -> u32 {
            self.lines.lock().unwrap().push(format!(
                "particles {name} x{count} at {},{},{} spread {} speed {speed}",
                pos[0], pos[1], pos[2], spread[0]
            ));
            1
        }
        fn title(&mut self, uuid: u128, kind: &str, text: &str) -> bool {
            let who = if uuid == 0x123e4567_e89b_12d3_a456_426614174000 {
                "steve"
            } else {
                "?"
            };
            self.lines
                .lock()
                .unwrap()
                .push(format!("{kind} {who}: {text}"));
            true
        }
        fn title_times(&mut self, _: u128, fade_in: i32, stay: i32, fade_out: i32) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("times steve: {fade_in}/{stay}/{fade_out}"));
            true
        }
        fn tell(&mut self, _: u128, line: horizon_client::mod_api::Text) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("tell steve: {}", line.plain()));
            true
        }
        fn feedback(&mut self, line: horizon_client::mod_api::Text) {
            self.lines
                .lock()
                .unwrap()
                .push(format!("feedback {}", line.plain()));
        }
        fn player_raycast(
            &self,
            uuid: u128,
            distance: f32,
        ) -> Option<horizon_client::mod_api::NativeHit> {
            (uuid == 0x123e4567_e89b_12d3_a456_426614174000 && distance == 8.0).then(|| {
                horizon_client::mod_api::NativeHit {
                    block_x: 4,
                    block_y: 5,
                    block_z: 6,
                    x: 4.5,
                    y: 6.0,
                    z: 6.5,
                    block: "minecraft:dirt".into(),
                }
            })
        }
        fn broadcast(&self, text: horizon_client::mod_api::Text) {
            self.lines.lock().unwrap().push(text.plain());
        }
        fn state_named(&self, name: &str) -> Option<BlockStateId> {
            match name {
                "minecraft:stone" => Some(BlockStateId(1)),
                "ruby:hive[stage=mature]" => Some(BlockStateId(2)),
                _ => None,
            }
        }
        fn state_name(&self, state: BlockStateId) -> String {
            match state.0 {
                1 => "minecraft:stone".into(),
                2 => "ruby:hive[stage=mature]".into(),
                _ => "minecraft:air".into(),
            }
        }
        fn set_block(&mut self, _: &str, pos: [i32; 3], state: BlockStateId, flags: i32) -> bool {
            self.lines.lock().unwrap().push(format!(
                "set_block {},{},{} state {} flags {flags}",
                pos[0], pos[1], pos[2], state.0
            ));
            true
        }
        fn edit_blocks(
            &mut self,
            _: &str,
            edit: &horizon_client::mod_api::NativeBlockEdit,
        ) -> Result<u64, String> {
            let run = edit.runs[0];
            self.lines.lock().unwrap().push(format!(
                "edit {} {} runs y {} h {}",
                edit.palette[0],
                edit.runs.len(),
                run.y,
                run.height
            ));
            Ok(edit.runs.len() as u64 * u64::from(run.height))
        }
        fn run_command(&mut self, line: &str) -> bool {
            self.lines.lock().unwrap().push(format!("run {line}"));
            true
        }
        fn give_stack(&mut self, uuid: u128, stack: ItemStack) -> Option<ItemStack> {
            self.lines.lock().unwrap().push(format!(
                "give_stack {} {} {}",
                if uuid == 0x123e4567_e89b_12d3_a456_426614174000 {
                    "steve"
                } else {
                    "?"
                },
                stack.count,
                stack.item
            ));
            None
        }
        fn take_from(&mut self, uuid: u128, item: &str, count: u32) -> u32 {
            self.lines.lock().unwrap().push(format!(
                "take_from {} {count} {item}",
                if uuid == 0x123e4567_e89b_12d3_a456_426614174000 {
                    "steve"
                } else {
                    "?"
                }
            ));
            count
        }
        fn player_stacks(&self, uuid: u128) -> Option<Vec<Option<ItemStack>>> {
            (uuid == 2).then(|| {
                vec![
                    None,
                    Some(ItemStack {
                        item: "minecraft:bone".into(),
                        count: 5,
                        damage: 0,
                        extra: None,
                    }),
                ]
            })
        }
        fn light(&self, pos: [i32; 3]) -> (u8, u8) {
            if pos == [1, 2, 3] { (7, 15) } else { (0, 0) }
        }
        fn surface_heights(
            &self,
            _: &str,
            columns: &[(i32, i32)],
            heightmap: horizon_client::mod_api::NativeHeightmap,
        ) -> Result<Vec<Option<i32>>, String> {
            Ok(columns
                .iter()
                .map(|c| {
                    (*c == (1, 3)).then(|| {
                        if heightmap == horizon_client::mod_api::NativeHeightmap::OceanFloor {
                            70
                        } else {
                            64
                        }
                    })
                })
                .collect())
        }
        fn spawn_falling_block(
            &mut self,
            _: &str,
            state: BlockStateId,
            pos: [f64; 3],
            velocity: [f64; 3],
            _: horizon_client::mod_api::NativeFalling,
        ) -> bool {
            self.lines.lock().unwrap().push(format!(
                "falling state {} at {},{},{} v {},{},{}",
                state.0, pos[0], pos[1], pos[2], velocity[0], velocity[1], velocity[2]
            ));
            true
        }
        fn summon(&mut self, _: &str, kind: &str, pos: [f64; 3], nbt: Compound) -> Option<u32> {
            use horizon_world::nbt::{JavaString, Tag};
            let text = |t: Option<&Tag>| match t {
                Some(Tag::String(s)) => s.to_string_lossy(),
                Some(Tag::Int(n)) => n.to_string(),
                _ => "?".to_owned(),
            };
            let item = match nbt.get(&JavaString::from("Item")) {
                Some(Tag::Compound(item)) => format!(
                    " of {} x{}",
                    text(item.get(&JavaString::from("id"))),
                    text(item.get(&JavaString::from("count")))
                ),
                _ => String::new(),
            };
            self.lines.lock().unwrap().push(format!(
                "summon {kind} at {},{},{}{item}",
                pos[0], pos[1], pos[2]
            ));
            Some(42)
        }
        fn set_nickname(&mut self, _: u128, nickname: Option<&str>) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("nickname steve {}", nickname.unwrap_or("none")));
            true
        }
        fn give_experience(&mut self, _: u128, points: u32) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("give_experience steve {points}"));
            true
        }
        fn give_levels(&mut self, _: u128, levels: i32) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("give_levels steve {levels}"));
            true
        }
        fn data(&self, path: &str) -> Option<Vec<u8>> {
            match path {
                "data/ruby/x.json" => Some(br#"{"a": 1, "list": [2, 3]}"#.to_vec()),
                "data/ruby/t.txt" => Some(b"hi".to_vec()),
                _ => None,
            }
        }
        fn mob_effect(
            &mut self,
            _: &str,
            id: u32,
            effect: &str,
            ticks: i32,
            amplifier: i32,
        ) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("mob_effect {id} {effect} {ticks} x{amplifier}"));
            true
        }
        fn player_effect(&mut self, _: u128, effect: &str, ticks: i32, amplifier: i32) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("player_effect steve {effect} {ticks} x{amplifier}"));
            true
        }
    }

    fn wand() -> ItemStack {
        ItemStack {
            item: "ruby:wand".into(),
            count: 3,
            damage: 0,
            extra: None,
        }
    }

    /// Handler firings since load: the helper-read tests bump `__witness`
    /// first, so 0 means the handler never ran.
    fn witness(lua: &mlua::Lua) -> i64 {
        lua.globals().get("__witness").unwrap_or(0)
    }

    #[test]
    fn the_item_hooks_reach_the_handlers_of_their_item() {
        let (dir, path) = hmod(
            "item-hooks",
            &[(
                "scripts/server/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                -- No kind filter any more: every use wakes the handler, which
                -- keeps its own item, and the player comes from the host read.
                horizon.on("use_item", function(event)
                  if event.stack.id ~= "ruby:wand" then return end
                  __witness = (__witness or 0) + 1
                  local player = horizon.players()[1]
                  log("use " .. event.stack.id .. " x" .. event.stack.count .. " by " .. player.name)
                  horizon.take_items(event.stack.id, 1)
                  horizon.cancel()
                end)
                horizon.on("use_item_on", function(event)
                  if event.stack.id ~= "ruby:wand" then return end
                  __witness = (__witness or 0) + 1
                  local id = horizon.block(event.pos.x, event.pos.y, event.pos.z)
                  log("use_on " .. id .. " at " .. event.pos.x .. "," .. event.pos.y .. "," .. event.pos.z .. " face " .. event.face.y)
                end)
                -- The blow's entity kind filters, but the held item neither
                -- filters nor arrives: the handler reads the event only.
                horizon.on("attack_entity", function(event)
                  __witness = (__witness or 0) + 1
                  log("hit " .. event.kind .. " " .. event.entity .. " at " .. event.pos.x)
                  horizon.cancel()
                end, "minecraft:cow")
                horizon.on("living_entity_use_item_finish", function(event)
                  if event.stack.id ~= "ruby:wand" then return end
                  __witness = (__witness or 0) + 1
                  log("finished " .. event.stack.id .. " in " .. event.hand)
                  event.result = { id = "minecraft:stick", count = 2 }
                end)
                -- The tick names no item at all: the slot and its selection.
                horizon.on("inventory_tick", function(event)
                  __witness = (__witness or 0) + 1
                  log("tick slot " .. event.slot .. " " .. tostring(event.selected))
                  horizon.broadcast("ticked")
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let wand = wand();
        // The wand's use is taken, and one is consumed; an apple's use wakes
        // the handler too, and its in-Lua filter keeps it quiet.
        assert_eq!(
            m.use_item(&mut host, &mut test_events::use_item_event(&wand)),
            Flow::Cancel
        );
        let apple = ItemStack {
            item: "minecraft:apple".into(),
            count: 1,
            damage: 0,
            extra: None,
        };
        assert_eq!(
            m.use_item(&mut host, &mut test_events::use_item_event(&apple)),
            Flow::Continue
        );
        let mut stone = test_events::use_item_on_event(&wand, [1, 2, 3], [0, 1, 0]);
        assert_eq!(m.use_item_on(&mut host, &mut stone), Flow::Continue);
        assert_eq!(
            m.attack_entity(
                &mut host,
                &mut test_events::attack_entity_event(7, "minecraft:cow", [2.0, 64.0, 3.0])
            ),
            Flow::Cancel
        );
        // A skeleton's blow wakes nobody: the entity kind filters.
        assert_eq!(
            m.attack_entity(
                &mut host,
                &mut test_events::attack_entity_event(9, "minecraft:skeleton", [5.0, 64.0, 5.0])
            ),
            Flow::Continue
        );
        let mut result = Some(wand.clone());
        m.living_entity_use_item_finish(
            &mut host,
            &mut test_events::living_entity_use_item_finish_event(
                &wand,
                None,
                horizon_client::mod_api::Hand::Main,
                0,
                &mut result,
            ),
        );
        assert_eq!(
            result.as_ref().map(|s| (&*s.item, s.count)),
            Some(("minecraft:stick", 2))
        );
        m.inventory_tick(&mut host, &mut test_events::inventory_tick_event(3, true));
        m.inventory_tick(&mut host, &mut test_events::inventory_tick_event(4, false));
        let log: Vec<String> = m
            .lua
            .globals()
            .get::<Table>("LOG")
            .unwrap()
            .sequence_values()
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(
            log,
            [
                "use ruby:wand x3 by steve",
                "use_on minecraft:stone at 1,2,3 face 1",
                "hit minecraft:cow 7 at 2",
                "finished ruby:wand in main",
                "tick slot 3 true",
                "tick slot 4 false",
            ]
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            ["took 1 ruby:wand", "ticked", "ticked"]
        );
        assert_eq!(m.lua.globals().get::<f64>("__witness").unwrap(), 6.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_player_hooks_tell_of_joins_leaves_respawns_and_deaths() {
        let (dir, path) = hmod(
            "world-hooks",
            &[(
                "scripts/server/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                horizon.on("player_logged_in", function(event)
                  __witness = (__witness or 0) + 1
                  log("joined " .. event.player.name .. " hp " .. event.player.health .. " " .. event.player.uuid)
                end)
                horizon.on("player_logged_out", function(event)
                  __witness = (__witness or 0) + 1
                  log("left " .. event.player.name)
                end)
                horizon.on("player_respawned", function(event)
                  __witness = (__witness or 0) + 1
                  log("respawned " .. event.player.name)
                end)
                horizon.on("player_died", function(event)
                  __witness = (__witness or 0) + 1
                  log("player died " .. event.player .. " " .. tostring(event.cause))
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = 0x123e4567_e89b_12d3_a456_426614174000;
        m.player_logged_in(
            &mut host,
            &mut test_events::player_logged_in_event(steve, "steve"),
        );
        m.player_logged_out(
            &mut host,
            &mut test_events::player_logged_out_event(5, "alex"),
        );
        m.player_respawned(
            &mut host,
            &mut test_events::player_respawned_event(steve, "steve"),
        );
        m.player_died(
            &mut host,
            &mut test_events::player_died_event(steve, Some("death.attack.lava")),
        );
        // A death names no one and the host has no record of alex any more:
        // the native event carries only the uuid, so no name is told.
        m.player_died(&mut host, &mut test_events::player_died_event(5, None));
        // A logout with no name keeps the name the earlier logout told.
        m.player_logged_out(&mut host, &mut test_events::player_logged_out_event(5, ""));
        let log: Vec<String> = m
            .lua
            .globals()
            .get::<Table>("LOG")
            .unwrap()
            .sequence_values()
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(
            log,
            [
                "joined steve hp 20 123e4567-e89b-12d3-a456-426614174000",
                "left alex",
                "respawned steve",
                "player died 123e4567-e89b-12d3-a456-426614174000 death.attack.lava",
                "player died 00000000-0000-0000-0000-000000000005 nil",
                "left alex",
            ]
        );
        assert_eq!(m.lua.globals().get::<f64>("__witness").unwrap(), 6.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn destroy_place_and_pickup_reach_their_handlers() {
        let (dir, path) = hmod(
            "break-place-pickup",
            &[(
                "scripts/server/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                -- No kind filter any more: every break wakes the handler,
                -- which keeps its own block, and the player comes from the
                -- host read.
                horizon.on("destroy_block", function(event)
                  if event.state ~= "minecraft:stone" then return end
                  __witness = (__witness or 0) + 1
                  local player = horizon.players()[1]
                  log("broke " .. event.state .. " at " .. event.pos.x .. "," .. event.pos.y .. "," .. event.pos.z .. " by " .. player.name)
                  horizon.cancel()
                end)
                horizon.on("place_block", function(event)
                  if event.state ~= "minecraft:stone" then return end
                  __witness = (__witness or 0) + 1
                  log("placed " .. event.state .. " x" .. event.stack.count .. " with " .. event.stack.id)
                end)
                horizon.on("item_picked_up", function(event)
                  if event.item ~= "ruby:wand" then return end
                  __witness = (__witness or 0) + 1
                  local player = horizon.player(event.player)
                  log("pickup " .. event.item .. " x" .. event.count .. " by " .. player.name)
                  horizon.cancel()
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = 0x123e4567_e89b_12d3_a456_426614174000;
        assert_eq!(
            m.destroy_block(
                &mut host,
                &mut test_events::destroy_block_event([1, 2, 3], BlockStateId(1))
            ),
            Flow::Cancel
        );
        // Air breaks nothing: the handler's own filter keeps it quiet.
        assert_eq!(
            m.destroy_block(
                &mut host,
                &mut test_events::destroy_block_event([0, 0, 0], BlockStateId(0))
            ),
            Flow::Continue
        );
        let wand = wand();
        m.place_block(
            &mut host,
            &mut test_events::place_block_event(&wand, [1, 2, 3], BlockStateId(1)),
        );
        assert_eq!(
            m.item_picked_up(
                &mut host,
                &mut test_events::item_picked_up_event(steve, "ruby:wand", 2)
            ),
            Flow::Cancel
        );
        assert_eq!(
            m.item_picked_up(
                &mut host,
                &mut test_events::item_picked_up_event(steve, "minecraft:apple", 2)
            ),
            Flow::Continue
        );
        let log: Vec<String> = m
            .lua
            .globals()
            .get::<Table>("LOG")
            .unwrap()
            .sequence_values()
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(
            log,
            [
                "broke minecraft:stone at 1,2,3 by steve",
                "placed minecraft:stone x3 with ruby:wand",
                "pickup ruby:wand x2 by steve",
            ]
        );
        assert_eq!(m.lua.globals().get::<f64>("__witness").unwrap(), 3.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mob_died_reports_cause_and_credit_through_the_native_hook() {
        let (dir, path) = hmod(
            "died-hooks",
            &[(
                "scripts/server/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                horizon.on("mob_died", function(event)
                  log("died " .. event.kind .. " " .. event.entity .. " by " .. event.damage_type .. " causing " .. event.causing .. " credited " .. (event.player or "nobody"))
                end, "minecraft:cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = 0x123e4567_e89b_12d3_a456_426614174000;
        m.mob_died(
            &mut host,
            &mut test_events::mob_died_event(
                "minecraft:overworld",
                7,
                "minecraft:cow",
                "minecraft:on_fire",
                0,
                Some(steve),
            ),
        );
        // A zombie is nobody's business: the kind filter stays quiet.
        m.mob_died(
            &mut host,
            &mut test_events::mob_died_event(
                "minecraft:overworld",
                9,
                "minecraft:zombie",
                "minecraft:fall",
                0,
                None,
            ),
        );
        let log: Vec<String> = m
            .lua
            .globals()
            .get::<Table>("LOG")
            .unwrap()
            .sequence_values()
            .map(|v| v.unwrap())
            .collect();
        // One entry: the cow's death ran the handler, the zombie stayed quiet.
        assert_eq!(
            log,
            [
                "died minecraft:cow 7 by minecraft:on_fire causing 0 credited 123e4567-e89b-12d3-a456-426614174000"
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mob_killed_player_uuid_round_trips_as_text() {
        let (dir, path) = hmod(
            "killed-uuid",
            &[(
                "scripts/server/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                horizon.on("mob_killed", function(event)
                  log("killed " .. event.entity .. " by " .. event.player .. " (" .. type(event.player) .. ")")
                end, "minecraft:cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        // A real 128-bit player UUID: Lua sees its uuid text, never a Lua
        // number (an f64 cannot hold 128 bits, so a numeric seed of this
        // value would not round-trip).
        let steve = 0x6ba7b811_9dad_11d1_80b4_00c04fd430c8;
        m.mob_killed(
            &mut host,
            &mut test_events::mob_killed_event(steve, 7, "minecraft:cow"),
        );
        // A zombie is nobody's business: the kind filter stays quiet.
        m.mob_killed(
            &mut host,
            &mut test_events::mob_killed_event(steve, 9, "minecraft:zombie"),
        );
        let log: Vec<String> = m
            .lua
            .globals()
            .get::<Table>("LOG")
            .unwrap()
            .sequence_values()
            .map(|v| v.unwrap())
            .collect();
        // One entry, with the killer's uuid text (not a lossy number).
        assert_eq!(
            log,
            ["killed 7 by 6ba7b811-9dad-11d1-80b4-00c04fd430c8 (string)"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn timers_run_from_the_server_tick_and_world_and_player_data_persist() {
        use horizon_world::nbt::Tag;
        let (dir, path) = hmod(
            "timers-data",
            &[(
                "scripts/server/init.lua",
                br#"
                N = 0
                horizon.after(3, function()
                  N = N + 10
                  local w = horizon.world_data()
                  w.runs = (w.runs or 0) + 1
                end)
                horizon.every(2, function() N = N + 1 end)
                horizon.every(1, function() error("a timer that fails") end)
                horizon.on("interact_entity", function(event)
                  local w = horizon.world_data()
                  w.clicks = (w.clicks or 0) + 1
                  local p = horizon.player_data("steve")
                  p.pets = (p.pets or 0) + 1
                  local again = horizon.player_data("steve")
                  again.pets = again.pets + 1
                  PD = horizon.player_data("alex")
                end, "minecraft:cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let key = |k: &str| horizon_world::nbt::JavaString::from(k);
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        for _ in 0..5 {
            for phase in [
                ServerTickPhase::ServerTickStart {
                    world_running: true,
                },
                ServerTickPhase::ServerTickEnd {
                    world_running: true,
                },
            ] {
                m.server_tick(
                    &mut host,
                    &mut test_events::server_tick_event(&context, phase),
                );
            }
        }
        // The one-shot ran once at tick 3, the repeating one at 2 and 4; the
        // failing one did not stop them.
        assert_eq!(m.lua.globals().get::<i32>("N").unwrap(), 12);
        let world = m
            .world_attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Level)
            .unwrap()
            .unwrap();
        assert_eq!(world.get(&key("runs")), Some(&Tag::Int(1)));
        // Two clicks: the world counts them, the player's table is one per
        // call (both writes land), and an unknown player has none.
        for _ in 0..2 {
            m.interact_entity(
                &mut host,
                &mut test_events::interact_entity_event(7, "minecraft:cow", None),
            );
        }
        let world = m
            .world_attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Level)
            .unwrap()
            .unwrap();
        assert_eq!(world.get(&key("clicks")), Some(&Tag::Int(2)));
        let steve = 0x123e4567_e89b_12d3_a456_426614174000;
        let player = m
            .player_attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Player(steve))
            .unwrap()
            .unwrap();
        assert_eq!(player.get(&key("pets")), Some(&Tag::Int(4)));
        assert!(m.lua.globals().get::<Value>("PD").unwrap().is_nil());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A host with two entities that keep attachments, three players and a
    /// swappable acting player, for the helper-read tests: steve acts and
    /// holds a wand, alex holds a damaged bone with components, cam holds
    /// nothing.
    struct DataHost {
        world: horizon_client::mod_api::Attachments,
        players_data: HashMap<u128, horizon_client::mod_api::Attachments>,
        entities: HashMap<u32, horizon_client::mod_api::Attachments>,
        players: Vec<horizon_client::mod_api::NativeServerPlayer>,
        acting: Option<u128>,
        held: HashMap<u128, ItemStack>,
    }
    const STEVE_UUID: u128 = 0x123e4567_e89b_12d3_a456_426614174000;
    const ALEX_UUID: u128 = 0x123e4567_e89b_12d3_a456_426614174001;
    const CAM_UUID: u128 = 0x123e4567_e89b_12d3_a456_426614174002;
    fn data_player(uuid: u128, name: &str) -> horizon_client::mod_api::NativeServerPlayer {
        test_events::native_server_player(
            uuid,
            name.into(),
            0,
            "minecraft:overworld".into(),
            [1.0, 64.0, 1.0],
            0,
            20.0,
            0.0,
            0.0,
            0,
            String::new(),
        )
    }
    fn data_host() -> DataHost {
        let mut held = HashMap::new();
        held.insert(STEVE_UUID, wand());
        held.insert(
            ALEX_UUID,
            ItemStack {
                item: "minecraft:bone".into(),
                count: 5,
                damage: 2,
                extra: Some(Box::new(horizon_client::item_data::StackData {
                    repair_cost: 7,
                    ..Default::default()
                })),
            },
        );
        DataHost {
            world: Default::default(),
            players_data: Default::default(),
            entities: [(7, Default::default()), (9, Default::default())]
                .into_iter()
                .collect(),
            // The acting player is not first: a test that takes
            // `players()[1]` by mistake names alex, not steve.
            players: [
                data_player(ALEX_UUID, "alex"),
                data_player(STEVE_UUID, "steve"),
                data_player(CAM_UUID, "cam"),
            ]
            .into_iter()
            .collect(),
            acting: Some(STEVE_UUID),
            held,
        }
    }
    impl Host for DataHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }
        fn attachments(
            &mut self,
            holder: Holder,
        ) -> horizon_client::mod_api::AttachmentResult<&mut horizon_client::mod_api::Attachments>
        {
            match holder {
                Holder::Level => Ok(&mut self.world),
                Holder::Player(uuid) => self
                    .players_data
                    .get_mut(&uuid)
                    .ok_or(horizon_client::mod_api::AttachmentError::MissingHolder),
                Holder::Entity(id) => self
                    .entities
                    .get_mut(&id)
                    .ok_or(horizon_client::mod_api::AttachmentError::MissingHolder),
                _ => Err(horizon_client::mod_api::AttachmentError::MissingHolder),
            }
        }
        fn acting_player(&self) -> Option<horizon_client::mod_api::NativeServerPlayer> {
            self.acting
                .and_then(|uuid| self.players.iter().find(|p| p.uuid == uuid).cloned())
        }
        fn held_stack(&self, uuid: u128) -> Option<ItemStack> {
            self.held.get(&uuid).cloned()
        }
        fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
            self.players.clone()
        }
    }

    #[test]
    fn entity_data_persists_per_entity_with_world_data_rules() {
        use horizon_world::nbt::{JavaString, Tag};
        let (dir, path) = hmod(
            "entity-data",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("interact_entity", function(event)
                  __witness = (__witness or 0) + 1
                  -- The same table twice in one call, so no edit is lost.
                  assert(horizon.entity_data(event.entity) == horizon.entity_data(event.entity), "same table")
                  local d = horizon.entity_data(event.entity)
                  if ROUND == 2 then
                    READBACK = d.count .. "/" .. d.who .. "/" .. d.nest.deep.x .. "/" .. d.big
                  elseif event.entity == 7 then
                    d.count = (d.count or 0) + 1
                    d.who = "steve"
                    d.nest = { deep = { x = 1 } }
                  else
                    d.other = 2
                  end
                  -- The other entity's table is its own, and an unknown id
                  -- reads nil.
                  assert(horizon.entity_data(9).count == nil or event.entity == 9 or ROUND == 2, "mixing")
                  UNKNOWN = horizon.entity_data(4242)
                end, "minecraft:cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = data_host();
        let key = |k: &str| JavaString::from(k);
        // Entity 7 starts with a long: leaving it alone must keep the type.
        m.entity_attachment
            .set(
                &mut host as &mut dyn Host,
                Holder::Entity(7),
                [(key("big"), Tag::Long(42))].into_iter().collect(),
            )
            .unwrap();
        m.interact_entity(
            &mut host,
            &mut test_events::interact_entity_event(7, "minecraft:cow", None),
        );
        m.interact_entity(
            &mut host,
            &mut test_events::interact_entity_event(9, "minecraft:cow", None),
        );
        assert_eq!(witness(&m.lua), 2);
        let seven = m
            .entity_attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Entity(7))
            .unwrap()
            .unwrap();
        // An int, a string and a nested table, written fresh; the long left
        // alone is still a long.
        assert_eq!(seven.get(&key("count")), Some(&Tag::Int(1)));
        assert_eq!(
            seven.get(&key("who")),
            Some(&Tag::String(JavaString::from("steve")))
        );
        assert_eq!(
            seven.get(&key("big")),
            Some(&Tag::Long(42)),
            "a long left alone keeps its NBT type"
        );
        let Tag::Compound(nest) = &seven[&key("nest")] else {
            panic!("a nested table is a compound");
        };
        let Tag::Compound(deep) = &nest[&key("deep")] else {
            panic!("tables nest");
        };
        assert_eq!(deep.get(&key("x")), Some(&Tag::Int(1)));
        // Entity 9's own write landed on 9 only.
        let nine = m
            .entity_attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Entity(9))
            .unwrap()
            .unwrap();
        assert_eq!(nine.get(&key("other")), Some(&Tag::Int(2)));
        assert_eq!(nine.get(&key("count")), None, "entities do not mix");
        assert_eq!(seven.get(&key("other")), None, "entities do not mix");
        assert!(m.lua.globals().get::<Value>("UNKNOWN").unwrap().is_nil());
        // A later call reads it all back, with the types intact.
        m.lua.globals().set("ROUND", 2).unwrap();
        m.interact_entity(
            &mut host,
            &mut test_events::interact_entity_event(7, "minecraft:cow", None),
        );
        assert_eq!(
            m.lua.globals().get::<String>("READBACK").unwrap(),
            "1/steve/1/42"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entity_data_error_drops_the_writes() {
        let (dir, path) = hmod(
            "entity-data-error",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("interact_entity", function(event)
                  __witness = (__witness or 0) + 1
                  local d = horizon.entity_data(event.entity)
                  d.boom = 1
                  error("nope")
                end, "minecraft:cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = data_host();
        m.interact_entity(
            &mut host,
            &mut test_events::interact_entity_event(7, "minecraft:cow", None),
        );
        assert_eq!(witness(&m.lua), 1);
        // The handler ran, then failed: nothing it wrote is kept, like the
        // queued actions.
        let seven = m
            .entity_attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Entity(7))
            .unwrap();
        assert_eq!(seven, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn player_tables_carry_their_own_held_but_seeds_do_not() {
        let (dir, path) = hmod(
            "held",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("player_tick", function(event)
                  __witness = (__witness or 0) + 1
                  SEEN = {}
                  for _, p in ipairs(horizon.players()) do
                    SEEN[p.name] = p.held and (p.held.id .. "|" .. p.held.count .. "|" .. p.held.damage .. "|" .. tostring(p.held.plain) .. "|" .. (p.held.extra and "extra" or "noextra")) or "nil"
                  end
                  local alex = horizon.player("alex")
                  ALEX = alex.held.id .. "|" .. alex.held.count
                  NOPE = horizon.player("nobody")
                end)
                horizon.on("player_logged_in", function(event)
                  assert(event.player.held == nil, "a generated player seed carries no held")
                  JOINED = event.player.name
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = data_host();
        m.player_tick(&mut host, &mut test_events::player_tick_event());
        assert_eq!(witness(&m.lua), 1);
        let seen: Table = m.lua.globals().get("SEEN").unwrap();
        assert_eq!(
            seen.get::<String>("steve").unwrap(),
            "ruby:wand|3|0|true|noextra"
        );
        assert_eq!(
            seen.get::<String>("alex").unwrap(),
            "minecraft:bone|5|2|false|extra"
        );
        assert_eq!(seen.get::<String>("cam").unwrap(), "nil");
        assert_eq!(
            m.lua.globals().get::<String>("ALEX").unwrap(),
            "minecraft:bone|5"
        );
        assert!(m.lua.globals().get::<Value>("NOPE").unwrap().is_nil());
        m.player_logged_in(
            &mut host,
            &mut test_events::player_logged_in_event(ALEX_UUID, "alex"),
        );
        assert_eq!(m.lua.globals().get::<String>("JOINED").unwrap(), "alex");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn acting_player_is_the_ticking_player_or_nil() {
        let (dir, path) = hmod(
            "acting",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("player_tick", function(event)
                  __witness = (__witness or 0) + 1
                  local a = horizon.acting_player()
                  ACT = a and (a.name .. "|" .. a.uuid .. "|" .. (a.held and a.held.id or "nil")) or "nil"
                end)
                horizon.on("living_heal", function(event)
                  __witness = (__witness or 0) + 1
                  -- Hostless means nil, not an error: `pcall` must succeed.
                  local ok, v = pcall(horizon.acting_player)
                  OK4 = ok
                  GOT = v
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = data_host();
        m.player_tick(&mut host, &mut test_events::player_tick_event());
        // Steve acts, though alex is `players()[1]`: the acting player is
        // returned, not the first of the list.
        assert_eq!(
            m.lua.globals().get::<String>("ACT").unwrap(),
            "steve|123e4567-e89b-12d3-a456-426614174000|ruby:wand"
        );
        // A hostless hook has no acting player: nil, not an error.
        m.living_heal(&mut test_events::living_heal_event(7, 3.0));
        assert_eq!(witness(&m.lua), 2);
        assert!(m.lua.globals().get::<bool>("OK4").unwrap());
        assert!(m.lua.globals().get::<Value>("GOT").unwrap().is_nil());
        // No player swapped in, though the host is there: nil too.
        host.acting = None;
        m.player_tick(&mut host, &mut test_events::player_tick_event());
        assert_eq!(m.lua.globals().get::<String>("ACT").unwrap(), "nil");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn timers_fire_only_on_the_end_phase_of_a_running_world() {
        let (dir, path) = hmod(
            "timers-gate",
            &[(
                "scripts/server/init.lua",
                br#"
                N = 0
                horizon.after(1, function() N = N + 1 end)
                horizon.every(1, function() N = N + 1 end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        // A start tick while the world runs: not the timers' phase.
        m.server_tick(
            &mut host,
            &mut test_events::server_tick_event(
                &context,
                ServerTickPhase::ServerTickStart {
                    world_running: true,
                },
            ),
        );
        assert_eq!(m.lua.globals().get::<i32>("N").unwrap(), 0);
        // The end of a tick while the world is stopped: not the timers'
        // world.
        m.server_tick(
            &mut host,
            &mut test_events::server_tick_event(
                &context,
                ServerTickPhase::ServerTickEnd {
                    world_running: false,
                },
            ),
        );
        assert_eq!(m.lua.globals().get::<i32>("N").unwrap(), 0);
        // The end of a running tick fires both timers once.
        m.server_tick(
            &mut host,
            &mut test_events::server_tick_event(
                &context,
                ServerTickPhase::ServerTickEnd {
                    world_running: true,
                },
            ),
        );
        assert_eq!(m.lua.globals().get::<i32>("N").unwrap(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn client_timers_fire_on_client_ticks() {
        let (dir, path) = hmod(
            "client-timers",
            &[(
                "scripts/client/init.lua",
                br#"
                N = 0
                horizon.after(2, function() N = N + 1 end)
                horizon.every(3, function() N = N + 10 end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        let lua = m.lua.clone();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let n = |lua: &Lua| lua.globals().get::<i32>("N").unwrap();
        horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        assert_eq!(n(&lua), 0);
        horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        assert_eq!(n(&lua), 1);
        horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        assert_eq!(n(&lua), 11);
        for _ in 0..2 {
            horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        }
        assert_eq!(n(&lua), 11);
        horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        assert_eq!(n(&lua), 21);
        horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        assert_eq!(n(&lua), 21);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn client_timers_do_not_fire_on_server_tick() {
        let (dir, path) = hmod(
            "client-timers-server-tick",
            &[(
                "scripts/client/init.lua",
                b"N = 0\nhorizon.after(1, function() N = N + 1 end)",
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        let lua = m.lua.clone();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        for _ in 0..3 {
            horizon_client::mod_api::dispatch_server_tick(
                &mut host,
                &mods,
                &context,
                ServerTickPhase::ServerTickEnd {
                    world_running: true,
                },
            );
        }
        assert_eq!(lua.globals().get::<i32>("N").unwrap(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn server_timers_do_not_fire_on_client_tick_post() {
        let (dir, path) = hmod(
            "server-timers-client-tick",
            &[(
                "scripts/server/init.lua",
                b"N = 0\nhorizon.after(1, function() N = N + 1 end)\nhorizon.every(1, function() N = N + 10 end)",
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let lua = m.lua.clone();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        for _ in 0..3 {
            horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        }
        assert_eq!(lua.globals().get::<i32>("N").unwrap(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn commands_feedback_titles_sounds_and_particles_reach_the_host() {
        let (dir, path) = hmod(
            "commands",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.command("parasite count [kind:word] [n:integer]", 2, function(player, args)
                  horizon.feedback("count " .. (args.kind or "all") .. " " .. tostring(args.n) .. " by " .. player.name)
                  horizon.sound("minecraft:entity.zombie.ambient", 1, 2, 3, 0.5)
                  horizon.particles("minecraft:flame", 1, 2, 3, 5, 0.5, 0.1)
                  horizon.title(player, "Parasites")
                  horizon.subtitle(player.uuid, "everywhere")
                  horizon.title_times(player, 5, 40, 5)
                  horizon.action_bar(player.name, "bar")
                  horizon.tell("steve", "hello")
                  horizon.tell("alex", "nobody")
                end)
                horizon.command("say <what:text>", 0, function(player, args) horizon.feedback(args.what) end)
                horizon.command("parasite hives", 2, function(player, args) horizon.feedback("hives") end)
                horizon.command("probeadd <a:integer> <b:number> [label:word]", 0, function(player, args)
                  horizon.feedback((args.label or "sum") .. " = " .. tostring(args.a + args.b))
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let mut commands = Vec::new();
        m.register_commands(
            &mut host,
            &mut test_events::register_commands_event(&mut commands),
        );
        // The collect appends the declared defs, with their levels and no
        // permission node (`horizon.command` takes none).
        assert_eq!(
            commands
                .iter()
                .map(|c| (c.usage.as_str(), c.level, c.permission.clone()))
                .collect::<Vec<_>>(),
            [
                ("parasite count [kind:word] [n:integer]", 2, None::<String>,),
                ("say <what:text>", 0, None),
                ("parasite hives", 2, None),
                ("probeadd <a:integer> <b:number> [label:word]", 0, None)
            ]
        );
        let steve = host.acting_player().unwrap();
        let run = |m: &LuaMod, host: &mut MobHost, line: &str| {
            m.perform_command(host, &mut test_events::perform_command_event(&steve, line))
        };
        assert_eq!(run(&m, &mut host, "parasite count zombie 3"), Flow::Cancel);
        assert_eq!(run(&m, &mut host, "parasite count"), Flow::Cancel);
        assert_eq!(run(&m, &mut host, "say hello there  world"), Flow::Cancel);
        assert_eq!(run(&m, &mut host, "parasite hives"), Flow::Cancel);
        // Arguments of three kinds; a bad integer and a missing required
        // argument are refused with a message, and the handler is not called.
        assert_eq!(run(&m, &mut host, "probeadd 2 0.5 total"), Flow::Cancel);
        // The refusals are translated: the bundled English here.
        let (flows, missed) = horizon_client::lang::scoped(
            horizon_client::lang::Language::load(&[], "en_us"),
            || {
                (
                    run(&m, &mut host, "probeadd x 1"),
                    run(&m, &mut host, "probeadd 2"),
                )
            },
        );
        assert_eq!(flows, (Flow::Cancel, Flow::Cancel));
        assert_eq!(missed, Vec::<String>::new());
        assert_eq!(run(&m, &mut host, "parasite"), Flow::Continue);
        assert_eq!(run(&m, &mut host, "other thing"), Flow::Continue);
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "feedback count zombie 3 by steve",
                "sound minecraft:entity.zombie.ambient at 1,2,3 x0.5 p1",
                "particles minecraft:flame x5 at 1,2,3 spread 0.5 speed 0.1",
                "title steve: Parasites",
                "subtitle steve: everywhere",
                "times steve: 5/40/5",
                "actionbar steve: bar",
                "tell steve: hello",
                "feedback count all nil by steve",
                "sound minecraft:entity.zombie.ambient at 1,2,3 x0.5 p1",
                "particles minecraft:flame x5 at 1,2,3 spread 0.5 speed 0.1",
                "title steve: Parasites",
                "subtitle steve: everywhere",
                "times steve: 5/40/5",
                "actionbar steve: bar",
                "tell steve: hello",
                "feedback hello there world",
                "feedback hives",
                "feedback total = 2.5",
                "feedback a needs an integer, not \"x\"",
                "feedback Usage: /probeadd <a:integer> <b:number> [label:word]",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn horizon_on_hears_colonless_command_words() {
        // `horizon.on("perform_command", fn, words)` hears the commands whose
        // words are `words`: command words are not registry ids, so the kind
        // is stored as given (`parasite count`, not
        // `minecraft:parasite count`), like `horizon.command` stores it.
        let (dir, path) = hmod(
            "on-hears-words",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.command("parasite count [kind:word]", 0, function(player, args)
                  COUNT = (COUNT or 0) + 1
                end)
                horizon.on("perform_command", function(player, args)
                  ON_COUNT = (ON_COUNT or 0) + 1
                end, "parasite count")
                horizon.command("test:probe [kind:word]", 0, function(player, args)
                  COLON_COUNT = (COLON_COUNT or 0) + 1
                end)
                horizon.on("perform_command", function(player, args)
                  COLON_ON = (COLON_ON or 0) + 1
                end, "test:probe")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = host.acting_player().unwrap();
        let run = |m: &LuaMod, host: &mut MobHost, line: &str| {
            m.perform_command(host, &mut test_events::perform_command_event(&steve, line))
        };
        // Words without a colon hear the declaration's handler and the
        // `horizon.on` one; a colon word still hears both.
        assert_eq!(run(&m, &mut host, "parasite count zombie"), Flow::Cancel);
        assert_eq!(run(&m, &mut host, "test:probe zombie"), Flow::Cancel);
        let g = m.lua.globals();
        assert_eq!(g.get::<i64>("COUNT").unwrap(), 1);
        assert_eq!(g.get::<i64>("ON_COUNT").unwrap(), 1);
        assert_eq!(g.get::<i64>("COLON_COUNT").unwrap(), 1);
        assert_eq!(g.get::<i64>("COLON_ON").unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_world_player_and_config_helpers_reach_the_host() {
        let (dir, path) = hmod(
            "helpers",
            &[(
                "scripts/server/init.lua",
                br#"
                -- The settings file, at load: empty the first time, then what was saved.
                local c = horizon.config()
                CONFIG_LIMIT = c.limit
                c.limit = (c.limit or 8) * 2
                c.kinds = { "a", "b" }
                horizon.save_config(c)
                horizon.command("helpers", 0, function(player, args)
                  local id = horizon.summon(player.dimension, "ruby:crawler", 1, 2, 3)
                  SUMMONED = id
                  horizon.effect(id, "speed", 100, 1)
                  horizon.effect(player, "minecraft:regeneration", 60)
                  horizon.give_items(player, "bone", 3)
                  horizon.take_items("steve", "minecraft:bone", 2)
                  horizon.give_items("minecraft:bone", 1)
                  horizon.give_experience(player, 10)
                  horizon.give_levels(player.uuid, -2)
                  horizon.set_nickname(player, "Steve the Bold")
                  horizon.set_nickname(player)
                  horizon.run_command("/weather clear")
                  horizon.set_block(1, 2, 3, "stone")
                  horizon.set_block(1, 3, 3, "ruby:hive", { stage = "mature" }, 2)
                  horizon.fill(2, 65, 1, 0, 64, 0, "stone")
                  horizon.falling_block(1.5, 70, 3.5, "minecraft:stone", 0, 0.5, 0)
                  horizon.drop_item(1.5, 65, 3.5, "bone", 4)
                  INV = horizon.inventory(player)
                  COUNT = horizon.count_items(player, "ruby:wand")
                  CARRIED = horizon.carried_items(player)
                  OTHER = horizon.count_items("00000000-0000-0000-0000-000000000002", "minecraft:bone")
                  NOBODY = horizon.inventory("nobody")
                  LIGHT = horizon.light(1, 2, 3)
                  HEIGHT = horizon.surface_height(1, 3)
                  FLOOR = horizon.surface_height(1, 3, "ocean_floor")
                  NOWHERE = horizon.surface_height(9, 9)
                  DATA = horizon.data("data/ruby/x.json")
                  TEXT = horizon.data("data/ruby/t.txt")
                  NONE = horizon.data("data/ruby/none.json")
                  HELD = player.held
                end)
                horizon.command("badblock", 0, function(player, args)
                  horizon.set_block(0, 0, 0, "ruby:nothing")
                end)
                -- The widest fill there is: refused for its columns, not an
                -- overflow in the subtraction.
                horizon.command("bigfill", 0, function(player, args)
                  horizon.fill(-2147483648, 0, 0, 2147483647, 0, 0, "stone")
                end)
                "#,
            )],
        );
        let config = dir.join("config");
        let m = LuaMod::load_with(&path, LuaSide::Server, &[], Some(&config)).unwrap();
        // Saved as JSON a person can edit, and read back at the next load.
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(config.join("ruby.json")).unwrap())
                .unwrap();
        assert_eq!(
            saved,
            serde_json::json!({ "limit": 16, "kinds": ["a", "b"] })
        );
        assert!(
            m.lua
                .globals()
                .get::<Value>("CONFIG_LIMIT")
                .unwrap()
                .is_nil()
        );
        m.reload().unwrap();
        assert_eq!(m.lua.globals().get::<i64>("CONFIG_LIMIT").unwrap(), 16);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(config.join("ruby.json")).unwrap())
                .unwrap();
        assert_eq!(saved["limit"], 32);

        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = host.acting_player().unwrap();
        let mut run = |line: &str| {
            m.perform_command(
                &mut host,
                &mut test_events::perform_command_event(&steve, line),
            )
        };
        assert_eq!(run("helpers"), Flow::Cancel);
        assert_eq!(run("badblock"), Flow::Cancel);
        assert_eq!(run("bigfill"), Flow::Cancel);
        let g = m.lua.globals();
        // The summon answered at once, with the id the actions then used.
        assert_eq!(g.get::<i64>("SUMMONED").unwrap(), 42);
        let inv: Table = g.get("INV").unwrap();
        assert_eq!(
            inv.get::<Table>(4).unwrap().get::<String>("id").unwrap(),
            "ruby:wand"
        );
        assert!(inv.get::<Value>(1).unwrap().is_nil());
        assert_eq!(g.get::<i64>("COUNT").unwrap(), 3);
        assert_eq!(
            g.get::<Table>("CARRIED")
                .unwrap()
                .get::<i64>("ruby:wand")
                .unwrap(),
            3
        );
        assert_eq!(g.get::<i64>("OTHER").unwrap(), 5);
        assert!(g.get::<Value>("NOBODY").unwrap().is_nil());
        let light: Table = g.get("LIGHT").unwrap();
        assert_eq!(
            (
                light.get::<i64>("block").unwrap(),
                light.get::<i64>("sky").unwrap()
            ),
            (7, 15)
        );
        assert_eq!(g.get::<i64>("HEIGHT").unwrap(), 64);
        assert_eq!(g.get::<i64>("FLOOR").unwrap(), 70);
        assert!(g.get::<Value>("NOWHERE").unwrap().is_nil());
        let data: Table = g.get("DATA").unwrap();
        assert_eq!(data.get::<i64>("a").unwrap(), 1);
        assert_eq!(data.get::<Vec<i64>>("list").unwrap(), [2, 3]);
        assert_eq!(g.get::<String>("TEXT").unwrap(), "hi");
        assert!(g.get::<Value>("NONE").unwrap().is_nil());
        assert_eq!(
            g.get::<Table>("HELD")
                .unwrap()
                .get::<String>("item")
                .unwrap(),
            "ruby:wand"
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "summon ruby:crawler at 1,2,3",
                "mob_effect 42 minecraft:speed 100 x1",
                "player_effect steve minecraft:regeneration 60 x0",
                "give_stack steve 3 minecraft:bone",
                "take_from steve 2 minecraft:bone",
                "gave 1 minecraft:bone",
                "give_experience steve 10",
                "give_levels steve -2",
                "nickname steve Steve the Bold",
                "nickname steve none",
                "run weather clear",
                "set_block 1,2,3 state 1 flags 3",
                "set_block 1,3,3 state 2 flags 2",
                "edit minecraft:stone 6 runs y 64 h 2",
                "falling state 1 at 1.5,70,3.5 v 0,0.5,0",
                "summon minecraft:item at 1.5,65,3.5 of minecraft:bone x4",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A full-world fill is refused with its honest column count (2^32 by
    /// 2^32), not `i64::MAX` from a wrapping multiply: still refused before
    /// any allocation.
    #[test]
    fn fill_overflow_reports_the_honest_count() {
        let (m, mut host, dir) = dead_mod(
            "fill_honest_count",
            br#"
            horizon.command("hugefill", 0, function(player, args)
              horizon.fill(-2147483648, 0, -2147483648, 2147483647, 0, 2147483647, "stone")
            end)
            "#,
        );
        let steve = host.acting_player().unwrap();
        let mut event = test_events::perform_command_event(&steve, "hugefill");
        assert_eq!(m.perform_command(&mut host, &mut event), Flow::Cancel);
        let e = hook_error_of(&m);
        assert!(e.contains("18446744073709551616"), "{e}");
        assert!(!e.contains("9223372036854775807"), "{e}");
        assert!(
            !host
                .lines
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.starts_with("edit ")),
            "no edit ran: {:?}",
            host.lines.lock().unwrap()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// NaN and infinity are refused at write-back, for a plain number and a
    /// knockback-style `{x, y, z}` table alike: the values stay as they were.
    #[test]
    fn non_finite_floats_are_refused_at_write_back() {
        let (m, _host, dir) = dead_mod(
            "non_finite_floats",
            br#"
            horizon.on("living_fall", function(event)
              if MODE == "nan" then event.distance = 0/0 end
              if MODE == "inf" then event.distance = 1/0 end
            end, "minecraft:cow")
            "#,
        );
        for mode in ["nan", "inf"] {
            m.lua.globals().set("MODE", mode).unwrap();
            let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
            assert_eq!(m.living_fall(&mut fall), Flow::Continue);
            assert_eq!(fall.distance, 4.0);
            let e = hook_error_of(&m);
            assert!(e.contains("living_fall"), "{mode}: {e}");
            assert!(e.contains("distance"), "{mode}: {e}");
        }
        // A knockback-style `{x, y, z}` table with a NaN component.
        let lua = Lua::new();
        let bad = lua.create_table().unwrap();
        bad.set("x", f64::NAN).unwrap();
        bad.set("y", 0.0).unwrap();
        bad.set("z", 0.0).unwrap();
        let err = <[f64; 3] as crate::generated_runtime::LuaStruct>::from_value(
            &lua,
            Value::Table(bad),
            "knockback_velocity",
        )
        .unwrap_err();
        assert!(err.to_string().contains("knockback_velocity"), "{err}");
        // Finite values still land.
        let good = lua.create_table().unwrap();
        good.set("x", 1.0).unwrap();
        good.set("y", 2.0).unwrap();
        good.set("z", 3.0).unwrap();
        assert_eq!(
            <[f64; 3] as crate::generated_runtime::LuaStruct>::from_value(
                &lua,
                Value::Table(good),
                "knockback_velocity",
            )
            .unwrap(),
            [1.0, 2.0, 3.0]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A numeric string that coerces to a non-finite float is refused at
    /// write-back, on the converted value: `"nan"`/`"inf"` never reach the
    /// game as a quiet NaN, for an f64 field and an f32 field alike. A
    /// numeric string that parses finite keeps working.
    #[test]
    fn coerced_string_floats_are_refused_at_write_back() {
        let (m, _host, dir) = dead_mod(
            "coerced_string_floats",
            br#"
            horizon.on("living_fall", function(event)
              if MODE == "nan" then event.distance = "nan" end
              if MODE == "inf" then event.distance = "inf" end
              if MODE == "mult_nan" then event.multiplier = "nan" end
              if MODE == "finite" then event.distance = "1.5" end
            end, "minecraft:cow")
            "#,
        );
        for (mode, field) in [
            ("nan", "distance"),
            ("inf", "distance"),
            ("mult_nan", "multiplier"),
        ] {
            m.lua.globals().set("MODE", mode).unwrap();
            let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
            m.living_fall(&mut fall);
            assert!(
                fall.distance.is_finite(),
                "{mode}: distance is {}",
                fall.distance
            );
            assert!(
                fall.multiplier.is_finite(),
                "{mode}: multiplier is {}",
                fall.multiplier
            );
            assert_eq!((fall.distance, fall.multiplier), (4.0, 1.0), "{mode}");
            let e = hook_error_of(&m);
            assert!(e.contains("living_fall"), "{mode}: {e}");
            assert!(e.contains(field), "{mode}: {e}");
        }
        // A numeric string that parses finite still lands.
        m.lua.globals().set("MODE", "finite").unwrap();
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        assert_eq!((fall.distance, fall.multiplier), (1.5, 1.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A finite Lua number that does not fit f32 is refused at write-back:
    /// `1e39` would otherwise land as infinity. The f32 values that are
    /// finite stay accepted, bit for bit (the smallest subnormal is
    /// `from_bits(1)`).
    #[test]
    fn overflowing_f32_write_back_is_refused() {
        let (m, _host, dir) = dead_mod(
            "overflowing_f32",
            br#"
            horizon.on("living_fall", function(event)
              if MODE == "huge" then event.multiplier = 1e39 end
              if MODE == "roundtrip" then event.multiplier = event.multiplier end
              if MODE == "zero" then event.multiplier = 0.0 end
              if MODE == "negzero" then event.multiplier = -0.0 end
            end, "minecraft:cow")
            "#,
        );
        m.lua.globals().set("MODE", "huge").unwrap();
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        assert!(
            fall.multiplier.is_finite(),
            "multiplier is {}",
            fall.multiplier
        );
        assert_eq!(fall.multiplier, 1.0);
        let e = hook_error_of(&m);
        assert!(e.contains("living_fall"), "{e}");
        assert!(e.contains("multiplier"), "{e}");
        // The finite f32 edges round-trip unchanged.
        for edge in [f32::MAX, f32::MIN_POSITIVE, f32::from_bits(1), 0.0] {
            m.lua.globals().set("MODE", "roundtrip").unwrap();
            // A fresh status: an error left by an earlier case would hide a
            // refusal here.
            set_status(ModStatus {
                id: m.id().into(),
                side: m.side().as_str().to_owned(),
                ..Default::default()
            });
            let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, edge);
            m.living_fall(&mut fall);
            let status_error = horizon_client::dev_input::mods::status()
                .into_iter()
                .find(|s| s.id == m.id())
                .and_then(|s| s.error);
            assert_eq!(status_error, None, "edge {edge} refused");
            assert_eq!(fall.multiplier.to_bits(), edge.to_bits(), "edge {edge}");
        }
        m.lua.globals().set("MODE", "zero").unwrap();
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        assert_eq!(fall.multiplier.to_bits(), 0.0f32.to_bits());
        m.lua.globals().set("MODE", "negzero").unwrap();
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        assert_eq!(fall.multiplier.to_bits(), (-0.0f32).to_bits());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A script that nils `debug` must not break later tracebacks: the runner
    /// captured `debug.traceback` when it was built, so a later string error
    /// still names its message and its file and line.
    #[test]
    fn handler_traceback_survives_debug_nil() {
        let (m, _host, dir) = dead_mod(
            "debug_nil",
            br#"
            horizon.on("living_fall", function(event)
              debug = nil
              error("boom-after-nil")
            end, "minecraft:cow")
            "#,
        );
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        let e = hook_error_of(&m);
        assert!(e.contains("boom-after-nil"), "{e}");
        assert!(e.contains("scripts/server/init.lua"), "{e}");
        assert!(!e.contains("error in error handling"), "{e}");
        // The traceback's first frame is the handler's own line: only the
        // wrapper's traceback (after its header) produces that frame.
        let first_frame = e
            .lines()
            .skip_while(|l| !l.contains("stack traceback:"))
            .nth(1)
            .unwrap_or("");
        assert!(
            first_frame
                .trim_start()
                .starts_with("[string \"scripts/server/init.lua\"]"),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The timers report failures only to the log, so a test-only recorder
    /// observes them here: the timer chunk captured `debug.traceback` when
    /// it was built, so a timer that fails after `debug = nil` still
    /// reports its own message.
    static TEST_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    struct TestLogger;
    impl log::Log for TestLogger {
        fn enabled(&self, _meta: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            if record.level() <= log::Level::Error {
                TEST_LOG.lock().unwrap().push(record.args().to_string());
            }
        }
        fn flush(&self) {}
    }
    /// Every test that reads `TEST_LOG` holds its guard for the whole test:
    /// `cargo test` runs them on threads of one process, and a clear in one
    /// would wipe another's records.
    static LOG_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn capture_logs() -> std::sync::MutexGuard<'static, ()> {
        let guard = LOG_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = log::set_logger(&TestLogger);
            log::set_max_level(log::LevelFilter::Error);
        });
        TEST_LOG.lock().unwrap().clear();
        guard
    }

    #[test]
    fn timer_traceback_survives_debug_nil() {
        let _logs = capture_logs();
        let (dir, path) = hmod(
            "timers-debug-nil",
            &[(
                "scripts/server/init.lua",
                br#"
                debug = nil
                horizon.after(1, function() error("timer-boom") end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        m.server_tick(
            &mut host,
            &mut test_events::server_tick_event(
                &context,
                ServerTickPhase::ServerTickEnd {
                    world_running: true,
                },
            ),
        );
        let logs = TEST_LOG.lock().unwrap().join("\n");
        assert!(logs.contains("timer-boom"), "{logs}");
        assert!(logs.contains("scripts/server/init.lua"), "{logs}");
        // The timer's traceback is formatted and capped like a handler's: its
        // first frame, after the header, is the timer's own line.
        let after_header = logs.split("stack traceback:").nth(1).unwrap_or("");
        assert!(
            after_header
                .trim_start()
                .starts_with("[string \"scripts/server/init.lua\"]"),
            "{logs}"
        );
        assert!(!logs.contains("error in error handling"), "{logs}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A non-string error names its type and still carries the handler's
    /// frames: `error({})` is reported as a table error with the handler's
    /// file and line, not the runner chunk's.
    #[test]
    fn table_errors_name_their_type_and_handler_frames() {
        let (m, _host, dir) = dead_mod(
            "table_error",
            br#"
            horizon.on("living_fall", function(event)
              error({})
            end, "minecraft:cow")
            "#,
        );
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        let e = hook_error_of(&m);
        assert!(e.contains("table"), "{e}");
        assert!(e.contains("scripts/server/init.lua"), "{e}");
        // The handler's frames lead the traceback, not the runner chunk's.
        let first_frame = e
            .lines()
            .find(|l| l.trim_start().starts_with('['))
            .unwrap_or("");
        assert!(first_frame.contains("scripts/server/init.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A script that replaces the globals the runner calls (`error`, `xpcall`,
    /// `debug`) cannot swallow a handler's failure: the failure is reported,
    /// the handler's write-back is not applied, and the traceback survives.
    #[test]
    fn script_globals_cannot_swallow_or_strip_a_handler_failure() {
        for (name, setup) in [
            ("swallow_error", "error = function() end"),
            ("xpcall_pcall", "xpcall = pcall"),
            ("debug_nil", "debug = nil"),
        ] {
            let script = format!(
                r#"
                {setup}
                horizon.on("living_fall", function(event)
                  event.distance = 99
                  missing_function()
                end, "minecraft:cow")
                "#
            );
            let (m, _host, dir) = dead_mod(name, script.as_bytes());
            let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
            m.living_fall(&mut fall);
            assert_eq!(fall.distance, 4.0, "{name}: write-back applied");
            let e = hook_error_of(&m);
            assert!(e.contains("attempt to call a nil value"), "{name}: {e}");
            assert!(e.contains("stack traceback"), "{name}: {e}");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// A script that replaces the `__tostring` of strings does not change a
    /// message: a handler's and a timer's keep their text, the traceback
    /// included, because neither is run through `tostring` on the way out.
    #[test]
    fn a_replaced_string_tostring_keeps_the_messages() {
        let _logs = capture_logs();
        let (m, mut host, dir) = dead_mod(
            "string_tostring",
            br#"
            getmetatable("").__tostring = function() return "REPLACED" end
            horizon.after(1, function() error("timer-boom") end)
            horizon.on("living_fall", function(event)
              error("handler-boom")
            end, "minecraft:cow")
            "#,
        );
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        let e = hook_error_of(&m);
        assert!(e.contains("handler-boom"), "{e}");
        assert!(e.contains("stack traceback"), "{e}");
        assert!(!e.contains("REPLACED"), "{e}");
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        m.server_tick(
            &mut host,
            &mut test_events::server_tick_event(
                &context,
                ServerTickPhase::ServerTickEnd {
                    world_running: true,
                },
            ),
        );
        let logs = TEST_LOG.lock().unwrap().join("\n");
        assert!(logs.contains("timer-boom"), "{logs}");
        assert!(!logs.contains("REPLACED"), "{logs}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A handler that raises 15000 frames deep reports in milliseconds, with
    /// a traceback of at most 20 frames and a `... N more` line: the traceback
    /// is not built from the whole stack, and no memory error comes out.
    /// Measured at 14 ms and 964 bytes (VERIFIED, nextest); the bound is loose.
    #[test]
    fn a_deep_handler_error_is_fast_and_its_traceback_capped() {
        let (m, _host, dir) = dead_mod(
            "deep_pin",
            br#"
            horizon.on("living_fall", function(event)
              local function f(n)
                if n == 0 then error("deep-boom") end
                return 1 + f(n - 1)
              end
              f(15000)
            end, "minecraft:cow")
            "#,
        );
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        let started = std::time::Instant::now();
        m.living_fall(&mut fall);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        let e = hook_error_of(&m);
        assert!(e.contains("deep-boom"), "{e}");
        assert!(e.len() < 4096, "{} bytes", e.len());
        let frames = e.lines().filter(|l| l.contains("function f")).count();
        assert_eq!(frames, 20, "{e}");
        assert!(
            e.lines()
                .any(|l| l.trim_start().starts_with("... ") && l.ends_with(" more")),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A timer that raises deep is reported like a handler: its traceback is
    /// capped to 20 frames with a `... N more` line, not the whole stack.
    #[test]
    fn timer_tracebacks_are_capped_like_handlers() {
        let _logs = capture_logs();
        let (dir, path) = hmod(
            "timer-deep",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.after(1, function()
                  local function f(n)
                    if n == 0 then error("timer-deep-boom") end
                    return 1 + f(n - 1)
                  end
                  f(15000)
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        m.server_tick(
            &mut host,
            &mut test_events::server_tick_event(
                &context,
                ServerTickPhase::ServerTickEnd {
                    world_running: true,
                },
            ),
        );
        let logs = TEST_LOG.lock().unwrap().join("\n");
        assert!(logs.contains("timer-deep-boom"), "{logs}");
        let frames = logs.lines().filter(|l| l.contains("function f")).count();
        assert_eq!(frames, 20, "{logs}");
        assert!(
            logs.lines()
                .any(|l| l.trim_start().starts_with("... ") && l.ends_with(" more")),
            "{logs}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A table error whose `__tostring` throws still reports: the message
    /// names the type, not "error in error handling".
    #[test]
    fn an_error_value_whose_tostring_throws_still_reports() {
        let (m, _host, dir) = dead_mod(
            "tostring_throws",
            br#"
            horizon.on("living_fall", function(event)
              error(setmetatable({}, {__tostring = function() error("inner") end}))
            end, "minecraft:cow")
            "#,
        );
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        let e = hook_error_of(&m);
        assert!(e.contains("(table)"), "{e}");
        assert!(!e.contains("error in error handling"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `/reload` after a script set `debug = nil` still reloads: the timer
    /// chunk runs from what the state captured at load. A reload that fails
    /// keeps the old handler firing, and a clean one registers the new.
    #[test]
    fn reload_after_debug_nil_keeps_the_handlers_firing() {
        let (dir, path) = hmod(
            "reload-debug-nil",
            &[(
                "scripts/server/init.lua",
                b"debug = nil\nTICKS = 0\nhorizon.on('server_tick', function() TICKS = TICKS + 1 end)",
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let rewrite = |src: &[u8]| {
            std::fs::write(
                &path,
                Package::write_files(MANIFEST_TEXT, None, &[("scripts/server/init.lua", src)])
                    .unwrap(),
            )
            .unwrap();
        };
        let tick = |m: &LuaMod| {
            let event = m.lua.create_table().unwrap();
            event.set("phase", "end").unwrap();
            event.set("world_running", true).unwrap();
            m.call_bare("server_tick", None, event, &mut |_, _| Ok(()))
                .unwrap();
        };
        rewrite(b"debug = nil\nhorizon.on('server_tick', function() TICKS = TICKS + 100 end)\nerror('x')");
        let e = m.reload().unwrap_err();
        assert!(e.contains("x"), "{e}");
        tick(&m);
        assert_eq!(m.lua.globals().get::<i64>("TICKS").unwrap(), 1);
        rewrite(b"debug = nil\nhorizon.on('server_tick', function() TICKS = TICKS + 10 end)");
        m.reload().unwrap();
        tick(&m);
        assert_eq!(m.lua.globals().get::<i64>("TICKS").unwrap(), 11);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A deep handler's traceback is capped to the first 20 frames with a
    /// "... N more" line.
    #[test]
    fn long_tracebacks_are_capped_at_twenty_frames() {
        let (m, _host, dir) = dead_mod(
            "deep_traceback",
            br#"
            horizon.on("living_fall", function(event)
              local function f(n)
                if n == 0 then error("deep-boom") end
                f(n - 1)
              end
              f(30)
            end, "minecraft:cow")
            "#,
        );
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        m.living_fall(&mut fall);
        let e = hook_error_of(&m);
        assert!(e.contains("deep-boom"), "{e}");
        // The traceback body: the `stack traceback:` header, then exactly 20
        // frames, then the more line (the header is not a frame).
        let lines: Vec<&str> = e.lines().collect();
        let body: Vec<&&str> = lines[1..].iter().filter(|l| !l.trim().is_empty()).collect();
        assert!(
            body.len() == 22,
            "header, 20 frames, the more line: {body:?}"
        );
        assert_eq!(body[0].trim(), "stack traceback:", "{body:?}");
        assert!(!body[1..21].iter().any(|l| l.contains("more")), "{body:?}");
        assert!(body[21].contains("more"), "{body:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A failing handler's error names the hook and the Lua file and line,
    /// whether Lua raised it or a Rust call inside the handler did: the log,
    /// the console and the status row all read this text.
    #[test]
    fn handler_errors_name_the_file_and_line() {
        let (m, _host, dir) = dead_mod(
            "handler_traceback",
            br#"
            horizon.on("living_fall", function(event)
              if MODE == "boom" then error("boom") end
              horizon.set_distance(1)
            end, "minecraft:cow")
            "#,
        );
        for mode in ["boom", "rust"] {
            m.lua.globals().set("MODE", mode).unwrap();
            let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
            assert_eq!(m.living_fall(&mut fall), Flow::Continue);
            let e = hook_error_of(&m);
            // The first line names the hook and the message; a traceback with
            // the handler's file and line rides along, indented under it.
            let mut lines = e.lines();
            let first = lines.next().unwrap_or("");
            assert!(first.contains("living_fall"), "{mode}: {e}");
            // A Lua-raised message carries its file and line on the first line.
            if mode == "boom" {
                assert!(first.contains("scripts/server/init.lua"), "{mode}: {e}");
            }
            let rest: Vec<&str> = lines.collect();
            assert!(
                rest.iter().any(|l| l.contains("stack traceback")),
                "{mode}: {e}"
            );
            assert!(
                rest.iter()
                    .any(|l| l.starts_with("  ") && l.contains("scripts/server/init.lua")),
                "{mode}: {e}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The `/lua` console reports a failing chunk and a failing tick handler
    /// with the chunk's name and the Lua line.
    #[test]
    fn console_errors_name_the_chunk_and_line() {
        use horizon_client::dev_input::console as lua_console;
        lua_console::clear();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        let mut tick = |console: &crate::console::Console| {
            console.server_tick(
                &mut host,
                &mut test_events::server_tick_event(
                    &context,
                    ServerTickPhase::ServerTickEnd {
                        world_running: true,
                    },
                ),
            )
        };
        let console = crate::console::Console;
        lua_console::submit("lua1", "error('boom')");
        tick(&console);
        let lines = lua_console::lines();
        assert!(
            lines
                .iter()
                .any(|(k, t)| *k == lua_console::Kind::Err && t.contains("lua1")),
            "{lines:?}"
        );
        lua_console::submit("lua2", "mc.on_tick(function() error('tick-boom') end)");
        tick(&console);
        // The window keeps one entry per line, so join the error entries.
        let lines = lua_console::lines();
        let errs: Vec<&str> = lines
            .iter()
            .filter(|(k, _)| *k == lua_console::Kind::Err)
            .map(|(_, t)| t.as_str())
            .collect();
        let joined = errs.join("\n");
        assert!(joined.contains("tick-boom"), "{joined}");
        assert!(joined.contains("stack traceback"), "{joined}");
        // The handler's own file and line rides in the traceback, not just
        // the message's first line.
        assert!(joined.matches("lua2").count() >= 2, "{joined}");
    }

    /// A load-time failure names its file and line through `--check-mod`.
    #[test]
    fn a_load_error_reports_its_file_and_line_through_check_mod() {
        let (dir, path) = hmod(
            "load-boom",
            &[("scripts/server/init.lua", b"error('boom')")],
        );
        let e = check_mod(&path, &[], None).unwrap_err();
        assert!(e.contains("scripts/server/init.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A host whose summon tells the mod of the spawn at once, as the game
    /// does: the hook runs nested inside the handler that summoned.
    struct ReentrantHost {
        lines: Mutex<Vec<String>>,
        m: Option<Arc<LuaMod>>,
        world: horizon_client::mod_api::Attachments,
        summoned_goals: Mutex<Vec<String>>,
    }
    impl Host for ReentrantHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }
        fn attachments(
            &mut self,
            holder: Holder,
        ) -> horizon_client::mod_api::AttachmentResult<&mut horizon_client::mod_api::Attachments>
        {
            match holder {
                Holder::Level => Ok(&mut self.world),
                _ => Err(horizon_client::mod_api::AttachmentError::MissingHolder),
            }
        }
        fn acting_player(&self) -> Option<horizon_client::mod_api::NativeServerPlayer> {
            Some(test_events::native_server_player(
                7,
                "steve".into(),
                0,
                "minecraft:overworld".into(),
                [1.0, 64.0, 1.0],
                0,
                20.0,
                0.0,
                0.0,
                0,
                String::new(),
            ))
        }
        fn held(&self) -> Option<(String, u32, bool)> {
            None
        }
        fn held_stack(&self, _: u128) -> Option<ItemStack> {
            None
        }
        fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
            self.acting_player().into_iter().collect()
        }
        fn block(&self, pos: [i32; 3]) -> String {
            if pos == [1, 2, 3] {
                "minecraft:stone".into()
            } else {
                "minecraft:air".into()
            }
        }
        fn broadcast(&self, text: horizon_client::mod_api::Text) {
            self.lines.lock().unwrap().push(text.plain());
        }
        fn summon(&mut self, _: &str, kind: &str, pos: [f64; 3], _: Compound) -> Option<u32> {
            let m = self.m.clone().unwrap();
            // As the game: the mob's goals are set up (no host), then the spawn is told.
            let mut mob = horizon_client::mobs::Mob::new(
                42,
                horizon_client::mobs::MobType::cow_for_tests(),
                horizon_core::geometry::Vec3::new(pos[0], pos[1], pos[2]),
                0.0,
            );
            m.mob_ai_register(&mut test_events::mob_ai_register_event(&mut mob));
            *self.summoned_goals.lock().unwrap() = mob
                .ai
                .goals
                .entries()
                .map(|(_, name, _, _)| name.to_owned())
                .collect();
            m.mob_spawned(
                self,
                &mut test_events::mob_spawned_event("minecraft:overworld", kind, pos, false),
            );
            Some(42)
        }
    }

    #[test]
    fn a_hook_fired_inside_a_handler_keeps_the_outer_calls_host_and_actions() {
        let (dir, path) = hmod(
            "reentrant",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.goal("idle", { can_use = function() return false end })
                horizon.on("mob_ai_register", function(event) horizon.add_goal(1, "idle") end)
                horizon.on("mob_spawned", function(event)
                  horizon.broadcast("spawned " .. event.kind .. " at " .. event.pos.x)
                  SPAWN_READ = horizon.block(1, 2, 3)
                end, "ruby:crawler")
                horizon.command("nest", 0, function(player, args)
                  horizon.broadcast("before")
                  local id = horizon.summon(player.dimension, "ruby:crawler", 5, 64, 5)
                  horizon.broadcast("after " .. tostring(id))
                  AFTER_READ = horizon.block(1, 2, 3)
                end)
                horizon.block_entity("ruby:hive", { ticks = true, data = { bred = 0 } })
                horizon.on("block_entity_tick", function(event)
                  local id = horizon.summon(event.dimension, "ruby:crawler", event.pos.x + 0.5, event.pos.y + 1, event.pos.z + 0.5)
                  event.entity.data.bred = event.entity.data.bred + 1
                  local w = horizon.world_data()
                  TICK_READ = horizon.block(1, 2, 3) .. " " .. tostring(id) .. " " .. tostring(w ~= nil)
                end, "ruby:hive")
                "#,
            )],
        );
        let m = Arc::new(LuaMod::load(&path, LuaSide::Server, &[]).unwrap());
        let mut host = ReentrantHost {
            lines: Mutex::new(Vec::new()),
            m: Some(m.clone()),
            world: Default::default(),
            summoned_goals: Mutex::new(Vec::new()),
        };
        let steve = host.acting_player().unwrap();
        assert_eq!(
            m.perform_command(
                &mut host,
                &mut test_events::perform_command_event(&steve, "nest"),
            ),
            Flow::Cancel
        );
        // The inner hook applied only what it queued, inside the summon; the
        // outer handler's lines came after it, and its reads still worked.
        assert_eq!(
            *host.lines.lock().unwrap(),
            ["spawned ruby:crawler at 5", "before", "after 42"]
        );
        // The nested summon's `mob_ai_register` attached the idle goal: a
        // no-op `add_goal` leaves this empty.
        let summoned: Vec<String> = host.summoned_goals.lock().unwrap().clone();
        assert!(summoned.contains(&"ruby:idle".to_owned()), "{summoned:?}");
        let g = m.lua.globals();
        assert_eq!(g.get::<String>("SPAWN_READ").unwrap(), "minecraft:stone");
        assert_eq!(g.get::<String>("AFTER_READ").unwrap(), "minecraft:stone");
        // The same from a block entity's tick, with its data written back after.
        let mut entity = horizon_client::mod_api::ModBlockEntity::default();
        entity.id = "ruby:hive".into();
        entity.nbt = m
            .block_entity(&mut host, &mut test_events::block_entity_event("ruby:hive"))
            .unwrap()
            .1;
        host.lines.lock().unwrap().clear();
        m.block_entity_tick(
            &mut host,
            &mut test_events::block_entity_tick_event([1, 2, 3], &mut entity),
        );
        assert_eq!(*host.lines.lock().unwrap(), ["spawned ruby:crawler at 1.5"]);
        assert_eq!(
            g.get::<String>("TICK_READ").unwrap(),
            "minecraft:stone 42 true"
        );
        assert_eq!(
            entity
                .nbt
                .get(&horizon_world::nbt::JavaString::from("bred")),
            Some(&horizon_world::nbt::Tag::Int(1))
        );
        host.m = None;
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_loot_player_and_menu_hooks_reach_their_handlers() {
        let (dir, path) = hmod(
            "events",
            &[(
                "scripts/server/init.lua",
                br#"
                local function say(line) horizon.broadcast(line) end
                -- The hooks without a host (mid-hit) have no broadcast: they note.
                NOTES = {}
                local function note(line) NOTES[#NOTES + 1] = line end
                horizon.on("player_tick", function(event)
                  __witness = (__witness or 0) + 1
                  -- A single-player host: `players()` holds only the owner,
                  -- so this does not identify the ticking player.
                  local player = horizon.players()[1]
                  say("tick " .. player.name .. " hp " .. player.health)
                end)
                horizon.on("player_chat", function(event)
                  __witness = (__witness or 0) + 1
                  note("chat " .. event.name .. " " .. event.message)
                  if event.message == "shh" then horizon.cancel() else event.replies = { "heard " .. event.message } end
                end)
                horizon.on("player_command", function(event)
                  __witness = (__witness or 0) + 1
                  say("command " .. event.player.name .. " " .. event.command)
                  if event.command == "secret" then event.replies = { "not here" }; horizon.cancel() end
                end)
                horizon.on("loot_drops", function(event)
                  say(("loot %s %s at %d,%d,%d by %s tool %s"):format(event.table, event.block.id, event.block.x, event.block.y, event.block.z, event.player and event.player.name or "nobody", event.tool and event.tool.id or "hand"))
                  event.drops[1].count = 2
                  event.drops[#event.drops + 1] = { id = "diamond", count = 1 }
                end, "minecraft:stone")
                horizon.on("loot_drops", function(event)
                  say(("loot %s mob %d %s"):format(event.table, event.entity.id, event.entity.kind))
                  event.drops[1] = nil
                end, "minecraft:cow")
                horizon.on("player_changed_dimension", function(event)
                  __witness = (__witness or 0) + 1
                  say("moved " .. event.player.name .. " " .. event.from .. " > " .. event.to)
                end)
                horizon.on("player_bed_enter", function(event)
                  __witness = (__witness or 0) + 1
                  say(("bed %s %d,%d,%d"):format(event.player.name, event.pos.x, event.pos.y, event.pos.z)); horizon.cancel()
                end)
                horizon.on("player_exp_change", function(event)
                  __witness = (__witness or 0) + 1
                  say("exp " .. event.amount); event.amount = event.amount * 2
                end)
                horizon.on("player_container_open", function(event)
                  __witness = (__witness or 0) + 1
                  say("menu " .. event.menu .. " " .. (event.pos and event.pos.x or "nowhere") .. " in " .. event.dimension)
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = host.acting_player().unwrap();
        m.player_tick(&mut host, &mut test_events::player_tick_event());
        let mut replies = Vec::new();
        assert_eq!(
            m.player_chat(&mut test_events::player_chat_event(
                steve.uuid,
                "steve",
                "hello all",
                &mut replies
            )),
            Flow::Continue
        );
        assert_eq!(
            replies,
            [horizon_client::mod_api::Text::literal("heard hello all")]
        );
        let mut replies = Vec::new();
        assert_eq!(
            m.player_chat(&mut test_events::player_chat_event(
                steve.uuid,
                "steve",
                "shh",
                &mut replies
            )),
            Flow::Cancel
        );
        let mut replies = Vec::new();
        let mut cmd =
            test_events::player_command_event(steve.uuid, "steve", "secret", &mut replies);
        assert_eq!(m.player_command(&mut host, &mut cmd), Flow::Cancel);
        assert_eq!(
            replies.iter().map(|t| t.plain()).collect::<Vec<_>>(),
            ["not here"]
        );
        let mut cmd =
            test_events::player_command_event(steve.uuid, "steve", "time set day", &mut replies);
        assert_eq!(m.player_command(&mut host, &mut cmd), Flow::Continue);
        // A block's loot, changed in place: the count, and a diamond added.
        let mut drops = vec![ItemStack {
            item: "minecraft:cobblestone".into(),
            count: 1,
            damage: 0,
            extra: None,
        }];
        let pick = wand();
        m.loot_drops(
            &mut host,
            &mut test_events::loot_drops_event(
                &test_events::native_loot_drops(
                    "minecraft:blocks/stone",
                    "minecraft:overworld",
                    [1.5, 2.5, 3.5],
                    Some(([1, 2, 3], BlockStateId(1))),
                    None,
                    Some(steve.uuid),
                    Some(&pick),
                    None,
                    false,
                ),
                &mut drops,
            ),
        );
        assert_eq!(
            drops
                .iter()
                .map(|s| (s.item.to_string(), s.count))
                .collect::<Vec<_>>(),
            [
                ("minecraft:cobblestone".to_owned(), 2),
                ("minecraft:diamond".to_owned(), 1)
            ]
        );
        // A mob's loot, emptied.
        let mut drops = vec![ItemStack {
            item: "minecraft:leather".into(),
            count: 1,
            damage: 0,
            extra: None,
        }];
        m.loot_drops(
            &mut host,
            &mut test_events::loot_drops_event(
                &test_events::native_loot_drops(
                    "minecraft:entities/cow",
                    "minecraft:overworld",
                    [2.0, 64.0, 3.0],
                    None,
                    Some((7, "minecraft:cow")),
                    None,
                    None,
                    None,
                    false,
                ),
                &mut drops,
            ),
        );
        assert!(drops.is_empty());
        m.player_changed_dimension(
            &mut host,
            &mut test_events::player_changed_dimension_event(
                steve.uuid,
                "steve",
                "minecraft:overworld",
                "minecraft:the_nether",
            ),
        );
        assert_eq!(
            m.player_bed_enter(
                &mut host,
                &mut test_events::player_bed_enter_event(steve.uuid, "steve", [4, 5, 6])
            ),
            Flow::Cancel
        );
        let mut exp = test_events::player_exp_change_event(steve.uuid, "steve", 3);
        m.player_exp_change(&mut host, &mut exp);
        assert_eq!(exp.amount, 6);
        m.player_container_open(
            &mut host,
            &mut test_events::player_container_open_event(
                steve.uuid,
                "steve",
                "minecraft:overworld",
                "minecraft:generic_9x3",
                Some([7, 8, 9]),
            ),
        );
        // A menu with no block: `pos` is nil, and the dimension is told.
        m.player_container_open(
            &mut host,
            &mut test_events::player_container_open_event(
                steve.uuid,
                "steve",
                "minecraft:overworld",
                "minecraft:crafting",
                None,
            ),
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "tick steve hp 20",
                "command steve secret",
                "command steve time set day",
                "loot minecraft:blocks/stone minecraft:stone at 1,2,3 by steve tool ruby:wand",
                "loot minecraft:entities/cow mob 7 minecraft:cow",
                "moved steve minecraft:overworld > minecraft:the_nether",
                "bed steve 4,5,6",
                "exp 3",
                "menu minecraft:generic_9x3 7 in minecraft:overworld",
                "menu minecraft:crafting nowhere in minecraft:overworld",
            ]
        );
        assert_eq!(m.lua.globals().get::<f64>("__witness").unwrap(), 10.0);
        assert_eq!(
            m.lua.globals().get::<Vec<String>>("NOTES").unwrap(),
            ["chat steve hello all", "chat steve shh"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn crafted_dropped_hurt_start_impact_launch_and_detonation_reach_their_handlers() {
        let (dir, path) = hmod(
            "item-projectile-explosion",
            &[(
                "scripts/server/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                local function say(line) horizon.broadcast(line) end
                -- No kind filter any more: every craft wakes the handler, which
                -- keeps its own recipe, and the player is uuid text resolved
                -- through the host read.
                horizon.on("item_crafted", function(event)
                  if event.recipe ~= "minecraft:oak_planks" then return end
                  __witness = (__witness or 0) + 1
                  local player = horizon.player(event.player)
                  log(("crafted %s %s x%d by %s"):format(event.recipe, event.stack.id, event.stack.count, player.name))
                end)
                horizon.on("item_dropped", function(event)
                  if event.item ~= "minecraft:stick" then return end
                  __witness = (__witness or 0) + 1
                  local player = horizon.player(event.player)
                  log("dropped " .. event.item .. " x" .. event.count .. " by " .. player.name)
                end)
                -- The projectile kind still filters; the hit's block id comes
                -- from the host read, as before.
                horizon.on("projectile_impact", function(event)
                  __witness = (__witness or 0) + 1
                  local where = event.entity and ("entity " .. event.entity .. " " .. event.entity_kind) or ("block " .. horizon.block(event.block.x, event.block.y, event.block.z) .. " face " .. event.face.y)
                  say(("impact %d %s by %d on %s"):format(event.projectile, event.kind, event.owner, where))
                  if event.entity then horizon.cancel() end
                end, "minecraft:snowball")
                horizon.on("projectile_launch", function(event)
                  __witness = (__witness or 0) + 1
                  say(("launched %d %s by %s in %s"):format(event.entity, event.kind, event.player and event.player.name or "nobody", horizon.time().dimension))
                end, "minecraft:snowball")
                -- The detonation gains the dimension; the blocks write back.
                horizon.on("explosion_detonate", function(event)
                  __witness = (__witness or 0) + 1
                  say(("explosion at %.0f,%.0f,%.0f breaking %d in %s"):format(event.center.x, event.center.y, event.center.z, #event.blocks, event.dimension))
                  table.remove(event.blocks, 1)
                end)
                -- The foe's entity kind filters; the held stack arrives, and
                -- the player comes from the host read.
                horizon.on("hurt_enemy", function(event)
                  if event.stack.id ~= "ruby:wand" then return end
                  __witness = (__witness or 0) + 1
                  local player = horizon.players()[1]
                  say(("landed %s on %d %s by %s"):format(event.stack.id, event.entity, event.kind, player.name))
                end, "minecraft:cow")
                -- A player alone starts this use: a bare handler, an in-Lua
                -- item filter, and the duration writes back.
                horizon.on("living_entity_use_item_start", function(event)
                  if event.stack.id ~= "ruby:wand" then return end
                  __witness = (__witness or 0) + 1
                  say(("use start %s %s %d"):format(event.stack.id, event.hand, event.duration))
                  event.duration = 10
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = host.acting_player().unwrap();
        let planks = ItemStack {
            item: "minecraft:oak_planks".into(),
            count: 4,
            damage: 0,
            extra: None,
        };
        m.item_crafted(
            &mut host,
            &mut test_events::item_crafted_event(steve.uuid, "minecraft:oak_planks", &planks),
        );
        m.item_dropped(
            &mut host,
            &mut test_events::item_dropped_event(steve.uuid, "minecraft:stick", 2),
        );
        // The flag answers from the sibling's registry: a snowball handler
        // hears impacts.
        let mut hears = test_events::hears_projectile_impacts_event(false);
        m.hears_projectile_impacts(&host, &mut hears);
        assert!(hears.hears);
        let mut impact = test_events::projectile_impact_event(
            "minecraft:overworld",
            30,
            "minecraft:snowball",
            5,
            Some(7),
            "minecraft:cow",
            [0, 0, 0],
            [0, 0, 0],
            [2.0, 64.5, 3.0],
        );
        assert_eq!(m.projectile_impact(&mut host, &mut impact), Flow::Cancel);
        impact.entity = None;
        impact.block = [1, 2, 3];
        impact.face = [0, 1, 0];
        assert_eq!(m.projectile_impact(&mut host, &mut impact), Flow::Continue);
        m.projectile_launch(
            &mut host,
            &mut test_events::projectile_launch_event(
                Some(steve.uuid),
                "steve",
                30,
                "minecraft:snowball",
            ),
        );
        // A dispenser's launch names nobody.
        m.projectile_launch(
            &mut host,
            &mut test_events::projectile_launch_event(None, "", 31, "minecraft:snowball"),
        );
        let mut blocks = vec![[1, 2, 3], [1, 3, 3]];
        m.explosion_detonate(
            &mut host,
            &mut test_events::explosion_detonate_event(
                "minecraft:overworld",
                [1.0, 2.0, 3.0],
                &mut blocks,
            ),
        );
        assert_eq!(blocks, [[1, 3, 3]]);
        m.hurt_enemy(
            &mut host,
            &mut test_events::hurt_enemy_event(&wand(), 7, "minecraft:cow"),
        );
        let stick = wand();
        let mut start = test_events::living_entity_use_item_start_event(
            &stick,
            None,
            horizon_client::first_person::Hand::Off,
            32,
        );
        assert_eq!(
            m.living_entity_use_item_start(&mut host, &mut start),
            Flow::Continue
        );
        assert_eq!(start.duration, 10);
        let log: Vec<String> = m
            .lua
            .globals()
            .get::<Table>("LOG")
            .unwrap()
            .sequence_values()
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(
            log,
            [
                "crafted minecraft:oak_planks minecraft:oak_planks x4 by steve",
                "dropped minecraft:stick x2 by steve",
            ]
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "impact 30 minecraft:snowball by 5 on entity 7 minecraft:cow",
                "impact 30 minecraft:snowball by 5 on block minecraft:stone face 1",
                "launched 30 minecraft:snowball by steve in minecraft:overworld",
                "launched 31 minecraft:snowball by nobody in minecraft:overworld",
                "explosion at 1,2,3 breaking 2 in minecraft:overworld",
                "landed ruby:wand on 7 minecraft:cow by steve",
                "use start ruby:wand off 32",
            ]
        );
        assert_eq!(m.lua.globals().get::<f64>("__witness").unwrap(), 9.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_mob_fall_heal_knockback_hurt_equip_and_join_hooks_reach_their_handlers() {
        let (dir, path) = hmod(
            "mob-events",
            &[(
                "scripts/server/init.lua",
                br#"
                local function say(line) horizon.broadcast(line) end
                -- The hooks without a host (mid-hit) have no broadcast: they note.
                NOTES = {}
                local function note(line) NOTES[#NOTES + 1] = line end
                horizon.on("living_incoming_damage", function(event)
                  note(("hurt %d %s %s %.1f"):format(event.entity, event.kind, event.damage_type, event.amount)); event.amount = event.amount / 2
                end)
                horizon.on("living_fall", function(event)
                  note(("fall %s %.1f x%.1f"):format(event.kind, event.distance, event.multiplier)); event.distance = 1.5
                end, "minecraft:cow")
                horizon.on("living_fall", function(event) note("player fall " .. event.distance); horizon.cancel() end, "minecraft:player")
                horizon.on("living_heal", function(event) note("heal " .. event.entity .. " " .. event.amount); event.amount = event.amount + 1 end)
                horizon.on("living_knockback", function(event)
                  note(("knockback %s %s %.1f %.1f %.1f"):format(event.kind, event.damage_type, event.strength, event.x, event.z)); event.strength = 0.25
                end, "minecraft:cow")
                horizon.on("living_equipment_change", function(event)
                  say(("equipped %d %s %s > %s"):format(event.entity, event.slot, event.from and event.from.id or "none", event.to and event.to.id or "none"))
                end, "minecraft:cow")
                horizon.on("entity_join_level", function(event)
                  say(("joined %d %s at %.0f loaded=%s"):format(event.entity, event.kind, event.pos.x, tostring(event.loaded_from_disk))); if event.entity == 99 then horizon.cancel() end
                end, "minecraft:arrow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let mut hit = test_events::living_incoming_damage_event(
            5,
            "minecraft:player",
            "minecraft:fall",
            -1,
            -1,
            6.0,
        );
        assert_eq!(m.living_incoming_damage(&mut hit), Flow::Continue);
        assert_eq!(hit.amount, 3.0);
        let mut fall = test_events::living_fall_event(7, "minecraft:cow", 4.0, 1.0);
        assert_eq!(m.living_fall(&mut fall), Flow::Continue);
        assert_eq!(fall.distance, 1.5);
        let mut fall = test_events::living_fall_event(5, "minecraft:player", 9.0, 1.0);
        assert_eq!(m.living_fall(&mut fall), Flow::Cancel);
        let mut heal = test_events::living_heal_event(7, 2.0);
        assert_eq!(m.living_heal(&mut heal), Flow::Continue);
        assert_eq!(heal.amount, 3.0);
        let mut knock = test_events::living_knockback_event(
            7,
            "minecraft:cow",
            "minecraft:player_attack",
            5,
            5,
            0.4,
            1.0,
            0.0,
        );
        assert_eq!(m.living_knockback(&mut knock), Flow::Continue);
        assert_eq!(knock.strength, 0.25);
        // The equipment query is answered from the change hook's registry:
        // a registered change handler counts as listening.
        let mut listened = test_events::living_equipment_listened_event(false);
        m.living_equipment_listened(&mut host, &mut listened);
        assert!(listened.listened);
        let sword = ItemStack {
            item: "minecraft:iron_sword".into(),
            count: 1,
            damage: 0,
            extra: None,
        };
        m.living_equipment_change(
            &mut host,
            &mut test_events::living_equipment_change_event(
                7,
                "minecraft:cow",
                "mainhand",
                None,
                Some(&sword),
            ),
        );
        assert_eq!(
            m.entity_join_level(
                &mut host,
                &mut test_events::entity_join_level_event(
                    "minecraft:overworld",
                    99,
                    "minecraft:arrow",
                    [1.0, 64.0, 1.0],
                    false
                )
            ),
            Flow::Cancel
        );
        assert_eq!(
            m.entity_join_level(
                &mut host,
                &mut test_events::entity_join_level_event(
                    "minecraft:overworld",
                    100,
                    "minecraft:arrow",
                    [2.0, 64.0, 2.0],
                    true
                )
            ),
            Flow::Continue
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "equipped 7 mainhand none > minecraft:iron_sword",
                "joined 99 minecraft:arrow at 1 loaded=false",
                "joined 100 minecraft:arrow at 2 loaded=true",
            ]
        );
        assert_eq!(
            m.lua.globals().get::<Vec<String>>("NOTES").unwrap(),
            [
                "hurt 5 minecraft:player minecraft:fall 6.0",
                "fall minecraft:cow 4.0 x1.0",
                "player fall 9",
                "heal 7 2",
                "knockback minecraft:cow minecraft:player_attack 0.4 1.0 0.0",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_mob_damage_tick_kill_and_click_hooks_reach_their_kind_handlers() {
        let (dir, path) = hmod(
            "mob-hooks",
            &[(
                "scripts/server/init.lua",
                br#"
                HURT = {}
                horizon.on("living_incoming_damage", function(event)
                  HURT[#HURT + 1] = event.kind .. ":" .. event.damage_type
                  HURT_READ = pcall(horizon.block, 1, 2, 3)
                  if event.damage_type == "minecraft:on_fire" then horizon.cancel() end
                  event.amount = event.amount * 2
                end, "minecraft:cow")
                TICKS = 0
                horizon.on("entity_tick_post", function(event)
                  TICKS = TICKS + 1
                  if TICKS == 2 then horizon.set_mob_health(event.dimension, event.entity, horizon.mob(event.entity).max_health - 1) end
                  local seen = horizon.mob(event.entity)
                  if seen ~= nil and seen.position.x ~= 2 then error("view") end
                end, "minecraft:cow")
                horizon.on("mob_killed", function(event)
                  horizon.broadcast("killed " .. event.kind .. " " .. event.entity)
                end, "minecraft:cow")
                horizon.on("interact_entity", function(event)
                  local mob = horizon.mob(event.entity)
                  if mob ~= nil and mob.health ~= 10 then error("view") end
                  horizon.broadcast("block " .. horizon.block(1, 2, 3) .. "," .. horizon.block(0, 0, 0))
                  horizon.broadcast("facing " .. (horizon.block_property(1, 2, 3, "facing") or "none"))
                  horizon.broadcast("players " .. #horizon.players() .. ":" .. horizon.players()[1].name .. ":" .. (horizon.player("steve") and "found" or "lost") .. ":" .. (horizon.player("alex") and "found" or "lost"))
                  local t = horizon.time()
                  horizon.broadcast("time " .. t.day .. ":" .. t.time_of_day .. ":" .. t.rain .. ":" .. t.dimension)
                  horizon.broadcast("mobs " .. #horizon.mobs() .. ":" .. horizon.mob(7).max_health .. ":" .. tostring(horizon.mob(8)))
                  local hit = horizon.raycast("steve", 8)
                  horizon.broadcast("hit " .. hit.id .. ":" .. hit.block.y .. ":" .. hit.x .. ":" .. tostring(horizon.raycast("alex", 8)))
                  KEPT = horizon.block
                  if event.stack and event.stack.id == "minecraft:wheat" then horizon.cancel() end
                end, "minecraft:cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // A hurt zombie is nobody's business; the cow's hit is doubled, fire refused.
        let mut hit = test_events::living_incoming_damage_event(
            9,
            "minecraft:zombie",
            "minecraft:mob_attack",
            1,
            1,
            3.0,
        );
        assert_eq!(m.living_incoming_damage(&mut hit), Flow::Continue);
        assert_eq!(hit.amount, 3.0);
        hit.kind = "minecraft:cow";
        assert_eq!(m.living_incoming_damage(&mut hit), Flow::Continue);
        assert_eq!(hit.amount, 6.0);
        hit.damage_type = "minecraft:on_fire";
        assert_eq!(m.living_incoming_damage(&mut hit), Flow::Cancel);
        let hurt: Vec<String> = m
            .lua
            .globals()
            .get::<Table>("HURT")
            .unwrap()
            .sequence_values()
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(
            hurt,
            [
                "minecraft:cow:minecraft:mob_attack",
                "minecraft:cow:minecraft:on_fire"
            ]
        );
        // The mod listens to entity ticks: it has an entity_tick_post handler.
        // No host mid-hit: the reads refuse there.
        assert!(!m.lua.globals().get::<bool>("HURT_READ").unwrap());
        let mut listened = test_events::entity_tick_listened_event(false);
        m.entity_tick_listened(&mut NoHost, &mut listened);
        assert!(listened.listened);
        // Ticks count per firing; the second one sets the mob's health.
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        for _ in 0..3 {
            m.entity_tick_post(
                &mut host,
                &mut test_events::entity_tick_post_event("minecraft:overworld", 7, "minecraft:cow"),
            );
        }
        // A mob the host has no view of is told all the same now: one hook
        // and no view check, where the old mob hook stayed silent.
        m.entity_tick_post(
            &mut host,
            &mut test_events::entity_tick_post_event("minecraft:overworld", 8, "minecraft:cow"),
        );
        assert_eq!(host.health, [(7, 9.0)]);
        assert_eq!(m.lua.globals().get::<i64>("TICKS").unwrap(), 4);
        // A skeleton's tick is nobody's business: the kind filter stays quiet.
        m.entity_tick_post(
            &mut host,
            &mut test_events::entity_tick_post_event(
                "minecraft:overworld",
                7,
                "minecraft:skeleton",
            ),
        );
        assert_eq!(m.lua.globals().get::<i64>("TICKS").unwrap(), 4);
        // A click without wheat passes through; wheat is taken.
        assert_eq!(
            m.interact_entity(
                &mut host,
                &mut test_events::interact_entity_event(7, "minecraft:cow", None)
            ),
            Flow::Continue
        );
        let wheat = horizon_client::mod_api::ItemStack {
            item: "minecraft:wheat".into(),
            count: 1,
            damage: 0,
            extra: None,
        };
        assert_eq!(
            m.interact_entity(
                &mut host,
                &mut test_events::interact_entity_event(7, "minecraft:cow", Some(&wheat))
            ),
            Flow::Cancel
        );
        // A click on a mob the host has no view of (dead, gone) is told all
        // the same now: one hook and no view check, where the old mob hook
        // stayed silent — so wheat is taken there too.
        assert_eq!(
            m.interact_entity(
                &mut host,
                &mut test_events::interact_entity_event(8, "minecraft:cow", Some(&wheat))
            ),
            Flow::Cancel
        );
        // The reads answered from the host during the clicks; a read kept
        // past the call is refused.
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "block minecraft:stone,minecraft:air",
                "facing north",
                "players 1:steve:found:lost",
                "time 1:6000:0.5:minecraft:overworld",
                "mobs 1:10:nil",
                "hit minecraft:dirt:5:4.5:nil",
                "block minecraft:stone,minecraft:air",
                "facing north",
                "players 1:steve:found:lost",
                "time 1:6000:0.5:minecraft:overworld",
                "mobs 1:10:nil",
                "hit minecraft:dirt:5:4.5:nil",
                "block minecraft:stone,minecraft:air",
                "facing north",
                "players 1:steve:found:lost",
                "time 1:6000:0.5:minecraft:overworld",
                "mobs 1:10:nil",
                "hit minecraft:dirt:5:4.5:nil",
            ]
        );
        let kept: Function = m.lua.globals().get("KEPT").unwrap();
        let e = kept.call::<String>((1, 2, 3)).unwrap_err().to_string();
        assert!(e.contains("inside a hook with a host"), "{e}");
        // The kill reaches the host; a zombie's death is nobody's business.
        m.mob_killed(
            &mut host,
            &mut test_events::mob_killed_event(1, 7, "minecraft:cow"),
        );
        m.mob_killed(
            &mut host,
            &mut test_events::mob_killed_event(1, 9, "minecraft:zombie"),
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "block minecraft:stone,minecraft:air",
                "facing north",
                "players 1:steve:found:lost",
                "time 1:6000:0.5:minecraft:overworld",
                "mobs 1:10:nil",
                "hit minecraft:dirt:5:4.5:nil",
                "block minecraft:stone,minecraft:air",
                "facing north",
                "players 1:steve:found:lost",
                "time 1:6000:0.5:minecraft:overworld",
                "mobs 1:10:nil",
                "hit minecraft:dirt:5:4.5:nil",
                "block minecraft:stone,minecraft:air",
                "facing north",
                "players 1:steve:found:lost",
                "time 1:6000:0.5:minecraft:overworld",
                "mobs 1:10:nil",
                "hit minecraft:dirt:5:4.5:nil",
                "killed minecraft:cow 7",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reload_reads_the_scripts_again_and_forgets_the_old_handlers() {
        let (dir, path) = hmod(
            "reload",
            &[
                ("scripts/server/init.lua", b"VERSION = require('v')\nhorizon.on('server_tick', function() end)\nhorizon.on('server_tick', function() end)"),
                ("scripts/server/v.lua", b"return 1"),
            ],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let handlers = |m: &LuaMod| -> i64 {
            let h: Table = m.lua.named_registry_value(HANDLERS).unwrap();
            h.get::<Option<Table>>("server_tick")
                .unwrap()
                .map_or(0, |l| l.len().unwrap())
        };
        let rewrite = |files: &[(&str, &[u8])]| {
            std::fs::write(
                &path,
                Package::write_files(MANIFEST_TEXT, None, files).unwrap(),
            )
            .unwrap();
        };
        // Two of the script's, after the timers' own.
        assert_eq!(handlers(&m), 3);
        // The mod's file changes under it: one handler that counts the
        // ticks, a timer, a command, a goal, a new module value.
        rewrite(&[
            (
                "scripts/server/init.lua",
                br#"
                VERSION = require('v')
                TICKS = 0
                horizon.on('server_tick', function() TICKS = TICKS + 1 end)
                horizon.every(1, function() TIMED = (TIMED or 0) + 1 end)
                horizon.command("ruby", 0, function() end)
                horizon.goal("sit", { can_use = function() return false end })
                "#,
            ),
            ("scripts/server/v.lua", b"return 2"),
        ]);
        // The first data load is the one the scripts were loaded for; the next reloads.
        let mut files = Vec::new();
        m.data_pack_files(&NoHost, &mut test_events::data_pack_files_event(&mut files));
        assert_eq!(handlers(&m), 3);
        m.data_pack_files(&NoHost, &mut test_events::data_pack_files_event(&mut files));
        assert_eq!(handlers(&m), 2);
        assert_eq!(m.lua.globals().get::<i64>("VERSION").unwrap(), 2);
        // The tick handlers hear an event table now, not a bare pair.
        let tick = |m: &LuaMod| {
            let event = m.lua.create_table().unwrap();
            event.set("phase", "end").unwrap();
            event.set("world_running", true).unwrap();
            m.call_bare("server_tick", None, event, &mut |_, _| Ok(()))
                .unwrap();
        };
        tick(&m);
        let counts = |m: &LuaMod| -> (i64, i64) {
            let g = m.lua.globals();
            (g.get("TICKS").unwrap(), g.get("TIMED").unwrap())
        };
        assert_eq!(counts(&m), (1, 1));
        // A broken edit leaves the running handlers, timers, commands and
        // goals alone, whether the script fails before or after registering
        // anything: the half-registered handler is not kept either.
        rewrite(&[("scripts/server/init.lua", b"error('x')")]);
        let e = m.reload().unwrap_err();
        assert!(e.contains("x"), "{e}");
        rewrite(&[(
            "scripts/server/init.lua",
            b"horizon.on('server_tick', function() HALF = true end)\nhorizon.command('half', 0, function() end)\nrequire('missing')",
        )]);
        let e = m.reload().unwrap_err();
        assert!(e.contains("missing"), "{e}");
        assert_eq!(handlers(&m), 2);
        tick(&m);
        assert_eq!(counts(&m), (2, 2));
        assert!(m.lua.globals().get::<Value>("HALF").unwrap().is_nil());
        assert_eq!(
            m.commands
                .lock()
                .unwrap()
                .iter()
                .map(|c| c.usage.clone())
                .collect::<Vec<_>>(),
            ["ruby"]
        );
        assert!(m.goals.lock().unwrap().contains_key("sit"));
        // The timers registered by the old script keep running from the old
        // list, and a timer the old handlers add later joins that list.
        m.lua
            .load("horizon.after(1, function() LATER = true end)")
            .exec()
            .unwrap();
        tick(&m);
        assert_eq!(counts(&m), (3, 3));
        assert!(m.lua.globals().get::<bool>("LATER").unwrap());
        // The next good edit is live again.
        rewrite(&[(
            "scripts/server/init.lua",
            b"horizon.on('server_tick', function() end)",
        )]);
        m.reload().unwrap();
        assert_eq!(handlers(&m), 2);
        tick(&m);
        assert_eq!(counts(&m), (3, 3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_handler_that_never_returns_is_stopped_and_the_state_goes_on() {
        let (dir, path) = hmod(
            "budget",
            &[(
                "scripts/server/init.lua",
                b"N = 0\n\
                  horizon.on('server_tick', function() while true do end end)\n\
                  horizon.on('world_lifecycle', function() N = N + 1 end)",
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // The tick handlers hear an event table now, not a bare pair.
        let tick = m.lua.create_table().unwrap();
        tick.set("phase", "end").unwrap();
        tick.set("world_running", true).unwrap();
        let started = std::time::Instant::now();
        let e = m
            .call_bare("server_tick", None, tick, &mut |_, _| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(e.contains("ran over"), "{e}");
        assert!(
            started.elapsed() < HANDLER_BUDGET * 4,
            "{:?}",
            started.elapsed()
        );
        // The state is fine afterwards, and the next chunk has a budget of its own.
        let cycle = m.lua.create_table().unwrap();
        cycle.set("phase", "start").unwrap();
        cycle.set("error_text", "").unwrap();
        m.call_bare("world_lifecycle", None, cycle, &mut |_, _| Ok(()))
            .unwrap();
        assert_eq!(m.lua.globals().get::<i64>("N").unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_pcall_around_a_slow_loop_does_not_defeat_the_budget() {
        let (dir, path) = hmod(
            "budget-pcall",
            &[(
                "scripts/server/init.lua",
                br#"
                N = 0
                -- The first loop's error is caught; the second loop is stopped
                -- all the same, as is a loop that keeps catching.
                horizon.on('mob_spawned', function()
                  pcall(function() while true do end end)
                  while true do end
                end)
                horizon.on('mob_killed', function()
                  while true do pcall(function() while true do end end) end
                end)
                -- The timers' own pcall catches the first timer's error; the
                -- timers after it get no budget of their own.
                horizon.after(1, function() while true do end end)
                horizon.after(1, function() while true do end end)
                horizon.on('world_lifecycle', function() N = N + 1 end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // The tick handlers hear an event table now, not a bare pair: the
        // table is what runs the timers, so the tick's overrun is the
        // timers' (they share its budget).
        let tick_event = || {
            let event = m.lua.create_table().unwrap();
            event.set("phase", "end").unwrap();
            event.set("world_running", true).unwrap();
            event
        };
        for hook in ["mob_spawned", "mob_killed"] {
            let started = std::time::Instant::now();
            let e = m
                .call_bare(hook, None, ("end", true), &mut |_, _| Ok(()))
                .unwrap_err()
                .to_string();
            assert!(e.contains("ran over"), "{hook}: {e}");
            assert!(
                started.elapsed() < HANDLER_BUDGET * 4,
                "{hook}: {:?}",
                started.elapsed()
            );
        }
        let started = std::time::Instant::now();
        let e = m
            .call_bare("server_tick", None, tick_event(), &mut |_, _| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(e.contains("ran over"), "server_tick: {e}");
        assert!(
            started.elapsed() < HANDLER_BUDGET * 4,
            "server_tick: {:?}",
            started.elapsed()
        );
        // The host slot and the queue are back as they were, and the next
        // call has a budget of its own.
        let slot: Table = m.lua.named_registry_value(HOST_SLOT).unwrap();
        assert!(slot.get::<Value>(1).unwrap().is_nil());
        assert_eq!(slot.get::<Option<i64>>(3).unwrap().unwrap_or(0), 0);
        let queue: Table = m.lua.named_registry_value(QUEUE).unwrap();
        assert_eq!(queue.raw_len(), 0);
        let event = m.lua.create_table().unwrap();
        event.set("phase", "start").unwrap();
        event.set("error_text", "").unwrap();
        m.call_bare("world_lifecycle", None, event, &mut |_, _| Ok(()))
            .unwrap();
        assert_eq!(m.lua.globals().get::<i64>("N").unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A handler fired N times reports N calls for that hook, none for a
    /// hook that never fired, and no overruns for clean runs.
    #[test]
    fn profiler_counts_calls_per_mod_and_hook() {
        let _lock = crate::profiler::test_lock();
        let (m, _host, dir) = dead_mod(
            "profile_calls",
            br#"
            horizon.on("mob_spawned", function() end)
            horizon.on("mob_killed", function() end)
            "#,
        );
        for _ in 0..3 {
            m.call_bare("mob_spawned", None, ("end", true), &mut |_, _| Ok(()))
                .unwrap();
        }
        let rows = crate::profiler::profile_report();
        let fired = rows
            .iter()
            .find(|r| r.r#mod == "profile_calls" && r.hook == "mob_spawned")
            .expect("the fired hook is reported");
        assert_eq!(fired.calls, 3, "{fired:?}");
        assert_eq!(fired.overruns, 0, "{fired:?}");
        assert!(
            rows.iter()
                .all(|r| !(r.r#mod == "profile_calls" && r.hook == "mob_killed")),
            "a hook that never fired has no row: {rows:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A handler spinning past a tiny budget reports one call, one overrun,
    /// and a max time at least the budget: the same interrupt the budget
    /// tests trip, on a small scale.
    #[test]
    fn profiler_counts_an_overrun_past_a_tiny_budget() {
        let _lock = crate::profiler::test_lock();
        let (m, _host, dir) = dead_mod(
            "profile_overrun",
            br#"horizon.on("mob_spawned", function() while true do end end)"#,
        );
        let limit = Duration::from_millis(50);
        let e = m
            .run_handlers_with("mob_spawned", None, Reader::None, ("end", true), limit)
            .unwrap_err()
            .to_string();
        assert!(e.contains("ran over"), "{e}");
        let rows = crate::profiler::profile_report();
        let row = rows
            .iter()
            .find(|r| r.r#mod == "profile_overrun" && r.hook == "mob_spawned")
            .expect("the stopped call is reported");
        assert_eq!(row.calls, 1, "{row:?}");
        assert_eq!(row.overruns, 1, "{row:?}");
        assert!(
            row.max_ns >= limit.as_nanos() as u64,
            "the call ran past its budget: {row:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two mods hearing the same hook are reported as two rows, each with
    /// its own count.
    #[test]
    fn profiler_reports_two_mods_separately() {
        let _lock = crate::profiler::test_lock();
        let (a, _ha, dir_a) = dead_mod(
            "profile_mod_a",
            br#"horizon.on("mob_spawned", function() end)"#,
        );
        let (b, _hb, dir_b) = dead_mod(
            "profile_mod_b",
            br#"horizon.on("mob_spawned", function() end)"#,
        );
        for _ in 0..2 {
            a.call_bare("mob_spawned", None, ("end", true), &mut |_, _| Ok(()))
                .unwrap();
        }
        b.call_bare("mob_spawned", None, ("end", true), &mut |_, _| Ok(()))
            .unwrap();
        let rows = crate::profiler::profile_report();
        let row_a = rows
            .iter()
            .find(|r| r.r#mod == "profile_mod_a" && r.hook == "mob_spawned")
            .expect("mod a is reported");
        let row_b = rows
            .iter()
            .find(|r| r.r#mod == "profile_mod_b" && r.hook == "mob_spawned")
            .expect("mod b is reported");
        assert_eq!(row_a.calls, 2, "{row_a:?}");
        assert_eq!(row_b.calls, 1, "{row_b:?}");
        std::fs::remove_dir_all(&dir_a).unwrap();
        std::fs::remove_dir_all(&dir_b).unwrap();
    }

    /// `profile_reset` drops the report: no row of the mod survives it.
    #[test]
    fn profiler_reset_zeroes_everything() {
        let _lock = crate::profiler::test_lock();
        let (m, _host, dir) = dead_mod(
            "profile_reset_me",
            br#"horizon.on("mob_spawned", function() end)"#,
        );
        m.call_bare("mob_spawned", None, ("end", true), &mut |_, _| Ok(()))
            .unwrap();
        assert!(
            crate::profiler::profile_report()
                .iter()
                .any(|r| r.r#mod == "profile_reset_me"),
            "the call is reported before the reset"
        );
        crate::profiler::profile_reset();
        assert!(
            crate::profiler::profile_report()
                .iter()
                .all(|r| r.r#mod != "profile_reset_me"),
            "no row of the mod survives the reset"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn loot_drops_keeps_same_id_stacks_truncates_on_nil_and_appends() {
        let (dir, path) = hmod(
            "loot-writeback",
            &[(
                "scripts/server/init.lua",
                br#"
                RAN = 0
                horizon.on("loot_drops", function(event)
                  RAN = RAN + 1
                  -- The same id at the same place keeps the stack: the count edits.
                  event.drops[1].count = 5
                  -- A different id replaces the stack wholesale (plain).
                  event.drops[2] = { id = "minecraft:diamond", count = 2 }
                  -- A zero count drops the stack, nil or not.
                  event.drops[3].count = 0
                  -- Nil stops the list: the fourth seed stack drops out.
                  event.drops[4] = nil
                  -- And a new stack joins at the end.
                  event.drops[#event.drops + 1] = { id = "minecraft:stick", count = 3 }
                end, "minecraft:stone")
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let steve = host.acting_player().unwrap();
        let pick = wand();
        // Cobble and dirt carry components (dirt also damage), gold and
        // the feather are plain.
        let mut drops = vec![
            ItemStack {
                item: "minecraft:cobblestone".into(),
                count: 1,
                damage: 0,
                extra: Some(Box::new(horizon_client::item_data::StackData {
                    repair_cost: 7,
                    ..Default::default()
                })),
            },
            ItemStack {
                item: "minecraft:dirt".into(),
                count: 1,
                damage: 5,
                extra: Some(Box::new(horizon_client::item_data::StackData {
                    repair_cost: 9,
                    ..Default::default()
                })),
            },
            ItemStack {
                item: "minecraft:gold_ingot".into(),
                count: 1,
                damage: 0,
                extra: None,
            },
            ItemStack {
                item: "minecraft:feather".into(),
                count: 1,
                damage: 0,
                extra: None,
            },
        ];
        m.loot_drops(
            &mut host,
            &mut test_events::loot_drops_event(
                &test_events::native_loot_drops(
                    "minecraft:blocks/stone",
                    "minecraft:overworld",
                    [1.5, 2.5, 3.5],
                    Some(([1, 2, 3], BlockStateId(1))),
                    None,
                    Some(steve.uuid),
                    Some(&pick),
                    None,
                    false,
                ),
                &mut drops,
            ),
        );
        assert_eq!(m.lua.globals().get::<i64>("RAN").unwrap(), 1);
        assert_eq!(drops.len(), 3);
        // Same id, same place: the stack (and its repair cost) is kept.
        assert_eq!(drops[0].item.to_string(), "minecraft:cobblestone");
        assert_eq!(drops[0].count, 5);
        assert_eq!(drops[0].extra.as_ref().unwrap().repair_cost, 7);
        // Different id: a plain new stack, without the dirt's damage or
        // repair cost.
        assert_eq!(drops[1].item.to_string(), "minecraft:diamond");
        assert_eq!(drops[1].count, 2);
        assert_eq!(drops[1].damage, 0);
        assert!(drops[1].extra.is_none());
        // The zero count cut the gold out, the nil the feather; the stick
        // joined after the diamond.
        assert_eq!(drops[2].item.to_string(), "minecraft:stick");
        assert_eq!(drops[2].count, 3);
        // Neither a break nor a death: the handler stays quiet and the
        // drops are untouched.
        let mut drops = vec![ItemStack {
            item: "minecraft:cobblestone".into(),
            count: 1,
            damage: 0,
            extra: None,
        }];
        m.loot_drops(
            &mut host,
            &mut test_events::loot_drops_event(
                &test_events::native_loot_drops(
                    "test:empty",
                    "minecraft:overworld",
                    [0.0, 64.0, 0.0],
                    None,
                    None,
                    None,
                    None,
                    None,
                    false,
                ),
                &mut drops,
            ),
        );
        assert_eq!(m.lua.globals().get::<i64>("RAN").unwrap(), 1);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].count, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn loot_drops_absent_both_stays_silent_for_unfiltered_handlers() {
        let (dir, path) = hmod(
            "loot-absent-both",
            &[(
                "scripts/server/init.lua",
                br#"
                RAN = 0
                horizon.on("loot_drops", function(event)
                  RAN = RAN + 1
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        // Neither a break nor a death: no block and no entity names a kind,
        // so even a handler with no kind stays quiet and the drops stand.
        let mut drops = vec![ItemStack {
            item: "minecraft:cobblestone".into(),
            count: 2,
            damage: 1,
            extra: None,
        }];
        m.loot_drops(
            &mut host,
            &mut test_events::loot_drops_event(
                &test_events::native_loot_drops(
                    "test:empty",
                    "minecraft:overworld",
                    [0.0, 64.0, 0.0],
                    None,
                    None,
                    None,
                    None,
                    None,
                    false,
                ),
                &mut drops,
            ),
        );
        assert_eq!(m.lua.globals().get::<i64>("RAN").unwrap(), 0);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item.to_string(), "minecraft:cobblestone");
        assert_eq!(drops[0].count, 2);
        assert_eq!(drops[0].damage, 1);
        assert!(drops[0].extra.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn data_pack_files_appends_in_load_order_after_the_packs_files() {
        let (dir, path) = hmod(
            "pack-order",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("data_pack_files", function(event)
                  assert(event.files[1].path == "data/test/pack.json", "pack first")
                  event.files[#event.files + 1] = { path = "data/test/mod.json", data = "m" }
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // The world's own pack file is already there when the mod appends.
        let mut files = vec![("data/test/pack.json".to_owned(), b"p".to_vec())];
        m.data_pack_files(&NoHost, &mut test_events::data_pack_files_event(&mut files));
        assert_eq!(
            files,
            [
                ("data/test/pack.json".to_owned(), b"p".to_vec()),
                ("data/test/mod.json".to_owned(), b"m".to_vec()),
            ]
        );
        // Every call collects, including the second (which reloads first):
        // the mod appends again, after what is already there.
        m.data_pack_files(&NoHost, &mut test_events::data_pack_files_event(&mut files));
        assert_eq!(
            files,
            [
                ("data/test/pack.json".to_owned(), b"p".to_vec()),
                ("data/test/mod.json".to_owned(), b"m".to_vec()),
                ("data/test/mod.json".to_owned(), b"m".to_vec()),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_action_outside_a_hooks_handlers_is_an_error_and_leaves_nothing_queued() {
        use horizon_client::mod_api::NativeBehaviour;
        // From a goal function: no runner takes the action, so it is refused
        // where it is called, not left in the queue for the state's life.
        let (dir, path) = hmod(
            "goal-action",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.goal("summoner", {
                  can_use = function(mob, state, out)
                    local ok, e = pcall(horizon.summon, "minecraft:overworld", "minecraft:cow", 0, 0, 0)
                    SUMMON = tostring(e)
                    return ok
                  end,
                })
                -- Inside a handler the same call is taken.
                horizon.on("mob_spawned", function() horizon.summon("minecraft:overworld", "minecraft:cow", 0, 0, 0) end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let def = m.goals.lock().unwrap().get("summoner").cloned().unwrap();
        let behaviour = def
            .behaviour(m.lua.clone(), m.id(), m.budget.clone())
            .unwrap();
        let view = MobView::of(
            &horizon_client::mobs::Mob::new(
                7,
                horizon_client::mobs::MobType::cow_for_tests(),
                horizon_core::geometry::Vec3::new(0.0, 64.0, 0.0),
                0.0,
            ),
            None,
        );
        let mut state = goals::LuaGoalState::default();
        let mut out = horizon_client::mod_api::Actions::default();
        assert!(!behaviour.can_use(&mut state, &view, &mut out));
        let e: String = m.lua.globals().get("SUMMON").unwrap();
        assert!(e.contains("horizon.summon") && e.contains("handler"), "{e}");
        let queue: Table = m.lua.named_registry_value(QUEUE).unwrap();
        assert_eq!(queue.raw_len(), 0);
        let mut seen = Vec::new();
        m.call_bare(
            "mob_spawned",
            None,
            ("d", "minecraft:cow"),
            &mut |action, _| {
                seen.push(action.to_owned());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, ["summon"]);
        std::fs::remove_dir_all(&dir).unwrap();
        // At init.lua's top level the same: the load fails, naming the call.
        let (dir, path) = hmod(
            "load-action",
            &[("scripts/server/init.lua", b"horizon.broadcast('hello')")],
        );
        let e = LuaMod::load(&path, LuaSide::Server, &[]).unwrap_err();
        assert!(e.contains("horizon.broadcast"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_later_handlers_error_drops_the_actions_and_the_data_edits_alike() {
        let (dir, path) = hmod(
            "half",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("mob_spawned", function()
                  local w = horizon.world_data()
                  w.spawns = (w.spawns or 0) + 1
                  horizon.broadcast("spawned")
                end)
                horizon.on("mob_spawned", function(dimension, kind) if kind == "minecraft:zombie" then error("boom") end end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let spawns = |host: &mut MobHost| -> Option<i64> {
            let host: &mut dyn Host = host;
            let c = m
                .world_attachment
                .get_existing(host, Holder::Level)
                .unwrap()?;
            match c.get(&"spawns".into()) {
                Some(horizon_world::nbt::Tag::Int(n)) => Some(i64::from(*n)),
                other => panic!("{other:?}"),
            }
        };
        let seen = std::cell::RefCell::new(Vec::new());
        let mut apply = |host: &mut dyn Host, action: &str, args: &Table| {
            seen.borrow_mut().push(action.to_owned());
            m.run_action(host, action, args)
        };
        // The second handler fails: neither the first's broadcast nor its
        // count reaches the world.
        let e = m
            .call_kind(
                "mob_spawned",
                None,
                &mut host,
                ("d", "minecraft:zombie"),
                &mut apply,
            )
            .unwrap_err()
            .to_string();
        assert!(e.contains("boom"), "{e}");
        assert!(seen.borrow().is_empty());
        assert_eq!(spawns(&mut host), None);
        assert!(host.lines.lock().unwrap().is_empty());
        // When every handler runs, both do.
        m.call_kind(
            "mob_spawned",
            None,
            &mut host,
            ("d", "minecraft:cow"),
            &mut apply,
        )
        .unwrap();
        assert_eq!(*seen.borrow(), ["broadcast"]);
        assert_eq!(spawns(&mut host), Some(1));
        assert_eq!(*host.lines.lock().unwrap(), ["spawned"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mods_load_after_what_they_depend_on_and_without_it_not_at_all() {
        let c = |id: &str, depends: &[&str]| Candidate {
            id: id.into(),
            version: "0.6.0".into(),
            path: PathBuf::from(id),
            scripted: true,
            depends: depends.iter().map(|d| d.to_string()).collect(),
        };
        let on = [
            c("parasite", &["modkit >= 0.6"]),
            c("modkit", &[]),
            c("lost", &["nowhere"]),
            c("a", &["b"]),
            c("b", &["a"]),
            c("leaf", &["lost"]),
            c("newer", &["modkit >= 0.7"]),
        ];
        let steps = load_order(&on);
        let order: Vec<(&str, Option<&str>)> = steps
            .iter()
            .map(|s| (on[s.index].id.as_str(), s.missing.as_deref()))
            .collect();
        // modkit before parasite; lost names its missing dependency, leaf
        // fails with it; the cycle fails naming a member.
        let at = |id: &str| order.iter().position(|(i, _)| *i == id).unwrap();
        assert!(at("modkit") < at("parasite"));
        assert_eq!(order[at("parasite")], ("parasite", None));
        assert_eq!(order[at("modkit")], ("modkit", None));
        assert!(order.contains(&("lost", Some("nowhere"))));
        assert!(order.contains(&("leaf", Some("lost"))));
        assert!(order.contains(&("a", Some("b"))) || order.contains(&("a", Some("a"))));
        // A least version not met: told with what was found.
        assert!(order.contains(&("newer", Some("modkit >= 0.7 (found 0.6.0)"))));
        assert_eq!(steps.len(), on.len());
    }

    #[test]
    fn require_reaches_a_librarys_modules_by_dotted_name() {
        let (lib_dir, lib) = hmod(
            "lib",
            &[
                ("scripts/server/init.lua", b"LIB_RAN = true"),
                (
                    "scripts/server/modkit.lua",
                    b"local m = { VERSION = '1', entity = require('modkit.entity') }\nreturn m",
                ),
                (
                    "scripts/server/modkit/entity/init.lua",
                    b"return { kind = 'entity' }",
                ),
            ],
        );
        let (dir, path) = hmod(
            "user",
            &[
                ("scripts/server/init.lua", b"local modkit = require('@modkit')\nV = modkit.VERSION .. modkit.entity.kind\nMINE = require('mine')\nSAME = require('modkit/entity') == modkit.entity and require('modkit.entity') == modkit.entity"),
                ("scripts/server/mine.lua", b"return 'own'"),
                // The mod's own module wins a shared name.
                ("scripts/server/modkit/entity.lua", b"return { kind = 'own entity' }"),
            ],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[("lib".into(), lib.clone())]).unwrap();
        assert_eq!(m.lua.globals().get::<String>("V").unwrap(), "1own entity");
        assert_eq!(m.lua.globals().get::<String>("MINE").unwrap(), "own");
        // `@modkit/entity` and `modkit/entity` name the same module as `modkit.entity`.
        assert!(m.lua.globals().get::<bool>("SAME").unwrap());
        // A library's init.lua runs in the library's own state, not here.
        assert!(
            m.lua
                .globals()
                .get::<Option<bool>>("LIB_RAN")
                .unwrap()
                .is_none()
        );
        // Without the library, the first require fails and names the search.
        let e = LuaMod::load(&path, LuaSide::Server, &[]).unwrap_err();
        assert!(
            e.contains("scripts/server/modkit.lua") && e.contains("depends on"),
            "{e}"
        );
        // A bad name is refused before any lookup.
        let (bad_dir, bad) = hmod(
            "badname",
            &[("scripts/server/init.lua", b"require('../x')")],
        );
        assert!(
            LuaMod::load(&bad, LuaSide::Server, &[])
                .unwrap_err()
                .contains("for folders")
        );
        // The registry script's modules are not the scripts' to run.
        let (reg_dir, reg) = hmod(
            "registry-module",
            &[
                ("scripts/server/init.lua", b"require('@registry/items')"),
                ("scripts/registry/items.lua", b"return 1"),
            ],
        );
        let e = LuaMod::load(&reg, LuaSide::Server, &[]).unwrap_err();
        assert!(e.contains("scripts/registry/") && e.contains("once"), "{e}");
        for d in [lib_dir, dir, bad_dir, reg_dir] {
            std::fs::remove_dir_all(&d).unwrap();
        }
    }

    #[test]
    fn a_block_entity_declared_in_lua_is_made_ticked_and_keeps_its_data() {
        use horizon_world::nbt::Tag;
        let (dir, path) = hmod(
            "block-entity",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.block_entity("ruby:hive", { ticks = true, data = { charge = 0, name = "x" } })
                horizon.block_entity("ruby:lamp")
                horizon.on("block_entity_tick", function(event)
                  assert(event.entity.id == "ruby:hive", "id")
                  event.entity.data.charge = event.entity.data.charge + 1
                  event.entity.data.seen = event.dimension .. ":" .. event.pos.x .. "," .. event.pos.y .. "," .. event.pos.z
                  event.entity.data.name = nil
                end)
                horizon.on("random_tick", function(event)
                  if event.data then event.data.charge = event.data.charge * 10; event.data.under = horizon.block(event.pos.x, event.pos.y - 1, event.pos.z) end
                  if pcall(horizon.players) then error("a level hook reads blocks only") end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mut host = NoHost;
        let key = |k: &str| horizon_world::nbt::JavaString::from(k);
        // A declared block gets a block entity with its starting data; others none.
        let (id, data) = m
            .block_entity(&mut host, &mut test_events::block_entity_event("ruby:hive"))
            .unwrap();
        assert_eq!(id, "ruby:hive");
        assert_eq!(data.get(&key("charge")), Some(&Tag::Int(0)));
        assert_eq!(data.get(&key("name")), Some(&Tag::String("x".into())));
        let (_, lamp) = m
            .block_entity(&mut host, &mut test_events::block_entity_event("ruby:lamp"))
            .unwrap();
        assert!(lamp.is_empty());
        assert!(
            m.block_entity(
                &mut host,
                &mut test_events::block_entity_event("ruby:other")
            )
            .is_none()
        );
        // Only the hive ticks.
        let mut ticks = test_events::block_entity_ticks_event("ruby:hive", false);
        m.block_entity_ticks(&host, &mut ticks);
        assert!(ticks.ticks);
        let mut ticks = test_events::block_entity_ticks_event("ruby:lamp", false);
        m.block_entity_ticks(&host, &mut ticks);
        assert!(!ticks.ticks);
        // The tick handler changes the data, which is written back: the int
        // stays an int, the removed key is gone, the new one is there.
        let mut entity = horizon_client::mod_api::ModBlockEntity::default();
        entity.id = "ruby:hive".into();
        entity.nbt = data;
        m.block_entity_tick(
            &mut host,
            &mut test_events::block_entity_tick_event([1, 2, 3], &mut entity),
        );
        m.block_entity_tick(
            &mut host,
            &mut test_events::block_entity_tick_event([1, 2, 3], &mut entity),
        );
        assert_eq!(entity.nbt.get(&key("charge")), Some(&Tag::Int(2)));
        assert_eq!(
            entity.nbt.get(&key("seen")),
            Some(&Tag::String("minecraft:overworld:1,2,3".into()))
        );
        assert!(!entity.nbt.contains_key(&key("name")));
        // A level hook sees the block entity's data and its changes are stored.
        let mut level = FakeLevel {
            block: "ruby:hive",
            props: Vec::new(),
            writes: Vec::new(),
            ticks: Vec::new(),
            summons: Vec::new(),
            data: HashMap::from([([1, 64, -3], entity.nbt.clone())]),
        };
        m.random_tick(
            &mut level,
            &mut test_events::random_tick_event(
                LevelDimension::Overworld,
                BlockPos::new(1, 64, -3),
                BlockStateId(7),
            ),
        );
        assert_eq!(
            level.data[&[1, 64, -3]].get(&key("charge")),
            Some(&Tag::Int(20))
        );
        assert_eq!(
            level.data[&[1, 64, -3]].get(&key("under")),
            Some(&Tag::String("ruby:hive".into()))
        );
        // A block with no mod entity gets `data` as nil, and the firing
        // writes nothing back: the data map gains no entry.
        m.random_tick(
            &mut level,
            &mut test_events::random_tick_event(
                LevelDimension::Overworld,
                BlockPos::new(9, 9, 9),
                BlockStateId(7),
            ),
        );
        assert!(!level.data.contains_key(&[9, 9, 9]));
        // A second position and the entity's own id are told as dispatched:
        // a hardcoded pos or id in the arm fails this firing.
        m.block_entity_tick(
            &mut host,
            &mut test_events::block_entity_tick_event([7, 8, 9], &mut entity),
        );
        assert_eq!(entity.nbt.get(&key("charge")), Some(&Tag::Int(3)));
        assert_eq!(
            entity.nbt.get(&key("seen")),
            Some(&Tag::String("minecraft:overworld:7,8,9".into()))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A hook name the runtime does not know is refused when the script
    /// loads, with the file and line, and the headless check says so; a mod
    /// that loads is listed with its scripts and libraries.
    #[test]
    fn an_unknown_hook_name_fails_the_load_and_the_headless_check_reports_it() {
        let (dir, path) = hmod(
            "unknown-hook",
            &[(
                "scripts/server/init.lua",
                b"horizon.on(\"player_logged_in\", function() end)\nhorizon.on(\"player_jioned\", function() end)\n",
            )],
        );
        let e = LuaMod::load(&path, LuaSide::Server, &[]).unwrap_err();
        assert!(
            e.contains("scripts/server/init.lua\"]:2:")
                && e.contains("no such hook")
                && e.contains("player_jioned")
                && e.contains("player_logged_in")
                && e.contains("server_tick"),
            "{e}"
        );
        let e = check_mod(&path, &[], None).unwrap_err();
        assert!(
            e.contains(":2:")
                && e.contains("player_jioned")
                && e.contains("player_logged_in")
                && e.contains("server_tick"),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        let (dir, path) = hmod(
            "fine",
            &[
                (
                    "scripts/server/init.lua",
                    b"local m = require(\"helper\")\nhorizon.on(\"player_logged_in\", m.hello)\n",
                ),
                (
                    "scripts/server/helper.lua",
                    b"return { hello = function() end }\n",
                ),
            ],
        );
        let lines = check_mod(&path, &[], None).unwrap();
        assert_eq!(
            lines,
            [
                "ruby: [server] scripts loaded",
                "  scripts/server/helper.lua",
                "  scripts/server/init.lua"
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A mod names the least version of a library it needs: the headless
    /// check refuses an older one and says what it found, and takes a newer
    /// one, whose version the mod reads from `horizon.libraries`.
    #[test]
    fn a_dependency_s_least_version_is_checked_and_the_versions_are_read() {
        let dir = std::env::temp_dir().join(format!("horizon-lua-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = |id: &str, version: &str, depends: &str| {
            format!(
                "id = \"{id}\"\nname = \"{id}\"\nversion = \"{version}\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"scripts\"]\nexperimental = [\"scripts\"]\ndepends = [{depends}]\n"
            )
        };
        let write = |name: &str, manifest: String, init: &[u8]| {
            let path = dir.join(name);
            std::fs::write(
                &path,
                Package::write_files(&manifest, None, &[("scripts/server/init.lua", init)])
                    .unwrap(),
            )
            .unwrap();
            path
        };
        let old = write("lib-old.hmod", manifest("lib", "1.0.0", ""), b"");
        let new = write("lib-new.hmod", manifest("lib", "2.1.0", ""), b"");
        let user = write(
            "user.hmod",
            manifest("user", "0.3.0", "\"lib >= 2.0\""),
            b"MINE = horizon.mod_version\nLIB = horizon.libraries.lib\n",
        );
        let e = check_mod(&user, std::slice::from_ref(&old), None).unwrap_err();
        assert!(
            e.contains("lib >= 2.0") && e.contains("found lib 1.0.0"),
            "{e}"
        );
        let lines = check_mod(&user, std::slice::from_ref(&new), None).unwrap();
        assert!(
            lines.iter().any(|l| l.contains("with lib from")),
            "{lines:?}"
        );
        let m = LuaMod::load(&user, LuaSide::Server, &[("lib".into(), new)]).unwrap();
        assert_eq!(m.lua.globals().get::<String>("MINE").unwrap(), "0.3.0");
        assert_eq!(m.lua.globals().get::<String>("LIB").unwrap(), "2.1.0");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_developer_tools_see_a_mods_status_and_reload_it() {
        let (dir, path) = hmod(
            "status",
            &[(
                "scripts/server/init.lua",
                b"horizon.goal('stalk', { can_use = function() return false end })\n\
                  horizon.on('server_tick', function() end)\n\
                  horizon.on('mob_spawned', function() error('oops') end)",
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        m.publish(None);
        let of = |m: &LuaMod| {
            horizon_client::dev_input::mods::status()
                .into_iter()
                .find(|s| s.id == m.id())
                .unwrap()
        };
        let s = of(&m);
        assert_eq!((s.scripts, s.reloads), (1, 0));
        assert_eq!(s.goals, ["stalk"]);
        assert_eq!(
            s.handlers,
            [
                ("mob_spawned".to_string(), 1),
                ("server_tick".to_string(), 1)
            ]
        );
        assert!(s.error.is_none());
        // The goal's name for F3 and /goals says whose it is.
        assert_eq!(
            m.goals.lock().unwrap()["stalk"].name,
            format!("{}:stalk", m.id())
        );
        // A handler's error is the status's last error.
        if let Err(e) = m.call_bare("mob_spawned", None, (), &mut |action, _| {
            m.warn(format!("mob_spawned: {action} needs a host; dropped"));
            Ok(())
        }) {
            m.error(format!("mob_spawned: {e}"));
        }
        assert!(of(&m).error.unwrap().contains("oops"));
        // The console asks for a reload, by id or for everyone: each is served once.
        assert!(!m.reload_asked());
        horizon_client::dev_input::mods::request_reload(Some(m.id()), None);
        assert!(m.reload_asked() && !m.reload_asked());
        horizon_client::dev_input::mods::request_reload(None, None);
        assert!(m.reload_asked() && !m.reload_asked());
        m.reload_now("the test");
        let s = of(&m);
        assert_eq!(s.reloads, 1);
        assert!(s.error.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reload_now_holds_the_turn_across_reload_and_publish() {
        let (dir, path) = hmod(
            "reload-turn",
            &[(
                "scripts/server/init.lua",
                br#"horizon.on("server_tick", function() end)"#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        m.publish(None);
        let of = |m: &LuaMod| {
            horizon_client::dev_input::mods::status()
                .into_iter()
                .find(|s| s.id == m.id())
                .unwrap()
        };
        let before = of(&m).reloads;
        // A firing started from another thread during the reload is
        // serialised: reload_now holds the turn until publish returns.
        // Modelled on turn::tests::two_threads_serialise: the holder
        // keeps the turn for 50 ms, reload_now waits for it.
        let waited = std::thread::scope(|s| {
            let m_ref = &m;
            let h = s.spawn(move || {
                let _g = m_ref.turn.try_acquire_timeout(turn::TURN_TIMEOUT).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(50));
            });
            std::thread::sleep(std::time::Duration::from_millis(10));
            let t0 = std::time::Instant::now();
            m.reload_now("the test");
            let waited = t0.elapsed();
            h.join().unwrap();
            waited
        });
        assert!(
            waited >= std::time::Duration::from_millis(35),
            "reload_now was serialised behind the holder: {waited:?}"
        );
        let s = of(&m);
        assert_eq!(s.reloads, before + 1, "the status store shows the reload");
        assert!(s.error.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_module_or_a_failing_script_is_an_error_with_the_file_named() {
        let (dir, path) = hmod(
            "missing",
            &[("scripts/server/init.lua", b"require('nothing')")],
        );
        let e = LuaMod::load(&path, LuaSide::Server, &[]).unwrap_err();
        assert!(
            e.contains(
                "no scripts/server/nothing.lua or scripts/shared/nothing.lua in the mod or the mods it depends on"
            ),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();

        let (dir, path) = hmod("broken", &[("scripts/server/init.lua", b"error('boom')")]);
        let e = LuaMod::load(&path, LuaSide::Server, &[]).unwrap_err();
        assert!(
            e.contains("boom") && e.contains("scripts/server/init.lua"),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A host whose acting player, steve, is in `game_mode` with operator
    /// `permission`, and holds the inspect stick. As in a guest's turn, the
    /// world's other players are out of reach: `level_players` is empty.
    struct StickHost {
        game_mode: i32,
        permission: u8,
    }
    impl Host for StickHost {
        fn dimension(&self) -> &'static str {
            "minecraft:the_nether"
        }
        fn attachments(
            &mut self,
            _: Holder,
        ) -> horizon_client::mod_api::AttachmentResult<&mut horizon_client::mod_api::Attachments>
        {
            Err(horizon_client::mod_api::AttachmentError::MissingHolder)
        }
        fn acting_player(&self) -> Option<horizon_client::mod_api::NativeServerPlayer> {
            Some(test_events::native_server_player(
                0x123e4567_e89b_12d3_a456_426614174000,
                "steve".into(),
                self.permission,
                "minecraft:the_nether".into(),
                [1.0, 64.0, 1.0],
                self.game_mode,
                20.0,
                0.0,
                0.0,
                5,
                String::new(),
            ))
        }
        fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
            Vec::new()
        }
        fn day_time(&self) -> i64 {
            30000
        }
        fn weather(&self) -> (f32, f32) {
            (0.5, 0.0)
        }
        fn held(&self) -> Option<(String, u32, bool)> {
            Some(("horizon_modkit:inspect_stick".into(), 1, true))
        }
        fn block(&self, _: [i32; 3]) -> String {
            "minecraft:chest".into()
        }
        fn mod_block_entity_data(&self, _: [i32; 3]) -> Option<Compound> {
            None
        }
        fn mob_view(&self, _: &str, _: u32) -> Option<MobView> {
            None
        }
    }

    #[test]
    fn entity_interact_hears_any_entity_and_the_stick_asks_for_its_window() {
        use horizon_client::mobs::observe::{self, Request, StickTarget};
        let (dir, path) = hmod(
            "stick",
            &[(
                "scripts/server/init.lua",
                br#"
                HEARD = {}
                horizon.on("interact_entity", function(event)
                  -- No player table any more: the acting player's uuid text,
                  -- as the old table carried it.
                  local inspected = horizon.dev.inspect("123e4567-e89b-12d3-a456-426614174000", event.entity)
                  HEARD[#HEARD + 1] = horizon.time().dimension .. ":" .. event.kind .. ":" .. event.entity .. ":" .. tostring(inspected)
                  if event.stack and event.stack.id == "horizon_modkit:inspect_stick" and inspected then
                    horizon.cancel()
                  end
                end)
                horizon.on("right_click_block", function(event)
                  -- No player table any more: the acting player's uuid text.
                  if horizon.dev.inspect("123e4567-e89b-12d3-a456-426614174000", { x = event.pos.x, y = event.pos.y, z = event.pos.z, face = event.face }) then horizon.cancel() end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let stick = horizon_client::mod_api::ItemStack {
            item: "horizon_modkit:inspect_stick".into(),
            count: 1,
            damage: 0,
            extra: None,
        };
        let _ = observe::take_requests();
        // In creative with permission 2, an item frame (no mob) is heard and
        // the click is the stick's.
        let mut op = StickHost {
            game_mode: 1,
            permission: 2,
        };
        let mut frame = test_events::interact_entity_event(9, "minecraft:item_frame", Some(&stick));
        assert_eq!(m.interact_entity(&mut op, &mut frame), Flow::Cancel);
        let mut block = test_events::right_click_block_event([4, 70, -2], [0, 0, -1]);
        assert_eq!(m.right_click_block(&mut op, &mut block), Flow::Cancel);
        assert_eq!(
            observe::take_requests(),
            vec![
                Request::Stick {
                    player: 0x123e4567_e89b_12d3_a456_426614174000,
                    target: StickTarget::Entity(9),
                },
                Request::Stick {
                    player: 0x123e4567_e89b_12d3_a456_426614174000,
                    target: StickTarget::Block {
                        dimension: "minecraft:the_nether".into(),
                        pos: [4, 70, -2],
                        face: [0, 0, -1],
                    },
                },
            ]
        );
        // In survival, or without the permission, the stick is any item:
        // nothing asked, nothing cancelled.
        for (game_mode, permission) in [(0, 4), (1, 1)] {
            let mut host = StickHost {
                game_mode,
                permission,
            };
            assert_eq!(m.interact_entity(&mut host, &mut frame), Flow::Continue);
            assert_eq!(m.right_click_block(&mut host, &mut block), Flow::Continue);
        }
        assert!(observe::take_requests().is_empty());
        let heard: Vec<String> = m.lua.globals().get::<Vec<String>>("HEARD").unwrap();
        assert_eq!(
            heard,
            [
                "minecraft:the_nether:minecraft:item_frame:9:true",
                "minecraft:the_nether:minecraft:item_frame:9:false",
                "minecraft:the_nether:minecraft:item_frame:9:false",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `<n:number>` word is a number only when it is finite: `NaN`,
    /// `inf`, `-inf` and a value past f64's range are refused the way a
    /// non-number is (the usage message, nothing passed on).
    #[test]
    fn a_command_number_must_be_finite() {
        let lua = mlua::Lua::new();
        let usage = "mycmd <n:number>";
        for word in ["NaN", "inf", "-inf", "1e400"] {
            let args = lua.create_table().unwrap();
            let refusal = command_args(&lua, usage, &format!("mycmd {word}"), &args).unwrap();
            assert!(refusal.is_some(), "{word} is refused");
            assert!(
                args.get::<mlua::Value>("n").unwrap().is_nil(),
                "{word} is not passed on"
            );
        }
        let args = lua.create_table().unwrap();
        assert!(
            command_args(&lua, usage, "mycmd 2.5", &args)
                .unwrap()
                .is_none()
        );
        assert_eq!(args.get::<f64>("n").unwrap(), 2.5);
    }

    /// `horizon.after` takes a delay of zero or more ticks and `horizon.every`
    /// one of at least one, both finite. A NaN or infinite delay (a timer
    /// that never fires and leaks until reload), a negative `after`, or a
    /// zero `every` is refused when the mod's scripts run; `after(0)` stays
    /// legal. The mod keeps its old timers, as a failed reload does.
    #[test]
    fn a_timer_delay_must_be_finite_and_in_range() {
        let (dir, path) = hmod("timer-delay", &[("scripts/server/init.lua", b"")]);
        let cases = [
            ("horizon.after(1/0, function() end)", "horizon.after"),
            ("horizon.after(0/0, function() end)", "horizon.after"),
            ("horizon.after(-1, function() end)", "horizon.after"),
            ("horizon.every(1/0, function() end)", "horizon.every"),
            ("horizon.every(0/0, function() end)", "horizon.every"),
            ("horizon.every(0, function() end)", "horizon.every"),
        ];
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        for (code, func) in cases {
            std::fs::write(
                &path,
                Package::write_files(
                    MANIFEST_TEXT,
                    None,
                    &[("scripts/server/init.lua", code.as_bytes())],
                )
                .unwrap(),
            )
            .unwrap();
            let e = m.reload().expect_err(&format!("{code} is refused"));
            assert!(e.contains(func), "{code}: the error names {func}: {e}");
        }
        std::fs::write(
            &path,
            Package::write_files(
                MANIFEST_TEXT,
                None,
                &[(
                    "scripts/server/init.lua",
                    b"horizon.after(0, function() end)\nhorizon.every(1, function() end)",
                )],
            )
            .unwrap(),
        )
        .unwrap();
        m.reload().expect("after(0) and every(1) load");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `hmod` under the mod id `id`, so one test can hold two mods.
    fn hmod_as(id: &str, files: &[(&str, &[u8])]) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("horizon-lua-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{id}.hmod"));
        let manifest = MANIFEST_TEXT.replacen("id = \"ruby\"", &format!("id = \"{id}\""), 1);
        std::fs::write(&path, Package::write_files(&manifest, None, files).unwrap()).unwrap();
        (dir, path)
    }

    #[test]
    fn each_side_reads_its_own_folder_and_shared_only() {
        let (dir, path) = hmod_as(
            "three_folders",
            &[
                (
                    "scripts/server/init.lua",
                    b"SIDE = horizon.side\nSHARED = require('util').who\nSAME = require('same').who\nlocal ok, err = pcall(require, 'client_only')\nOK = ok\nERR = tostring(err)",
                ),
                ("scripts/server/same.lua", b"return { who = 'server' }"),
                (
                    "scripts/client/init.lua",
                    b"SIDE = horizon.side\nSHARED = require('util').who\nSAME = require('same').who\nONLY = require('client_only').who",
                ),
                ("scripts/client/same.lua", b"return { who = 'client' }"),
                ("scripts/client/client_only.lua", b"return { who = 'client' }"),
                ("scripts/shared/util.lua", b"return { who = 'shared' }"),
                ("scripts/shared/same.lua", b"return { who = 'shared' }"),
            ],
        );
        let server = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let client = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        let text = |m: &LuaMod, name: &str| m.lua.globals().get::<String>(name).unwrap();
        assert_eq!(server.side(), LuaSide::Server);
        assert_eq!(text(&server, "SIDE"), "server");
        assert_eq!(text(&server, "SHARED"), "shared");
        assert_eq!(text(&server, "SAME"), "server");
        assert!(!server.lua.globals().get::<bool>("OK").unwrap());
        assert!(text(&server, "ERR").contains("client_only"));
        assert_eq!(client.side(), LuaSide::Client);
        assert_eq!(text(&client, "SIDE"), "client");
        assert_eq!(text(&client, "SHARED"), "shared");
        assert_eq!(text(&client, "SAME"), "client");
        assert_eq!(text(&client, "ONLY"), "client");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn discover_gives_a_state_per_side_the_mod_has_and_no_client_when_dedicated() {
        let (dir_a, both) = hmod_as(
            "sides_both",
            &[
                ("scripts/server/init.lua", b"horizon.log('server')"),
                ("scripts/client/init.lua", b"horizon.log('client')"),
            ],
        );
        let (dir_b, server_only) = hmod_as(
            "sides_server",
            &[("scripts/server/init.lua", b"horizon.log('only')")],
        );
        let (dir_c, client_only) = hmod_as(
            "sides_client",
            &[("scripts/client/init.lua", b"horizon.log('only')")],
        );
        let game = std::env::temp_dir().join(format!("horizon-lua-{}-game", std::process::id()));
        std::fs::create_dir_all(&game).unwrap();
        let states = |dedicated: bool| -> Vec<(String, LuaSide)> {
            let mut args = vec!["--game-dir".to_string(), game.display().to_string()];
            if dedicated {
                args.push("--dedicated".into());
            }
            for path in [&both, &server_only, &client_only] {
                args.extend(["--dev-mod".to_string(), path.display().to_string()]);
            }
            load_states(args.into_iter())
                .iter()
                .map(|m| (m.id().to_owned(), m.side()))
                .collect()
        };
        let pair = |id: &str, side: LuaSide| (id.to_owned(), side);
        assert_eq!(
            states(false),
            [
                pair("sides_both", LuaSide::Server),
                pair("sides_both", LuaSide::Client),
                pair("sides_server", LuaSide::Server),
                pair("sides_client", LuaSide::Client),
            ]
        );
        assert_eq!(
            states(true),
            [
                pair("sides_both", LuaSide::Server),
                pair("sides_server", LuaSide::Server),
            ]
        );
        for dir in [dir_a, dir_b, dir_c, game] {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_shared_only_library_satisfies_a_dependent_require_on_both_sides() {
        let dir =
            std::env::temp_dir().join(format!("horizon-lua-{}-shared-lib", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = |id: &str, depends: &str| {
            format!(
                "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"scripts\"]\nexperimental = [\"scripts\"]\ndepends = [{depends}]\n"
            )
        };
        let lib = dir.join("lib.hmod");
        std::fs::write(
            &lib,
            Package::write_files(
                &manifest("lib", ""),
                None,
                &[("scripts/shared/helper.lua", b"return 42")],
            )
            .unwrap(),
        )
        .unwrap();
        let user = dir.join("user.hmod");
        std::fs::write(
            &user,
            Package::write_files(
                &manifest("user", "\"lib\""),
                None,
                &[
                    ("scripts/server/init.lua", b"local v = require('helper')\nV = v\nhorizon.log('helper ' .. tostring(v))"),
                    ("scripts/client/init.lua", b"local v = require('helper')\nV = v\nhorizon.log('helper ' .. tostring(v))"),
                ],
            )
            .unwrap(),
        )
        .unwrap();
        let server = LuaMod::load(&user, LuaSide::Server, &[("lib".into(), lib.clone())]).unwrap();
        let client = LuaMod::load(&user, LuaSide::Client, &[("lib".into(), lib.clone())]).unwrap();
        assert_eq!(server.lua.globals().get::<i32>("V").unwrap(), 42);
        assert_eq!(client.lua.globals().get::<i32>("V").unwrap(), 42);
        let game = dir.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let states: Vec<(String, LuaSide)> = load_states(
            [
                "--game-dir".to_string(),
                game.display().to_string(),
                "--dev-mod".to_string(),
                lib.display().to_string(),
                "--dev-mod".to_string(),
                user.display().to_string(),
            ]
            .into_iter(),
        )
        .iter()
        .map(|m| (m.id().to_owned(), m.side()))
        .collect();
        assert_eq!(
            states,
            [
                ("user".to_string(), LuaSide::Server),
                ("user".to_string(), LuaSide::Client)
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn check_mod_names_the_side_whose_init_fails() {
        let (dir, path) = hmod_as(
            "check_sides",
            &[
                ("scripts/server/init.lua", b"horizon.log('server ok')"),
                ("scripts/client/init.lua", b"error('client boom')"),
            ],
        );
        let e = check_mod(&path, &[], None).unwrap_err();
        assert!(e.contains("[client]") && e.contains("boom"), "{e}");
        assert!(!e.contains("[server]"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn check_mod_reports_each_side_it_loads() {
        let (dir, path) = hmod_as(
            "check_both",
            &[
                ("scripts/server/init.lua", b"horizon.log('server ok')"),
                ("scripts/client/init.lua", b"horizon.log('client ok')"),
            ],
        );
        let lines = check_mod(&path, &[], None).unwrap();
        assert!(
            lines.iter().any(|l| l.contains("[server] scripts loaded")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("[client] scripts loaded")),
            "{lines:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The first hook of `side` in the generated table that `horizon.on`
    /// accepts: the gate tests read their hooks from it, so no test names a
    /// hook the table could move.
    fn first_hook(side: HookSide) -> &'static str {
        crate::generated_hooks::HOOK_SIDES
            .iter()
            .find(|(name, s)| {
                *s == side && crate::generated_hooks::GENERATED_HOOK_NAMES.contains(name)
            })
            .map(|(name, _)| *name)
            .unwrap()
    }

    #[test]
    fn a_side_allows_its_own_hooks_and_both() {
        assert!(LuaSide::Server.allows(HookSide::Server));
        assert!(LuaSide::Server.allows(HookSide::Both));
        assert!(!LuaSide::Server.allows(HookSide::Client));
        assert!(LuaSide::Client.allows(HookSide::Client));
        assert!(LuaSide::Client.allows(HookSide::Both));
        assert!(!LuaSide::Client.allows(HookSide::Server));
    }

    #[test]
    fn the_generated_table_has_a_hook_of_each_side() {
        for side in [HookSide::Client, HookSide::Server, HookSide::Both] {
            assert!(
                crate::generated_hooks::HOOK_SIDES
                    .iter()
                    .any(|(_, s)| *s == side),
                "no {side:?} hook"
            );
        }
    }

    #[test]
    fn a_server_state_refuses_a_client_hook_at_load() {
        let hook = first_hook(HookSide::Client);
        let lua = format!("horizon.on('{hook}', function() end)");
        let (dir, path) = hmod_as(
            "gate_server",
            &[("scripts/server/init.lua", lua.as_bytes())],
        );
        let e = LuaMod::load(&path, LuaSide::Server, &[])
            .expect_err("a client hook in the server state must fail to load");
        assert!(
            e.contains(&format!("hook '{hook}' runs on the client")),
            "{e}"
        );
        assert!(e.contains("register it from scripts/client/"), "{e}");
        assert!(
            e.contains("this is the server state of mod gate_server"),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_state_refuses_a_server_hook_at_load() {
        let hook = first_hook(HookSide::Server);
        let lua = format!("horizon.on('{hook}', function() end)");
        let (dir, path) = hmod_as(
            "gate_client",
            &[("scripts/client/init.lua", lua.as_bytes())],
        );
        let e = LuaMod::load(&path, LuaSide::Client, &[])
            .expect_err("a server hook in the client state must fail to load");
        assert!(
            e.contains(&format!("hook '{hook}' runs on the server")),
            "{e}"
        );
        assert!(e.contains("register it from scripts/server/"), "{e}");
        assert!(
            e.contains("this is the client state of mod gate_client"),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_state_registers_its_own_side_and_both_sides_share_a_both_hook() {
        let own_server = format!(
            "horizon.on('{}', function() end)",
            first_hook(HookSide::Server)
        );
        let own_client = format!(
            "horizon.on('{}', function() end)",
            first_hook(HookSide::Client)
        );
        let both = format!(
            "horizon.on('{}', function() end)",
            first_hook(HookSide::Both)
        );
        let (dir, path) = hmod_as(
            "gate_each",
            &[
                (
                    "scripts/server/init.lua",
                    format!("{own_server}\n{both}").as_bytes(),
                ),
                (
                    "scripts/client/init.lua",
                    format!("{own_client}\n{both}").as_bytes(),
                ),
            ],
        );
        assert!(LuaMod::load(&path, LuaSide::Server, &[]).is_ok());
        assert!(LuaMod::load(&path, LuaSide::Client, &[]).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shared_module_with_a_client_hook_loads_only_in_the_client_state() {
        let hook = first_hook(HookSide::Client);
        let shared = format!("horizon.on('{hook}', function() end)");
        let (dir, path) = hmod_as(
            "gate_shared",
            &[
                ("scripts/server/init.lua", b"require('hud')"),
                ("scripts/client/init.lua", b"require('hud')"),
                ("scripts/shared/hud.lua", shared.as_bytes()),
            ],
        );
        let e = LuaMod::load(&path, LuaSide::Server, &[])
            .expect_err("the server state requires the client hook's module");
        assert!(e.contains("runs on the client"), "{e}");
        assert!(LuaMod::load(&path, LuaSide::Client, &[]).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn the_engine_filter_enters_only_the_callers_side_for_a_both_hook() {
        // `can_eat` is a Both hook with a listen contract (`native-hooks.json`): a counter
        // handler is all it needs. `client_start` is a Client hook with a listen contract.
        assert_eq!(
            crate::generated_hooks::hook_side("can_eat"),
            Some(HookSide::Both)
        );
        assert!(crate::generated_hooks::GENERATED_HOOK_NAMES.contains(&"can_eat"));
        assert_eq!(
            crate::generated_hooks::hook_side("client_start"),
            Some(HookSide::Client)
        );
        assert!(crate::generated_hooks::GENERATED_HOOK_NAMES.contains(&"client_start"));
        let (dir, path) = hmod_as(
            "entered_filter",
            &[
                (
                    "scripts/server/init.lua",
                    br#"BOTH_COUNTER = 0
horizon.on("can_eat", function(event) BOTH_COUNTER = BOTH_COUNTER + 1 end)"#,
                ),
                (
                    "scripts/client/init.lua",
                    br#"BOTH_COUNTER = 0
CLIENT_COUNTER = 0
horizon.on("can_eat", function(event) BOTH_COUNTER = BOTH_COUNTER + 1 end)
horizon.on("client_start", function(event) CLIENT_COUNTER = CLIENT_COUNTER + 1 end)"#,
                ),
            ],
        );
        let game = std::env::temp_dir().join(format!(
            "horizon-lua-{}-entered-filter-game",
            std::process::id()
        ));
        std::fs::create_dir_all(&game).unwrap();
        let args = vec![
            "--game-dir".to_string(),
            game.display().to_string(),
            "--dev-mod".to_string(),
            path.display().to_string(),
        ];
        let states = load_states(args.into_iter());
        assert_eq!(states.len(), 2);
        let server_idx = states
            .iter()
            .position(|m| m.side() == LuaSide::Server)
            .unwrap();
        let client_idx = states
            .iter()
            .position(|m| m.side() == LuaSide::Client)
            .unwrap();
        // `discover` boxes the states, which would hide their Lua states; keep handles to read
        // the per-state counters.
        let lua_server = states[server_idx].lua.clone();
        let lua_client = states[client_idx].lua.clone();
        let mods: Vec<Box<dyn Hooks>> = states
            .into_iter()
            .map(|m| Box::new(m) as Box<dyn Hooks>)
            .collect();
        assert_eq!(mods[server_idx].mod_side(), Some(ModSide::Server));
        assert_eq!(mods[client_idx].mod_side(), Some(ModSide::Client));
        let entered_sides = |caller: Option<ModSide>, hook: &'static str, side: HookSide| {
            horizon_client::mod_api::with_caller_side(caller, || {
                horizon_client::mod_api::entered(&mods, hook, side)
                    .map(|m| m.mod_side())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            entered_sides(Some(ModSide::Server), "can_eat", HookSide::Both),
            vec![Some(ModSide::Server)]
        );
        assert_eq!(
            entered_sides(Some(ModSide::Client), "can_eat", HookSide::Both),
            vec![Some(ModSide::Client)]
        );
        assert_eq!(
            entered_sides(None, "can_eat", HookSide::Both),
            vec![Some(ModSide::Server), Some(ModSide::Client)]
        );
        // A Client hook reaches only the client state, whatever the caller; the server state
        // never registers it (the gate), so the filter is asserted on `entered` directly.
        for caller in [None, Some(ModSide::Client), Some(ModSide::Server)] {
            assert_eq!(
                entered_sides(caller, "client_start", HookSide::Client),
                vec![Some(ModSide::Client)]
            );
        }
        struct NoopHost;
        impl Host for NoopHost {}
        let counter = |lua: &mlua::Lua, name: &str| -> i64 { lua.globals().get(name).unwrap_or(0) };
        let fire_both = |caller: Option<ModSide>| {
            horizon_client::mod_api::with_caller_side(caller, || {
                let host = NoopHost;
                let stack = ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                };
                let mut can_eat = false;
                horizon_client::mod_api::dispatch_can_eat(&host, &mods, &stack, &mut can_eat);
            });
        };
        fire_both(Some(ModSide::Server));
        assert_eq!(counter(&lua_server, "BOTH_COUNTER"), 1);
        assert_eq!(counter(&lua_client, "BOTH_COUNTER"), 0);
        fire_both(Some(ModSide::Client));
        assert_eq!(counter(&lua_server, "BOTH_COUNTER"), 1);
        assert_eq!(counter(&lua_client, "BOTH_COUNTER"), 1);
        fire_both(None);
        assert_eq!(counter(&lua_server, "BOTH_COUNTER"), 2);
        assert_eq!(counter(&lua_client, "BOTH_COUNTER"), 2);
        // A Client hook still reaches the client state when the caller is the server.
        horizon_client::mod_api::with_caller_side(Some(ModSide::Server), || {
            let mut host = NoopHost;
            horizon_client::mod_api::dispatch_client_start(&mut host, &mods);
        });
        assert_eq!(counter(&lua_client, "CLIENT_COUNTER"), 1);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&game).unwrap();
    }

    #[test]
    fn profiler_splits_the_two_states_of_one_mod_by_side() {
        let _lock = crate::profiler::test_lock();
        crate::profiler::profile_reset();
        let (dir, path) = hmod_as(
            "profile_sides_both",
            &[
                (
                    "scripts/server/init.lua",
                    br#"horizon.on("can_eat", function(event) end)"#,
                ),
                (
                    "scripts/client/init.lua",
                    br#"horizon.on("can_eat", function(event) end)"#,
                ),
            ],
        );
        let game = std::env::temp_dir().join(format!(
            "horizon-lua-{}-profile-sides-game",
            std::process::id()
        ));
        std::fs::create_dir_all(&game).unwrap();
        let args = vec![
            "--game-dir".to_string(),
            game.display().to_string(),
            "--dev-mod".to_string(),
            path.display().to_string(),
        ];
        let states = load_states(args.into_iter());
        assert_eq!(states.len(), 2);
        let mods: Vec<Box<dyn Hooks>> = states
            .into_iter()
            .map(|m| Box::new(m) as Box<dyn Hooks>)
            .collect();
        struct NoopHost;
        impl Host for NoopHost {}
        let fire = |caller: Option<ModSide>| {
            horizon_client::mod_api::with_caller_side(caller, || {
                let host = NoopHost;
                let stack = ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                };
                let mut can_eat = false;
                horizon_client::mod_api::dispatch_can_eat(&host, &mods, &stack, &mut can_eat);
            });
        };
        fire(Some(ModSide::Server));
        fire(Some(ModSide::Client));
        let rows: Vec<_> = crate::profiler::profile_report()
            .into_iter()
            .filter(|r| r.r#mod == "profile_sides_both")
            .collect();
        assert_eq!(rows.len(), 2, "one row per side: {rows:?}");
        assert_eq!(rows[0].side, "client");
        assert_eq!(rows[0].calls, 1);
        assert_eq!(rows[1].side, "server");
        assert_eq!(rows[1].calls, 1);
        assert_eq!(rows[0].hook, "can_eat");
        assert_eq!(rows[1].hook, "can_eat");
        crate::profiler::profile_reset();
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&game).unwrap();
    }

    #[test]
    fn a_client_state_raises_for_server_only_api_and_stays_usable() {
        let (dir, path) = hmod(
            "client_stubs",
            &[(
                "scripts/client/init.lua",
                br#"
                local function expect_server_only(name, f)
                  local ok, err = pcall(f)
                  assert(not ok, name .. " should fail in a client state")
                  local text = tostring(err)
                  assert(
                    string.find(text, "horizon." .. name .. " is only available in scripts/server/", 1, true),
                    name .. ": " .. text
                  )
                  assert(
                    string.find(text, "this is the client state of mod ruby", 1, true),
                    name .. ": " .. text
                  )
                end
                expect_server_only("set_block", function() horizon.set_block(0, 0, 0, "minecraft:stone") end)
                expect_server_only("world_data", function() return horizon.world_data() end)
                expect_server_only("player_data", function() return horizon.player_data("x") end)
                expect_server_only("entity_data", function() return horizon.entity_data(1) end)
                expect_server_only("summon", function() return horizon.summon("minecraft:overworld", "minecraft:cow", 0, 0, 0) end)
                expect_server_only("players", function() return horizon.players() end)
                -- A stubbed error does not break the state.
                horizon.log("after stubs")
                assert(horizon.side == "client")
                local ok, err = pcall(horizon.after, 1, function() end)
                assert(ok, tostring(err))
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        assert_eq!(m.id(), "ruby");
        assert_eq!(m.side(), LuaSide::Client);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn net_send_to_player_and_custom_payload_reach_the_host() {
        // The server half, byte for byte: what the handler queued is what
        // the host is handed, NUL and high bytes included, and an empty
        // string sends an empty payload.
        let (dir, path) = hmod(
            "net_send_server",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  horizon.net_send_to_player(event.player, "ruby:back", event.data)
                  horizon.send_custom_payload_to_player(event.player, "ruby:back", string.char(0, 200, 65))
                  horizon.net_send_to_player("steve", "ruby:back", "")
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host,
            &mods,
            0x123e4567_e89b_12d3_a456_426614174000,
            "ruby:from",
            &[116u8, 0, 159],
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "net_send_to_player 24249434048109030647017182301789831168 ruby:back [116, 0, 159]",
                "send_custom_payload_to_player 24249434048109030647017182301789831168 ruby:back [0, 200, 65]",
                "net_send_to_player 24249434048109030647017182301789831168 ruby:back []",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn net_send_to_server_runs_after_client_hooks() {
        // The client half: queued actions apply after a client hook
        // (`net_from_server`), as they do after the client tick, so a
        // client mod answers the server from a handler.
        let (dir, path) = hmod(
            "net_send_client",
            &[(
                "scripts/client/init.lua",
                br#"
                horizon.on("net_from_server", function(event)
                  horizon.net_send_to_server("ruby:back", event.data)
                end)
                horizon.on("client_tick_post", function(event)
                  horizon.net_send_to_server("ruby:tick", string.char(0, 129))
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        horizon_client::mod_api::dispatch_net_from_server(
            &mut host,
            &mods,
            "ruby:from",
            &[1u8, 0, 255],
        );
        horizon_client::mod_api::dispatch_client_tick_post(&mut host, &mods);
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "net_send_to_server ruby:back [1, 0, 255]",
                "net_send_to_server ruby:tick [0, 129]",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failing_net_send_surfaces_and_later_actions_still_run() {
        // A refused net send does not cancel the rest of the handler's
        // queued actions: one `net_from_client` handler queues a failing
        // send and then a `broadcast`, and the `broadcast` runs while the
        // refusal is reported with the action's name. (Driving the real
        // `call_kind` drain, not `run_action` on its own.)
        struct NetFailHost {
            lines: Mutex<Vec<String>>,
        }
        impl Host for NetFailHost {
            fn net_send_to_player(
                &mut self,
                _player: u128,
                _channel: &str,
                _data: &[u8],
            ) -> Result<(), String> {
                Err("past 32 messages this tick".into())
            }
            fn net_send_to_server(&mut self, _channel: &str, _data: &[u8]) -> Result<(), String> {
                Err("the server carries no mod messages".into())
            }
            fn broadcast(&self, line: horizon_client::mod_api::Text) {
                self.lines
                    .lock()
                    .unwrap()
                    .push(format!("broadcast {}", line.plain()));
            }
        }
        let (dir, path) = hmod(
            "net_send_err",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  horizon.net_send_to_player(event.player, "ruby:back", event.data)
                  horizon.broadcast("after")
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        set_status(ModStatus {
            id: m.id().into(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        let id = m.id().to_owned();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = NetFailHost {
            lines: Mutex::new(Vec::new()),
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host,
            &mods,
            0x123e4567_e89b_12d3_a456_426614174000,
            "ruby:from",
            &[116u8, 0, 159],
        );
        assert_eq!(*host.lines.lock().unwrap(), ["broadcast after"]);
        let e = horizon_client::dev_input::mods::status()
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.error)
            .expect("the refusal is reported");
        assert!(
            e.contains("net_send_to_player: past 32 messages this tick"),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_state_stubs_the_player_helpers_but_keeps_net_send_to_server() {
        // The two player helpers raise the server-only error on the
        // client; `net_send_to_server` is kept, so at load it fails only
        // for lack of a hook to give it a host.
        let (dir, path) = hmod(
            "client_net_stubs",
            &[(
                "scripts/client/init.lua",
                br#"
                local function expect_server_only(name, f)
                  local ok, err = pcall(f)
                  assert(not ok, name .. " should fail in a client state")
                  assert(
                    string.find(tostring(err), "horizon." .. name .. " is only available in scripts/server/", 1, true),
                    name .. ": " .. tostring(err)
                  )
                end
                expect_server_only("net_send_to_player", function()
                  horizon.net_send_to_player("steve", "ruby:x", "hi")
                end)
                expect_server_only("send_custom_payload_to_player", function()
                  horizon.send_custom_payload_to_player("steve", "ruby:x", "hi")
                end)
                local ok, err = pcall(horizon.net_send_to_server, "ruby:x", "hi")
                assert(not ok, "a kept helper fails at load for want of a handler, not as a stub")
                assert(not string.find(tostring(err), "is only available in scripts/server/", 1, true), tostring(err))
                assert(string.find(tostring(err), "horizon.net_send_to_server: actions work inside a hook's handler", 1, true), tostring(err))
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        assert_eq!(m.side(), LuaSide::Client);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn in_a_draw_hook_the_player_helpers_raise_and_net_send_to_server_applies_after_the_frame() {
        // In a client draw hook the server-only helpers raise the
        // server-only error, like `set_block`; the kept client action
        // queues and applies after the frame, like `tell`/`broadcast`.
        let (dir, path) = hmod(
            "client_net_draw",
            &[(
                "scripts/client/init.lua",
                br#"
                horizon.on("hud", function(event)
                  __witness = (__witness or 0) + 1
                  local ok1, e1 = pcall(horizon.net_send_to_player, "steve", "ruby:x", "hi")
                  PA = (not ok1) and tostring(e1) or "ran"
                  local ok2, e2 = pcall(horizon.send_custom_payload_to_player, "steve", "ruby:x", "hi")
                  PB = (not ok2) and tostring(e2) or "ran"
                  local ok3, e3 = pcall(horizon.net_send_to_server, "ruby:x", "hi")
                  PC = ok3 and "queued" or tostring(e3)
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        let state = m.lua.clone();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        let mut batch = horizon_client::mod_api::HudBatch::new([800, 600]);
        let mut corners = horizon_client::mod_api::Corners::new(800.0, 600.0, 0.0);
        horizon_client::mod_api::dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        assert_eq!(state.globals().get::<i64>("__witness").unwrap(), 1);
        let pa: String = state.globals().get("PA").unwrap();
        assert!(
            pa.contains("horizon.net_send_to_player is only available in scripts/server/"),
            "{pa}"
        );
        let pb: String = state.globals().get("PB").unwrap();
        assert!(
            pb.contains(
                "horizon.send_custom_payload_to_player is only available in scripts/server/"
            ),
            "{pb}"
        );
        let pc: String = state.globals().get("PC").unwrap();
        assert_eq!(pc, "queued");
        assert_eq!(
            *host.lines.lock().unwrap(),
            ["net_send_to_server ruby:x [104, 105]"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_state_keeps_the_allow_list() {
        let (dir, path) = hmod(
            "client_api",
            &[
                (
                    "scripts/client/init.lua",
                    br#"
                    assert(horizon.mod_id == "ruby", "mod_id")
                    assert(horizon.side == "client", "side")
                    assert(type(horizon.mod_version) == "string", "mod_version")
                    assert(type(horizon.libraries) == "table", "libraries")
                    assert(type(horizon.log) == "function", "log")
                    assert(type(horizon.on) == "function", "on")
                    assert(type(horizon.after) == "function", "after")
                    assert(type(horizon.every) == "function", "every")
                    assert(type(horizon.ticks) == "function", "ticks")
                    assert(type(horizon.config) == "function", "config")
                    assert(type(horizon.save_config) == "function", "save_config")
                    assert(type(horizon.dev) == "table", "dev")
                    assert(type(horizon.dev.inspect) == "function", "dev.inspect")
                    assert(type(horizon.distance) == "function", "distance")
                    assert(type(horizon.handled) == "function", "handled")
                    assert(type(horizon.cancel) == "function", "cancel")
                    assert(type(horizon.require) == "function", "require")
                    local ok, e = pcall(horizon.log, "x")
                    assert(ok, tostring(e))
                    local ok2, e2 = pcall(horizon.after, 1, function() end)
                    assert(ok2, tostring(e2))
                    local ok3, e3 = pcall(horizon.every, 1, function() end)
                    assert(ok3, tostring(e3))
                    local ok4, e4 = pcall(horizon.ticks)
                    assert(ok4 and type(e4) == "number", tostring(e4))
                    local d = horizon.distance({ x = 0, y = 0, z = 0 }, { x = 3, y = 4, z = 0 })
                    assert(d == 5, tostring(d))
                    assert(pcall(horizon.on, "client_start", function(event) end))
                    assert(require("util") == 42)
                    local c = horizon.config()
                    assert(type(c) == "table", "config")
                    horizon.save_config({ k = 1 })
                    local ok5, e5 = pcall(horizon.handled)
                    assert(not ok5)
                    assert(not string.find(tostring(e5), "is only available in scripts/server/", 1, true), tostring(e5))
                    local ok6, e6 = pcall(horizon.cancel)
                    assert(not ok6)
                    assert(not string.find(tostring(e6), "is only available in scripts/server/", 1, true), tostring(e6))
                    "#,
                ),
                ("scripts/shared/util.lua", b"return 42"),
            ],
        );
        let config = dir.join("config");
        let m = LuaMod::load_with(&path, LuaSide::Client, &[], Some(&config)).unwrap();
        assert_eq!(m.side(), LuaSide::Client);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_state_keeps_its_own_settings_file() {
        let (dir, path) = hmod(
            "per-side-config",
            &[
                (
                    "scripts/server/init.lua",
                    br#"
                    local c = horizon.config()
                    SERVER_LOAD_A = c.a
                    SERVER_LOAD_B = c.b
                    horizon.save_config({a = 1})
                    local d = horizon.config()
                    SERVER_AFTER_A = d.a
                    SERVER_AFTER_B = d.b
                    "#,
                ),
                (
                    "scripts/client/init.lua",
                    br#"
                    local c = horizon.config()
                    CLIENT_LOAD_A = c.a
                    CLIENT_LOAD_B = c.b
                    horizon.save_config({b = 2})
                    local d = horizon.config()
                    CLIENT_AFTER_A = d.a
                    CLIENT_AFTER_B = d.b
                    "#,
                ),
            ],
        );
        let config = dir.join("config");
        let server = LuaMod::load_with(&path, LuaSide::Server, &[], Some(&config)).unwrap();
        assert!(config.join("ruby.json").exists());
        assert!(!config.join("ruby.client.json").exists());
        let g = server.lua.globals();
        assert!(g.get::<Value>("SERVER_LOAD_A").unwrap().is_nil());
        assert!(g.get::<Value>("SERVER_LOAD_B").unwrap().is_nil());
        assert_eq!(g.get::<i64>("SERVER_AFTER_A").unwrap(), 1);
        assert!(g.get::<Value>("SERVER_AFTER_B").unwrap().is_nil());

        let client = LuaMod::load_with(&path, LuaSide::Client, &[], Some(&config)).unwrap();
        assert!(config.join("ruby.client.json").exists());
        let saved_server: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(config.join("ruby.json")).unwrap())
                .unwrap();
        assert_eq!(saved_server, serde_json::json!({"a": 1}));
        let saved_client: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(config.join("ruby.client.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved_client, serde_json::json!({"b": 2}));
        let g = client.lua.globals();
        assert!(g.get::<Value>("CLIENT_LOAD_A").unwrap().is_nil());
        assert!(g.get::<Value>("CLIENT_LOAD_B").unwrap().is_nil());
        assert_eq!(g.get::<i64>("CLIENT_AFTER_B").unwrap(), 2);
        assert!(g.get::<Value>("CLIENT_AFTER_A").unwrap().is_nil());

        server.reload().unwrap();
        client.reload().unwrap();
        let g = server.lua.globals();
        assert_eq!(g.get::<i64>("SERVER_LOAD_A").unwrap(), 1);
        assert!(g.get::<Value>("SERVER_LOAD_B").unwrap().is_nil());
        let g = client.lua.globals();
        assert!(g.get::<Value>("CLIENT_LOAD_A").unwrap().is_nil());
        assert_eq!(g.get::<i64>("CLIENT_LOAD_B").unwrap(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_state_services_its_own_reload_on_client_tick_pre() {
        let (dir, path) = hmod_as(
            "ps7a_client_reload",
            &[
                ("scripts/server/init.lua", b"horizon.log('ps7a server')"),
                ("scripts/client/init.lua", b"horizon.log('ps7a client')"),
            ],
        );
        let game =
            std::env::temp_dir().join(format!("horizon-lua-{}-ps7a-game", std::process::id()));
        std::fs::create_dir_all(&game).unwrap();
        let args = vec![
            "--game-dir".to_string(),
            game.display().to_string(),
            "--dev-mod".to_string(),
            path.display().to_string(),
        ];
        let states = load_states(args.into_iter());
        assert_eq!(states.len(), 2, "one state per side");
        let id = states[0].id().to_owned();
        let mods: Vec<Box<dyn Hooks>> = states
            .into_iter()
            .map(|m| Box::new(m) as Box<dyn Hooks>)
            .collect();
        let reloads_of = |side: &str| -> u32 {
            horizon_client::dev_input::mods::status()
                .into_iter()
                .find(|s| s.id == id && s.side == side)
                .map(|s| s.reloads)
                .unwrap_or(0)
        };
        assert_eq!(reloads_of("server"), 0);
        assert_eq!(reloads_of("client"), 0);
        let mut host = MobHost {
            attachments: Default::default(),
            health: Vec::new(),
            lines: Mutex::new(Vec::new()),
            pushes: Vec::new(),
            carried: wand(),
            world: Default::default(),
            player: Default::default(),
        };
        // A client-only reload is serviced by the client tick, not the server tick.
        horizon_client::dev_input::mods::request_reload(Some(&id), Some("client"));
        horizon_client::mod_api::dispatch_client_tick_pre(&mut host, &mods);
        assert_eq!(
            reloads_of("client"),
            1,
            "client state reloaded on client_tick_pre"
        );
        assert_eq!(
            reloads_of("server"),
            0,
            "server state untouched by a client-only reload"
        );
        // A no-side reload moves both: server on its tick, client on its tick.
        horizon_client::dev_input::mods::request_reload(Some(&id), None);
        let context = test_events::native_lifecycle_context(
            horizon_client::mod_api::NativeAuthority::IntegratedServer,
            horizon_client::mod_api::NativeWorldEpoch::new(1),
            horizon_client::mod_api::NativeTickId::new(1),
        );
        horizon_client::mod_api::dispatch_server_tick(
            &mut host,
            &mods,
            &context,
            ServerTickPhase::ServerTickStart {
                world_running: true,
            },
        );
        horizon_client::mod_api::dispatch_client_tick_pre(&mut host, &mods);
        assert_eq!(
            reloads_of("server"),
            1,
            "server state reloaded on server_tick"
        );
        assert_eq!(
            reloads_of("client"),
            2,
            "client state reloaded again on client_tick_pre"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&game).unwrap();
    }

    #[test]
    fn concurrent_hooks_on_one_state_lose_no_queue_action_50x() {
        let (dir, path) = hmod(
            "concurrent50",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("loot_table_load", function()
                  local t0 = os.clock()
                  while os.clock() - t0 < 0.003 do end
                  horizon.broadcast("hi")
                end)
                "#,
            )],
        );
        let m = std::sync::Arc::new(LuaMod::load(&path, LuaSide::Server, &[]).unwrap());
        let mut total = 0;
        for _ in 0..50 {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
            let m1 = m.clone();
            let b1 = barrier.clone();
            let h1 = std::thread::spawn(move || {
                b1.wait();
                let a = m1
                    .run_handlers_with("loot_table_load", None, Reader::None, (), HANDLER_BUDGET)
                    .unwrap();
                assert_eq!(a.len(), 1, "one queued action per firing");
                assert_eq!(a[0].0, "broadcast");
                a.len()
            });
            let m2 = m.clone();
            let b2 = barrier.clone();
            let h2 = std::thread::spawn(move || {
                b2.wait();
                let a = m2
                    .run_handlers_with("loot_table_load", None, Reader::None, (), HANDLER_BUDGET)
                    .unwrap();
                assert_eq!(a.len(), 1, "one queued action per firing");
                assert_eq!(a[0].0, "broadcast");
                a.len()
            });
            barrier.wait();
            total += h1.join().unwrap();
            total += h2.join().unwrap();
        }
        assert_eq!(total, 100, "every queued action exactly once");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_states_do_not_block_each_other() {
        let (dir_a, path_a) = hmod(
            "slowA",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("loot_table_load", function()
                  local t0 = os.clock()
                  while os.clock() - t0 < 0.08 do end
                  horizon.broadcast("slow")
                end)
                "#,
            )],
        );
        let (dir_b, path_b) = hmod(
            "fastB",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("loot_table_load", function()
                  horizon.broadcast("fast")
                end)
                "#,
            )],
        );
        let ma = std::sync::Arc::new(LuaMod::load(&path_a, LuaSide::Server, &[]).unwrap());
        let mb = std::sync::Arc::new(LuaMod::load(&path_b, LuaSide::Server, &[]).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let ma2 = ma.clone();
        let b1 = barrier.clone();
        let ha = std::thread::spawn(move || {
            b1.wait();
            let a = ma2
                .run_handlers_with("loot_table_load", None, Reader::None, (), HANDLER_BUDGET)
                .unwrap();
            assert_eq!(a.len(), 1);
        });
        barrier.wait();
        std::thread::sleep(std::time::Duration::from_millis(15));
        let t0 = std::time::Instant::now();
        let a = mb
            .run_handlers_with("loot_table_load", None, Reader::None, (), HANDLER_BUDGET)
            .unwrap();
        let elapsed = t0.elapsed();
        assert_eq!(a.len(), 1);
        assert!(
            elapsed < std::time::Duration::from_millis(40),
            "state B blocked by A: {elapsed:?}"
        );
        ha.join().unwrap();
        std::fs::remove_dir_all(&dir_a).unwrap();
        std::fs::remove_dir_all(&dir_b).unwrap();
    }

    #[test]
    fn reentrancy_same_thread_via_turn() {
        let (dir, path) = hmod(
            "reenter",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("loot_table_load", function()
                  horizon.broadcast("hi")
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        // Hold the turn, then a nested entry must succeed at once.
        let _outer = m.turn.try_acquire_timeout(turn::TURN_TIMEOUT).unwrap();
        let t0 = std::time::Instant::now();
        let a = m
            .run_handlers_with("loot_table_load", None, Reader::None, (), HANDLER_BUDGET)
            .unwrap();
        assert_eq!(a.len(), 1);
        assert!(
            t0.elapsed() < std::time::Duration::from_millis(50),
            "nested blocked"
        );
        // A second nested has_handlers also re-enters.
        assert!(m.has_handlers("server_tick"));
        drop(_outer);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- chest menus (issue 52) ----
    #[test]
    fn chest_menu_actions_reach_the_host() {
        use std::sync::Mutex as M;
        struct MenuHost {
            lines: M<Vec<String>>,
            opened: M<Vec<(u128, u8, String, Vec<Option<ItemStack>>)>>,
            opened_in: M<
                Vec<(
                    u128,
                    u8,
                    String,
                    Vec<Option<ItemStack>>,
                    std::ops::Range<usize>,
                )>,
            >,
            set: M<Vec<(u128, Vec<Option<ItemStack>>)>>,
            closed: M<Vec<u128>>,
            contents: Option<Vec<Option<ItemStack>>>,
            cursor: Option<ItemStack>,
        }
        impl Host for MenuHost {
            fn dimension(&self) -> &'static str {
                "minecraft:overworld"
            }
            fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
                vec![test_events::native_server_player(
                    0x123e4567_e89b_12d3_a456_426614174000,
                    "steve".into(),
                    0,
                    "minecraft:overworld".into(),
                    [0.0, 64.0, 0.0],
                    0,
                    20.0,
                    0.0,
                    0.0,
                    0,
                    String::new(),
                )]
            }
            fn broadcast(&self, t: horizon_client::mod_api::Text) {
                self.lines
                    .lock()
                    .unwrap()
                    .push(format!("broadcast {}", t.plain()));
            }
            fn open_menu(
                &mut self,
                uuid: u128,
                rows: u8,
                title: horizon_client::mod_api::Text,
                items: Vec<Option<ItemStack>>,
            ) -> Option<i32> {
                self.opened
                    .lock()
                    .unwrap()
                    .push((uuid, rows, title.plain(), items));
                Some(7)
            }
            fn open_menu_with_input(
                &mut self,
                uuid: u128,
                rows: u8,
                title: horizon_client::mod_api::Text,
                items: Vec<Option<ItemStack>>,
                input: std::ops::Range<usize>,
            ) -> Option<i32> {
                self.opened_in
                    .lock()
                    .unwrap()
                    .push((uuid, rows, title.plain(), items, input));
                Some(8)
            }
            fn set_menu_items(&mut self, uuid: u128, items: Vec<Option<ItemStack>>) {
                self.set.lock().unwrap().push((uuid, items));
            }
            fn close_menu(&mut self, uuid: u128) {
                self.closed.lock().unwrap().push(uuid);
            }
            fn menu_contents(&self, _uuid: u128) -> Option<Vec<Option<ItemStack>>> {
                self.contents.clone()
            }
            fn menu_cursor(&self, _uuid: u128) -> Option<ItemStack> {
                self.cursor.clone()
            }
        }
        let (dir, path) = hmod(
            "menu_actions",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  horizon.open_menu(event.player, 2, "Shop", {
                    { id = "minecraft:stick", count = 3 },
                    nil,
                    { id = "minecraft:apple", count = 2, extra = { note = "hi" } },
                  })
                  horizon.open_menu_with_input("steve", 3, "Input", {
                    { id = "minecraft:stone", count = 1 },
                  }, { start = 0, ["end"] = 9 })
                  horizon.set_menu_items(event.player, {
                    { id = "minecraft:dirt", count = 5 },
                  })
                  horizon.close_menu(event.player)
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = MenuHost {
            lines: M::new(Vec::new()),
            opened: M::new(Vec::new()),
            opened_in: M::new(Vec::new()),
            set: M::new(Vec::new()),
            closed: M::new(Vec::new()),
            contents: None,
            cursor: None,
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host,
            &mods,
            0x123e4567_e89b_12d3_a456_426614174000,
            "ruby:from",
            b"hi",
        );
        let opened = host.opened.lock().unwrap();
        assert_eq!(opened.len(), 1);
        let (uuid, rows, title, items) = &opened[0];
        assert_eq!(*uuid, 0x123e4567_e89b_12d3_a456_426614174000);
        assert_eq!(*rows, 2);
        assert_eq!(title, "Shop");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_ref().unwrap().item.as_ref(), "minecraft:stick");
        assert_eq!(items[0].as_ref().unwrap().count, 3);
        assert!(items[1].is_none(), "hole stays None");
        assert_eq!(items[2].as_ref().unwrap().item.as_ref(), "minecraft:apple");
        assert_eq!(items[2].as_ref().unwrap().count, 2);
        assert!(
            items[2].as_ref().unwrap().extra.is_some(),
            "extra preserved"
        );
        let opened_in = host.opened_in.lock().unwrap();
        assert_eq!(opened_in.len(), 1);
        let (uuid2, rows2, title2, items2, range) = &opened_in[0];
        assert_eq!(*uuid2, 0x123e4567_e89b_12d3_a456_426614174000);
        assert_eq!(*rows2, 3);
        assert_eq!(title2, "Input");
        assert_eq!(items2.len(), 1);
        assert_eq!(range.clone(), 0..9);
        let set = host.set.lock().unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].0, 0x123e4567_e89b_12d3_a456_426614174000);
        assert_eq!(set[0].1.len(), 1);
        assert_eq!(
            set[0].1[0].as_ref().unwrap().item.as_ref(),
            "minecraft:dirt"
        );
        let closed = host.closed.lock().unwrap();
        assert_eq!(*closed, vec![0x123e4567_e89b_12d3_a456_426614174000]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn menu_rows_saturate_before_host() {
        use std::sync::Mutex as M;
        struct RowsHost {
            opened: M<Vec<u8>>,
            opened_in: M<Vec<(u8, std::ops::Range<usize>)>>,
        }
        impl Host for RowsHost {
            fn dimension(&self) -> &'static str {
                "minecraft:overworld"
            }
            fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
                vec![test_events::native_server_player(
                    0x123e4567_e89b_12d3_a456_426614174000,
                    "steve".into(),
                    0,
                    "minecraft:overworld".into(),
                    [0.0, 64.0, 0.0],
                    0,
                    20.0,
                    0.0,
                    0.0,
                    0,
                    String::new(),
                )]
            }
            fn open_menu(
                &mut self,
                _uuid: u128,
                rows: u8,
                _title: horizon_client::mod_api::Text,
                _items: Vec<Option<ItemStack>>,
            ) -> Option<i32> {
                self.opened.lock().unwrap().push(rows);
                Some(7)
            }
            fn open_menu_with_input(
                &mut self,
                _uuid: u128,
                rows: u8,
                _title: horizon_client::mod_api::Text,
                _items: Vec<Option<ItemStack>>,
                input: std::ops::Range<usize>,
            ) -> Option<i32> {
                self.opened_in.lock().unwrap().push((rows, input));
                Some(8)
            }
        }
        let (dir, path) = hmod(
            "menu_rows",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  horizon.open_menu(event.player, -1, "A", {})
                  horizon.open_menu(event.player, 256, "B", {})
                  horizon.open_menu_with_input(event.player, -1, "C", {}, { start = 0, ["end"] = 10 })
                  horizon.open_menu_with_input(event.player, 256, "D", {}, { start = 0, ["end"] = 10 })
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = RowsHost {
            opened: M::new(Vec::new()),
            opened_in: M::new(Vec::new()),
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host,
            &mods,
            0x123e4567_e89b_12d3_a456_426614174000,
            "x",
            b"",
        );
        assert_eq!(*host.opened.lock().unwrap(), vec![0, 255]);
        assert_eq!(
            host.opened_in
                .lock()
                .unwrap()
                .iter()
                .map(|(r, _)| *r)
                .collect::<Vec<_>>(),
            vec![0, 255]
        );
        assert_eq!(host.opened_in.lock().unwrap()[0].1.clone(), 0..10);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn menu_contents_and_cursor_read_back() {
        struct ReadHost {
            contents: Option<Vec<Option<ItemStack>>>,
            cursor: Option<ItemStack>,
        }
        impl Host for ReadHost {
            fn dimension(&self) -> &'static str {
                "minecraft:overworld"
            }
            fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
                vec![test_events::native_server_player(
                    0x123e4567_e89b_12d3_a456_426614174000,
                    "steve".into(),
                    0,
                    "minecraft:overworld".into(),
                    [0.0, 64.0, 0.0],
                    0,
                    20.0,
                    0.0,
                    0.0,
                    0,
                    String::new(),
                )]
            }
            fn menu_contents(&self, _u: u128) -> Option<Vec<Option<ItemStack>>> {
                self.contents.clone()
            }
            fn menu_cursor(&self, _u: u128) -> Option<ItemStack> {
                self.cursor.clone()
            }
            fn held_stack(&self, _u: u128) -> Option<ItemStack> {
                None
            }
        }
        let (dir, path) = hmod(
            "menu_reads",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  local c = horizon.menu_contents(event.player)
                  if c == nil then __C_NIL = true else
                    __C_LEN = #c
                    if c[1] then __C1_ID = c[1].id; __C1_COUNT = c[1].count; __C1_PLAIN = c[1].plain end
                    __C2_NIL = c[2] == nil
                    if c[3] then __C3_ID = c[3].id; __C3_EXTRA_NIL = c[3].extra == nil end
                  end
                  local cur = horizon.menu_cursor(event.player)
                  if cur == nil then __CUR_NIL = true else __CUR_ID = cur.id; __CUR_COUNT = cur.count end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let state = m.lua.clone();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = ReadHost {
            contents: Some(vec![
                Some(ItemStack {
                    item: "minecraft:stick".into(),
                    count: 2,
                    damage: 0,
                    extra: None,
                }),
                None,
                Some(ItemStack {
                    item: "minecraft:apple".into(),
                    count: 1,
                    damage: 0,
                    extra: Some(Box::new(horizon_client::mod_api::StackData {
                        repair_cost: 5,
                        ..Default::default()
                    })),
                }),
            ]),
            cursor: Some(ItemStack {
                item: "minecraft:dirt".into(),
                count: 4,
                damage: 0,
                extra: None,
            }),
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host,
            &mods,
            0x123e4567_e89b_12d3_a456_426614174000,
            "x",
            b"",
        );
        let g = state.globals();
        assert_eq!(g.get::<String>("__C1_ID").unwrap(), "minecraft:stick");
        assert_eq!(g.get::<i64>("__C1_COUNT").unwrap(), 2);
        assert!(g.get::<bool>("__C1_PLAIN").unwrap());
        assert!(g.get::<bool>("__C2_NIL").unwrap());
        assert_eq!(g.get::<String>("__C3_ID").unwrap(), "minecraft:apple");
        assert!(!g.get::<bool>("__C3_EXTRA_NIL").unwrap());
        assert_eq!(g.get::<String>("__CUR_ID").unwrap(), "minecraft:dirt");
        assert_eq!(g.get::<i64>("__CUR_COUNT").unwrap(), 4);
        // Now None -> nil
        let (dir2, path2) = hmod(
            "menu_reads_nil",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  __C_NIL = horizon.menu_contents(event.player) == nil
                  __CUR_NIL = horizon.menu_cursor(event.player) == nil
                end)
                "#,
            )],
        );
        let m2 = LuaMod::load(&path2, LuaSide::Server, &[]).unwrap();
        let state2 = m2.lua.clone();
        let mods2: Vec<Box<dyn Hooks>> = vec![Box::new(m2)];
        let mut host2 = ReadHost {
            contents: None,
            cursor: None,
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host2,
            &mods2,
            0x123e4567_e89b_12d3_a456_426614174000,
            "x",
            b"",
        );
        assert!(state2.globals().get::<bool>("__C_NIL").unwrap());
        assert!(state2.globals().get::<bool>("__CUR_NIL").unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&dir2).unwrap();
    }

    #[test]
    fn wrong_item_shape_raises_and_none_does_not() {
        use std::sync::Mutex as M;
        struct BadHost {
            lines: M<Vec<String>>,
        }
        impl Host for BadHost {
            fn dimension(&self) -> &'static str {
                "minecraft:overworld"
            }
            fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
                vec![test_events::native_server_player(
                    0x123e4567_e89b_12d3_a456_426614174000,
                    "steve".into(),
                    0,
                    "minecraft:overworld".into(),
                    [0.0, 64.0, 0.0],
                    0,
                    20.0,
                    0.0,
                    0.0,
                    0,
                    String::new(),
                )]
            }
            fn broadcast(&self, t: horizon_client::mod_api::Text) {
                self.lines.lock().unwrap().push(t.plain());
            }
            fn open_menu(
                &mut self,
                _u: u128,
                _r: u8,
                _t: horizon_client::mod_api::Text,
                _i: Vec<Option<ItemStack>>,
            ) -> Option<i32> {
                Some(1)
            }
            fn set_menu_items(&mut self, _u: u128, _i: Vec<Option<ItemStack>>) {}
            fn close_menu(&mut self, _u: u128) {}
            fn menu_contents(&self, _u: u128) -> Option<Vec<Option<ItemStack>>> {
                None
            }
            fn menu_cursor(&self, _u: u128) -> Option<ItemStack> {
                None
            }
        }
        // bad shape: count = "x" -> script error naming field, status error mentions it
        let (dir, path) = hmod(
            "menu_bad",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  horizon.open_menu(event.player, 1, "T", { { id = "minecraft:stick", count = "x" } })
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        set_status(horizon_client::dev_input::mods::ModStatus {
            id: m.id().into(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        let id = m.id().to_owned();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        let mut host = BadHost {
            lines: M::new(Vec::new()),
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host,
            &mods,
            0x123e4567_e89b_12d3_a456_426614174000,
            "x",
            b"",
        );
        let e = horizon_client::dev_input::mods::status()
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.error)
            .expect("bad item reports");
        assert!(
            e.contains("items") && (e.contains("count") || e.contains("item")),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        // None does not raise: unknown player -> silent no-op, status stays None
        let (dir2, path2) = hmod(
            "menu_none",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  horizon.open_menu("00000000-0000-0000-0000-000000000099", 1, "T", { { id = "minecraft:stick", count = 1 } })
                  horizon.set_menu_items("00000000-0000-0000-0000-000000000099", {})
                  horizon.close_menu("00000000-0000-0000-0000-000000000099")
                  __NIL = horizon.menu_contents("00000000-0000-0000-0000-000000000099") == nil
                  __NIL2 = horizon.menu_cursor("00000000-0000-0000-0000-000000000099") == nil
                  horizon.broadcast("still")
                end)
                "#,
            )],
        );
        let m2 = LuaMod::load(&path2, LuaSide::Server, &[]).unwrap();
        set_status(horizon_client::dev_input::mods::ModStatus {
            id: m2.id().into(),
            side: m2.side().as_str().to_owned(),
            ..Default::default()
        });
        let mods2: Vec<Box<dyn Hooks>> = vec![Box::new(m2)];
        let mut host2 = BadHost {
            lines: M::new(Vec::new()),
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host2,
            &mods2,
            0x123e4567_e89b_12d3_a456_426614174000,
            "x",
            b"",
        );
        // No error (or unrelated): the broadcast ran and reads were nil
        assert_eq!(host2.lines.lock().unwrap().as_slice(), ["still"]);
        // The nil reads are nil, not error
        std::fs::remove_dir_all(&dir2).unwrap();
        // continuation via real drain: open_menu None still lets broadcast run
        struct NoneHost {
            lines: M<Vec<String>>,
        }
        impl Host for NoneHost {
            fn dimension(&self) -> &'static str {
                "minecraft:overworld"
            }
            fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
                vec![test_events::native_server_player(
                    0x123e4567_e89b_12d3_a456_426614174000,
                    "steve".into(),
                    0,
                    "minecraft:overworld".into(),
                    [0.0, 64.0, 0.0],
                    0,
                    20.0,
                    0.0,
                    0.0,
                    0,
                    String::new(),
                )]
            }
            fn broadcast(&self, t: horizon_client::mod_api::Text) {
                self.lines.lock().unwrap().push(t.plain());
            }
            fn open_menu(
                &mut self,
                _u: u128,
                _r: u8,
                _t: horizon_client::mod_api::Text,
                _i: Vec<Option<ItemStack>>,
            ) -> Option<i32> {
                None
            }
            fn set_menu_items(&mut self, _u: u128, _i: Vec<Option<ItemStack>>) {}
            fn close_menu(&mut self, _u: u128) {}
            fn menu_contents(&self, _u: u128) -> Option<Vec<Option<ItemStack>>> {
                None
            }
            fn menu_cursor(&self, _u: u128) -> Option<ItemStack> {
                None
            }
        }
        let (dir3, path3) = hmod(
            "menu_cont",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("net_from_client", function(event)
                  horizon.open_menu(event.player, 1, "T", { { id = "minecraft:stick", count = 1 } })
                  horizon.broadcast("after")
                end)
                "#,
            )],
        );
        let m3 = LuaMod::load(&path3, LuaSide::Server, &[]).unwrap();
        let mods3: Vec<Box<dyn Hooks>> = vec![Box::new(m3)];
        let mut host3 = NoneHost {
            lines: M::new(Vec::new()),
        };
        horizon_client::mod_api::dispatch_net_from_client(
            &mut host3,
            &mods3,
            0x123e4567_e89b_12d3_a456_426614174000,
            "x",
            b"",
        );
        assert_eq!(
            host3.lines.lock().unwrap().as_slice(),
            ["after"],
            "broadcast still runs after None open_menu"
        );
        std::fs::remove_dir_all(&dir3).unwrap();
    }

    #[test]
    fn menu_helpers_raise_in_client_and_draw() {
        // client stubs at load
        let (dir, path) = hmod(
            "menu_client_stubs",
            &[(
                "scripts/client/init.lua",
                br#"
                local function expect(name, f)
                  local ok, err = pcall(f)
                  assert(not ok, name.." should fail")
                  assert(string.find(tostring(err), "horizon."..name.." is only available in scripts/server/", 1, true), name..": "..tostring(err))
                end
                expect("open_menu", function() horizon.open_menu("steve", 1, "T", {}) end)
                expect("open_menu_with_input", function() horizon.open_menu_with_input("steve", 1, "T", {}, {start=0, ["end"]=1}) end)
                expect("set_menu_items", function() horizon.set_menu_items("steve", {}) end)
                expect("close_menu", function() horizon.close_menu("steve") end)
                expect("menu_contents", function() horizon.menu_contents("steve") end)
                expect("menu_cursor", function() horizon.menu_cursor("steve") end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        assert_eq!(m.side(), LuaSide::Client);
        std::fs::remove_dir_all(&dir).unwrap();
        // draw hook on client
        let (dir2, path2) = hmod(
            "menu_draw",
            &[(
                "scripts/client/init.lua",
                br#"
                horizon.on("hud", function(event)
                  __witness = (__witness or 0) + 1
                  local function tryit(name, f)
                    local ok, e = pcall(f)
                    if not ok then _G[name] = tostring(e) else _G[name] = "ran" end
                  end
                  tryit("OPEN", function() horizon.open_menu("steve", 1, "T", {}) end)
                  tryit("INPUT", function() horizon.open_menu_with_input("steve", 1, "T", {}, {start=0, ["end"]=1}) end)
                  tryit("SET", function() horizon.set_menu_items("steve", {}) end)
                  tryit("CLOSE", function() horizon.close_menu("steve") end)
                  tryit("CONTENTS", function() horizon.menu_contents("steve") end)
                  tryit("CURSOR", function() horizon.menu_cursor("steve") end)
                end)
                "#,
            )],
        );
        let m2 = LuaMod::load(&path2, LuaSide::Client, &[]).unwrap();
        let state = m2.lua.clone();
        let mods2: Vec<Box<dyn Hooks>> = vec![Box::new(m2)];
        struct DHost;
        impl Host for DHost {
            fn dimension(&self) -> &'static str {
                "minecraft:overworld"
            }
            fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {
                vec![]
            }
        }
        let mut host = DHost;
        let mut batch = horizon_client::mod_api::HudBatch::new([800, 600]);
        let mut corners = horizon_client::mod_api::Corners::new(800.0, 600.0, 0.0);
        horizon_client::mod_api::dispatch_hud(&mut host, &mods2, &mut batch, &mut corners);
        assert_eq!(state.globals().get::<i64>("__witness").unwrap(), 1);
        for (k, name) in [
            ("OPEN", "open_menu"),
            ("INPUT", "open_menu_with_input"),
            ("SET", "set_menu_items"),
            ("CLOSE", "close_menu"),
            ("CONTENTS", "menu_contents"),
            ("CURSOR", "menu_cursor"),
        ] {
            let v: String = state.globals().get(k).unwrap();
            assert!(
                v.contains(&format!(
                    "horizon.{name} is only available in scripts/server/"
                )),
                "{k}: {v}"
            );
        }
        std::fs::remove_dir_all(&dir2).unwrap();
    }

    #[test]
    fn menu_clicked_event_round_trip() {
        let (dir, path) = hmod(
            "menu_clicked",
            &[(
                "scripts/server/init.lua",
                br#"
                horizon.on("menu_clicked", function(event)
                  __W = event.window
                  __S = event.slot
                  __UUID = event.uuid
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
        let state = m.lua.clone();
        let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
        struct EHost;
        impl Host for EHost {
            fn dimension(&self) -> &'static str {
                "minecraft:overworld"
            }
        }
        let mut host = EHost;
        horizon_client::mod_api::dispatch_menu_clicked(
            &mut host,
            &mods,
            0x123e4567_e89b_12d3_a456_426614174000,
            42,
            5,
            0,
            0,
        );
        assert_eq!(state.globals().get::<i64>("__W").unwrap(), 42);
        assert_eq!(state.globals().get::<i64>("__S").unwrap(), 5);
        assert_eq!(
            state.globals().get::<String>("__UUID").unwrap(),
            "123e4567-e89b-12d3-a456-426614174000"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
