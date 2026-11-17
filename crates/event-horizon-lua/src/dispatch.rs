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
use mlua::{Error, Lua, Table, Value};

use crate::declare;
use crate::discover::Side;
use crate::generated::hooks::{HookSide, hook_info};
use crate::host::{LOG_TARGET, LuaHost, call_handler};

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

/// Takes the value a mod wrote to one output key, if it wrote one (`nil` is no answer). A value that
/// does not convert to the output's type is refused: the slot keeps its previous value, and one
/// warning names the mod, the hook and the output. Other outputs of the same turn are not affected.
/// Returns whether the value was refused.
pub fn accept<T: FromLuaValue>(
    slot: &RefCell<T>,
    id: &str,
    hook: &str,
    output: &str,
    value: &Value,
) -> bool {
    if matches!(value, Value::Nil) {
        return false;
    }
    match T::from_lua_value(value) {
        Ok(next) => {
            *slot.borrow_mut() = next;
            false
        }
        Err(err) => {
            log::warn!(
                target: LOG_TARGET,
                "{id}: {hook}: output `{output}` refused, the previous value is kept: {err}"
            );
            true
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
    /// their budgets. `build` makes the mod's event after the earlier mods committed, so the mod
    /// sees their accepted values. When the turn is over and none of its handlers failed, `commit`
    /// takes the answers (the output keys the mod wrote, read with `Event::answer`); a turn with a
    /// failing handler is void and is not committed. `commit` returns whether the chain goes on.
    pub fn dispatch_turns(
        &self,
        hook: &str,
        is_cancelled: &dyn Fn() -> bool,
        build: &dyn Fn(&Lua) -> mlua::Result<Event>,
        commit: &mut dyn FnMut(&str, &Event) -> bool,
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
            let mut failed = false;
            for handler in &handlers {
                if let Err(err) = call_handler(m.lua(), handler, event.table()) {
                    log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                    failed = true;
                }
            }
            if failed {
                log::warn!(
                    target: LOG_TARGET,
                    "{}: {hook}: a handler failed, so this mod's changes to the event are dropped",
                    m.id()
                );
                continue;
            }
            if !commit(m.id(), &event) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{Discovered, ENTRY};
    use crate::host::{LuaHost, LuaMod};

    /// A server hook the generated impl has, with a cancel answer: the tests fire it by name.
    const HOOK: &str = "block_receive_game";
    use std::cell::Cell;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// Loads a mod named `id` whose main.luau is `source`.
    fn load(root: &Path, id: &str, source: &str, side: Side) -> LuaMod {
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
            &|| false,
            &|lua| fov_event(lua, *slot.borrow()),
            &mut |id, event| {
                if accept(&slot, id, FOV, "degrees", &event.answer("degrees")) {
                    refused.set(refused.get() + 1);
                }
                true
            },
        );
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
}
