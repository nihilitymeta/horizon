//! Hook dispatch: one hook fired into every mod that declared it, each with its own event table.
//!
//! A handler gets ONE argument, the event. Its data fields are read-only: the host copies them in
//! when it builds the event, and the mod can read them but not change them. The mod's answers go
//! through a few writable keys (`cancelled`, `result`, ...) named by the caller. The host reads them
//! back with `Event::answer` after each handler, and decides from them whether the chain goes on.
//! A hook with outputs goes through `LuaHost::dispatch_turns`: a mod's answers are kept only when
//! its whole turn succeeded, and each one is converted by `accept` before it is kept.
//!
//! The handler sees an empty proxy table with a locked metatable. Reads go to the answers the mod
//! has written, then to the data. Writes go through a host function that accepts only the writable
//! keys. The proxy is read-only too, so `rawset` cannot store a field on it and shadow the data.

use std::cell::RefCell;

use event_horizon_lua_convert::scalar::FromLuaValue;
use mlua::{Error, Lua, MetaMethod, Table, UserData, UserDataMethods, Value};

use crate::declare;
use crate::discover::Side;
use crate::generated::hooks::{HookSide, hook_info};
use crate::host::{LOG_TARGET, LuaHost, LuaMod, call_handler, call_handler_value};
use crate::hostcall::{TurnHost, note_skipped, open_call, run_hosted, scoped};

/// What `getmetatable` returns for an event, so a mod cannot reach or replace its metatable.
const LOCKED: bool = false;

/// The order the handlers of one hook run in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// The order the mods declared the hook: load order, then `horizon.on` order within a mod.
    Registration,
}

/// One mod's event for one hook call: the table its handlers get, and the answers it wrote.
pub struct Event {
    table: Table,
    answers: Table,
}

impl Event {
    /// The table the handlers receive.
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// The raw value the mod wrote to `key` (nil if it wrote nothing there).
    pub fn answer(&self, key: &str) -> Value {
        self.answers.raw_get(key).unwrap_or(Value::Nil)
    }
}

/// The value a mod writes to answer `None` on an `Option` output: `horizon.none`. It is a userdata
/// with no fields and no way to make another, so a table, a string or a number cannot forge it. The
/// host makes one per Lua state (`host::runtime_table`), and [`is_none`] tells it apart.
pub struct NoneAnswer;

impl UserData for NoneAnswer {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, _, ()| Ok("horizon.none"));
    }
}

/// Whether `value` is `horizon.none`. Only the host makes a [`NoneAnswer`], one per state, so the
/// type is the identity.
pub fn is_none(value: &Value) -> bool {
    matches!(value, Value::UserData(ud) if ud.is::<NoneAnswer>())
}

/// Takes the value a mod wrote to a non-optional output key, if it wrote one (`nil` is no answer).
/// `horizon.none` is refused: such an output has no None. A value that does not convert to the
/// output's type is refused too. A refusal keeps the slot's previous value and logs one warning
/// naming the mod, the hook and the output. Other outputs of the same turn are not affected.
/// Returns whether the value was refused.
pub fn accept<T: FromLuaValue>(
    slot: &RefCell<T>,
    id: &str,
    hook: &str,
    output: &str,
    value: &Value,
) -> bool {
    accept_by(slot, id, hook, output, value, |value| {
        T::from_lua_value(value).map_err(|err| err.to_string())
    })
}

/// The same as [`accept`], with the conversion given: a stack or a block state converts through
/// its host, so it cannot be a `FromLuaValue` impl. The same warning and the same kept value.
pub fn accept_by<T>(
    slot: &RefCell<T>,
    id: &str,
    hook: &str,
    output: &str,
    value: &Value,
    convert: impl FnOnce(&Value) -> Result<T, String>,
) -> bool {
    if is_none(value) {
        return refuse(id, hook, output, "this output cannot be none");
    }
    keep(slot, id, hook, output, value, convert)
}

/// Takes the value a mod wrote to an `Option` output key. `horizon.none` sets the slot to `None`;
/// any other value converts to the inner type and sets `Some`. `nil` is no answer, as for
/// [`accept`]. Returns whether the value was refused.
pub fn accept_opt<T: FromLuaValue>(
    slot: &RefCell<Option<T>>,
    id: &str,
    hook: &str,
    output: &str,
    value: &Value,
) -> bool {
    accept_opt_by(slot, id, hook, output, value, |value| {
        T::from_lua_value(value).map_err(|err| err.to_string())
    })
}

/// The same as [`accept_opt`], with the conversion of the inner type given (a stack or a block
/// state converts through its host). The conversion never sees `horizon.none`.
pub fn accept_opt_by<T>(
    slot: &RefCell<Option<T>>,
    id: &str,
    hook: &str,
    output: &str,
    value: &Value,
    convert: impl FnOnce(&Value) -> Result<T, String>,
) -> bool {
    if is_none(value) {
        *slot.borrow_mut() = None;
        return false;
    }
    keep(slot, id, hook, output, value, |value| {
        convert(value).map(Some)
    })
}

/// Stores the converted value, unless the value is nil (no answer) or does not convert (refused).
fn keep<T>(
    slot: &RefCell<T>,
    id: &str,
    hook: &str,
    output: &str,
    value: &Value,
    convert: impl FnOnce(&Value) -> Result<T, String>,
) -> bool {
    if matches!(value, Value::Nil) {
        return false;
    }
    match convert(value) {
        Ok(next) => {
            *slot.borrow_mut() = next;
            false
        }
        Err(err) => refuse(
            id,
            hook,
            output,
            &format!("the previous value is kept: {err}"),
        ),
    }
}

/// Logs the one warning for a refused output and returns `true`.
fn refuse(id: &str, hook: &str, output: &str, why: &str) -> bool {
    log::warn!(target: LOG_TARGET, "{id}: {hook}: output `{output}` refused, {why}");
    true
}

