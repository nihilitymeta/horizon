//! Declaring mode: a mod's `main.luau` runs once, at start-up, to learn which hooks it subscribes to.
//!
//! The game reads the hooks a mod listens to before any world or host exists, so the chunk runs
//! now, with one function in reach, `horizon.on(hook, handler)`. The handlers it registers stay in
//! the state for later. The chunk is untrusted code run before the game starts, so it has a time
//! budget, cannot touch anything but `horizon.on`, and can register at most `MAX_HANDLERS`.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use event_horizon_lua_convert::scalar::{ConvertError, FromLuaValue};
use mlua::{Error, Function, Lua, Table, Value};

use crate::budget;
use crate::data_maps;
use crate::setting_api;
use crate::settings::Settings;

/// The most `horizon.on` calls one mod may make.
pub const MAX_HANDLERS: usize = 256;

/// The most `horizon.block` calls one mod may make.
pub const MAX_BLOCKS: usize = 256;

/// The most `horizon.item` calls one mod may make.
pub const MAX_ITEMS: usize = 256;

/// The most `horizon.block_part` calls one mod may make, and the most `horizon.item_part` calls.
pub const MAX_PARTS: usize = 128;

/// The most `horizon.block_entity_type` calls one mod may make.
pub const MAX_BLOCK_ENTITY_TYPES: usize = 64;

/// The most `horizon.world_fluid` calls one mod may make.
pub const MAX_WORLD_FLUIDS: usize = 8;

/// The most `horizon.entity_part` calls one mod may make: one per entity type.
pub const MAX_ENTITY_PARTS: usize = 16;

/// The most `horizon.menu_part` calls one mod may make: one per menu type.
pub const MAX_MENU_PARTS: usize = 16;

/// The most blocks one `horizon.block_entity_type` may name.
pub const MAX_BLOCKS_PER_TYPE: usize = 32;

/// The most `horizon.client_command` calls one mod may make.
pub const MAX_CLIENT_COMMANDS: usize = 64;

/// The longest usage `horizon.client_command` accepts, in characters.
pub const MAX_USAGE: usize = 256;

/// The longest first word (the command's name) of a client command's usage, in characters.
pub const MAX_COMMAND_NAME: usize = 32;

/// The most `horizon.command` calls one mod may make.
pub const MAX_COMMANDS: usize = 64;

/// The most `horizon.config` calls one mod may make.
pub const MAX_CONFIGS: usize = 8;

/// The longest config file path `horizon.config` accepts, in characters.
pub const MAX_CONFIG_FILE: usize = 128;

/// The most goals one entity part may declare in its `goals` table.
pub const MAX_GOALS: usize = 8;

/// The longest goal name, in bytes.
pub const MAX_GOAL_NAME: usize = 64;

/// The most behaviors one entity part may declare in its `behaviors` table (#74, B2).
pub const MAX_BEHAVIORS: usize = 8;

/// The longest behavior name, in bytes.
pub const MAX_BEHAVIOR_NAME: usize = 64;

/// The highest priority of a behavior, as `horizon.entity_part` takes it (a lower number runs first).
pub const MAX_BEHAVIOR_PRIORITY: i64 = 20;

/// The duration window a behavior gets when it declares none: 40 to 80 brain ticks. An assumption, not a
/// vanilla value; documented in the d.luau.
pub const DEFAULT_BEHAVIOR_DURATION: (i32, i32) = (40, 80);

/// The highest priority of a goal, as `horizon.entity_part` takes it (as vanilla's `addGoal`: a lower
/// number runs first).
pub const MAX_GOAL_PRIORITY: i64 = 20;

/// The most bytes of a `horizon.config` default. Smaller than the game's config file cap on purpose.
pub const MAX_CONFIG_DEFAULT: usize = 65536;

/// The time a mod's top level may run while it loads.
pub const DECLARE_BUDGET: Duration = Duration::from_secs(2);

/// Registry key of the table that holds each hook's handlers: hook name -> sequence of functions.
const HANDLERS_KEY: &str = "eh_handlers";

/// Registry key of the environment the mod's chunk ran in.
const ENV_KEY: &str = "eh_env";

const LATE_ON: &str = "horizon.on can only be used while the mod loads";

const LATE_DECLARE: &str = "declarations are only allowed while the mod loads";

/// The `horizon.on` options table's `priority` range.
const PRIORITY_MIN: i64 = i32::MIN as i64;
const PRIORITY_MAX: i64 = i32::MAX as i64;

/// The hooks a mod subscribed to: each name once, in the order of its first `horizon.on`.
/// `options` has one entry per hook, in the same order, with the options that hook was declared
/// with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declaration {
    pub hooks: Vec<String>,
    pub options: Vec<(String, ListenerOptions)>,
    /// The blocks `horizon.block` declared, in declaration order.
    pub blocks: Vec<BlockRecord>,
    /// The items `horizon.item` declared, in declaration order.
    pub items: Vec<ItemRecord>,
    /// The block parts `horizon.block_part` declared, in declaration order.
    pub parts: Vec<PartRecord>,
    /// The item parts `horizon.item_part` declared, in declaration order.
    pub item_parts: Vec<PartRecord>,
    /// The block entity types `horizon.block_entity_type` declared, in declaration order.
    pub block_entity_types: Vec<BlockEntityTypeRecord>,
    /// The block entity parts `horizon.block_entity_part` declared, in declaration order.
    pub block_entity_parts: Vec<PartRecord>,
    /// The world fluids `horizon.world_fluid` declared, in declaration order.
    pub fluids: Vec<FluidRecord>,
    /// The fluid parts `horizon.fluid_part` declared, in declaration order.
    pub fluid_parts: Vec<PartRecord>,
    /// The entity parts `horizon.entity_part` declared, in declaration order.
    pub entity_parts: Vec<PartRecord>,
    /// The menu parts `horizon.menu_part` declared, in declaration order.
    pub menu_parts: Vec<PartRecord>,
    /// The mob goals the `goals` tables of the entity parts declared, in declaration order.
    pub goals: Vec<GoalRecord>,
    /// The brain behaviors the `behaviors` tables of the entity parts declared, in declaration order.
    pub behaviors: Vec<BehaviorRecord>,
    /// The client commands `horizon.client_command` declared, in declaration order.
    pub client_commands: Vec<ClientCommandRecord>,
    /// The configs `horizon.config` declared, in declaration order. Each names the mod as its owner.
    pub configs: Vec<mod_api::Config>,
    /// The server commands `horizon.command` declared, in declaration order.
    pub commands: Vec<CommandRecord>,
    /// The data map types `horizon.data_map_type` declared, in declaration order.
    pub data_map_types: Vec<mod_api::DataMapType>,
    /// The data pack files `horizon.data_pack_file` declared, as (key, JSON text), in order.
    pub data_pack_files: Vec<(String, String)>,
    /// Whether the chunk called `horizon.on_setting_changed`. Its handler is in the registry, not a hook.
    pub setting_changed: bool,
}

/// The kind of a part a mod declares: `horizon.block_part` (a BlockPart), `horizon.item_part` (an
/// ItemPart), `horizon.block_entity_part` (a BlockEntityPart), `horizon.fluid_part` (a FluidPart) or
/// `horizon.entity_part` (an EntityPart) or `horizon.menu_part` (a MenuPart). Each kind has its own
/// handler table, its own id check and its own cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartKind {
    Block,
    Item,
    BlockEntity,
    Fluid,
    Entity,
    Menu,
}

impl PartKind {
    /// The declaring function's name, as a mod calls it.
    pub fn keyword(self) -> &'static str {
        match self {
            PartKind::Block => "block_part",
            PartKind::Item => "item_part",
            PartKind::BlockEntity => "block_entity_part",
            PartKind::Fluid => "fluid_part",
            PartKind::Entity => "entity_part",
            PartKind::Menu => "menu_part",
        }
    }

    /// What the id names, in the errors (`block`, `item`, `block entity`).
    fn noun(self) -> &'static str {
        match self {
            PartKind::Block => "block",
            PartKind::Item => "item",
            PartKind::BlockEntity => "block entity",
            PartKind::Fluid => "fluid",
            PartKind::Entity => "entity",
            PartKind::Menu => "menu",
        }
    }

    /// The handler names of this kind the host binds, with the reason each is not bound.
    fn handlers(self) -> &'static [(&'static str, Option<&'static str>)] {
        match self {
            PartKind::Block => crate::generated::parts::BLOCK_PARTS,
            PartKind::Item => crate::generated::item_parts::ITEM_PARTS,
            PartKind::BlockEntity => crate::generated::block_entity_parts::BLOCK_ENTITY_PARTS,
            PartKind::Fluid => crate::generated::fluid_parts::FLUID_PARTS,
            PartKind::Entity => crate::generated::entity_parts::ENTITY_PARTS,
            PartKind::Menu => crate::generated::menu_parts::MENU_PARTS,
        }
    }
}

/// A block entity type a mod declared with `horizon.block_entity_type{id, blocks}`: its id in the
/// mod's own namespace, and the blocks that hold it (each declared by this mod with `horizon.block`),
/// in the order given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEntityTypeRecord {
    pub id: String,
    pub blocks: Vec<String>,
}

/// A block part a mod declared with `horizon.block_part(id, {hook = fn, ...})`: the block it attaches
/// to (any namespace) and the BlockPart handler names it has, sorted. Each handler is stored under
/// [`part_handler_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartRecord {
    pub id: String,
    pub hooks: Vec<String>,
}

/// A mob goal a mod declared in the `goals` table of an entity part: the entity type it is for, its
/// name in the mod's namespace, its priority and flags, and the handlers it has (sorted). Each handler
/// is stored under [`goal_handler_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalRecord {
    pub entity: String,
    pub name: String,
    pub priority: i32,
    pub flags: mod_api::GoalFlags,
    pub hooks: Vec<String>,
}

/// The key a goal's handler is stored under in the mod's handler table. A space cannot be in a goal
/// name or a hook name, so the key is never a hook's own name or a part's.
pub fn goal_handler_key(name: &str, hook: &str) -> String {
    format!("goal {name} {hook}")
}

/// A brain behavior a mod declared in the `behaviors` table of an entity part (#74, B2): the entity type
/// it is for, its name in the mod's namespace, its activity, priority and duration window (as
/// `BehaviorRegistrar::add` takes them), and the handlers it has (sorted). Each handler is stored under
/// [`behavior_handler_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BehaviorRecord {
    pub entity: String,
    pub name: String,
    pub activity: mod_api::Activity,
    pub priority: i32,
    pub min_duration: i32,
    pub max_duration: i32,
    pub hooks: Vec<String>,
}

/// The key a behavior's handler is stored under in the mod's handler table. Its prefix differs from a
/// goal's, so a behavior and a goal of one name never share a key or a quarantine entry.
pub fn behavior_handler_key(name: &str, hook: &str) -> String {
    format!("behavior {name} {hook}")
}

/// A world fluid a mod declared with `horizon.world_fluid{...}`: its id in the mod's own namespace and
/// the simulation settings the game declares it with (checked by `WorldFluid::check`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluidRecord {
    pub id: String,
    pub fluid: mod_api::WorldFluid,
}

/// The key a part's handler is stored under in the mod's handler table. A space cannot be in a hook
/// name or an id, so the key is never a hook's own name.
pub fn part_handler_key(kind: PartKind, id: &str, hook: &str) -> String {
    format!("{} {id} {hook}", kind.keyword())
}

/// A block a mod declared with `horizon.block{id, template}`. The id is in the mod's own
/// namespace; the template is a vanilla block (`minecraft:<path>`) it copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRecord {
    pub id: String,
    pub template: String,
}

/// An item a mod declared with `horizon.item{id, template}`, as `BlockRecord` does for blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRecord {
    pub id: String,
    pub template: String,
}

/// A client command a mod declared with `horizon.client_command{usage}`: the usage as the mod wrote
/// it. The host parses it (the game leaves out a usage it cannot parse).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientCommandRecord {
    pub usage: String,
}

impl ClientCommandRecord {
    /// The game's declaration of this command: the usage, at operator level 0 (a client command has
    /// no operator level).
    pub fn to_command(&self) -> mod_api::Command {
        mod_api::Command::new(&self.usage)
    }
}

/// A server command a mod declared with `horizon.command{usage, level}`: the usage as the mod wrote
/// it, and the operator level a player needs (0 to 4, 0 when the mod gives none). The host parses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRecord {
    pub usage: String,
    pub level: u8,
}

impl CommandRecord {
    /// The command's name: the first word of its usage (see [`command_name`]).
    pub fn name(&self) -> &str {
        command_name(&self.usage)
    }

    /// The game's declaration of this command: the usage at the declared level.
    pub fn to_command(&self) -> mod_api::Command {
        mod_api::Command::op(&self.usage, self.level)
    }
}

impl BlockRecord {
    /// The game's declaration of this block: the template and the id, every other field default.
    pub fn to_block(&self) -> mod_api::Block {
        mod_api::Block::new(self.id.clone(), self.template.clone())
    }
}

impl ItemRecord {
    /// The game's declaration of this item: the template and the id, every other field default.
    pub fn to_item(&self) -> mod_api::Item {
        mod_api::Item::new(self.id.clone(), self.template.clone())
    }
}

/// How a mod's listener for one hook is ordered and whether it still runs once the event is
/// cancelled. `priority` runs higher first; a mod that did not set `receive_cancelled` is skipped
/// while the event is cancelled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListenerOptions {
    pub priority: i32,
    pub receive_cancelled: bool,
}

/// Runs `source` (the mod's main.luau, named `chunk_name`) in declaring mode, with the default
/// time budget. `owner` is the mod's id: its declared blocks and items must be in its namespace.
/// `is_hook` says whether a hook name exists in the game's catalog.
pub fn declare(
    lua: &Lua,
    owner: &str,
    source: &str,
    chunk_name: &str,
    is_hook: &dyn Fn(&str) -> bool,
) -> Result<Declaration, String> {
    declare_scripts(lua, owner, &[(chunk_name, source)], is_hook)
}

/// The same as `declare` with an explicit time budget.
pub fn declare_with_budget(
    lua: &Lua,
    owner: &str,
    source: &str,
    chunk_name: &str,
    is_hook: &dyn Fn(&str) -> bool,
    budget: Duration,
) -> Result<Declaration, String> {
    declare_scripts_with_budget(lua, owner, &[(chunk_name, source)], is_hook, budget)
}

/// Runs the mod's scripts in manifest order, each as (chunk name, source), in one declaring
/// environment, with the default time budget. The scripts are one mod: they share the environment
/// (a global set by one is seen by the next), the time budget (the whole sequence gets
/// `DECLARE_BUDGET`, not each script), and the caps and records of the `Declaration`. A runtime
/// error in any script refuses the mod, and the scripts after it do not run.
pub fn declare_scripts(
    lua: &Lua,
    owner: &str,
    scripts: &[(&str, &str)],
    is_hook: &dyn Fn(&str) -> bool,
) -> Result<Declaration, String> {
    declare_scripts_with_budget(lua, owner, scripts, is_hook, DECLARE_BUDGET)
}

/// The same as `declare_scripts` with an explicit time budget for the whole sequence.
pub fn declare_scripts_with_budget(
    lua: &Lua,
    owner: &str,
    scripts: &[(&str, &str)],
    is_hook: &dyn Fn(&str) -> bool,
    budget: Duration,
) -> Result<Declaration, String> {
    declare_scripts_settings(
        lua,
        owner,
        scripts,
        is_hook,
        budget,
        &Settings::empty(owner),
    )
}

/// The same as `declare_scripts_with_budget`, with the mod's loaded settings: `horizon.setting` reads
/// them while the mod loads, so a declaration can depend on a setting.
pub fn declare_scripts_settings(
    lua: &Lua,
    owner: &str,
    scripts: &[(&str, &str)],
    is_hook: &dyn Fn(&str) -> bool,
    budget: Duration,
    settings: &Settings,
) -> Result<Declaration, String> {
    let (recorded, loaded) =
        run(lua, owner, scripts, is_hook, budget, settings).map_err(|e| e.to_string())?;
    let hooks = recorded
        .options
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    loaded.map_err(|e| e.to_string())?;
    check_block_entities(&recorded)?;
    check_fluid_parts(&recorded)?;
    Ok(Declaration {
        hooks,
        options: recorded.options,
        blocks: recorded.blocks,
        items: recorded.items,
        parts: recorded.parts,
        item_parts: recorded.item_parts,
        block_entity_types: recorded.block_entity_types,
        block_entity_parts: recorded.block_entity_parts,
        fluids: recorded.fluids,
        fluid_parts: recorded.fluid_parts,
        entity_parts: recorded.entity_parts,
        menu_parts: recorded.menu_parts,
        goals: recorded.goals,
        behaviors: recorded.behaviors,
        client_commands: recorded.client_commands,
        configs: recorded.configs,
        commands: recorded.commands,
        data_map_types: recorded.data_map_types,
        data_pack_files: recorded.data_pack_files,
        setting_changed: recorded.setting_changed,
    })
}

