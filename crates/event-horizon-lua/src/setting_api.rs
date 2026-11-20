//! The settings a Lua mod reads and is told about (issue 74, 3e Phase B).
//!
//! - `horizon.setting(key)` reads the mod's current value. It is in the declaring environment (so a
//!   mod can declare content from a setting) and in the runtime `horizon` table (handlers). It is a
//!   host-free read, so it spends no turn budget; the handler's wall-clock budget still applies.
//! - `horizon.on_setting_changed(fn)` registers the mod's one change handler, at load only. The
//!   function is kept in the Lua registry under [`HANDLER_KEY`]; it is not a hook.
//! - [`deliver`] runs that handler once for one [`Change`], in a turn with no host (the `None` turn of
//!   `hostcall`). A handler that fails is logged once and does not undo the change; a panic is
//!   contained and quarantines the handler under the hook name [`HOOK`]. Lua has no `set`, so a
//!   handler cannot change a setting.

use mlua::{Error, Function, Lua, Value as LuaValue};

use crate::budget;
use crate::dispatch::side_runs;
use crate::host::{Escaped, LuaMod, handler_budget, warn_once};
use crate::hostcall::{TurnHost, Void, contained, scoped};
use crate::settings::{Change, Settings, Value};

/// Registry key of the `on_setting_changed` handler, when the mod declared one.
pub(crate) const HANDLER_KEY: &str = "eh_setting_changed";
/// The name the log and the quarantine give the handler.
pub(crate) const HOOK: &str = "setting_changed";
/// Raised by `horizon.on_setting_changed` once the mod has loaded.
pub(crate) const LATE: &str = "horizon.on_setting_changed can only be used while the mod loads";
/// Raised by a second `horizon.on_setting_changed` in one mod.
pub(crate) const TWICE: &str = "horizon.on_setting_changed may be called only once per mod";
/// Raised by `horizon.setting` when its key is not a string.
const NOT_STRING: &str = "horizon.setting needs the key as a string";

/// A setting value as Lua sees it: a boolean, a number or a string.
pub(crate) fn lua_value(lua: &Lua, value: &Value) -> mlua::Result<LuaValue> {
    Ok(match value {
        Value::Bool(on) => LuaValue::Boolean(*on),
        Value::Int(n) => LuaValue::Integer(*n),
        Value::Float(v) => LuaValue::Number(*v),
        Value::Str(text) => LuaValue::String(lua.create_string(text)?),
    })
}

/// `horizon.setting(key)`: the value of `key` in `settings`, or an error that names the key.
pub(crate) fn read(lua: &Lua, settings: &Settings, key: &LuaValue) -> mlua::Result<LuaValue> {
    let LuaValue::String(key) = key else {
        return Err(Error::runtime(NOT_STRING));
    };
    let key = key.to_str()?.to_string();
    match settings.get(&key) {
        Some(value) => lua_value(lua, &value),
        None => Err(Error::runtime(format!(
            "horizon.setting: no setting named '{key}' in this mod"
        ))),
    }
}

/// The arguments of the change handler: the key, the new value and the old one.
fn args(lua: &Lua, change: &Change) -> mlua::Result<(LuaValue, LuaValue, LuaValue)> {
    Ok((
        LuaValue::String(lua.create_string(&change.key)?),
        lua_value(lua, &change.value)?,
        lua_value(lua, &change.old)?,
    ))
}

/// Delivers `change` to the change handler of mod `m`. A mod without one, or whose handler is
/// quarantined, gets nothing. The call takes the mod's turn (the caller must not hold it here).
pub(crate) fn deliver(m: &LuaMod, change: &Change) {
    deliver_with(m, change, &args);
}

/// `deliver`, with the arguments made by `build`. A test passes a `build` that panics to see the
/// containment; the arguments themselves cannot fail in production.
pub(crate) fn deliver_with(
    m: &LuaMod,
    change: &Change,
    build: &dyn Fn(&Lua, &Change) -> mlua::Result<(LuaValue, LuaValue, LuaValue)>,
) {
    if !m.declares_setting_handler() {
        return;
    }
    let _turn = m.turn().enter();
    if !side_runs(m, HOOK, None) {
        return;
    }
    let lua = m.lua();
    let handler: Option<Function> = match lua.named_registry_value(HANDLER_KEY) {
        Ok(handler) => handler,
        Err(err) => {
            warn_once(m.id(), &format!("{}: {HOOK}: {}", m.id(), Escaped(err)));
            return;
        }
    };
    let Some(handler) = handler else {
        return;
    };
    let void = Void::default();
    let mut host = TurnHost::None;
    let outcome = contained(m.id(), HOOK, || {
        scoped(lua, &mut host, &void, |lua| {
            build(lua, change).and_then(|event| {
                budget::within_with(
                    lua,
                    handler_budget(),
                    "the handler ran over its time budget",
                    || handler.call::<()>(event),
                )
            })
        })
    });
    match outcome {
        // A panic was logged and quarantined the handler by `contained`.
        None => {}
        Some(Ok(Ok(()))) => {}
        Some(Ok(Err(err))) | Some(Err(err)) => {
            warn_once(m.id(), &format!("{}: {HOOK}: {}", m.id(), Escaped(err)));
        }
    }
}
