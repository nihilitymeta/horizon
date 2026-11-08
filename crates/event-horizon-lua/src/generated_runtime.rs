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

use crate::lua_mods::item_table_of;
use horizon_client::mod_api::ItemStack;
use mlua::{Lua, Table, Value};

/// Round 3 (`adapter`) converters: one [`LuaStruct`] per native field type
/// the classifier maps to `Conv::Struct`, plus the small scalar shapes
/// (`Duration`, paths, addresses, bytes, text, the pre-login gate) the
/// generator transcribes as their own seed/write tags. The generator only
/// names these; every shape lives here, reviewed once.
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

/// Resolve a placement answer: the named block must be the event state's
/// own block; each `[prop=value]` applies through the level.
pub(crate) fn placement_answer(
    level: &dyn horizon_client::mod_api::LevelHost,
    state: horizon_client::mod_api::BlockStateId,
    name: &str,
    field: &str,
) -> mlua::Result<horizon_client::mod_api::BlockStateId> {
    let (block, props) = match name.split_once('[') {
        Some((block, rest)) => (block, rest.strip_suffix(']').unwrap_or(rest)),
        None => (name, ""),
    };
    match level.state_block(state) {
        Some(own) if own == block => {}
        _ => {
            return Err(mlua::Error::runtime(format!(
                "{field}: answer {name:?} is not this placement's block"
            )));
        }
    }
    let mut current = state;
    for prop in props.split(',').filter(|p| !p.is_empty()) {
        let (key, value) = prop.split_once('=').ok_or_else(|| {
            mlua::Error::runtime(format!("{field}: bad property {prop:?} in {name:?}"))
        })?;
        current = level
            .state_with_property(current, key, value)
            .ok_or_else(|| {
                mlua::Error::runtime(format!(
                    "{field}: {block} has no property {key:?}={value:?}"
                ))
            })?;
    }
    Ok(current)
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
pub(crate) fn string_seq<S: AsRef<str>>(lua: &Lua, items: &[S]) -> mlua::Result<Table> {
    lua.create_sequence_from(items.iter().map(|s| s.as_ref()))
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
        )?;
        $table.set(
            concat!(stringify!($field), "_props"),
            crate::generated_runtime::props_of($lua, $level.state_properties($event.$field))?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : state_game) => {
        let (id, props) =
            crate::generated_runtime::split_state_name($lua, &$host.state_name($event.$field))?;
        $table.set(stringify!($field), id)?;
        $table.set(concat!(stringify!($field), "_props"), props)?
    };
    // Seed-only notes: the state arms above already seeded these.
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : state_props_note) => {};
    // A `LuaStruct` by value, by shared borrow, by exclusive borrow, optional,
    // in a `&mut Vec` (collect), or in a shared slice.
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_ref($t:ty)) => {
        $table.set(
            stringify!($field),
            <$t as crate::generated_runtime::LuaStruct>::to_value($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_val($t:ty)) => {
        $table.set(
            stringify!($field),
            <$t as crate::generated_runtime::LuaStruct>::to_value($lua, &$event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_mut($t:ty)) => {
        $table.set(
            stringify!($field),
            <$t as crate::generated_runtime::LuaStruct>::to_value($lua, &*$event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_opt($t:ty)) => {
        $table.set(
            stringify!($field),
            match &$event.$field {
                Some(v) => <$t as crate::generated_runtime::LuaStruct>::to_value($lua, v)?,
                None => mlua::Value::Nil,
            },
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_vec($t:ty)) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::struct_vec_to_lua($lua, &$event.$field[..])?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_slice($t:ty)) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::struct_vec_to_lua($lua, $event.$field)?,
        )?
    };
    // Bytes as a Lua string (Luau strings are byte-safe).
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_bytes) => {
        $table.set(stringify!($field), $lua.create_string($event.$field)?)?
    };
    // A path as text.
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_path) => {
        $table.set(
            stringify!($field),
            $event.$field.to_string_lossy().into_owned(),
        )?
    };
    // Rich text: the plain string and the component table.
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_text) => {{
        let (plain, component) = crate::generated_runtime::text_parts($lua, &*$event.$field)?;
        $table.set(stringify!($field), plain)?;
        $table.set(concat!(stringify!($field), "_component"), component)?;
    }};
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_opt_text) => {{
        match &*$event.$field {
            Some(text) => {
                let (plain, component) = crate::generated_runtime::text_parts($lua, text)?;
                $table.set(stringify!($field), plain)?;
                $table.set(concat!(stringify!($field), "_component"), component)?;
            }
            None => {
                $table.set(stringify!($field), mlua::Value::Nil)?;
                $table.set(concat!(stringify!($field), "_component"), mlua::Value::Nil)?;
            }
        }
    }};
    // The pre-login gate: how many answers are pending; the handler refuses
    // by setting `refuse`.
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_defer) => {{
        let gate = $lua.create_table()?;
        gate.set("count", $event.$field.len())?;
        $table.set(stringify!($field), gate)?;
    }};
    // Lifetime-carrying structs seed through their own arms (the macro
    // cannot name the elided lifetime in a type parameter).
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_mapinfo) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::LuaStruct::to_value($lua, $event.$field)?,
        )?
    };
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_notechange) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::LuaStruct::to_value($lua, $event.$field)?,
        )?
    };
    // A shared `&SoundPlayback`: the field already is the `&Self`
    // `to_value` takes, so unlike `s_ref` this seeds without borrowing.
    ($this:expr, $table:ident, $lua:ident, $host:expr, $level:expr, $event:ident, $field:ident : s_sound) => {
        $table.set(
            stringify!($field),
            crate::generated_runtime::LuaStruct::to_value($lua, $event.$field)?,
        )?
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
/// write tag expand to nothing. A `nil` where the native type is not optional
/// leaves the native value unchanged.
macro_rules! gen_writeback {
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident) => {};
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_scalar) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            v => $event.$field = mlua::FromLua::from_lua(v, $lua)?,
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_scalar_ref) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            v => *$event.$field = mlua::FromLua::from_lua(v, $lua)?,
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_string) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            v => *$event.$field = mlua::FromLua::from_lua(v, $lua)?,
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_opt_string) => {
        *$event.$field =
            mlua::FromLua::from_lua($table.get::<mlua::Value>(stringify!($field))?, $lua)?
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_pos) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            mlua::Value::Table(updated) => {
                *$event.$field = crate::generated_runtime::read_pos(&updated)?
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected {{x, y, z}}, got {}",
                    stringify!($field),
                    v.type_name()
                )));
            }
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_vec3) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            mlua::Value::Table(updated) => {
                *$event.$field = [
                    updated.get::<f64>("x")?,
                    updated.get::<f64>("y")?,
                    updated.get::<f64>("z")?,
                ]
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected {{x, y, z}}, got {}",
                    stringify!($field),
                    v.type_name()
                )));
            }
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_pair) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            mlua::Value::Table(updated) => {
                *$event.$field = crate::generated_runtime::read_pair(&updated)?
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected {{x, y}}, got {}",
                    stringify!($field),
                    v.type_name()
                )));
            }
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_strvec) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            mlua::Value::Table(updated) => {
                *$event.$field = crate::generated_runtime::read_string_seq(&updated)?
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected a sequence, got {}",
                    stringify!($field),
                    v.type_name()
                )));
            }
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, mut_strvec_static) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            mlua::Value::Table(updated) => {
                let owned = crate::generated_runtime::read_string_seq(&updated)?;
                *$event.$field = owned
                    .iter()
                    .map(|s| crate::generated_runtime::static_str(s))
                    .collect();
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected a sequence, got {}",
                    stringify!($field),
                    v.type_name()
                )));
            }
        }
    };
    // A `LuaStruct` in place, optional, or in a `&mut Vec` (collect): `nil`
    // where the type is not optional leaves the value unchanged.
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, w_mut($t:ty)) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            v => {
                *$event.$field = <$t as crate::generated_runtime::LuaStruct>::from_value(
                    $lua,
                    v,
                    stringify!($field),
                )?
            }
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, w_optmut($t:ty)) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => *$event.$field = None,
            v => {
                *$event.$field = Some(<$t as crate::generated_runtime::LuaStruct>::from_value(
                    $lua,
                    v,
                    stringify!($field),
                )?)
            }
        }
    };
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, w_vec($t:ty)) => {
        match $table.get::<mlua::Value>(stringify!($field))? {
            mlua::Value::Nil => {}
            mlua::Value::Table(updated) => {
                *$event.$field = crate::generated_runtime::struct_vec_from_lua(
                    $lua,
                    &updated,
                    stringify!($field),
                )?
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected a sequence, got {}",
                    stringify!($field),
                    v.type_name()
                )));
            }
        }
    };
    // Rich text: both missing is unchanged; a component table wins.
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, w_text) => {{
        let text: mlua::Value = $table.get(stringify!($field))?;
        let component: mlua::Value = $table.get(concat!(stringify!($field), "_component"))?;
        match (&text, &component) {
            (mlua::Value::Nil, mlua::Value::Nil) => {}
            _ => {
                *$event.$field = crate::generated_runtime::text_from_parts(
                    $lua,
                    text,
                    component,
                    stringify!($field),
                )?
            }
        }
    }};
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, w_opt_text) => {{
        let text: mlua::Value = $table.get(stringify!($field))?;
        let component: mlua::Value = $table.get(concat!(stringify!($field), "_component"))?;
        match (&text, &component) {
            (mlua::Value::Nil, mlua::Value::Nil) => *$event.$field = None,
            _ => {
                *$event.$field = Some(crate::generated_runtime::text_from_parts(
                    $lua,
                    text,
                    component,
                    stringify!($field),
                )?)
            }
        }
    }};
    // The pre-login gate: a `refuse` string or component table answers every
    // pending gate at once with that reason.
    ($table:ident, $lua:ident, $event:ident, $level:expr, $field:ident, w_defer) => {{
        let gate: mlua::Table = $table.get(stringify!($field))?;
        match gate.get::<mlua::Value>("refuse")? {
            mlua::Value::Nil => {}
            refuse => {
                let reason = crate::generated_runtime::text_from_parts(
                    $lua,
                    refuse,
                    mlua::Value::Nil,
                    concat!(stringify!($field), ".refuse"),
                )?;
                let (tx, rx) = std::sync::mpsc::channel();
                let _ = tx.send(Err(reason));
                $event.$field.push(rx);
            }
        }
    }};
}

