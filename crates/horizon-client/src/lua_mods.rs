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
use crate::mod_api::{
    BlockPos, BlockStateId, DataPackFilesEvent, Flow, Hooks, Host, LevelDimension, LevelWriteHost,
    MobAiRegisterEvent, MobSpawnedEvent, MobView, Mods, RandomTickEvent, RightClickBlockEvent,
    ServerTickEvent, ServerTickPhase, TickBlockEvent, WorldLifecycleEvent,
};
use horizon_mods::{Capability, Content, Package};
use mlua::{Function, Lua, MultiValue, Table, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub mod goals;

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
    /// Makes the state stop a chunk that runs past its deadline.
    fn install(&self, lua: &Lua) {
        let budget = self.clone();
        lua.set_interrupt(move |_| {
            let deadline = budget.0.load(Ordering::Relaxed);
            if deadline != 0 && epoch().elapsed().as_nanos() as u64 > deadline {
                // Cleared, so the error can unwind without being raised again.
                budget.0.store(0, Ordering::Relaxed);
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
        self.0.store(deadline.max(1), Ordering::Relaxed);
        let r = f();
        self.0.store(0, Ordering::Relaxed);
        r
    }
}

/// The file a scripted mod starts from, inside its folder.
pub const ENTRY: &str = horizon_mods::content::SCRIPT_ENTRY;

/// The content mod's manifest, which names the mod.
const MANIFEST: &str = "horizon-mod.toml";
/// The Lua global the game provides.
const GLOBAL: &str = "horizon";
/// The registry key of the handlers table: `name -> { fn, fn, ... }`.
const HANDLERS: &str = "horizon_handlers";
/// The registry key of the queue of actions a handler asked for.
const QUEUE: &str = "horizon_queue";
/// The registry key of the modules `require` ran: `name -> value`.
const LOADED: &str = "horizon_loaded";

/// One scripted mod: its folder and its Lua state.
pub struct LuaMod {
    id: String,
    /// The `.hmod` or folder the scripts came from.
    source: PathBuf,
    lua: Lua,
    /// The scripts, by path, as last read from `source`.
    scripts: Scripts,
    /// The goals the scripts defined (`horizon.goal`), by name.
    goals: Goals,
    /// Data loads seen (`data_pack_files`): the scripts reload from the second on.
    loads: AtomicU32,
    /// The running chunk's deadline, for the state's interrupt.
    budget: Budget,
}

/// A mod's goal definitions, shared with the `horizon.goal` function.
type Goals = Arc<Mutex<HashMap<String, goals::GoalDef>>>;

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
pub fn discover(args: impl Iterator<Item = String>) -> Mods {
    let mut dev: Vec<PathBuf> = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let folder = match arg.strip_prefix("--dev-mod=") {
            Some(path) => path.to_owned(),
            None if arg == "--dev-mod" => match args.next() {
                Some(path) => path,
                None => break,
            },
            None => continue,
        };
        let dir = PathBuf::from(folder);
        dev.push(std::path::absolute(&dir).unwrap_or(dir));
    }
    let here = std::env::current_dir().unwrap_or_default();
    let enabled = crate::mod_list::ModList::load(&crate::logging::shared_dir(&here)).enabled;
    let mut paths = horizon_mods::find(&here.join(crate::mod_list::DIR));
    paths.retain(|p| !dev.contains(p));
    paths.extend(dev.iter().cloned());
    let mut mods: Vec<Box<dyn Hooks>> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for path in &paths {
        let Ok(package) = Package::read(path) else {
            continue;
        };
        let m = &package.manifest;
        if !m.capabilities.contains(&Capability::Scripts) {
            if dev.contains(path) && path.join(ENTRY).is_file() {
                log::warn!(
                    "Horizon: {} has {ENTRY} but no capability scripts in {MANIFEST}: not run",
                    path.display()
                );
            }
            continue;
        }
        if !(enabled.contains(&m.id) || dev.contains(path)) || ids.contains(&m.id) {
            continue;
        }
        match LuaMod::load(path) {
            Ok(lua) => {
                log::info!("Horizon: Lua mod {} loaded from {}", lua.id, path.display());
                ids.push(lua.id.clone());
                mods.push(Box::new(lua));
            }
            Err(e) => log::error!("Horizon: Lua mod in {} not loaded: {e}", path.display()),
        }
    }
    mods.into()
}

impl LuaMod {
    /// Load the mod at `path` (a `.hmod` or a mod folder): its id from
    /// `horizon-mod.toml`, its scripts read once, `scripts/init.lua` run.
    pub fn load(path: &Path) -> Result<Self, String> {
        let (id, scripts) = Self::read_scripts(path)?;
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY).map_err(|e| e.to_string())?;
        let budget = Budget::default();
        budget.install(&lua);
        let m = LuaMod {
            id,
            source: path.to_owned(),
            lua,
            scripts: Arc::new(Mutex::new(scripts)),
            goals: Goals::default(),
            loads: AtomicU32::new(0),
            budget,
        };
        m.install_api().map_err(|e| e.to_string())?;
        m.run(ENTRY).map_err(|e| e.to_string())?;
        Ok(m)
    }

    /// The mod's id and scripts, read from its `.hmod` or folder.
    fn read_scripts(path: &Path) -> Result<(String, HashMap<String, String>), String> {
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
        Ok((manifest.id, scripts))
    }

    /// Reads the scripts again and runs `scripts/init.lua` afresh: the
    /// handlers, the goals and the modules `require` ran are forgotten first;
    /// the Lua state and its globals stay. On an error the mod keeps running
    /// as it was, with its old handlers.
    pub fn reload(&self) -> Result<(), String> {
        let (_, scripts) = Self::read_scripts(&self.source)?;
        let lua = &self.lua;
        let reset = || -> mlua::Result<()> {
            lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
            lua.set_named_registry_value(QUEUE, lua.create_table()?)?;
            lua.set_named_registry_value(LOADED, lua.create_table()?)?;
            Ok(())
        };
        reset().map_err(|e| e.to_string())?;
        if let Ok(mut goals) = self.goals.lock() {
            goals.clear();
        }
        if let Ok(mut current) = self.scripts.lock() {
            *current = scripts;
        }
        self.run(ENTRY).map_err(|e| e.to_string())
    }

    /// The mod's id, as its manifest says.
    pub fn id(&self) -> &str {
        &self.id
    }

    fn install_api(&self) -> mlua::Result<()> {
        let lua = &self.lua;
        let scripts = self.scripts.clone();
        lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
        lua.set_named_registry_value(QUEUE, lua.create_table()?)?;
        lua.set_named_registry_value(LOADED, lua.create_table()?)?;
        let api = lua.create_table()?;
        let id = self.id.clone();
        api.set("mod_id", id.as_str())?;
        api.set(
            "log",
            lua.create_function(move |_, msg: String| {
                log::info!("[lua:{id}] {msg}");
                Ok(())
            })?,
        )?;
        api.set(
            "on",
            lua.create_function(|lua, (name, f): (String, Function)| {
                let handlers: Table = lua.named_registry_value(HANDLERS)?;
                let list: Table = match handlers.get::<Option<Table>>(name.as_str())? {
                    Some(list) => list,
                    None => {
                        let list = lua.create_table()?;
                        handlers.set(name.as_str(), &list)?;
                        list
                    }
                };
                list.push(f)?;
                Ok(())
            })?,
        )?;
        // Goals: `horizon.goal(name, def)` defines one for `horizon.add_goal`.
        let goals = self.goals.clone();
        api.set(
            "goal",
            lua.create_function(move |lua, (name, def): (String, Table)| {
                let mut goals = goals.lock().map_err(|_| mlua::Error::runtime("goals"))?;
                // One name for the game's life: a redefinition keeps the first string.
                let leaked: &'static str = match goals.get(name.as_str()) {
                    Some(g) => g.name,
                    None => Box::leak(name.clone().into_boxed_str()),
                };
                let def = goals::GoalDef::parse(lua, leaked, def)?;
                goals.insert(name, def);
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
            "add_goal",
            "add_target_goal",
            "clear_goals",
            "clear_target_goals",
            "set_block_property",
            "schedule_tick",
            "handled",
            "cancel",
        ] {
            api.set(
                action,
                lua.create_function(move |lua, args: MultiValue| {
                    let queue: Table = lua.named_registry_value(QUEUE)?;
                    let entry = lua.create_table()?;
                    entry.set("action", action)?;
                    entry.set("args", lua.create_sequence_from(args)?)?;
                    queue.push(entry)?;
                    Ok(())
                })?,
            )?;
        }
        // `require("name")`: `scripts/name.lua` of this mod, run once.
        api.set(
            "require",
            lua.create_function(move |lua, name: String| {
                if name.contains("..") || name.contains('/') || name.contains('\\') {
                    return Err(mlua::Error::runtime(format!(
                        "require({name:?}): a plain module name"
                    )));
                }
                let loaded: Table = lua.named_registry_value(LOADED)?;
                if let Some(v) = loaded.get::<Option<Value>>(name.as_str())? {
                    return Ok(v);
                }
                let key = format!("scripts/{name}.lua");
                let code = scripts
                    .lock()
                    .ok()
                    .and_then(|s| s.get(&key).cloned())
                    .ok_or_else(|| {
                        mlua::Error::runtime(format!("require({name:?}): no {key} in the mod"))
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
        self.budget
            .run(LOAD_BUDGET, || self.lua.load(code.as_str()).set_name(name).exec())
    }

    /// Whether any handler is registered for `hook`: the hooks that fire
    /// often (random ticks) build their arguments only then.
    fn has_handlers(&self, hook: &str) -> bool {
        let handlers: mlua::Result<Table> = self.lua.named_registry_value(HANDLERS);
        handlers.is_ok_and(|h| h.get::<Option<Table>>(hook).ok().flatten().is_some())
    }

    /// A block hook on a level (`random_tick`, `tick_block`): handlers get
    /// `(dimension, block, x, y, z, props)` and may queue
    /// `set_block_property(key, value[, flags])`, `schedule_tick(delay)` and
    /// `handled()`, which skips the block's own tick.
    fn level_hook(
        &self,
        hook: &str,
        level: &mut dyn LevelWriteHost,
        dimension: LevelDimension,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Flow {
        if !self.has_handlers(hook) {
            return Flow::Continue;
        }
        let Some(block) = level.state_block(state) else {
            return Flow::Continue;
        };
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
        let mut handled = false;
        let r = self.call_with(
            hook,
            (
                dimension_id(dimension),
                block.as_str(),
                pos.x,
                pos.y,
                pos.z,
                props,
            ),
            &mut |action, args| {
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
                    "handled" => handled = true,
                    other => log::warn!("[lua:{}] {hook}: {other} needs a host; dropped", self.id),
                }
                Ok(())
            },
        );
        if let Err(e) = r {
            return self.hook_error(hook, e);
        }
        if handled {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn hook_error(&self, hook: &str, e: mlua::Error) -> Flow {
        log::error!("[lua:{}] {hook}: {e}", self.id);
        Flow::Continue
    }

    /// Call the handlers of `hook` with `args`, then run what they queued
    /// with the hook's host (none: the actions are dropped with a warning).
    fn call(&self, host: Option<&mut dyn Host>, hook: &str, args: impl mlua::IntoLuaMulti) {
        let r = match host {
            Some(host) => self.call_with(hook, args, &mut |action, a| {
                self.run_action(host, action, a)
            }),
            None => self.call_with(hook, args, &mut |action, _| {
                log::warn!("[lua:{}] {hook}: {action} needs a host; dropped", self.id);
                Ok(())
            }),
        };
        if let Err(e) = r {
            log::error!("[lua:{}] {hook}: {e}", self.id);
        }
    }

    /// Call the handlers of `hook` with `args`, then hand each queued action
    /// (its name and arguments) to `apply`, in order.
    fn call_with(
        &self,
        hook: &str,
        args: impl mlua::IntoLuaMulti,
        apply: &mut dyn FnMut(&str, &Table) -> mlua::Result<()>,
    ) -> mlua::Result<()> {
        let lua = &self.lua;
        let handlers: Table = lua.named_registry_value(HANDLERS)?;
        let Some(list) = handlers.get::<Option<Table>>(hook)? else {
            return Ok(());
        };
        let args = args.into_lua_multi(lua)?;
        for f in list.sequence_values::<Function>() {
            let f = f?;
            self.budget
                .run(HANDLER_BUDGET, || f.call::<()>(args.clone()))?;
        }
        let queue: Table = lua.named_registry_value(QUEUE)?;
        if queue.raw_len() == 0 {
            return Ok(());
        }
        let actions: Vec<Table> = queue.sequence_values().collect::<mlua::Result<_>>()?;
        queue.clear()?;
        for entry in actions {
            let action: String = entry.get("action")?;
            let args: Table = entry.get("args")?;
            apply(&action, &args)?;
        }
        Ok(())
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
            other => log::warn!("[lua:{}] unknown action {other}", self.id),
        }
        Ok(())
    }
}

/// `Level.setBlock`'s flag a crop grows with: clients see it, no neighbour update.
const UPDATE_CLIENTS: i32 = 2;

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
            return;
        }
        match self.reload() {
            Ok(()) => log::info!("Horizon: Lua mod {} reloaded", self.id),
            Err(e) => log::error!(
                "Horizon: Lua mod {} not reloaded, still running as before: {e}",
                self.id
            ),
        }
    }
    /// A block that ticks randomly does: `random_tick(dimension, block, x, y, z, props)`.
    fn random_tick(&self, level: &mut dyn LevelWriteHost, event: &mut RandomTickEvent) -> Flow {
        self.level_hook(
            "random_tick",
            level,
            event.dimension,
            event.pos,
            event.state,
        )
    }
    /// A scheduled block tick is due: `tick_block(dimension, block, x, y, z, props)`.
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
            let player = match host.acting_player() {
                Some(p) => {
                    let t = lua.create_table()?;
                    t.set("name", p.name.as_str())?;
                    t.set("dimension", p.dimension.as_str())?;
                    t.set("position", goals::vec3(lua, p.position)?)?;
                    t.set("game_mode", p.game_mode)?;
                    Value::Table(t)
                }
                None => Value::Nil,
            };
            let held = match host.held() {
                Some((item, count, plain)) => {
                    let t = lua.create_table()?;
                    t.set("item", item)?;
                    t.set("count", count)?;
                    t.set("plain", plain)?;
                    Value::Table(t)
                }
                None => Value::Nil,
            };
            Ok((face, player, held))
        })();
        let (face, player, held) = match tables {
            Ok(t) => t,
            Err(e) => return self.hook_error(hook, e),
        };
        let mut cancel = false;
        let r = self.call_with(
            hook,
            (x, y, z, block.as_str(), face, player, held),
            &mut |action, args| {
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
        if cancel { Flow::Cancel } else { Flow::Continue }
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
            Err(e) => return log::error!("[lua:{}] mob_ai_register: {e}", self.id),
        };
        let r = self.call_with("mob_ai_register", view, &mut |action, args| {
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
                other => log::warn!(
                    "[lua:{}] mob_ai_register: {other} needs a host; dropped",
                    self.id
                ),
            }
            Ok(())
        });
        if let Err(e) = r {
            log::error!("[lua:{}] mob_ai_register: {e}", self.id);
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
        self.call(
            Some(host),
            "mob_spawned",
            (dimension, kind, pos[0], pos[1], pos[2], natural),
        );
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
        let m = LuaMod::load(&path).unwrap();
        assert_eq!(m.id(), "ruby");
        let handlers: Table = m.lua.named_registry_value(HANDLERS).unwrap();
        let ticks: Table = handlers.get("server_tick").unwrap();
        assert_eq!(ticks.len().unwrap(), 1);
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
        let m = LuaMod::load(&path).unwrap();
        let view = m.lua.create_table().unwrap();
        view.set("kind", "minecraft:cow").unwrap();
        let mut seen = Vec::new();
        let r = m.call_with("mob_ai_register", view, &mut |action, args| {
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
                horizon.on("random_tick", function(dimension, block, x, y, z, props)
                  if block ~= "ruby:hive" then return end
                  if props.stage == "dormant" then
                    horizon.set_block_property("stage", "growing")
                    horizon.schedule_tick(40)
                    horizon.handled()
                  end
                end)
                "#,
            )],
        );
        let m = LuaMod::load(&path).unwrap();
        let mut level = FakeLevel {
            block: "ruby:hive",
            props: vec![("stage".into(), "dormant".into())],
            writes: Vec::new(),
            ticks: Vec::new(),
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

        // A mature hive, and another block: nothing happens and the block keeps its tick.
        level.props = vec![("stage".into(), "mature".into())];
        assert!(matches!(
            m.random_tick(&mut level, &mut event),
            Flow::Continue
        ));
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
    impl Host for NoHost {}

    #[test]
    fn reload_reads_the_scripts_again_and_forgets_the_old_handlers() {
        let (dir, path) = hmod(
            "reload",
            &[
                ("scripts/init.lua", b"VERSION = require('v')\nhorizon.on('server_tick', function() end)\nhorizon.on('server_tick', function() end)"),
                ("scripts/v.lua", b"return 1"),
            ],
        );
        let m = LuaMod::load(&path).unwrap();
        let handlers = |m: &LuaMod| -> i64 {
            let h: Table = m.lua.named_registry_value(HANDLERS).unwrap();
            h.get::<Option<Table>>("server_tick")
                .unwrap()
                .map_or(0, |l| l.len().unwrap())
        };
        assert_eq!(handlers(&m), 2);
        // The mod's file changes under it: one handler, a new module value.
        std::fs::write(
            &path,
            Package::write_files(
                MANIFEST_TEXT,
                None,
                &[
                    (
                        "scripts/init.lua",
                        b"VERSION = require('v')\nhorizon.on('server_tick', function() end)",
                    ),
                    ("scripts/v.lua", b"return 2"),
                ],
            )
            .unwrap(),
        )
        .unwrap();
        // The first data load is the one the scripts were loaded for; the next reloads.
        let mut files = Vec::new();
        m.data_pack_files(&NoHost, &mut DataPackFilesEvent { files: &mut files });
        assert_eq!(handlers(&m), 2);
        m.data_pack_files(&NoHost, &mut DataPackFilesEvent { files: &mut files });
        assert_eq!(handlers(&m), 1);
        assert_eq!(m.lua.globals().get::<i64>("VERSION").unwrap(), 2);
        // A broken edit leaves the running handlers alone.
        std::fs::write(
            &path,
            Package::write_files(MANIFEST_TEXT, None, &[("scripts/init.lua", b"error('x')")])
                .unwrap(),
        )
        .unwrap();
        assert!(m.reload().is_err());
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
        let m = LuaMod::load(&path).unwrap();
        let started = std::time::Instant::now();
        let e = m
            .call_with("server_tick", ("end", true), &mut |_, _| Ok(()))
            .unwrap_err()
            .to_string();
        assert!(e.contains("ran over"), "{e}");
        assert!(started.elapsed() < HANDLER_BUDGET * 4, "{:?}", started.elapsed());
        // The state is fine afterwards, and the next chunk has a budget of its own.
        m.call_with("world_lifecycle", ("start", "x"), &mut |_, _| Ok(()))
            .unwrap();
        assert_eq!(m.lua.globals().get::<i64>("N").unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_module_or_a_failing_script_is_an_error_with_the_file_named() {
        let (dir, path) = hmod("missing", &[("scripts/init.lua", b"require('nothing')")]);
        let e = LuaMod::load(&path).unwrap_err();
        assert!(e.contains("scripts/nothing.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();

        let (dir, path) = hmod("broken", &[("scripts/init.lua", b"error('boom')")]);
        let e = LuaMod::load(&path).unwrap_err();
        assert!(e.contains("boom") && e.contains("scripts/init.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
