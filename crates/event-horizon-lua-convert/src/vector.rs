//! Vectors, block positions and rotations between Lua and Rust.
//!
//! A vector or position is a table `{x=, y=, z=}` (two-dimensional: `{x=, y=}`),
//! and a rotation is `{pitch=, yaw=}`. A Luau `vector` is accepted on input for
//! three components, but output is always a table: a Luau vector holds f32 only.
//! Each component goes through the scalar conversions, so integer and finiteness
//! rules apply. A missing component is an error; extra keys are ignored.

use mlua::{Lua, Table, Value};
use mod_api::BlockPos;

use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};

const XYZ: [&str; 3] = ["x", "y", "z"];

/// A rotation: `{pitch=, yaw=}` in Lua.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rotation {
    pub pitch: f32,
    pub yaw: f32,
}

impl FromLuaValue for Rotation {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let [pitch, yaw] = from_components::<f32, 2>(value, ["pitch", "yaw"])?;
        Ok(Self { pitch, yaw })
    }
}

impl ToLuaValue for Rotation {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        to_table(lua, ["pitch", "yaw"], &[self.pitch, self.yaw])
    }
}

impl FromLuaValue for BlockPos {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let [x, y, z] = from_components::<i32, 3>(value, XYZ)?;
        Ok(BlockPos::new(x, y, z))
    }
}

impl ToLuaValue for BlockPos {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        to_table(lua, XYZ, &[self.x, self.y, self.z])
    }
}

macro_rules! array_conversions {
    ($($ty:ty, $len:literal, [$($name:literal),+];)+) => {$(
        impl FromLuaValue for [$ty; $len] {
            fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
                from_components(value, [$($name),+])
            }
        }

        impl ToLuaValue for [$ty; $len] {
            fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
                to_table(lua, [$($name),+], self)
            }
        }
    )+};
}

array_conversions! {
    i32, 3, ["x", "y", "z"];
    i32, 2, ["x", "y"];
    f64, 3, ["x", "y", "z"];
    f64, 2, ["x", "y"];
    f32, 3, ["x", "y", "z"];
    f32, 2, ["x", "y"];
}

/// Reads `N` named components of a table, or of a Luau vector for `N == 3`.
fn from_components<T: FromLuaValue, const N: usize>(
    value: &Value,
    names: [&str; N],
) -> Result<[T; N], ConvertError> {
    let parts: Vec<Value> = match value {
        Value::Table(table) => names
            .iter()
            .map(|name| component(table, name))
            .collect::<Result<_, _>>()?,
        Value::Vector(v) if N == 3 => [v.x(), v.y(), v.z()]
            .into_iter()
            .map(|c| Value::Number(f64::from(c)))
            .collect(),
        Value::Vector(_) => return Err(ConvertError::expected("table", value)),
        other => return Err(ConvertError::expected("vector", other)),
    };
    let values = parts
        .iter()
        .zip(names)
        .map(|(part, name)| {
            T::from_lua_value(part).map_err(|e| ConvertError::new(format!("component {name}: {e}")))
        })
        .collect::<Result<Vec<T>, _>>()?;
    values
        .try_into()
        .map_err(|_| ConvertError::new(format!("expected {N} components")))
}

/// One named component of a table, read without metamethods. A nil one is missing.
fn component(table: &Table, name: &str) -> Result<Value, ConvertError> {
    let part: Value = table
        .raw_get(name)
        .map_err(|_| ConvertError::new(format!("cannot read component {name}")))?;
    if part.is_nil() {
        return Err(ConvertError::new(format!("missing component {name}")));
    }
    Ok(part)
}