/// A `listen` hook: one _event table per hook under the native name; what
/// the handler leaves in mutable fields is kept.
macro_rules! gen_listen {
    ($name:ident, none, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, (), (), _event, $field : $seed $(($st))?);)*
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        self.warn(format!("{hook}: {action} needs a host; dropped"));
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, game_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, host, (), _event, $field : $seed $(($st))?);)*
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
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, game_ref, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, _host, (), _event, $field : $seed $(($st))?);)*
                self.call_bare(
                    hook,
                    kind.as_deref(),
                    table.clone(),
                    &mut |action, _args| {
                        self.warn(format!("{hook}: {action} needs a host; dropped"));
                        Ok(())
                    },
                )?;
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, level, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed $(($st))?);)*
                let reader = Reader::Level(level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, level, $field $(, $write $(($wt))?)?);)*
                Ok(())
            })();
            if let Err(e) = outcome {
                self.error(format!("{hook}: {e}"));
            }
        }
    };
    ($name:ident, level_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed $(($st))?);)*
                let reader = Reader::Level(&*level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, level, $field $(, $write $(($wt))?)?);)*
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
    ($name:ident, none, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, (), (), _event, $field : $seed $(($st))?);)*
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
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, game_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, host, (), _event, $field : $seed $(($st))?);)*
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
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, game_ref, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, _host, (), _event, $field : $seed $(($st))?);)*
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
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, level, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed $(($st))?);)*
                let reader = Reader::Level(level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        "cancel" => cancel = true,
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, level, $field $(, $write $(($wt))?)?);)*
                Ok(cancel)
            })();
            match outcome {
                Ok(true) => horizon_client::mod_api::Flow::Cancel,
                Ok(false) => horizon_client::mod_api::Flow::Continue,
                Err(e) => self.hook_error(hook, e),
            }
        }
    };
    ($name:ident, level_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?]) => {
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
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed $(($st))?);)*
                let reader = Reader::Level(&*level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        "cancel" => cancel = true,
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, level, $field $(, $write $(($wt))?)?);)*
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

/// A native field type as a Lua value and back. Structs become tables of
/// their fields by Rust name (nested by these same rules); fieldless enums
/// become lowercase name strings; tuples become sequences in order. `field`
/// names the event-table field, so a wrong shape errors naming it.
pub trait LuaStruct: Sized {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value>;
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self>;
}

/// A required table key back through its own shape; a wrong shape errors
/// naming the event field and key.
pub(crate) fn req_key<T: mlua::FromLua>(t: &Table, key: &str, field: &str) -> mlua::Result<T> {
    t.get::<T>(key).map_err(|_| {
        mlua::Error::runtime(format!("{field}.{key}: expected a different shape here"))
    })
}

/// A required nested table: `{field}.{key}` must be a table for `from_value`.
pub(crate) fn req_table(
    _lua: &Lua,
    t: &Table,
    key: &str,
    field: &str,
) -> mlua::Result<(Table, String)> {
    match t.get::<Value>(key)? {
        Value::Table(sub) => Ok((sub, format!("{field}.{key}"))),
        _ => Err(mlua::Error::runtime(format!(
            "{field}.{key}: expected a table"
        ))),
    }
}

/// A `&str` as its Lua string.
fn str_value(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(s)?))
}

/// A Lua string back; anything else errors naming the field.
pub(crate) fn lua_string(v: Value, field: &str) -> mlua::Result<String> {
    match v {
        Value::String(s) => Ok(s.to_string_lossy()),
        other => Err(mlua::Error::runtime(format!(
            "{field}: expected a string, got {}",
            other.type_name()
        ))),
    }
}

