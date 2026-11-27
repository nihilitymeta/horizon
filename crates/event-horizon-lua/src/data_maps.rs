//! Data maps a Lua mod declares (issue 74): the types it registers for `data_map_types` and the JSON
//! files it ships for `data_pack_files`, and the read `horizon.data_map` makes in a turn.
//!
//! The JSON is mod-supplied and it changes gameplay values, so every limit is checked before the
//! text is parsed, and a limit that is broken refuses that one declaration with a warning that
//! names the limit. The mod still loads and its other declarations stand. Nothing is cut short: a
//! file over a limit is not loaded in part.
//!
//! The ids are the game's: a namespace and a path of lowercase letters, digits, `_`, `-`, `.` and
//! `/` (`declare::check_path`), and a type's namespace is the mod's own (`declare::check_id`'s rule).

use std::cell::RefCell;

use event_horizon_lua_convert::scalar::ToLuaValue;
use mlua::{Error, Function, Lua, MultiValue, UserData, Value};
use mod_api::DataMapType;

use crate::declare::check_path;
use crate::hostcall::{NOT_HERE, TurnHost, host_call};

/// The types one mod may declare.
pub const MAX_DATA_MAP_TYPES: usize = 16;
/// The files one mod may ship.
pub const MAX_DATA_PACK_FILES: usize = 64;
/// The longest type id, in bytes.
pub const MAX_DATA_MAP_ID: usize = 64;
/// The largest JSON text of one file, in bytes.
pub const MAX_JSON_BYTES: usize = 256 * 1024;
/// The deepest nesting of arrays and objects in one file: the outer container is level 1.
pub const MAX_JSON_DEPTH: usize = 32;
/// The entries of one file: object members and array elements, all levels together.
pub const MAX_JSON_ENTRIES: usize = 4096;
/// The operation a `horizon.data_map` call is, in the turn's host call table.
pub(crate) const DATA_MAP_OP: &str = "data_map";
/// What one `horizon.data_map` read spends of the turn's budget.
pub(crate) const DATA_MAP_COST: u32 = 1;
/// The folder every data map file key starts with.
const KEY_PREFIX: &str = "data_maps/";

/// The userdata a `horizon.data_map` call passes to the turn's host. It holds nothing.
pub(crate) struct DataMapReader;

impl UserData for DataMapReader {}

/// Reads `horizon.data_map_type{id, registry}` and records the type. A refused type is logged and
/// left out; a call that is not a table with those two strings raises, as the other declarations do.
pub(crate) fn declare_type(
    owner: &str,
    value: Value,
    types: &RefCell<Vec<DataMapType>>,
) -> mlua::Result<()> {
    let [id, registry] = string_fields("data_map_type", value, ["id", "registry"])?;
    let mut types = types.borrow_mut();
    let refusal = if types.len() >= MAX_DATA_MAP_TYPES {
        Some(format!(
            "a mod may declare at most {MAX_DATA_MAP_TYPES} data map types"
        ))
    } else {
        check_type(owner, &id, &registry, &types).err()
    };
    match refusal {
        Some(reason) => refuse(owner, &format!("data map type '{id}'"), &reason),
        None => types.push(DataMapType { id, registry }),
    }
    Ok(())
}

/// The reason a type `id` for `registry` is refused, if any: its length and namespace, its id
/// declared once, and the registry's syntax (the game checks that the registry exists).
fn check_type(
    owner: &str,
    id: &str,
    registry: &str,
    declared: &[DataMapType],
) -> Result<(), String> {
    if id.len() > MAX_DATA_MAP_ID {
        return Err(format!("the id is longer than {MAX_DATA_MAP_ID} bytes"));
    }
    let Some((namespace, path)) = id.split_once(':') else {
        return Err(format!("needs a namespaced id such as {owner}:name"));
    };
    if namespace != owner {
        return Err(format!("is outside this mod's namespace '{owner}'"));
    }
    check_path(id, path)?;
    if declared.iter().any(|other| other.id == id) {
        return Err("is declared twice".to_owned());
    }
    check_registry(registry)
}

/// A registry id: `namespace:path`, both parts in the game's charset.
fn check_registry(registry: &str) -> Result<(), String> {
    let Some((namespace, path)) = registry.split_once(':') else {
        return Err(format!(
            "registry '{registry}' needs a namespaced id such as minecraft:item"
        ));
    };
    check_path(registry, namespace)?;
    check_path(registry, path)
}

/// Reads `horizon.data_pack_file{key, json}` and records the file. A refused file is logged and left
/// out. The key is `data_maps/<owner>:<registry folder>/<path>`, so a mod ships files only for its
/// own namespace's maps.
pub(crate) fn declare_file(
    owner: &str,
    value: Value,
    files: &RefCell<Vec<(String, String)>>,
) -> mlua::Result<()> {
    let [key, json] = string_fields("data_pack_file", value, ["key", "json"])?;
    let mut files = files.borrow_mut();
    let refusal = if files.len() >= MAX_DATA_PACK_FILES {
        Some(format!(
            "a mod may ship at most {MAX_DATA_PACK_FILES} data pack files"
        ))
    } else {
        check_key(owner, &key)
            .and_then(|()| check_json(&json).map(drop))
            .err()
    };
    match refusal {
        Some(reason) => refuse(owner, &format!("data pack file '{key}'"), &reason),
        None => files.push((key, json)),
    }
    Ok(())
}