/// Converts a replace hook's answer, a mod's return value, to `T`. A value that does not convert is
/// refused: one warning names the mod, the hook and the error, and the mod gives no answer.
pub fn answer_of<T: FromLuaValue>(id: &str, hook: &str, value: &Value) -> Option<T> {
    answer_by(id, hook, value, |value| {
        T::from_lua_value(value).map_err(|err| err.to_string())
    })
}

/// The same as [`answer_of`], with the conversion given (a stack or a block state converts through
/// its host). `horizon.none` is refused: a replace answer is always a value.
pub fn answer_by<T>(
    id: &str,
    hook: &str,
    value: &Value,
    convert: impl FnOnce(&Value) -> Result<T, String>,
) -> Option<T> {
    let converted = if is_none(value) {
        Err("this answer cannot be none".to_owned())
    } else {
        convert(value)
    };
    match converted {
        Ok(answer) => Some(answer),
        Err(err) => {
            log::warn!(
                target: LOG_TARGET,
                "{id}: {hook}: answer refused, the mod gives no answer: {err}"
            );
            None
        }
    }
}

/// Makes the event for one mod. `data` is the caller's table of data fields (any shape); it is
/// frozen here, nested tables included. `writable` names the keys the mod may write.
pub fn new_event(lua: &Lua, data: Table, writable: &[&'static str]) -> mlua::Result<Event> {
    freeze(&data);
    let answers = lua.create_table()?;
    let answers_meta = lua.create_table()?;
    answers_meta.raw_set("__index", data)?;
    answers.set_metatable(Some(answers_meta))?;

    let allowed: Vec<&'static str> = writable.to_vec();
    let sink = answers.clone();
    let write = lua.create_function(move |_, (_, key, value): (Value, Value, Value)| {
        let name = match &key {
            Value::String(s) => s.to_string_lossy(),
            other => other.type_name().to_string(),
        };
        if !allowed.iter().any(|w| *w == name) {
            return Err(Error::runtime(format!("event.{name} is read-only")));
        }
        sink.raw_set(name, value)
    })?;

    let meta = lua.create_table()?;
    meta.raw_set("__index", answers.clone())?;
    meta.raw_set("__newindex", write)?;
    meta.raw_set("__metatable", LOCKED)?;
    let table = lua.create_table()?;
    table.set_metatable(Some(meta))?;
    table.set_readonly(true);
    Ok(Event { table, answers })
}

/// Makes `table` and every table under it read-only. A table already read-only is left alone,
/// which also ends a cycle.
fn freeze(table: &Table) {
    if table.is_readonly() {
        return;
    }
    table.set_readonly(true);
    for (_, value) in table.pairs::<Value, Value>().flatten() {
        if let Value::Table(child) = value {
            freeze(&child);
        }
    }
}

impl LuaHost {
    /// Fires `hook` for every mod that declared it. A client-only mod is skipped for the server
    /// hooks. For each mod: take its turn, build its event with `build` (an error is logged and
    /// skips that mod), then call each of its handlers with the event table under `HANDLER_BUDGET`.
    /// A failing handler is logged and does not stop the others. After each handler, `answers` gets
    /// the mod's id and its event, and returns whether the chain goes on: `false` stops the mod's
    /// remaining handlers and every later mod.
    pub fn dispatch(
        &self,
        hook: &str,
        build: &dyn Fn(&Lua) -> mlua::Result<Event>,
        answers: &mut dyn FnMut(&str, &Event) -> bool,
    ) {
        self.dispatch_cancellable(hook, &|| false, build, answers);
    }

    /// The same as `dispatch`, in the order the mods' priorities give (`run_order`). Before each
    /// mod, if `is_cancelled()` says the event is already cancelled, a mod that did not declare
    /// `receive_cancelled` for this hook is skipped; the others see the event as it is.
    /// Whether any mod that declared `hook` will run its handlers for it: the same side rule the
    /// dispatch loops apply. A hook no mod handles converts nothing (see the generated pre-checks).
    pub(crate) fn has_handlers(&self, hook: &str) -> bool {
        let mods = self.mods();
        let server_hook = hook_info(hook).is_some_and(|info| info.side == HookSide::Server);
        self.run_order(hook)
            .iter()
            .any(|&index| !(server_hook && mods[index].side() == Side::Client))
    }

    pub fn dispatch_cancellable(
        &self,
        hook: &str,
        is_cancelled: &dyn Fn() -> bool,
        build: &dyn Fn(&Lua) -> mlua::Result<Event>,
        answers: &mut dyn FnMut(&str, &Event) -> bool,
    ) {
        let mods = self.mods();
        for &index in self.run_order(hook) {
            let m = &mods[index];
            if is_cancelled() && !m.options(hook).receive_cancelled {
                continue;
            }
            let server_hook = hook_info(hook).is_some_and(|info| info.side == HookSide::Server);
            if server_hook && m.side() == Side::Client {
                continue;
            }
            let _turn = m.turn().enter();
            let handlers = match declare::handlers(m.lua(), hook) {
                Ok(handlers) => handlers,
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    continue;
                }
            };
            if handlers.is_empty() {
                continue;
            }
            let event = match build(m.lua()) {
                Ok(event) => event,
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    continue;
                }
            };
            for handler in &handlers {
                if let Err(err) = call_handler(m.lua(), handler, event.table()) {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                }
                if !answers(m.id(), &event) {
                    return;
                }
            }
        }
    }
}