/// The checks that need the whole chunk, after it ran: each block entity type names only blocks this
/// mod declared with `horizon.block` (the game refuses an unknown block at start-up), and each block
/// entity part attaches to a type this mod declared.
fn check_block_entities(recorded: &Recorded) -> Result<(), String> {
    for ty in &recorded.block_entity_types {
        for block in &ty.blocks {
            if !recorded.blocks.iter().any(|record| record.id == *block) {
                return Err(format!(
                    "horizon.block_entity_type '{}': block '{block}' is not declared by this mod with horizon.block",
                    ty.id
                ));
            }
        }
    }
    for part in &recorded.block_entity_parts {
        if !recorded
            .block_entity_types
            .iter()
            .any(|ty| ty.id == part.id)
        {
            return Err(format!(
                "horizon.block_entity_part for '{}' needs a horizon.block_entity_type of this mod with that id",
                part.id
            ));
        }
    }
    Ok(())
}

/// Each fluid part attaches to a world fluid this mod declared: the game can only attach a fluid
/// part to a fluid it knows, and a mod's fluids are its own.
fn check_fluid_parts(recorded: &Recorded) -> Result<(), String> {
    for part in &recorded.fluid_parts {
        if !recorded.fluids.iter().any(|fluid| fluid.id == part.id) {
            return Err(format!(
                "horizon.fluid_part for '{}' needs a horizon.world_fluid of this mod with that id",
                part.id
            ));
        }
    }
    Ok(())
}

/// The handlers a mod registered for `hook`, in registration order.
pub fn handlers(lua: &Lua, hook: &str) -> mlua::Result<Vec<Function>> {
    let Some(all) = lua.named_registry_value::<Option<Table>>(HANDLERS_KEY)? else {
        return Ok(Vec::new());
    };
    let Some(list) = all.raw_get::<Option<Table>>(hook)? else {
        return Ok(Vec::new());
    };
    (1..=list.raw_len())
        .map(|i| list.raw_get::<Function>(i))
        .collect()
}

/// The environment the mod's chunk ran in, the table kept under `ENV_KEY`.
pub fn environment(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value::<Table>(ENV_KEY)
}

/// Reads the optional options table of `horizon.on`. Nil means the defaults. Only `priority` (an
/// integer in the i32 range) and `receive_cancelled` (a boolean) are known; anything else is an
/// error that names the key.
fn parse_options(value: Value) -> mlua::Result<ListenerOptions> {
    let table = match value {
        Value::Nil => return Ok(ListenerOptions::default()),
        Value::Table(table) => table,
        other => {
            return Err(Error::runtime(format!(
                "horizon.on needs its options as a table, not {}",
                other.type_name()
            )));
        }
    };
    let mut options = ListenerOptions::default();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let name = match &key {
            Value::String(s) => s.to_str()?.to_string(),
            other => {
                return Err(Error::runtime(format!(
                    "horizon.on option {} is not a known option",
                    other.type_name()
                )));
            }
        };
        match (name.as_str(), value) {
            ("priority", Value::Integer(n)) if (PRIORITY_MIN..=PRIORITY_MAX).contains(&n) => {
                options.priority = n as i32;
            }
            ("priority", Value::Number(n))
                if n.fract() == 0.0 && (PRIORITY_MIN as f64..=PRIORITY_MAX as f64).contains(&n) =>
            {
                options.priority = n as i32;
            }
            ("priority", _) => {
                return Err(Error::runtime(
                    "horizon.on option 'priority' needs a whole number from -2147483648 to 2147483647",
                ));
            }
            ("receive_cancelled", Value::Boolean(flag)) => options.receive_cancelled = flag,
            ("receive_cancelled", _) => {
                return Err(Error::runtime(
                    "horizon.on option 'receive_cancelled' needs true or false",
                ));
            }
            (other, _) => {
                return Err(Error::runtime(format!(
                    "unknown horizon.on option '{other}'; the options are priority and receive_cancelled"
                )));
            }
        }
    }
    Ok(options)
}

/// Everything a chunk declared while it loaded.
struct Recorded {
    options: Vec<(String, ListenerOptions)>,
    blocks: Vec<BlockRecord>,
    items: Vec<ItemRecord>,
    parts: Vec<PartRecord>,
    item_parts: Vec<PartRecord>,
    block_entity_types: Vec<BlockEntityTypeRecord>,
    block_entity_parts: Vec<PartRecord>,
    fluids: Vec<FluidRecord>,
    fluid_parts: Vec<PartRecord>,
    entity_parts: Vec<PartRecord>,
    menu_parts: Vec<PartRecord>,
    goals: Vec<GoalRecord>,
    behaviors: Vec<BehaviorRecord>,
    client_commands: Vec<ClientCommandRecord>,
    configs: Vec<mod_api::Config>,
    commands: Vec<CommandRecord>,
    data_map_types: Vec<mod_api::DataMapType>,
    data_pack_files: Vec<(String, String)>,
    setting_changed: bool,
}

/// Sets up the environment and the `horizon` table, runs the chunk, and then replaces the declaring
/// functions with ones that only refuse. Returns what the chunk declared, and the chunk's own
/// result: the setup errors come first, and the chunk's result is kept apart so the caller can
/// report it.
fn run(
    lua: &Lua,
    owner: &str,
    scripts: &[(&str, &str)],
    is_hook: &dyn Fn(&str) -> bool,
    budget_limit: Duration,
    settings: &Settings,
) -> mlua::Result<(Recorded, mlua::Result<()>)> {
    let handlers = lua.create_table()?;
    lua.set_named_registry_value(HANDLERS_KEY, handlers.clone())?;

    let horizon = lua.create_table()?;
    let horizon_meta = lua.create_table()?;
    horizon_meta.raw_set(
        "__index",
        lua.create_function(|_, (_, field): (Value, String)| -> mlua::Result<()> {
            Err(Error::runtime(format!(
                "horizon.{field} is not available while the mod loads; use it inside a handler"
            )))
        })?,
    )?;
    horizon.set_metatable(Some(horizon_meta))?;

    let env = lua.create_table()?;
    let env_meta = lua.create_table()?;
    env_meta.raw_set("__index", lua.globals())?;
    env.set_metatable(Some(env_meta))?;
    lua.set_named_registry_value(ENV_KEY, env.clone())?;

    let recorded = RefCell::new(Vec::<(String, ListenerOptions)>::new());
    let blocks = RefCell::new(Vec::<BlockRecord>::new());
    let items = RefCell::new(Vec::<ItemRecord>::new());
    let parts = RefCell::new(Vec::<PartRecord>::new());
    let item_parts = RefCell::new(Vec::<PartRecord>::new());
    let block_entity_types = RefCell::new(Vec::<BlockEntityTypeRecord>::new());
    let block_entity_parts = RefCell::new(Vec::<PartRecord>::new());
    let fluids = RefCell::new(Vec::<FluidRecord>::new());
    let fluid_parts = RefCell::new(Vec::<PartRecord>::new());
    let entity_parts = RefCell::new(Vec::<PartRecord>::new());
    let menu_parts = RefCell::new(Vec::<PartRecord>::new());
    let goals = RefCell::new(Vec::<GoalRecord>::new());
    let behaviors = RefCell::new(Vec::<BehaviorRecord>::new());
    let client_commands = RefCell::new(Vec::<ClientCommandRecord>::new());
    let configs = RefCell::new(Vec::<mod_api::Config>::new());
    let commands = RefCell::new(Vec::<CommandRecord>::new());
    let data_map_types = RefCell::new(Vec::<mod_api::DataMapType>::new());
    let data_pack_files = RefCell::new(Vec::<(String, String)>::new());
    let calls = Cell::new(0_usize);
    let finished = Cell::new(false);
    let setting_seen = Cell::new(false);

    // The declaring functions borrow the caller's `is_hook`, owner and the cells above, so they are
    // scoped functions: they expire when the scope ends, and are replaced below before anything can
    // call them.
    let loaded = lua
        .scope(|scope| {
            let on = scope.create_function(|_, (name, handler, opts): (Value, Value, Value)| {
                if finished.get() {
                    return Err(Error::runtime(LATE_ON));
                }
                let count = calls.get() + 1;
                calls.set(count);
                if count > MAX_HANDLERS {
                    return Err(Error::runtime(format!(
                        "a mod may register at most {MAX_HANDLERS} handlers"
                    )));
                }
                let Value::String(name) = name else {
                    return Err(Error::runtime("horizon.on needs the hook name as a string"));
                };
                let name = name.to_str()?.to_string();
                let Value::Function(handler) = handler else {
                    return Err(Error::runtime("horizon.on needs a function as the handler"));
                };
                if !is_hook(&name) {
                    return Err(Error::runtime(format!("unknown hook '{name}'")));
                }
                let options = parse_options(opts)?;
                {
                    let mut hooks = recorded.borrow_mut();
                    match hooks.iter().find(|(seen, _)| *seen == name) {
                        Some((_, first)) if *first != options => {
                            return Err(Error::runtime(format!(
                                "conflicting options for hook '{name}': every horizon.on of one hook must use the same options"
                            )));
                        }
                        Some(_) => {}
                        None => hooks.push((name.clone(), options)),
                    }
                }
                let list = match handlers.raw_get::<Option<Table>>(name.as_str())? {
                    Some(list) => list,
                    None => {
                        let list = lua.create_table()?;
                        handlers.raw_set(name.as_str(), list.clone())?;
                        list
                    }
                };
                list.raw_set(list.raw_len() + 1, handler)
            })?;
            let block = scope.create_function(|lua, value: Value| {
                let (id, template) = parse_record(lua, "block", owner, value)?;
                let mut records = blocks.borrow_mut();
                if records.len() >= MAX_BLOCKS {
                    return Err(at_line(
                        lua,
                        format!("a mod may declare at most {MAX_BLOCKS} blocks"),
                    ));
                }
                if records.iter().any(|record| record.id == id) {
                    return Err(at_line(
                        lua,
                        format!("horizon.block: id '{id}' is declared twice"),
                    ));
                }
                records.push(BlockRecord { id, template });
                Ok(())
            })?;
            let item = scope.create_function(|lua, value: Value| {
                let (id, template) = parse_record(lua, "item", owner, value)?;
                let mut records = items.borrow_mut();
                if records.len() >= MAX_ITEMS {
                    return Err(at_line(
                        lua,
                        format!("a mod may declare at most {MAX_ITEMS} items"),
                    ));
                }
                if records.iter().any(|record| record.id == id) {
                    return Err(at_line(
                        lua,
                        format!("horizon.item: id '{id}' is declared twice"),
                    ));
                }
                records.push(ItemRecord { id, template });
                Ok(())
            })?;
            let block_part = scope.create_function(|lua, (id, spec): (Value, Value)| {
                declare_part(lua, PartKind::Block, id, spec, &parts, &handlers, None)
            })?;
            let item_part = scope.create_function(|lua, (id, spec): (Value, Value)| {
                declare_part(lua, PartKind::Item, id, spec, &item_parts, &handlers, None)
            })?;
            let block_entity_type = scope.create_function(|lua, value: Value| {
                declare_block_entity_type(lua, owner, value, &block_entity_types)
            })?;
            let block_entity_part = scope.create_function(|lua, (id, spec): (Value, Value)| {
                declare_part(
                    lua,
                    PartKind::BlockEntity,
                    id,
                    spec,
                    &block_entity_parts,
                    &handlers,
                    None,
                )
            })?;
            let world_fluid = scope.create_function(|lua, value: Value| {
                declare_world_fluid(lua, owner, value, &fluids)
            })?;
            let fluid_part = scope.create_function(|lua, (id, spec): (Value, Value)| {
                declare_part(lua, PartKind::Fluid, id, spec, &fluid_parts, &handlers, None)
            })?;
            let entity_part = scope.create_function(|lua, (id, spec): (Value, Value)| {
                declare_part(
                    lua,
                    PartKind::Entity,
                    id,
                    spec,
                    &entity_parts,
                    &handlers,
                    Some(EntityRecords {
                        owner,
                        goals: &goals,
                        behaviors: &behaviors,
                    }),
                )
            })?;
            let menu_part = scope.create_function(|lua, (id, spec): (Value, Value)| {
                declare_part(lua, PartKind::Menu, id, spec, &menu_parts, &handlers, None)
            })?;
            let client_command = scope.create_function(|lua, value: Value| {
                declare_client_command(lua, value, &client_commands)
            })?;
            let config = scope.create_function(|lua, value: Value| {
                declare_config(lua, owner, value, &configs)
            })?;
            let command = scope.create_function(|lua, value: Value| {
                declare_command(lua, value, &commands)
            })?;
            let data_map_type = scope.create_function(|_, value: Value| {
                data_maps::declare_type(owner, value, &data_map_types)
            })?;
            let data_pack_file = scope.create_function(|_, value: Value| {
                data_maps::declare_file(owner, value, &data_pack_files)
            })?;
            let setting = scope.create_function(|lua, key: Value| {
                setting_api::read(lua, settings, &key)
            })?;
            let on_setting_changed = scope.create_function(|lua, handler: Value| {
                if finished.get() {
                    return Err(Error::runtime(setting_api::LATE));
                }
                if setting_seen.get() {
                    return Err(Error::runtime(setting_api::TWICE));
                }
                let Value::Function(handler) = handler else {
                    return Err(Error::runtime(
                        "horizon.on_setting_changed needs a function as the handler",
                    ));
                };
                lua.set_named_registry_value(setting_api::HANDLER_KEY, handler)?;
                setting_seen.set(true);
                Ok(())
            })?;
            horizon.raw_set("setting", setting)?;
            horizon.raw_set("on_setting_changed", on_setting_changed)?;
            horizon.raw_set("on", on)?;
            horizon.raw_set("client_command", client_command)?;
            horizon.raw_set("config", config)?;
            horizon.raw_set("command", command)?;
            horizon.raw_set("data_map_type", data_map_type)?;
            horizon.raw_set("data_pack_file", data_pack_file)?;
            horizon.raw_set("block", block)?;
            horizon.raw_set("item", item)?;
            horizon.raw_set("block_part", block_part)?;
            horizon.raw_set("item_part", item_part)?;
            horizon.raw_set("block_entity_type", block_entity_type)?;
            horizon.raw_set("block_entity_part", block_entity_part)?;
            horizon.raw_set("world_fluid", world_fluid)?;
            horizon.raw_set("fluid_part", fluid_part)?;
            horizon.raw_set("entity_part", entity_part)?;
            horizon.raw_set("menu_part", menu_part)?;
            horizon.set_readonly(true);
            env.raw_set("horizon", horizon.clone())?;

            // One budget frame for the whole sequence: the scripts share the time, not each its own.
            let result = budget::within_with(
                lua,
                budget_limit,
                "the mod ran over its loading time budget",
                || {
                    scripts.iter().try_for_each(|(chunk_name, source)| {
                        lua.load(*source)
                            .set_name(*chunk_name)
                            .set_environment(env.clone())
                            .exec()
                    })
                },
            );
            finished.set(true);
            Ok(result)
        })
        .and_then(|result| result);

    horizon.set_readonly(false);
    for (name, message) in [
        ("on", LATE_ON),
        ("block", LATE_DECLARE),
        ("item", LATE_DECLARE),
        ("block_part", LATE_DECLARE),
        ("item_part", LATE_DECLARE),
        ("block_entity_type", LATE_DECLARE),
        ("block_entity_part", LATE_DECLARE),
        ("world_fluid", LATE_DECLARE),
        ("fluid_part", LATE_DECLARE),
        ("entity_part", LATE_DECLARE),
        ("menu_part", LATE_DECLARE),
        ("client_command", LATE_DECLARE),
        ("config", LATE_DECLARE),
        ("command", LATE_DECLARE),
        ("data_map_type", LATE_DECLARE),
        ("data_pack_file", LATE_DECLARE),
        ("on_setting_changed", setting_api::LATE),
    ] {
        let refuse = lua.create_function(move |_, _: mlua::MultiValue| -> mlua::Result<()> {
            Err(Error::runtime(message))
        })?;
        horizon.raw_set(name, refuse)?;
    }
    horizon.set_readonly(true);

    let recorded = Recorded {
        options: recorded.into_inner(),
        blocks: blocks.into_inner(),
        items: items.into_inner(),
        parts: parts.into_inner(),
        item_parts: item_parts.into_inner(),
        block_entity_types: block_entity_types.into_inner(),
        block_entity_parts: block_entity_parts.into_inner(),
        fluids: fluids.into_inner(),
        fluid_parts: fluid_parts.into_inner(),
        entity_parts: entity_parts.into_inner(),
        menu_parts: menu_parts.into_inner(),
        goals: goals.into_inner(),
        behaviors: behaviors.into_inner(),
        client_commands: client_commands.into_inner(),
        configs: configs.into_inner(),
        commands: commands.into_inner(),
        data_map_types: data_map_types.into_inner(),
        data_pack_files: data_pack_files.into_inner(),
        setting_changed: setting_seen.get(),
    };
    Ok((recorded, loaded))
}

/// The error for a declaring call, with the line of the call when Lua can name it.
fn at_line(lua: &Lua, message: String) -> Error {
    match lua.inspect_stack(1, |debug| debug.current_line()).flatten() {
        Some(line) => Error::runtime(format!("line {line}: {message}")),
        None => Error::runtime(message),
    }
}