/// One interned copy of each distinct string the scripts hand over where the
/// engine needs `&'static str` (command names, emissive layers): native mods
/// are trusted code, and the set of names is small and stable.
pub(crate) fn static_str(s: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static INTERNED: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut guard = INTERNED.lock().unwrap_or_else(|e| e.into_inner());
    let set = guard.get_or_insert_with(HashSet::new);
    if let Some(found) = set.get(s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    set.insert(leaked);
    leaked
}

/// A sequence table of converted elements; a `nil` element has no Lua form
/// and reads as `false`, which reads back as `nil`.
pub(crate) fn struct_vec_to_lua<T: LuaStruct>(lua: &Lua, items: &[T]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (i, item) in items.iter().enumerate() {
        let v = T::to_value(lua, item)?;
        match v {
            Value::Nil => t.set(i + 1, false)?,
            v => t.set(i + 1, v)?,
        }
    }
    Ok(t)
}

/// A sequence table back into elements; `false` reads as `nil`.
pub(crate) fn struct_vec_from_lua<T: LuaStruct>(
    lua: &Lua,
    t: &Table,
    field: &str,
) -> mlua::Result<Vec<T>> {
    let mut out = Vec::new();
    for pair in t.sequence_values::<Value>() {
        let v = pair?;
        let v = match v {
            Value::Boolean(false) => Value::Nil,
            v => v,
        };
        out.push(T::from_value(lua, v, field)?);
    }
    Ok(out)
}

/// A fieldless enum from its lowercase name; a wrong name errors listing the
/// valid ones, as the handler's error (through `hook_error` like other
/// handler errors).
fn enum_from<'a>(v: Value, field: &str, valid: &[&'a str]) -> mlua::Result<&'a str> {
    match v {
        Value::String(s) => {
            let name = s.to_string_lossy();
            valid
                .iter()
                .find(|n| ***n == *name)
                .copied()
                .ok_or_else(|| {
                    let mut names: Vec<&str> = valid.to_vec();
                    names.sort();
                    mlua::Error::runtime(format!(
                        "{field}: expected one of {}, got {name:?}",
                        names
                            .iter()
                            .map(|n| format!("{n:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })
        }
        other => Err(mlua::Error::runtime(format!(
            "{field}: expected a name, got {}",
            other.type_name()
        ))),
    }
}

use horizon_client::mod_api::{Compound, Direction, Hand, Text};
use horizon_world::consumable::UseAnimation;
use std::time::Duration;

/// A tooltip line: `{text, component, count}`.
impl LuaStruct for (Text, u32) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let (plain, component) = text_parts(lua, &v.0)?;
        let t = lua.create_table()?;
        t.set("text", plain)?;
        t.set("component", component)?;
        t.set("count", v.1)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let component: Value = t.get("component")?;
                let text: Value = t.get("text")?;
                Ok((
                    text_from_parts(lua, text, component, field)?,
                    req_key::<u32>(&t, "count", field)?,
                ))
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a tooltip line, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A bar value (`{value, max}` in order).
impl LuaStruct for (i32, u32) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("value", v.0)?;
        t.set("max", v.1)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok((
                req_key::<i32>(&t, "value", field)?,
                req_key::<u32>(&t, "max", field)?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{value, max}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Fire odds (`{min, max}` in order).
impl LuaStruct for (i32, i32) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("min", v.0)?;
        t.set("max", v.1)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok((
                req_key::<i32>(&t, "min", field)?,
                req_key::<i32>(&t, "max", field)?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{min, max}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A dispenser answer: the remainder (`nil` empties) and the level event.
impl LuaStruct for (Option<ItemStack>, i32) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("remainder", Option::<ItemStack>::to_value(lua, &v.0)?)?;
        t.set("event", v.1)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let remainder: Value = t.get("remainder")?;
                Ok((
                    Option::<ItemStack>::from_value(lua, remainder, field)?,
                    req_key::<i32>(&t, "event", field)?,
                ))
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{remainder, event}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A named tag (`{name, tag}` in order).
/// A named blob: `{name, tag}`, for block-entity data answers.
pub(crate) type NamedTag = (String, Compound);

impl LuaStruct for (String, Compound) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("name", v.0.as_str())?;
        t.set("tag", Compound::to_value(lua, &v.1)?)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (tag, at) = req_table(lua, &t, "tag", field)?;
                Ok((
                    req_key::<String>(&t, "name", field)?,
                    Compound::from_value(lua, Value::Table(tag), &at)?,
                ))
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{name, tag}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A reload file (`{path, data}` in order).
impl LuaStruct for (String, serde_json::Value) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("path", v.0.as_str())?;
        t.set("data", serde_json::Value::to_value(lua, &v.1)?)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok((
                req_key::<String>(&t, "path", field)?,
                serde_json::Value::from_value(lua, t.get::<Value>("data")?, field)?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{path, data}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// An emissive layer (`{a, b}` in order); written back interned.
impl LuaStruct for (&'static str, &'static str) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("a", v.0)?;
        t.set("b", v.1)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let a: Value = t.get("a")?;
                let b: Value = t.get("b")?;
                Ok((
                    <&'static str>::from_value(_lua, a, field)?,
                    <&'static str>::from_value(_lua, b, field)?,
                ))
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{a, b}}, got {}",
                other.type_name()
            ))),
        }
    }
}

use horizon_client::merchant::ItemCost;
use horizon_client::mod_api::{
    Corner, MerchantOffer, Music, MusicLayer, NativeConfig, NativeConfigFile, NativeConfigKind,
    NativeDataMapType, NativeFluidAction, NativeFluidDrain, NativeFluidStack, NativeFluidTankView,
    NativeMapDecoration, NativeMapInfo, NativeMinimap, NativeMod, NativeNoteBlockChange,
    NativeScreen, NativeZoom, RecipeBookCategory, SpawnEntry, Suggestion,
};

impl LuaStruct for Corner {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(lua, v.name())
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(
            match enum_from(
                v,
                field,
                &["top_left", "top_right", "bottom_left", "bottom_right"],
            )? {
                "top_left" => Corner::TopLeft,
                "top_right" => Corner::TopRight,
                "bottom_left" => Corner::BottomLeft,
                _ => Corner::BottomRight,
            },
        )
    }
}

impl LuaStruct for NativeFluidAction {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(
            lua,
            match v {
                NativeFluidAction::Simulate => "simulate",
                NativeFluidAction::Execute => "execute",
                _ => "simulate",
            },
        )
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(match enum_from(v, field, &["simulate", "execute"])? {
            "simulate" => NativeFluidAction::Simulate,
            _ => NativeFluidAction::Execute,
        })
    }
}

impl LuaStruct for NativeConfigKind {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(
            lua,
            match v {
                NativeConfigKind::Client => "client",
                NativeConfigKind::Common => "common",
                NativeConfigKind::Server => "server",
            },
        )
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(
            match enum_from(v, field, &["client", "common", "server"])? {
                "client" => NativeConfigKind::Client,
                "server" => NativeConfigKind::Server,
                "common" => NativeConfigKind::Common,
                _ => unreachable!("enum_from only returns listed names"),
            },
        )
    }
}

use horizon_client::mod_api::{BlockPos, LevelDimension};

impl LuaStruct for BlockPos {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("x", v.x)?;
        t.set("y", v.y)?;
        t.set("z", v.z)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(BlockPos::new(
                req_key::<i32>(&t, "x", field)?,
                req_key::<i32>(&t, "y", field)?,
                req_key::<i32>(&t, "z", field)?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{x, y, z}}, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for LevelDimension {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(lua, crate::lua_mods::dimension_id(*v))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(
            match enum_from(
                v,
                field,
                &[
                    "minecraft:overworld",
                    "minecraft:the_nether",
                    "minecraft:the_end",
                ],
            )? {
                "minecraft:the_nether" => LevelDimension::Nether,
                "minecraft:the_end" => LevelDimension::End,
                _ => LevelDimension::Overworld,
            },
        )
    }
}

/// Three strings (`{key, label, default}`): a config key.
impl LuaStruct for [String; 3] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("key", v[0].as_str())?;
        t.set("label", v[1].as_str())?;
        t.set("default", v[2].as_str())?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok([
                req_key::<String>(&t, "key", field)?,
                req_key::<String>(&t, "label", field)?,
                req_key::<String>(&t, "default", field)?,
            ]),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{key, label, default}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// An optional float pair (`nil` or `{x, y}`): a screen slot.
impl LuaStruct for Option<[f32; 2]> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        match v {
            Some(pair) => {
                let t = lua.create_table()?;
                t.set("x", pair[0])?;
                t.set("y", pair[1])?;
                Ok(Value::Table(t))
            }
            None => Ok(Value::Nil),
        }
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Nil | Value::Boolean(false) => Ok(None),
            Value::Table(t) => Ok(Some([
                req_key::<f32>(&t, "x", field)?,
                req_key::<f32>(&t, "y", field)?,
            ])),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{x, y}} or nil, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeZoom {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("factor", v.factor)?;
        t.set("smooth", v.smooth)?;
        t.set("turn", v.turn)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(NativeZoom {
                factor: req_key::<f32>(&t, "factor", field)?,
                smooth: req_key::<bool>(&t, "smooth", field)?,
                turn: req_key::<f32>(&t, "turn", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a zoom table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeMinimap {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("corner", Corner::to_value(lua, &v.corner)?)?;
        t.set("size", v.size)?;
        t.set("blocks_per_pixel", v.blocks_per_pixel)?;
        t.set("rotate", v.rotate)?;
        t.set("round", v.round)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let corner: Value = t.get("corner")?;
                Ok(NativeMinimap {
                    corner: Corner::from_value(lua, corner, field)?,
                    size: req_key::<f32>(&t, "size", field)?,
                    blocks_per_pixel: req_key::<f32>(&t, "blocks_per_pixel", field)?,
                    rotate: req_key::<bool>(&t, "rotate", field)?,
                    round: req_key::<bool>(&t, "round", field)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a minimap table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeMod {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("id", v.id.as_str())?;
        t.set("title", v.title.as_str())?;
        t.set("description", v.description.as_str())?;
        t.set("keys", struct_vec_to_lua(lua, &v.keys)?)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (keys, at) = req_table(lua, &t, "keys", field)?;
                Ok(NativeMod {
                    id: req_key::<String>(&t, "id", field)?,
                    title: req_key::<String>(&t, "title", field)?,
                    description: req_key::<String>(&t, "description", field)?,
                    keys: struct_vec_from_lua(lua, &keys, &at)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a mod table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeScreen {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("width", v.width)?;
        t.set("height", v.height)?;
        t.set("slots", struct_vec_to_lua(lua, &v.slots)?)?;
        t.set("inventory_y", v.inventory_y)?;
        t.set("inventory_x", v.inventory_x)?;
        t.set("own_labels", v.own_labels)?;
        t.set("own_background", v.own_background)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (slots, at) = req_table(lua, &t, "slots", field)?;
                Ok(NativeScreen {
                    width: req_key::<f32>(&t, "width", field)?,
                    height: req_key::<f32>(&t, "height", field)?,
                    slots: struct_vec_from_lua(lua, &slots, &at)?,
                    inventory_y: req_key::<f32>(&t, "inventory_y", field)?,
                    inventory_x: req_key::<f32>(&t, "inventory_x", field)?,
                    own_labels: req_key::<bool>(&t, "own_labels", field)?,
                    own_background: req_key::<bool>(&t, "own_background", field)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a screen table, got {}",
                other.type_name()
            ))),
        }
    }
}

use horizon_client::mod_api::{ModContainer, ModEnergyBuffer, ModFluidTanks};

/// Six face lists as one sequence of six sequences.
fn faces_to_lua(lua: &Lua, faces: &[Vec<usize>; 6]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (i, face) in faces.iter().enumerate() {
        t.set(i + 1, lua.create_sequence_from(face.iter().copied())?)?;
    }
    Ok(t)
}

fn faces_from_lua(_lua: &Lua, v: Value, field: &str) -> mlua::Result<[Vec<usize>; 6]> {
    match v {
        Value::Table(t) => {
            let mut out: [Vec<usize>; 6] = Default::default();
            for (i, face) in t.sequence_values::<Table>().enumerate() {
                if i >= 6 {
                    break;
                }
                let face = face.map_err(|_| {
                    mlua::Error::runtime(format!("{field}: expected six slot sequences"))
                })?;
                let at = format!("{field}[{}]", i + 1);
                let mut slots = Vec::new();
                for slot in face.sequence_values::<usize>() {
                    slots.push(slot.map_err(|_| {
                        mlua::Error::runtime(format!("{at}: expected slot indices"))
                    })?);
                }
                out[i] = slots;
            }
            Ok(out)
        }
        other => Err(mlua::Error::runtime(format!(
            "{field}: expected six slot sequences, got {}",
            other.type_name()
        ))),
    }
}

impl LuaStruct for ModContainer {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("slots", v.slots)?;
        t.set("faces", faces_to_lua(lua, &v.faces)?)?;
        t.set(
            "inputs",
            lua.create_sequence_from(v.inputs.iter().copied())?,
        )?;
        t.set(
            "outputs",
            lua.create_sequence_from(v.outputs.iter().copied())?,
        )?;
        t.set("menu", v.menu.clone())?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (faces, at) = req_table(lua, &t, "faces", field)?;
                let mut inputs = Vec::new();
                for slot in t
                    .get::<Table>("inputs")
                    .map_err(|_| {
                        mlua::Error::runtime(format!("{field}.inputs: expected a sequence"))
                    })?
                    .sequence_values::<usize>()
                {
                    inputs.push(slot.map_err(|_| {
                        mlua::Error::runtime(format!("{field}.inputs: expected slot indices"))
                    })?);
                }
                let mut outputs = Vec::new();
                for slot in t
                    .get::<Table>("outputs")
                    .map_err(|_| {
                        mlua::Error::runtime(format!("{field}.outputs: expected a sequence"))
                    })?
                    .sequence_values::<usize>()
                {
                    outputs.push(slot.map_err(|_| {
                        mlua::Error::runtime(format!("{field}.outputs: expected slot indices"))
                    })?);
                }
                Ok(ModContainer {
                    slots: req_key::<usize>(&t, "slots", field)?,
                    faces: faces_from_lua(lua, Value::Table(faces), &at)?,
                    inputs,
                    outputs,
                    menu: t.get::<Option<String>>("menu").map_err(|_| {
                        mlua::Error::runtime(format!("{field}.menu: expected a string or nil"))
                    })?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a container table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for ModFluidTanks {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("tanks", lua.create_sequence_from(v.tanks.iter().copied())?)?;
        t.set("faces", faces_to_lua(lua, &v.faces)?)?;
        t.set(
            "inputs",
            lua.create_sequence_from(v.inputs.iter().copied())?,
        )?;
        t.set(
            "outputs",
            lua.create_sequence_from(v.outputs.iter().copied())?,
        )?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (faces, at) = req_table(lua, &t, "faces", field)?;
                let mut tanks = Vec::new();
                for cap in t
                    .get::<Table>("tanks")
                    .map_err(|_| {
                        mlua::Error::runtime(format!("{field}.tanks: expected a sequence"))
                    })?
                    .sequence_values::<i32>()
                {
                    tanks.push(cap.map_err(|_| {
                        mlua::Error::runtime(format!("{field}.tanks: expected capacities"))
                    })?);
                }
                let mut inputs = Vec::new();
                for slot in t
                    .get::<Table>("inputs")
                    .map_err(|_| {
                        mlua::Error::runtime(format!("{field}.inputs: expected a sequence"))
                    })?
                    .sequence_values::<usize>()
                {
                    inputs.push(slot.map_err(|_| {
                        mlua::Error::runtime(format!("{field}.inputs: expected tank indices"))
                    })?);
                }
                let mut outputs = Vec::new();
                for slot in t
                    .get::<Table>("outputs")
                    .map_err(|_| {
                        mlua::Error::runtime(format!("{field}.outputs: expected a sequence"))
                    })?
                    .sequence_values::<usize>()
                {
                    outputs.push(slot.map_err(|_| {
                        mlua::Error::runtime(format!("{field}.outputs: expected tank indices"))
                    })?);
                }
                Ok(ModFluidTanks {
                    tanks,
                    faces: faces_from_lua(lua, Value::Table(faces), &at)?,
                    inputs,
                    outputs,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a tanks table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Six face flags as one sequence of six booleans.
fn face_flags_to_lua(lua: &Lua, faces: &[bool; 6]) -> mlua::Result<Table> {
    lua.create_sequence_from(faces.iter().copied())
}

fn face_flags_from_lua(v: Value, field: &str) -> mlua::Result<[bool; 6]> {
    match v {
        Value::Table(t) => {
            let mut out = [false; 6];
            for (i, flag) in t.sequence_values::<bool>().enumerate() {
                if i >= 6 {
                    break;
                }
                out[i] = flag
                    .map_err(|_| mlua::Error::runtime(format!("{field}: expected six booleans")))?;
            }
            Ok(out)
        }
        other => Err(mlua::Error::runtime(format!(
            "{field}: expected six booleans, got {}",
            other.type_name()
        ))),
    }
}

impl LuaStruct for ModEnergyBuffer {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("capacity", v.capacity)?;
        t.set("max_insert", v.max_insert)?;
        t.set("max_extract", v.max_extract)?;
        t.set("insert_faces", face_flags_to_lua(lua, &v.insert_faces)?)?;
        t.set("extract_faces", face_flags_to_lua(lua, &v.extract_faces)?)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let insert: Value = t.get("insert_faces")?;
                let extract: Value = t.get("extract_faces")?;
                Ok(ModEnergyBuffer {
                    capacity: req_key::<i32>(&t, "capacity", field)?,
                    max_insert: req_key::<i32>(&t, "max_insert", field)?,
                    max_extract: req_key::<i32>(&t, "max_extract", field)?,
                    insert_faces: face_flags_from_lua(insert, field)?,
                    extract_faces: face_flags_from_lua(extract, field)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected an energy table, got {}",
                other.type_name()
            ))),
        }
    }
}

use horizon_client::mod_api::ItemEnergyBuffer;

impl LuaStruct for ItemEnergyBuffer {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("capacity", v.capacity)?;
        t.set("max_insert", v.max_insert)?;
        t.set("max_extract", v.max_extract)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(ItemEnergyBuffer {
                capacity: req_key::<i32>(&t, "capacity", field)?,
                max_insert: req_key::<i32>(&t, "max_insert", field)?,
                max_extract: req_key::<i32>(&t, "max_extract", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected an energy table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl<'a> LuaStruct for NativeMapInfo<'a> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("id", v.id)?;
        t.set("center", <[i32; 2]>::to_value(lua, &v.center)?)?;
        t.set("scale", v.scale)?;
        t.set("dimension", v.dimension)?;
        t.set("locked", v.locked)?;
        t.set("in_frame", v.in_frame)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!("{field}: a map is read-only")))
    }
}

impl LuaStruct for NativeMapDecoration {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("sprite", v.sprite.as_str())?;
        t.set("x", v.x)?;
        t.set("y", v.y)?;
        t.set("rot", v.rot)?;
        t.set("shown_on_frame", v.shown_on_frame)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(NativeMapDecoration {
                sprite: req_key::<String>(&t, "sprite", field)?,
                x: req_key::<i8>(&t, "x", field)?,
                y: req_key::<i8>(&t, "y", field)?,
                rot: req_key::<u8>(&t, "rot", field)?,
                shown_on_frame: req_key::<bool>(&t, "shown_on_frame", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a decoration table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeDataMapType {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("id", v.id.as_str())?;
        t.set("registry", v.registry.as_str())?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(NativeDataMapType {
                id: req_key::<String>(&t, "id", field)?,
                registry: req_key::<String>(&t, "registry", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a data map table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeConfig {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("mod_id", v.mod_id.as_str())?;
        t.set("kind", NativeConfigKind::to_value(lua, &v.kind)?)?;
        t.set("file", v.file.as_str())?;
        t.set("default", v.default.as_str())?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let kind: Value = t.get("kind")?;
                let mut config = NativeConfig::new(
                    &req_key::<String>(&t, "mod_id", field)?,
                    NativeConfigKind::from_value(lua, kind, field)?,
                    &req_key::<String>(&t, "default", field)?,
                );
                config.file = req_key::<String>(&t, "file", field)?;
                Ok(config)
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a config table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeConfigFile {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("mod_id", v.mod_id.as_str())?;
        t.set("kind", NativeConfigKind::to_value(lua, &v.kind)?)?;
        t.set("file", v.file.as_str())?;
        t.set("path", v.path.to_string_lossy())?;
        t.set("text", v.text.as_str())?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: a config file is read-only"
        )))
    }
}

impl LuaStruct for SpawnEntry {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("entity", v.entity.as_str())?;
        t.set("weight", v.weight)?;
        t.set("min", v.min)?;
        t.set("max", v.max)?;
        t.set("constant", v.constant)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(SpawnEntry {
                entity: req_key::<String>(&t, "entity", field)?,
                weight: req_key::<u32>(&t, "weight", field)?,
                min: req_key::<u32>(&t, "min", field)?,
                max: req_key::<u32>(&t, "max", field)?,
                constant: req_key::<bool>(&t, "constant", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a spawn table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for RecipeBookCategory {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("id", v.id.as_str())?;
        t.set("recipe_type", v.recipe_type.as_str())?;
        t.set("icon", v.icon.as_str())?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(RecipeBookCategory {
                id: req_key::<String>(&t, "id", field)?,
                recipe_type: req_key::<String>(&t, "recipe_type", field)?,
                icon: req_key::<String>(&t, "icon", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a category table, got {}",
                other.type_name()
            ))),
        }
    }
}

use horizon_client::mod_api::NativeReloadListener;

impl LuaStruct for NativeReloadListener {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("id", v.id.as_str())?;
        t.set("folder", v.folder.as_str())?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(NativeReloadListener::json(
                &req_key::<String>(&t, "id", field)?,
                &req_key::<String>(&t, "folder", field)?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a listener table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for Suggestion {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("range", <(usize, usize)>::to_value(lua, &v.range)?)?;
        t.set("text", v.text.as_str())?;
        t.set("tooltip", v.tooltip.clone())?;
        t.set("int_value", v.int_value)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (range, at) = req_table(lua, &t, "range", field)?;
                Ok(Suggestion {
                    range: <(usize, usize)>::from_value(lua, Value::Table(range), &at)?,
                    text: req_key::<String>(&t, "text", field)?,
                    tooltip: t.get::<Option<String>>("tooltip").map_err(|_| {
                        mlua::Error::runtime(format!("{field}.tooltip: expected a string or nil"))
                    })?,
                    int_value: t.get::<Option<i32>>("int_value").map_err(|_| {
                        mlua::Error::runtime(format!("{field}.int_value: expected a number or nil"))
                    })?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a suggestion table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for ItemCost {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("stack", ItemStack::to_value(lua, &v.stack)?)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (stack, at) = req_table(lua, &t, "stack", field)?;
                Ok(ItemCost {
                    stack: ItemStack::from_value(lua, Value::Table(stack), &at)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a cost table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for MerchantOffer {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("base_cost_a", ItemCost::to_value(lua, &v.base_cost_a)?)?;
        t.set("cost_b", Option::<ItemCost>::to_value(lua, &v.cost_b)?)?;
        t.set("result", ItemStack::to_value(lua, &v.result)?)?;
        t.set("uses", v.uses)?;
        t.set("max_uses", v.max_uses)?;
        t.set("reward_exp", v.reward_exp)?;
        t.set("special_price_diff", v.special_price_diff)?;
        t.set("demand", v.demand)?;
        t.set("price_multiplier", v.price_multiplier)?;
        t.set("xp", v.xp)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (base, at) = req_table(lua, &t, "base_cost_a", field)?;
                let (result, rt) = req_table(lua, &t, "result", field)?;
                let cost_b: Value = t.get("cost_b")?;
                let cost_b = match cost_b {
                    Value::Nil | Value::Boolean(false) => None,
                    Value::Table(c) => Some(ItemCost::from_value(lua, Value::Table(c), field)?),
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "{field}.cost_b: expected a cost table or nil, got {}",
                            other.type_name()
                        )));
                    }
                };
                Ok(MerchantOffer {
                    base_cost_a: ItemCost::from_value(lua, Value::Table(base), &at)?,
                    cost_b,
                    result: ItemStack::from_value(lua, Value::Table(result), &rt)?,
                    uses: req_key::<i32>(&t, "uses", field)?,
                    max_uses: req_key::<i32>(&t, "max_uses", field)?,
                    reward_exp: req_key::<bool>(&t, "reward_exp", field)?,
                    special_price_diff: req_key::<i32>(&t, "special_price_diff", field)?,
                    demand: req_key::<i32>(&t, "demand", field)?,
                    price_multiplier: req_key::<f32>(&t, "price_multiplier", field)?,
                    xp: req_key::<i32>(&t, "xp", field)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected an offer table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// `Option<ItemCost>`: `nil` (or `false` in a sequence) is no second cost.
impl LuaStruct for Option<ItemCost> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        match v {
            Some(cost) => ItemCost::to_value(lua, cost),
            None => Ok(Value::Nil),
        }
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Nil | Value::Boolean(false) => Ok(None),
            Value::Table(_) => Ok(Some(ItemCost::from_value(lua, v, field)?)),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a cost table or nil, got {}",
                other.type_name()
            ))),
        }
    }
}

use horizon_client::menu::AnvilCraft;

impl LuaStruct for AnvilCraft {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("left", Option::<ItemStack>::to_value(lua, &v.left)?)?;
        t.set("right", Option::<ItemStack>::to_value(lua, &v.right)?)?;
        t.set("output", ItemStack::to_value(lua, &v.output)?)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: an anvil craft is read-only"
        )))
    }
}

impl<'a> LuaStruct for NativeNoteBlockChange<'a> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("dimension", LevelDimension::to_value(lua, &v.dimension)?)?;
        t.set("pos", BlockPos::to_value(lua, &v.pos)?)?;
        t.set("instrument", v.instrument)?;
        t.set("old_note", v.old_note)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: a note change is read-only"
        )))
    }
}

impl LuaStruct for NativeFluidStack {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("fluid", v.fluid.as_str())?;
        t.set("amount", v.amount)?;
        t.set("components", Compound::to_value(lua, &v.components)?)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let (components, at) = req_table(lua, &t, "components", field)?;
                Ok(NativeFluidStack {
                    fluid: req_key::<String>(&t, "fluid", field)?,
                    amount: req_key::<i32>(&t, "amount", field)?,
                    components: Compound::from_value(lua, Value::Table(components), &at)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a fluid table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeFluidDrain {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        match v {
            NativeFluidDrain::Any(amount) => {
                t.set("kind", "any")?;
                t.set("amount", *amount)?;
            }
            NativeFluidDrain::Exact(stack) => {
                t.set("kind", "exact")?;
                t.set("stack", NativeFluidStack::to_value(lua, stack)?)?;
            }
            _ => {
                t.set("kind", "any")?;
                t.set("amount", 0)?;
            }
        }
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let kind: String = req_key::<String>(&t, "kind", field)?;
                match kind.as_str() {
                    "any" => Ok(NativeFluidDrain::Any(req_key::<i32>(&t, "amount", field)?)),
                    "exact" => {
                        let (stack, at) = req_table(lua, &t, "stack", field)?;
                        Ok(NativeFluidDrain::Exact(NativeFluidStack::from_value(
                            lua,
                            Value::Table(stack),
                            &at,
                        )?))
                    }
                    other => Err(mlua::Error::runtime(format!(
                        "{field}.kind: expected \"any\" or \"exact\", got {other:?}"
                    ))),
                }
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a drain table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for NativeFluidTankView {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("tank", v.tank)?;
        t.set(
            "content",
            Option::<NativeFluidStack>::to_value(lua, &v.content)?,
        )?;
        t.set("capacity", v.capacity)?;
        t.set("can_fill", v.can_fill)?;
        t.set("can_drain", v.can_drain)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let content: Value = t.get("content")?;
                Ok(NativeFluidTankView {
                    tank: req_key::<usize>(&t, "tank", field)?,
                    content: Option::<NativeFluidStack>::from_value(lua, content, field)?,
                    capacity: req_key::<i32>(&t, "capacity", field)?,
                    can_fill: req_key::<bool>(&t, "can_fill", field)?,
                    can_drain: req_key::<bool>(&t, "can_drain", field)?,
                })
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a tank table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// An optional fluid payload: `nil` (or `false` in a sequence) is empty.
impl LuaStruct for Option<NativeFluidStack> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        match v {
            Some(stack) => NativeFluidStack::to_value(lua, stack),
            None => Ok(Value::Nil),
        }
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Nil | Value::Boolean(false) => Ok(None),
            Value::Table(_) => Ok(Some(NativeFluidStack::from_value(lua, v, field)?)),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a fluid table or nil, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for Music {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("sound", v.sound.as_str())?;
        t.set("min_delay", v.min_delay)?;
        t.set("max_delay", v.max_delay)?;
        t.set("replace_current_music", v.replace_current_music)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(Music {
                sound: req_key::<String>(&t, "sound", field)?,
                min_delay: req_key::<i32>(&t, "min_delay", field)?,
                max_delay: req_key::<i32>(&t, "max_delay", field)?,
                replace_current_music: req_key::<bool>(&t, "replace_current_music", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a music table, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for MusicLayer {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("sound", v.sound.as_str())?;
        t.set("volume", v.volume)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok(MusicLayer {
                sound: req_key::<String>(&t, "sound", field)?,
                volume: req_key::<f32>(&t, "volume", field)?,
            }),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a layer table, got {}",
                other.type_name()
            ))),
        }
    }
}

use horizon_client::mod_api::{SoundPlayback, TickSpan};

impl<'a> LuaStruct for SoundPlayback<'a> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("instance", v.instance)?;
        t.set("identifier", v.identifier)?;
        t.set("source", v.source)?;
        t.set("position", <[f64; 3]>::to_value(lua, &v.position)?)?;
        t.set("relative", v.relative)?;
        t.set("starting", v.starting)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: a playback is read-only"
        )))
    }
}

impl LuaStruct for TickSpan {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("name", v.name)?;
        t.set("ms", v.ms)?;
        t.set("cpu_ms", v.cpu_ms)?;
        t.set("depth", v.depth)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: a span is read-only"
        )))
    }
}

use horizon_client::mod_api::BlockProperties as StateProperties;

/// A block's properties as `{name = value}`.
impl LuaStruct for StateProperties {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        for (key, value) in v.iter() {
            t.set(key.as_str(), value.as_str())?;
        }
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: properties are read-only"
        )))
    }
}

use horizon_client::mod_api::{FluidState, PlacementContext};

impl LuaStruct for PlacementContext {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("pos", BlockPos::to_value(lua, &v.pos)?)?;
        t.set("hit_pos", BlockPos::to_value(lua, &v.hit_pos)?)?;
        t.set("face", Direction::to_value(lua, &v.face)?)?;
        let hit = lua.create_table()?;
        hit.set("x", v.hit.x)?;
        hit.set("y", v.hit.y)?;
        hit.set("z", v.hit.z)?;
        t.set("hit", hit)?;
        t.set("inside", v.inside)?;
        t.set("yaw", v.yaw)?;
        t.set("pitch", v.pitch)?;
        t.set("sneaking", v.sneaking)?;
        t.set("item", v.item.as_str())?;
        t.set("replace_clicked", v.replace_clicked)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: a placement is read-only"
        )))
    }
}

