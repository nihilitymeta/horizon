//! The hand-written runtime of the generated hooks (`generated_hooks.rs`).
//! The generator emits one macro invocation per hook (a data table: the
//! hook's name, host kind, _event type, kind filter and field converters);
//! these macros expand it into the `Hooks` methods. Review this file once:
//! the generated file is data on top of it.
//!
//! The macros expand inside `impl Hooks for LuaMod` in `lua_mods.rs`, so
//! they use that module's private helpers (`qualify`, `player_by_uuid`,
//! `dimension_id`, `item_table_of`, `call_kind`, `run_handlers`...)
//! alongside the helpers below. Conventions the generator relies on: the
//! native _event binds as `_event`, the host as `host` (a read-only game host
//! as `_host`, a level as `level`), the state as `lua` and the table as
//! `table`.

use horizon_client::mod_api::ItemStack;
use mlua::{Lua, Table, Value};

/// `{{x, y, z}}` of a block position, as the hand-written hooks name it.
pub(crate) fn pos_table(lua: &Lua, pos: [i32; 3]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", pos[0])?;
    t.set("y", pos[1])?;
    t.set("z", pos[2])?;
    Ok(t)
}

/// `{{x, z}}` of a chunk or column, as the hand-written hooks name it.
pub(crate) fn chunk_table(lua: &Lua, chunk: [i32; 2]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", chunk[0])?;
    t.set("z", chunk[1])?;
    Ok(t)
}

/// `{{x, y}}` of a 2D float pair.
pub(crate) fn pair_table(lua: &Lua, pos: [f32; 2]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", pos[0])?;
    t.set("y", pos[1])?;
    Ok(t)
}

/// `{{x, y, z}}` of a block position table back into a position: the
/// handler writes numbers, whole or fractional parts dropped.
pub(crate) fn read_pos(t: &Table) -> mlua::Result<[i32; 3]> {
    Ok([t.get("x")?, t.get("y")?, t.get("z")?])
}

/// A number field back as a float: scripts write `16` or `16.0` alike.
pub(crate) fn read_number(t: &Table, key: &str) -> mlua::Result<f32> {
    match t.get::<Value>(key)? {
        Value::Integer(n) => Ok(n as f32),
        Value::Number(n) => Ok(n as f32),
        v => Err(mlua::Error::runtime(format!(
            "{key}: expected a number, got {}",
            v.type_name()
        ))),
    }
}

/// A `{{x, y}}` table back into a float pair.
pub(crate) fn read_pair(t: &Table) -> mlua::Result<[f32; 2]> {
    Ok([read_number(t, "x")?, read_number(t, "y")?])
}

/// An item stack from its table: `id`, `count`, `damage`. Extra components
/// do not survive: a replaced stack is plain.
pub(crate) fn item_from_table(table: &Table) -> mlua::Result<ItemStack> {
    Ok(ItemStack {
        item: std::sync::Arc::from(table.get::<String>("id")?.as_str()),
        count: table.get::<Option<u32>>("count")?.unwrap_or(1),
        damage: table.get::<Option<u32>>("damage")?.unwrap_or(0),
        extra: None,
    })
}

/// A string slice as a sequence table.
pub(crate) fn string_seq(lua: &Lua, items: &[String]) -> mlua::Result<Table> {
    lua.create_sequence_from(items.iter().map(String::as_str))
}

/// An int slice as a sequence table.
pub(crate) fn int_seq(lua: &Lua, items: &[i32]) -> mlua::Result<Table> {
    lua.create_sequence_from(items.iter().copied())
}

/// A string sequence table back into a vector.
pub(crate) fn read_string_seq(t: &Table) -> mlua::Result<Vec<String>> {
    t.sequence_values::<String>().collect()
}

/// A uuid (or its absence) as the text the hand-written hooks use.
pub(crate) fn uuid_value(lua: &Lua, uuid: Option<u128>) -> mlua::Result<Value> {
    match uuid {
        Some(uuid) => Ok(Value::String(lua.create_string(uuid_text(uuid))?)),
        None => Ok(Value::Nil),
    }
}

/// `text` as a UUID is written: `8-4-4-4-12` hex digits. (Mirrors the
/// private helper in `lua_mods.rs`, which the macros also call directly.)
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