/// A table with the named components written from the values.
fn to_table<T: ToLuaValue, const N: usize>(
    lua: &Lua,
    names: [&str; N],
    values: &[T; N],
) -> mlua::Result<Value> {
    let table = lua.create_table()?;
    for (name, value) in names.into_iter().zip(values) {
        table.raw_set(name, value.to_lua_value(lua)?)?;
    }
    Ok(Value::Table(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(lua: &Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk runs")
    }

    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    /// A table built from `(key, value)` pairs.
    fn table(lua: &Lua, fields: &[(&str, Value)]) -> Value {
        let t = lua.create_table().expect("table");
        for (key, value) in fields {
            t.raw_set(*key, value.clone()).expect("set");
        }
        Value::Table(t)
    }

    #[test]
    fn int_array_round_trips_through_a_table() {
        let lua = Lua::new();
        let v = [3i32, -4, 64].to_lua_value(&lua).expect("to lua");
        let Value::Table(t) = &v else {
            panic!("expected a table");
        };
        assert_eq!(t.raw_get::<i32>("y").expect("y"), -4);
        assert_eq!(<[i32; 3]>::from_lua_value(&v), Ok([3, -4, 64]));
        let two = table(&lua, &[("x", num(1.0)), ("y", num(2.0))]);
        assert_eq!(<[i32; 2]>::from_lua_value(&two), Ok([1, 2]));
    }

    #[test]
    fn float_arrays_round_trip() {
        let lua = Lua::new();
        let v = [0.25f64, -1.5, 1e9].to_lua_value(&lua).expect("to lua");
        assert_eq!(<[f64; 3]>::from_lua_value(&v), Ok([0.25, -1.5, 1e9]));
        let v = [0.5f64, 2.0].to_lua_value(&lua).expect("to lua");
        assert_eq!(<[f64; 2]>::from_lua_value(&v), Ok([0.5, 2.0]));
        let v = [0.5f32, -0.75, 8.0].to_lua_value(&lua).expect("to lua");
        assert_eq!(<[f32; 3]>::from_lua_value(&v), Ok([0.5, -0.75, 8.0]));
        let v = [0.5f32, 2.0].to_lua_value(&lua).expect("to lua");
        assert_eq!(<[f32; 2]>::from_lua_value(&v), Ok([0.5, 2.0]));
    }

    #[test]
    fn missing_component_is_an_error_that_names_it() {
        let lua = Lua::new();
        let t = table(&lua, &[("x", num(1.0)), ("y", num(2.0))]);
        let err = <[i32; 3]>::from_lua_value(&t).expect_err("missing z");
        assert!(err.to_string().contains("missing component z"), "{err}");
    }

    #[test]
    fn fractional_integer_component_is_an_error_that_names_it() {
        let lua = Lua::new();
        let t = table(&lua, &[("x", num(1.5)), ("y", num(0.0)), ("z", num(0.0))]);
        let err = <[i32; 3]>::from_lua_value(&t).expect_err("fraction");
        let message = err.to_string();
        assert!(message.contains("component x"), "{message}");
        assert!(message.contains("expected integer"), "{message}");
    }

    #[test]
    fn nan_component_is_an_error() {
        let lua = Lua::new();
        let nan = eval(&lua, "return 0/0");
        let t = table(&lua, &[("x", nan), ("y", num(0.0)), ("z", num(0.0))]);
        let err = <[f64; 3]>::from_lua_value(&t).expect_err("nan");
        assert!(err.to_string().contains("component x"), "{err}");
        assert!(
            <[f32; 2]>::from_lua_value(&table(&lua, &[("x", num(f64::INFINITY)), ("y", num(0.0))]))
                .is_err()
        );
    }

    #[test]
    fn luau_vector_is_accepted_for_three_components() {
        let lua = Lua::new();
        let v = eval(&lua, "return vector.create(1,2,3)");
        assert!(matches!(v, Value::Vector(_)));
        assert_eq!(<[f64; 3]>::from_lua_value(&v), Ok([1.0, 2.0, 3.0]));
        assert_eq!(<[i32; 3]>::from_lua_value(&v), Ok([1, 2, 3]));
        assert_eq!(BlockPos::from_lua_value(&v), Ok(BlockPos::new(1, 2, 3)));
    }

    #[test]
    fn luau_vector_with_a_fraction_is_not_an_integer_position() {
        let lua = Lua::new();
        let v = eval(&lua, "return vector.create(1.5,0,0)");
        assert!(<[i32; 3]>::from_lua_value(&v).is_err());
        assert_eq!(<[f64; 3]>::from_lua_value(&v), Ok([1.5, 0.0, 0.0]));
    }

    #[test]
    fn luau_vector_is_not_a_two_component_value() {
        let lua = Lua::new();
        let v = eval(&lua, "return vector.create(1,2,3)");
        assert!(<[i32; 2]>::from_lua_value(&v).is_err());
    }

    #[test]
    fn absent_optional_vector_is_none() {
        let lua = Lua::new();
        assert_eq!(Option::<[i32; 3]>::from_lua_value(&Value::Nil), Ok(None));
        let t = table(&lua, &[("x", num(4.0)), ("y", num(5.0)), ("z", num(6.0))]);
        assert_eq!(Option::<[i32; 3]>::from_lua_value(&t), Ok(Some([4, 5, 6])));
    }

    #[test]
    fn rotation_round_trips() {
        let lua = Lua::new();
        let r = Rotation {
            pitch: 90.0,
            yaw: -45.5,
        };
        let v = r.to_lua_value(&lua).expect("to lua");
        assert_eq!(Rotation::from_lua_value(&v), Ok(r));
        let t = table(&lua, &[("pitch", num(0.5)), ("yaw", num(-1.25))]);
        assert_eq!(
            Rotation::from_lua_value(&t),
            Ok(Rotation {
                pitch: 0.5,
                yaw: -1.25
            })
        );
    }

    #[test]
    fn block_pos_round_trips_through_its_fields() {
        let lua = Lua::new();
        let p = BlockPos::new(-30_000_000, 64, 12);
        let v = p.to_lua_value(&lua).expect("to lua");
        assert_eq!(BlockPos::from_lua_value(&v), Ok(p));
        let t = table(&lua, &[("x", num(1.0)), ("y", num(2.0)), ("z", num(3.0))]);
        assert_eq!(BlockPos::from_lua_value(&t), Ok(BlockPos::new(1, 2, 3)));
    }

    #[test]
    fn extra_keys_are_ignored() {
        let lua = Lua::new();
        let t = table(
            &lua,
            &[
                ("x", num(1.0)),
                ("y", num(2.0)),
                ("z", num(3.0)),
                ("w", num(4.0)),
                (
                    "name",
                    Value::String(lua.create_string("a").expect("string")),
                ),
            ],
        );
        assert_eq!(<[i32; 3]>::from_lua_value(&t), Ok([1, 2, 3]));
    }

    #[test]
    fn positional_array_is_not_a_vector() {
        let lua = Lua::new();
        let t = eval(&lua, "return {1, 2, 3}");
        let err = <[i32; 3]>::from_lua_value(&t).expect_err("positional");
        assert!(err.to_string().contains("missing component x"), "{err}");
    }

    #[test]
    fn string_is_rejected_as_a_vector() {
        let lua = Lua::new();
        let s = Value::String(lua.create_string("1,2,3").expect("string"));
        let err = <[i32; 3]>::from_lua_value(&s).expect_err("string");
        assert_eq!(err.to_string(), "expected vector, got string");
        assert!(Rotation::from_lua_value(&num(1.0)).is_err());
    }
}