impl LuaStruct for FluidState {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("kind", v.kind().name())?;
        t.set("amount", v.amount())?;
        t.set("falling", v.falling())?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, _v: Value, field: &str) -> mlua::Result<Self> {
        Err(mlua::Error::runtime(format!(
            "{field}: a fluid is read-only"
        )))
    }
}

/// Rich text as the settled shapes: `event.<field>` is the plain string and
/// `event.<field>_component` the vanilla JSON component table; a write is a
/// string (plain spans) or a component table. Nested, a text is
/// `{text, component}`.
pub(crate) fn text_parts(lua: &Lua, v: &Text) -> mlua::Result<(String, Value)> {
    let component = crate::lua_mods::json::json_to_lua(lua, &v.component())?;
    Ok((v.plain(), component))
}

/// A written text back: a component table wins when present, else a string
/// is plain spans and a table is the component itself.
pub(crate) fn text_from_parts(
    _lua: &Lua,
    text: Value,
    component: Value,
    field: &str,
) -> mlua::Result<Text> {
    match component {
        Value::Table(t) => Ok(Text::empty().with_component(
            crate::lua_mods::json::lua_to_json(&Value::Table(t))
                .map_err(|e| mlua::Error::runtime(format!("{field}_component: {e}")))?,
        )),
        _ => match text {
            Value::String(s) => Ok(Text::literal(s.to_string_lossy())),
            Value::Table(t) => Ok(Text::empty().with_component(
                crate::lua_mods::json::lua_to_json(&Value::Table(t))
                    .map_err(|e| mlua::Error::runtime(format!("{field}: {e}")))?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a string or a component table, got {}",
                other.type_name()
            ))),
        },
    }
}

