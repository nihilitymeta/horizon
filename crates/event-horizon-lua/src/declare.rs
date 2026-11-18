//! Declaring mode: a mod's `main.luau` runs once, at start-up, to learn which hooks it subscribes to.
//!
//! The game reads the hooks a mod listens to before any world or host exists, so the chunk runs
//! now, with one function in reach, `horizon.on(hook, handler)`. The handlers it registers stay in
//! the state for later. The chunk is untrusted code run before the game starts, so it has a time
//! budget, cannot touch anything but `horizon.on`, and can register at most `MAX_HANDLERS`.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use mlua::{Error, Function, Lua, Table, Value};

use crate::budget;

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

/// The most blocks one `horizon.block_entity_type` may name.
pub const MAX_BLOCKS_PER_TYPE: usize = 32;

/// The most `horizon.client_command` calls one mod may make.
pub const MAX_CLIENT_COMMANDS: usize = 64;

/// The longest usage `horizon.client_command` accepts, in characters.
pub const MAX_USAGE: usize = 256;

/// The longest first word (the command's name) of a client command's usage, in characters.
pub const MAX_COMMAND_NAME: usize = 32;

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
    /// The client commands `horizon.client_command` declared, in declaration order.
    pub client_commands: Vec<ClientCommandRecord>,
}

/// The kind of a part a mod declares: `horizon.block_part` (a BlockPart), `horizon.item_part` (an
/// ItemPart) or `horizon.block_entity_part` (a BlockEntityPart). Each kind has its own handler table,
/// its own id check and its own cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartKind {
    Block,
    Item,
    BlockEntity,
}

