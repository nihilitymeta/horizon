//! JSON values (`serde_json::Value`) between Lua tables and Rust: a loot table's JSON as a hook
//! sees it, and the JSON a loot function or condition's data holds.
//!
//! An object is a Lua table with string keys, an array is a sequence table (keys 1 to n). A non-empty
//! table is read by its keys. An empty table is read by its shape: an empty JSON object goes to Lua
//! as an empty table carrying the object marker (a metatable one per Lua state, kept in the
//! registry, with no metamethods and `__metatable` false, so `getmetatable` shows nothing), and an
//! empty JSON array goes to Lua as a plain empty table. Coming back, a marked empty table is an
//! empty object and any other empty table is an empty array, so `"conditions": []` and
//! `"predicate": {}` round-trip as they were given, and a mod's own `{}` is an array. A JSON `null`
//! inside an object is omitted (the key is absent), while a `null` in an array or at the top is
//! refused: Lua has no value for it there. A number that is a whole number of at most 2^53 in
//! magnitude is an integer, any other finite number is a float.
//!
//! The data comes from mods, so both directions are bounded ([`MAX_DEPTH`] levels, [`MAX_NODES`]
//! values) and strict: a mixed table, a table with a hole, a non-string key, a function, a userdata
//! or a non-finite number is an error. Nothing panics.

use mlua::{Lua, Table, Value};
use serde_json::{Map, Number};

/// The deepest nesting of arrays and objects, the top value counting as 1.
pub const MAX_DEPTH: usize = 64;
/// The most values (arrays, objects and scalars) one JSON value may hold, the top included.
pub const MAX_NODES: usize = 100_000;

/// 2^53: the whole numbers a Lua number holds exactly.
const MAX_EXACT: f64 = 9_007_199_254_740_992.0;
/// The same bound as an integer.
const MAX_EXACT_INT: i64 = 1 << 53;

/// Counts the values of one JSON value against [`MAX_NODES`], and checks [`MAX_DEPTH`].
#[derive(Default)]
struct Budget {
    nodes: usize,
}

impl Budget {
    fn enter(&mut self, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(format!("JSON nests deeper than {MAX_DEPTH} levels"));
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(format!("JSON has more than {MAX_NODES} values"));
        }
        Ok(())
    }
}

/// Whether `value` can be written to Lua: the same checks as [`to_lua`], without a Lua state.
/// `Some(())` when it can. A hook whose JSON cannot be written is not delivered.
pub fn fits(value: &serde_json::Value) -> Option<()> {
    let mut budget = Budget::default();
    check(value, 1, &mut budget).ok()
}