/// How a generated hook finds the kind its kind-filtered handlers watch:
/// none, a `kind` string field of the _event, or an entity id the host
/// resolves (`host.entity`, then `host.entity_kind`; `None` when the entity
/// is gone, so only unfiltered handlers run).
macro_rules! gen_kind {
    ((none), $host:expr, $event:ident) => {
        None::<String>
    };
    ((field $f:ident), $host:expr, $event:ident) => {
        Some(qualify($event.$f))
    };
    ((entity $f:ident), $host:expr, $event:ident) => {
        u32::try_from($event.$f)
            .ok()
            .and_then(|id| $host.entity(id))
            .and_then(|entity| $host.entity_kind(entity))
    };
    ((opt_entity $f:ident), $host:expr, $event:ident) => {
        $event
            .$f
            .and_then(|id| u32::try_from(id).ok())
            .and_then(|id| $host.entity(id))
            .and_then(|entity| $host.entity_kind(entity))
    };
}

/// One _event-table field's seed: the value the handler is told. The host
/// (`$host`) and level (`$level`) are only named by the arms that read
/// through them; the others take `()` for those.
macro_rules! gen_seed {
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : copy) => {
        $table.set(
            stringify!($field),
            mlua::IntoLua::into_lua($event.$field, $lua)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : cloned) => {
        $table.set(
            stringify!($field),
            mlua::IntoLua::into_lua($event.$field.clone(), $lua)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : copy_ref) => {
        $table.set(
            stringify!($field),
            mlua::IntoLua::into_lua(*$event.$field, $lua)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : mut_string) => {
        $table.set(
            stringify!($field),
            mlua::IntoLua::into_lua($event.$field.as_str(), $lua)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : mut_opt_string) => {
        $table.set(
            stringify!($field),
            mlua::IntoLua::into_lua((*$event.$field).clone(), $lua)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : mut_pos) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::pos_table($lua, *$event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : mut_vec3) => {
        $table.set(stringify!($field), goals::vec3($lua, *$event.$field)?)?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : mut_pair) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::pair_table($lua, *$event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : mut_pair64) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::pair_table(
                $lua,
                [(*$event.$field)[0] as f32, (*$event.$field)[1] as f32],
            )?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : uuid) => {
        $table.set(stringify!($field), uuid_text($event.$field))?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : opt_uuid) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::uuid_value($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : player) => {
        $table.set(
            stringify!($field),
            $this.player_by_uuid($host, $event.player, Some($event.name))?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : player_opt) => {
        $table.set(
            stringify!($field),
            match $event.player {
                Some(player) => {
                    mlua::Value::Table($this.player_by_uuid($host, player, Some($event.name))?)
                }
                None => mlua::Value::Nil,
            },
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : dim) => {
        $table.set(stringify!($field), dimension_id($event.$field))?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : pos) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::pos_table($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : pos_struct) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::pos_table(
                $lua,
                [$event.$field.x, $event.$field.y, $event.$field.z],
            )?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : opt_pos) => {
        $table.set(
            stringify!($field),
            match $event.$field {
                Some(pos) => mlua::Value::Table(crate::generated_runtime::pos_table($lua, pos)?),
                None => mlua::Value::Nil,
            },
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : chunk) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::chunk_table($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : pair) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::pair_table($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : pair64) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::pair_table(
                $lua,
                [$event.$field[0] as f32, $event.$field[1] as f32],
            )?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : vec3) => {
        $table.set(stringify!($field), goals::vec3($lua, $event.$field)?)?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : int_seq) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::int_seq($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : str_seq) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::string_seq($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : item) => {
        $table.set(stringify!($field), item_table_of($lua, $event.$field)?)?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : opt_item) => {
        $table.set(
            stringify!($field),
            match $event.$field {
                Some(stack) => mlua::Value::Table(item_table_of($lua, stack)?),
                None => mlua::Value::Nil,
            },
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : state_level) => {
        $table.set(
            stringify!($field),
            $level
                .state_block($event.$field)
                .unwrap_or_else(|| "minecraft:air".to_owned()),
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : state_game) => {
        $table.set(stringify!($field), $host.state_name($event.$field))?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : block_at_pos) => {
        $table.set(
            stringify!($field),
            $level
                .state_block($level.block_state([$event.pos.x, $event.pos.y, $event.pos.z]))
                .unwrap_or_else(|| "minecraft:air".to_owned()),
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : block_from_state) => {
        $table.set(
            stringify!($field),
            $level
                .state_block($event.state)
                .unwrap_or_else(|| "minecraft:air".to_owned()),
        )?
    };
}

/// One _event-table field's write-back: what the handler changed, read into
/// the native _event after the handlers ran. Fields transcribed without a
/// write tag expand to nothing.
macro_rules! gen_writeback {
    ($table:ident, $lua:ident, $event:ident, $field:ident) => {};
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_scalar) => {
        $event.$field =
            mlua::FromLua::from_lua($table.get::<mlua::Value>(stringify!($field))?, $lua)?
    };
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_scalar_ref) => {
        *$event.$field =
            mlua::FromLua::from_lua($table.get::<mlua::Value>(stringify!($field))?, $lua)?
    };
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_string) => {
        *$event.$field =
            mlua::FromLua::from_lua($table.get::<mlua::Value>(stringify!($field))?, $lua)?
    };
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_opt_string) => {
        *$event.$field =
            mlua::FromLua::from_lua($table.get::<mlua::Value>(stringify!($field))?, $lua)?
    };
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_pos) => {
        *$event.$field = crate::generated_runtime::read_pos(&$table.get(stringify!($field))?)?
    };
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_vec3) => {
        *$event.$field = {
            let updated: mlua::Table = $table.get(stringify!($field))?;
            [
                updated.get::<f64>("x")?,
                updated.get::<f64>("y")?,
                updated.get::<f64>("z")?,
            ]
        }
    };
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_pair) => {
        *$event.$field = crate::generated_runtime::read_pair(&$table.get(stringify!($field))?)?
    };
    ($table:ident, $lua:ident, $event:ident, $field:ident, mut_strvec) => {
        *$event.$field =
            crate::generated_runtime::read_string_seq(&$table.get(stringify!($field))?)?
    };
}