impl LuaHost {
    /// Fires `hook` for every mod that declared it, for a hook with outputs. The order and the
    /// cancelled rule are `dispatch_cancellable`'s. A mod's turn is its handlers, run in order under
    /// their budgets, with `host` open to them (see `hostcall`). `build` makes the mod's event after
    /// the earlier mods committed, so the mod sees their accepted values; it gets the host too, for
    /// the values it converts. When the turn is over and none of its handlers failed, `commit` takes
    /// the answers (the output keys the mod wrote, read with `Event::answer`); a turn with a failing
    /// handler is void and is not committed. `commit` returns whether the chain goes on.
    ///
    /// A hook call nested deeper than `MAX_DEPTH` is refused with `Err` and no mod runs. A mod whose
    /// turn is already open on this thread is skipped there.
    pub fn dispatch_turns(
        &self,
        hook: &str,
        host: TurnHost<'_>,
        is_cancelled: &dyn Fn() -> bool,
        build: &dyn Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event>,
        commit: &mut dyn FnMut(&str, &Event, &TurnHost<'_>) -> bool,
    ) -> Result<(), String> {
        turns_in(
            self.mods(),
            self.run_order(hook),
            hook,
            hook,
            host,
            is_cancelled,
            build,
            commit,
        )
    }
}

/// Whether a hook of `hook`'s side runs for mod `m`: a client-only mod does not run a server hook.
pub(crate) fn side_runs(m: &LuaMod, hook: &str) -> bool {
    let server_hook = hook_info(hook).is_some_and(|info| info.side == HookSide::Server);
    !(server_hook && m.side() == Side::Client)
}

/// The body of `LuaHost::dispatch_turns`, over the mods at `order` in `mods`. A mod's handlers for
/// `hook` are read under `key`: the hook's own name, or a block part's key (see
/// `declare::part_handler_key`), so a part reaches only its own mod's handler.
#[allow(clippy::too_many_arguments)]
pub(crate) fn turns_in(
    mods: &[LuaMod],
    order: &[usize],
    hook: &str,
    key: &str,
    host: TurnHost<'_>,
    is_cancelled: &dyn Fn() -> bool,
    build: &dyn Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event>,
    commit: &mut dyn FnMut(&str, &Event, &TurnHost<'_>) -> bool,
) -> Result<(), String> {
    {
        let _open = open_call(hook)?;
        let mut host = host;
        for &index in order {
            let m = &mods[index];
            if is_cancelled() && !m.options(hook).receive_cancelled {
                continue;
            }
            let server_hook = hook_info(hook).is_some_and(|info| info.side == HookSide::Server);
            if server_hook && m.side() == Side::Client {
                continue;
            }
            if m.turn().held_here() {
                note_skipped(m.id(), hook);
                continue;
            }
            let _turn = m.turn().enter();
            let handlers = match declare::handlers(m.lua(), key) {
                Ok(handlers) => handlers,
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    continue;
                }
            };
            if handlers.is_empty() {
                continue;
            }
            let event = match build(m.lua(), &host) {
                Ok(event) => event,
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    continue;
                }
            };
            if run_hosted(m, hook, &mut host, &handlers, event.table()) {
                log::warn!(
                    target: LOG_TARGET,
                    "{}: {hook}: a handler failed, so this mod's changes to the event are dropped",
                    m.id()
                );
                continue;
            }
            if !commit(m.id(), &event, &host) {
                return Ok(());
            }
        }
        Ok(())
    }
}

