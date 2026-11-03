//! JSON as Lua values and back, for a mod's configuration file
//! (`horizon.config`, `horizon.save_config`) and the data files it reads
//! (`horizon.data`): an object is a table, an array a sequence, numbers are
//! numbers (an integer stays one), strings strings, `null` is nil (so a key
//! set to null is gone from the table).
use mlua::{Lua, Table, Value};
use serde_json::{Map, Number, Value as Json};

/// A JSON value as a Lua value.
pub fn json_to_lua(lua: &Lua, json: &Json) -> mlua::Result<Value> {
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        },
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(items) => {
            let t = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        Json::Object(map) => {
            let t = lua.create_table()?;
            for (k, v) in map {
                t.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}

/// A Lua value as JSON: a table with the keys 1..n (and no others) is an
/// array, any other table an object with string keys (a number key is
/// written as text); a function or userdata is an error.
pub fn lua_to_json(value: &Value) -> mlua::Result<Json> {
    Ok(match value {
        Value::Nil => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => Json::Number((*i).into()),
        Value::Number(n) => match Number::from_f64(*n) {
            Some(n) => Json::Number(n),
            None => return Err(mlua::Error::runtime(format!("{n} has no JSON form"))),
        },
        Value::String(s) => Json::String(s.to_str()?.to_owned()),
        Value::Table(t) => table_to_json(t)?,
        other => {
            return Err(mlua::Error::runtime(format!(
                "a {} has no JSON form",
                other.type_name()
            )));
        }
    })
}

fn table_to_json(t: &Table) -> mlua::Result<Json> {
    let len = t.raw_len();
    let mut keys = 0;
    let mut object = Map::new();
    for pair in t.pairs::<Value, Value>() {
        let (k, v) = pair?;
        keys += 1;
        let key = match &k {
            Value::String(s) => s.to_str()?.to_owned(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => n.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "a {} is no JSON key",
                    other.type_name()
                )));
            }
        };
        object.insert(key, lua_to_json(&v)?);
    }
    if len > 0 && keys == len {
        let mut items = Vec::with_capacity(len);
        for i in 1..=len {
            items.push(lua_to_json(&t.raw_get::<Value>(i)?)?);
        }
        return Ok(Json::Array(items));
    }
    Ok(Json::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_lua_round_trip_with_arrays_objects_and_number_kinds() {
        let lua = Lua::new();
        let json: Json = serde_json::from_str(
            r#"{"limit": 16, "rate": 0.5, "on": true, "name": "hive", "kinds": ["a", "b"], "nested": {"x": [1, {"y": null}]}, "empty": {}}"#,
        )
        .unwrap();
        let value = json_to_lua(&lua, &json).unwrap();
        let t = value.as_table().unwrap();
        assert_eq!(t.get::<i64>("limit").unwrap(), 16);
        assert_eq!(t.get::<f64>("rate").unwrap(), 0.5);
        assert_eq!(t.get::<Vec<String>>("kinds").unwrap(), ["a", "b"]);
        let back = lua_to_json(&value).unwrap();
        assert_eq!(back["limit"], 16);
        assert_eq!(back["rate"], 0.5);
        assert_eq!(back["kinds"], serde_json::json!(["a", "b"]));
        assert_eq!(back["nested"]["x"][0], 1);
        // A null is gone: the nested object lost its key.
        assert_eq!(back["nested"]["x"][1], serde_json::json!({}));
        assert_eq!(back["empty"], serde_json::json!({}));
        let f = lua.create_function(|_, ()| Ok(())).unwrap();
        assert!(lua_to_json(&Value::Function(f)).is_err());
    }
}