impl LuaStruct for Text {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let (plain, component) = text_parts(lua, v)?;
        let t = lua.create_table()?;
        t.set("text", plain)?;
        t.set("component", component)?;
        Ok(Value::Table(t))
    }
    fn from_value(lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let component: Value = t.get("component")?;
                let text: Value = t.get("text")?;
                text_from_parts(lua, text, component, field)
            }
            Value::String(_) => text_from_parts(lua, v, Value::Nil, field),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a string or a component table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Saved data as the documented rules: numbers, strings, booleans, nested
/// tables, sequences; a number keeps its NBT type when left as read, a new
/// whole number is an int, a fraction a double, a key set to nil is removed,
/// nesting stops at 128.
impl LuaStruct for Compound {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        Ok(Value::Table(crate::lua_mods::nbt::compound_to_table(
            lua, v,
        )?))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => crate::lua_mods::nbt::table_to_compound(&t, None)
                .map_err(|e| mlua::Error::runtime(format!("{field}: {e}"))),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected a table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// JSON data (ingredient files, loot tables, configs): an object is a table,
/// an array a sequence, a whole number an int, a fraction a double, nesting
/// stops at 128.
impl LuaStruct for serde_json::Value {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        crate::lua_mods::json::json_to_lua(lua, v)
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        crate::lua_mods::json::lua_to_json(&v)
            .map_err(|e| mlua::Error::runtime(format!("{field}: {e}")))
    }
}