/// A `listen` hook: one _event table per hook under the native name; what
/// the handler leaves in mutable fields is kept.
macro_rules! gen_listen {
    ($name:ident, none, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(&self, _event: &mut $event) {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return;
                }
            } else if !self.has_handlers(hook) {
                return;
            }
            let outcome = (|| -> mlua::Result<()> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), (), _event, $field : $seed);)*
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        self.warn(format!("{hook}: {action} needs a host; dropped"));
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, game_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(&self, host: &mut dyn horizon_client::mod_api::Host, _event: &mut $event) {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return;
            }
            let kind: Option<String> = gen_kind!($kind, host, _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return;
                }
            } else if !self.has_handlers(hook) {
                return;
            }
            let outcome = (|| -> mlua::Result<()> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, host, (), _event, $field : $seed);)*
                self.call_kind(
                    hook,
                    kind.as_deref(),
                    host,
                    table.clone(),
                    &mut |host, action, args| {
                        self.run_action(host, action, args)?;
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, game_ref, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(&self, _host: &dyn horizon_client::mod_api::Host, _event: &mut $event) {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return;
            }
            let kind: Option<String> = gen_kind!($kind, _host, _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return;
                }
            } else if !self.has_handlers(hook) {
                return;
            }
            let outcome = (|| -> mlua::Result<()> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, _host, (), _event, $field : $seed);)*
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        self.warn(format!("{hook}: {action} needs a host; dropped"));
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, level, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(&self, level: &dyn horizon_client::mod_api::LevelHost, _event: &mut $event) {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return;
                }
            } else if !self.has_handlers(hook) {
                return;
            }
            let outcome = (|| -> mlua::Result<()> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed);)*
                let reader = Reader::Level(level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, level_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(
            &self,
            level: &mut dyn horizon_client::mod_api::LevelRandomHost,
            _event: &mut $event,
        ) {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return;
                }
            } else if !self.has_handlers(hook) {
                return;
            }
            let outcome = (|| -> mlua::Result<()> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed);)*
                let reader = Reader::Level(&*level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
}