/// Reads the fields of `horizon.block{...}` or `horizon.item{...}` (`kind` names the call): `id` and
/// `template`, both strings, and nothing else. Returns the id and the template, checked.
fn parse_record(
    lua: &Lua,
    kind: &str,
    owner: &str,
    value: Value,
) -> mlua::Result<(String, String)> {
    let table = match value {
        Value::Table(table) => table,
        other => {
            return Err(at_line(
                lua,
                format!(
                    "horizon.{kind} needs its fields as a table, not {}",
                    other.type_name()
                ),
            ));
        }
    };
    let mut id = None;
    let mut template = None;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let name = match &key {
            Value::String(name) => name.to_str()?.to_string(),
            _ => {
                return Err(at_line(
                    lua,
                    format!("horizon.{kind} field names are strings"),
                ));
            }
        };
        let slot = match name.as_str() {
            "id" => &mut id,
            "template" => &mut template,
            other => {
                return Err(at_line(
                    lua,
                    format!(
                        "horizon.{kind} has no field '{other}'; its fields are id and template"
                    ),
                ));
            }
        };
        let Value::String(text) = value else {
            return Err(at_line(
                lua,
                format!("horizon.{kind} field '{name}' needs a string"),
            ));
        };
        *slot = Some(text.to_str()?.to_string());
    }
    let id = id.ok_or_else(|| at_line(lua, format!("horizon.{kind} needs a field 'id'")))?;
    let template =
        template.ok_or_else(|| at_line(lua, format!("horizon.{kind} needs a field 'template'")))?;
    check_id(owner, &id)
        .map_err(|reason| at_line(lua, format!("horizon.{kind} field 'id': {reason}")))?;
    check_template(&template)
        .map_err(|reason| at_line(lua, format!("horizon.{kind} field 'template': {reason}")))?;
    Ok((id, template))
}

/// Reads `horizon.block_entity_type{id, blocks}`: `id` in the mod's own namespace, `blocks` a list of
/// block ids in that namespace (at least one, at most `MAX_BLOCKS_PER_TYPE`, none named twice among
/// this mod's types). The record is kept in `records`.
fn declare_block_entity_type(
    lua: &Lua,
    owner: &str,
    value: Value,
    records: &RefCell<Vec<BlockEntityTypeRecord>>,
) -> mlua::Result<()> {
    let table = match value {
        Value::Table(table) => table,
        other => {
            return Err(at_line(
                lua,
                format!(
                    "horizon.block_entity_type needs its fields as a table, not {}",
                    other.type_name()
                ),
            ));
        }
    };
    let mut id = None;
    let mut blocks = None;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let name = match &key {
            Value::String(name) => name.to_str()?.to_string(),
            _ => {
                return Err(at_line(
                    lua,
                    "horizon.block_entity_type field names are strings".to_owned(),
                ));
            }
        };
        match (name.as_str(), value) {
            ("id", Value::String(text)) => id = Some(text.to_str()?.to_string()),
            ("blocks", Value::Table(list)) => {
                let mut named = Vec::<(i64, String)>::new();
                for item in list.pairs::<Value, Value>() {
                    match item? {
                        (Value::Integer(at), Value::String(block)) => {
                            named.push((at, block.to_str()?.to_string()));
                        }
                        _ => {
                            return Err(at_line(
                                lua,
                                "horizon.block_entity_type field 'blocks' needs a list of block id strings"
                                    .to_owned(),
                            ));
                        }
                    }
                }
                named.sort_by_key(|(at, _)| *at);
                blocks = Some(
                    named
                        .into_iter()
                        .map(|(_, block)| block)
                        .collect::<Vec<_>>(),
                );
            }
            ("id" | "blocks", _) => {
                return Err(at_line(
                    lua,
                    format!("horizon.block_entity_type field '{name}' has the wrong type"),
                ));
            }
            (other, _) => {
                return Err(at_line(
                    lua,
                    format!(
                        "horizon.block_entity_type has no field '{other}'; its fields are id and blocks"
                    ),
                ));
            }
        }
    }
    let id = id.ok_or_else(|| {
        at_line(
            lua,
            "horizon.block_entity_type needs a field 'id'".to_owned(),
        )
    })?;
    let blocks = blocks.ok_or_else(|| {
        at_line(
            lua,
            "horizon.block_entity_type needs a field 'blocks'".to_owned(),
        )
    })?;
    check_id(owner, &id).map_err(|reason| {
        at_line(
            lua,
            format!("horizon.block_entity_type field 'id': {reason}"),
        )
    })?;
    if blocks.is_empty() {
        return Err(at_line(
            lua,
            format!("horizon.block_entity_type for '{id}' needs at least one block"),
        ));
    }
    if blocks.len() > MAX_BLOCKS_PER_TYPE {
        return Err(at_line(
            lua,
            format!("a block entity type may name at most {MAX_BLOCKS_PER_TYPE} blocks"),
        ));
    }
    for block in &blocks {
        check_id(owner, block).map_err(|reason| {
            at_line(
                lua,
                format!("horizon.block_entity_type field 'blocks': {reason}"),
            )
        })?;
    }
    let mut records = records.borrow_mut();
    if records.len() >= MAX_BLOCK_ENTITY_TYPES {
        return Err(at_line(
            lua,
            format!("a mod may declare at most {MAX_BLOCK_ENTITY_TYPES} block entity types"),
        ));
    }
    if records.iter().any(|record| record.id == id) {
        return Err(at_line(
            lua,
            format!("horizon.block_entity_type: type '{id}' is declared twice"),
        ));
    }
    for (at, block) in blocks.iter().enumerate() {
        let twice = blocks[..at].contains(block);
        let elsewhere = records.iter().any(|record| record.blocks.contains(block));
        if twice || elsewhere {
            return Err(at_line(
                lua,
                format!(
                    "horizon.block_entity_type: block '{block}' is in two block entity types of this mod"
                ),
            ));
        }
    }
    records.push(BlockEntityTypeRecord { id, blocks });
    Ok(())
}

/// Reads `horizon.world_fluid{id, tick_delay, drop_off, slope_find_distance, source_conversion,
/// bucket}`: `id` in the mod's own namespace, the numbers checked by `WorldFluid::check` (a fluid
/// the simulation cannot run is refused here, by name and line). The missing settings are the
/// game's defaults. At most `MAX_WORLD_FLUIDS` per mod, none declared twice.
fn declare_world_fluid(
    lua: &Lua,
    owner: &str,
    value: Value,
    records: &RefCell<Vec<FluidRecord>>,
) -> mlua::Result<()> {
    let Value::Table(table) = value else {
        return Err(at_line(
            lua,
            "horizon.world_fluid needs its fields as a table".to_owned(),
        ));
    };
    let mut id = None;
    let mut fluid = mod_api::WorldFluid::default();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let name = match &key {
            Value::String(name) => name.to_str()?.to_string(),
            _ => {
                return Err(at_line(
                    lua,
                    "horizon.world_fluid field names are strings".to_owned(),
                ));
            }
        };
        let wrong = |reason: ConvertError| {
            at_line(lua, format!("horizon.world_fluid field '{name}': {reason}"))
        };
        match name.as_str() {
            "id" => {
                let Value::String(text) = &value else {
                    return Err(wrong(ConvertError::expected("a string", &value)));
                };
                id = Some(text.to_str()?.to_string());
            }
            "tick_delay" => fluid.tick_delay = i32::from_lua_value(&value).map_err(wrong)?,
            "drop_off" => fluid.drop_off = i32::from_lua_value(&value).map_err(wrong)?,
            "slope_find_distance" => {
                fluid.slope_find_distance = i32::from_lua_value(&value).map_err(wrong)?;
            }
            "source_conversion" => {
                fluid.source_conversion = bool::from_lua_value(&value).map_err(wrong)?;
            }
            "bucket" => fluid.bucket = bool::from_lua_value(&value).map_err(wrong)?,
            other => {
                return Err(at_line(
                    lua,
                    format!(
                        "horizon.world_fluid has no field '{other}'; its fields are id, tick_delay, drop_off, slope_find_distance, source_conversion and bucket"
                    ),
                ));
            }
        }
    }
    let id = id.ok_or_else(|| at_line(lua, "horizon.world_fluid needs a field 'id'".to_owned()))?;
    check_id(owner, &id)
        .map_err(|reason| at_line(lua, format!("horizon.world_fluid field 'id': {reason}")))?;
    fluid
        .check()
        .map_err(|reason| at_line(lua, format!("horizon.world_fluid '{id}': {reason}")))?;
    let mut records = records.borrow_mut();
    if records.len() >= MAX_WORLD_FLUIDS {
        return Err(at_line(
            lua,
            format!("a mod may declare at most {MAX_WORLD_FLUIDS} world fluids"),
        ));
    }
    if records.iter().any(|record| record.id == id) {
        return Err(at_line(
            lua,
            format!("horizon.world_fluid: fluid '{id}' is declared twice"),
        ));
    }
    records.push(FluidRecord { id, fluid });
    Ok(())
}

/// The name a usage gives its command: its first word, which `check_usage` has already made a
/// literal of `a-z`, `0-9`, `_` and `-`. A usage that passed it has no leading space, so splitting on
/// a space gives the same word as the game's own split on whitespace (`native_server::usage_nodes`).
/// Usages that did not pass it are not declared, so this is never asked of them.
pub fn command_name(usage: &str) -> &str {
    usage.split(' ').next().unwrap_or_default()
}

/// Checks a client command's usage. It must be non-empty and at most `MAX_USAGE` characters, with no
/// control character (a newline included). Its first word is the command's name: a literal of at
/// most `MAX_COMMAND_NAME` characters from `a-z`, `0-9`, `_` and `-`, so it cannot start with an
/// argument. The rest of the usage is the host's to parse, and Lua does not.
pub fn check_usage(usage: &str) -> Result<(), String> {
    if usage.is_empty() {
        return Err("the usage is empty".to_owned());
    }
    if usage.chars().count() > MAX_USAGE {
        return Err(format!("the usage is longer than {MAX_USAGE} characters"));
    }
    if usage.chars().any(char::is_control) {
        return Err("the usage has a control character or a newline".to_owned());
    }
    let name = usage.split(' ').next().unwrap_or_default();
    if name.starts_with(['<', '[']) {
        return Err(
            "the first word is an argument; a client command starts with a literal word".to_owned(),
        );
    }
    if name.is_empty() {
        return Err("the first word is empty".to_owned());
    }
    if name.chars().count() > MAX_COMMAND_NAME {
        return Err(format!(
            "the first word is longer than {MAX_COMMAND_NAME} characters"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(format!(
            "the first word '{name}' must use only a-z, 0-9, '_' and '-'"
        ));
    }
    Ok(())
}

/// Reads the fields of a command declaration: `usage` (a string that passes `check_usage`) and, when
/// `levels` is set, `level` (an integer from 0 to 4, 0 when absent). `name` is the function's name in
/// the messages. Returns the usage and the level.
fn command_fields(lua: &Lua, value: Value, name: &str, levels: bool) -> mlua::Result<(String, u8)> {
    let table = match value {
        Value::Table(table) => table,
        other => {
            return Err(at_line(
                lua,
                format!(
                    "{name} needs its fields as a table, not {}",
                    other.type_name()
                ),
            ));
        }
    };
    let mut usage = None;
    let mut level = None;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let field = match &key {
            Value::String(field) => field.to_str()?.to_string(),
            _ => {
                return Err(at_line(lua, format!("{name} field names are strings")));
            }
        };
        match (field.as_str(), value) {
            ("usage", Value::String(text)) => usage = Some(text.to_str()?.to_string()),
            ("usage", _) => {
                return Err(at_line(lua, format!("{name} field 'usage' needs a string")));
            }
            ("level", value) if levels => level = Some(level_value(lua, name, &value)?),
            (other, _) => {
                let fields = if levels { "usage and level" } else { "usage" };
                return Err(at_line(
                    lua,
                    format!("{name} has no field '{other}'; its fields are {fields}"),
                ));
            }
        }
    }
    let usage = usage.ok_or_else(|| at_line(lua, format!("{name} needs a field 'usage'")))?;
    check_usage(&usage)
        .map_err(|reason| at_line(lua, format!("{name} field 'usage': {reason}")))?;
    Ok((usage, level.unwrap_or(0)))
}

/// The operator level a `horizon.command` names: an integer from 0 to 4. Anything else is refused,
/// not clamped, so a level is never widened by accident.
fn level_value(lua: &Lua, name: &str, value: &Value) -> mlua::Result<u8> {
    let number = match value {
        Value::Integer(number) => *number as f64,
        Value::Number(number) if number.fract() == 0.0 => *number,
        _ => f64::NAN,
    };
    if (0.0..=4.0).contains(&number) {
        Ok(number as u8)
    } else {
        Err(at_line(
            lua,
            format!("{name} field 'level' needs an integer from 0 to 4"),
        ))
    }
}

/// Reads `horizon.client_command{usage = "..."}`: the one field `usage`, a string that passes
/// `check_usage`. The record is kept in `records`, at most `MAX_CLIENT_COMMANDS` and no usage twice.
fn declare_client_command(
    lua: &Lua,
    value: Value,
    records: &RefCell<Vec<ClientCommandRecord>>,
) -> mlua::Result<()> {
    let (usage, _) = command_fields(lua, value, "horizon.client_command", false)?;
    let mut records = records.borrow_mut();
    if records.len() >= MAX_CLIENT_COMMANDS {
        return Err(at_line(
            lua,
            format!("a mod may declare at most {MAX_CLIENT_COMMANDS} client commands"),
        ));
    }
    if records.iter().any(|record| record.usage == usage) {
        return Err(at_line(
            lua,
            format!("horizon.client_command: usage '{usage}' is declared twice"),
        ));
    }
    records.push(ClientCommandRecord { usage });
    Ok(())
}

/// Reads `horizon.command{usage = "...", level = n}`: `usage` as for a client command, and `level`
/// (0 to 4, 0 when absent). The record is kept in `records`, at most `MAX_COMMANDS`, no usage twice.
fn declare_command(
    lua: &Lua,
    value: Value,
    records: &RefCell<Vec<CommandRecord>>,
) -> mlua::Result<()> {
    let (usage, level) = command_fields(lua, value, "horizon.command", true)?;
    let mut records = records.borrow_mut();
    if records.len() >= MAX_COMMANDS {
        return Err(at_line(
            lua,
            format!("a mod may declare at most {MAX_COMMANDS} commands"),
        ));
    }
    if records.iter().any(|record| record.usage == usage) {
        return Err(at_line(
            lua,
            format!("horizon.command: usage '{usage}' is declared twice"),
        ));
    }
    records.push(CommandRecord { usage, level });
    Ok(())
}

/// Checks a config file a mod named with `horizon.config{file}`: a relative path of at most
/// `MAX_CONFIG_FILE` characters, segments of `a-z`, `0-9`, `_`, `.` and `-`, no empty segment and no
/// `..`, ending in `.toml`. It must be the mod's own under the game's rule (`ConfigPath::owned_by`):
/// a folder `<owner>/`, or a file `<owner>-...` or `<owner>.toml`. The game checks it again.
pub fn check_config_file(owner: &str, file: &str) -> Result<(), String> {
    if file.is_empty() {
        return Err("the file is empty".to_owned());
    }
    if file.len() > MAX_CONFIG_FILE {
        return Err(format!(
            "the file is longer than {MAX_CONFIG_FILE} characters"
        ));
    }
    if !file.bytes().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-' | b'/')
    }) {
        return Err(format!(
            "the file '{file}' must use only a-z, 0-9, '_', '.', '-' and '/'"
        ));
    }
    if file
        .split('/')
        .any(|segment| segment.is_empty() || segment == "..")
    {
        return Err("the file has an empty folder name or '..'".to_owned());
    }
    if !file.ends_with(".toml") {
        return Err(format!("the file '{file}' must end with .toml"));
    }
    let path = mod_api::ConfigPath::new(file)
        .map_err(|_| format!("the file '{file}' is not a checked relative path"))?;
    if !path.owned_by(owner) {
        return Err(format!(
            "the file '{file}' is not this mod's: it must be '{owner}/...', '{owner}-...' or '{owner}.toml'"
        ));
    }
    Ok(())
}