impl LuaStruct for serde_json::Map<String, serde_json::Value> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        crate::lua_mods::json::json_to_lua(lua, &serde_json::Value::Object(v.clone()))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match crate::lua_mods::json::lua_to_json(&v)
            .map_err(|e| mlua::Error::runtime(format!("{field}: {e}")))?
        {
            serde_json::Value::Object(map) => Ok(map),
            _ => Err(mlua::Error::runtime(format!(
                "{field}: expected an object table"
            ))),
        }
    }
}

impl LuaStruct for ItemStack {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        Ok(Value::Table(item_table_of(lua, v)?))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => crate::generated_runtime::item_from_table(&t)
                .map_err(|e| mlua::Error::runtime(format!("{field}: {e}"))),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected an item table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// An optional stack: `nil` (or `false` in a sequence) is no stack.
impl LuaStruct for Option<ItemStack> {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        match v {
            Some(stack) => Ok(Value::Table(item_table_of(lua, stack)?)),
            None => Ok(Value::Nil),
        }
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Nil | Value::Boolean(false) => Ok(None),
            Value::Table(t) => Ok(Some(
                crate::generated_runtime::item_from_table(&t)
                    .map_err(|e| mlua::Error::runtime(format!("{field}: {e}")))?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected an item table or nil, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A 3-float position (`{x, y, z}`), as the hand-written hooks name it.
impl LuaStruct for [f64; 3] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        Ok(Value::Table(crate::lua_mods::goals::vec3(lua, *v)?))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok([
                req_key::<f64>(&t, "x", field)?,
                req_key::<f64>(&t, "y", field)?,
                req_key::<f64>(&t, "z", field)?,
            ]),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{x, y, z}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A block position (`{x, y, z}`), for block lists.
impl LuaStruct for [i32; 3] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("x", v[0])?;
        t.set("y", v[1])?;
        t.set("z", v[2])?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok([
                req_key::<i32>(&t, "x", field)?,
                req_key::<i32>(&t, "y", field)?,
                req_key::<i32>(&t, "z", field)?,
            ]),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{x, y, z}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Three floats as `{x, y, z}` (lightning rgb, beacon beams).
impl LuaStruct for [f32; 3] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("x", v[0])?;
        t.set("y", v[1])?;
        t.set("z", v[2])?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok([
                req_key::<f32>(&t, "x", field)?,
                req_key::<f32>(&t, "y", field)?,
                req_key::<f32>(&t, "z", field)?,
            ]),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{x, y, z}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Two floats as `{x, y}` (entity rotation).
impl LuaStruct for [f32; 2] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("x", v[0])?;
        t.set("y", v[1])?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok([
                req_key::<f32>(&t, "x", field)?,
                req_key::<f32>(&t, "y", field)?,
            ]),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{x, y}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A fixed 2-int pair (`{x, z}`).
impl LuaStruct for [i32; 2] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("x", v[0])?;
        t.set("z", v[1])?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok([
                req_key::<i32>(&t, "x", field)?,
                req_key::<i32>(&t, "z", field)?,
            ]),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{x, z}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Three toggles as a sequence (mouse buttons).
impl LuaStruct for [bool; 3] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        Ok(Value::Table(lua.create_sequence_from(v.iter().copied())?))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let mut out = [false; 3];
                for (i, value) in t.sequence_values::<bool>().enumerate() {
                    if i >= 3 {
                        break;
                    }
                    out[i] = value.map_err(|_| {
                        mlua::Error::runtime(format!("{field}: expected three booleans"))
                    })?;
                }
                Ok(out)
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected three booleans, got {}",
                other.type_name()
            ))),
        }
    }
}