/// A `collect` hook: like a `listen` hook whose out field the handlers edit
/// in turn, written back after the call.
macro_rules! gen_collect {
    ($($tokens:tt)*) => {
        gen_listen!($($tokens)*);
    };
}

/// A `cancel` hook: `horizon.cancel()` in the handler becomes `Flow::Cancel`.
macro_rules! gen_cancel {
    ($name:ident, none, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(&self, _event: &mut $event) -> horizon_client::mod_api::Flow {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return horizon_client::mod_api::Flow::Continue;
                }
            } else if !self.has_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let mut cancel = false;
            let outcome = (|| -> mlua::Result<bool> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), (), _event, $field : $seed);)*
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        match action {
                            "cancel" => cancel = true,
                            _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                        }
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, game_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(
            &self,
            host: &mut dyn horizon_client::mod_api::Host,
            _event: &mut $event,
        ) -> horizon_client::mod_api::Flow {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let kind: Option<String> = gen_kind!($kind, host, _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return horizon_client::mod_api::Flow::Continue;
                }
            } else if !self.has_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let mut cancel = false;
            let outcome = (|| -> mlua::Result<bool> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, host, (), _event, $field : $seed);)*
                self.call_kind(
                    hook,
                    kind.as_deref(),
                    host,
                    table.clone(),
                    &mut |host, action, _args| {
                        match action {
                            "cancel" => cancel = true,
                            _ => self.run_action(host, action, _args)?,
                        }
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, game_ref, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(
            &self,
            _host: &dyn horizon_client::mod_api::Host,
            _event: &mut $event,
        ) -> horizon_client::mod_api::Flow {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let kind: Option<String> = gen_kind!($kind, _host, _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return horizon_client::mod_api::Flow::Continue;
                }
            } else if !self.has_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let mut cancel = false;
            let outcome = (|| -> mlua::Result<bool> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, _host, (), _event, $field : $seed);)*
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        match action {
                            "cancel" => cancel = true,
                            _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                        }
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, level, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(
            &self,
            level: &dyn horizon_client::mod_api::LevelHost,
            _event: &mut $event,
        ) -> horizon_client::mod_api::Flow {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return horizon_client::mod_api::Flow::Continue;
                }
            } else if !self.has_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let mut cancel = false;
            let outcome = (|| -> mlua::Result<bool> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed);)*
                let reader = Reader::Level(level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        "cancel" => cancel = true,
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, level_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?]) => {
        fn $name(
            &self,
            level: &mut dyn horizon_client::mod_api::LevelRandomHost,
            _event: &mut $event,
        ) -> horizon_client::mod_api::Flow {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return horizon_client::mod_api::Flow::Continue;
                }
            } else if !self.has_handlers(hook) {
                return horizon_client::mod_api::Flow::Continue;
            }
            let mut cancel = false;
            let outcome = (|| -> mlua::Result<bool> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed);)*
                let reader = Reader::Level(&*level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        "cancel" => cancel = true,
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
}

/// The replace out value back: `nil` reads as `None`, anything else as
/// `Some` (a table through the shape's own reader). `ret` is the trait
/// method's exact return type.
macro_rules! gen_replace_out {
    ($table:ident, $lua:ident, $out:ident : value) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            v => Some(mlua::FromLua::from_lua(v, $lua)?),
        })
    };
    ($table:ident, $lua:ident, $out:ident : pair) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::Table(t) => Some(crate::generated_runtime::read_pair(&t)?),
            _ => return Err(mlua::Error::runtime("expected a table or nil")),
        })
    };
    ($table:ident, $lua:ident, $out:ident : item) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::Table(t) => Some(crate::generated_runtime::item_from_table(&t)?),
            _ => return Err(mlua::Error::runtime("expected a table or nil")),
        })
    };
}

