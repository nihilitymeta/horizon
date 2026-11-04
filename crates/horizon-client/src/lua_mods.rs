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
//! mods ([`status`]) and reloads one on demand ([`request_reload`]).
use crate::mod_api::{
    AttachmentCodec, AttachmentType, BlockEntityEvent, BlockEntityTickEvent, BlockEntityTicksEvent,
    BlockPos, BlockStateId, Compound, DataPackFilesEvent, EntityTickListenedEvent,
    EntityTickPostEvent, Flow, Holder, Hooks, Host, InteractEntityEvent, LevelDimension, LevelHost,
    LevelWriteHost, LivingIncomingDamageEvent, MobAiRegisterEvent, MobKilledEvent, MobSpawnedEvent,
    MobView, RandomTickEvent, RightClickBlockEvent, ServerTickEvent, ServerTickPhase,
    TickBlockEvent, WorldLifecycleEvent,
};
use crate::mod_api::{
    AttackEntityEvent, DestroyBlockEvent, InventoryTickEvent, ItemPickedUpEvent, ItemStack,
    LivingEntityUseItemFinishEvent, MobDiedEvent, PlaceBlockEvent, PlayerDiedEvent,
    PlayerLoggedInEvent, PlayerLoggedOutEvent, PlayerRespawnedEvent, UseItemEvent, UseItemOnEvent,
};
use crate::mod_api::{
    EntityJoinLevelEvent, ExplosionDetonateEvent, HearsProjectileImpactsEvent, HurtEnemyEvent,
    ItemCraftedEvent, ItemDroppedEvent, LivingEntityUseItemStartEvent, LivingEquipmentChangeEvent,
    LivingEquipmentListenedEvent, LivingFallEvent, LivingHealEvent, LivingKnockbackEvent,
    LootDropsEvent, PlayerBedEnterEvent, PlayerChangedDimensionEvent, PlayerChatEvent,
    PlayerCommandEvent, PlayerContainerOpenEvent, PlayerExpChangeEvent, PlayerTickEvent,
    ProjectileImpactEvent, ProjectileLaunchEvent,
};
use crate::mod_api::{NativeCommand, PerformCommandEvent, RegisterCommandsEvent};
use horizon_mods::{Capability, Content, Package};
use mlua::{Function, Lua, MultiValue, Table, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What the developer tools show of one scripted mod (docs/dev-tools.md).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModStatus {
    /// The mod's id, or its path's file name when it did not load.
    pub id: String,
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

/// Every scripted mod's status, in load order.
static STATUS: Mutex<Vec<ModStatus>> = Mutex::new(Vec::new());
/// Reloads asked for from the developer tools: by mod id, or everyone with a
/// new generation of `RELOAD_ALL`. Served on the server thread at the start
/// of a tick.
static RELOAD: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RELOAD_ALL: AtomicU32 = AtomicU32::new(0);

/// The scripted mods and what is known of them, for the developer tools.
pub fn status() -> Vec<ModStatus> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Ask the mod `id` (None: every mod) to read its scripts again and run
/// `init.lua` afresh at the next server tick, as `/reload` makes it do.
pub fn request_reload(id: Option<&str>) {
    match id {
        Some(id) => RELOAD
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(id.to_owned()),
        None => {
            RELOAD_ALL.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A line for the developer tools' console, when they are on.
fn to_console(error: bool, line: String) {
    if crate::dev_tools::enabled() {
        use crate::dev_tools::console::{Kind, push};
        push(if error { Kind::Err } else { Kind::Out }, line);
    }
}

fn set_status(status: ModStatus) {
    let mut all = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    match all.iter_mut().find(|s| s.id == status.id) {
        Some(s) => *s = status,
        None => all.push(status),
    }
}

pub mod goals;
pub mod json;
pub mod nbt;

/// Wall-clock time one handler or one goal function may take before Luau's
/// interrupt stops it with an error: the server thread runs them, so an
/// endless loop would otherwise freeze the game.
pub const HANDLER_BUDGET: Duration = Duration::from_millis(250);
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

/// The file a scripted mod starts from, inside its folder.
pub const ENTRY: &str = horizon_mods::content::SCRIPT_ENTRY;

/// The hooks a script may register for with `horizon.on` (`docs/lua-bindings.md`):
/// a name not here is refused when the script loads, so a misspelling is a
/// load error with its line, not a handler that never runs. `command` is
/// `horizon.command`'s own.
pub const LUA_HOOK_NAMES: &[&str] = &[
    "server_tick",
    "world_lifecycle",
    "mob_spawned",
    "mob_ai_register",
    "random_tick",
    "tick_block",
    "right_click_block",
    "block_entity_tick",
    "mob_hurt",
    "player_hurt",
    "mob_killed",
    "mob_interact",
    "mob_tick",
    "mob_died",
    "mob_fall",
    "player_fall",
    "mob_heal",
    "mob_knockback",
    "player_knockback",
    "mob_equipped",
    "entity_joined",
    "item_use",
    "item_use_on",
    "item_hit",
    "item_hurt_enemy",
    "item_use_start",
    "item_finished",
    "item_tick",
    "item_picked_up",
    "item_crafted",
    "item_dropped",
    "loot_drops",
    "projectile_impact",
    "projectile_launched",
    "explosion",
    "player_joined",
    "player_left",
    "player_respawned",
    "player_died",
    "player_tick",
    "player_chat",
    "player_command",
    "player_changed_dimension",
    "player_bed_enter",
    "player_exp_change",
    "player_container_open",
    "block_broken",
    "block_placed",
    "command",
];

/// The content mod's manifest, which names the mod.
const MANIFEST: &str = "horizon-mod.toml";
/// The Lua global the game provides.
const GLOBAL: &str = "horizon";
/// The registry key of the handlers table: `name -> { fn, fn, ... }`.
const HANDLERS: &str = "horizon_handlers";
/// The registry key of the handlers that watch one kind of mob
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
return function(slot)
  return function(handlers, host, full, ...)
    -- A hook fired from inside a handler (a summon answered at once tells of
    -- the spawn) runs nested: the outer call's host is kept for it. slot[3]
    -- counts the runners in progress: an action outside any is refused.
    local outer_host, outer_full, depth = slot[1], slot[2], slot[3] or 0
    slot[1] = host
    slot[2] = full
    slot[3] = depth + 1
    local failed
    for _, f in ipairs(handlers) do
      local ok, e = pcall(f, ...)
      if not ok then failed = e break end
    end
    slot[1] = outer_host
    slot[2] = outer_full
    slot[3] = depth
    if failed ~= nil then error(failed, 0) end
  end
end
"#;
/// The registry key of the table holding the call in progress: `[1]` its
/// read function (`horizon.block` and the other reads go through it; nil in
/// a hook without a host), `[2]` whether the host is a full one, `[3]` how
/// many runners are in progress (0: no hook's handlers run, and an action
/// has nowhere to go).
const HOST_SLOT: &str = "horizon_host";
/// The registry key of the timers' own `server_tick` handler, so a listing
/// of the handlers can leave it out.
pub(crate) const TIMERS_HANDLER: &str = "horizon_timers";
/// `horizon.block`, `block_property`, `mob`, `mobs`, `players`, `player`,
/// `time` and `raycast`: wrappers over the call's read function, which the
/// runner keeps in the slot while the handlers run; outside a call there is
/// none, and a read is an error.
const READS_CODE: &str = r#"
return function(slot, horizon)
  local function read(what, ...)
    local f = slot[1]
    if f == nil then
      error("horizon." .. what .. ": reads work inside a hook with a host (not in mob_hurt, mob_ai_register or a goal)", 3)
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
  horizon.light = function(x, y, z) return read("light", x, y, z) end
  horizon.surface_height = function(x, z, heightmap) return read("surface_height", x, z, heightmap) end
  horizon.inventory = function(who) return read("inventory", who) end
  horizon.count_items = function(who, item) return read("count_items", who, item) end
  horizon.carried_items = function(who) return read("carried_items", who) end
  horizon.data = function(path) return read("data", path) end
  -- A refused registration (a hook name the runtime does not know) is
  -- raised from the script's own line, as any Lua error is.
  local on = horizon.on
  horizon.on = function(name, f, kind)
    local ok, e = pcall(on, name, f, kind)
    if not ok then error(tostring(e), 2) end
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
/// the server's tick (the end of a tick while the world runs), as a handler
/// of their own; a timer that fails is reported to the log (`report`) and,
/// if repeating, kept, and the timers after it run: the handler itself does
/// not fail, so what the other timers asked for and saved stands.
const TIMERS_CODE: &str = r#"
return function(horizon, report)
  local timers, tick = {}, 0
  horizon.after = function(ticks, fn)
    timers[#timers + 1] = { due = tick + math.max(1, math.floor(ticks)), fn = fn }
  end
  horizon.every = function(ticks, fn)
    local n = math.max(1, math.floor(ticks))
    timers[#timers + 1] = { due = tick + n, every = n, fn = fn }
  end
  horizon.ticks = function() return tick end
  local function run_timers(phase, running)
    if phase ~= "end" or not running then return end
    tick = tick + 1
    local i = 1
    while i <= #timers do
      local t = timers[i]
      if t.due <= tick then
        if t.every then t.due = tick + t.every; i = i + 1 else table.remove(timers, i) end
        local ok, e = pcall(t.fn)
        if not ok then report(tostring(e)) end
      else
        i = i + 1
      end
    end
  end
  horizon.on("server_tick", run_timers)
  return run_timers
end
"#;

/// What a hook's handlers may read from while they run: the hook's host.
/// It reaches Lua as a function of the call's scope (`Lua::scope`), gone
/// when the call returns, whatever kept it.
enum Reader<'a> {
    /// A hook without a host (`mob_hurt`, `mob_ai_register`): reads fail.
    None,
    Host(&'a mut dyn Host),
    Level(&'a dyn LevelHost),
}

/// A saved table a call handed out (`horizon.world_data`,
/// `horizon.player_data`): written back when the call ends.
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
fn player_table_of(lua: &Lua, p: &crate::mod_api::NativeServerPlayer) -> mlua::Result<Table> {
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

/// An item stack as a table: `id`, `count`, `damage`, `plain` (no
/// components beyond count and damage).
fn item_table_of(lua: &Lua, stack: &ItemStack) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", &*stack.item)?;
    t.set("count", stack.count)?;
    t.set("damage", stack.damage)?;
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
        Ok(match what {
            "block" => {
                let (x, y, z): (i32, i32, i32) = FromLuaMulti::from_lua_multi(rest, lua)?;
                let id = match reader {
                    Reader::Host(h) => h.block([x, y, z]),
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
            "world_data" | "player_data" => {
                let Reader::Host(h) = reader else {
                    return Err(level_only());
                };
                let (holder, attachment) = if what == "world_data" {
                    (Holder::Level, &self.world_attachment)
                } else {
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
            _ => {
                let Reader::Host(h) = reader else {
                    return Err(level_only());
                };
                let h: &dyn Host = &**h;
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
                    "players" => {
                        let out = lua.create_table()?;
                        for p in h.level_players() {
                            out.push(player_table_of(lua, &p)?)?;
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
                            Some(p) => Value::Table(player_table_of(lua, &p)?),
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
                        use crate::mod_api::NativeHeightmap;
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
                        let Reader::Host(h) = reader else {
                            unreachable!()
                        };
                        let stacks: Vec<Option<ItemStack>> = if acting {
                            (0..36).map(|slot| h.carried_stack(slot).cloned()).collect()
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
                        let Reader::Host(h) = reader else {
                            unreachable!()
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
    let install: Function = lua.load(READS_CODE).set_name("horizon").eval()?;
    install.call::<()>((slot, api))
}

/// Installs the timers (`TIMERS_CODE`) on the `horizon` table: their handler
/// is the first `server_tick` handler.
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
) -> mlua::Result<Option<crate::mod_api::Text>> {
    use crate::mod_api::Text;
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
            "number" => match word.parse::<f64>() {
                Ok(n) => args.set(*name, n)?,
                Err(_) => {
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

fn install_timers(lua: &Lua, api: &Table, id: &str) -> mlua::Result<()> {
    let install: Function = lua.load(TIMERS_CODE).set_name("horizon").eval()?;
    let id = id.to_owned();
    let report = lua.create_function(move |_, message: String| {
        log::error!("[lua:{id}] server_tick: timer: {message}");
        Ok(())
    })?;
    let handler: Function = install.call((api, report))?;
    lua.set_named_registry_value(TIMERS_HANDLER, handler)
}
/// The registry key of the modules `require` ran: `name -> value`.
const LOADED: &str = "horizon_loaded";

/// One scripted mod: its folder and its Lua state.
pub struct LuaMod {
    id: String,
    /// The mod's version, from its manifest (`horizon.mod_version`).
    version: String,
    /// The versions of the mods it depends on, by id (`horizon.libraries`).
    library_versions: Vec<(String, String)>,
    /// The `.hmod` or folder the scripts came from.
    source: PathBuf,
    lua: Lua,
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
    /// server's tree; their handlers are kind handlers of the `command` hook.
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
    /// The mod's saved data on a mob (`data` in the mob hooks): the engine's
    /// attachment under the mod's id, saved with the mob.
    attachment: AttachmentType<Compound>,
    /// The mod's saved data on the world (`horizon.world_data()`) and on a
    /// player (`horizon.player_data(who)`): attachments on the level and
    /// the player, under the mod's id.
    world_attachment: AttachmentType<Compound>,
    player_attachment: AttachmentType<Compound>,
    /// The names the player hooks told, by uuid: the name of a player the
    /// host has no record of at the moment (one dying, whose state is out of
    /// reach then).
    names: Mutex<HashMap<u128, String>>,
    /// The mod's settings file (`horizon.config`): `config/<id>.json` of
    /// the game folder; none when the folder is not known (a test).
    config_path: Option<PathBuf>,
    /// Reloads done, for the status.
    reloads: AtomicU32,
    /// The `RELOAD_ALL` generation served last.
    reload_seen: AtomicU32,
}

/// A mod's goal definitions, shared with the `horizon.goal` function.
type Goals = Arc<Mutex<HashMap<String, goals::GoalDef>>>;
/// The kinds watched by hook, shared with the `horizon.on` function.
type Kinds = Arc<Mutex<HashMap<String, HashSet<String>>>>;

/// A mob kind as the mob hooks name it: `minecraft:zombie`, or a content
/// mod's own id as it is.
fn qualify(kind: &str) -> String {
    if kind.contains(':') {
        kind.to_owned()
    } else {
        ["minecraft:", kind].concat()
    }
}

/// The mod's attachment on mobs: its saved data, a compound under the mod's
/// id in the engine's `horizon:attachments`, kept on disk as it is.
fn mod_attachment(id: &str) -> AttachmentType<Compound> {
    mod_attachment_named(id, "data")
}

/// [`mod_attachment`] under another name: one per holder kind.
fn mod_attachment_named(id: &str, name: &'static str) -> AttachmentType<Compound> {
    AttachmentType {
        namespace: crate::mobs::native_goal::intern(id),
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
/// folders of the command line, when they have capability `scripts`. One
/// whose scripts fail to load is left out with the error in the log.
pub fn discover(args: impl Iterator<Item = String>) -> Vec<Box<dyn Hooks>> {
    let mut dev: Vec<PathBuf> = Vec::new();
    let mut game_dir: Option<PathBuf> = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
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
    let enabled = crate::mod_list::ModList::load(&crate::logging::shared_dir(&here)).enabled;
    let mut paths = horizon_mods::find(&here.join(crate::mod_list::DIR));
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
        if !scripted && dev.contains(path) && path.join(ENTRY).is_file() {
            log::warn!(
                "Horizon: {} has {ENTRY} but no capability scripts in {MANIFEST}: not run",
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
    let mut mods: Vec<Box<dyn Hooks>> = Vec::new();
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
        match LuaMod::load_with(&c.path, &libraries, Some(&config_dir)) {
            Ok(lua) => {
                log::info!(
                    "Horizon: Lua mod {} loaded from {}",
                    lua.id,
                    c.path.display()
                );
                lua.publish(None);
                mods.push(Box::new(lua));
            }
            Err(e) => {
                log::error!("Horizon: Lua mod in {} not loaded: {e}", c.path.display());
                to_console(true, format!("[{}] not loaded: {e}", c.path.display()));
                set_status(ModStatus {
                    id: c.id.clone(),
                    source: c.path.display().to_string(),
                    error: Some(e),
                    ..Default::default()
                });
            }
        }
    }
    mods
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
    let loaded = LuaMod::load_with(path, &libraries, Some(&config));
    let _ = std::fs::remove_dir_all(&config);
    let m = loaded?;
    lines.push(format!("{}: scripts loaded", m.id));
    if let Ok(scripts) = m.scripts.lock() {
        let mut names: Vec<&String> = scripts.keys().collect();
        names.sort();
        for name in names {
            lines.push(format!("  {name}"));
        }
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
    let mods = [crate::mod_content::ContentMod {
        id: id.to_owned(),
        content,
    }];
    let built = crate::mod_content::build(base, &mods, &out);
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
    /// Load the mod at `path` (a `.hmod` or a mod folder): its id from
    /// `horizon-mod.toml`, its scripts read once, `scripts/init.lua` run.
    pub fn load(path: &Path, libraries: &[(String, PathBuf)]) -> Result<Self, String> {
        Self::load_with(path, libraries, None)
    }

    /// `load`, with `config_dir` the folder of the mods' settings files
    /// (`config/` of the game folder: `horizon.config` reads `<id>.json`
    /// there, at load too).
    pub fn load_with(
        path: &Path,
        libraries: &[(String, PathBuf)],
        config_dir: Option<&Path>,
    ) -> Result<Self, String> {
        let (id, version, scripts) = Self::read_scripts(path)?;
        let (library_scripts, library_versions) = Self::read_libraries(libraries)?;
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY).map_err(|e| e.to_string())?;
        let budget = Budget::default();
        budget.install(&lua);
        let attachment = mod_attachment(&id);
        let world_attachment = mod_attachment_named(&id, "world");
        let player_attachment = mod_attachment_named(&id, "player");
        let config_path = config_dir.map(|d| d.join(format!("{id}.json")));
        let m = LuaMod {
            id,
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
            attachment,
            world_attachment,
            player_attachment,
            config_path,
            reloads: AtomicU32::new(0),
            reload_seen: AtomicU32::new(RELOAD_ALL.load(Ordering::Relaxed)),
        };
        m.install_api().map_err(|e| e.to_string())?;
        m.run(ENTRY).map_err(|e| e.to_string())?;
        Ok(m)
    }

    /// The mod's id, version and scripts, read from its `.hmod` or folder.
    fn read_scripts(path: &Path) -> Result<(String, String, HashMap<String, String>), String> {
        let at = |e: String| format!("{}: {e}", path.display());
        let package = Package::read(path).map_err(at)?;
        let manifest = package.manifest;
        if !manifest.capabilities.contains(&Capability::Scripts) {
            return Err(at(format!("no capability scripts in {MANIFEST}")));
        }
        let content = Content::read(path, &manifest).map_err(at)?;
        let mut reader = content.reader().map_err(at)?;
        let mut scripts = HashMap::with_capacity(content.scripts.len());
        for name in &content.scripts {
            let bytes = reader.read(name).map_err(at)?;
            let code = String::from_utf8(bytes).map_err(|_| at(format!("{name}: not UTF-8")))?;
            scripts.insert(name.clone(), code);
        }
        Ok((manifest.id, manifest.version, scripts))
    }

    /// The libraries' scripts, the first library's winning a shared name.
    /// A library without scripts (content only) contributes none.
    /// The libraries' scripts, the first library's winning a shared name, and
    /// each library's version by id. A library without scripts (content only)
    /// contributes none.
    fn read_libraries(
        libraries: &[(String, PathBuf)],
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
                Self::read_scripts(path).map_err(|e| format!("library {id}: {e}"))?;
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
        let (_, _, scripts) = Self::read_scripts(&self.source)?;
        let (library_scripts, _) = Self::read_libraries(&self.libraries)?;
        let lua = &self.lua;
        // What the running mod registered, kept aside while the new script
        // runs: the registry tables, the timers (their handler and the
        // `horizon.after`/`every`/`ticks` closures over their list) and the
        // maps the `horizon` functions fill.
        let api: Table = lua
            .globals()
            .get::<Table>(GLOBAL)
            .map_err(|e| e.to_string())?;
        let stage = || -> mlua::Result<(Vec<(&str, Value)>, Vec<(&str, Value)>)> {
            let mut registry = Vec::new();
            for key in [HANDLERS, KIND_HANDLERS, LOADED, TIMERS_HANDLER] {
                registry.push((key, lua.named_registry_value::<Value>(key)?));
            }
            let mut timers = Vec::new();
            for name in ["after", "every", "ticks"] {
                timers.push((name, api.get::<Value>(name)?));
            }
            lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
            lua.set_named_registry_value(KIND_HANDLERS, lua.create_table()?)?;
            lua.set_named_registry_value(LOADED, lua.create_table()?)?;
            // The timers' handler is the first `server_tick` handler: a new
            // one, over an empty list, for the new script.
            install_timers(lua, &api, &self.id)?;
            Ok((registry, timers))
        };
        let (registry, timers) = stage().map_err(|e| e.to_string())?;
        fn swap<T>(m: &Mutex<T>, new: T) -> Result<T, String> {
            let mut guard = m.lock().map_err(|_| "a mod's registrations".to_owned())?;
            Ok(std::mem::replace(&mut *guard, new))
        }
        let old_commands = swap(&self.commands, Vec::new())?;
        let old_goals = swap(&self.goals, HashMap::new())?;
        let old_kinds = swap(&self.kinds, HashMap::new())?;
        let old_declared = swap(&self.block_entities, HashMap::new())?;
        let old_scripts = swap(&self.scripts, scripts)?;
        let old_library_scripts = swap(&self.library_scripts, library_scripts)?;
        let Err(e) = self.run(ENTRY) else {
            return Ok(());
        };
        // Back as it was: whatever the failed script registered is dropped
        // with the fresh tables.
        let restore = || -> mlua::Result<()> {
            for (key, value) in registry {
                lua.set_named_registry_value(key, value)?;
            }
            for (name, value) in timers {
                api.set(name, value)?;
            }
            Ok(())
        };
        let restored = restore();
        swap(&self.commands, old_commands)?;
        swap(&self.goals, old_goals)?;
        swap(&self.kinds, old_kinds)?;
        swap(&self.block_entities, old_declared)?;
        swap(&self.scripts, old_scripts)?;
        swap(&self.library_scripts, old_library_scripts)?;
        restored.map_err(|e| e.to_string())?;
        Err(e.to_string())
    }

    /// The mod's id, as its manifest says.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Reload because of `why`, and tell the log and the console how it went.
    fn reload_now(&self, why: &str) {
        match self.reload() {
            Ok(()) => {
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
        let all = RELOAD_ALL.load(Ordering::Relaxed);
        let mut asked = self.reload_seen.swap(all, Ordering::Relaxed) != all;
        let mut list = RELOAD.lock().unwrap_or_else(|e| e.into_inner());
        if list.contains(&self.id) {
            list.retain(|id| *id != self.id);
            asked = true;
        }
        asked
    }

    /// The status the developer tools show, with `error` as the last error.
    fn publish(&self, error: Option<String>) {
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
            source: self.source.display().to_string(),
            scripts: self.scripts.lock().map(|s| s.len()).unwrap_or(0),
            reloads: self.reloads.load(Ordering::Relaxed),
            handlers,
            goals,
            error,
        });
    }

    /// An error of a hook or an action: the log, the console, the status.
    fn error(&self, text: String) {
        log::error!("[lua:{}] {text}", self.id);
        to_console(true, format!("[{}] {text}", self.id));
        if let Some(s) = STATUS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .find(|s| s.id == self.id)
        {
            s.error = Some(text);
        }
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
        lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
        lua.set_named_registry_value(KIND_HANDLERS, lua.create_table()?)?;
        lua.set_named_registry_value(QUEUE, lua.create_table()?)?;
        lua.set_named_registry_value(LOADED, lua.create_table()?)?;
        let slot = lua.create_table()?;
        lua.set_named_registry_value(HOST_SLOT, &slot)?;
        let make_runner: Function = lua.load(RUNNER_CODE).set_name("horizon").eval()?;
        let runner: Function = make_runner.call(&slot)?;
        lua.set_named_registry_value(RUNNER, runner)?;
        let api = lua.create_table()?;
        let id = self.id.clone();
        api.set("mod_id", id.as_str())?;
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
        // `horizon.on(name, fn[, kind])`: a handler of the hook `name`; with
        // `kind`, only for mobs of that kind (the mob hooks), which keeps a
        // hook that fires for every mob free for the others.
        let kinds = self.kinds.clone();
        api.set(
            "on",
            lua.create_function(
                move |lua, (name, f, kind): (String, Function, Option<String>)| {
                    if !LUA_HOOK_NAMES.contains(&name.as_str()) {
                        return Err(mlua::Error::runtime(format!(
                            "horizon.on({name:?}): no such hook; the hooks are {}",
                            LUA_HOOK_NAMES.join(", ")
                        )));
                    }
                    let list = match kind {
                        None => {
                            let handlers: Table = lua.named_registry_value(HANDLERS)?;
                            table_in(lua, &handlers, &name)?
                        }
                        Some(kind) => {
                            let kind = qualify(&kind);
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
        // runs it (the `command` hook, by the command's name).
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
                    let by_kind = table_in(lua, &all, "command")?;
                    kinds
                        .lock()
                        .map_err(|_| mlua::Error::runtime("kinds"))?
                        .entry("command".to_owned())
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
            "set_amount",
            "take_items",
            "give_items",
            "set_result",
            "set_count",
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
            "set_distance",
            "set_multiplier",
            "set_strength",
            "set_duration",
            "reply",
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
        // The reads and the timers over the actions: a summon with a host at
        // hand is a read (it answers the id).
        install_reads(lua, &api)?;
        install_timers(lua, &api, &self.id)?;
        // `horizon.config()`: the mod's own settings, `config/<id>.json` of
        // the game folder as a table (empty when there is none);
        // `horizon.save_config(t)` writes it. No host needed: at load too.
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
        // `require("name")`: `scripts/name.lua` of this mod, run once.
        api.set(
            "require",
            lua.create_function(move |lua, name: String| {
                // `modkit.entity`, `modkit/entity` or `@modkit/entity` (the
                // form a Luau editor resolves through `.luaurc` aliases) is
                // `scripts/modkit/entity.lua`, or
                // `scripts/modkit/entity/init.lua`: the mod's own first, then
                // the mods it depends on.
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
                let loaded: Table = lua.named_registry_value(LOADED)?;
                if let Some(v) = loaded.get::<Option<Value>>(name.as_str())? {
                    return Ok(v);
                }
                let path = name.replace('.', "/");
                let keys = [
                    format!("scripts/{path}.lua"),
                    format!("scripts/{path}/init.lua"),
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
                            "require({name:?}): no {} in the mod or the mods it depends on",
                            keys[0]
                        ))
                    })?;
                let v: Value = lua.load(code.as_str()).set_name(&key).eval()?;
                let v = if v.is_nil() { Value::Boolean(true) } else { v };
                loaded.set(name.as_str(), &v)?;
                Ok(v)
            })?,
        )?;
        lua.globals().set(GLOBAL, &api)?;
        lua.globals()
            .set("require", api.get::<Function>("require")?)?;
        Ok(())
    }

    /// Runs the script at `name` (`scripts/init.lua`).
    fn run(&self, name: &str) -> mlua::Result<()> {
        let code = self
            .scripts
            .lock()
            .ok()
            .and_then(|s| s.get(name).cloned())
            .ok_or_else(|| mlua::Error::runtime(format!("{name}: not in the mod")))?;
        self.budget.run(LOAD_BUDGET, || {
            self.lua.load(code.as_str()).set_name(name).exec()
        })
    }

    /// Whether any handler is registered for `hook`: the hooks that fire
    /// often (random ticks) build their arguments only then.
    fn has_handlers(&self, hook: &str) -> bool {
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

    /// The acting player as a table (`uuid`, `name`, `dimension`, `position`,
    /// `game_mode`, `health`), or nil when no player acts.
    fn player_table(&self, host: &dyn Host) -> mlua::Result<Value> {
        let lua = &self.lua;
        Ok(match host.acting_player() {
            Some(p) => {
                let t = player_table_of(lua, &p)?;
                t.set("held", self.held_table(host)?)?;
                Value::Table(t)
            }
            None => Value::Nil,
        })
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

    /// `{id, kind}` of an entity, for a hook without a host or a view.
    fn id_kind_table(&self, id: u32, kind: &str) -> mlua::Result<Table> {
        let t = self.lua.create_table()?;
        t.set("id", id)?;
        t.set("kind", kind)?;
        Ok(t)
    }

    /// Mob `id` of `kind` as a table for a handler: its view (`goals::view_table`)
    /// while the host has it alive, else just `id` and `kind` (a mob that has
    /// died, or a host without the level).
    fn mob_table(
        &self,
        host: &dyn Host,
        dimension: &str,
        id: u32,
        kind: &str,
    ) -> mlua::Result<Table> {
        match host.mob_view(dimension, id) {
            Some(view) => goals::view_table(&self.lua, &view),
            None => {
                let t = self.lua.create_table()?;
                t.set("id", id)?;
                t.set("kind", kind)?;
                Ok(t)
            }
        }
    }

    /// The player `uuid` as a table: the world's record when they are on,
    /// else just `uuid` and `name` (the one given, else the one a player
    /// hook told before).
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

    /// A hook about a player, told by uuid (`player_joined`...): `hook(player[,
    /// cause])`, with the host actions.
    fn player_hook(
        &self,
        host: &mut dyn Host,
        hook: &str,
        uuid: u128,
        name: &str,
        cause: Option<Option<&str>>,
    ) {
        if !self.has_handlers(hook) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, uuid, Some(name))?;
            let cause = match cause {
                Some(Some(c)) => Value::String(self.lua.create_string(c)?),
                _ => Value::Nil,
            };
            self.call_kind(
                hook,
                None,
                host,
                (player, cause),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }

    /// The mod's saved data on mob `id`, as the compound it is and as a table
    /// for a handler (an empty one for a mob without any yet); none when the
    /// host has no such mob (a client's copy, a mob already gone).
    fn mob_data(&self, host: &mut dyn Host, id: u32) -> mlua::Result<Option<(Compound, Table)>> {
        let before = match self.attachment.get_existing(host, Holder::Entity(id)) {
            Ok(Some(c)) => c,
            Ok(None) => Compound::default(),
            Err(_) => return Ok(None),
        };
        let table = nbt::compound_to_table(&self.lua, &before)?;
        Ok(Some((before, table)))
    }

    /// Stores what a handler changed in a mob's data (`mob_data`).
    fn store_mob_data(
        &self,
        host: &mut dyn Host,
        hook: &str,
        id: u32,
        data: Option<(Compound, Table)>,
    ) {
        let Some((before, table)) = data else {
            return;
        };
        match nbt::table_to_compound(&table, Some(&before)) {
            Ok(after) if after != before => {
                if let Err(e) = self.attachment.set(host, Holder::Entity(id), after) {
                    log::debug!("[lua:{}] {hook}: mob {id}'s data not kept: {e:?}", self.id);
                }
            }
            Ok(_) => {}
            Err(e) => {
                self.hook_error(hook, e);
            }
        }
    }

    /// A block hook on a level (`random_tick`, `tick_block`): handlers get
    /// `(dimension, block, x, y, z, props, data)`, `data` the block entity's
    /// saved table when the block has one (nil without), written back after
    /// the call, and may queue `set_block_property(key, value[, flags])`,
    /// `schedule_tick(delay)`, `summon(...)` and `handled()`, which skips the
    /// block's own tick.
    fn level_hook(
        &self,
        hook: &str,
        level: &mut dyn LevelWriteHost,
        dimension: LevelDimension,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Flow {
        let Some(block) = level.state_block(state) else {
            return Flow::Continue;
        };
        // Handlers for every block hear every random tick of the level
        // (hundreds a tick): a mod's own blocks register by block id.
        if !self.watches(hook, &block) {
            return Flow::Continue;
        }
        let at = [pos.x, pos.y, pos.z];
        let props = match self.lua.create_table() {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        for (k, v) in level.state_properties(state) {
            if let Err(e) = props.set(k, v) {
                return self.hook_error(hook, e);
            }
        }
        // The block entity's saved data, as a table the handler may change.
        let before = level.mod_block_entity_data(at);
        let data = match before
            .as_ref()
            .map(|c| nbt::compound_to_table(&self.lua, c))
            .transpose()
        {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let mut handled = false;
        let r = self.call_level(
            hook,
            Some(&block),
            level,
            (
                dimension_id(dimension),
                block.as_str(),
                pos.x,
                pos.y,
                pos.z,
                props,
                data.clone(),
            ),
            &mut |level, action, args| {
                match action {
                    "set_block_property" => {
                        let (key, value): (String, String) = (args.get(1)?, args.get(2)?);
                        let flags = args.get::<Option<i32>>(3)?.unwrap_or(UPDATE_CLIENTS);
                        level.set_block_property(at, &key, &value, flags);
                    }
                    "schedule_tick" => {
                        let delay: i32 = args.get(1)?;
                        level.schedule_block_tick(at, &block, delay);
                    }
                    // As the host's `summon(dimension, kind, x, y, z)`: this
                    // level's dimension is the only one here, so the first
                    // argument is accepted and ignored.
                    "summon" => {
                        let kind: String = args.get(2)?;
                        let pos = [
                            args.get::<f64>(3)?,
                            args.get::<f64>(4)?,
                            args.get::<f64>(5)?,
                        ];
                        if !level.summon(&kind, pos, Default::default()) {
                            log::warn!("[lua:{}] {hook}: summon {kind}: not here", self.id);
                        }
                    }
                    "handled" => handled = true,
                    other => log::warn!("[lua:{}] {hook}: {other} needs a host; dropped", self.id),
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if let (Some(before), Some(data)) = (before, data) {
            match nbt::table_to_compound(&data, Some(&before)) {
                Ok(after) if after != before => {
                    level.set_mod_block_entity_data(at, after);
                }
                Ok(_) => {}
                Err(e) => return self.hook_error(hook, e),
            }
        }
        if handled {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// Writes a block entity's data table back through `host` when the
    /// handlers changed it.
    fn store_data(
        &self,
        host: &mut dyn Host,
        hook: &str,
        pos: [i32; 3],
        before: Option<Compound>,
        data: Option<Table>,
    ) {
        if let (Some(before), Some(data)) = (before, data) {
            match nbt::table_to_compound(&data, Some(&before)) {
                Ok(after) if after != before => {
                    host.set_mod_block_entity_data(pos, after);
                }
                Ok(_) => {}
                Err(e) => {
                    self.hook_error(hook, e);
                }
            }
        }
    }

    fn hook_error(&self, hook: &str, e: mlua::Error) -> Flow {
        self.error(format!("{hook}: {e}"));
        Flow::Continue
    }

    /// Call the handlers of `hook` with `args`, then run what they queued
    /// with the hook's host (none: the actions are dropped with a warning).
    fn call(&self, host: Option<&mut dyn Host>, hook: &str, args: impl mlua::IntoLuaMulti) {
        self.call_for(host, hook, None, args);
    }

    /// `call` for a hook about a mob of `kind`: its handlers are those for
    /// every kind and those watching `kind`.
    fn call_for(
        &self,
        host: Option<&mut dyn Host>,
        hook: &str,
        kind: Option<&str>,
        args: impl mlua::IntoLuaMulti,
    ) {
        let r = match host {
            Some(host) => self.call_kind(hook, kind, host, args, &mut |host, action, a| {
                self.run_action(host, action, a)
            }),
            None => self.call_bare(hook, kind, args, &mut |action, _| {
                self.warn(format!("{hook}: {action} needs a host; dropped"));
                Ok(())
            }),
        };
        if let Err(e) = r {
            self.error(format!("{hook}: {e}"));
        }
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
        if outer.2 == 0 {
            queue.clear()?;
        }
        let start = queue.raw_len();
        // One call runs every handler; the read function lives for the
        // call's scope only, whatever kept it, and the saved tables it handed
        // out are written back after.
        let reader = std::cell::RefCell::new(reader);
        let fetched: std::cell::RefCell<Vec<Fetched>> = std::cell::RefCell::new(Vec::new());
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
            let full = matches!(&*reader.borrow(), Reader::Host(_));
            let mut args = args.into_lua_multi(lua)?;
            args.push_front(Value::Boolean(full));
            args.push_front(read);
            args.push_front(Value::Table(handlers));
            self.budget
                .run(HANDLER_BUDGET * count, || runner.call::<()>(args))
        });
        // The runner restores the slot itself when it gets that far; one
        // stopped by the budget (raised at every safepoint past the
        // deadline) may not, so it is restored here in any case.
        slot.set(1, outer.0)?;
        slot.set(2, outer.1)?;
        slot.set(3, outer.2)?;
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

    fn run_action(&self, host: &mut dyn Host, action: &str, args: &Table) -> mlua::Result<()> {
        match action {
            "broadcast" => {
                let line: String = args.get(1)?;
                host.broadcast(crate::mod_api::Text::literal(line));
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
                            host.tell(uuid, crate::mod_api::Text::literal(text));
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
                use crate::mod_api::{NativeBlockEdit, NativeBlockRun};
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
                let span = |a: i32, b: i32| i64::from(b) - i64::from(a) + 1;
                let columns = span(x1, x2) * span(z1, z2);
                if columns > FILL_COLUMNS {
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
            other => self.warn(format!("unknown action {other}")),
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

fn dimension_id(dimension: LevelDimension) -> &'static str {
    match dimension {
        LevelDimension::Overworld => "minecraft:overworld",
        LevelDimension::Nether => "minecraft:the_nether",
        LevelDimension::End => "minecraft:the_end",
    }
}

impl Hooks for LuaMod {
    /// The server's data tables load (the world opens, `/reload`): from the
    /// second load on, the scripts are read and run again, so an edited
    /// script is live after `/reload`.
    fn data_pack_files(&self, _host: &dyn Host, _event: &mut DataPackFilesEvent<'_>) {
        if self.loads.fetch_add(1, Ordering::Relaxed) == 0 {
            // The first data load: the logger is up now (it was not when the
            // mods were gathered), so this is where the mod says it runs.
            log::info!(
                "Horizon: Lua mod {} running from {}",
                self.id,
                self.source.display()
            );
            return;
        }
        self.reload_now("/reload");
    }
    /// A block that ticks randomly does: `random_tick(dimension, block, x, y,
    /// z, props, data)`, `data` the block entity's saved table when the block
    /// has one, written back after the call.
    fn random_tick(&self, level: &mut dyn LevelWriteHost, event: &mut RandomTickEvent) -> Flow {
        self.level_hook(
            "random_tick",
            level,
            event.dimension,
            event.pos,
            event.state,
        )
    }
    /// A scheduled block tick is due: `tick_block(dimension, block, x, y, z,
    /// props, data)`, `data` as in `random_tick`.
    fn tick_block(&self, level: &mut dyn LevelWriteHost, event: &mut TickBlockEvent) -> Flow {
        self.level_hook("tick_block", level, event.dimension, event.pos, event.state)
    }
    /// A player right-clicks a block: `right_click_block(x, y, z, block, face,
    /// player, held)`, `face` and `player.position` as `{x, y, z}`, `player`
    /// with `name`, `dimension` and `game_mode`, `held` with `item`, `count`
    /// and `plain`. Handlers may queue `set_block_property(key, value)`,
    /// `cancel()` (neither the block nor the item acts) and the host actions.
    fn right_click_block(&self, host: &mut dyn Host, event: &mut RightClickBlockEvent) -> Flow {
        let hook = "right_click_block";
        if !self.has_handlers(hook) {
            return Flow::Continue;
        }
        let lua = &self.lua;
        let [x, y, z] = event.pos;
        let block = host.block(event.pos);
        let tables = (|| -> mlua::Result<(Table, Value, Value)> {
            let face = goals::vec3(
                lua,
                [
                    event.face[0] as f64,
                    event.face[1] as f64,
                    event.face[2] as f64,
                ],
            )?;
            let player = self.player_table(host)?;
            let held = self.held_table(host)?;
            Ok((face, player, held))
        })();
        let (face, player, held) = match tables {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let before = host.mod_block_entity_data(event.pos);
        let data = match before
            .as_ref()
            .map(|c| nbt::compound_to_table(lua, c))
            .transpose()
        {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let mut cancel = false;
        let r = self.call_kind(
            hook,
            None,
            host,
            (x, y, z, block.as_str(), face, player, held, data.clone()),
            &mut |host, action, args| {
                match action {
                    "set_block_property" => {
                        let (key, value): (String, String) = (args.get(1)?, args.get(2)?);
                        host.set_block_property(event.pos, &key, &value);
                    }
                    "cancel" => cancel = true,
                    other => self.run_action(host, other, args)?,
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        self.store_data(host, hook, event.pos, before, data);
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A block the scripts gave a block entity gets one, with its starting data.
    fn block_entity(
        &self,
        _host: &mut dyn Host,
        event: &mut BlockEntityEvent<'_>,
    ) -> Option<(String, Compound)> {
        let declared = self.block_entities.lock().ok()?;
        let decl = declared.get(event.block)?;
        Some((event.block.to_owned(), decl.data.clone()))
    }
    fn block_entity_ticks(&self, _host: &dyn Host, event: &mut BlockEntityTicksEvent<'_>) {
        if self
            .block_entities
            .lock()
            .ok()
            .is_some_and(|d| d.get(event.block).is_some_and(|decl| decl.ticks))
        {
            event.ticks = true;
        }
    }
    /// A mod block entity's tick: `block_entity_tick(dimension, block, x, y, z, data)`;
    /// `data` is the saved compound as a table, written back after the call.
    fn block_entity_tick(&self, host: &mut dyn Host, event: &mut BlockEntityTickEvent<'_>) {
        let hook = "block_entity_tick";
        let id = event.entity.id.clone();
        if !self.watches(hook, &id) {
            return;
        }
        if !self
            .block_entities
            .lock()
            .ok()
            .is_some_and(|d| d.contains_key(&id))
        {
            return;
        }
        let before = event.entity.nbt.clone();
        let data = match nbt::compound_to_table(&self.lua, &before) {
            Ok(t) => t,
            Err(e) => {
                self.hook_error(hook, e);
                return;
            }
        };
        let [x, y, z] = event.pos;
        let pos = event.pos;
        let dimension = host.dimension();
        let r = self.call_kind(
            hook,
            Some(&id),
            host,
            (dimension, id.as_str(), x, y, z, data.clone()),
            &mut |host, action, args| {
                match action {
                    "set_block_property" => {
                        let (key, value): (String, String) = (args.get(1)?, args.get(2)?);
                        host.set_block_property(pos, &key, &value);
                    }
                    other => self.run_action(host, other, args)?,
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            self.hook_error(hook, e);
            return;
        }
        match nbt::table_to_compound(&data, Some(&before)) {
            Ok(after) if after != before => event.entity.nbt = after,
            Ok(_) => {}
            Err(e) => {
                self.hook_error(hook, e);
            }
        }
    }
    fn world_lifecycle(&self, host: &mut dyn Host, event: &mut WorldLifecycleEvent<'_>) {
        let phase = format!("{:?}", event.phase);
        self.call(Some(host), "world_lifecycle", phase);
    }
    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        let (phase, running) = match event.phase {
            ServerTickPhase::ServerTickStart { world_running } => ("start", world_running),
            ServerTickPhase::ServerTickEnd { world_running } => ("end", world_running),
        };
        // The developer tools asked for a reload: before this tick's handlers.
        if phase == "start" && self.reload_asked() {
            self.reload_now("the console");
        }
        self.call(Some(host), "server_tick", (phase, running));
    }
    /// A mob's goals as its species set them up: handlers see the mob's view
    /// and add Lua goals (`horizon.add_goal(priority, name)`,
    /// `horizon.add_target_goal`) or clear the selectors first
    /// (`horizon.clear_goals()`, `horizon.clear_target_goals()`).
    fn mob_ai_register(&self, event: &mut MobAiRegisterEvent<'_>) {
        let mob = &mut *event.mob;
        let view = match goals::view_table(&self.lua, &MobView::of(mob, None)) {
            Ok(view) => view,
            Err(e) => return self.error(format!("mob_ai_register: {e}")),
        };
        let kind = qualify(&mob.kind.id);
        let r = self.call_bare("mob_ai_register", Some(&kind), view, &mut |action, args| {
            match action {
                "add_goal" | "add_target_goal" => {
                    let (priority, name): (i32, String) = (args.get(1)?, args.get(2)?);
                    let def = self
                        .goals
                        .lock()
                        .ok()
                        .and_then(|g| g.get(&name).cloned())
                        .ok_or_else(|| {
                            mlua::Error::runtime(format!(
                                "{action}({priority}, {name:?}): no such goal; define it with horizon.goal"
                            ))
                        })?;
                    let goal = def.goal(self.lua.clone(), &self.id, self.budget.clone())?;
                    if action == "add_goal" {
                        mob.ai.goals.add(priority, goal);
                    } else {
                        mob.ai.targets.add(priority, goal);
                    }
                }
                "clear_goals" => mob.ai.goals.clear(),
                "clear_target_goals" => mob.ai.targets.clear(),
                other => self.warn(format!("mob_ai_register: {other} needs a host; dropped")),
            }
            Ok(())
        });
        if let Err(e) = r {
            self.error(format!("mob_ai_register: {e}"));
        }
    }
    fn mob_spawned(&self, host: &mut dyn Host, event: &mut MobSpawnedEvent<'_>) {
        let MobSpawnedEvent {
            dimension,
            kind,
            pos,
            natural,
            ..
        } = *event;
        let kind = qualify(kind);
        self.call_for(
            Some(host),
            "mob_spawned",
            Some(&kind),
            (dimension, kind.as_str(), pos[0], pos[1], pos[2], natural),
        );
    }
    /// A player right-clicks with an item, in the air or on a block the block
    /// let through: `item_use(item, player)`, `item` `{id, count, damage,
    /// plain}`; `horizon.cancel()` takes the use (the hand swings, the item's
    /// own use does not run). Register by item id to hear one item.
    fn use_item(&self, host: &mut dyn Host, event: &mut UseItemEvent<'_>) -> Flow {
        let hook = "item_use";
        let id = qualify(&event.stack.item);
        if !self.watches(hook, &id) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let r = (|| -> mlua::Result<()> {
            let item = item_table_of(&self.lua, event.stack)?;
            let player = self.player_table(host)?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (item, player),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player right-clicks a block with an item, once the block let the
    /// click through: `item_use_on(item, player, block, face)`, `block`
    /// `{x, y, z, id}`, `face` `{x, y, z}` (`{0, 1, 0}` the top);
    /// `horizon.cancel()` takes the use.
    fn use_item_on(&self, host: &mut dyn Host, event: &mut UseItemOnEvent<'_>) -> Flow {
        let hook = "item_use_on";
        let id = qualify(&event.stack.item);
        if !self.watches(hook, &id) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let item = item_table_of(lua, event.stack)?;
            let player = self.player_table(host)?;
            let block = lua.create_table()?;
            block.set("x", event.pos[0])?;
            block.set("y", event.pos[1])?;
            block.set("z", event.pos[2])?;
            block.set("id", host.block(event.pos))?;
            let face = goals::vec3(
                lua,
                [
                    event.face[0] as f64,
                    event.face[1] as f64,
                    event.face[2] as f64,
                ],
            )?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (item, player, block, face),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player attacks an entity with what they hold: `item_hit(item,
    /// player, target)`, `target` `{id, kind, position}`; `horizon.cancel()`
    /// and the blow does nothing. Register by the held item's id (an empty
    /// hand is `minecraft:air`).
    fn attack_entity(&self, host: &mut dyn Host, event: &mut AttackEntityEvent<'_>) -> Flow {
        let hook = "item_hit";
        let held = host.held();
        let id = match &held {
            Some((item, _, _)) => qualify(item),
            None => "minecraft:air".to_owned(),
        };
        if !self.watches(hook, &id) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let item = lua.create_table()?;
            match &held {
                Some((item_id, count, plain)) => {
                    item.set("id", item_id.as_str())?;
                    item.set("count", *count)?;
                    item.set("plain", *plain)?;
                }
                None => {
                    item.set("id", "minecraft:air")?;
                    item.set("count", 0)?;
                    item.set("plain", true)?;
                }
            }
            let player = self.player_table(host)?;
            let target = lua.create_table()?;
            target.set("id", event.entity)?;
            target.set("kind", qualify(event.kind))?;
            target.set("position", goals::vec3(lua, event.pos)?)?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (item, player, target),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player finished using an item over time (ate it, drank it, used a
    /// mod item for its duration): `item_finished(item, player)`. Handlers
    /// may queue `horizon.set_result(id, count)` for what stays in the hand
    /// (`set_result()` for nothing) and the host actions.
    fn living_entity_use_item_finish(
        &self,
        host: &mut dyn Host,
        event: &mut LivingEntityUseItemFinishEvent<'_>,
    ) {
        let hook = "item_finished";
        let id = qualify(&event.stack.item);
        if !self.watches(hook, &id) {
            return;
        }
        let mut result: Option<Option<ItemStack>> = None;
        let r = (|| -> mlua::Result<()> {
            let item = item_table_of(&self.lua, event.stack)?;
            let player = self.player_table(host)?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (item, player),
                &mut |host, action, args| {
                    match action {
                        "set_result" => {
                            result = Some(match args.get::<Option<String>>(1)? {
                                Some(item) => Some(ItemStack {
                                    item: qualify(&item).into(),
                                    count: args.get::<Option<u32>>(2)?.unwrap_or(1),
                                    damage: 0,
                                    extra: None,
                                }),
                                None => None,
                            });
                        }
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
            return;
        }
        if let Some(result) = result {
            *event.result = result;
        }
    }
    /// A server tick of a stack a player carries: `item_tick(item, player,
    /// slot, selected)`, `slot` as the inventory saves it (0 to 35, 100 to
    /// 103 armour, -106 the off hand), `selected` when it is the held one.
    /// Handlers may queue `horizon.set_count(n)` on that stack (0 empties the
    /// slot) and the host actions. It runs for every stack of every player
    /// each tick: register by item id.
    fn inventory_tick(&self, host: &mut dyn Host, event: &mut InventoryTickEvent) {
        let hook = "item_tick";
        let Some(stack) = host.carried_stack(event.slot).map(|s| s.clone()) else {
            return;
        };
        let id = qualify(&stack.item);
        if !self.watches(hook, &id) {
            return;
        }
        let slot = event.slot;
        let r = (|| -> mlua::Result<()> {
            let item = item_table_of(&self.lua, &stack)?;
            let player = self.player_table(host)?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (item, player, slot, event.selected),
                &mut |host, action, args| {
                    match action {
                        "set_count" => {
                            let count: u32 = args.get(1)?;
                            if let Some(s) = host.carried_stack(slot) {
                                s.count = count;
                            }
                        }
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A mob died, by any cause: `mob_died(dimension, mob, cause, data)`,
    /// `mob` its `{id, kind}` (gone: no view), `cause` `{damage_type,
    /// causing, player}` (`causing` the mob behind it or 0, `player` the
    /// player credited with the kill, or nil), `data` the mod's saved data
    /// on it while the game still holds it. Register by kind.
    fn mob_died(&self, host: &mut dyn Host, event: &mut MobDiedEvent<'_>) {
        let hook = "mob_died";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return;
        }
        let id = event.entity;
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let mob = self.mob_table(host, event.dimension, id, &kind)?;
            let cause = lua.create_table()?;
            cause.set("damage_type", event.damage_type)?;
            cause.set("causing", event.causing)?;
            if let Some(uuid) = event.player {
                cause.set("player", self.player_by_uuid(host, uuid, None)?)?;
            }
            let data = self.mob_data(host, id)?;
            self.call_kind(
                hook,
                Some(&kind),
                host,
                (
                    event.dimension,
                    mob,
                    cause,
                    data.as_ref().map(|(_, t)| t.clone()),
                ),
                &mut |host, action, args| self.run_action(host, action, args),
            )?;
            self.store_mob_data(host, hook, id, data);
            Ok(())
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A player joined the game: `player_joined(player)`.
    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        self.player_hook(host, "player_joined", event.player, event.name, None);
    }
    /// A player left the game, by any path: `player_left(player)`.
    fn player_logged_out(&self, host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        self.player_hook(host, "player_left", event.player, event.name, None);
    }
    /// A player respawned after a death: `player_respawned(player)`.
    fn player_respawned(&self, host: &mut dyn Host, event: &mut PlayerRespawnedEvent<'_>) {
        self.player_hook(host, "player_respawned", event.player, event.name, None);
    }
    /// A player died: `player_died(player, cause)`, `cause` the death
    /// message's key (`death.attack.lava`) when the game knows it, else nil.
    fn player_died(&self, host: &mut dyn Host, event: &mut PlayerDiedEvent<'_>) {
        self.player_hook(host, "player_died", event.player, "", Some(event.cause));
    }
    /// A player breaks a block, once the game allowed it: `block_broken(x,
    /// y, z, block, player)`; `horizon.cancel()` keeps the block (nothing
    /// breaks or drops). Register by block id.
    fn destroy_block(&self, host: &mut dyn Host, event: &mut DestroyBlockEvent) -> Flow {
        let hook = "block_broken";
        let block = host.block(event.pos);
        if !self.watches(hook, &block) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let [x, y, z] = event.pos;
        let r = (|| -> mlua::Result<()> {
            let player = self.player_table(host)?;
            self.call_kind(
                hook,
                Some(&block),
                host,
                (x, y, z, block.as_str(), player),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player placed a block: `block_placed(x, y, z, block, player, item)`,
    /// `item` the stack placed. It only hears (a use hook stops a placement
    /// before it). Register by block id.
    fn place_block(&self, host: &mut dyn Host, event: &mut PlaceBlockEvent<'_>) {
        let hook = "block_placed";
        let block = host.block(event.pos);
        if !self.watches(hook, &block) {
            return;
        }
        let [x, y, z] = event.pos;
        let r = (|| -> mlua::Result<()> {
            let player = self.player_table(host)?;
            let item = item_table_of(&self.lua, event.stack)?;
            self.call_kind(
                hook,
                Some(&block),
                host,
                (x, y, z, block.as_str(), player, item),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A player is about to pick up items from the ground:
    /// `item_picked_up(player, item, count)`; `horizon.cancel()` leaves them
    /// there. Register by item id.
    fn item_picked_up(&self, host: &mut dyn Host, event: &mut ItemPickedUpEvent<'_>) -> Flow {
        let hook = "item_picked_up";
        let id = qualify(event.item);
        if !self.watches(hook, &id) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, event.player, None)?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (player, id.as_str(), event.count),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A living player's server tick ends: `player_tick(player)`, the
    /// acting player's table; the host actions and the reads work. Every
    /// player, every tick: keep it light.
    fn player_tick(&self, host: &mut dyn Host, _event: &mut PlayerTickEvent) {
        let hook = "player_tick";
        if !self.has_handlers(hook) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let player = self.player_table(host)?;
            self.call_kind(hook, None, host, player, &mut |host, action, args| {
                self.run_action(host, action, args)
            })
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A living entity is about to handle a fall: `mob_fall(mob, distance,
    /// multiplier)` by kind (`player_fall` for a player), `mob` its `{id,
    /// kind}` (no host). Handlers may queue `set_distance(n)`,
    /// `set_multiplier(n)` and `cancel()` (no fall damage).
    fn living_fall(&self, event: &mut LivingFallEvent<'_>) -> Flow {
        let kind = qualify(event.kind);
        let (hook, by_kind) = if kind == "minecraft:player" {
            ("player_fall", None)
        } else {
            ("mob_fall", Some(kind.as_str()))
        };
        if by_kind.is_some_and(|k| !self.watches(hook, k))
            || (by_kind.is_none() && !self.has_handlers(hook))
        {
            return Flow::Continue;
        }
        let mut cancel = false;
        let (mut distance, mut multiplier) = (event.distance, event.multiplier);
        let mob = match self.id_kind_table(event.entity, &kind) {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let r = self.call_bare(
            hook,
            by_kind,
            (mob, event.distance, event.multiplier),
            &mut |action, args| {
                match action {
                    "set_distance" => distance = args.get::<f64>(1)?,
                    "set_multiplier" => multiplier = args.get::<f32>(1)?,
                    "cancel" => cancel = true,
                    other => {
                        log::warn!("[lua:{}] {hook}: {other} needs a host; dropped", self.id)
                    }
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if distance.is_finite() {
            event.distance = distance.max(0.0);
        }
        if multiplier.is_finite() {
            event.multiplier = multiplier.max(0.0);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A living entity is about to heal: `mob_heal(mob, amount)`, `mob` its
    /// `{id}` (the engine names no kind here, so every handler hears every
    /// entity; no host). Handlers may queue `set_amount(n)` and `cancel()`.
    fn living_heal(&self, event: &mut LivingHealEvent) -> Flow {
        let hook = "mob_heal";
        if !self.has_handlers(hook) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let mut amount = event.amount;
        let mob = match self.lua.create_table().and_then(|t| {
            t.set("id", event.entity)?;
            Ok(t)
        }) {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let r = self.call_bare(hook, None, (mob, event.amount), &mut |action, args| {
            match action {
                "set_amount" => amount = args.get::<f32>(1)?,
                "cancel" => cancel = true,
                other => log::warn!("[lua:{}] {hook}: {other} needs a host; dropped", self.id),
            }
            Ok(())
        });
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if amount.is_finite() {
            event.amount = amount.max(0.0);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A living entity is about to be knocked back: `mob_knockback(mob,
    /// damage_type, causing, direct, strength, x, z)` by kind
    /// (`player_knockback` for a player), `mob` its `{id, kind}` (mid-hit: no
    /// host). Handlers may queue `set_strength(n)` and `cancel()`.
    fn living_knockback(&self, event: &mut LivingKnockbackEvent<'_>) -> Flow {
        let kind = qualify(event.kind);
        let (hook, by_kind) = if kind == "minecraft:player" {
            ("player_knockback", None)
        } else {
            ("mob_knockback", Some(kind.as_str()))
        };
        if by_kind.is_some_and(|k| !self.watches(hook, k))
            || (by_kind.is_none() && !self.has_handlers(hook))
        {
            return Flow::Continue;
        }
        let mut cancel = false;
        let mut strength = event.strength;
        let mob = match self.id_kind_table(event.entity.max(0) as u32, &kind) {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let r = self.call_bare(
            hook,
            by_kind,
            (
                mob,
                event.damage_type,
                event.causing,
                event.direct,
                event.strength,
                event.x,
                event.z,
            ),
            &mut |action, args| {
                match action {
                    "set_strength" => strength = args.get::<f64>(1)?,
                    "cancel" => cancel = true,
                    other => {
                        log::warn!("[lua:{}] {hook}: {other} needs a host; dropped", self.id)
                    }
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if strength.is_finite() {
            event.strength = strength.max(0.0);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player's chat line is about to go to everyone:
    /// `player_chat(player, line)`, `player` their `{uuid, name}` (the hub
    /// is mid-packet: no host). Handlers may queue `cancel()` (nobody sees
    /// it) and `reply(text)` (a line back to the sender). The line itself
    /// cannot change: a signed message would no longer verify.
    fn player_chat(&self, event: &mut PlayerChatEvent<'_>) -> Flow {
        let hook = "player_chat";
        if !self.has_handlers(hook) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let mut replies: Vec<String> = Vec::new();
        let player = match self.lua.create_table().and_then(|t| {
            t.set("uuid", uuid_text(event.player))?;
            t.set("name", event.name)?;
            Ok(t)
        }) {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        if let Ok(mut names) = self.names.lock() {
            names.insert(event.player, event.name.to_owned());
        }
        let r = self.call_bare(hook, None, (player, event.message), &mut |action, args| {
            match action {
                "cancel" => cancel = true,
                "reply" => replies.push(args.get::<String>(1)?),
                other => log::warn!("[lua:{}] {hook}: {other} needs a host; dropped", self.id),
            }
            Ok(())
        });
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        for line in replies {
            event.replies.push(crate::mod_api::Text::literal(line));
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player typed a command: `player_command(player, line)`, the line
    /// without its slash, before it runs. Handlers may queue `cancel()`
    /// (nothing runs) and `reply(text)` (a line back to the player, the only
    /// way to answer here: no player's state is in place).
    fn player_command(&self, host: &mut dyn Host, event: &mut PlayerCommandEvent<'_>) -> Flow {
        let hook = "player_command";
        if !self.has_handlers(hook) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let mut replies: Vec<String> = Vec::new();
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, event.player, Some(event.name))?;
            self.call_kind(
                hook,
                None,
                host,
                (player, event.command),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        "reply" => replies.push(args.get::<String>(1)?),
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        for line in replies {
            event.replies.push(crate::mod_api::Text::literal(line));
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player took a crafting grid's result: `item_crafted(player, recipe,
    /// item)`, by the result's item id. It only hears.
    fn item_crafted(&self, host: &mut dyn Host, event: &mut ItemCraftedEvent<'_>) {
        let hook = "item_crafted";
        let id = qualify(&event.stack.item);
        if !self.watches(hook, &id) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, event.player, None)?;
            let item = item_table_of(&self.lua, event.stack)?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (player, event.recipe, item),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A player threw an item (the drop key, a menu's throw):
    /// `item_dropped(player, item, count)`, by item id. It only hears.
    fn item_dropped(&self, host: &mut dyn Host, event: &mut ItemDroppedEvent<'_>) {
        let hook = "item_dropped";
        let id = qualify(event.item);
        if !self.watches(hook, &id) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, event.player, None)?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (player, id.as_str(), event.count),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A block broke or a mob died and its loot is about to drop:
    /// `loot_drops(source, drops)`, registered by the block's id or the
    /// mob's kind. `source` has `table`, `dimension`, `origin`, and `block`
    /// (`{x, y, z, id}`) or `mob` (`{id, kind}`), `player` (the one credited,
    /// or nil), `tool`, `explosion`; `drops` is a sequence of `{id, count,
    /// damage, plain}` the handler changes in place (add, change or remove
    /// entries), written back after the call.
    fn loot_drops(&self, host: &mut dyn Host, event: &mut LootDropsEvent<'_>) {
        let hook = "loot_drops";
        let key = match (event.context.block, event.context.entity) {
            (Some((_, state)), _) => host.state_name(state),
            (None, Some((_, kind))) => qualify(kind),
            (None, None) => return,
        };
        let key = key.split('[').next().unwrap_or(&key).to_owned();
        if !self.watches(hook, &key) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let c = event.context;
            let source = lua.create_table()?;
            source.set("table", c.table)?;
            source.set("dimension", c.dimension)?;
            source.set("origin", goals::vec3(lua, c.origin)?)?;
            if let Some((pos, state)) = c.block {
                let block = lua.create_table()?;
                block.set("x", pos[0])?;
                block.set("y", pos[1])?;
                block.set("z", pos[2])?;
                block.set("id", key.as_str())?;
                block.set("state", host.state_name(state))?;
                source.set("block", block)?;
            }
            if let Some((id, kind)) = c.entity {
                source.set("mob", self.id_kind_table(id, &qualify(kind))?)?;
            }
            if let Some(uuid) = c.player {
                source.set("player", self.player_by_uuid(host, uuid, None)?)?;
            }
            if let Some(tool) = c.tool {
                source.set("tool", item_table_of(lua, tool)?)?;
            }
            if let Some(radius) = c.explosion {
                source.set("explosion", radius)?;
            }
            let drops = lua.create_table()?;
            for (i, stack) in event.drops.iter().enumerate() {
                drops.set(i + 1, item_table_of(lua, stack)?)?;
            }
            self.call_kind(
                hook,
                Some(&key),
                host,
                (source, drops.clone()),
                &mut |host, action, args| self.run_action(host, action, args),
            )?;
            // Written back: an entry with the item of the stack at its place
            // keeps that stack (its components), others are plain.
            let mut out = Vec::new();
            for (i, entry) in drops.sequence_values::<Table>().enumerate() {
                let entry = entry?;
                let id = qualify(&entry.get::<String>("id")?);
                let count: u32 = entry.get::<Option<u32>>("count")?.unwrap_or(1);
                if count == 0 {
                    continue;
                }
                let mut stack = match event.drops.get(i) {
                    Some(s) if *s.item == *id => s.clone(),
                    _ => ItemStack {
                        item: id.as_str().into(),
                        count,
                        damage: 0,
                        extra: None,
                    },
                };
                stack.count = count;
                out.push(stack);
            }
            *event.drops = out;
            Ok(())
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// Projectile impacts are heard when a script watches one
    /// (`projectile_impact` by projectile kind).
    fn hears_projectile_impacts(&self, _host: &dyn Host, event: &mut HearsProjectileImpactsEvent) {
        if self
            .kinds
            .lock()
            .is_ok_and(|k| k.get("projectile_impact").is_some_and(|s| !s.is_empty()))
        {
            event.hears = true;
        }
    }
    /// A projectile hit something, before the hit acts:
    /// `projectile_impact(projectile, kind, owner, hit)` by projectile kind;
    /// `hit` has `x`, `y`, `z` and either `entity` and `entity_kind` or
    /// `block` (`{x, y, z, id}`) and `face`. `horizon.cancel()` and the
    /// projectile flies on.
    fn projectile_impact(
        &self,
        host: &mut dyn Host,
        event: &mut ProjectileImpactEvent<'_>,
    ) -> Flow {
        let hook = "projectile_impact";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let hit = lua.create_table()?;
            hit.set("x", event.at[0])?;
            hit.set("y", event.at[1])?;
            hit.set("z", event.at[2])?;
            match event.entity {
                Some(id) => {
                    hit.set("entity", id)?;
                    hit.set("entity_kind", qualify(event.entity_kind))?;
                }
                None => {
                    let block = lua.create_table()?;
                    block.set("x", event.block[0])?;
                    block.set("y", event.block[1])?;
                    block.set("z", event.block[2])?;
                    block.set("id", host.block(event.block))?;
                    hit.set("block", block)?;
                    let face = lua.create_table()?;
                    face.set("x", event.face[0])?;
                    face.set("y", event.face[1])?;
                    face.set("z", event.face[2])?;
                    hit.set("face", face)?;
                }
            }
            self.call_kind(
                hook,
                Some(&kind),
                host,
                (event.projectile, kind.as_str(), event.owner, hit),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A projectile was launched: `projectile_launched(projectile, kind,
    /// player)` by projectile kind, `player` the launcher's table or nil
    /// (a mob's, a dispenser's). It only hears.
    fn projectile_launch(&self, host: &mut dyn Host, event: &mut ProjectileLaunchEvent<'_>) {
        let hook = "projectile_launched";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let player = match event.player {
                Some(uuid) => Value::Table(self.player_by_uuid(host, uuid, Some(event.name))?),
                None => Value::Nil,
            };
            self.call_kind(
                hook,
                Some(&kind),
                host,
                (event.entity, kind.as_str(), player),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// An explosion is about to break blocks: `explosion(dimension, center,
    /// blocks)`, `blocks` a sequence of `{x, y, z}` the handler changes in
    /// place (take entries out to spare them), written back after the call.
    fn explosion_detonate(&self, host: &mut dyn Host, event: &mut ExplosionDetonateEvent<'_>) {
        let hook = "explosion";
        if !self.has_handlers(hook) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let blocks = lua.create_table()?;
            for (i, pos) in event.blocks.iter().enumerate() {
                let t = lua.create_table()?;
                t.set("x", pos[0])?;
                t.set("y", pos[1])?;
                t.set("z", pos[2])?;
                blocks.set(i + 1, t)?;
            }
            self.call_kind(
                hook,
                None,
                host,
                (
                    event.dimension,
                    goals::vec3(lua, event.center)?,
                    blocks.clone(),
                ),
                &mut |host, action, args| self.run_action(host, action, args),
            )?;
            let mut out = Vec::new();
            for t in blocks.sequence_values::<Table>() {
                let t = t?;
                out.push([t.get::<i32>("x")?, t.get::<i32>("y")?, t.get::<i32>("z")?]);
            }
            *event.blocks = out;
            Ok(())
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A player's move to another level has ended:
    /// `player_changed_dimension(player, from, to)`.
    fn player_changed_dimension(
        &self,
        host: &mut dyn Host,
        event: &mut PlayerChangedDimensionEvent<'_>,
    ) {
        let hook = "player_changed_dimension";
        if !self.has_handlers(hook) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, event.player, Some(event.name))?;
            self.call_kind(
                hook,
                None,
                host,
                (player, event.from, event.to),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A player is about to enter a bed: `player_bed_enter(player, x, y, z)`;
    /// `horizon.cancel()` keeps them out.
    fn player_bed_enter(&self, host: &mut dyn Host, event: &mut PlayerBedEnterEvent<'_>) -> Flow {
        let hook = "player_bed_enter";
        if !self.has_handlers(hook) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, event.player, Some(event.name))?;
            self.call_kind(
                hook,
                None,
                host,
                (player, event.pos[0], event.pos[1], event.pos[2]),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player's experience is about to grow from an orb:
    /// `player_exp_change(player, amount)`; `horizon.set_amount(n)` changes
    /// the points added.
    fn player_exp_change(&self, host: &mut dyn Host, event: &mut PlayerExpChangeEvent<'_>) {
        let hook = "player_exp_change";
        if !self.has_handlers(hook) {
            return;
        }
        let mut amount = event.amount;
        let r = (|| -> mlua::Result<()> {
            let player = self.player_by_uuid(host, event.player, Some(event.name))?;
            self.call_kind(
                hook,
                None,
                host,
                (player, event.amount),
                &mut |host, action, args| {
                    match action {
                        "set_amount" => amount = args.get::<i32>(1)?,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
            return;
        }
        event.amount = amount.max(0);
    }
    /// A player opened a menu: `player_container_open(player, menu, pos)`,
    /// `menu` the menu type (`minecraft:generic_9x3`), `pos` the block it is
    /// of (`{x, y, z}`) or nil.
    fn player_container_open(&self, host: &mut dyn Host, event: &mut PlayerContainerOpenEvent<'_>) {
        let hook = "player_container_open";
        if !self.has_handlers(hook) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let player = self.player_by_uuid(host, event.player, Some(event.name))?;
            let pos = match event.pos {
                Some(p) => {
                    let t = lua.create_table()?;
                    t.set("x", p[0])?;
                    t.set("y", p[1])?;
                    t.set("z", p[2])?;
                    Value::Table(t)
                }
                None => Value::Nil,
            };
            self.call_kind(
                hook,
                None,
                host,
                (player, event.menu, pos),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// Equipment changes are watched while a script watches `mob_equipped`.
    fn living_equipment_listened(
        &self,
        _host: &mut dyn Host,
        event: &mut LivingEquipmentListenedEvent,
    ) {
        if self
            .kinds
            .lock()
            .is_ok_and(|k| k.get("mob_equipped").is_some_and(|s| !s.is_empty()))
        {
            event.listened = true;
        }
    }
    /// A mob's equipment slot changed: `mob_equipped(mob, slot, from, to)`
    /// by kind, `mob` its view, `from` and `to` item tables or nil.
    fn living_equipment_change(
        &self,
        host: &mut dyn Host,
        event: &mut LivingEquipmentChangeEvent<'_>,
    ) {
        let hook = "mob_equipped";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let dimension = host.dimension().to_owned();
            let mob = self.mob_table(host, &dimension, event.entity, &kind)?;
            let stack = |s: Option<&ItemStack>| -> mlua::Result<Value> {
                Ok(match s {
                    Some(s) => Value::Table(item_table_of(lua, s)?),
                    None => Value::Nil,
                })
            };
            let (from, to) = (stack(event.from)?, stack(event.to)?);
            self.call_kind(
                hook,
                Some(&kind),
                host,
                (mob, event.slot, from, to),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// An entity joins the level: `entity_joined(dimension, id, kind, x, y,
    /// z, loaded)` by kind (`minecraft:item`, `minecraft:arrow`, a mob),
    /// `loaded` true for one coming back with its chunk. `horizon.cancel()`
    /// keeps it out.
    fn entity_join_level(&self, host: &mut dyn Host, event: &mut EntityJoinLevelEvent<'_>) -> Flow {
        let hook = "entity_joined";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let r = self.call_kind(
            hook,
            Some(&kind),
            host,
            (
                event.dimension,
                event.entity,
                kind.as_str(),
                event.pos[0],
                event.pos[1],
                event.pos[2],
                event.loaded_from_disk,
            ),
            &mut |host, action, args| {
                match action {
                    "cancel" => cancel = true,
                    other => self.run_action(host, other, args)?,
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player's blow landed on a mob: `item_hurt_enemy(item, player,
    /// target)` by the held item's id (`item_hit` is before the blow). It
    /// only hears.
    fn hurt_enemy(&self, host: &mut dyn Host, event: &mut HurtEnemyEvent<'_>) {
        let hook = "item_hurt_enemy";
        let id = qualify(&event.stack.item);
        if !self.watches(hook, &id) {
            return;
        }
        let r = (|| -> mlua::Result<()> {
            let item = item_table_of(&self.lua, event.stack)?;
            let player = self.player_table(host)?;
            let target = self.id_kind_table(event.entity, &qualify(event.kind))?;
            self.call_kind(
                hook,
                Some(&id),
                host,
                (item, player, target),
                &mut |host, action, args| self.run_action(host, action, args),
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A player starts using an item over time (eating, drawing a bow):
    /// `item_use_start(item, player, hand, duration)` by item id, `hand`
    /// `"main"` or `"off"`. Handlers may queue `set_duration(ticks)` and
    /// `cancel()` (the use does not start).
    fn living_entity_use_item_start(
        &self,
        host: &mut dyn Host,
        event: &mut LivingEntityUseItemStartEvent<'_>,
    ) -> Flow {
        let hook = "item_use_start";
        let id = qualify(&event.stack.item);
        if event.entity.is_some() || !self.watches(hook, &id) {
            return Flow::Continue;
        }
        let mut cancel = false;
        let mut duration = event.duration;
        let r = (|| -> mlua::Result<()> {
            let item = item_table_of(&self.lua, event.stack)?;
            let player = self.player_table(host)?;
            let hand = format!("{:?}", event.hand).to_lowercase();
            self.call_kind(
                hook,
                Some(&id),
                host,
                (item, player, hand, event.duration),
                &mut |host, action, args| {
                    match action {
                        "set_duration" => duration = args.get::<i32>(1)?,
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        event.duration = duration.max(0);
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// The commands the scripts declared (`horizon.command`): into the
    /// server's tree, with their levels.
    fn register_commands(&self, _host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        if let Ok(commands) = self.commands.lock() {
            event.commands.extend(commands.iter().cloned());
        }
    }
    /// A player runs a command of the scripts': `command(player, args)` for
    /// the handler of that command, `args` the usage's arguments by name
    /// (`<n:integer>` a number, `<who:player>` a name...) plus `args.raw`,
    /// the line as typed. Handlers may queue `horizon.feedback(text)`, the
    /// answer to the player, and the host actions.
    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        let hook = "command";
        let line = event.command.trim();
        // The usage whose words the line starts with, the longest first:
        // `parasite count` before `parasite`.
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(usage) = self.commands.lock().ok().and_then(|c| {
            c.iter()
                .map(|c| c.usage.clone())
                .filter(|u| {
                    let name = command_words(u);
                    let n = name.split_whitespace().count();
                    n > 0
                        && words.len() >= n
                        && name.split_whitespace().eq(words[..n].iter().copied())
                })
                .max_by_key(|u| command_words(u).split_whitespace().count())
        }) else {
            return Flow::Continue;
        };
        let name = command_words(&usage);
        if !self.watches(hook, &name) {
            return Flow::Continue;
        }
        let r = (|| -> mlua::Result<()> {
            let lua = &self.lua;
            let player = player_table_of(lua, event.player)?;
            player.set("held", self.held_table(host)?)?;
            let args = lua.create_table()?;
            args.set("raw", line)?;
            if let Some(refusal) = command_args(lua, &usage, line, &args)? {
                host.feedback(refusal);
                return Ok(());
            }
            self.call_kind(
                hook,
                Some(&name),
                host,
                (player, args),
                &mut |host, action, args| {
                    match action {
                        "feedback" => {
                            let text: String = args.get(1)?;
                            host.feedback(crate::mod_api::Text::literal(text));
                        }
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
        Flow::Cancel
    }
    /// A mob is about to take damage: `mob_hurt(mob, damage_type, causing,
    /// direct, amount)`, `mob` just `{id, kind}` (the game is mid-hit: no
    /// host, so no view, no data, and the host actions are dropped),
    /// `causing` and `direct` entity ids (-1: none). Handlers may queue
    /// `horizon.set_amount(n)` and `horizon.cancel()` (the hit is refused).
    fn living_incoming_damage(&self, event: &mut LivingIncomingDamageEvent<'_>) -> Flow {
        let kind = qualify(event.kind);
        // A player is told as `player_hurt(player, ...)`, `player` their
        // `{id, kind}` (mid-hit: no host, so no name); a mob as `mob_hurt`.
        let (hook, by_kind) = if kind == "minecraft:player" {
            ("player_hurt", None)
        } else {
            ("mob_hurt", Some(kind.as_str()))
        };
        if by_kind.is_some_and(|k| !self.watches(hook, k))
            || (by_kind.is_none() && !self.has_handlers(hook))
        {
            return Flow::Continue;
        }
        let mut cancel = false;
        let mut amount = event.amount;
        let mob = match self.id_kind_table(event.entity.max(0) as u32, &kind) {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let r = self.call_bare(
            hook,
            by_kind,
            (
                mob,
                event.damage_type,
                event.causing,
                event.direct,
                event.amount,
            ),
            &mut |action, args| {
                match action {
                    "set_amount" => amount = args.get::<f32>(1)?,
                    "cancel" => cancel = true,
                    other => self.warn(format!("{hook}: {other} needs a host; dropped")),
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if amount.is_finite() {
            event.amount = amount.max(0.0);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// A player's blow killed a mob: `mob_killed(dimension, mob, killer,
    /// data)`, `mob` its `{id, kind}` (it is dead: no view), `killer` the
    /// player's table, `data` the mod's saved data on the mob (written back,
    /// though the mob is going).
    fn mob_killed(&self, host: &mut dyn Host, event: &mut MobKilledEvent<'_>) {
        let hook = "mob_killed";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return;
        }
        let id = event.entity;
        let r = (|| -> mlua::Result<()> {
            let killer = self.player_table(host)?;
            let dimension = host.dimension().to_owned();
            let mob = self.mob_table(host, &dimension, id, &kind)?;
            let data = self.mob_data(host, id)?;
            self.call_kind(
                hook,
                Some(&kind),
                host,
                (
                    dimension,
                    mob,
                    killer,
                    data.as_ref().map(|(_, t)| t.clone()),
                ),
                &mut |host, action, args| self.run_action(host, action, args),
            )?;
            self.store_mob_data(host, hook, id, data);
            Ok(())
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
    /// A player right-clicks a mob: `mob_interact(dimension, mob, player,
    /// held, data)`, before the mob and the item act; `mob` is its view (a
    /// mob the host has no view of, dead or gone, is not told).
    /// `horizon.cancel()` takes the click (the hand swings, nothing else
    /// happens); `data` is written back.
    fn interact_entity(&self, host: &mut dyn Host, event: &mut InteractEntityEvent<'_>) -> Flow {
        let hook = "mob_interact";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return Flow::Continue;
        }
        let Ok(id) = u32::try_from(event.entity) else {
            return Flow::Continue;
        };
        let mut cancel = false;
        let r = (|| -> mlua::Result<()> {
            let player = self.player_table(host)?;
            let held = match event.stack {
                Some(stack) => {
                    let t = self.lua.create_table()?;
                    t.set("item", &*stack.item)?;
                    t.set("count", stack.count)?;
                    t.set("plain", stack.extra.is_none())?;
                    Value::Table(t)
                }
                None => Value::Nil,
            };
            let dimension = host.dimension().to_owned();
            let Some(view) = host.mob_view(&dimension, id) else {
                return Ok(());
            };
            let mob = goals::view_table(&self.lua, &view)?;
            let data = self.mob_data(host, id)?;
            self.call_kind(
                hook,
                Some(&kind),
                host,
                (
                    dimension,
                    mob,
                    player,
                    held,
                    data.as_ref().map(|(_, t)| t.clone()),
                ),
                &mut |host, action, args| {
                    match action {
                        "cancel" => cancel = true,
                        other => self.run_action(host, other, args)?,
                    }
                    Ok(())
                },
            )?;
            self.store_mob_data(host, hook, id, data);
            Ok(())
        })();
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if cancel { Flow::Cancel } else { Flow::Continue }
    }
    /// The game keeps the list of ticking entities only while a mod listens:
    /// this one does while a `mob_tick` handler is registered.
    fn entity_tick_listened(&self, _host: &mut dyn Host, event: &mut EntityTickListenedEvent) {
        if self.has_handlers("mob_tick")
            || self
                .kinds
                .lock()
                .ok()
                .is_some_and(|k| k.get("mob_tick").is_some_and(|set| !set.is_empty()))
        {
            event.listened = true;
        }
    }
    /// A mob ticked: `mob_tick(dimension, mob, data)` for the
    /// handlers watching `kind` (`horizon.on("mob_tick", fn, kind)`; one for
    /// every kind hears every mob, so register by kind). `mob` is the mob's
    /// view without the fields that need the world (`nearby`,
    /// `target_position`), `data` the mod's saved data on it, written back.
    fn entity_tick_post(&self, host: &mut dyn Host, event: &mut EntityTickPostEvent<'_>) {
        let hook = "mob_tick";
        let kind = qualify(event.kind);
        if !self.watches(hook, &kind) {
            return;
        }
        let (dimension, id) = (event.dimension, event.entity);
        let Some(view) = host.mob_view(dimension, id) else {
            return;
        };
        let r = (|| -> mlua::Result<()> {
            let view = goals::view_table(&self.lua, &view)?;
            let data = self.mob_data(host, id)?;
            self.call_kind(
                hook,
                Some(&kind),
                host,
                (dimension, view, data.as_ref().map(|(_, t)| t.clone())),
                &mut |host, action, args| self.run_action(host, action, args),
            )?;
            self.store_mob_data(host, hook, id, data);
            Ok(())
        })();
        if let Err(e) = r {
            self.hook_error(hook, e);
        }
    }
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
                    "scripts/init.lua",
                    b"local n = require('util')\nassert(n == 42, 'util')\nassert(require('util') == 42)\nhorizon.on('server_tick', function() end)\nhorizon.log('ready ' .. horizon.mod_id)",
                ),
                ("scripts/util.lua", b"return 42"),
            ],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
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
                "scripts/init.lua",
                br#"
                horizon.goal("stalk", {
                  flags = {"move", "look"},
                  can_use = function(mob, state, out) return mob.nearest_player ~= nil end,
                  tick = function(mob, state, out) out.look_at(0, 0, 0) end,
                })
                horizon.on("mob_ai_register", function(mob)
                  if mob.kind == "minecraft:cow" then
                    horizon.clear_goals()
                    horizon.add_goal(2, "stalk")
                    horizon.add_target_goal(1, "missing")
                  end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
        let view = m.lua.create_table().unwrap();
        view.set("kind", "minecraft:cow").unwrap();
        let mut seen = Vec::new();
        let r = m.call_bare("mob_ai_register", None, view, &mut |action, args| {
            let name: Option<String> = args.get(2)?;
            if let Some(name) = &name {
                // The game resolves the goal here; a missing one is the handler's error.
                let def = m.goals.lock().unwrap().get(name).cloned();
                let def = def.ok_or_else(|| mlua::Error::runtime(format!("no goal {name}")))?;
                let goal = def.goal(m.lua.clone(), m.id(), m.budget.clone()).unwrap();
                assert!(!goal.state().started());
                assert_eq!(
                    def.flags,
                    crate::mod_api::GOAL_MOVE | crate::mod_api::GOAL_LOOK
                );
            }
            seen.push((action.to_owned(), args.get::<Option<i32>>(1)?, name));
            Ok(())
        });
        assert_eq!(
            seen,
            [
                ("clear_goals".to_owned(), None, None),
                ("add_goal".to_owned(), Some(2), Some("stalk".to_owned())),
            ]
        );
        let e = r.unwrap_err().to_string();
        assert!(e.contains("no goal missing"), "{e}");
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
    impl crate::mod_api::LevelHost for FakeLevel {
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
    impl crate::mod_api::LevelRandomHost for FakeLevel {
        fn attachments(
            &mut self,
            _: crate::mod_api::Holder,
        ) -> Result<&mut crate::mod_api::Attachments, crate::mod_api::AttachmentError> {
            Err(crate::mod_api::AttachmentError::MissingHolder)
        }
        fn random(&mut self) -> Option<&mut crate::mod_api::LegacyRandom> {
            None
        }
    }
    impl crate::mod_api::LevelTicksHost for FakeLevel {
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
        fn summon(&mut self, kind: &str, pos: [f64; 3], _: crate::mod_api::Compound) -> bool {
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
            _: &crate::mod_api::ParticleOptions,
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
                "scripts/init.lua",
                br#"
                LAMPS = 0
                horizon.on("random_tick", function() LAMPS = LAMPS + 1 end, "ruby:lamp")
                horizon.on("random_tick", function(dimension, block, x, y, z, props)
                  if block ~= "ruby:hive" then return end
                  if props.stage == "dormant" then
                    horizon.set_block_property("stage", "growing")
                    horizon.schedule_tick(40)
                    horizon.handled()
                  elseif props.stage == "mature" then
                    horizon.summon(dimension, "ruby:crawler", x + 0.5, y + 1, z + 0.5)
                  end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
        let mut level = FakeLevel {
            block: "ruby:hive",
            props: vec![("stage".into(), "dormant".into())],
            writes: Vec::new(),
            ticks: Vec::new(),
            summons: Vec::new(),
            data: HashMap::new(),
        };
        let mut event = RandomTickEvent {
            dimension: LevelDimension::Nether,
            pos: BlockPos::new(1, 64, -3),
            state: BlockStateId(7),
        };
        assert!(matches!(
            m.random_tick(&mut level, &mut event),
            Flow::Cancel
        ));
        assert_eq!(
            level.writes,
            [([1, 64, -3], "stage".to_owned(), "growing".to_owned(), 2)]
        );
        assert_eq!(level.ticks, [([1, 64, -3], "ruby:hive".to_owned(), 40)]);
        // The handler registered for the lamp never heard the hive's tick.
        assert_eq!(m.lua.globals().get::<i32>("LAMPS").unwrap(), 0);

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
            m.tick_block(
                &mut level,
                &mut TickBlockEvent {
                    dimension: LevelDimension::Overworld,
                    pos: BlockPos::new(0, 0, 0),
                    state: BlockStateId(7),
                }
            ),
            Flow::Continue
        ));
        assert_eq!(level.writes.len(), 1);
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
        ) -> crate::mod_api::AttachmentResult<&mut crate::mod_api::Attachments> {
            Err(crate::mod_api::AttachmentError::MissingHolder)
        }
        fn acting_player(&self) -> Option<crate::mod_api::NativeServerPlayer> {
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
        attachments: crate::mod_api::Attachments,
        health: Vec<(u32, f32)>,
        lines: Mutex<Vec<String>>,
        pushes: Vec<[f64; 3]>,
        carried: ItemStack,
        world: crate::mod_api::Attachments,
        player: crate::mod_api::Attachments,
    }
    impl Host for MobHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }
        fn attachments(
            &mut self,
            holder: Holder,
        ) -> crate::mod_api::AttachmentResult<&mut crate::mod_api::Attachments> {
            match holder {
                Holder::Entity(7) => Ok(&mut self.attachments),
                Holder::Level => Ok(&mut self.world),
                Holder::Player(0x123e4567_e89b_12d3_a456_426614174000) => Ok(&mut self.player),
                _ => Err(crate::mod_api::AttachmentError::MissingHolder),
            }
        }
        fn acting_player(&self) -> Option<crate::mod_api::NativeServerPlayer> {
            Some(crate::mod_api::NativeServerPlayer {
                uuid: 0x123e4567_e89b_12d3_a456_426614174000,
                name: "steve".into(),
                permission: 0,
                dimension: "minecraft:overworld".into(),
                position: [1.0, 64.0, 1.0],
                game_mode: 0,
                health: 20.0,
                yaw: 0.0,
                pitch: 0.0,
                entity_id: 0,
                address: String::new(),
            })
        }
        fn mob_view(&self, dimension: &str, id: u32) -> Option<MobView> {
            use horizon_core::geometry::Vec3;
            (dimension == "minecraft:overworld" && id == 7).then(|| {
                let kind = crate::mobs::MobType::cow_for_tests();
                MobView::of(
                    &crate::mobs::Mob::new(7, kind, Vec3::new(2.0, 64.0, 3.0), 0.0),
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
        fn level_players(&self) -> Vec<crate::mod_api::NativeServerPlayer> {
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
        fn tell(&mut self, _: u128, line: crate::mod_api::Text) -> bool {
            self.lines
                .lock()
                .unwrap()
                .push(format!("tell steve: {}", line.plain()));
            true
        }
        fn feedback(&mut self, line: crate::mod_api::Text) {
            self.lines
                .lock()
                .unwrap()
                .push(format!("feedback {}", line.plain()));
        }
        fn player_raycast(&self, uuid: u128, distance: f32) -> Option<crate::mod_api::NativeHit> {
            (uuid == 0x123e4567_e89b_12d3_a456_426614174000 && distance == 8.0).then(|| {
                crate::mod_api::NativeHit {
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
        fn broadcast(&self, text: crate::mod_api::Text) {
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
            edit: &crate::mod_api::NativeBlockEdit,
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
            heightmap: crate::mod_api::NativeHeightmap,
        ) -> Result<Vec<Option<i32>>, String> {
            Ok(columns
                .iter()
                .map(|c| {
                    (*c == (1, 3)).then(|| {
                        if heightmap == crate::mod_api::NativeHeightmap::OceanFloor {
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
            _: crate::mod_api::NativeFalling,
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

    #[test]
    fn the_item_hooks_reach_the_handlers_of_their_item() {
        let (dir, path) = hmod(
            "item-hooks",
            &[(
                "scripts/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                horizon.on("item_use", function(item, player)
                  log("use " .. item.id .. " x" .. item.count .. " by " .. player.name)
                  horizon.take_items(item.id, 1)
                  horizon.cancel()
                end, "ruby:wand")
                horizon.on("item_use_on", function(item, player, block, face)
                  log("use_on " .. block.id .. " at " .. block.x .. "," .. block.y .. "," .. block.z .. " face " .. face.y)
                end, "wand")
                horizon.on("item_hit", function(item, player, target)
                  log("hit " .. target.kind .. " " .. target.id .. " with " .. item.id .. " at " .. target.position.x)
                  horizon.cancel()
                end, "ruby:wand")
                horizon.on("item_finished", function(item, player)
                  log("finished " .. item.id)
                  horizon.set_result("minecraft:stick", 2)
                end, "ruby:wand")
                horizon.on("item_tick", function(item, player, slot, selected)
                  log("tick " .. item.id .. " slot " .. slot .. " " .. tostring(selected))
                  horizon.set_count(item.count - 1)
                end, "ruby:wand")
                horizon.on("item_use", function(item) error("not for apples") end, "minecraft:apple")
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
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
        // The wand's use is taken, and one is consumed; the registered name
        // without a namespace is minecraft's, so "wand" hears nothing.
        assert_eq!(
            m.use_item(&mut host, &mut UseItemEvent { stack: &wand }),
            Flow::Cancel
        );
        let mut stone = UseItemOnEvent {
            stack: &wand,
            pos: [1, 2, 3],
            face: [0, 1, 0],
        };
        assert_eq!(m.use_item_on(&mut host, &mut stone), Flow::Continue);
        assert_eq!(
            m.attack_entity(
                &mut host,
                &mut AttackEntityEvent {
                    entity: 7,
                    kind: "cow",
                    pos: [2.0, 64.0, 3.0]
                }
            ),
            Flow::Cancel
        );
        let mut result = Some(wand.clone());
        m.living_entity_use_item_finish(
            &mut host,
            &mut LivingEntityUseItemFinishEvent {
                stack: &wand,
                entity: None,
                hand: crate::mod_api::Hand::Main,
                duration: 0,
                result: &mut result,
            },
        );
        assert_eq!(
            result.as_ref().map(|s| (&*s.item, s.count)),
            Some(("minecraft:stick", 2))
        );
        m.inventory_tick(
            &mut host,
            &mut InventoryTickEvent {
                slot: 3,
                selected: true,
            },
        );
        m.inventory_tick(
            &mut host,
            &mut InventoryTickEvent {
                slot: 4,
                selected: false,
            },
        );
        assert_eq!(host.carried.count, 2);
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
                "hit minecraft:cow 7 with ruby:wand at 2",
                "finished ruby:wand",
                "tick ruby:wand slot 3 true",
            ]
        );
        assert_eq!(*host.lines.lock().unwrap(), ["took 1 ruby:wand"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_world_hooks_tell_of_deaths_players_blocks_and_pickups() {
        let (dir, path) = hmod(
            "world-hooks",
            &[(
                "scripts/init.lua",
                br#"
                LOG = {}
                local function log(s) LOG[#LOG + 1] = s end
                horizon.on("mob_died", function(dimension, mob, cause, data)
                  log("died " .. mob.kind .. " " .. mob.id .. " by " .. cause.damage_type .. " causing " .. cause.causing .. " credited " .. (cause.player and cause.player.name or "nobody"))
                end, "cow")
                horizon.on("player_joined", function(player) log("joined " .. player.name .. " hp " .. player.health) end)
                horizon.on("player_left", function(player) log("left " .. player.name) end)
                horizon.on("player_respawned", function(player) log("respawned " .. player.name) end)
                horizon.on("player_died", function(player, cause) log("player died " .. player.name .. " " .. tostring(cause)) end)
                horizon.on("block_broken", function(x, y, z, block, player)
                  log("broke " .. block .. " at " .. x .. "," .. y .. "," .. z .. " by " .. player.name)
                  horizon.cancel()
                end, "minecraft:stone")
                horizon.on("block_placed", function(x, y, z, block, player, item)
                  log("placed " .. block .. " with " .. item.id .. " x" .. item.count)
                end, "minecraft:stone")
                horizon.on("item_picked_up", function(player, item, count)
                  log("pickup " .. item .. " x" .. count .. " by " .. player.name)
                  horizon.cancel()
                end, "ruby:wand")
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
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
            &mut MobDiedEvent {
                dimension: "minecraft:overworld",
                entity: 7,
                kind: "cow",
                damage_type: "minecraft:on_fire",
                causing: 0,
                player: Some(steve),
            },
        );
        m.mob_died(
            &mut host,
            &mut MobDiedEvent {
                dimension: "minecraft:overworld",
                entity: 9,
                kind: "minecraft:zombie",
                damage_type: "minecraft:fall",
                causing: 0,
                player: None,
            },
        );
        m.player_logged_in(
            &mut host,
            &mut PlayerLoggedInEvent {
                player: steve,
                name: "steve",
            },
        );
        m.player_logged_out(
            &mut host,
            &mut PlayerLoggedOutEvent {
                player: 5,
                name: "alex",
            },
        );
        m.player_respawned(
            &mut host,
            &mut PlayerRespawnedEvent {
                player: steve,
                name: "steve",
            },
        );
        m.player_died(
            &mut host,
            &mut PlayerDiedEvent {
                player: steve,
                cause: Some("death.attack.lava"),
            },
        );
        // A death names no one and the host has no record of alex any more:
        // the name the logout told is kept.
        m.player_died(
            &mut host,
            &mut PlayerDiedEvent {
                player: 5,
                cause: None,
            },
        );
        assert_eq!(
            m.destroy_block(
                &mut host,
                &mut DestroyBlockEvent {
                    pos: [1, 2, 3],
                    state: BlockStateId(1)
                }
            ),
            Flow::Cancel
        );
        assert_eq!(
            m.destroy_block(
                &mut host,
                &mut DestroyBlockEvent {
                    pos: [0, 0, 0],
                    state: BlockStateId(0)
                }
            ),
            Flow::Continue
        );
        let wand = wand();
        m.place_block(
            &mut host,
            &mut PlaceBlockEvent {
                stack: &wand,
                pos: [1, 2, 3],
                state: BlockStateId(1),
            },
        );
        assert_eq!(
            m.item_picked_up(
                &mut host,
                &mut ItemPickedUpEvent {
                    player: steve,
                    item: "ruby:wand",
                    count: 2
                }
            ),
            Flow::Cancel
        );
        assert_eq!(
            m.item_picked_up(
                &mut host,
                &mut ItemPickedUpEvent {
                    player: steve,
                    item: "minecraft:apple",
                    count: 2
                }
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
                "died minecraft:cow 7 by minecraft:on_fire causing 0 credited steve",
                "joined steve hp 20",
                "left alex",
                "respawned steve",
                "player died steve death.attack.lava",
                "player died alex nil",
                "broke minecraft:stone at 1,2,3 by steve",
                "placed minecraft:stone with ruby:wand x3",
                "pickup ruby:wand x2 by steve",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn timers_run_from_the_server_tick_and_world_and_player_data_persist() {
        use horizon_world::nbt::Tag;
        let (dir, path) = hmod(
            "timers-data",
            &[(
                "scripts/init.lua",
                br#"
                N = 0
                horizon.after(3, function()
                  N = N + 10
                  local w = horizon.world_data()
                  w.runs = (w.runs or 0) + 1
                end)
                horizon.every(2, function() N = N + 1 end)
                horizon.every(1, function() error("a timer that fails") end)
                horizon.on("mob_interact", function(dimension, mob, player, held, data)
                  local w = horizon.world_data()
                  w.clicks = (w.clicks or 0) + 1
                  local p = horizon.player_data(player.uuid)
                  p.pets = (p.pets or 0) + 1
                  local again = horizon.player_data("steve")
                  again.pets = again.pets + 1
                  PD = horizon.player_data("alex")
                end, "cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
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
        let context = crate::mod_api::NativeLifecycleContext {
            authority: crate::mod_api::NativeAuthority::IntegratedServer,
            epoch: crate::mod_api::NativeWorldEpoch::new(1),
            tick: crate::mod_api::NativeTickId::new(1),
        };
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
                    &mut ServerTickEvent {
                        context: &context,
                        phase,
                    },
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
                &mut InteractEntityEvent {
                    entity: 7,
                    kind: "cow",
                    stack: None,
                },
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

    #[test]
    fn commands_feedback_titles_sounds_and_particles_reach_the_host() {
        let (dir, path) = hmod(
            "commands",
            &[(
                "scripts/init.lua",
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
        let m = LuaMod::load(&path, &[]).unwrap();
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
            &mut RegisterCommandsEvent {
                commands: &mut commands,
            },
        );
        assert_eq!(
            commands
                .iter()
                .map(|c| (c.usage.as_str(), c.level))
                .collect::<Vec<_>>(),
            [
                ("parasite count [kind:word] [n:integer]", 2),
                ("say <what:text>", 0),
                ("parasite hives", 2),
                ("probeadd <a:integer> <b:number> [label:word]", 0)
            ]
        );
        let steve = host.acting_player().unwrap();
        let run = |m: &LuaMod, host: &mut MobHost, line: &str| {
            m.perform_command(
                host,
                &mut PerformCommandEvent {
                    player: &steve,
                    command: line,
                },
            )
        };
        assert_eq!(run(&m, &mut host, "parasite count zombie 3"), Flow::Cancel);
        assert_eq!(run(&m, &mut host, "parasite count"), Flow::Cancel);
        assert_eq!(run(&m, &mut host, "say hello there  world"), Flow::Cancel);
        assert_eq!(run(&m, &mut host, "parasite hives"), Flow::Cancel);
        // Arguments of three kinds; a bad integer and a missing required
        // argument are refused with a message, and the handler is not called.
        assert_eq!(run(&m, &mut host, "probeadd 2 0.5 total"), Flow::Cancel);
        // The refusals are translated: the bundled English here.
        let (flows, missed) =
            crate::lang::scoped(crate::lang::Language::load(&[], "en_us"), || {
                (
                    run(&m, &mut host, "probeadd x 1"),
                    run(&m, &mut host, "probeadd 2"),
                )
            });
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
    fn the_world_player_and_config_helpers_reach_the_host() {
        let (dir, path) = hmod(
            "helpers",
            &[(
                "scripts/init.lua",
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
        let m = LuaMod::load_with(&path, &[], Some(&config)).unwrap();
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
                &mut PerformCommandEvent {
                    player: &steve,
                    command: line,
                },
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

    /// A host whose summon tells the mod of the spawn at once, as the game
    /// does: the hook runs nested inside the handler that summoned.
    struct ReentrantHost {
        lines: Mutex<Vec<String>>,
        m: Option<Arc<LuaMod>>,
        world: crate::mod_api::Attachments,
    }
    impl Host for ReentrantHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }
        fn attachments(
            &mut self,
            holder: Holder,
        ) -> crate::mod_api::AttachmentResult<&mut crate::mod_api::Attachments> {
            match holder {
                Holder::Level => Ok(&mut self.world),
                _ => Err(crate::mod_api::AttachmentError::MissingHolder),
            }
        }
        fn acting_player(&self) -> Option<crate::mod_api::NativeServerPlayer> {
            Some(crate::mod_api::NativeServerPlayer {
                uuid: 7,
                name: "steve".into(),
                permission: 0,
                dimension: "minecraft:overworld".into(),
                position: [1.0, 64.0, 1.0],
                game_mode: 0,
                health: 20.0,
                yaw: 0.0,
                pitch: 0.0,
                entity_id: 0,
                address: String::new(),
            })
        }
        fn held(&self) -> Option<(String, u32, bool)> {
            None
        }
        fn level_players(&self) -> Vec<crate::mod_api::NativeServerPlayer> {
            self.acting_player().into_iter().collect()
        }
        fn block(&self, pos: [i32; 3]) -> String {
            if pos == [1, 2, 3] {
                "minecraft:stone".into()
            } else {
                "minecraft:air".into()
            }
        }
        fn broadcast(&self, text: crate::mod_api::Text) {
            self.lines.lock().unwrap().push(text.plain());
        }
        fn summon(&mut self, _: &str, kind: &str, pos: [f64; 3], _: Compound) -> Option<u32> {
            let m = self.m.clone().unwrap();
            // As the game: the mob's goals are set up (no host), then the spawn is told.
            let mut mob = crate::mobs::Mob::new(
                42,
                crate::mobs::MobType::cow_for_tests(),
                horizon_core::geometry::Vec3::new(pos[0], pos[1], pos[2]),
                0.0,
            );
            m.mob_ai_register(&mut MobAiRegisterEvent { mob: &mut mob });
            m.mob_spawned(
                self,
                &mut MobSpawnedEvent {
                    dimension: "minecraft:overworld",
                    kind,
                    pos,
                    natural: false,
                },
            );
            Some(42)
        }
    }

    #[test]
    fn a_hook_fired_inside_a_handler_keeps_the_outer_calls_host_and_actions() {
        let (dir, path) = hmod(
            "reentrant",
            &[(
                "scripts/init.lua",
                br#"
                horizon.goal("idle", { can_use = function() return false end })
                horizon.on("mob_ai_register", function(mob) horizon.add_goal(1, "idle") end)
                horizon.on("mob_spawned", function(dimension, kind, x, y, z, natural)
                  horizon.broadcast("spawned " .. kind .. " at " .. x)
                  SPAWN_READ = horizon.block(1, 2, 3)
                end, "ruby:crawler")
                horizon.command("nest", 0, function(player, args)
                  horizon.broadcast("before")
                  local id = horizon.summon(player.dimension, "ruby:crawler", 5, 64, 5)
                  horizon.broadcast("after " .. tostring(id))
                  AFTER_READ = horizon.block(1, 2, 3)
                end)
                horizon.block_entity("ruby:hive", { ticks = true, data = { bred = 0 } })
                horizon.on("block_entity_tick", function(dimension, block, x, y, z, data)
                  local id = horizon.summon(dimension, "ruby:crawler", x + 0.5, y + 1, z + 0.5)
                  data.bred = data.bred + 1
                  local w = horizon.world_data()
                  TICK_READ = horizon.block(1, 2, 3) .. " " .. tostring(id) .. " " .. tostring(w ~= nil)
                end, "ruby:hive")
                "#,
            )],
        );
        let m = Arc::new(LuaMod::load(&path, &[]).unwrap());
        let mut host = ReentrantHost {
            lines: Mutex::new(Vec::new()),
            m: Some(m.clone()),
            world: Default::default(),
        };
        let steve = host.acting_player().unwrap();
        assert_eq!(
            m.perform_command(
                &mut host,
                &mut PerformCommandEvent {
                    player: &steve,
                    command: "nest",
                },
            ),
            Flow::Cancel
        );
        // The inner hook applied only what it queued, inside the summon; the
        // outer handler's lines came after it, and its reads still worked.
        assert_eq!(
            *host.lines.lock().unwrap(),
            ["spawned ruby:crawler at 5", "before", "after 42"]
        );
        let g = m.lua.globals();
        assert_eq!(g.get::<String>("SPAWN_READ").unwrap(), "minecraft:stone");
        assert_eq!(g.get::<String>("AFTER_READ").unwrap(), "minecraft:stone");
        // The same from a block entity's tick, with its data written back after.
        let mut entity = crate::mod_api::ModBlockEntity::default();
        entity.id = "ruby:hive".into();
        entity.nbt = m
            .block_entity(&mut host, &mut BlockEntityEvent { block: "ruby:hive" })
            .unwrap()
            .1;
        host.lines.lock().unwrap().clear();
        m.block_entity_tick(
            &mut host,
            &mut BlockEntityTickEvent {
                pos: [1, 2, 3],
                entity: &mut entity,
            },
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
    fn the_player_fall_loot_projectile_and_menu_hooks_reach_their_handlers() {
        let (dir, path) = hmod(
            "events",
            &[(
                "scripts/init.lua",
                br#"
                local function say(line) horizon.broadcast(line) end
                -- The hooks without a host (mid-hit) have no broadcast: they note.
                NOTES = {}
                local function note(line) NOTES[#NOTES + 1] = line end
                horizon.on("player_tick", function(player) say("tick " .. player.name .. " " .. tostring(player.held.item)) end)
                horizon.on("player_hurt", function(player, damage_type, causing, direct, amount)
                  note(("hurt %d %s %s %.1f"):format(player.id, player.kind, damage_type, amount)); horizon.set_amount(amount / 2)
                end)
                horizon.on("mob_fall", function(mob, distance, multiplier)
                  note(("fall %s %.1f x%.1f"):format(mob.kind, distance, multiplier)); horizon.set_distance(1.5)
                end, "minecraft:cow")
                horizon.on("player_fall", function(player, distance) note("player fall " .. distance); horizon.cancel() end)
                horizon.on("mob_heal", function(mob, amount) note("heal " .. mob.id .. " " .. amount); horizon.set_amount(amount + 1) end)
                horizon.on("mob_knockback", function(mob, damage_type, causing, direct, strength, x, z)
                  note(("knockback %s %s %.1f %.1f %.1f"):format(mob.kind, damage_type, strength, x, z)); horizon.set_strength(0.25)
                end, "minecraft:cow")
                horizon.on("player_chat", function(player, line)
                  note("chat " .. player.name .. " " .. line)
                  if line == "shh" then horizon.cancel() else horizon.reply("heard " .. line) end
                end)
                horizon.on("player_command", function(player, line)
                  say("command " .. player.name .. " " .. line)
                  if line == "secret" then horizon.reply("not here"); horizon.cancel() end
                end)
                horizon.on("item_crafted", function(player, recipe, item) say(("crafted %s %s x%d"):format(recipe, item.id, item.count)) end, "minecraft:oak_planks")
                horizon.on("item_dropped", function(player, item, count) say("dropped " .. item .. " x" .. count) end, "stick")
                horizon.on("loot_drops", function(source, drops)
                  say(("loot %s %s at %d,%d,%d by %s tool %s"):format(source.table, source.block.id, source.block.x, source.block.y, source.block.z, source.player and source.player.name or "nobody", source.tool and source.tool.id or "hand"))
                  drops[1].count = 2
                  drops[#drops + 1] = { id = "diamond", count = 1 }
                end, "minecraft:stone")
                horizon.on("loot_drops", function(source, drops)
                  say(("loot %s mob %d %s"):format(source.table, source.mob.id, source.mob.kind))
                  drops[1] = nil
                end, "minecraft:cow")
                horizon.on("projectile_impact", function(projectile, kind, owner, hit)
                  say(("impact %d %s by %d on %s"):format(projectile, kind, owner, hit.entity and ("entity " .. hit.entity .. " " .. hit.entity_kind) or ("block " .. hit.block.id .. " face " .. hit.face.y)))
                  if hit.entity then horizon.cancel() end
                end, "minecraft:snowball")
                horizon.on("projectile_launched", function(projectile, kind, player) say(("launched %d %s by %s"):format(projectile, kind, player and player.name or "nobody")) end, "minecraft:snowball")
                horizon.on("explosion", function(dimension, center, blocks)
                  say(("explosion at %.0f,%.0f,%.0f breaking %d"):format(center.x, center.y, center.z, #blocks))
                  table.remove(blocks, 1)
                end)
                horizon.on("player_changed_dimension", function(player, from, to) say("moved " .. player.name .. " " .. from .. " > " .. to) end)
                horizon.on("player_bed_enter", function(player, x, y, z) say(("bed %s %d,%d,%d"):format(player.name, x, y, z)); horizon.cancel() end)
                horizon.on("player_exp_change", function(player, amount) say("exp " .. amount); horizon.set_amount(amount * 2) end)
                horizon.on("player_container_open", function(player, menu, pos) say("menu " .. menu .. " " .. (pos and pos.x or "nowhere")) end)
                horizon.on("mob_equipped", function(mob, slot, from, to) say(("equipped %d %s %s > %s"):format(mob.id, slot, from and from.id or "none", to and to.id or "none")) end, "minecraft:cow")
                horizon.on("entity_joined", function(dimension, id, kind, x, y, z, loaded)
                  say(("joined %d %s at %.0f loaded=%s"):format(id, kind, x, tostring(loaded))); if id == 99 then horizon.cancel() end
                end, "minecraft:arrow")
                horizon.on("item_hurt_enemy", function(item, player, target) say(("landed %s on %d %s by %s"):format(item.id, target.id, target.kind, player.name)) end, "ruby:wand")
                horizon.on("item_use_start", function(item, player, hand, duration)
                  say(("use start %s %s %d"):format(item.id, hand, duration)); horizon.set_duration(10)
                end, "ruby:wand")
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
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
        m.player_tick(&mut host, &mut PlayerTickEvent {});
        let mut hit = LivingIncomingDamageEvent {
            entity: 5,
            kind: "minecraft:player",
            damage_type: "minecraft:fall",
            causing: -1,
            direct: -1,
            amount: 6.0,
        };
        assert_eq!(m.living_incoming_damage(&mut hit), Flow::Continue);
        assert_eq!(hit.amount, 3.0);
        let mut fall = LivingFallEvent {
            entity: 7,
            kind: "minecraft:cow",
            distance: 4.0,
            multiplier: 1.0,
        };
        assert_eq!(m.living_fall(&mut fall), Flow::Continue);
        assert_eq!(fall.distance, 1.5);
        let mut fall = LivingFallEvent {
            entity: 5,
            kind: "minecraft:player",
            distance: 9.0,
            multiplier: 1.0,
        };
        assert_eq!(m.living_fall(&mut fall), Flow::Cancel);
        let mut heal = LivingHealEvent {
            entity: 7,
            amount: 2.0,
        };
        assert_eq!(m.living_heal(&mut heal), Flow::Continue);
        assert_eq!(heal.amount, 3.0);
        let mut knock = LivingKnockbackEvent {
            entity: 7,
            kind: "minecraft:cow",
            damage_type: "minecraft:player_attack",
            causing: 5,
            direct: 5,
            strength: 0.4,
            x: 1.0,
            z: 0.0,
        };
        assert_eq!(m.living_knockback(&mut knock), Flow::Continue);
        assert_eq!(knock.strength, 0.25);
        let mut replies = Vec::new();
        assert_eq!(
            m.player_chat(&mut PlayerChatEvent {
                player: steve.uuid,
                name: "steve",
                message: "hello all",
                replies: &mut replies,
            }),
            Flow::Continue
        );
        assert_eq!(replies, [crate::mod_api::Text::literal("heard hello all")]);
        let mut replies = Vec::new();
        assert_eq!(
            m.player_chat(&mut PlayerChatEvent {
                player: steve.uuid,
                name: "steve",
                message: "shh",
                replies: &mut replies,
            }),
            Flow::Cancel
        );
        let mut replies = Vec::new();
        let mut cmd = PlayerCommandEvent {
            player: steve.uuid,
            name: "steve",
            command: "secret",
            replies: &mut replies,
        };
        assert_eq!(m.player_command(&mut host, &mut cmd), Flow::Cancel);
        assert_eq!(
            replies.iter().map(|t| t.plain()).collect::<Vec<_>>(),
            ["not here"]
        );
        let mut cmd = PlayerCommandEvent {
            player: steve.uuid,
            name: "steve",
            command: "time set day",
            replies: &mut replies,
        };
        assert_eq!(m.player_command(&mut host, &mut cmd), Flow::Continue);
        let planks = ItemStack {
            item: "minecraft:oak_planks".into(),
            count: 4,
            damage: 0,
            extra: None,
        };
        m.item_crafted(
            &mut host,
            &mut ItemCraftedEvent {
                player: steve.uuid,
                recipe: "minecraft:oak_planks",
                stack: &planks,
            },
        );
        m.item_dropped(
            &mut host,
            &mut ItemDroppedEvent {
                player: steve.uuid,
                item: "minecraft:stick",
                count: 2,
            },
        );
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
            &mut LootDropsEvent {
                context: &crate::mod_api::NativeLootDrops {
                    table: "minecraft:blocks/stone",
                    dimension: "minecraft:overworld",
                    origin: [1.5, 2.5, 3.5],
                    block: Some(([1, 2, 3], BlockStateId(1))),
                    entity: None,
                    player: Some(steve.uuid),
                    tool: Some(&pick),
                    explosion: None,
                    used: false,
                },
                drops: &mut drops,
            },
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
            &mut LootDropsEvent {
                context: &crate::mod_api::NativeLootDrops {
                    table: "minecraft:entities/cow",
                    dimension: "minecraft:overworld",
                    origin: [2.0, 64.0, 3.0],
                    block: None,
                    entity: Some((7, "minecraft:cow")),
                    player: None,
                    tool: None,
                    explosion: None,
                    used: false,
                },
                drops: &mut drops,
            },
        );
        assert!(drops.is_empty());
        let mut hears = HearsProjectileImpactsEvent { hears: false };
        m.hears_projectile_impacts(&host, &mut hears);
        assert!(hears.hears);
        let mut listened = LivingEquipmentListenedEvent { listened: false };
        m.living_equipment_listened(&mut host, &mut listened);
        assert!(listened.listened);
        let mut impact = ProjectileImpactEvent {
            dimension: "minecraft:overworld",
            projectile: 30,
            kind: "minecraft:snowball",
            owner: 5,
            entity: Some(7),
            entity_kind: "minecraft:cow",
            block: [0, 0, 0],
            face: [0, 0, 0],
            at: [2.0, 64.5, 3.0],
        };
        assert_eq!(m.projectile_impact(&mut host, &mut impact), Flow::Cancel);
        impact.entity = None;
        impact.block = [1, 2, 3];
        impact.face = [0, 1, 0];
        assert_eq!(m.projectile_impact(&mut host, &mut impact), Flow::Continue);
        m.projectile_launch(
            &mut host,
            &mut ProjectileLaunchEvent {
                player: Some(steve.uuid),
                name: "steve",
                entity: 30,
                kind: "minecraft:snowball",
            },
        );
        let mut blocks = vec![[1, 2, 3], [1, 3, 3]];
        m.explosion_detonate(
            &mut host,
            &mut ExplosionDetonateEvent {
                dimension: "minecraft:overworld",
                center: [1.0, 2.0, 3.0],
                blocks: &mut blocks,
            },
        );
        assert_eq!(blocks, [[1, 3, 3]]);
        m.player_changed_dimension(
            &mut host,
            &mut PlayerChangedDimensionEvent {
                player: steve.uuid,
                name: "steve",
                from: "minecraft:overworld",
                to: "minecraft:the_nether",
            },
        );
        assert_eq!(
            m.player_bed_enter(
                &mut host,
                &mut PlayerBedEnterEvent {
                    player: steve.uuid,
                    name: "steve",
                    pos: [4, 5, 6],
                }
            ),
            Flow::Cancel
        );
        let mut exp = PlayerExpChangeEvent {
            player: steve.uuid,
            name: "steve",
            amount: 3,
        };
        m.player_exp_change(&mut host, &mut exp);
        assert_eq!(exp.amount, 6);
        m.player_container_open(
            &mut host,
            &mut PlayerContainerOpenEvent {
                player: steve.uuid,
                name: "steve",
                dimension: "minecraft:overworld",
                menu: "minecraft:generic_9x3",
                pos: Some([7, 8, 9]),
            },
        );
        let sword = ItemStack {
            item: "minecraft:iron_sword".into(),
            count: 1,
            damage: 0,
            extra: None,
        };
        m.living_equipment_change(
            &mut host,
            &mut LivingEquipmentChangeEvent {
                entity: 7,
                kind: "minecraft:cow",
                slot: "mainhand",
                from: None,
                to: Some(&sword),
            },
        );
        assert_eq!(
            m.entity_join_level(
                &mut host,
                &mut EntityJoinLevelEvent {
                    dimension: "minecraft:overworld",
                    entity: 99,
                    kind: "minecraft:arrow",
                    pos: [1.0, 64.0, 1.0],
                    loaded_from_disk: false,
                }
            ),
            Flow::Cancel
        );
        m.hurt_enemy(
            &mut host,
            &mut HurtEnemyEvent {
                stack: &wand(),
                entity: 7,
                kind: "minecraft:cow",
            },
        );
        let mut start = LivingEntityUseItemStartEvent {
            stack: &wand(),
            entity: None,
            hand: horizon_client::first_person::Hand::Off,
            duration: 32,
        };
        assert_eq!(
            m.living_entity_use_item_start(&mut host, &mut start),
            Flow::Continue
        );
        assert_eq!(start.duration, 10);
        assert_eq!(
            *host.lines.lock().unwrap(),
            [
                "tick steve ruby:wand",
                "command steve secret",
                "command steve time set day",
                "crafted minecraft:oak_planks minecraft:oak_planks x4",
                "dropped minecraft:stick x2",
                "loot minecraft:blocks/stone minecraft:stone at 1,2,3 by steve tool ruby:wand",
                "loot minecraft:entities/cow mob 7 minecraft:cow",
                "impact 30 minecraft:snowball by 5 on entity 7 minecraft:cow",
                "impact 30 minecraft:snowball by 5 on block minecraft:stone face 1",
                "launched 30 minecraft:snowball by steve",
                "explosion at 1,2,3 breaking 2",
                "moved steve minecraft:overworld > minecraft:the_nether",
                "bed steve 4,5,6",
                "exp 3",
                "menu minecraft:generic_9x3 7",
                "equipped 7 mainhand none > minecraft:iron_sword",
                "joined 99 minecraft:arrow at 1 loaded=false",
                "landed ruby:wand on 7 minecraft:cow by steve",
                "use start ruby:wand off 32",
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
                "chat steve hello all",
                "chat steve shh",
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_mob_hooks_reach_the_handlers_of_their_kind_with_the_mobs_saved_data() {
        use horizon_world::nbt::Tag;
        let (dir, path) = hmod(
            "mob-hooks",
            &[(
                "scripts/init.lua",
                br#"
                HURT = {}
                horizon.on("mob_hurt", function(mob, damage_type, causing, direct, amount)
                  HURT[#HURT + 1] = mob.kind .. ":" .. damage_type
                  HURT_READ = pcall(horizon.block, 1, 2, 3)
                  if damage_type == "minecraft:on_fire" then horizon.cancel() end
                  horizon.set_amount(amount * 2)
                end, "cow")
                horizon.on("mob_tick", function(dimension, mob, data)
                  data.ticks = (data.ticks or 0) + 1
                  if data.ticks == 2 then horizon.set_mob_health(dimension, mob.id, mob.max_health - 1) end
                  if mob.position.x ~= 2 then error("view") end
                end, "minecraft:cow")
                horizon.on("mob_killed", function(dimension, mob, killer, data)
                  horizon.broadcast(killer.name .. " killed " .. mob.kind .. " " .. mob.id .. " after " .. data.ticks .. " ticks")
                end, "minecraft:cow")
                horizon.on("mob_interact", function(dimension, mob, player, held, data)
                  data.pets = (data.pets or 0) + 1
                  if mob.health ~= 10 then error("view") end
                  data.block = horizon.block(1, 2, 3) .. "," .. horizon.block(0, 0, 0)
                  data.facing = horizon.block_property(1, 2, 3, "facing") or "none"
                  data.players = #horizon.players() .. ":" .. horizon.players()[1].name .. ":" .. (horizon.player("steve") and "found" or "lost") .. ":" .. (horizon.player(player.uuid) and "found" or "lost") .. ":" .. (horizon.player("alex") and "found" or "lost")
                  local t = horizon.time()
                  data.time = t.day .. ":" .. t.time_of_day .. ":" .. t.rain .. ":" .. t.dimension
                  data.mobs = #horizon.mobs() .. ":" .. horizon.mob(7).max_health .. ":" .. tostring(horizon.mob(8))
                  local hit = horizon.raycast(player.uuid, 8)
                  data.hit = hit.id .. ":" .. hit.block.y .. ":" .. hit.x .. ":" .. tostring(horizon.raycast("alex", 8))
                  KEPT = horizon.block
                  if held and held.item == "minecraft:wheat" then horizon.cancel() end
                end, "minecraft:cow")
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
        let key = |k: &str| horizon_world::nbt::JavaString::from(k);
        // A hurt zombie is nobody's business; the cow's hit is doubled, fire refused.
        let mut hit = LivingIncomingDamageEvent {
            entity: 9,
            kind: "zombie",
            damage_type: "minecraft:mob_attack",
            causing: 1,
            direct: 1,
            amount: 3.0,
        };
        assert_eq!(m.living_incoming_damage(&mut hit), Flow::Continue);
        assert_eq!(hit.amount, 3.0);
        hit.kind = "cow";
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
        // The mod listens to entity ticks: it has a mob_tick handler.
        // No host mid-hit: the reads refuse there.
        assert!(!m.lua.globals().get::<bool>("HURT_READ").unwrap());
        let mut listened = EntityTickListenedEvent { listened: false };
        m.entity_tick_listened(&mut NoHost, &mut listened);
        assert!(listened.listened);
        // Ticks count in the mob's saved data; the second one sets its health.
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
                &mut EntityTickPostEvent {
                    dimension: "minecraft:overworld",
                    entity: 7,
                    kind: "cow",
                },
            );
            m.entity_tick_post(
                &mut host,
                &mut EntityTickPostEvent {
                    dimension: "minecraft:overworld",
                    entity: 8,
                    kind: "cow",
                },
            );
        }
        assert_eq!(host.health, [(7, 9.0)]);
        let saved = m
            .attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Entity(7))
            .unwrap()
            .unwrap();
        assert_eq!(saved.get(&key("ticks")), Some(&Tag::Int(3)));
        // A click keeps count too, and wheat is taken.
        assert_eq!(
            m.interact_entity(
                &mut host,
                &mut InteractEntityEvent {
                    entity: 7,
                    kind: "cow",
                    stack: None
                }
            ),
            Flow::Continue
        );
        let wheat = crate::mod_api::ItemStack {
            item: "minecraft:wheat".into(),
            count: 1,
            damage: 0,
            extra: None,
        };
        assert_eq!(
            m.interact_entity(
                &mut host,
                &mut InteractEntityEvent {
                    entity: 7,
                    kind: "cow",
                    stack: Some(&wheat)
                }
            ),
            Flow::Cancel
        );
        // A click on a mob the host has no view of (dead, gone) is not told.
        assert_eq!(
            m.interact_entity(
                &mut host,
                &mut InteractEntityEvent {
                    entity: 8,
                    kind: "cow",
                    stack: Some(&wheat)
                }
            ),
            Flow::Continue
        );
        let saved = m
            .attachment
            .get_existing(&mut host as &mut dyn Host, Holder::Entity(7))
            .unwrap()
            .unwrap();
        assert_eq!(saved.get(&key("pets")), Some(&Tag::Int(2)));
        assert_eq!(saved.get(&key("ticks")), Some(&Tag::Int(3)));
        // The reads answered from the host during the click; a read kept
        // past the call is refused.
        let text = |k: &str| match saved.get(&key(k)) {
            Some(Tag::String(s)) => s.to_string().unwrap(),
            other => panic!("{k}: {other:?}"),
        };
        assert_eq!(text("block"), "minecraft:stone,minecraft:air");
        assert_eq!(text("facing"), "north");
        assert_eq!(text("players"), "1:steve:found:found:lost");
        assert_eq!(text("time"), "1:6000:0.5:minecraft:overworld");
        assert_eq!(text("mobs"), "1:10:nil");
        assert_eq!(text("hit"), "minecraft:dirt:5:4.5:nil");
        let kept: Function = m.lua.globals().get("KEPT").unwrap();
        let e = kept.call::<String>((1, 2, 3)).unwrap_err().to_string();
        assert!(e.contains("inside a hook with a host"), "{e}");
        // The kill hears the player and the data.
        m.mob_killed(
            &mut host,
            &mut MobKilledEvent {
                player: 1,
                entity: 7,
                kind: "minecraft:cow",
            },
        );
        assert_eq!(
            *host.lines.lock().unwrap(),
            ["steve killed minecraft:cow 7 after 3 ticks"]
        );
        // Without a host that knows the mob there is no data, and nothing breaks.
        m.mob_killed(
            &mut NoHost,
            &mut MobKilledEvent {
                player: 1,
                entity: 7,
                kind: "minecraft:cow",
            },
        );
        assert_eq!(host.lines.lock().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reload_reads_the_scripts_again_and_forgets_the_old_handlers() {
        let (dir, path) = hmod(
            "reload",
            &[
                ("scripts/init.lua", b"VERSION = require('v')\nhorizon.on('server_tick', function() end)\nhorizon.on('server_tick', function() end)"),
                ("scripts/v.lua", b"return 1"),
            ],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
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
                "scripts/init.lua",
                br#"
                VERSION = require('v')
                TICKS = 0
                horizon.on('server_tick', function() TICKS = TICKS + 1 end)
                horizon.every(1, function() TIMED = (TIMED or 0) + 1 end)
                horizon.command("ruby", 0, function() end)
                horizon.goal("sit", { can_use = function() return false end })
                "#,
            ),
            ("scripts/v.lua", b"return 2"),
        ]);
        // The first data load is the one the scripts were loaded for; the next reloads.
        let mut files = Vec::new();
        m.data_pack_files(&NoHost, &mut DataPackFilesEvent { files: &mut files });
        assert_eq!(handlers(&m), 3);
        m.data_pack_files(&NoHost, &mut DataPackFilesEvent { files: &mut files });
        assert_eq!(handlers(&m), 2);
        assert_eq!(m.lua.globals().get::<i64>("VERSION").unwrap(), 2);
        let tick = |m: &LuaMod| {
            m.call_bare("server_tick", None, ("end", true), &mut |_, _| Ok(()))
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
        rewrite(&[("scripts/init.lua", b"error('x')")]);
        let e = m.reload().unwrap_err();
        assert!(e.contains("x"), "{e}");
        rewrite(&[(
            "scripts/init.lua",
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
            "scripts/init.lua",
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
                "scripts/init.lua",
                b"N = 0\n\
                  horizon.on('server_tick', function() while true do end end)\n\
                  horizon.on('world_lifecycle', function() N = N + 1 end)",
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
        let started = std::time::Instant::now();
        let e = m
            .call_bare("server_tick", None, ("end", true), &mut |_, _| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(e.contains("ran over"), "{e}");
        assert!(
            started.elapsed() < HANDLER_BUDGET * 4,
            "{:?}",
            started.elapsed()
        );
        // The state is fine afterwards, and the next chunk has a budget of its own.
        m.call_bare("world_lifecycle", None, ("start", "x"), &mut |_, _| Ok(()))
            .unwrap();
        assert_eq!(m.lua.globals().get::<i64>("N").unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_pcall_around_a_slow_loop_does_not_defeat_the_budget() {
        let (dir, path) = hmod(
            "budget-pcall",
            &[(
                "scripts/init.lua",
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
        let m = LuaMod::load(&path, &[]).unwrap();
        for hook in ["mob_spawned", "mob_killed", "server_tick"] {
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
        // The host slot and the queue are back as they were, and the next
        // call has a budget of its own.
        let slot: Table = m.lua.named_registry_value(HOST_SLOT).unwrap();
        assert!(slot.get::<Value>(1).unwrap().is_nil());
        assert_eq!(slot.get::<Option<i64>>(3).unwrap().unwrap_or(0), 0);
        let queue: Table = m.lua.named_registry_value(QUEUE).unwrap();
        assert_eq!(queue.raw_len(), 0);
        m.call_bare("world_lifecycle", None, ("start", "x"), &mut |_, _| Ok(()))
            .unwrap();
        assert_eq!(m.lua.globals().get::<i64>("N").unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_action_outside_a_hooks_handlers_is_an_error_and_leaves_nothing_queued() {
        use crate::mod_api::NativeBehaviour;
        // From a goal function: no runner takes the action, so it is refused
        // where it is called, not left in the queue for the state's life.
        let (dir, path) = hmod(
            "goal-action",
            &[(
                "scripts/init.lua",
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
        let m = LuaMod::load(&path, &[]).unwrap();
        let def = m.goals.lock().unwrap().get("summoner").cloned().unwrap();
        let behaviour = def
            .behaviour(m.lua.clone(), m.id(), m.budget.clone())
            .unwrap();
        let view = MobView::of(
            &crate::mobs::Mob::new(
                7,
                crate::mobs::MobType::cow_for_tests(),
                horizon_core::geometry::Vec3::new(0.0, 64.0, 0.0),
                0.0,
            ),
            None,
        );
        let mut state = goals::LuaGoalState::default();
        let mut out = crate::mod_api::Actions::default();
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
            &[("scripts/init.lua", b"horizon.broadcast('hello')")],
        );
        let e = LuaMod::load(&path, &[]).unwrap_err();
        assert!(e.contains("horizon.broadcast"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_later_handlers_error_drops_the_actions_and_the_data_edits_alike() {
        let (dir, path) = hmod(
            "half",
            &[(
                "scripts/init.lua",
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
        let m = LuaMod::load(&path, &[]).unwrap();
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
                ("scripts/init.lua", b"LIB_RAN = true"),
                (
                    "scripts/modkit.lua",
                    b"local m = { VERSION = '1', entity = require('modkit.entity') }\nreturn m",
                ),
                (
                    "scripts/modkit/entity/init.lua",
                    b"return { kind = 'entity' }",
                ),
            ],
        );
        let (dir, path) = hmod(
            "user",
            &[
                ("scripts/init.lua", b"local modkit = require('@modkit')\nV = modkit.VERSION .. modkit.entity.kind\nMINE = require('mine')\nSAME = require('modkit/entity') == modkit.entity and require('modkit.entity') == modkit.entity"),
                ("scripts/mine.lua", b"return 'own'"),
                // The mod's own module wins a shared name.
                ("scripts/modkit/entity.lua", b"return { kind = 'own entity' }"),
            ],
        );
        let m = LuaMod::load(&path, &[("lib".into(), lib.clone())]).unwrap();
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
        let e = LuaMod::load(&path, &[]).unwrap_err();
        assert!(
            e.contains("scripts/modkit.lua") && e.contains("depends on"),
            "{e}"
        );
        // A bad name is refused before any lookup.
        let (bad_dir, bad) = hmod("badname", &[("scripts/init.lua", b"require('../x')")]);
        assert!(LuaMod::load(&bad, &[]).unwrap_err().contains("for folders"));
        for d in [lib_dir, dir, bad_dir] {
            std::fs::remove_dir_all(&d).unwrap();
        }
    }

    #[test]
    fn a_block_entity_declared_in_lua_is_made_ticked_and_keeps_its_data() {
        use horizon_world::nbt::Tag;
        let (dir, path) = hmod(
            "block-entity",
            &[(
                "scripts/init.lua",
                br#"
                horizon.block_entity("ruby:hive", { ticks = true, data = { charge = 0, name = "x" } })
                horizon.block_entity("ruby:lamp")
                horizon.on("block_entity_tick", function(dimension, block, x, y, z, data)
                  data.charge = data.charge + 1
                  data.seen = dimension .. ":" .. x
                  data.name = nil
                end)
                horizon.on("random_tick", function(dimension, block, x, y, z, props, data)
                  if data then data.charge = data.charge * 10; data.under = horizon.block(x, y - 1, z) end
                  if pcall(horizon.players) then error("a level hook reads blocks only") end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
        let mut host = NoHost;
        let key = |k: &str| horizon_world::nbt::JavaString::from(k);
        // A declared block gets a block entity with its starting data; others none.
        let (id, data) = m
            .block_entity(&mut host, &mut BlockEntityEvent { block: "ruby:hive" })
            .unwrap();
        assert_eq!(id, "ruby:hive");
        assert_eq!(data.get(&key("charge")), Some(&Tag::Int(0)));
        assert_eq!(data.get(&key("name")), Some(&Tag::String("x".into())));
        let (_, lamp) = m
            .block_entity(&mut host, &mut BlockEntityEvent { block: "ruby:lamp" })
            .unwrap();
        assert!(lamp.is_empty());
        assert!(
            m.block_entity(
                &mut host,
                &mut BlockEntityEvent {
                    block: "ruby:other"
                }
            )
            .is_none()
        );
        // Only the hive ticks.
        let mut ticks = BlockEntityTicksEvent {
            block: "ruby:hive",
            ticks: false,
        };
        m.block_entity_ticks(&host, &mut ticks);
        assert!(ticks.ticks);
        let mut ticks = BlockEntityTicksEvent {
            block: "ruby:lamp",
            ticks: false,
        };
        m.block_entity_ticks(&host, &mut ticks);
        assert!(!ticks.ticks);
        // The tick handler changes the data, which is written back: the int
        // stays an int, the removed key is gone, the new one is there.
        let mut entity = crate::mod_api::ModBlockEntity::default();
        entity.id = "ruby:hive".into();
        entity.nbt = data;
        m.block_entity_tick(
            &mut host,
            &mut BlockEntityTickEvent {
                pos: [1, 2, 3],
                entity: &mut entity,
            },
        );
        m.block_entity_tick(
            &mut host,
            &mut BlockEntityTickEvent {
                pos: [1, 2, 3],
                entity: &mut entity,
            },
        );
        assert_eq!(entity.nbt.get(&key("charge")), Some(&Tag::Int(2)));
        assert_eq!(
            entity.nbt.get(&key("seen")),
            Some(&Tag::String("minecraft:overworld:1".into()))
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
            &mut RandomTickEvent {
                dimension: LevelDimension::Overworld,
                pos: BlockPos::new(1, 64, -3),
                state: BlockStateId(7),
            },
        );
        assert_eq!(
            level.data[&[1, 64, -3]].get(&key("charge")),
            Some(&Tag::Int(20))
        );
        assert_eq!(
            level.data[&[1, 64, -3]].get(&key("under")),
            Some(&Tag::String("ruby:hive".into()))
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
                "scripts/init.lua",
                b"horizon.on(\"player_joined\", function() end)\nhorizon.on(\"player_jioned\", function() end)\n",
            )],
        );
        let e = LuaMod::load(&path, &[]).unwrap_err();
        assert!(
            e.contains("scripts/init.lua\"]:2:")
                && e.contains("no such hook")
                && e.contains("player_joined"),
            "{e}"
        );
        let e = check_mod(&path, &[], None).unwrap_err();
        assert!(e.contains(":2:") && e.contains("player_jioned"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
        let (dir, path) = hmod(
            "fine",
            &[
                (
                    "scripts/init.lua",
                    b"local m = require(\"helper\")\nhorizon.on(\"player_joined\", m.hello)\n",
                ),
                ("scripts/helper.lua", b"return { hello = function() end }\n"),
            ],
        );
        let lines = check_mod(&path, &[], None).unwrap();
        assert_eq!(
            lines,
            [
                "ruby: scripts loaded",
                "  scripts/helper.lua",
                "  scripts/init.lua"
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
                Package::write_files(&manifest, None, &[("scripts/init.lua", init)]).unwrap(),
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
        let m = LuaMod::load(&user, &[("lib".into(), new)]).unwrap();
        assert_eq!(m.lua.globals().get::<String>("MINE").unwrap(), "0.3.0");
        assert_eq!(m.lua.globals().get::<String>("LIB").unwrap(), "2.1.0");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_developer_tools_see_a_mods_status_and_reload_it() {
        let (dir, path) = hmod(
            "status",
            &[(
                "scripts/init.lua",
                b"horizon.goal('stalk', { can_use = function() return false end })\n\
                  horizon.on('server_tick', function() end)\n\
                  horizon.on('mob_spawned', function() error('oops') end)",
            )],
        );
        let m = LuaMod::load(&path, &[]).unwrap();
        m.publish(None);
        let of = |m: &LuaMod| status().into_iter().find(|s| s.id == m.id()).unwrap();
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
        m.call(None, "mob_spawned", ());
        assert!(of(&m).error.unwrap().contains("oops"));
        // The console asks for a reload, by id or for everyone: each is served once.
        assert!(!m.reload_asked());
        request_reload(Some(m.id()));
        assert!(m.reload_asked() && !m.reload_asked());
        request_reload(None);
        assert!(m.reload_asked() && !m.reload_asked());
        m.reload_now("the test");
        let s = of(&m);
        assert_eq!(s.reloads, 1);
        assert!(s.error.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_module_or_a_failing_script_is_an_error_with_the_file_named() {
        let (dir, path) = hmod("missing", &[("scripts/init.lua", b"require('nothing')")]);
        let e = LuaMod::load(&path, &[]).unwrap_err();
        assert!(e.contains("scripts/nothing.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();

        let (dir, path) = hmod("broken", &[("scripts/init.lua", b"error('boom')")]);
        let e = LuaMod::load(&path, &[]).unwrap_err();
        assert!(e.contains("boom") && e.contains("scripts/init.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
