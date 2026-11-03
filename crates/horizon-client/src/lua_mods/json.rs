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

/// How deep a table may nest on its way to JSON: stops here, with the path
/// to it in the error.
pub const MAX_DEPTH: usize = 128;

/// The walk down a table: the keys from the root to the value in hand (an
/// error names them) and the tables on that path, so a table that holds
/// itself is an error and not a stack that never ends.
#[derive(Default)]
struct Walk {
    path: Vec<String>,
    tables: Vec<*const std::ffi::c_void>,
}
impl Walk {
    fn at(&self) -> String {
        if self.path.is_empty() {
            "the table".to_owned()
        } else {
            self.path.concat()
        }
    }
    fn enter(&mut self, t: &Table) -> mlua::Result<()> {
        let p = t.to_pointer();
        if self.tables.contains(&p) {
            return Err(mlua::Error::runtime(format!(
                "{}: a table that holds itself (a cycle) has no JSON form",
                self.at()
            )));
        }
        if self.tables.len() >= MAX_DEPTH {
            return Err(mlua::Error::runtime(format!(
                "{}: nested deeper than {MAX_DEPTH} tables",
                self.at()
            )));
        }
        self.tables.push(p);
        Ok(())
    }
    fn leave(&mut self) {
        self.tables.pop();
    }
    fn under<R>(
        &mut self,
        segment: String,
        f: impl FnOnce(&mut Self) -> mlua::Result<R>,
    ) -> mlua::Result<R> {
        self.path.push(segment);
        let r = f(self);
        self.path.pop();
        r
    }
}

/// A Lua value as JSON: a table with the keys 1..n (and no others) is an
/// array, any other table an object with string keys (a number key is
/// written as text); a function or userdata is an error, as is a table
/// nested past [`MAX_DEPTH`] or holding itself (the error names the key path).
pub fn lua_to_json(value: &Value) -> mlua::Result<Json> {
    Walk::default().json(value)
}

impl Walk {
    fn json(&mut self, value: &Value) -> mlua::Result<Json> {
        Ok(match value {
            Value::Nil => Json::Null,
            Value::Boolean(b) => Json::Bool(*b),
            Value::Integer(i) => Json::Number((*i).into()),
            Value::Number(n) => match Number::from_f64(*n) {
                Some(n) => Json::Number(n),
                None => {
                    return Err(mlua::Error::runtime(format!(
                        "{}: {n} has no JSON form",
                        self.at()
                    )));
                }
            },
            Value::String(s) => Json::String(s.to_str()?.to_owned()),
            Value::Table(t) => {
                self.enter(t)?;
                let json = self.table(t)?;
                self.leave();
                json
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "{}: a {} has no JSON form",
                    self.at(),
                    other.type_name()
                )));
            }
        })
    }

    fn table(&mut self, t: &Table) -> mlua::Result<Json> {
        let len = t.raw_len();
        let mut keys = 0;
        for pair in t.pairs::<Value, Value>() {
            let (k, _): (Value, Value) = pair?;
            keys += 1;
            if !matches!(k, Value::String(_) | Value::Integer(_) | Value::Number(_)) {
                return Err(mlua::Error::runtime(format!(
                    "{}: a {} is no JSON key",
                    self.at(),
                    k.type_name()
                )));
            }
        }
        if len > 0 && keys == len {
            let mut items = Vec::with_capacity(len);
            for i in 1..=len {
                let item: Value = t.raw_get(i)?;
                items.push(self.under(format!("[{i}]"), |w| w.json(&item))?);
            }
            return Ok(Json::Array(items));
        }
        let mut object = Map::new();
        for pair in t.pairs::<Value, Value>() {
            let (k, v) = pair?;
            let key = match &k {
                Value::String(s) => s.to_str()?.to_owned(),
                Value::Integer(i) => i.to_string(),
                Value::Number(n) => n.to_string(),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "{}: a {} is no JSON key",
                        self.at(),
                        other.type_name()
                    )));
                }
            };
            let segment = if self.path.is_empty() {
                key.clone()
            } else {
                format!(".{key}")
            };
            let value = self.under(segment, |w| w.json(&v))?;
            object.insert(key, value);
        }
        Ok(Json::Object(object))
    }
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

    #[test]
    fn a_cycle_or_a_deep_nest_is_an_error_naming_the_path() {
        let lua = Lua::new();
        let t: Table = lua
            .load("local d = { items = { { count = 1 } } }; d.items[1].me = d; return d")
            .eval()
            .unwrap();
        let e = lua_to_json(&Value::Table(t)).unwrap_err().to_string();
        assert!(
            e.contains("items[1].me") && e.contains("holds itself"),
            "{e}"
        );
        let t: Table = lua
            .load("local t = {} for i = 1, 1000 do t = { t } end return { deep = t }")
            .eval()
            .unwrap();
        let e = lua_to_json(&Value::Table(t)).unwrap_err().to_string();
        assert!(e.contains("deeper than") && e.contains("deep[1][1]"), "{e}");
        // A table reached twice, not on its own path, is written twice.
        let t: Table = lua
            .load("local shared = { x = 1 } return { a = shared, b = { shared } }")
            .eval()
            .unwrap();
        let json = lua_to_json(&Value::Table(t)).unwrap();
        assert_eq!(json["a"], json["b"][0]);
    }
}