/// A `replace` hook: the trait method is not told the current value, so the
/// out field seeds as `nil`; the handler's value for it becomes `Some`.
macro_rules! gen_replace {
    ($name:ident, none, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?], $out:ident : $read:ident, $ret:ty) => {
        fn $name(&self, _event: &mut $event) -> $ret {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return None;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return None;
                }
            } else if !self.has_handlers(hook) {
                return None;
            }
            let outcome = (|| -> mlua::Result<$ret> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), (), _event, $field : $seed);)*
                table.set(stringify!($out), mlua::Value::Nil)?;
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        self.warn(format!("{hook}: {action} needs a host; dropped"));
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                gen_replace_out!(table, lua, $out : $read)
            })();
            match outcome {
                Ok(value) => value,
                Err(e) => {
                    self.error(format!("{hook}: {e}"));
                    None
                }
            }
        }
    };
    ($name:ident, game_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?], $out:ident : $read:ident, $ret:ty) => {
        fn $name(
            &self,
            host: &mut dyn horizon_client::mod_api::Host,
            _event: &mut $event,
        ) -> $ret {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return None;
            }
            let kind: Option<String> = gen_kind!($kind, host, _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return None;
                }
            } else if !self.has_handlers(hook) {
                return None;
            }
            let outcome = (|| -> mlua::Result<$ret> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, host, (), _event, $field : $seed);)*
                table.set(stringify!($out), mlua::Value::Nil)?;
                self.call_kind(
                    hook,
                    kind.as_deref(),
                    host,
                    table.clone(),
                    &mut |host, action, args| {
                        self.run_action(host, action, args)?;
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                gen_replace_out!(table, lua, $out : $read)
            })();
            match outcome {
                Ok(value) => value,
                Err(e) => {
                    self.error(format!("{hook}: {e}"));
                    None
                }
            }
        }
    };
    ($name:ident, game_ref, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?], $out:ident : $read:ident, $ret:ty) => {
        fn $name(
            &self,
            _host: &dyn horizon_client::mod_api::Host,
            _event: &mut $event,
        ) -> $ret {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return None;
            }
            let kind: Option<String> = gen_kind!($kind, _host, _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return None;
                }
            } else if !self.has_handlers(hook) {
                return None;
            }
            let outcome = (|| -> mlua::Result<$ret> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, _host, (), _event, $field : $seed);)*
                table.set(stringify!($out), mlua::Value::Nil)?;
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        self.warn(format!("{hook}: {action} needs a host; dropped"));
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                gen_replace_out!(table, lua, $out : $read)
            })();
            match outcome {
                Ok(value) => value,
                Err(e) => {
                    self.error(format!("{hook}: {e}"));
                    None
                }
            }
        }
    };
    ($name:ident, level, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?], $out:ident : $read:ident, $ret:ty) => {
        fn $name(
            &self,
            level: &dyn horizon_client::mod_api::LevelHost,
            _event: &mut $event,
        ) -> $ret {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return None;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return None;
                }
            } else if !self.has_handlers(hook) {
                return None;
            }
            let outcome = (|| -> mlua::Result<$ret> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed);)*
                table.set(stringify!($out), mlua::Value::Nil)?;
                let reader = Reader::Level(level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                gen_replace_out!(table, lua, $out : $read)
            })();
            match outcome {
                Ok(value) => value,
                Err(e) => {
                    self.error(format!("{hook}: {e}"));
                    None
                }
            }
        }
    };
    ($name:ident, level_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(=> $write:ident)?),* $(,)?], $out:ident : $read:ident, $ret:ty) => {
        fn $name(
            &self,
            level: &mut dyn horizon_client::mod_api::LevelRandomHost,
            _event: &mut $event,
        ) -> $ret {
            let hook = stringify!($name);
            if !self.has_any_handlers(hook) {
                return None;
            }
            let kind: Option<String> = gen_kind!($kind, (), _event);
            if let Some(kind) = kind.as_ref() {
                if !self.watches(hook, kind) {
                    return None;
                }
            } else if !self.has_handlers(hook) {
                return None;
            }
            let outcome = (|| -> mlua::Result<$ret> {
                let lua = &self.lua;
                let table = lua.create_table()?;
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed);)*
                table.set(stringify!($out), mlua::Value::Nil)?;
                let reader = Reader::Level(&*level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, $field $(, $write)?);)*
                gen_replace_out!(table, lua, $out : $read)
            })();
            match outcome {
                Ok(value) => value,
                Err(e) => {
                    self.error(format!("{hook}: {e}"));
                    None
                }
            }
        }
    };
}