impl LuaHost {
    /// Fires `hook`, a replace hook, for every mod that declared it. The order is the one
    /// `dispatch` uses. A mod's turn runs all its handlers in order under their budgets, with `host`
    /// open to them, and the mod's answer is the first return value of its handlers that is not nil.
    /// A failing handler voids the turn: that mod gives no answer, whatever its other handlers
    /// returned. `answer` takes the mod's id, the value and the host, and returns whether the value
    /// was answered (the chain stops then); when it was refused, the chain goes on to the next mod.
    /// A call nested too deep is refused with `Err`, as `dispatch_turns` refuses it.
    pub fn dispatch_replace(
        &self,
        hook: &str,
        host: TurnHost<'_>,
        build: &dyn Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event>,
        answer: &mut dyn FnMut(&str, Value, &TurnHost<'_>) -> bool,
    ) -> Result<(), String> {
        replace_in(
            self.mods(),
            self.run_order(hook),
            hook,
            hook,
            host,
            build,
            answer,
        )
    }
}

/// The body of `LuaHost::dispatch_replace`, over the mods at `order` in `mods`, with the handler
/// `key` as `turns_in` reads it.
pub(crate) fn replace_in(
    mods: &[LuaMod],
    order: &[usize],
    hook: &str,
    key: &str,
    host: TurnHost<'_>,
    build: &dyn Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event>,
    answer: &mut dyn FnMut(&str, Value, &TurnHost<'_>) -> bool,
) -> Result<(), String> {
    {
        let _open = open_call(hook)?;
        let mut host = host;
        for &index in order {
            let m = &mods[index];
            let server_hook = hook_info(hook).is_some_and(|info| info.side == HookSide::Server);
            if server_hook && m.side() == Side::Client {
                continue;
            }
            if m.turn().held_here() {
                note_skipped(m.id(), hook);
                continue;
            }
            let _turn = m.turn().enter();
            let handlers = match declare::handlers(m.lua(), key) {
                Ok(handlers) => handlers,
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    continue;
                }
            };
            if handlers.is_empty() {
                continue;
            }
            let event = match build(m.lua(), &host) {
                Ok(event) => event,
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    continue;
                }
            };
            let (found, failed) = match scoped(m.lua(), &mut host, |lua| {
                let mut found: Option<Value> = None;
                let mut failed = false;
                for handler in &handlers {
                    match call_handler_value(lua, handler, event.table()) {
                        Ok(value) => {
                            if found.is_none() && !matches!(value, Value::Nil) {
                                found = Some(value);
                            }
                        }
                        Err(err) => {
                            log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                            failed = true;
                        }
                    }
                }
                (found, failed)
            }) {
                Ok(((found, failed), spent)) => (found, failed || spent),
                Err(err) => {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    (None, true)
                }
            };
            if failed {
                log::warn!(
                    target: LOG_TARGET,
                    "{}: {hook}: a handler failed, so this mod gives no answer",
                    m.id()
                );
                continue;
            }
            let answered = match found {
                Some(value) => answer(m.id(), value, &host),
                None => false,
            };
            if answered {
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{Discovered, ENTRY};
    use crate::host::tests::{captured_with_prefix, install_capture};
    use crate::host::{LuaHost, LuaMod};

    /// A server hook the generated impl has, with a cancel answer: the tests fire it by name.
    const HOOK: &str = "block_receive_game";
    use std::cell::Cell;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// Loads a mod named `id` whose main.luau is `source`.
    pub(super) fn load(root: &Path, id: &str, source: &str, side: Side) -> LuaMod {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), source).unwrap();
        LuaMod::load(&Discovered {
            dir,
            id: id.to_string(),
            version: "1".to_string(),
            side,
        })
        .unwrap()
    }

    /// Data `{pos = {x = 1}}`, the shape the first tests use.
    fn pos_data(lua: &Lua) -> Table {
        let data = lua.create_table().unwrap();
        let pos = lua.create_table().unwrap();
        pos.raw_set("x", 1).unwrap();
        data.raw_set("pos", pos).unwrap();
        data
    }

    /// Fires `HOOK` with `build` and a callback that always continues.
    fn fire_tick(host: &LuaHost, build: &dyn Fn(&Lua) -> mlua::Result<Event>) {
        host.dispatch(HOOK, build, &mut |_, _| true);
    }

    /// Fires `HOOK` with pos data and the writable keys the tests use.
    fn fire_with_pos(host: &LuaHost) {
        fire_tick(host, &|lua| {
            new_event(lua, pos_data(lua), &["cancelled", "result"])
        });
    }

    #[test]
    fn handlers_read_data_fields() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "mod_a",
            "horizon.on(\"block_receive_game\", function(event) horizon.log(tostring(event.pos.x)) end)",
            Side::Both,
        );
        let host = LuaHost::new(vec![m]);
        fire_with_pos(&host);
        assert_eq!(host.mods()[0].log_lines(), ["1"]);
    }

    #[test]
    fn a_data_field_cannot_be_replaced() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event) event.pos = 1 end)\n\
                      horizon.on(\"block_receive_game\", function(event) horizon.log(tostring(event.pos.x)) end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        fire_with_pos(&host);
        assert_eq!(host.mods()[0].log_lines(), ["1"]);
    }

    #[test]
    fn only_writable_keys_are_accepted_and_read_back() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event)\n\
                      event.cancelled = true\n\
                      local ok, err = pcall(function() event.other = 1 end)\n\
                      horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
                      end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        let mut seen = Vec::new();
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, pos_data(lua), &["cancelled", "result"]),
            &mut |_, event| {
                seen.push((event.answer("cancelled"), event.answer("other")));
                true
            },
        );
        let log = host.mods()[0].log_lines();
        assert_eq!(log.len(), 1);
        assert!(log[0].starts_with("false "), "{log:?}");
        assert!(log[0].contains("event.other is read-only"), "{log:?}");
        assert_eq!(seen.len(), 1);
        assert!(matches!(seen[0].0, Value::Boolean(true)));
        assert!(matches!(seen[0].1, Value::Nil));
    }

    #[test]
    fn a_writable_key_shadows_data_for_later_handlers() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event) event.result = 5 end)\n\
                      horizon.on(\"block_receive_game\", function(event) horizon.log(tostring(event.result == 5)) end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        fire_with_pos(&host);
        assert_eq!(host.mods()[0].log_lines(), ["true"]);
    }

    #[test]
    fn the_metatable_cannot_be_reached() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event)\n\
                      horizon.log(tostring(getmetatable(event)))\n\
                      local ok, err = pcall(setmetatable, event, {})\n\
                      horizon.log(tostring(ok))\n\
                      end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        fire_with_pos(&host);
        assert_eq!(host.mods()[0].log_lines(), ["false", "false"]);
    }

    #[test]
    fn rawset_cannot_shadow_the_data() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event)\n\
                      local ok = pcall(rawset, event, \"pos\", 1)\n\
                      horizon.log(tostring(ok) .. \" \" .. tostring(event.pos.x))\n\
                      end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        fire_with_pos(&host);
        assert_eq!(host.mods()[0].log_lines(), ["false 1"]);
    }

    #[test]
    fn nested_data_cannot_be_changed_either() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event)\n\
                      local ok = pcall(function() event.pos.x = 9 end)\n\
                      horizon.log(tostring(ok) .. \" \" .. tostring(event.pos.x))\n\
                      end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        fire_with_pos(&host);
        assert_eq!(host.mods()[0].log_lines(), ["false 1"]);
    }

    #[test]
    fn the_answers_callback_can_stop_the_chain() {
        let root = tempfile::tempdir().unwrap();
        let a = "horizon.on(\"block_receive_game\", function(event) event.cancelled = true; horizon.log(\"a1\") end)\n\
                 horizon.on(\"block_receive_game\", function(event) horizon.log(\"a2\") end)";
        let b = "horizon.on(\"block_receive_game\", function(event) horizon.log(\"b1\") end)";
        let mod_a = load(root.path(), "mod_a", a, Side::Both);
        let mod_b = load(root.path(), "mod_b", b, Side::Both);
        let host = LuaHost::new(vec![mod_a, mod_b]);
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, pos_data(lua), &["cancelled"]),
            &mut |_, event| !matches!(event.answer("cancelled"), Value::Boolean(true)),
        );
        assert_eq!(host.mods()[0].log_lines(), ["a1"]);
        assert!(host.mods()[1].log_lines().is_empty());
    }

    #[test]
    fn a_build_error_skips_only_that_mod() {
        let root = tempfile::tempdir().unwrap();
        let logging =
            "horizon.on(\"block_receive_game\", function(event) horizon.log(\"ran\") end)";
        let host = LuaHost::new(vec![
            load(root.path(), "mod_a", logging, Side::Both),
            load(root.path(), "mod_b", logging, Side::Both),
        ]);
        let calls = Cell::new(0);
        host.dispatch(
            HOOK,
            &|lua| {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    return Err(Error::runtime("no event for this mod"));
                }
                new_event(lua, pos_data(lua), &[])
            },
            &mut |_, _| true,
        );
        assert!(host.mods()[0].log_lines().is_empty());
        assert_eq!(host.mods()[1].log_lines(), ["ran"]);
    }

    #[test]
    fn two_mods_have_separate_events() {
        let root = tempfile::tempdir().unwrap();
        let a = "horizon.on(\"block_receive_game\", function(event) event.result = 7 end)";
        let b = "horizon.on(\"block_receive_game\", function(event) horizon.log(tostring(event.result)) end)";
        let host = LuaHost::new(vec![
            load(root.path(), "mod_a", a, Side::Both),
            load(root.path(), "mod_b", b, Side::Both),
        ]);
        let mut results = Vec::new();
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, pos_data(lua), &["result"]),
            &mut |id, event| {
                results.push((id.to_string(), event.answer("result")));
                true
            },
        );
        assert_eq!(host.mods()[1].log_lines(), ["nil"]);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "mod_a");
        assert!(matches!(
            results[0].1,
            Value::Integer(7) | Value::Number(7.0)
        ));
        assert_eq!(results[1].0, "mod_b");
        assert!(matches!(results[1].1, Value::Nil));
    }

    #[test]
    fn a_runaway_handler_is_still_budgeted() {
        let root = tempfile::tempdir().unwrap();
        let runaway = "horizon.on(\"block_receive_game\", function(event) while true do end end)";
        let logging =
            "horizon.on(\"block_receive_game\", function(event) horizon.log(\"ran\") end)";
        let host = LuaHost::new(vec![
            load(root.path(), "mod_a", runaway, Side::Both),
            load(root.path(), "mod_b", logging, Side::Both),
        ]);
        let started = Instant::now();
        fire_with_pos(&host);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(host.mods()[1].log_lines(), ["ran"]);
    }

    /// The ids of the mods a `dispatch_cancellable` call ran, in order.
    fn run_ids(host: &LuaHost, is_cancelled: bool) -> Vec<String> {
        let mut ids = Vec::new();
        host.dispatch_cancellable(
            HOOK,
            &|| is_cancelled,
            &|lua| new_event(lua, pos_data(lua), &[]),
            &mut |id, _| {
                ids.push(id.to_string());
                true
            },
        );
        ids
    }

    #[test]
    fn a_cancelled_event_is_hidden_from_mods_that_do_not_receive_it() {
        let root = tempfile::tempdir().unwrap();
        let plain = "horizon.on(\"block_receive_game\", function() horizon.log(\"plain\") end)";
        let receiving = "horizon.on(\"block_receive_game\", function() horizon.log(\"receiving\") end, {receive_cancelled = true})";
        let host = LuaHost::new(vec![
            load(root.path(), "mod_a", plain, Side::Both),
            load(root.path(), "mod_b", receiving, Side::Both),
        ]);
        assert_eq!(run_ids(&host, true), ["mod_b"]);
        assert!(host.mods()[0].log_lines().is_empty());
        assert_eq!(host.mods()[1].log_lines(), ["receiving"]);
    }

    #[test]
    fn an_uncancelled_event_reaches_every_mod() {
        let root = tempfile::tempdir().unwrap();
        let plain = "horizon.on(\"block_receive_game\", function() end)";
        let receiving =
            "horizon.on(\"block_receive_game\", function() end, {receive_cancelled = true})";
        let host = LuaHost::new(vec![
            load(root.path(), "mod_a", plain, Side::Both),
            load(root.path(), "mod_b", receiving, Side::Both),
        ]);
        assert_eq!(run_ids(&host, false), ["mod_a", "mod_b"]);
    }

    #[test]
    fn dispatch_runs_every_mod_like_an_uncancelled_event() {
        let root = tempfile::tempdir().unwrap();
        let plain = "horizon.on(\"block_receive_game\", function() end)";
        let host = LuaHost::new(vec![
            load(root.path(), "mod_a", plain, Side::Both),
            load(root.path(), "mod_b", plain, Side::Both),
        ]);
        let mut ids = Vec::new();
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, pos_data(lua), &[]),
            &mut |id, _| {
                ids.push(id.to_string());
                true
            },
        );
        assert_eq!(ids, ["mod_a", "mod_b"]);
    }

    /// The hook the f32 tests fire: `compute_fov`, a client hook with one f32 output.
    const FOV: &str = "compute_fov";

    /// The event the generated `compute_fov` method builds: the current `degrees` as data, and the
    /// key writable.
    fn fov_event(lua: &Lua, degrees: f32) -> mlua::Result<Event> {
        let data = lua.create_table()?;
        data.raw_set("degrees", f64::from(degrees))?;
        new_event(lua, data, &["degrees"])
    }

    /// Fires `FOV` once from `start` with the same outputs handling as the generated method.
    /// Returns the final value and how many answers were refused.
    fn fire_fov(host: &LuaHost, start: f32) -> (f32, usize) {
        let slot = RefCell::new(start);
        let refused = Cell::new(0);
        host.dispatch_turns(
            FOV,
            TurnHost::None,
            &|| false,
            &|lua, _| fov_event(lua, *slot.borrow()),
            &mut |id, event, _| {
                if accept(&slot, id, FOV, "degrees", &event.answer("degrees")) {
                    refused.set(refused.get() + 1);
                }
                true
            },
        )
        .unwrap();
        (slot.into_inner(), refused.get())
    }

    fn fov_mod(root: &Path, id: &str, source: &str) -> LuaMod {
        load(
            root,
            id,
            &format!("horizon.on(\"{FOV}\", function(event)\n{source}\nend)"),
            Side::Both,
        )
    }

    #[test]
    fn a_later_mod_sees_the_value_an_earlier_mod_accepted() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            fov_mod(root.path(), "mod_a", "event.degrees = 2.0"),
            fov_mod(root.path(), "mod_b", "event.degrees = event.degrees * 3"),
        ]);
        assert_eq!(fire_fov(&host, 1.0), (6.0, 0));
    }

    #[test]
    fn a_string_for_a_number_output_is_refused_once_and_the_value_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            fov_mod(root.path(), "mod_a", "event.degrees = \"far\""),
            fov_mod(root.path(), "mod_b", "event.degrees = event.degrees + 3"),
        ]);
        assert_eq!(fire_fov(&host, 1.0), (4.0, 1));
    }

    #[test]
    fn a_handler_that_assigns_and_then_fails_is_void() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            fov_mod(root.path(), "mod_a", "event.degrees = 5.0\nerror(\"boom\")"),
            fov_mod(root.path(), "mod_b", "event.degrees = event.degrees + 1"),
        ]);
        assert_eq!(fire_fov(&host, 1.0), (2.0, 0));
    }

    #[test]
    fn a_handler_over_its_budget_after_assigning_is_void() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            fov_mod(
                root.path(),
                "mod_a",
                "event.degrees = 5.0\nwhile true do end",
            ),
            fov_mod(root.path(), "mod_b", "event.degrees = event.degrees + 1"),
        ]);
        let started = Instant::now();
        assert_eq!(fire_fov(&host, 1.0), (2.0, 0));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn writing_a_key_that_is_not_an_output_voids_the_turn() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![fov_mod(
            root.path(),
            "mod_a",
            "event.degrees = 5.0\nevent.partial = 1",
        )]);
        assert_eq!(fire_fov(&host, 1.0), (1.0, 0));
    }

    #[test]
    fn a_reference_kept_from_an_earlier_fire_cannot_change_a_later_one() {
        let root = tempfile::tempdir().unwrap();
        let source = "local saved = nil\n\
                      horizon.on(\"compute_fov\", function(event)\n\
                      if saved == nil then saved = event else saved.degrees = 9 end\n\
                      end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        assert_eq!(fire_fov(&host, 1.0), (1.0, 0));
        assert_eq!(fire_fov(&host, 1.0), (1.0, 0));
    }

    #[test]
    fn values_outside_an_output_type_are_refused_and_the_slot_keeps_its_value() {
        let u32_slot = RefCell::new(7_u32);
        assert!(accept(&u32_slot, "m", FOV, "x", &Value::Integer(-1)));
        assert_eq!(u32_slot.into_inner(), 7);
        let f32_slot = RefCell::new(7.0_f32);
        assert!(accept(&f32_slot, "m", FOV, "x", &Value::Number(f64::NAN)));
        assert_eq!(f32_slot.into_inner(), 7.0);
        let i32_slot = RefCell::new(7_i32);
        assert!(accept(&i32_slot, "m", FOV, "x", &Value::Number(1.5)));
        assert_eq!(i32_slot.into_inner(), 7);
        let bool_slot = RefCell::new(false);
        assert!(accept(&bool_slot, "m", FOV, "x", &Value::Number(1.0)));
        assert!(!bool_slot.into_inner());
        let good = RefCell::new(0_u32);
        assert!(!accept(&good, "m", FOV, "x", &Value::Integer(4)));
        assert!(!accept(&good, "m", FOV, "x", &Value::Nil));
        assert_eq!(good.into_inner(), 4);
    }

    /// Fires `FOV` with an `Option<u32>` output held in a slot, as the generated method does for an
    /// `Option` output: the event gets the current value (nil for None), and the answer goes through
    /// `accept_opt`. Returns the final value and how many answers were refused.
    fn fire_opt(host: &LuaHost, start: Option<u32>) -> (Option<u32>, usize) {
        let slot = RefCell::new(start);
        let refused = Cell::new(0);
        host.dispatch_turns(
            FOV,
            TurnHost::None,
            &|| false,
            &|lua, _| {
                let data = lua.create_table()?;
                if let Some(value) = *slot.borrow() {
                    data.raw_set("degrees", value)?;
                }
                new_event(lua, data, &["degrees"])
            },
            &mut |id, event, _| {
                if accept_opt(&slot, id, FOV, "degrees", &event.answer("degrees")) {
                    refused.set(refused.get() + 1);
                }
                true
            },
        )
        .unwrap();
        (slot.into_inner(), refused.get())
    }

    #[test]
    fn horizon_none_clears_an_option_output_and_the_next_mod_sees_nil() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            fov_mod(root.path(), "mod_a", "event.degrees = horizon.none"),
            fov_mod(root.path(), "mod_b", "horizon.log(tostring(event.degrees))"),
            fov_mod(root.path(), "mod_c", "event.degrees = 5"),
        ]);
        assert_eq!(fire_opt(&host, Some(3)), (Some(5), 0));
        assert_eq!(host.mods()[1].log_lines(), ["nil"]);
    }

    #[test]
    fn horizon_none_is_refused_for_a_number_output_with_one_warning() {
        install_capture();
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            fov_mod(
                root.path(),
                "mod_none_refused",
                "event.degrees = horizon.none",
            ),
            fov_mod(
                root.path(),
                "mod_none_later",
                "event.degrees = event.degrees + 3",
            ),
        ]);
        assert_eq!(fire_fov(&host, 1.0), (4.0, 1));
        let warnings = captured_with_prefix(
            "mod_none_refused: compute_fov: output `degrees` refused, this output cannot be none",
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn horizon_none_is_its_own_value_and_horizon_is_frozen() {
        let root = tempfile::tempdir().unwrap();
        let source = "local n = horizon.none\n\
                      horizon.log(type(n) .. \" \" .. tostring(n == nil) .. \" \" .. tostring(n == false) .. \" \" .. tostring(n == 0) .. \" \" .. tostring(n == \"\") .. \" \" .. tostring(n == {}))\n\
                      horizon.log(tostring(pcall(function() horizon.none = 1 end)) .. \" \" .. tostring(n))";
        let host = LuaHost::new(vec![load(
            root.path(),
            "mod_a",
            &format!("horizon.on(\"{FOV}\", function(event)\n{source}\nend)"),
            Side::Both,
        )]);
        assert_eq!(fire_opt(&host, Some(3)), (Some(3), 0));
        assert_eq!(
            host.mods()[0].log_lines(),
            [
                "userdata false false false false false",
                "false horizon.none"
            ]
        );
    }

    #[test]
    fn nil_leaves_an_option_output_and_a_table_is_refused() {
        let lua = Lua::new();
        let slot = RefCell::new(Some(7_u32));
        assert!(!accept_opt(&slot, "m", FOV, "x", &Value::Nil));
        assert_eq!(slot.borrow().clone(), Some(7));
        let table = Value::Table(lua.create_table().unwrap());
        assert!(accept_opt(&slot, "m", FOV, "x", &table));
        assert_eq!(slot.borrow().clone(), Some(7));
        assert!(!accept_opt(&slot, "m", FOV, "x", &Value::Integer(4)));
        assert_eq!(slot.into_inner(), Some(4));
    }

    #[test]
    fn horizon_none_clears_an_option_string_output() {
        let lua = Lua::new();
        let slot = RefCell::new(Some(String::from("far")));
        let text = Value::String(lua.create_string("near").unwrap());
        assert!(!accept_opt(&slot, "m", FOV, "x", &text));
        assert_eq!(slot.borrow().as_deref(), Some("near"));
        let none = Value::UserData(lua.create_userdata(NoneAnswer).unwrap());
        assert!(!accept_opt(&slot, "m", FOV, "x", &none));
        assert_eq!(slot.into_inner(), None);
    }

    #[test]
    fn a_turn_that_assigns_horizon_none_and_then_fails_is_void() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            fov_mod(
                root.path(),
                "mod_a",
                "event.degrees = horizon.none\nerror(\"boom\")",
            ),
            fov_mod(root.path(), "mod_b", "horizon.log(tostring(event.degrees))"),
        ]);
        assert_eq!(fire_opt(&host, Some(3)), (Some(3), 0));
        assert_eq!(host.mods()[1].log_lines(), ["3"]);
    }

    #[test]
    fn a_replace_answer_of_horizon_none_is_refused_with_one_warning() {
        install_capture();
        let lua = Lua::new();
        let none = Value::UserData(lua.create_userdata(NoneAnswer).unwrap());
        assert_eq!(answer_of::<String>("mod_replace_none", HOOK, &none), None);
        let warnings = captured_with_prefix(
            "mod_replace_none: block_receive_game: answer refused, the mod gives no answer: this answer cannot be none",
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    /// The `(mod, answer)` pairs a replace fire of `HOOK` gives, read as strings.
    fn replies(host: &LuaHost) -> Vec<(String, String)> {
        let mut out = Vec::new();
        host.dispatch_replace(
            HOOK,
            TurnHost::None,
            &|lua, _| new_event(lua, pos_data(lua), &[]),
            &mut |id, value, _| match answer_of::<String>(id, HOOK, &value) {
                Some(text) => {
                    out.push((id.to_string(), text));
                    true
                }
                None => false,
            },
        )
        .unwrap();
        out
    }

    /// A mod whose one handler returns `value` (a Lua expression).
    fn replying(root: &Path, id: &str, value: &str) -> LuaMod {
        load(
            root,
            id,
            &format!("horizon.on(\"{HOOK}\", function(event) return {value} end)"),
            Side::Both,
        )
    }

    #[test]
    fn the_first_mod_that_returns_a_value_answers_and_a_nil_mod_does_not() {
        let root = tempfile::tempdir().unwrap();
        let silent = load(
            root.path(),
            "mod_a",
            "horizon.on(\"block_receive_game\", function(event) end)",
            Side::Both,
        );
        let host = LuaHost::new(vec![silent, replying(root.path(), "mod_b", "\"pass\"")]);
        assert_eq!(replies(&host), [("mod_b".to_owned(), "pass".to_owned())]);
    }

    #[test]
    fn an_answer_stops_the_later_mods() {
        let root = tempfile::tempdir().unwrap();
        let counted =
            "horizon.on(\"block_receive_game\", function(event) horizon.log(\"ran\") end)";
        let host = LuaHost::new(vec![
            replying(root.path(), "mod_a", "\"first\""),
            load(root.path(), "mod_b", counted, Side::Both),
        ]);
        assert_eq!(replies(&host), [("mod_a".to_owned(), "first".to_owned())]);
        assert!(host.mods()[1].log_lines().is_empty());
    }

    #[test]
    fn a_value_that_does_not_convert_gives_no_answer_and_the_chain_goes_on() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            replying(root.path(), "mod_a", "5"),
            replying(root.path(), "mod_b", "\"pass\""),
        ]);
        let mut refused = 0;
        let mut got = Vec::new();
        host.dispatch_replace(
            HOOK,
            TurnHost::None,
            &|lua, _| new_event(lua, pos_data(lua), &[]),
            &mut |id, value, _| match answer_of::<String>(id, HOOK, &value) {
                Some(text) => {
                    got.push(text);
                    true
                }
                None => {
                    refused += 1;
                    false
                }
            },
        )
        .unwrap();
        assert_eq!(refused, 1);
        assert_eq!(got, ["pass"]);
    }

    #[test]
    fn a_failing_handler_voids_the_mod_even_after_another_handler_returned_a_value() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event) error(\"boom\") end)\n\
                      horizon.on(\"block_receive_game\", function(event) return \"a\" end)";
        let host = LuaHost::new(vec![
            load(root.path(), "mod_a", source, Side::Both),
            replying(root.path(), "mod_b", "\"b\""),
        ]);
        assert_eq!(replies(&host), [("mod_b".to_owned(), "b".to_owned())]);
    }

    #[test]
    fn of_two_values_from_one_mod_the_first_answers() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event) return \"first\" end)\n\
                      horizon.on(\"block_receive_game\", function(event) return \"second\" end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        assert_eq!(replies(&host), [("mod_a".to_owned(), "first".to_owned())]);
    }

    #[test]
    fn answer_of_refuses_a_value_of_the_wrong_type() {
        assert_eq!(
            answer_of::<bool>("m", HOOK, &Value::Boolean(true)),
            Some(true)
        );
        assert_eq!(answer_of::<bool>("m", HOOK, &Value::Integer(1)), None);
    }

    /// The hook the list tests fire: `component_types`, a client hook whose output is `types`.
    const LIST_HOOK: &str = "component_types";

    /// Fires `LIST_HOOK` once from `start`, with `types` as the writable list, as the generated
    /// method does for a list output. Returns the final list.
    fn fire_list(host: &LuaHost, start: Vec<String>) -> Vec<String> {
        use event_horizon_lua_convert::scalar::ToLuaValue;
        let slot = RefCell::new(start);
        host.dispatch_turns(
            LIST_HOOK,
            TurnHost::None,
            &|| false,
            &|lua, _| {
                let data = lua.create_table()?;
                data.raw_set("types", slot.borrow().to_lua_value(lua)?)?;
                new_event(lua, data, &["types"])
            },
            &mut |id, event, _| {
                accept(&slot, id, LIST_HOOK, "types", &event.answer("types"));
                true
            },
        )
        .unwrap();
        slot.into_inner()
    }

    fn list_mod(root: &Path, id: &str, body: &str) -> LuaMod {
        load(
            root,
            id,
            &format!("horizon.on(\"{LIST_HOOK}\", function(event)\n{body}\nend)"),
            Side::Both,
        )
    }

    #[test]
    fn a_later_mod_reads_the_list_an_earlier_mod_assigned_and_the_assignment_replaces_it() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            list_mod(root.path(), "mod_a", "event.types = {\"a\", \"b\"}"),
            list_mod(
                root.path(),
                "mod_b",
                "local t = {}\nfor _, v in ipairs(event.types) do t[#t + 1] = v end\nt[#t + 1] = \"c\"\nevent.types = t",
            ),
        ]);
        assert_eq!(fire_list(&host, vec!["start".to_owned()]), ["a", "b", "c"]);
    }

    #[test]
    fn a_bad_list_is_refused_whole_and_the_previous_list_is_kept() {
        let root = tempfile::tempdir().unwrap();
        for bad in [
            "local t = {} for i = 1, 1025 do t[i] = \"x\" end event.types = t",
            "event.types = {[1] = \"a\", [3] = \"b\"}",
            "event.types = {\"a\", 2}",
        ] {
            let host = LuaHost::new(vec![list_mod(root.path(), "mod_a", bad)]);
            assert_eq!(fire_list(&host, vec!["kept".to_owned()]), ["kept"], "{bad}");
        }
    }

    #[test]
    fn a_list_of_exactly_the_cap_is_taken() {
        let root = tempfile::tempdir().unwrap();
        let body = "local t = {} for i = 1, 1024 do t[i] = \"x\" end event.types = t";
        let host = LuaHost::new(vec![list_mod(root.path(), "mod_a", body)]);
        assert_eq!(fire_list(&host, Vec::new()).len(), 1024);
    }
}