/// A range (`{start, end}` in order).
impl LuaStruct for (usize, usize) {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("start", v.0)?;
        t.set("end", v.1)?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok((
                req_key::<usize>(&t, "start", field)?,
                req_key::<usize>(&t, "end", field)?,
            )),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{start, end}}, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for [usize; 2] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        t.set("start", v[0])?;
        t.set("end", v[1])?;
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => Ok([
                req_key::<usize>(&t, "start", field)?,
                req_key::<usize>(&t, "end", field)?,
            ]),
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected {{start, end}}, got {}",
                other.type_name()
            ))),
        }
    }
}

/// Eight `vec4<f32>` shader values, one `{x, y, z, w}` table each.
impl LuaStruct for [[f32; 4]; 8] {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        let t = lua.create_table()?;
        for (i, slot) in v.iter().enumerate() {
            let s = lua.create_table()?;
            s.set("x", slot[0])?;
            s.set("y", slot[1])?;
            s.set("z", slot[2])?;
            s.set("w", slot[3])?;
            t.set(i + 1, s)?;
        }
        Ok(Value::Table(t))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Table(t) => {
                let mut out = [[0.0f32; 4]; 8];
                for (i, slot) in t.sequence_values::<Table>().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    let slot = slot.map_err(|_| {
                        mlua::Error::runtime(format!(
                            "{field}: expected eight {{x, y, z, w}} tables"
                        ))
                    })?;
                    let at = format!("{field}[{}]", i + 1);
                    out[i] = [
                        req_key::<f32>(&slot, "x", &at)?,
                        req_key::<f32>(&slot, "y", &at)?,
                        req_key::<f32>(&slot, "z", &at)?,
                        req_key::<f32>(&slot, "w", &at)?,
                    ];
                }
                Ok(out)
            }
            other => Err(mlua::Error::runtime(format!(
                "{field}: expected eight {{x, y, z, w}} tables, got {}",
                other.type_name()
            ))),
        }
    }
}

impl LuaStruct for Duration {
    fn to_value(_lua: &Lua, v: &Self) -> mlua::Result<Value> {
        Ok(Value::Number(v.as_secs_f64()))
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        match v {
            Value::Number(n) => Ok(Duration::from_secs_f64(n.max(0.0))),
            Value::Integer(i) => Ok(Duration::from_secs_f64((i as f64).max(0.0))),
            _ => Err(mlua::Error::runtime(format!(
                "{field}: expected seconds, got {}",
                v.type_name()
            ))),
        }
    }
}

impl LuaStruct for std::net::SocketAddr {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(lua, &v.to_string())
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        let text = lua_string(v, field)?;
        text.parse().map_err(|_| {
            mlua::Error::runtime(format!("{field}: expected an address, got {text:?}"))
        })
    }
}

/// A borrowed static string (command and emissive-layer names): read as its
/// string, written back interned.
impl LuaStruct for &'static str {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(lua, v)
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(static_str(&lua_string(v, field)?))
    }
}

impl LuaStruct for Direction {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(
            lua,
            match v {
                Direction::Down => "down",
                Direction::Up => "up",
                Direction::North => "north",
                Direction::South => "south",
                Direction::West => "west",
                Direction::East => "east",
            },
        )
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(
            match enum_from(v, field, &["down", "up", "north", "south", "west", "east"])? {
                "down" => Direction::Down,
                "up" => Direction::Up,
                "north" => Direction::North,
                "south" => Direction::South,
                "west" => Direction::West,
                _ => Direction::East,
            },
        )
    }
}

impl LuaStruct for Hand {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(
            lua,
            match v {
                Hand::Main => "main",
                Hand::Off => "off",
            },
        )
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(match enum_from(v, field, &["main", "off"])? {
            "main" => Hand::Main,
            _ => Hand::Off,
        })
    }
}