fn check(value: &serde_json::Value, depth: usize, budget: &mut Budget) -> Result<(), String> {
    budget.enter(depth)?;
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                if item.is_null() {
                    return Err("json null in an array is not carried".to_owned());
                }
                check(item, depth + 1, budget)?;
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values().filter(|item| !item.is_null()) {
                check(item, depth + 1, budget)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The Lua value of a JSON value: a table for an array or an object (an empty object carries the
/// object marker, see the module's rules), a string, a boolean, or a number (an integer when it is a
/// whole number of at most 2^53 in magnitude). A top-level or array `null` is an error, and an
/// object's `null` keys are left out.
pub fn to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    if value.is_null() {
        return Err(mlua::Error::runtime("json null at the top is not carried"));
    }
    write(lua, value, 1, &mut Budget::default()).map_err(mlua::Error::runtime)
}

fn write(
    lua: &Lua,
    value: &serde_json::Value,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, String> {
    budget.enter(depth)?;
    let runtime = |e: mlua::Error| e.to_string();
    Ok(match value {
        serde_json::Value::Null => return Err("json null in an array is not carried".to_owned()),
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => number_to_lua(n),
        serde_json::Value::String(s) => Value::String(lua.create_string(s).map_err(runtime)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table().map_err(runtime)?;
            for (index, item) in items.iter().enumerate() {
                let item = write(lua, item, depth + 1, budget)?;
                table.raw_set(index + 1, item).map_err(runtime)?;
            }
            Value::Table(table)
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table().map_err(runtime)?;
            for (key, item) in map.iter().filter(|(_, item)| !item.is_null()) {
                let item = write(lua, item, depth + 1, budget)?;
                table.raw_set(key.as_str(), item).map_err(runtime)?;
            }
            if map.values().all(serde_json::Value::is_null) {
                table
                    .set_metatable(Some(object_marker(lua).map_err(runtime)?))
                    .map_err(runtime)?;
            }
            Value::Table(table)
        }
    })
}

/// The Lua registry name of the object marker, and the key that marks a metatable as one.
const OBJECT_MARKER: &str = "horizon.json_object";
const MARKER_KEY: &str = "json_object";

/// The marker metatable of an empty JSON object, one per Lua state. It has no metamethods, and
/// `__metatable` is false, so a mod's `getmetatable` returns false and cannot reach the marker.
fn object_marker(lua: &Lua) -> mlua::Result<Table> {
    if let Value::Table(marker) = lua.named_registry_value::<Value>(OBJECT_MARKER)? {
        return Ok(marker);
    }
    let marker = lua.create_table()?;
    marker.raw_set(MARKER_KEY, true)?;
    marker.raw_set("__metatable", false)?;
    lua.set_named_registry_value(OBJECT_MARKER, marker.clone())?;
    Ok(marker)
}

/// Whether a table carries the object marker.
fn is_object_marker(table: &Table) -> bool {
    table.metatable().is_some_and(|marker| {
        matches!(
            marker.raw_get::<Value>(MARKER_KEY),
            Ok(Value::Boolean(true))
        )
    })
}

fn number_to_lua(n: &Number) -> Value {
    match n.as_i64() {
        Some(i) if (-MAX_EXACT_INT..=MAX_EXACT_INT).contains(&i) => Value::Integer(i),
        _ => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
    }
}

/// The JSON value of a Lua value that must be a table: a loot table's JSON is an object, so a hook's
/// output that is not a table (`event.table = 5`) is refused. Otherwise as [`from_lua`].
pub fn from_lua_table(value: &Value) -> Result<serde_json::Value, String> {
    if !matches!(value, Value::Table(_)) {
        return Err(format!("expected a table, got {}", value.type_name()));
    }
    from_lua(value)
}

/// The JSON value of a Lua value (see the module's rules). An empty table with the object marker is
/// an empty object, any other empty table an empty array. A table that is neither a sequence nor
/// string-keyed is an error, and so is a value Lua cannot hold as JSON (a function, a userdata, a
/// thread, nil, a non-finite number).
pub fn from_lua(value: &Value) -> Result<serde_json::Value, String> {
    read(value, 1, &mut Budget::default())
}

fn read(value: &Value, depth: usize, budget: &mut Budget) -> Result<serde_json::Value, String> {
    budget.enter(depth)?;
    Ok(match value {
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => number_from(*i as f64)?,
        Value::Number(n) => number_from(*n)?,
        Value::String(s) => {
            let bytes = s.as_bytes();
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| "expected UTF-8 string, got invalid bytes".to_owned())?;
            serde_json::Value::String(text.to_owned())
        }
        Value::Table(table) => table_from(table, depth, budget)?,
        other => return Err(format!("expected JSON value, got {}", other.type_name())),
    })
}

/// A Lua number as JSON: a whole number of at most 2^53 in magnitude is an integer, any other
/// finite number a float. NaN and the infinities are refused.
fn number_from(n: f64) -> Result<serde_json::Value, String> {
    if !n.is_finite() {
        return Err(format!("expected finite number, got {n}"));
    }
    if n.fract() == 0.0 && n.abs() <= MAX_EXACT {
        return Ok(serde_json::Value::Number(Number::from(n as i64)));
    }
    Number::from_f64(n)
        .map(serde_json::Value::Number)
        .ok_or_else(|| format!("expected finite number, got {n}"))
}

fn table_from(
    table: &Table,
    depth: usize,
    budget: &mut Budget,
) -> Result<serde_json::Value, String> {
    let mut entries = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|e| e.to_string())?;
        entries.push((key, value));
    }
    if entries.is_empty() {
        return Ok(if is_object_marker(table) {
            serde_json::Value::Object(Map::new())
        } else {
            serde_json::Value::Array(Vec::new())
        });
    }
    if entries
        .iter()
        .all(|(key, _)| matches!(key, Value::String(_)))
    {
        let mut map = Map::new();
        for (key, value) in entries {
            let Value::String(key) = key else {
                return Err("object keys must be strings".to_owned());
            };
            let bytes = key.as_bytes();
            let key = std::str::from_utf8(&bytes)
                .map_err(|_| "expected UTF-8 key, got invalid bytes".to_owned())?
                .to_owned();
            map.insert(key, read(&value, depth + 1, budget)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    sequence(entries, depth, budget)
}

/// A table whose keys are exactly 1 to n: an array in order. Anything else is an error.
fn sequence(
    entries: Vec<(Value, Value)>,
    depth: usize,
    budget: &mut Budget,
) -> Result<serde_json::Value, String> {
    let len = entries.len();
    let mut slots: Vec<Option<Value>> = vec![None; len];
    for (key, value) in entries {
        let index = match key {
            Value::Integer(i) => usize::try_from(i).ok(),
            Value::Number(n) if n.fract() == 0.0 && n >= 1.0 => Some(n as usize),
            _ => None,
        }
        .filter(|i| (1..=len).contains(i))
        .ok_or_else(|| "a table must have only string keys, or keys 1 to n".to_owned())?;
        slots[index - 1] = Some(value);
    }
    let values = slots
        .into_iter()
        .collect::<Option<Vec<Value>>>()
        .ok_or_else(|| "a table must have only string keys, or keys 1 to n".to_owned())?;
    let mut items = Vec::with_capacity(len);
    for value in &values {
        items.push(read(value, depth + 1, budget)?);
    }
    Ok(serde_json::Value::Array(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(lua: &Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk runs")
    }

    /// The Lua value of `source`, read back as JSON.
    fn read_source(source: &str) -> Result<serde_json::Value, String> {
        let lua = Lua::new();
        from_lua(&eval(&lua, source))
    }

    #[test]
    fn nested_objects_arrays_strings_and_numbers_round_trip() {
        let json = serde_json::json!({
            "pools": [{"rolls": 3, "entries": [{"type": "item", "name": "minecraft:stick"}]}],
            "half": 0.5,
            "flag": true,
            "text": "hi"
        });
        let lua = Lua::new();
        let back = from_lua(&to_lua(&lua, &json).expect("to lua")).expect("from lua");
        assert_eq!(back, json);
    }

    #[test]
    fn a_whole_number_stays_an_integer_and_a_fraction_stays_a_fraction() {
        assert_eq!(
            read_source("return 3").expect("three"),
            serde_json::json!(3)
        );
        assert_eq!(
            read_source("return 0.5").expect("half"),
            serde_json::json!(0.5)
        );
    }

    #[test]
    fn a_null_in_an_object_is_left_out() {
        let lua = Lua::new();
        let json = serde_json::json!({"a": null, "b": 1});
        let table = to_lua(&lua, &json).expect("to lua");
        let Value::Table(table) = table else {
            panic!("a table")
        };
        assert_eq!(table.raw_get::<Value>("a").expect("get"), Value::Nil);
        assert_eq!(table.raw_get::<i64>("b").expect("get"), 1);
    }

    #[test]
    fn a_null_in_an_array_is_refused() {
        let lua = Lua::new();
        let json = serde_json::json!([1, null]);
        let error = to_lua(&lua, &json).expect_err("refused");
        assert!(
            error
                .to_string()
                .contains("json null in an array is not carried")
        );
        assert!(fits(&json).is_none());
    }

    #[test]
    fn a_table_with_a_hole_is_refused() {
        assert!(read_source("return {1, nil, 3}").is_err());
    }

    #[test]
    fn a_mixed_table_is_refused() {
        assert!(read_source("return {1, a = 2}").is_err());
    }

    #[test]
    fn empty_arrays_and_objects_round_trip_as_they_were_given() {
        let lua = Lua::new();
        let json = serde_json::json!({"conditions": [], "predicate": {}});
        let back = from_lua(&to_lua(&lua, &json).expect("to lua")).expect("from lua");
        assert_eq!(back, json);
    }

    #[test]
    fn a_plain_empty_table_is_an_empty_array() {
        assert_eq!(
            read_source("return {}").expect("empty"),
            serde_json::json!([])
        );
    }

    #[test]
    fn the_object_marker_is_hidden_from_get_metatable() {
        let lua = Lua::new();
        let json = serde_json::json!({"predicate": {}});
        let value = to_lua(&lua, &json).expect("to lua");
        lua.globals().raw_set("t", value).expect("global");
        let hidden: bool = lua
            .load("return getmetatable(t.predicate) == false")
            .eval()
            .expect("chunk runs");
        assert!(hidden);
    }

    #[test]
    fn nan_is_refused() {
        assert!(read_source("return 0/0").is_err());
    }

    #[test]
    fn a_value_past_the_depth_limit_is_refused() {
        let lua = Lua::new();
        let mut json = serde_json::json!(1);
        for _ in 0..MAX_DEPTH {
            json = serde_json::json!([json]);
        }
        assert!(fits(&json).is_none());
        assert!(to_lua(&lua, &json).is_err());
        let mut shallow = serde_json::json!(1);
        for _ in 0..MAX_DEPTH - 1 {
            shallow = serde_json::json!([shallow]);
        }
        assert!(fits(&shallow).is_some());
    }
}