impl PartKind {
    /// The declaring function's name, as a mod calls it.
    pub fn keyword(self) -> &'static str {
        match self {
            PartKind::Block => "block_part",
            PartKind::Item => "item_part",
            PartKind::BlockEntity => "block_entity_part",
        }
    }

    /// What the id names, in the errors (`block`, `item`, `block entity`).
    fn noun(self) -> &'static str {
        match self {
            PartKind::Block => "block",
            PartKind::Item => "item",
            PartKind::BlockEntity => "block entity",
        }
    }

    /// The handler names of this kind the host binds, with the reason each is not bound.
    fn handlers(self) -> &'static [(&'static str, Option<&'static str>)] {
        match self {
            PartKind::Block => crate::generated::parts::BLOCK_PARTS,
            PartKind::Item => crate::generated::item_parts::ITEM_PARTS,
            PartKind::BlockEntity => crate::generated::block_entity_parts::BLOCK_ENTITY_PARTS,
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
    declare_with_budget(lua, owner, source, chunk_name, is_hook, DECLARE_BUDGET)
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
    let (recorded, loaded) =
        run(lua, owner, source, chunk_name, is_hook, budget).map_err(|e| e.to_string())?;
    let hooks = recorded
        .options
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    loaded.map_err(|e| e.to_string())?;
    check_block_entities(&recorded)?;
    Ok(Declaration {
        hooks,
        options: recorded.options,
        blocks: recorded.blocks,
        items: recorded.items,
        parts: recorded.parts,
        item_parts: recorded.item_parts,
        block_entity_types: recorded.block_entity_types,
        block_entity_parts: recorded.block_entity_parts,
        client_commands: recorded.client_commands,
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
    client_commands: Vec<ClientCommandRecord>,
}

/// Sets up the environment and the `horizon` table, runs the chunk, and then replaces the declaring
/// functions with ones that only refuse. Returns what the chunk declared, and the chunk's own
/// result: the setup errors come first, and the chunk's result is kept apart so the caller can
/// report it.
fn run(
    lua: &Lua,
    owner: &str,
    source: &str,
    chunk_name: &str,
    is_hook: &dyn Fn(&str) -> bool,
    budget_limit: Duration,
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
    let client_commands = RefCell::new(Vec::<ClientCommandRecord>::new());
    let calls = Cell::new(0_usize);
    let finished = Cell::new(false);

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
                declare_part(lua, PartKind::Block, id, spec, &parts, &handlers)
            })?;
            let item_part = scope.create_function(|lua, (id, spec): (Value, Value)| {
                declare_part(lua, PartKind::Item, id, spec, &item_parts, &handlers)
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
                )
            })?;
            let client_command = scope.create_function(|lua, value: Value| {
                declare_client_command(lua, value, &client_commands)
            })?;
            horizon.raw_set("on", on)?;
            horizon.raw_set("client_command", client_command)?;
            horizon.raw_set("block", block)?;
            horizon.raw_set("item", item)?;
            horizon.raw_set("block_part", block_part)?;
            horizon.raw_set("item_part", item_part)?;
            horizon.raw_set("block_entity_type", block_entity_type)?;
            horizon.raw_set("block_entity_part", block_entity_part)?;
            horizon.set_readonly(true);
            env.raw_set("horizon", horizon.clone())?;

            let result = budget::within_with(
                lua,
                budget_limit,
                "the mod ran over its loading time budget",
                || {
                    lua.load(source)
                        .set_name(chunk_name)
                        .set_environment(env.clone())
                        .exec()
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
        ("client_command", LATE_DECLARE),
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
        client_commands: client_commands.into_inner(),
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

/// Reads `horizon.client_command{usage = "..."}`: the one field `usage`, a string that passes
/// `check_usage`. The record is kept in `records`, at most `MAX_CLIENT_COMMANDS` and no usage twice.
fn declare_client_command(
    lua: &Lua,
    value: Value,
    records: &RefCell<Vec<ClientCommandRecord>>,
) -> mlua::Result<()> {
    let table = match value {
        Value::Table(table) => table,
        other => {
            return Err(at_line(
                lua,
                format!(
                    "horizon.client_command needs its fields as a table, not {}",
                    other.type_name()
                ),
            ));
        }
    };
    let mut usage = None;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let name = match &key {
            Value::String(name) => name.to_str()?.to_string(),
            _ => {
                return Err(at_line(
                    lua,
                    "horizon.client_command field names are strings".to_owned(),
                ));
            }
        };
        match (name.as_str(), value) {
            ("usage", Value::String(text)) => usage = Some(text.to_str()?.to_string()),
            ("usage", _) => {
                return Err(at_line(
                    lua,
                    "horizon.client_command field 'usage' needs a string".to_owned(),
                ));
            }
            (other, _) => {
                return Err(at_line(
                    lua,
                    format!(
                        "horizon.client_command has no field '{other}'; its only field is usage"
                    ),
                ));
            }
        }
    }
    let usage = usage.ok_or_else(|| {
        at_line(
            lua,
            "horizon.client_command needs a field 'usage'".to_owned(),
        )
    })?;
    check_usage(&usage).map_err(|reason| {
        at_line(
            lua,
            format!("horizon.client_command field 'usage': {reason}"),
        )
    })?;
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

/// Reads `horizon.block_part(id, {hook = fn, ...})` or `horizon.item_part(...)` (`kind` names it): the
/// part's id and handlers are checked, each handler is stored under [`part_handler_key`], and the
/// record is kept in `records`.
fn declare_part(
    lua: &Lua,
    kind: PartKind,
    id: Value,
    spec: Value,
    records: &RefCell<Vec<PartRecord>>,
    handlers: &Table,
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
    if records.len() >= MAX_PARTS {
        return Err(at_line(
            lua,
            format!(
                "a mod may declare at most {MAX_PARTS} {} parts",
                kind.noun()
            ),
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
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Value::String(name) = key else {
            return Err(at_line(
                lua,
                format!("horizon.{keyword} handler names are strings"),
            ));
        };
        let name = name.to_str()?.to_string();
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
    if hooks.is_empty() {
        return Err(at_line(
            lua,
            format!("horizon.{keyword} for '{id}' needs at least one handler"),
        ));
    }
    hooks.sort();
    records.push(PartRecord { id, hooks });
    Ok(())
}

/// Whether `name` is a handler the host binds for a part of `kind`: an error with the reason when it
/// is not.
fn check_part_handler(kind: PartKind, name: &str) -> Result<(), String> {
    match kind.handlers().iter().find(|(hook, _)| *hook == name) {
        None => Err(match kind {
            PartKind::Block => "not a block part handler".to_owned(),
            PartKind::Item => "not an item part handler".to_owned(),
            PartKind::BlockEntity => "not a block entity part handler".to_owned(),
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
fn check_path(whole: &str, path: &str) -> Result<(), String> {
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
        assert!(started.elapsed() < Duration::from_secs(2));
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
        assert!(started.elapsed() < Duration::from_secs(2));
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
}
