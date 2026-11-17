//! Declaring mode: a mod's `main.luau` runs once, at start-up, to learn which hooks it subscribes to.
//!
//! The game reads the hooks a mod listens to before any world or host exists, so the chunk runs
//! now, with one function in reach, `horizon.on(hook, handler)`. The handlers it registers stay in
//! the state for later. The chunk is untrusted code run before the game starts, so it has a time
//! budget, cannot touch anything but `horizon.on`, and can register at most `MAX_HANDLERS`.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use mlua::{Error, Function, Lua, Table, Value, VmState};

/// The most `horizon.on` calls one mod may make.
pub const MAX_HANDLERS: usize = 256;

/// The time a mod's top level may run while it loads.
pub const DECLARE_BUDGET: Duration = Duration::from_secs(2);

/// Registry key of the table that holds each hook's handlers: hook name -> sequence of functions.
const HANDLERS_KEY: &str = "eh_handlers";

/// Registry key of the environment the mod's chunk ran in.
const ENV_KEY: &str = "eh_env";

const LATE_ON: &str = "horizon.on can only be used while the mod loads";

/// The hooks a mod subscribed to: each name once, in the order of its first `horizon.on`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declaration {
    pub hooks: Vec<String>,
}

/// Runs `source` (the mod's main.luau, named `chunk_name`) in declaring mode, with the default
/// time budget. `is_hook` says whether a hook name exists in the game's catalog.
pub fn declare(
    lua: &Lua,
    source: &str,
    chunk_name: &str,
    is_hook: &dyn Fn(&str) -> bool,
) -> Result<Declaration, String> {
    declare_with_budget(lua, source, chunk_name, is_hook, DECLARE_BUDGET)
}

/// The same as `declare` with an explicit time budget.
pub fn declare_with_budget(
    lua: &Lua,
    source: &str,
    chunk_name: &str,
    is_hook: &dyn Fn(&str) -> bool,
    budget: Duration,
) -> Result<Declaration, String> {
    let (hooks, loaded) =
        run(lua, source, chunk_name, is_hook, budget).map_err(|e| e.to_string())?;
    loaded
        .map(|()| Declaration { hooks })
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

/// Sets up the environment and the `horizon` table, runs the chunk, and then replaces `horizon.on`
/// with a function that only refuses. Returns the hooks recorded and the chunk's own result: the
/// setup errors come first, and the chunk's result is kept apart so the caller can report it.
fn run(
    lua: &Lua,
    source: &str,
    chunk_name: &str,
    is_hook: &dyn Fn(&str) -> bool,
    budget: Duration,
) -> mlua::Result<(Vec<String>, mlua::Result<()>)> {
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

    let recorded = RefCell::new(Vec::<String>::new());
    let calls = Cell::new(0_usize);
    let finished = Cell::new(false);

    // `horizon.on` borrows the caller's `is_hook` and the cells above, so it is a scoped function:
    // it expires when the scope ends, and is replaced below before anything can call it.
    let loaded = lua
        .scope(|scope| {
            let on = scope.create_function(|_, (name, handler): (Value, Value)| {
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
                {
                    let mut hooks = recorded.borrow_mut();
                    if !hooks.contains(&name) {
                        hooks.push(name.clone());
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
            horizon.raw_set("on", on)?;
            horizon.set_readonly(true);
            env.raw_set("horizon", horizon.clone())?;

            let deadline = Instant::now() + budget;
            lua.set_interrupt(move |_| {
                if Instant::now() >= deadline {
                    Err(Error::runtime("the mod ran over its loading time budget"))
                } else {
                    Ok(VmState::Continue)
                }
            });
            let result = lua
                .load(source)
                .set_name(chunk_name)
                .set_environment(env.clone())
                .exec();
            lua.remove_interrupt();
            finished.set(true);
            Ok(result)
        })
        .and_then(|result| result);

    horizon.set_readonly(false);
    let refuse = lua.create_function(|_, _: mlua::MultiValue| -> mlua::Result<()> {
        Err(Error::runtime(LATE_ON))
    })?;
    horizon.raw_set("on", refuse)?;
    horizon.set_readonly(true);

    Ok((recorded.into_inner(), loaded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::new_state;

    fn is_known(name: &str) -> bool {
        name == "tick" || name == "chat"
    }

    fn declare_in_fresh_state(source: &str) -> Result<Declaration, String> {
        let lua = new_state().unwrap();
        declare(&lua, source, "@lua-mods/test/main.luau", &is_known)
    }

    #[test]
    fn on_records_hooks_in_order_without_duplicates() {
        let lua = new_state().unwrap();
        let source = "\
            horizon.on(\"tick\", function() end)\n\
            horizon.on(\"chat\", function() end)\n\
            horizon.on(\"tick\", function() end)\n";
        let declaration = declare(&lua, source, "@lua-mods/test/main.luau", &is_known).unwrap();
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
        declare(&lua, source, "@lua-mods/test/main.luau", &is_known).unwrap();
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
        declare(&lua, source, "=mod", &|n| n == "tick" || n == "chat").unwrap();
        let tick = handlers(&lua, "tick").unwrap();
        assert!(tick[0].call::<()>(()).is_err());
    }

    #[test]
    fn pcall_cannot_swallow_the_time_budget() {
        // The interrupt keeps failing after the deadline, so catching one error does not help.
        let lua = new_state().unwrap();
        let source = "while true do pcall(function() while true do end end) end";
        let started = std::time::Instant::now();
        let result =
            declare_with_budget(&lua, source, "=mod", &|_| true, Duration::from_millis(100));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
