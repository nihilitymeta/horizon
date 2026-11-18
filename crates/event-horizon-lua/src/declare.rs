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
    loaded
        .map(|()| Declaration {
            hooks,
            options: recorded.options,
            blocks: recorded.blocks,
            items: recorded.items,
        })
        .map_err(|e| e.to_string())
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
            horizon.raw_set("on", on)?;
            horizon.raw_set("block", block)?;
            horizon.raw_set("item", item)?;
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