#[cfg(test)]
mod handler_gate_tests {
    use super::tests::load;
    use super::*;

    #[test]
    fn a_hook_is_handled_only_when_a_mod_declared_it() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![load(
            root.path(),
            "mod_a",
            "horizon.on(\"compute_fov\", function() end)",
            Side::Both,
        )]);
        assert!(host.has_handlers("compute_fov"));
        assert!(!host.has_handlers("block_step_on"), "no mod declared it");
        let none = LuaHost::new(Vec::new());
        assert!(!none.has_handlers("compute_fov"));
    }

    #[test]
    fn a_client_mod_does_not_handle_a_server_hook() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![load(
            root.path(),
            "mod_a",
            "horizon.on(\"block_receive_game\", function() end)",
            Side::Client,
        )]);
        assert!(!host.has_handlers("block_receive_game"));
    }
}

#[cfg(test)]
mod accept_by_tests {
    use std::cell::RefCell;

    use mlua::Value;

    use super::accept_by;

    #[test]
    fn an_interaction_result_answer_is_its_name_and_an_unknown_name_is_no_answer() {
        let lua = mlua::Lua::new();
        let name = |text: &str| Value::String(lua.create_string(text).unwrap());
        assert!(matches!(
            crate::dispatch::answer_of::<mod_api::InteractionResult>(
                "m",
                "use_without_item",
                &name("success_server")
            ),
            Some(mod_api::InteractionResult::SuccessServer)
        ));
        assert!(
            crate::dispatch::answer_of::<mod_api::InteractionResult>(
                "m",
                "use_without_item",
                &name("bogus")
            )
            .is_none()
        );
    }

    #[test]
    fn a_refused_value_keeps_the_previous_one_and_nil_is_no_answer() {
        let slot = RefCell::new(5u32);
        assert!(!accept_by(&slot, "m", "h", "o", &Value::Nil, |_| Ok(9)));
        assert_eq!(*slot.borrow(), 5, "nil assigns nothing");

        let refused = accept_by(&slot, "m", "h", "o", &Value::Boolean(true), |_| {
            Err("an unknown item id".to_owned())
        });
        assert!(refused);
        assert_eq!(*slot.borrow(), 5, "a refused value keeps the previous one");

        assert!(!accept_by(
            &slot,
            "m",
            "h",
            "o",
            &Value::Boolean(true),
            |_| Ok(7)
        ));
        assert_eq!(*slot.borrow(), 7);
    }
}