/// Reads `horizon.config{kind, file, default}`: `kind` is "client", "common" or "server"; `default`
/// is the text written when the file is missing (a string of at most `MAX_CONFIG_DEFAULT` bytes,
/// UTF-8, no NUL); `file` is optional and checked by `check_config_file`, else the game names it
/// `<owner>-<kind>.toml`. The owner is the host's id, never the script's. At most `MAX_CONFIGS`, and
/// no file twice.
fn declare_config(
    lua: &Lua,
    owner: &str,
    value: Value,
    configs: &RefCell<Vec<mod_api::Config>>,
) -> mlua::Result<()> {
    let table = match value {
        Value::Table(table) => table,
        other => {
            return Err(at_line(
                lua,
                format!(
                    "horizon.config needs its fields as a table, not {}",
                    other.type_name()
                ),
            ));
        }
    };
    let (mut kind, mut file, mut default) = (None, None, None);
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let name = match &key {
            Value::String(name) => name.to_str()?.to_string(),
            _ => {
                return Err(at_line(
                    lua,
                    "horizon.config field names are strings".to_owned(),
                ));
            }
        };
        let text = match (name.as_str(), value) {
            ("kind" | "file" | "default", Value::String(text)) => text,
            ("kind" | "file" | "default", _) => {
                return Err(at_line(
                    lua,
                    format!("horizon.config field '{name}' needs a string"),
                ));
            }
            (other, _) => {
                return Err(at_line(
                    lua,
                    format!(
                        "horizon.config has no field '{other}'; its fields are kind, file and default"
                    ),
                ));
            }
        };
        match name.as_str() {
            "kind" => kind = Some(text),
            "file" => file = Some(text),
            _ => default = Some(text),
        }
    }
    let kind = match kind.map(|text| text.to_str().map(|t| t.to_string())) {
        None => {
            return Err(at_line(
                lua,
                "horizon.config needs a field 'kind'".to_owned(),
            ));
        }
        Some(Ok(kind)) => match kind.as_str() {
            "client" => mod_api::ConfigKind::Client,
            "common" => mod_api::ConfigKind::Common,
            "server" => mod_api::ConfigKind::Server,
            _ => {
                return Err(at_line(
                    lua,
                    "horizon.config field 'kind' must be \"client\", \"common\" or \"server\""
                        .to_owned(),
                ));
            }
        },
        Some(Err(err)) => return Err(err),
    };
    let default =
        default.ok_or_else(|| at_line(lua, "horizon.config needs a field 'default'".to_owned()))?;
    let default = default.to_str()?;
    if default.len() > MAX_CONFIG_DEFAULT {
        return Err(at_line(
            lua,
            format!("horizon.config field 'default' is larger than {MAX_CONFIG_DEFAULT} bytes"),
        ));
    }
    if default.contains('\0') {
        return Err(at_line(
            lua,
            "horizon.config field 'default' has a NUL byte".to_owned(),
        ));
    }
    let mut config = mod_api::Config::new(owner, kind, &default);
    if let Some(file) = file {
        let file = file.to_str()?.to_string();
        check_config_file(owner, &file)
            .map_err(|reason| at_line(lua, format!("horizon.config field 'file': {reason}")))?;
        config = config.file(&file);
    }
    let mut configs = configs.borrow_mut();
    if configs.len() >= MAX_CONFIGS {
        return Err(at_line(
            lua,
            format!("a mod may declare at most {MAX_CONFIGS} configs"),
        ));
    }
    if configs.iter().any(|other| other.file == config.file) {
        return Err(at_line(
            lua,
            format!("horizon.config: file '{}' is declared twice", config.file),
        ));
    }
    configs.push(config);
    Ok(())
}

/// Reads `horizon.block_part(id, {hook = fn, ...})` or `horizon.item_part(...)` (`kind` names it): the
/// part's id and handlers are checked, each handler is stored under [`part_handler_key`], and the
/// record is kept in `records`. An entity part also takes a `goals` and a `behaviors` table, read by
/// `declare_goals` and `declare_behaviors` into the records of `entity`.
fn declare_part(
    lua: &Lua,
    kind: PartKind,
    id: Value,
    spec: Value,
    records: &RefCell<Vec<PartRecord>>,
    handlers: &Table,
    entity: Option<EntityRecords<'_>>,
) -> mlua::Result<()> {
    let keyword = kind.keyword();
    let id = part_id(lua, kind, id)?;
    let Value::Table(table) = spec else {
        return Err(at_line(
            lua,
            format!("horizon.{keyword} needs its handlers as a table"),
        ));
    };
    let mut records = records.borrow_mut();
    let cap = match kind {
        PartKind::Entity => MAX_ENTITY_PARTS,
        PartKind::Menu => MAX_MENU_PARTS,
        _ => MAX_PARTS,
    };
    if records.len() >= cap {
        return Err(at_line(
            lua,
            format!("a mod may declare at most {cap} {} parts", kind.noun()),
        ));
    }
    if records.iter().any(|record| record.id == id) {
        return Err(at_line(
            lua,
            format!(
                "horizon.{keyword}: {} '{id}' has two parts in this mod",
                kind.noun()
            ),
        ));
    }
    let mut hooks = Vec::new();
    let mut goal_list = None;
    let mut behavior_list = None;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(name) = key else {
            return Err(at_line(
                lua,
                format!("horizon.{keyword} handler names are strings"),
            ));
        };
        let name = name.to_str()?.to_string();
        if name == "goals" && entity.is_some() {
            goal_list = Some(value);
            continue;
        }
        if name == "behaviors" && entity.is_some() {
            behavior_list = Some(value);
            continue;
        }
        let Value::Function(handler) = value else {
            return Err(at_line(
                lua,
                format!("horizon.{keyword} handler '{name}' needs a function"),
            ));
        };
        check_part_handler(kind, &name)
            .map_err(|reason| at_line(lua, format!("horizon.{keyword} '{name}': {reason}")))?;
        let list = lua.create_table()?;
        list.raw_set(1, handler)?;
        handlers.raw_set(part_handler_key(kind, &id, &name), list)?;
        hooks.push(name);
    }
    let mut declared = 0;
    if let Some(records) = &entity {
        if let Some(value) = goal_list {
            declared += declare_goals(lua, records.owner, &id, value, records.goals, handlers)?;
        }
        if let Some(value) = behavior_list {
            declared +=
                declare_behaviors(lua, records.owner, &id, value, records.behaviors, handlers)?;
        }
    }
    if hooks.is_empty() && declared == 0 {
        let or_goal = if entity.is_some() {
            " or goal or behavior"
        } else {
            ""
        };
        return Err(at_line(
            lua,
            format!("horizon.{keyword} for '{id}' needs at least one handler{or_goal}"),
        ));
    }
    hooks.sort();
    records.push(PartRecord { id, hooks });
    Ok(())
}

/// What an entity part's table may declare beside its handlers: the mod's id (its goals' and behaviors'
/// names must be in its namespace) and the mod's goal and behavior records.
struct EntityRecords<'a> {
    owner: &'a str,
    goals: &'a RefCell<Vec<GoalRecord>>,
    behaviors: &'a RefCell<Vec<BehaviorRecord>>,
}

/// Reads a list of tables from the `key` field (`goals` or `behaviors`) of the entity part `entity`: a
/// sequence of at most `max` entries, each returned for its own declaration. `noun` names an entry in
/// the messages.
fn entity_list(
    lua: &Lua,
    entity: &str,
    key: &str,
    noun: &str,
    value: Value,
    max: usize,
) -> mlua::Result<Vec<Value>> {
    let Value::Table(list) = value else {
        return Err(at_line(
            lua,
            format!("horizon.entity_part '{entity}': {key} is a list of {noun} tables"),
        ));
    };
    let count = list.raw_len();
    let mut entries = 0;
    for pair in list.pairs::<Value, Value>() {
        pair?;
        entries += 1;
    }
    if entries != count {
        return Err(at_line(
            lua,
            format!("horizon.entity_part '{entity}': {key} is a list, with entries 1 to {count}"),
        ));
    }
    if count > max {
        return Err(at_line(
            lua,
            format!("horizon.entity_part '{entity}' declares at most {max} {key}"),
        ));
    }
    (1..=count).map(|index| list.raw_get(index)).collect()
}

/// Reads the `goals` table of the entity part `entity`: a list of at most `MAX_GOALS` goal tables,
/// each read by `declare_goal`. Returns how many goals it declared.
fn declare_goals(
    lua: &Lua,
    owner: &str,
    entity: &str,
    value: Value,
    records: &RefCell<Vec<GoalRecord>>,
    handlers: &Table,
) -> mlua::Result<usize> {
    let goals = entity_list(lua, entity, "goals", "goal", value, MAX_GOALS)?;
    for goal in &goals {
        declare_goal(lua, owner, entity, goal.clone(), records, handlers)?;
    }
    Ok(goals.len())
}

/// Reads the `behaviors` table of the entity part `entity`: a list of at most `MAX_BEHAVIORS` behavior
/// tables, each read by `declare_behavior`. Returns how many behaviors it declared.
fn declare_behaviors(
    lua: &Lua,
    owner: &str,
    entity: &str,
    value: Value,
    records: &RefCell<Vec<BehaviorRecord>>,
    handlers: &Table,
) -> mlua::Result<usize> {
    let behaviors = entity_list(lua, entity, "behaviors", "behavior", value, MAX_BEHAVIORS)?;
    for behavior in &behaviors {
        declare_behavior(lua, owner, entity, behavior.clone(), records, handlers)?;
    }
    Ok(behaviors.len())
}

/// The vanilla activity a behavior's `activity` names (the brain's lanes, as `Activity` has them).
pub fn activity_named(name: &str) -> Option<mod_api::Activity> {
    use mod_api::Activity as A;
    Some(match name {
        "core" => A::Core,
        "idle" => A::Idle,
        "work" => A::Work,
        "play" => A::Play,
        "rest" => A::Rest,
        "meet" => A::Meet,
        "panic" => A::Panic,
        "raid" => A::Raid,
        "pre_raid" => A::PreRaid,
        "hide" => A::Hide,
        "fight" => A::Fight,
        "play_dead" => A::PlayDead,
        "sniff" => A::Sniff,
        "dig" => A::Dig,
        "emerge" => A::Emerge,
        "roar" => A::Roar,
        "investigate" => A::Investigate,
        _ => return None,
    })
}

/// Reads one behavior table of the entity part `entity`: `name` (namespaced to the mod, at most
/// `MAX_BEHAVIOR_NAME` bytes, declared once per mod), `activity` (a vanilla activity name), `priority`
/// (a whole number from 0 to `MAX_BEHAVIOR_PRIORITY`), `min_duration` and `max_duration` (optional, the
/// duration window of `DEFAULT_BEHAVIOR_DURATION`, which `BehaviorRegistrar::add` checks), and the
/// handlers. Any other key is refused by name.
fn declare_behavior(
    lua: &Lua,
    owner: &str,
    entity: &str,
    value: Value,
    records: &RefCell<Vec<BehaviorRecord>>,
    handlers: &Table,
) -> mlua::Result<()> {
    let Value::Table(table) = value else {
        return Err(at_line(
            lua,
            format!("horizon.entity_part '{entity}': each behavior is a table"),
        ));
    };
    let mut name = None;
    let mut activity = None;
    let mut priority = None;
    let mut min_duration = None;
    let mut max_duration = None;
    let mut found = Vec::<(String, mlua::Function)>::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(key) = key else {
            return Err(at_line(lua, "a behavior's keys are names".to_owned()));
        };
        let key = key.to_str()?.to_string();
        match (key.as_str(), value) {
            ("name", Value::String(text)) => name = Some(text.to_str()?.to_string()),
            ("name", _) => return Err(at_line(lua, "a behavior's name is a string".to_owned())),
            ("activity", Value::String(text)) => {
                let text = text.to_str()?.to_string();
                let Some(found) = activity_named(&text) else {
                    return Err(at_line(
                        lua,
                        format!(
                            "a behavior's activity '{text}' is not a brain activity; the activities are core, idle, work, play, rest, meet, panic, raid, pre_raid, hide, fight, play_dead, sniff, dig, emerge, roar and investigate"
                        ),
                    ));
                };
                activity = Some(found);
            }
            ("activity", _) => {
                return Err(at_line(lua, "a behavior's activity is a name".to_owned()));
            }
            ("priority", Value::Integer(number)) => {
                if !(0..=MAX_BEHAVIOR_PRIORITY).contains(&number) {
                    return Err(at_line(
                        lua,
                        format!(
                            "a behavior's priority is from 0 to {MAX_BEHAVIOR_PRIORITY}, not {number}"
                        ),
                    ));
                }
                priority = Some(number as i32);
            }
            ("priority", _) => {
                return Err(at_line(
                    lua,
                    "a behavior's priority is a whole number".to_owned(),
                ));
            }
            ("min_duration", Value::Integer(number)) => min_duration = Some(duration(lua, number)?),
            ("max_duration", Value::Integer(number)) => max_duration = Some(duration(lua, number)?),
            ("min_duration" | "max_duration", _) => {
                return Err(at_line(
                    lua,
                    format!("a behavior's {key} is a whole number"),
                ));
            }
            (
                hook @ ("check_extra_start_conditions"
                | "start"
                | "can_still_use"
                | "tick"
                | "stop"),
                Value::Function(f),
            ) => {
                found.push((hook.to_owned(), f));
            }
            (
                hook @ ("check_extra_start_conditions"
                | "start"
                | "can_still_use"
                | "tick"
                | "stop"),
                _,
            ) => {
                return Err(at_line(
                    lua,
                    format!("a behavior's handler '{hook}' needs a function"),
                ));
            }
            (other, _) => {
                return Err(at_line(
                    lua,
                    format!(
                        "unknown behavior key '{other}'; the keys are name, activity, priority, min_duration, max_duration, check_extra_start_conditions, start, can_still_use, tick and stop"
                    ),
                ));
            }
        }
    }
    let Some(name) = name else {
        return Err(at_line(
            lua,
            format!("a behavior of '{entity}' needs a name"),
        ));
    };
    if name.len() > MAX_BEHAVIOR_NAME {
        return Err(at_line(
            lua,
            format!("the behavior '{name}' is longer than {MAX_BEHAVIOR_NAME} bytes"),
        ));
    }
    check_id(owner, &name)
        .map_err(|reason| at_line(lua, format!("horizon.entity_part behavior: {reason}")))?;
    let mut records = records.borrow_mut();
    if records.iter().any(|record| record.name == name) {
        return Err(at_line(
            lua,
            format!("horizon.entity_part: the behavior '{name}' is declared twice in this mod"),
        ));
    }
    let Some(activity) = activity else {
        return Err(at_line(
            lua,
            format!("the behavior '{name}' needs an activity"),
        ));
    };
    let Some(priority) = priority else {
        return Err(at_line(
            lua,
            format!("the behavior '{name}' needs a priority"),
        ));
    };
    let (default_min, default_max) = DEFAULT_BEHAVIOR_DURATION;
    let min_duration = min_duration.unwrap_or(default_min);
    let max_duration = max_duration.unwrap_or(default_max);
    // The window `BehaviorRegistrar::add` draws from: `nextInt(max + 1 - min)` needs min >= 0 and a span of 1 or more.
    let span = i64::from(max_duration) + 1 - i64::from(min_duration);
    if min_duration < 0 || span < 1 {
        return Err(at_line(
            lua,
            format!(
                "the behavior '{name}': the duration ({min_duration}, {max_duration}) is not one vanilla draws: min_duration >= 0 and max_duration + 1 - min_duration >= 1"
            ),
        ));
    }
    if found.is_empty() {
        return Err(at_line(
            lua,
            format!("the behavior '{name}' needs a handler"),
        ));
    }
    let mut hooks = Vec::new();
    for (hook, handler) in found {
        let list = lua.create_table()?;
        list.raw_set(1, handler)?;
        handlers.raw_set(behavior_handler_key(&name, &hook), list)?;
        hooks.push(hook);
    }
    hooks.sort();
    records.push(BehaviorRecord {
        entity: entity.to_owned(),
        name,
        activity,
        priority,
        min_duration,
        max_duration,
        hooks,
    });
    Ok(())
}

/// A duration bound of a behavior: a whole number that fits the registrar's `i32`.
fn duration(lua: &Lua, number: i64) -> mlua::Result<i32> {
    i32::try_from(number).map_err(|_| {
        at_line(
            lua,
            format!("a behavior's duration {number} does not fit a 32-bit whole number"),
        )
    })
}

