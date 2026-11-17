//! Hook dispatch: one hook fired into every mod that declared it, each with its own event table.
//!
//! A handler gets ONE argument, the event. Its data fields are read-only: the host copies them in
//! when it builds the event, and the mod can read them but not change them. The mod's answers go
//! through a few writable keys (`cancel`, `result`, ...) named by the caller. The host reads them
//! back with `Event::answer` after each handler, and decides from them whether the chain goes on.
//!
//! The handler sees an empty proxy table with a locked metatable. Reads go to the answers the mod
//! has written, then to the data. Writes go through a host function that accepts only the writable
//! keys. The proxy is read-only too, so `rawset` cannot store a field on it and shadow the data.

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
        for m in self.mods() {
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
            new_event(lua, pos_data(lua), &["cancel", "result"])
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
                      event.cancel = true\n\
                      local ok, err = pcall(function() event.other = 1 end)\n\
                      horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
                      end)";
        let host = LuaHost::new(vec![load(root.path(), "mod_a", source, Side::Both)]);
        let mut seen = Vec::new();
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, pos_data(lua), &["cancel", "result"]),
            &mut |_, event| {
                seen.push((event.answer("cancel"), event.answer("other")));
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
        let a = "horizon.on(\"block_receive_game\", function(event) event.cancel = true; horizon.log(\"a1\") end)\n\
                 horizon.on(\"block_receive_game\", function(event) horizon.log(\"a2\") end)";
        let b = "horizon.on(\"block_receive_game\", function(event) horizon.log(\"b1\") end)";
        let mod_a = load(root.path(), "mod_a", a, Side::Both);
        let mod_b = load(root.path(), "mod_b", b, Side::Both);
        let host = LuaHost::new(vec![mod_a, mod_b]);
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, pos_data(lua), &["cancel"]),
            &mut |_, event| !matches!(event.answer("cancel"), Value::Boolean(true)),
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
}