impl LuaStruct for UseAnimation {
    fn to_value(lua: &Lua, v: &Self) -> mlua::Result<Value> {
        str_value(
            lua,
            match v {
                UseAnimation::None => "none",
                UseAnimation::Eat => "eat",
                UseAnimation::Drink => "drink",
                UseAnimation::Block => "block",
                UseAnimation::Bow => "bow",
                UseAnimation::Trident => "trident",
                UseAnimation::Crossbow => "crossbow",
                UseAnimation::Spyglass => "spyglass",
                UseAnimation::TootHorn => "toot_horn",
                UseAnimation::Brush => "brush",
                UseAnimation::Bundle => "bundle",
                UseAnimation::Spear => "spear",
            },
        )
    }
    fn from_value(_lua: &Lua, v: Value, field: &str) -> mlua::Result<Self> {
        Ok(
            match enum_from(
                v,
                field,
                &[
                    "none",
                    "eat",
                    "drink",
                    "block",
                    "bow",
                    "trident",
                    "crossbow",
                    "spyglass",
                    "toot_horn",
                    "brush",
                    "bundle",
                    "spear",
                ],
            )? {
                "none" => UseAnimation::None,
                "eat" => UseAnimation::Eat,
                "drink" => UseAnimation::Drink,
                "block" => UseAnimation::Block,
                "bow" => UseAnimation::Bow,
                "trident" => UseAnimation::Trident,
                "crossbow" => UseAnimation::Crossbow,
                "spyglass" => UseAnimation::Spyglass,
                "toot_horn" => UseAnimation::TootHorn,
                "brush" => UseAnimation::Brush,
                "bundle" => UseAnimation::Bundle,
                _ => UseAnimation::Spear,
            },
        )
    }
}

/// Level properties as a `{name = value}` table.
pub(crate) fn props_of(lua: &Lua, props: Vec<(String, String)>) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (key, value) in &props {
        t.set(key.as_str(), value.as_str())?;
    }
    Ok(t)
}

/// A game host's state name split into its block id and property table: the
/// name is `id` or `id[prop=value,...]`, as a command writes one.
pub(crate) fn split_state_name(lua: &Lua, name: &str) -> mlua::Result<(String, Table)> {
    let t = lua.create_table()?;
    match name.split_once('[') {
        Some((id, rest)) => {
            let props = rest.strip_suffix(']').unwrap_or(rest);
            for pair in props.split(',') {
                if let Some((key, value)) = pair.split_once('=') {
                    t.set(key, value)?;
                }
            }
            Ok((id.to_owned(), t))
        }
        None => Ok((name.to_owned(), t)),
    }
}

/// The replace out value back: `nil` reads as `None`, anything else as
/// `Some` (a table through the shape's own reader). `ret` is the trait
/// method's exact return type. Tails with a host pass it after the reader;
/// only `answer_state` uses it (a block state name needs the registry).
macro_rules! gen_replace_out {
    ($table:ident, $lua:ident, $out:ident : value $(, $r:expr $(, $s:expr)?)?) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            v => Some(mlua::FromLua::from_lua(v, $lua)?),
        })
    };
    ($table:ident, $lua:ident, $out:ident : pair $(, $r:expr $(, $s:expr)?)?) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::Table(t) => Some(crate::generated_runtime::read_pair(&t)?),
            _ => return Err(mlua::Error::runtime("expected a table or nil")),
        })
    };
    ($table:ident, $lua:ident, $out:ident : item $(, $r:expr $(, $s:expr)?)?) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::Table(t) => Some(crate::generated_runtime::item_from_table(&t)?),
            _ => return Err(mlua::Error::runtime("expected a table or nil")),
        })
    };
    ($table:ident, $lua:ident, $out:ident : struct_out($st:ty) $(, $r:expr $(, $s:expr)?)?) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::Table(t) => {
                Some(<$st as crate::generated_runtime::LuaStruct>::from_value(
                    $lua,
                    mlua::Value::Table(t),
                    stringify!($out),
                )?)
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected a table or nil, got {}",
                    stringify!($out),
                    v.type_name()
                )));
            }
        })
    };
    ($table:ident, $lua:ident, $out:ident : struct_vec_out($st:ty) $(, $r:expr $(, $s:expr)?)?) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::Table(t) => {
                let mut out = Vec::new();
                for item in t.sequence_values::<mlua::Value>() {
                    out.push(<$st as crate::generated_runtime::LuaStruct>::from_value(
                        $lua,
                        item?,
                        stringify!($out),
                    )?);
                }
                Some(out)
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected a sequence or nil, got {}",
                    stringify!($out),
                    v.type_name()
                )));
            }
        })
    };
    ($table:ident, $lua:ident, $out:ident : bytes_out $(, $r:expr $(, $s:expr)?)?) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::String(s) => Some(s.as_bytes().to_vec()),
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected a byte string or nil, got {}",
                    stringify!($out),
                    v.type_name()
                )));
            }
        })
    };
    // A placement answer: nil leaves it to the next mod or vanilla; a
    // state name answers for the event's OWN block only, applying each of
    // its properties to the event's state through the level. Anything else
    // (another block, a bad property) errors through `hook_error`.
    ($table:ident, $lua:ident, $out:ident : answer_state, $level:expr, $state:expr) => {
        Ok(match $table.get::<mlua::Value>(stringify!($out))? {
            mlua::Value::Nil => None,
            mlua::Value::String(s) => {
                let name = s.to_str()?.to_owned();
                Some(Some(crate::generated_runtime::placement_answer(
                    $level,
                    $state,
                    &name,
                    stringify!($out),
                )?))
            }
            v => {
                return Err(mlua::Error::runtime(format!(
                    "{}: expected a block state name or nil, got {}",
                    stringify!($out),
                    v.type_name()
                )));
            }
        })
    };
}

/// A `replace` hook: the trait method is not told the current value, so the
/// out field seeds as `nil`; the handler's value for it becomes `Some`.
macro_rules! gen_replace {
    ($name:ident, none, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?], $out:ident : $read:ident $(($rt:ty))?, $ret:ty) => {
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
                $(gen_seed!(self, table, lua, (), (), _event, $field : $seed $(($st))?);)*
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
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                gen_replace_out!(table, lua, $out : $read $(($rt))?)
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
    ($name:ident, game_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?], $out:ident : $read:ident $(($rt:ty))?, $ret:ty) => {
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
                $(gen_seed!(self, table, lua, host, (), _event, $field : $seed $(($st))?);)*
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
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                gen_replace_out!(table, lua, $out : $read $(($rt))?)
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
    ($name:ident, game_ref, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?], $out:ident : $read:ident $(($rt:ty))?, $ret:ty) => {
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
                $(gen_seed!(self, table, lua, _host, (), _event, $field : $seed $(($st))?);)*
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
                $(gen_writeback!(table, lua, _event, (), $field $(, $write $(($wt))?)?);)*
                gen_replace_out!(table, lua, $out : $read $(($rt))?, _host)
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
    ($name:ident, level, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?], $out:ident : $read:ident $(($rt:ty))?, $ret:ty) => {
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
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed $(($st))?);)*
                table.set(stringify!($out), mlua::Value::Nil)?;
                let reader = Reader::Level(level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, level, $field $(, $write $(($wt))?)?);)*
                // Placement answers read the event's own `state` through
                // the level; other readers ignore the trailing resolvers.
                gen_replace_out!(table, lua, $out : $read $(($rt))?, level, _event.state)
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
    ($name:ident, level_mut, $event:ty, $kind:tt, [$($field:ident : $seed:ident $(($st:ty))? $(=> $write:ident $(($wt:ty))?)?),* $(,)?], $out:ident : $read:ident $(($rt:ty))?, $ret:ty) => {
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
                $(gen_seed!(self, table, lua, (), level, _event, $field : $seed $(($st))?);)*
                table.set(stringify!($out), mlua::Value::Nil)?;
                let reader = Reader::Level(&*level);
                for (action, _args) in self.run_handlers(hook, kind.as_deref(), reader, table.clone())? {
                    match action.as_str() {
                        _ => self.warn(format!("{hook}: {action} needs a host; dropped")),
                    }
                }
                $(gen_writeback!(table, lua, _event, level, $field $(, $write $(($wt))?)?);)*
                // Placement answers read the event's own `state` through
                // the level; other readers ignore the trailing resolvers.
                gen_replace_out!(table, lua, $out : $read $(($rt))?, level, _event.state)
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