/// Reads one goal table of the entity part `entity`: `name` (namespaced to the mod, at most
/// `MAX_GOAL_NAME` bytes, declared once), `priority` (a whole number from 0 to `MAX_GOAL_PRIORITY`),
/// `flags` (optional: a list of `move`, `look`, `jump` and `target`), and the handlers `can_use`
/// (required), `can_continue`, `start`, `tick` and `stop`. Any other key is refused by name.
fn declare_goal(
    lua: &Lua,
    owner: &str,
    entity: &str,
    value: Value,
    records: &RefCell<Vec<GoalRecord>>,
    handlers: &Table,
) -> mlua::Result<()> {
    let Value::Table(goal) = value else {
        return Err(at_line(
            lua,
            format!("horizon.entity_part '{entity}': each goal is a table"),
        ));
    };
    let mut name = None;
    let mut priority = None;
    let mut flags = mod_api::GoalFlags::NONE;
    let mut found = Vec::<(&str, mlua::Function)>::new();
    for pair in goal.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(key) = key else {
            return Err(at_line(lua, "a goal's keys are names".to_owned()));
        };
        let key = key.to_str()?.to_string();
        match (key.as_str(), value) {
            ("name", Value::String(text)) => name = Some(text.to_str()?.to_string()),
            ("priority", Value::Integer(number)) => {
                if !(0..=MAX_GOAL_PRIORITY).contains(&number) {
                    return Err(at_line(
                        lua,
                        format!("a goal's priority is from 0 to {MAX_GOAL_PRIORITY}, not {number}"),
                    ));
                }
                priority = Some(number as i32);
            }
            ("priority", _) => {
                return Err(at_line(
                    lua,
                    "a goal's priority is a whole number".to_owned(),
                ));
            }
            ("flags", value) => flags = goal_flags(lua, value)?,
            ("name", _) => return Err(at_line(lua, "a goal's name is a string".to_owned())),
            ("can_use", Value::Function(f)) => found.push(("can_use", f)),
            ("can_continue", Value::Function(f)) => found.push(("can_continue", f)),
            ("start", Value::Function(f)) => found.push(("start", f)),
            ("tick", Value::Function(f)) => found.push(("tick", f)),
            ("stop", Value::Function(f)) => found.push(("stop", f)),
            (hook @ ("can_use" | "can_continue" | "start" | "tick" | "stop"), _) => {
                return Err(at_line(
                    lua,
                    format!("a goal's handler '{hook}' needs a function"),
                ));
            }
            (other, _) => {
                return Err(at_line(
                    lua,
                    format!(
                        "unknown goal key '{other}'; the keys are name, priority, flags, can_use, can_continue, start, tick and stop"
                    ),
                ));
            }
        }
    }
    let Some(name) = name else {
        return Err(at_line(lua, format!("a goal of '{entity}' needs a name")));
    };
    if name.len() > MAX_GOAL_NAME {
        return Err(at_line(
            lua,
            format!("the goal '{name}' is longer than {MAX_GOAL_NAME} bytes"),
        ));
    }
    check_id(owner, &name)
        .map_err(|reason| at_line(lua, format!("horizon.entity_part goal: {reason}")))?;
    let mut records = records.borrow_mut();
    if records.iter().any(|record| record.name == name) {
        return Err(at_line(
            lua,
            format!("horizon.entity_part: the goal '{name}' is declared twice in this mod"),
        ));
    }
    let Some(priority) = priority else {
        return Err(at_line(lua, format!("the goal '{name}' needs a priority")));
    };
    if !found.iter().any(|(hook, _)| *hook == "can_use") {
        return Err(at_line(lua, format!("the goal '{name}' needs can_use")));
    }
    let mut hooks = Vec::new();
    for (hook, handler) in found {
        let list = lua.create_table()?;
        list.raw_set(1, handler)?;
        handlers.raw_set(goal_handler_key(&name, hook), list)?;
        hooks.push(hook.to_owned());
    }
    hooks.sort();
    records.push(GoalRecord {
        entity: entity.to_owned(),
        name,
        priority,
        flags,
        hooks,
    });
    Ok(())
}

/// The flags a goal's `flags` list names: `move`, `look`, `jump` and `target`, each as the game's
/// `GoalFlags`. Any other name is refused by name.
fn goal_flags(lua: &Lua, value: Value) -> mlua::Result<mod_api::GoalFlags> {
    let Value::Table(list) = value else {
        return Err(at_line(
            lua,
            "a goal's flags are a list of names: move, look, jump and target".to_owned(),
        ));
    };
    let mut flags = mod_api::GoalFlags::NONE;
    for entry in list.sequence_values::<Value>() {
        let Value::String(text) = entry? else {
            return Err(at_line(lua, "a goal's flags are names".to_owned()));
        };
        let text = text.to_str()?.to_string();
        flags = flags.union(match text.as_str() {
            "move" => mod_api::GoalFlags::MOVE,
            "look" => mod_api::GoalFlags::LOOK,
            "jump" => mod_api::GoalFlags::JUMP,
            "target" => mod_api::GoalFlags::TARGET,
            other => {
                return Err(at_line(
                    lua,
                    format!(
                        "unknown goal flag '{other}'; the flags are move, look, jump and target"
                    ),
                ));
            }
        });
    }
    Ok(flags)
}

/// Whether `name` is a handler the host binds for a part of `kind`: an error with the reason when it
/// is not.
fn check_part_handler(kind: PartKind, name: &str) -> Result<(), String> {
    match kind.handlers().iter().find(|(hook, _)| *hook == name) {
        None => Err(match kind {
            PartKind::Block => "not a block part handler".to_owned(),
            PartKind::Item => "not an item part handler".to_owned(),
            PartKind::BlockEntity => "not a block entity part handler".to_owned(),
            PartKind::Fluid => "not a fluid part handler".to_owned(),
            PartKind::Entity => "not an entity part handler".to_owned(),
            PartKind::Menu => "not a menu part handler".to_owned(),
        }),
        Some((_, None)) => Ok(()),
        Some((_, Some(reason))) => Err(format!("not available: {reason}")),
    }
}

/// The id of `horizon.block_part` or `horizon.item_part`: `namespace:path` in any namespace, with a
/// valid path. A part may attach to a block or item of another mod or a vanilla one, so the namespace
/// is not checked here.
fn part_id(lua: &Lua, kind: PartKind, value: Value) -> mlua::Result<String> {
    let keyword = kind.keyword();
    let Value::String(text) = value else {
        return Err(at_line(
            lua,
            format!("horizon.{keyword} needs the {} id as a string", kind.noun()),
        ));
    };
    let id = text.to_str()?.to_string();
    let valid = match id.split_once(':') {
        Some((namespace, path)) => !namespace.is_empty() && check_path(&id, path).is_ok(),
        None => false,
    };
    if !valid {
        let example = match kind {
            PartKind::Block => "minecraft:stone",
            PartKind::Item => "minecraft:stick",
            PartKind::BlockEntity => "mymod:press",
            PartKind::Fluid => "mymod:oil",
            PartKind::Entity => "minecraft:zombie",
            PartKind::Menu => "mymod:press",
        };
        return Err(at_line(
            lua,
            format!(
                "horizon.{keyword}: '{id}' needs a namespaced {} id such as {example}",
                kind.noun()
            ),
        ));
    }
    Ok(id)
}

/// An id `namespace:path` in `owner`'s own namespace, with a valid path.
fn check_id(owner: &str, id: &str) -> Result<(), String> {
    let Some((namespace, path)) = id.split_once(':') else {
        return Err(format!("'{id}' needs a namespaced id such as {owner}:name"));
    };
    if namespace != owner {
        return Err(format!("'{id}' is outside this mod's namespace '{owner}'"));
    }
    check_path(id, path)
}

/// A vanilla template `minecraft:<path>`.
fn check_template(template: &str) -> Result<(), String> {
    match template.split_once(':') {
        Some(("minecraft", path)) => check_path(template, path),
        _ => Err(format!(
            "'{template}' needs a vanilla id such as minecraft:stone"
        )),
    }
}