/// The reason a data map file `key` is refused, if any: under `data_maps/` in the mod's namespace,
/// with a folder and a path in the game's charset and no empty or dot segment.
fn check_key(owner: &str, key: &str) -> Result<(), String> {
    let rest = key
        .strip_prefix(KEY_PREFIX)
        .ok_or_else(|| format!("needs a key under {KEY_PREFIX}"))?;
    let Some((namespace, path)) = rest.split_once(':') else {
        return Err(format!("needs a key such as {KEY_PREFIX}{owner}:item/name"));
    };
    if namespace != owner {
        return Err(format!("is outside this mod's namespace '{owner}'"));
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("has an empty or dot segment in its path".to_owned());
    }
    check_path(key, path)
}

/// The checks on a file's JSON text, in order: its size, its nesting (read without parsing, so a
/// deep document never reaches the parser), that it parses, and its entries. Returns the parsed
/// value.
pub(crate) fn check_json(text: &str) -> Result<serde_json::Value, String> {
    if text.len() > MAX_JSON_BYTES {
        return Err(format!(
            "its JSON is {} bytes, more than {MAX_JSON_BYTES}",
            text.len()
        ));
    }
    if nested_deeper_than(text, MAX_JSON_DEPTH) {
        return Err(format!(
            "its JSON nests deeper than {MAX_JSON_DEPTH} levels"
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("its JSON does not parse: {err}"))?;
    let entries = entries_of(&value);
    if entries > MAX_JSON_ENTRIES {
        return Err(format!(
            "its JSON has {entries} entries, more than {MAX_JSON_ENTRIES}"
        ));
    }
    Ok(value)
}

/// Whether `text` nests arrays or objects deeper than `limit`. Strings are skipped, escapes
/// included, so a bracket inside a string does not count. Reads bytes only: it never recurses.
fn nested_deeper_than(text: &str, limit: usize) -> bool {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// The object members and array elements of a parsed document, at every level. Walks with a stack,
/// so the count never recurses.
fn entries_of(value: &serde_json::Value) -> usize {
    let mut total = 0;
    let mut pending = vec![value];
    while let Some(next) = pending.pop() {
        match next {
            serde_json::Value::Array(items) => {
                total += items.len();
                pending.extend(items);
            }
            serde_json::Value::Object(members) => {
                total += members.len();
                pending.extend(members.values());
            }
            _ => {}
        }
    }
    total
}

/// Logs a refused declaration: the mod still loads, and the other declarations stand. Through
/// `warn_once`, so a mod that loops on refused calls cannot flood the log.
fn refuse(owner: &str, what: &str, reason: &str) {
    crate::host::warn_once(owner, &format!("{owner}: {what} refused: {reason}"));
}

/// The string fields of a declaring call's table, in the order of `names`. Each name is given once
/// and nothing else is in the table; a missing or non-string field raises.
fn string_fields<const N: usize>(
    call: &str,
    value: Value,
    names: [&str; N],
) -> mlua::Result<[String; N]> {
    let Value::Table(table) = value else {
        return Err(Error::runtime(format!(
            "horizon.{call} needs its fields as a table, not {}",
            value.type_name()
        )));
    };
    let mut slots: [Option<String>; N] = std::array::from_fn(|_| None);
    for pair in table.pairs::<Value, Value>() {
        let (key, field) = pair?;
        let name = match &key {
            Value::String(name) => name.to_str()?.to_string(),
            _ => {
                return Err(Error::runtime(format!(
                    "horizon.{call} field names are strings"
                )));
            }
        };
        let Some(index) = names.iter().position(|known| *known == name) else {
            return Err(Error::runtime(format!(
                "horizon.{call} has no field '{name}'; its fields are {}",
                names.join(" and ")
            )));
        };
        let Value::String(text) = field else {
            return Err(Error::runtime(format!(
                "horizon.{call} field '{name}' needs a string, not {}",
                field.type_name()
            )));
        };
        slots[index] = Some(text.to_str()?.to_string());
    }
    if let Some(missing) = names.iter().zip(&slots).find(|(_, slot)| slot.is_none()) {
        return Err(Error::runtime(format!(
            "horizon.{call} needs the field '{}'",
            missing.0
        )));
    }
    Ok(slots.map(|slot| slot.unwrap_or_default()))
}

/// The `horizon.data_map(registry, entry, map)` read: the value `entry` of registry `registry` has in
/// data map `map`, as NBT in Lua, or nil where it has none. Refused outside a turn with a host that
/// reads; a refusal by the game (no maps loaded, no such map, an undecodable value) raises.
pub(crate) fn data_map_op(lua: &Lua, host: &TurnHost<'_>, args: MultiValue) -> mlua::Result<Value> {
    let mut args = args.into_iter();
    let (Some(Value::String(registry)), Some(Value::String(entry)), Some(Value::String(map)), None) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return Err(Error::runtime(
            "horizon.data_map needs (registry, entry, map) as three strings",
        ));
    };
    let Some(read) = host.read() else {
        return Err(Error::runtime(NOT_HERE));
    };
    let registry = registry.to_str()?;
    let entry = entry.to_str()?;
    let map = map.to_str()?;
    match mod_api::data_map_value(read, &registry, &entry, &map) {
        Ok(Some(value)) => value.to_lua_value(lua),
        Ok(None) => Ok(Value::Nil),
        Err(err) => Err(Error::runtime(format!("horizon.data_map: {err:?}"))),
    }
}

/// The `horizon.data_map` function of a mod's runtime table. Each call is a host call of the turn, so
/// it is answered only while the turn is open; outside one it raises `no host call outside a hook`.
pub(crate) fn data_map_function(lua: &Lua) -> mlua::Result<Function> {
    let reader = lua.create_userdata(DataMapReader)?;
    lua.create_function(move |lua, args: MultiValue| {
        host_call(lua, DATA_MAP_OP, reader.clone(), args)
    })
}