/// A path of lowercase letters, digits, `_`, `-`, `.` and `/`, not empty.
pub(crate) fn check_path(whole: &str, path: &str) -> Result<(), String> {
    let valid = !path.is_empty()
        && path.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/')
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{whole}' has an invalid path; use lowercase letters, digits, '_', '-', '.' and '/'"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::new_state;
    use std::time::Instant;

    fn is_known(name: &str) -> bool {
        name == "tick" || name == "chat"
    }

    fn declare_in_fresh_state(source: &str) -> Result<Declaration, String> {
        let lua = new_state().unwrap();
        declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known)
    }

    #[test]
    fn on_records_hooks_in_order_without_duplicates() {
        let lua = new_state().unwrap();
        let source = "\
            horizon.on(\"tick\", function() end)\n\
            horizon.on(\"chat\", function() end)\n\
            horizon.on(\"tick\", function() end)\n";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert_eq!(declaration.hooks, ["tick", "chat"]);
        assert_eq!(handlers(&lua, "tick").unwrap().len(), 2);
        assert_eq!(handlers(&lua, "chat").unwrap().len(), 1);
        assert!(handlers(&lua, "nope").unwrap().is_empty());
    }

    #[test]
    fn an_unknown_hook_is_an_error_naming_it() {
        let err = declare_in_fresh_state("horizon.on(\"nope\", function() end)").unwrap_err();
        assert!(err.contains("unknown hook 'nope'"), "got {err}");
    }

    #[test]
    fn other_api_is_not_available_while_loading() {
        let err = declare_in_fresh_state("horizon.world()").unwrap_err();
        assert!(
            err.contains("not available while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn the_horizon_table_cannot_be_changed() {
        let err = declare_in_fresh_state("horizon.on = nil").unwrap_err();
        assert!(err.contains("readonly"), "got {err}");
        let err = declare_in_fresh_state("horizon.extra = 1").unwrap_err();
        assert!(err.contains("readonly"), "got {err}");
    }

    #[test]
    fn horizon_on_is_refused_after_loading() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"tick\", function() horizon.on(\"chat\", function() end) end)";
        declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("only be used while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn a_runaway_top_level_is_stopped_and_the_state_stays_usable() {
        let lua = new_state().unwrap();
        let started = Instant::now();
        let err = declare_with_budget(
            &lua,
            "mymod",
            "while true do end",
            "@lua-mods/test/main.luau",
            &is_known,
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(err.contains("time budget"), "got {err}");
        assert!(started.elapsed() < crate::host::tests::HANG_GUARD);
        let two: i64 = lua.load("return 1 + 1").eval().unwrap();
        assert_eq!(two, 2);
    }

    #[test]
    fn a_syntax_error_names_the_chunk() {
        let err = declare(
            &new_state().unwrap(),
            "mymod",
            "this is not lua",
            "@lua-mods/x/main.luau",
            &is_known,
        )
        .unwrap_err();
        assert!(err.contains("lua-mods/x/main.luau"), "got {err}");
    }

    #[test]
    fn too_many_handlers_are_refused() {
        let source = "for i = 1, 300 do horizon.on(\"tick\", function() end) end";
        let err = declare_in_fresh_state(source).unwrap_err();
        assert!(err.contains("at most 256"), "got {err}");
    }

    #[test]
    fn the_standard_libraries_are_there() {
        let source = "horizon.on(\"tick\", function() return math.floor(1.5) end)";
        let declaration = declare_in_fresh_state(source).unwrap();
        assert_eq!(declaration.hooks, ["tick"]);
    }

    #[test]
    fn environment_returns_the_mods_environment() {
        let lua = new_state().unwrap();
        declare(
            &lua,
            "mymod",
            "horizon.on(\"tick\", function() end)",
            "@lua-mods/test/main.luau",
            &is_known,
        )
        .unwrap();
        let env = environment(&lua).unwrap();
        assert!(env.raw_get::<Table>("horizon").is_ok());
    }

    #[test]
    fn a_captured_on_is_dead_after_loading() {
        // The scoped `horizon.on` expires with the scope: a copy a mod kept must fail, not crash.
        let lua = new_state().unwrap();
        let source = "kept = horizon.on\n\
                      horizon.on(\"tick\", function() kept(\"chat\", function() end) end)";
        declare(&lua, "mymod", source, "=mod", &|n| {
            n == "tick" || n == "chat"
        })
        .unwrap();
        let tick = handlers(&lua, "tick").unwrap();
        assert!(tick[0].call::<()>(()).is_err());
    }

    #[test]
    fn pcall_cannot_swallow_the_time_budget() {
        // The interrupt keeps failing after the deadline, so catching one error does not help.
        let lua = new_state().unwrap();
        let source = "while true do pcall(function() while true do end end) end";
        let started = std::time::Instant::now();
        let result = declare_with_budget(
            &lua,
            "mymod",
            source,
            "=mod",
            &|_| true,
            Duration::from_millis(100),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < crate::host::tests::HANG_GUARD);
    }

    fn options_of(source: &str) -> Result<Vec<(String, ListenerOptions)>, String> {
        declare_in_fresh_state(source).map(|d| d.options)
    }

    #[test]
    fn options_default_to_priority_zero_and_not_receiving_cancelled() {
        let options = options_of("horizon.on(\"tick\", function() end)").unwrap();
        assert_eq!(options, [("tick".to_string(), ListenerOptions::default())]);
        assert_eq!(options[0].1.priority, 0);
        assert!(!options[0].1.receive_cancelled);
    }

    #[test]
    fn both_options_are_read() {
        let source =
            "horizon.on(\"tick\", function() end, {priority = -7, receive_cancelled = true})";
        let options = options_of(source).unwrap();
        assert_eq!(
            options[0].1,
            ListenerOptions {
                priority: -7,
                receive_cancelled: true
            }
        );
    }

    #[test]
    fn a_priority_at_the_i32_limits_is_kept() {
        let source = "horizon.on(\"tick\", function() end, {priority = 2147483647})\n\
                      horizon.on(\"chat\", function() end, {priority = -2147483648})";
        let options = options_of(source).unwrap();
        assert_eq!(options[0].1.priority, i32::MAX);
        assert_eq!(options[1].1.priority, i32::MIN);
    }

    #[test]
    fn a_priority_out_of_the_i32_range_is_refused() {
        for value in ["2147483648", "-2147483649", "1e12"] {
            let source = format!("horizon.on(\"tick\", function() end, {{priority = {value}}})");
            let err = options_of(&source).unwrap_err();
            assert!(err.contains("'priority'"), "{value}: got {err}");
        }
    }

    #[test]
    fn a_fractional_priority_is_refused() {
        let err = options_of("horizon.on(\"tick\", function() end, {priority = 1.5})").unwrap_err();
        assert!(err.contains("'priority'"), "got {err}");
    }

    #[test]
    fn a_string_priority_is_refused() {
        let err =
            options_of("horizon.on(\"tick\", function() end, {priority = \"5\"})").unwrap_err();
        assert!(err.contains("'priority'"), "got {err}");
    }

    #[test]
    fn an_unknown_option_key_is_refused_by_name() {
        let err = options_of("horizon.on(\"tick\", function() end, {colour = 1})").unwrap_err();
        assert!(
            err.contains("unknown horizon.on option 'colour'"),
            "got {err}"
        );
    }

    #[test]
    fn options_that_are_not_a_table_are_refused() {
        let err = options_of("horizon.on(\"tick\", function() end, 5)").unwrap_err();
        assert!(err.contains("needs its options as a table"), "got {err}");
    }

    #[test]
    fn receive_cancelled_must_be_a_boolean() {
        let err = options_of("horizon.on(\"tick\", function() end, {receive_cancelled = 1})")
            .unwrap_err();
        assert!(
            err.contains("'receive_cancelled' needs true or false"),
            "got {err}"
        );
    }

    #[test]
    fn two_calls_for_one_hook_with_the_same_options_are_fine() {
        let source = "horizon.on(\"tick\", function() end, {priority = 3})\n\
                      horizon.on(\"tick\", function() end, {priority = 3})";
        let declaration = declare_in_fresh_state(source).unwrap();
        assert_eq!(declaration.hooks, ["tick"]);
        assert_eq!(declaration.options[0].1.priority, 3);
    }

    #[test]
    fn two_calls_for_one_hook_with_different_options_are_refused() {
        let source = "horizon.on(\"tick\", function() end)\n\
                      horizon.on(\"tick\", function() end, {priority = 1})";
        let err = declare_in_fresh_state(source).unwrap_err();
        assert!(
            err.contains("conflicting options for hook 'tick'"),
            "got {err}"
        );
    }

    #[test]
    fn options_are_kept_per_hook_in_first_seen_order() {
        let source = "horizon.on(\"chat\", function() end, {priority = 2})\n\
                      horizon.on(\"tick\", function() end)";
        let declaration = declare_in_fresh_state(source).unwrap();
        assert_eq!(declaration.hooks, ["chat", "tick"]);
        assert_eq!(declaration.options.len(), 2);
        assert_eq!(declaration.options[0].0, "chat");
        assert_eq!(declaration.options[0].1.priority, 2);
        assert_eq!(
            declaration.options[1],
            ("tick".to_string(), ListenerOptions::default())
        );
    }

    fn block_error(source: &str) -> String {
        declare_in_fresh_state(source).unwrap_err()
    }

    #[test]
    fn blocks_and_items_are_recorded_in_declaration_order() {
        let source = "\
            horizon.block{id = \"mymod:lamp\", template = \"minecraft:stone\"}\n\
            horizon.item{id = \"mymod:wand\", template = \"minecraft:stick\"}\n\
            horizon.block{id = \"mymod:pipe\", template = \"minecraft:iron_block\"}\n";
        let declaration = declare_in_fresh_state(source).unwrap();
        let blocks: Vec<(&str, &str)> = declaration
            .blocks
            .iter()
            .map(|b| (b.id.as_str(), b.template.as_str()))
            .collect();
        assert_eq!(
            blocks,
            [
                ("mymod:lamp", "minecraft:stone"),
                ("mymod:pipe", "minecraft:iron_block")
            ]
        );
        assert_eq!(declaration.items.len(), 1);
        assert_eq!(declaration.items[0].id, "mymod:wand");
        assert_eq!(declaration.items[0].template, "minecraft:stick");
        assert!(declaration.hooks.is_empty());
    }

    #[test]
    fn a_record_becomes_the_game_block_and_item_with_its_id_and_template() {
        let source = "horizon.block{id = \"mymod:lamp\", template = \"minecraft:stone\"}\n\
                      horizon.item{id = \"mymod:wand\", template = \"minecraft:stick\"}";
        let declaration = declare_in_fresh_state(source).unwrap();
        let block = declaration.blocks[0].to_block();
        assert_eq!(block.id, "mymod:lamp");
        assert_eq!(block.template, "minecraft:stone");
        assert!(block.item, "a block has its item unless told not to");
        let item = declaration.items[0].to_item();
        assert_eq!(item.id, "mymod:wand");
        assert_eq!(item.template, "minecraft:stick");
    }

    #[test]
    fn a_mod_with_no_declarations_records_none() {
        let declaration = declare_in_fresh_state("horizon.on(\"tick\", function() end)").unwrap();
        assert!(declaration.blocks.is_empty());
        assert!(declaration.items.is_empty());
    }

    fn command_error(source: &str) -> String {
        declare_in_fresh_state(source).unwrap_err()
    }

    #[test]
    fn server_commands_are_recorded_with_their_levels_and_level_zero_by_default() {
        let source = "horizon.command{usage = \"ban <who:word>\", level = 2}\n\
                      horizon.command{usage = \"hush\"}\n";
        let declaration = declare_in_fresh_state(source).unwrap();
        let got: Vec<(&str, u8)> = declaration
            .commands
            .iter()
            .map(|c| (c.usage.as_str(), c.level))
            .collect();
        assert_eq!(got, [("ban <who:word>", 2), ("hush", 0)]);
        assert!(declaration.client_commands.is_empty());
        assert!(declaration.hooks.is_empty());
    }

    #[test]
    fn a_command_level_is_an_integer_from_0_to_4_and_nothing_else() {
        for level in ["5", "-1", "1.5", "\"2\"", "true"] {
            let err = command_error(&format!(
                "horizon.command{{usage = \"x\", level = {level}}}"
            ));
            assert!(
                err.contains("needs an integer from 0 to 4"),
                "{level}: got {err}"
            );
        }
        for level in ["0", "4", "4.0"] {
            let source = format!("horizon.command{{usage = \"x\", level = {level}}}");
            assert!(
                declare_in_fresh_state(&source).is_ok(),
                "{level} should pass"
            );
        }
    }

    #[test]
    fn a_command_usage_follows_the_client_rules_and_its_fields_are_usage_and_level() {
        let err = command_error("horizon.command{usage = \"<x:word>\"}");
        assert!(err.contains("first word is an argument"), "got {err}");
        let err = command_error("horizon.command{usage = \"Bad\"}");
        assert!(err.contains("must use only a-z"), "got {err}");
        let err = command_error("horizon.command{}");
        assert!(err.contains("needs a field 'usage'"), "got {err}");
        let err = command_error("horizon.command{usage = \"x\", op = 1}");
        assert!(
            err.contains("has no field 'op'; its fields are usage and level"),
            "got {err}"
        );
        let err = command_error("horizon.command \"x\"");
        assert!(err.contains("needs its fields as a table"), "got {err}");
    }

    #[test]
    fn a_duplicate_command_usage_is_refused_and_at_most_64_commands() {
        let err = command_error(
            "horizon.command{usage = \"x <a:word>\"}\nhorizon.command{usage = \"x <a:word>\"}",
        );
        assert!(err.contains("declared twice"), "got {err}");
        let mut source = String::new();
        for at in 0..=MAX_COMMANDS {
            source.push_str(&format!("horizon.command{{usage = \"c{at}\"}}\n"));
        }
        let err = command_error(&source);
        assert!(err.contains("at most 64 commands"), "got {err}");
        let at_cap = &source[..source.rfind("horizon.command").unwrap()];
        assert_eq!(
            declare_in_fresh_state(at_cap).unwrap().commands.len(),
            MAX_COMMANDS
        );
    }

    #[test]
    fn a_command_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"tick\", function() horizon.command{usage = \"late\"} end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.commands.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn the_register_commands_hook_cannot_be_subscribed_to() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"register_commands\", function() end)";
        let err = declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &|name| {
            crate::generated::hooks::is_emitted(name)
        })
        .unwrap_err();
        assert!(
            err.contains("unknown hook 'register_commands'"),
            "got {err}"
        );
    }

    #[test]
    fn a_command_becomes_a_game_command_at_its_level() {
        let record = CommandRecord {
            usage: "ban <who:word>".to_owned(),
            level: 2,
        };
        let command = record.to_command();
        assert_eq!(command.usage, "ban <who:word>");
        assert_eq!(command.level, 2);
    }

    fn client_command_error(source: &str) -> String {
        declare_in_fresh_state(source).unwrap_err()
    }

    #[test]
    fn client_commands_are_recorded_in_declaration_order() {
        let source = "\
            horizon.client_command{usage = \"hello <who:word>\"}\n\
            horizon.client_command{usage = \"mode-2 [level:integer]\"}\n";
        let declaration = declare_in_fresh_state(source).unwrap();
        let usages: Vec<&str> = declaration
            .client_commands
            .iter()
            .map(|c| c.usage.as_str())
            .collect();
        assert_eq!(usages, ["hello <who:word>", "mode-2 [level:integer]"]);
        assert!(declaration.hooks.is_empty());
    }

    #[test]
    fn a_client_command_becomes_a_level_zero_game_command_with_its_usage() {
        let record = ClientCommandRecord {
            usage: "hello <who:word>".to_owned(),
        };
        let command = record.to_command();
        assert_eq!(command.usage, "hello <who:word>");
        assert_eq!(command.level, 0);
    }

    #[test]
    fn a_command_is_named_by_the_first_word_of_its_usage() {
        assert_eq!(command_name("alpha <n:integer>"), "alpha");
        assert_eq!(command_name("bravo"), "bravo");
        assert_eq!(command_name("demo run [n:integer]"), "demo");
        assert_eq!(command_name("mode-2  x"), "mode-2");
    }

    #[test]
    fn a_usage_that_starts_with_an_argument_is_never_declared() {
        // The first word of a declared usage is a literal, so its name can match a command word.
        for usage in [
            "<n:integer>",
            "[n:integer] x",
            "<n:integer> x",
            "Demo",
            "/demo",
            " demo",
        ] {
            assert!(check_usage(usage).is_err(), "{usage} should be refused");
        }
    }

    #[test]
    fn a_usage_that_passes_the_shape_checks_is_accepted() {
        for usage in ["x", "x <mode:word>", "a-b_c9 [n:integer] literal"] {
            assert!(check_usage(usage).is_ok(), "{usage} should pass");
        }
        let longest = format!("x {}", "a".repeat(MAX_USAGE - 2));
        assert_eq!(longest.chars().count(), MAX_USAGE);
        assert!(check_usage(&longest).is_ok());
        let name = "n".repeat(MAX_COMMAND_NAME);
        assert!(check_usage(&name).is_ok());
    }

    #[test]
    fn a_usage_with_a_bad_shape_refuses_the_mod() {
        let cases = [
            ("", "usage is empty"),
            (" x", "first word is empty"),
            ("<x:word>", "first word is an argument"),
            ("[x:word]", "first word is an argument"),
            ("Hello", "must use only a-z"),
            ("he.llo", "must use only a-z"),
            ("x\ny", "control character"),
            ("x\ty", "control character"),
            ("x\u{7f}", "control character"),
        ];
        for (usage, reason) in cases {
            let err = client_command_error(&format!("horizon.client_command{{usage = {usage:?}}}"));
            assert!(err.contains(reason), "{usage:?}: got {err}");
        }
        let long = "n".repeat(MAX_COMMAND_NAME + 1);
        let err = client_command_error(&format!("horizon.client_command{{usage = \"{long}\"}}"));
        assert!(err.contains("first word is longer than 32"), "got {err}");
        let too_long = format!("x {}", "a".repeat(MAX_USAGE));
        let err =
            client_command_error(&format!("horizon.client_command{{usage = \"{too_long}\"}}"));
        assert!(err.contains("longer than 256 characters"), "got {err}");
    }

    #[test]
    fn a_client_command_with_a_level_key_or_another_field_is_an_error() {
        let err = client_command_error("horizon.client_command{usage = \"x\", level = 0}");
        assert!(err.contains("has no field 'level'"), "got {err}");
        let err = client_command_error("horizon.client_command{}");
        assert!(err.contains("needs a field 'usage'"), "got {err}");
        let err = client_command_error("horizon.client_command{usage = 3}");
        assert!(err.contains("needs a string"), "got {err}");
        let err = client_command_error("horizon.client_command \"x\"");
        assert!(err.contains("needs its fields as a table"), "got {err}");
    }

    #[test]
    fn a_duplicate_usage_is_an_error_and_a_limit_of_64_holds() {
        let err = client_command_error(
            "horizon.client_command{usage = \"x <a:word>\"}\n\
             horizon.client_command{usage = \"x <a:word>\"}",
        );
        assert!(err.contains("declared twice"), "got {err}");
        let mut source = String::new();
        for at in 0..=MAX_CLIENT_COMMANDS {
            source.push_str(&format!("horizon.client_command{{usage = \"c{at}\"}}\n"));
        }
        let err = client_command_error(&source);
        assert!(err.contains("at most 64 client commands"), "got {err}");
        let mut source = String::new();
        for at in 0..MAX_CLIENT_COMMANDS {
            source.push_str(&format!("horizon.client_command{{usage = \"c{at}\"}}\n"));
        }
        assert_eq!(
            declare_in_fresh_state(&source)
                .unwrap()
                .client_commands
                .len(),
            MAX_CLIENT_COMMANDS
        );
    }

    #[test]
    fn a_client_command_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source =
            "horizon.on(\"tick\", function() horizon.client_command{usage = \"late\"} end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.client_commands.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn the_client_commands_hook_cannot_be_subscribed_to() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"register_client_commands\", function() end)";
        let err = declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &|name| {
            crate::generated::hooks::is_emitted(name)
        })
        .unwrap_err();
        assert!(
            err.contains("unknown hook 'register_client_commands'"),
            "got {err}"
        );
    }

    fn config_error(source: &str) -> String {
        declare_in_fresh_state(source).unwrap_err()
    }

    #[test]
    fn a_config_gets_the_game_default_file_and_the_host_owner() {
        let declaration = declare_in_fresh_state(
            "horizon.config{kind = \"common\", default = \"answer = 42\\n\"}\n\
             horizon.config{kind = \"server\", default = \"\", file = \"mymod/machines.toml\"}\n",
        )
        .unwrap();
        let files: Vec<(&str, &str, mod_api::ConfigKind)> = declaration
            .configs
            .iter()
            .map(|c| (c.mod_id.as_str(), c.file.as_str(), c.kind))
            .collect();
        assert_eq!(
            files,
            [
                ("mymod", "mymod-common.toml", mod_api::ConfigKind::Common),
                ("mymod", "mymod/machines.toml", mod_api::ConfigKind::Server),
            ]
        );
        assert_eq!(declaration.configs[0].default, "answer = 42\n");
    }

    #[test]
    fn a_config_file_must_belong_to_the_mod_and_be_a_checked_toml_path() {
        let cases = [
            ("mymod.toml", None),
            ("mymod-extra.toml", None),
            ("mymod/a/b.toml", None),
            ("mymod/x_1-2.toml", None),
            ("../mymod.toml", Some("empty folder name")),
            ("mymod/../x.toml", Some("empty folder name")),
            ("/mymod.toml", Some("empty folder name")),
            ("mymod//x.toml", Some("empty folder name")),
            ("mymod\\x.toml", Some("must use only")),
            ("mymod:x.toml", Some("must use only")),
            ("MyMod-x.toml", Some("must use only")),
            ("mymod-x.json", Some("must end with .toml")),
            ("other-x.toml", Some("is not this mod's")),
            ("mymodx/y.toml", Some("is not this mod's")),
            ("mymod-x/y.toml", Some("is not this mod's")),
            ("", Some("the file is empty")),
        ];
        for (file, reason) in cases {
            let source =
                format!("horizon.config{{kind = \"common\", default = \"\", file = {file:?}}}");
            match reason {
                None => {
                    assert!(
                        declare_in_fresh_state(&source).is_ok(),
                        "{file} should pass"
                    );
                }
                Some(reason) => {
                    let err = config_error(&source);
                    assert!(err.contains(reason), "{file:?}: got {err}");
                }
            }
        }
        let long = format!("mymod/{}.toml", "a".repeat(MAX_CONFIG_FILE));
        assert!(
            config_error(&format!(
                "horizon.config{{kind = \"common\", default = \"\", file = \"{long}\"}}"
            ))
            .contains("longer than 128")
        );
    }

    #[test]
    fn a_bad_config_refuses_the_mod_with_the_reason() {
        let cases = [
            ("kind = \"both\", default = \"\"", "must be \"client\""),
            ("default = \"\"", "needs a field 'kind'"),
            ("kind = \"common\"", "needs a field 'default'"),
            (
                "kind = \"common\", default = 3",
                "field 'default' needs a string",
            ),
            ("kind = 1, default = \"\"", "field 'kind' needs a string"),
            ("kind = \"common\", default = \"a\\0b\"", "has a NUL byte"),
            (
                "kind = \"common\", default = \"\", mod_id = \"other\"",
                "has no field 'mod_id'",
            ),
            (
                "kind = \"common\", default = \"\", level = 0",
                "has no field 'level'",
            ),
        ];
        for (fields, reason) in cases {
            let err = config_error(&format!("horizon.config{{{fields}}}"));
            assert!(err.contains(reason), "{fields}: got {err}");
        }
        let big = "x".repeat(MAX_CONFIG_DEFAULT + 1);
        let err = config_error(&format!(
            "horizon.config{{kind = \"common\", default = \"{big}\"}}"
        ));
        assert!(err.contains("larger than 65536 bytes"), "got {err}");
        let edge = "x".repeat(MAX_CONFIG_DEFAULT);
        assert!(
            declare_in_fresh_state(&format!(
                "horizon.config{{kind = \"common\", default = \"{edge}\"}}"
            ))
            .is_ok()
        );
        assert!(config_error("horizon.config \"x\"").contains("needs its fields as a table"));
    }

    #[test]
    fn at_most_eight_configs_and_no_file_twice() {
        let mut source = String::new();
        for at in 0..=MAX_CONFIGS {
            source.push_str(&format!(
                "horizon.config{{kind = \"common\", default = \"\", file = \"mymod/f{at}.toml\"}}\n"
            ));
        }
        assert!(config_error(&source).contains("at most 8 configs"));
        let twice = "horizon.config{kind = \"common\", default = \"\"}\n\
                     horizon.config{kind = \"common\", default = \"\", file = \"mymod-common.toml\"}";
        assert!(config_error(twice).contains("declared twice"));
    }

    #[test]
    fn a_config_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"tick\", function() horizon.config{kind = \"common\", default = \"\"} end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.configs.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn the_register_configs_hook_cannot_be_subscribed_to() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"register_configs\", function() end)";
        let err = declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &|name| {
            crate::generated::hooks::is_emitted(name)
        })
        .unwrap_err();
        assert!(err.contains("unknown hook 'register_configs'"), "got {err}");
    }

    #[test]
    fn an_unknown_field_is_an_error_naming_it_and_the_line() {
        let err = block_error(
            "horizon.block{id = \"mymod:lamp\", template = \"minecraft:stone\"}\n\
             horizon.block{id = \"mymod:x\", template = \"minecraft:stone\", colour = 1}",
        );
        assert!(err.contains("line 2"), "got {err}");
        assert!(err.contains("no field 'colour'"), "got {err}");
    }

    #[test]
    fn an_id_without_a_namespace_is_refused() {
        let err = block_error("horizon.block{id = \"lamp\", template = \"minecraft:stone\"}");
        assert!(err.contains("namespaced id"), "got {err}");
        assert!(err.contains("field 'id'"), "got {err}");
    }

    #[test]
    fn an_id_in_another_namespace_is_refused() {
        let err =
            block_error("horizon.item{id = \"minecraft:wand\", template = \"minecraft:stick\"}");
        assert!(
            err.contains("outside this mod's namespace 'mymod'"),
            "got {err}"
        );
        let err = block_error("horizon.block{id = \"other:lamp\", template = \"minecraft:stone\"}");
        assert!(
            err.contains("outside this mod's namespace 'mymod'"),
            "got {err}"
        );
    }

    #[test]
    fn an_id_with_a_bad_path_is_refused() {
        let err = block_error("horizon.block{id = \"mymod:Lamp\", template = \"minecraft:stone\"}");
        assert!(err.contains("invalid path"), "got {err}");
    }

    #[test]
    fn a_template_must_be_a_vanilla_id() {
        let err = block_error("horizon.block{id = \"mymod:lamp\", template = \"stone\"}");
        assert!(err.contains("field 'template'"), "got {err}");
        let err = block_error("horizon.block{id = \"mymod:lamp\", template = \"other:stone\"}");
        assert!(err.contains("field 'template'"), "got {err}");
    }

    #[test]
    fn a_missing_field_is_refused() {
        let err = block_error("horizon.block{id = \"mymod:lamp\"}");
        assert!(err.contains("needs a field 'template'"), "got {err}");
        let err = block_error("horizon.item{template = \"minecraft:stick\"}");
        assert!(err.contains("needs a field 'id'"), "got {err}");
    }

    #[test]
    fn a_duplicate_id_in_one_mod_is_refused() {
        let err = block_error(
            "horizon.block{id = \"mymod:lamp\", template = \"minecraft:stone\"}\n\
             horizon.block{id = \"mymod:lamp\", template = \"minecraft:stone\"}",
        );
        assert!(err.contains("'mymod:lamp' is declared twice"), "got {err}");
        let err = block_error(
            "horizon.item{id = \"mymod:wand\", template = \"minecraft:stick\"}\n\
             horizon.item{id = \"mymod:wand\", template = \"minecraft:stick\"}",
        );
        assert!(err.contains("'mymod:wand' is declared twice"), "got {err}");
    }

    #[test]
    fn two_hundred_fifty_six_blocks_are_fine_and_the_257th_is_refused() {
        let line = |i: usize| {
            format!("horizon.block{{id = \"mymod:b{i}\", template = \"minecraft:stone\"}}\n")
        };
        let ok: String = (0..MAX_BLOCKS).map(line).collect();
        assert_eq!(
            declare_in_fresh_state(&ok).unwrap().blocks.len(),
            MAX_BLOCKS
        );
        let too_many: String = (0..=MAX_BLOCKS).map(line).collect();
        let err = declare_in_fresh_state(&too_many).unwrap_err();
        assert!(err.contains("at most 256 blocks"), "got {err}");
    }

    #[test]
    fn items_have_their_own_cap() {
        let line = |i: usize| {
            format!("horizon.item{{id = \"mymod:i{i}\", template = \"minecraft:stick\"}}\n")
        };
        let too_many: String = (0..=MAX_ITEMS).map(line).collect();
        let err = declare_in_fresh_state(&too_many).unwrap_err();
        assert!(err.contains("at most 256 items"), "got {err}");
    }

    /// The declared parts of `source`, or the load error.
    fn parts_of(source: &str) -> Result<Vec<PartRecord>, String> {
        declare_in_fresh_state(source).map(|declaration| declaration.parts)
    }

    #[test]
    fn a_block_part_records_its_block_and_sorted_handler_names() {
        let parts = parts_of(
            "horizon.block_part(\"minecraft:stone\", {can_survive = function() return nil end, may_place_on = function() end})",
        )
        .unwrap();
        assert_eq!(
            parts,
            [PartRecord {
                id: "minecraft:stone".to_owned(),
                hooks: vec!["can_survive".to_owned(), "may_place_on".to_owned()],
            }]
        );
    }

    #[test]
    fn a_block_part_may_attach_to_any_namespace_but_needs_one() {
        assert!(
            parts_of("horizon.block_part(\"other_mod:lamp\", {can_survive = function() end})")
                .is_ok()
        );
        let err =
            parts_of("horizon.block_part(\"lamp\", {can_survive = function() end})").unwrap_err();
        assert!(err.contains("namespaced block id"), "got {err}");
    }

    #[test]
    fn a_block_part_handler_must_be_an_emitted_block_part_method() {
        let unknown = parts_of("horizon.block_part(\"mymod:x\", {server_tick = function() end})")
            .unwrap_err();
        assert!(
            unknown.contains("not a block part handler"),
            "got {unknown}"
        );
        let hot =
            parts_of("horizon.block_part(\"mymod:x\", {block_render_visibility = function() end})")
                .unwrap_err();
        assert!(hot.contains("hot path (needs measurement)"), "got {hot}");
        let not_function =
            parts_of("horizon.block_part(\"mymod:x\", {can_survive = 1})").unwrap_err();
        assert!(
            not_function.contains("needs a function"),
            "got {not_function}"
        );
    }

    #[test]
    fn a_second_block_part_for_one_block_is_refused() {
        let err = parts_of(
            "horizon.block_part(\"mymod:x\", {can_survive = function() end})\n\
             horizon.block_part(\"mymod:x\", {may_place_on = function() end})",
        )
        .unwrap_err();
        assert!(err.contains("two parts in this mod"), "got {err}");
    }

    #[test]
    fn a_mod_may_declare_at_most_128_block_parts() {
        let source = |count: usize| -> String {
            (0..count)
                .map(|i| {
                    format!(
                        "horizon.block_part(\"mymod:p{i}\", {{can_survive = function() end}})\n"
                    )
                })
                .collect()
        };
        assert_eq!(parts_of(&source(MAX_PARTS)).unwrap().len(), MAX_PARTS);
        let err = parts_of(&source(MAX_PARTS + 1)).unwrap_err();
        assert!(err.contains("at most 128 block parts"), "got {err}");
    }

    #[test]
    fn a_block_part_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"tick\", function() horizon.block_part(\"mymod:x\", {can_survive = function() end}) end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.parts.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    /// The declared item parts of `source`, or the load error.
    fn item_parts_of(source: &str) -> Result<Vec<PartRecord>, String> {
        declare_in_fresh_state(source).map(|declaration| declaration.item_parts)
    }

    #[test]
    fn an_item_part_records_its_item_and_sorted_handler_names() {
        let declaration = declare_in_fresh_state(
            "horizon.item_part(\"minecraft:stick\", {use_item = function() end, hurt_enemy = function() end})",
        )
        .unwrap();
        assert!(declaration.parts.is_empty());
        assert_eq!(
            declaration.item_parts,
            [PartRecord {
                id: "minecraft:stick".to_owned(),
                hooks: vec!["hurt_enemy".to_owned(), "use_item".to_owned()],
            }]
        );
    }

    #[test]
    fn an_item_part_needs_a_namespaced_item_id() {
        assert!(
            item_parts_of("horizon.item_part(\"other_mod:wand\", {use_item = function() end})")
                .is_ok()
        );
        let err =
            item_parts_of("horizon.item_part(\"wand\", {use_item = function() end})").unwrap_err();
        assert!(err.contains("namespaced item id"), "got {err}");
    }

    #[test]
    fn an_item_part_handler_must_be_an_emitted_item_part_method() {
        let block_only =
            item_parts_of("horizon.item_part(\"mymod:x\", {can_survive = function() end})")
                .unwrap_err();
        assert!(
            block_only.contains("not an item part handler"),
            "got {block_only}"
        );
        let hot = item_parts_of("horizon.item_part(\"mymod:x\", {tooltip = function() end})")
            .unwrap_err();
        assert!(hot.contains("hot path (needs measurement)"), "got {hot}");
        let not_function =
            item_parts_of("horizon.item_part(\"mymod:x\", {use_item = 1})").unwrap_err();
        assert!(
            not_function.contains("needs a function"),
            "got {not_function}"
        );
        let empty = item_parts_of("horizon.item_part(\"mymod:x\", {})").unwrap_err();
        assert!(empty.contains("needs at least one handler"), "got {empty}");
    }

    #[test]
    fn a_second_item_part_for_one_item_is_refused_but_a_block_part_of_its_id_is_not() {
        let err = item_parts_of(
            "horizon.item_part(\"mymod:x\", {use_item = function() end})\n\
             horizon.item_part(\"mymod:x\", {hurt_enemy = function() end})",
        )
        .unwrap_err();
        assert!(err.contains("two parts in this mod"), "got {err}");
        let both = declare_in_fresh_state(
            "horizon.block_part(\"mymod:x\", {can_survive = function() end})\n\
             horizon.item_part(\"mymod:x\", {use_item = function() end})",
        )
        .unwrap();
        assert_eq!(both.parts.len(), 1);
        assert_eq!(both.item_parts.len(), 1);
    }

    #[test]
    fn a_mod_may_declare_at_most_128_item_parts() {
        let source = |count: usize| -> String {
            (0..count)
                .map(|i| {
                    format!("horizon.item_part(\"mymod:p{i}\", {{use_item = function() end}})\n")
                })
                .collect()
        };
        assert_eq!(item_parts_of(&source(MAX_PARTS)).unwrap().len(), MAX_PARTS);
        let err = item_parts_of(&source(MAX_PARTS + 1)).unwrap_err();
        assert!(err.contains("at most 128 item parts"), "got {err}");
    }

    #[test]
    fn an_item_part_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"tick\", function() horizon.item_part(\"mymod:x\", {use_item = function() end}) end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.item_parts.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    /// The block entity types of `source`, or the load error.
    fn block_entity_types_of(source: &str) -> Result<Vec<BlockEntityTypeRecord>, String> {
        declare_in_fresh_state(source).map(|declaration| declaration.block_entity_types)
    }

    /// One block of this mod, which a block entity type of the tests names.
    const PRESS_BLOCK: &str =
        "horizon.block{id = \"mymod:press_block\", template = \"minecraft:stone\"}\n";

    #[test]
    fn a_block_entity_type_and_its_part_are_recorded() {
        let declaration = declare_in_fresh_state(&format!(
            "{PRESS_BLOCK}\
             horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}\n\
             horizon.block_entity_part(\"mymod:press\", {{on_chunk_unloaded = function() end, block_entity_data_received = function() end}})"
        ))
        .unwrap();
        assert_eq!(
            declaration.block_entity_types,
            [BlockEntityTypeRecord {
                id: "mymod:press".to_owned(),
                blocks: vec!["mymod:press_block".to_owned()],
            }]
        );
        assert_eq!(
            declaration.block_entity_parts,
            [PartRecord {
                id: "mymod:press".to_owned(),
                hooks: vec![
                    "block_entity_data_received".to_owned(),
                    "on_chunk_unloaded".to_owned()
                ],
            }]
        );
        assert!(declaration.parts.is_empty() && declaration.item_parts.is_empty());
    }

    #[test]
    fn a_block_part_an_item_part_and_a_block_entity_part_of_one_mod_are_all_recorded() {
        let declaration = declare_in_fresh_state(&format!(
            "{PRESS_BLOCK}\
             horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}\n\
             horizon.block_part(\"mymod:press_block\", {{can_survive = function() end}})\n\
             horizon.item_part(\"mymod:press_block\", {{use_item = function() end}})\n\
             horizon.block_entity_part(\"mymod:press\", {{on_chunk_unloaded = function() end}})"
        ))
        .unwrap();
        assert_eq!(declaration.parts.len(), 1);
        assert_eq!(declaration.item_parts.len(), 1);
        assert_eq!(declaration.block_entity_parts.len(), 1);
    }

    #[test]
    fn a_type_keeps_its_blocks_in_the_order_given() {
        let types = block_entity_types_of(
            "horizon.block{id = \"mymod:b\", template = \"minecraft:stone\"}\n\
             horizon.block{id = \"mymod:a\", template = \"minecraft:stone\"}\n\
             horizon.block_entity_type{id = \"mymod:press\", blocks = {\"mymod:b\", \"mymod:a\"}}",
        )
        .unwrap();
        assert_eq!(types[0].blocks, ["mymod:b", "mymod:a"]);
    }

    #[test]
    fn a_block_entity_type_needs_an_id_in_the_mods_namespace() {
        let err = block_entity_types_of(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"other:press\", blocks = {{\"mymod:press_block\"}}}}"
        ))
        .unwrap_err();
        assert!(err.contains("outside this mod's namespace"), "got {err}");
        let bare = block_entity_types_of(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"press\", blocks = {{\"mymod:press_block\"}}}}"
        ))
        .unwrap_err();
        assert!(bare.contains("needs a namespaced id"), "got {bare}");
    }

    #[test]
    fn a_block_entity_type_names_only_its_mods_blocks_and_each_must_be_declared_here() {
        let foreign = block_entity_types_of(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"minecraft:furnace\"}}}}"
        ))
        .unwrap_err();
        assert!(
            foreign.contains("outside this mod's namespace"),
            "got {foreign}"
        );
        let undeclared = block_entity_types_of(
            "horizon.block_entity_type{id = \"mymod:press\", blocks = {\"mymod:ghost\"}}",
        )
        .unwrap_err();
        assert!(
            undeclared.contains("is not declared by this mod with horizon.block"),
            "got {undeclared}"
        );
        let empty = block_entity_types_of(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"mymod:press\", blocks = {{}}}}"
        ))
        .unwrap_err();
        assert!(empty.contains("needs at least one block"), "got {empty}");
    }

    #[test]
    fn a_block_entity_type_has_only_id_and_blocks_and_both_are_needed() {
        let extra = block_entity_types_of(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}, colour = 1}}"
        ))
        .unwrap_err();
        assert!(extra.contains("has no field 'colour'"), "got {extra}");
        let no_blocks =
            block_entity_types_of("horizon.block_entity_type{id = \"mymod:press\"}").unwrap_err();
        assert!(
            no_blocks.contains("needs a field 'blocks'"),
            "got {no_blocks}"
        );
        let not_a_list = block_entity_types_of(
            "horizon.block_entity_type{id = \"mymod:press\", blocks = {x = \"mymod:press_block\"}}",
        )
        .unwrap_err();
        assert!(
            not_a_list.contains("needs a list of block id strings"),
            "got {not_a_list}"
        );
    }

    #[test]
    fn a_block_is_in_at_most_one_block_entity_type_of_a_mod() {
        let twice_in_one = block_entity_types_of(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\", \"mymod:press_block\"}}}}"
        ))
        .unwrap_err();
        assert!(
            twice_in_one.contains("in two block entity types"),
            "got {twice_in_one}"
        );
        let two_types = block_entity_types_of(&format!(
            "{PRESS_BLOCK}\
             horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}\n\
             horizon.block_entity_type{{id = \"mymod:oven\", blocks = {{\"mymod:press_block\"}}}}"
        ))
        .unwrap_err();
        assert!(
            two_types.contains("in two block entity types"),
            "got {two_types}"
        );
        let twice_id = block_entity_types_of(&format!(
            "{PRESS_BLOCK}\
             horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}\n\
             horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}"
        ))
        .unwrap_err();
        assert!(twice_id.contains("declared twice"), "got {twice_id}");
    }

    #[test]
    fn a_block_entity_type_names_at_most_32_blocks_and_a_mod_declares_at_most_64_types() {
        let blocks = |count: usize| -> String {
            (0..count)
                .map(|i| format!("\"mymod:b{i}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let too_many_blocks = block_entity_types_of(&format!(
            "horizon.block_entity_type{{id = \"mymod:press\", blocks = {{{}}}}}",
            blocks(MAX_BLOCKS_PER_TYPE + 1)
        ))
        .unwrap_err();
        assert!(
            too_many_blocks.contains("at most 32 blocks"),
            "got {too_many_blocks}"
        );
        let source = |count: usize| -> String {
            (0..count)
                .map(|i| format!("horizon.block_entity_type{{id = \"mymod:t{i}\", blocks = {{\"mymod:b{i}\"}}}}\n"))
                .collect()
        };
        let err = block_entity_types_of(&source(MAX_BLOCK_ENTITY_TYPES + 1)).unwrap_err();
        assert!(err.contains("at most 64 block entity types"), "got {err}");
    }

    #[test]
    fn a_block_entity_part_needs_its_type_to_be_declared_by_this_mod() {
        let undeclared = declare_in_fresh_state(
            "horizon.block_entity_part(\"mymod:press\", {on_chunk_unloaded = function() end})",
        )
        .unwrap_err();
        assert!(
            undeclared.contains("needs a horizon.block_entity_type of this mod"),
            "got {undeclared}"
        );
        let foreign = declare_in_fresh_state(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}\n\
             horizon.block_entity_part(\"other:press\", {{on_chunk_unloaded = function() end}})"
        ))
        .unwrap_err();
        assert!(
            foreign.contains("needs a horizon.block_entity_type of this mod"),
            "got {foreign}"
        );
    }

    #[test]
    fn a_block_entity_part_handler_must_be_an_emitted_block_entity_method() {
        let typed = format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}\n"
        );
        let part = |handler: &str| {
            format!("{typed}horizon.block_entity_part(\"mymod:press\", {{{handler}}})")
        };
        let block_only = declare_in_fresh_state(&part("can_survive = function() end")).unwrap_err();
        assert!(
            block_only.contains("not a block entity part handler"),
            "got {block_only}"
        );
        let hot = declare_in_fresh_state(&part("block_entity_tick = function() end")).unwrap_err();
        assert!(hot.contains("hot path (needs measurement)"), "got {hot}");
        let nbt = declare_in_fresh_state(&part("block_entity = function() end")).unwrap_err();
        assert!(nbt.contains("NBT output"), "got {nbt}");
        let not_function = declare_in_fresh_state(&part("on_chunk_unloaded = 1")).unwrap_err();
        assert!(
            not_function.contains("needs a function"),
            "got {not_function}"
        );
        let empty = declare_in_fresh_state(&part("")).unwrap_err();
        assert!(empty.contains("needs at least one handler"), "got {empty}");
    }

    #[test]
    fn a_second_block_entity_part_for_one_type_is_refused() {
        let err = declare_in_fresh_state(&format!(
            "{PRESS_BLOCK}horizon.block_entity_type{{id = \"mymod:press\", blocks = {{\"mymod:press_block\"}}}}\n\
             horizon.block_entity_part(\"mymod:press\", {{on_chunk_unloaded = function() end}})\n\
             horizon.block_entity_part(\"mymod:press\", {{block_entity_data_received = function() end}})"
        ))
        .unwrap_err();
        assert!(err.contains("two parts in this mod"), "got {err}");
    }

    #[test]
    fn a_block_entity_declaration_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"tick\", function() horizon.block_entity_type{id = \"mymod:press\", blocks = {\"mymod:x\"}} end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.block_entity_types.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn a_declaration_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source = "horizon.on(\"tick\", function() horizon.block{id = \"mymod:late\", template = \"minecraft:stone\"} end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.blocks.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn the_declaring_functions_cannot_be_kept_and_called_later() {
        let lua = new_state().unwrap();
        let source = "kept = horizon.block\n\
                      horizon.on(\"tick\", function() kept{id = \"mymod:late\", template = \"minecraft:stone\"} end)";
        declare(&lua, "mymod", source, "=mod", &|n| n == "tick").unwrap();
        let tick = handlers(&lua, "tick").unwrap();
        assert!(tick[0].call::<()>(()).is_err());
    }

    #[test]
    fn a_hook_is_not_a_block_declaration() {
        // `blocks` is answered by the host, not by a handler: a mod cannot subscribe to it.
        let lua = new_state().unwrap();
        let err = declare(
            &lua,
            "mymod",
            "horizon.on(\"blocks\", function() end)",
            "=mod",
            &|name| crate::generated::hooks::is_emitted(name),
        )
        .unwrap_err();
        assert!(err.contains("unknown hook 'blocks'"), "got {err}");
    }

    /// A world fluid of `mymod` with `drop_off` set (the other settings default).
    const OIL: &str = "horizon.world_fluid{id = \"mymod:oil\", drop_off = 3}\n";

    #[test]
    fn a_world_fluid_and_its_fluid_part_are_recorded_with_the_settings_given() {
        let declaration = declare_in_fresh_state(&format!(
            "{OIL}horizon.fluid_part(\"mymod:oil\", {{fluid_rules = function() end}})"
        ))
        .unwrap();
        assert_eq!(declaration.fluids.len(), 1);
        assert_eq!(declaration.fluids[0].id, "mymod:oil");
        assert_eq!(declaration.fluids[0].fluid.drop_off, 3);
        assert_eq!(declaration.fluids[0].fluid.tick_delay, 5);
        assert!(declaration.fluids[0].fluid.bucket);
        assert_eq!(
            declaration.fluid_parts,
            [PartRecord {
                id: "mymod:oil".to_owned(),
                hooks: vec!["fluid_rules".to_owned()],
            }]
        );
    }

    #[test]
    fn a_fluid_setting_outside_what_the_simulation_runs_refuses_the_mod_by_line() {
        let drop = declare_in_fresh_state("horizon.world_fluid{id = \"mymod:oil\", drop_off = 8}")
            .unwrap_err();
        assert!(
            drop.contains("line 1") && drop.contains("drop_off is 1 to 7"),
            "got {drop}"
        );
        let tick =
            declare_in_fresh_state("horizon.world_fluid{id = \"mymod:oil\", tick_delay = 0}")
                .unwrap_err();
        assert!(tick.contains("tick_delay is 1 or more"), "got {tick}");
        let slope = declare_in_fresh_state(
            "horizon.world_fluid{id = \"mymod:oil\", slope_find_distance = 5}",
        )
        .unwrap_err();
        assert!(
            slope.contains("slope_find_distance is 1 to 4"),
            "got {slope}"
        );
        let wrong_type =
            declare_in_fresh_state("horizon.world_fluid{id = \"mymod:oil\", bucket = 1}")
                .unwrap_err();
        assert!(wrong_type.contains("field 'bucket'"), "got {wrong_type}");
    }

    #[test]
    fn a_world_fluid_id_is_in_the_mods_namespace_and_declared_once() {
        let foreign =
            declare_in_fresh_state("horizon.world_fluid{id = \"other:oil\"}").unwrap_err();
        assert!(
            foreign.contains("outside this mod's namespace 'mymod'"),
            "got {foreign}"
        );
        let twice = declare_in_fresh_state(&format!("{OIL}{OIL}")).unwrap_err();
        assert!(
            twice.contains("fluid 'mymod:oil' is declared twice"),
            "got {twice}"
        );
        let unknown = declare_in_fresh_state("horizon.world_fluid{id = \"mymod:oil\", depth = 2}")
            .unwrap_err();
        assert!(unknown.contains("has no field 'depth'"), "got {unknown}");
    }

    #[test]
    fn at_most_eight_world_fluids_per_mod() {
        let eight: String = (0..MAX_WORLD_FLUIDS)
            .map(|i| format!("horizon.world_fluid{{id = \"mymod:f{i}\"}}\n"))
            .collect();
        assert_eq!(declare_in_fresh_state(&eight).unwrap().fluids.len(), 8);
        let nine = format!("{eight}horizon.world_fluid{{id = \"mymod:f8\"}}");
        let err = declare_in_fresh_state(&nine).unwrap_err();
        assert!(err.contains("at most 8 world fluids"), "got {err}");
    }

    #[test]
    fn a_fluid_part_needs_a_world_fluid_of_this_mod_and_a_fluid_handler() {
        let undeclared = declare_in_fresh_state(
            "horizon.fluid_part(\"mymod:oil\", {fluid_rules = function() end})",
        )
        .unwrap_err();
        assert!(
            undeclared.contains("needs a horizon.world_fluid of this mod"),
            "got {undeclared}"
        );
        let foreign = declare_in_fresh_state(&format!(
            "{OIL}horizon.fluid_part(\"other:oil\", {{fluid_rules = function() end}})"
        ))
        .unwrap_err();
        assert!(
            foreign.contains("needs a horizon.world_fluid of this mod"),
            "got {foreign}"
        );
        let handler = declare_in_fresh_state(&format!(
            "{OIL}horizon.fluid_part(\"mymod:oil\", {{can_survive = function() end}})"
        ))
        .unwrap_err();
        assert!(
            handler.contains("not a fluid part handler"),
            "got {handler}"
        );
    }

    #[test]
    fn a_world_fluid_after_loading_is_refused() {
        let lua = new_state().unwrap();
        let source =
            "horizon.on(\"tick\", function() horizon.world_fluid{id = \"mymod:late\"} end)";
        let declaration =
            declare(&lua, "mymod", source, "@lua-mods/test/main.luau", &is_known).unwrap();
        assert!(declaration.fluids.is_empty());
        let tick = handlers(&lua, "tick").unwrap();
        let err = tick[0].call::<()>(()).unwrap_err().to_string();
        assert!(
            err.contains("declarations are only allowed while the mod loads"),
            "got {err}"
        );
    }

    fn entity_parts_of(count: usize) -> String {
        (0..count)
            .map(|i| {
                format!(
                    "horizon.entity_part(\"minecraft:mob{i}\", {{is_shearable = function() end}})\n"
                )
            })
            .collect()
    }

    #[test]
    fn an_entity_part_records_its_type_and_its_handlers_for_any_namespace() {
        let declaration = declare_in_fresh_state(
            "horizon.entity_part(\"minecraft:sheep\", {is_shearable = function() end})\n\
             horizon.entity_part(\"mymod:robot\", {is_shearable = function() end})",
        )
        .unwrap();
        let ids: Vec<(&str, Vec<&str>)> = declaration
            .entity_parts
            .iter()
            .map(|part| {
                (
                    part.id.as_str(),
                    part.hooks.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            ids,
            [
                ("minecraft:sheep", vec!["is_shearable"]),
                ("mymod:robot", vec!["is_shearable"])
            ]
        );
    }

    #[test]
    fn an_entity_part_records_the_three_plain_handlers_it_binds() {
        let declaration = declare_in_fresh_state(
            "horizon.entity_part(\"minecraft:sheep\", {is_shearable = function() end, on_sheared = function() end, spawn_cluster_size = function() end})",
        )
        .unwrap();
        let hooks: Vec<&str> = declaration.entity_parts[0]
            .hooks
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(hooks, ["is_shearable", "on_sheared", "spawn_cluster_size"]);
    }

    #[test]
    fn a_menu_part_records_its_menu_and_its_one_handler_for_any_namespace() {
        let declaration = declare_in_fresh_state(
            "horizon.menu_part(\"other:press_menu\", {menu_still_valid = function() end})",
        )
        .unwrap();
        let parts: Vec<(&str, Vec<&str>)> = declaration
            .menu_parts
            .iter()
            .map(|part| {
                (
                    part.id.as_str(),
                    part.hooks.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(parts, [("other:press_menu", vec!["menu_still_valid"])]);
    }

    #[test]
    fn a_menu_part_takes_only_menu_still_valid_and_names_the_skipped_handlers() {
        let unknown = declare_in_fresh_state(
            "horizon.menu_part(\"other:press_menu\", {close_me = function() end})",
        )
        .unwrap_err();
        assert!(
            unknown.contains("'close_me'") && unknown.contains("not a menu part handler"),
            "got {unknown}"
        );
        for later in ["menu_slot_rules", "menu_transfer"] {
            let err = declare_in_fresh_state(&format!(
                "horizon.menu_part(\"other:press_menu\", {{{later} = function() end}})"
            ))
            .unwrap_err();
            assert!(
                err.contains(&format!("'{later}'"))
                    && err.contains("not available: hot path (needs measurement)"),
                "got {err}"
            );
        }
    }

    #[test]
    fn a_menu_part_is_capped_at_sixteen_and_has_one_per_menu_id() {
        let sixteen: String = (0..MAX_MENU_PARTS)
            .map(|i| {
                format!(
                    "horizon.menu_part(\"other:menu{i}\", {{menu_still_valid = function() end}})\n"
                )
            })
            .collect();
        assert_eq!(
            declare_in_fresh_state(&sixteen).unwrap().menu_parts.len(),
            MAX_MENU_PARTS
        );
        let seventeen = format!(
            "{sixteen}horizon.menu_part(\"other:menu16\", {{menu_still_valid = function() end}})"
        );
        let err = declare_in_fresh_state(&seventeen).unwrap_err();
        assert!(err.contains("at most 16 menu parts"), "got {err}");
        let twice = "horizon.menu_part(\"other:press_menu\", {menu_still_valid = function() end})\n\
                     horizon.menu_part(\"other:press_menu\", {menu_still_valid = function() end})";
        let err = declare_in_fresh_state(twice).unwrap_err();
        assert!(err.contains("has two parts in this mod"), "got {err}");
    }

    #[test]
    fn a_menu_part_needs_a_namespaced_menu_id() {
        let err = declare_in_fresh_state(
            "horizon.menu_part(\"press_menu\", {menu_still_valid = function() end})",
        )
        .unwrap_err();
        assert!(
            err.contains("needs a namespaced menu id such as mymod:press"),
            "got {err}"
        );
    }

    #[test]
    fn an_entity_part_takes_only_the_handlers_it_can_bind() {
        let unknown = declare_in_fresh_state(
            "horizon.entity_part(\"minecraft:sheep\", {shear_me = function() end})",
        )
        .unwrap_err();
        assert!(
            unknown.contains("'shear_me'") && unknown.contains("not an entity part handler"),
            "got {unknown}"
        );
        for later in ["register_goals", "general_entity_tick"] {
            let err = declare_in_fresh_state(&format!(
                "horizon.entity_part(\"minecraft:sheep\", {{{later} = function() end}})"
            ))
            .unwrap_err();
            assert!(
                err.contains(&format!("'{later}'")) && err.contains("not available"),
                "got {err}"
            );
        }
    }

    #[test]
    fn an_entity_part_id_is_namespaced_and_one_per_entity_type() {
        let bare = declare_in_fresh_state(
            "horizon.entity_part(\"sheep\", {is_shearable = function() end})",
        )
        .unwrap_err();
        assert!(bare.contains("needs a namespaced entity id"), "got {bare}");
        let twice = declare_in_fresh_state(
            "horizon.entity_part(\"minecraft:sheep\", {is_shearable = function() end})\n\
             horizon.entity_part(\"minecraft:sheep\", {is_shearable = function() end})",
        )
        .unwrap_err();
        assert!(
            twice.contains("entity 'minecraft:sheep' has two parts"),
            "got {twice}"
        );
    }

    #[test]
    fn at_most_sixteen_entity_parts_per_mod() {
        let sixteen = declare_in_fresh_state(&entity_parts_of(16)).unwrap();
        assert_eq!(sixteen.entity_parts.len(), 16);
        let err = declare_in_fresh_state(&format!(
            "{}{}",
            entity_parts_of(16),
            "horizon.entity_part(\"minecraft:extra\", {is_shearable = function() end})"
        ))
        .unwrap_err();
        assert!(err.contains("at most 16 entity parts"), "got {err}");
    }

    /// Declares a mod made of `scripts` (chunk name, source) in a fresh state.
    fn declare_scripts_in_fresh_state(scripts: &[(&str, &str)]) -> Result<Declaration, String> {
        let lua = new_state().unwrap();
        declare_scripts(&lua, "mymod", scripts, &is_known)
    }

    /// A global `burn(ms)` that keeps the state busy for `ms` milliseconds, for the budget tests.
    fn with_burn(lua: &Lua) {
        let burn = lua
            .create_function(|_, ms: u64| {
                let until = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < until {}
                Ok(())
            })
            .unwrap();
        lua.globals().set("burn", burn).unwrap();
    }

    #[test]
    fn scripts_are_one_mod_recorded_in_manifest_order() {
        let declaration = declare_scripts_in_fresh_state(&[
            (
                "@lua-mods/test/main.luau",
                "horizon.block{id = \"mymod:lamp\", template = \"minecraft:stone\"}\n",
            ),
            (
                "@lua-mods/test/second.luau",
                "horizon.on(\"tick\", function() end)\n",
            ),
        ])
        .unwrap();
        assert_eq!(declaration.hooks, ["tick"]);
        let blocks: Vec<&str> = declaration.blocks.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(blocks, ["mymod:lamp"]);
    }

    #[test]
    fn a_later_script_sees_the_globals_of_an_earlier_one() {
        // One environment: the first script's global is there for the second.
        let declaration = declare_scripts_in_fresh_state(&[
            ("@lua-mods/test/main.luau", "shared = \"yes\"\n"),
            (
                "@lua-mods/test/second.luau",
                "if shared ~= \"yes\" then error(\"not shared\") end\nhorizon.on(\"tick\", function() end)\n",
            ),
        ])
        .unwrap();
        assert_eq!(declaration.hooks, ["tick"]);
    }

    #[test]
    fn an_error_in_a_later_script_refuses_the_mod_and_names_the_script_and_line() {
        let err = declare_scripts_in_fresh_state(&[
            (
                "@lua-mods/test/main.luau",
                "horizon.on(\"tick\", function() end)\n",
            ),
            (
                "@lua-mods/test/second.luau",
                "horizon.on(\"tick\", function() end)\nerror(\"boom\")\n",
            ),
        ])
        .unwrap_err();
        assert!(err.contains("second.luau:2"), "got {err}");
    }

    #[test]
    fn sixteen_scripts_declare_one_mod() {
        let names: Vec<String> = (1..=16)
            .map(|i| format!("@lua-mods/test/s{i}.luau"))
            .collect();
        let sources: Vec<String> = (1..=16)
            .map(|i| format!("horizon.on(\"tick\", function() end)\nlocal _ = {i}\n"))
            .collect();
        let scripts: Vec<(&str, &str)> = names
            .iter()
            .zip(&sources)
            .map(|(name, source)| (name.as_str(), source.as_str()))
            .collect();
        let declaration = declare_scripts_in_fresh_state(&scripts).unwrap();
        assert_eq!(declaration.hooks, ["tick"]);
        assert_eq!(declaration.options.len(), 1);
    }

    #[test]
    fn the_time_budget_is_shared_by_all_scripts_of_a_mod() {
        // Script 1 burns 120 ms of a 200 ms budget and fits alone. Script 2 burns 120 ms more and then
        // reaches a loop back-edge (where Luau checks the interrupt) after 240 ms. A budget per script
        // would let it pass; the one budget of the mod refuses it.
        let lua = new_state().unwrap();
        with_burn(&lua);
        let first = "burn(120)\n";
        let second = "burn(120)\nfor i = 1, 2 do local _ = i end\n";
        let budget = Duration::from_millis(200);
        declare_scripts_with_budget(&lua, "mymod", &[("=a", first)], &is_known, budget).unwrap();
        let err = declare_scripts_with_budget(
            &lua,
            "mymod",
            &[("=a", first), ("=b", second)],
            &is_known,
            budget,
        )
        .unwrap_err();
        assert!(err.contains("time budget"), "got {err}");
    }

    #[test]
    fn the_caps_count_across_scripts() {
        // 200 handlers in one script and 100 in the next: 300 is over the 256 a mod may register.
        let many = |n: usize| "horizon.on(\"tick\", function() end)\n".repeat(n);
        let first = many(200);
        let second = many(100);
        let err = declare_scripts_in_fresh_state(&[("=a", &first), ("=b", &second)]).unwrap_err();
        assert!(err.contains("at most 256 handlers"), "got {err}");
        let blocks = |from: usize, n: usize| -> String {
            (from..from + n)
                .map(|i| {
                    format!(
                        "horizon.block{{id = \"mymod:b{i}\", template = \"minecraft:stone\"}}\n"
                    )
                })
                .collect()
        };
        let one = blocks(0, 200);
        let two = blocks(200, 100);
        let err = declare_scripts_in_fresh_state(&[("=a", &one), ("=b", &two)]).unwrap_err();
        assert!(err.contains("at most 256 blocks"), "got {err}");
    }

    #[test]
    fn a_block_id_declared_in_two_scripts_is_refused() {
        let lamp = "horizon.block{id = \"mymod:lamp\", template = \"minecraft:stone\"}\n";
        let err = declare_scripts_in_fresh_state(&[("=a", lamp), ("=b", lamp)]).unwrap_err();
        assert!(err.contains("declared twice"), "got {err}");
    }
}
