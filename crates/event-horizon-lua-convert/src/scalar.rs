//! Scalar conversions between Lua values and Rust: booleans, integers, floats,
//! strings, `Option`, `Duration` and plain enums (see [`lua_enum!`]).
//!
//! The values come from mods, so every conversion is strict: a value that does
//! not fit the Rust type is an error, never a silent cast, and nothing panics.

use std::fmt;
use std::time::Duration;

pub use mlua;
use mlua::{Lua, Value};

/// The largest number of seconds a Duration may hold when it comes from Lua.
const MAX_DURATION_SECS: f64 = 1e9;

/// Lua numbers at or above this magnitude are out of range for every integer type here.
const INTEGER_LIMIT: f64 = 1e30;

/// A Lua value that does not convert to the Rust type asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertError(String);

impl ConvertError {
    /// An error with the given message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The error for a value that is not of the `expected` kind.
    pub fn expected(expected: &str, found: &Value) -> Self {
        Self(format!("expected {expected}, got {}", found.type_name()))
    }

    /// The error for a string that is not one of a plain enum's names.
    pub fn unknown_variant(name: &str, allowed: &[&str]) -> Self {
        let allowed: Vec<String> = allowed.iter().map(|a| format!("{a:?}")).collect();
        Self(format!(
            "expected one of {}, got {name:?}",
            allowed.join(", ")
        ))
    }
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConvertError {}

/// Reads a Rust value from a Lua value.
pub trait FromLuaValue: Sized {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError>;
}

/// Writes a Rust value as a Lua value.
pub trait ToLuaValue {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value>;
}

/// The integer a Lua number holds, or an error if it has a fraction, is not finite,
/// or is too large to be an integer of any type here.
fn integer_from_number(n: f64, ty: &str) -> Result<i128, ConvertError> {
    if !n.is_finite() || n.fract() != 0.0 {
        return Err(ConvertError::new(format!(
            "expected integer, got number {n}"
        )));
    }
    if n.abs() >= INTEGER_LIMIT {
        return Err(ConvertError::new(format!("expected {ty}, got {n}")));
    }
    let wide = n as i128;
    if wide as f64 != n {
        return Err(ConvertError::new(format!(
            "expected integer, got number {n}"
        )));
    }
    Ok(wide)
}

/// The Lua number for an integer, or an error if it does not survive the trip to a
/// Lua number (Lua numbers are doubles, so integers above 2^53 may not).
fn number_from_integer(i: i64) -> Result<f64, ConvertError> {
    let n = i as f64;
    if n as i128 != i128::from(i) {
        return Err(ConvertError::new(format!(
            "integer {i} is not exact as a Lua number"
        )));
    }
    Ok(n)
}

fn finite(n: f64) -> Result<f64, ConvertError> {
    if n.is_finite() {
        Ok(n)
    } else {
        Err(ConvertError::new(format!(
            "expected finite number, got {n}"
        )))
    }
}

impl FromLuaValue for bool {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        match value {
            Value::Boolean(b) => Ok(*b),
            other => Err(ConvertError::expected("boolean", other)),
        }
    }
}

impl ToLuaValue for bool {
    fn to_lua_value(&self, _lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::Boolean(*self))
    }
}

macro_rules! int_conversions {
    ($($ty:ty),+ $(,)?) => {$(
        impl FromLuaValue for $ty {
            fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
                let wide = match value {
                    Value::Integer(i) => i128::from(*i),
                    Value::Number(n) => integer_from_number(*n, stringify!($ty))?,
                    other => return Err(ConvertError::expected("integer", other)),
                };
                <$ty>::try_from(wide)
                    .map_err(|_| ConvertError::new(format!("expected {}, got {wide}", stringify!($ty))))
            }
        }

        impl ToLuaValue for $ty {
            fn to_lua_value(&self, _lua: &Lua) -> mlua::Result<Value> {
                let wide = i128::from(*self);
                if wide as f64 as i128 != wide {
                    return Err(mlua::Error::runtime(format!("integer {wide} is not exact as a Lua number")));
                }
                Ok(match i64::try_from(wide) {
                    Ok(i) => Value::Integer(i),
                    Err(_) => Value::Number(wide as f64),
                })
            }
        }
    )+};
}

int_conversions!(i8, u8, i16, u16, i32, u32, i64, u64);

impl FromLuaValue for f64 {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        match value {
            Value::Number(n) => finite(*n),
            Value::Integer(i) => finite(number_from_integer(*i)?),
            other => Err(ConvertError::expected("number", other)),
        }
    }
}

impl ToLuaValue for f64 {
    fn to_lua_value(&self, _lua: &Lua) -> mlua::Result<Value> {
        finite(*self)
            .map(Value::Number)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    }
}

impl FromLuaValue for f32 {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let n = f64::from_lua_value(value)?;
        let narrowed = n as f32;
        if narrowed.is_finite() {
            Ok(narrowed)
        } else {
            Err(ConvertError::new(format!("expected f32, got {n}")))
        }
    }
}

impl ToLuaValue for f32 {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        f64::from(*self).to_lua_value(lua)
    }
}

impl FromLuaValue for String {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        match value {
            Value::String(s) => std::str::from_utf8(&s.as_bytes())
                .map(str::to_owned)
                .map_err(|_| ConvertError::new("expected UTF-8 string, got invalid bytes")),
            other => Err(ConvertError::expected("string", other)),
        }
    }
}

impl ToLuaValue for String {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::String(lua.create_string(self)?))
    }
}

/// `nil` is `None`; any other value is read as `T`.
impl<T: FromLuaValue> FromLuaValue for Option<T> {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        match value {
            Value::Nil => Ok(None),
            present => T::from_lua_value(present).map(Some),
        }
    }
}

impl<T: ToLuaValue> ToLuaValue for Option<T> {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            Some(value) => value.to_lua_value(lua),
            None => Ok(Value::Nil),
        }
    }
}

/// A Duration is a number of seconds: finite, from 0 to 1e9.
impl FromLuaValue for Duration {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let secs = match value {
            Value::Number(n) => *n,
            Value::Integer(i) => number_from_integer(*i)?,
            other => return Err(ConvertError::expected("number of seconds", other)),
        };
        if !secs.is_finite() || !(0.0..=MAX_DURATION_SECS).contains(&secs) {
            return Err(ConvertError::new(format!(
                "expected seconds from 0 to {MAX_DURATION_SECS}, got {secs}"
            )));
        }
        // `-0.0` is in range, but it is not a duration Rust accepts as is.
        let secs = if secs == 0.0 { 0.0 } else { secs };
        Ok(Duration::from_secs_f64(secs))
    }
}

impl ToLuaValue for Duration {
    fn to_lua_value(&self, _lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::Number(self.as_secs_f64()))
    }
}

/// Implements both conversions for a plain enum, as lowercase names:
/// `lua_enum!(Shade { Light => "light", Dark => "dark" });`
/// An unknown name is an error that lists the allowed ones.
#[macro_export]
macro_rules! lua_enum {
    ($ty:ident { $($variant:ident => $name:literal),+ $(,)? }) => {
        impl $crate::scalar::FromLuaValue for $ty {
            fn from_lua_value(
                value: &$crate::scalar::mlua::Value,
            ) -> ::std::result::Result<Self, $crate::scalar::ConvertError> {
                let name = <::std::string::String as $crate::scalar::FromLuaValue>::from_lua_value(value)?;
                match name.as_str() {
                    $($name => Ok($ty::$variant),)+
                    _ => Err($crate::scalar::ConvertError::unknown_variant(&name, &[$($name),+])),
                }
            }
        }

        impl $crate::scalar::ToLuaValue for $ty {
            fn to_lua_value(
                &self,
                lua: &$crate::scalar::mlua::Lua,
            ) -> $crate::scalar::mlua::Result<$crate::scalar::mlua::Value> {
                let name = match self {
                    $($ty::$variant => $name,)+
                };
                Ok($crate::scalar::mlua::Value::String(lua.create_string(name)?))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    enum Shade {
        Light,
        Dark,
    }

    lua_enum!(Shade { Light => "light", Dark => "dark" });

    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    fn text(lua: &Lua, bytes: &[u8]) -> Value {
        Value::String(lua.create_string(bytes).expect("string"))
    }

    #[test]
    fn bool_round_trips_and_rejects_numbers() {
        let lua = Lua::new();
        let v = true.to_lua_value(&lua).expect("to lua");
        assert!(matches!(v, Value::Boolean(true)));
        assert_eq!(bool::from_lua_value(&v), Ok(true));
        assert!(bool::from_lua_value(&num(1.0)).is_err());
    }

    #[test]
    fn integers_accept_whole_numbers_in_range() {
        assert_eq!(i8::from_lua_value(&num(-128.0)), Ok(-128));
        assert_eq!(u8::from_lua_value(&num(255.0)), Ok(255));
        assert_eq!(i64::from_lua_value(&Value::Integer(i64::MIN)), Ok(i64::MIN));
        assert_eq!(
            u64::from_lua_value(&num(9_007_199_254_740_992.0)),
            Ok(1 << 53)
        );
    }

    #[test]
    fn integer_rejects_fraction_and_names_the_type() {
        let err = i32::from_lua_value(&num(1.5)).expect_err("fraction");
        assert!(err.to_string().contains("expected integer"), "{err}");
    }

    #[test]
    fn integer_rejects_out_of_range() {
        assert!(i8::from_lua_value(&num(128.0)).is_err());
        assert!(u8::from_lua_value(&num(-1.0)).is_err());
        assert!(i64::from_lua_value(&num(9_223_372_036_854_775_808.0)).is_err());
        assert!(u64::from_lua_value(&num(18_446_744_073_709_551_616.0)).is_err());
    }

    #[test]
    fn integer_rejects_nan_and_infinity() {
        assert!(i64::from_lua_value(&num(f64::NAN)).is_err());
        assert!(i64::from_lua_value(&num(f64::INFINITY)).is_err());
        assert!(i64::from_lua_value(&num(f64::NEG_INFINITY)).is_err());
    }

    #[test]
    fn integer_never_reads_a_string() {
        let lua = Lua::new();
        assert!(i32::from_lua_value(&text(&lua, b"12")).is_err());
        assert!(f64::from_lua_value(&text(&lua, b"1.5")).is_err());
    }

    #[test]
    fn integer_to_lua_rejects_values_a_lua_number_cannot_hold() {
        let lua = Lua::new();
        assert!(u64::to_lua_value(&((1u64 << 53) + 1), &lua).is_err());
        let v = u64::to_lua_value(&(1u64 << 53), &lua).expect("exact");
        assert_eq!(u64::from_lua_value(&v), Ok(1 << 53));
    }

    #[test]
    fn float_accepts_finite_numbers_only() {
        assert_eq!(f64::from_lua_value(&num(0.25)), Ok(0.25));
        assert_eq!(f64::from_lua_value(&Value::Integer(3)), Ok(3.0));
        assert!(f64::from_lua_value(&num(f64::NAN)).is_err());
        assert!(f64::from_lua_value(&num(f64::INFINITY)).is_err());
        assert!(f64::from_lua_value(&Value::Integer(i64::MAX)).is_err());
        assert!(f64::to_lua_value(&f64::NAN, &Lua::new()).is_err());
    }

    #[test]
    fn f32_rejects_values_too_large_for_it() {
        assert_eq!(f32::from_lua_value(&num(0.5)), Ok(0.5));
        assert!(f32::from_lua_value(&num(1e300)).is_err());
    }

    #[test]
    fn string_round_trips_utf8() {
        let lua = Lua::new();
        let v = "héllo".to_string().to_lua_value(&lua).expect("to lua");
        assert_eq!(String::from_lua_value(&v), Ok("héllo".to_string()));
    }

    #[test]
    fn string_rejects_invalid_utf8_and_numbers() {
        let lua = Lua::new();
        assert!(String::from_lua_value(&text(&lua, &[0xff])).is_err());
        assert!(String::from_lua_value(&num(1.0)).is_err());
    }

    #[test]
    fn option_maps_nil_to_none() {
        let lua = Lua::new();
        assert_eq!(Option::<i32>::from_lua_value(&Value::Nil), Ok(None));
        assert_eq!(Option::<i32>::from_lua_value(&num(5.0)), Ok(Some(5)));
        assert!(Option::<i32>::from_lua_value(&text(&lua, b"5")).is_err());
        let v = Option::<i32>::None.to_lua_value(&lua).expect("to lua");
        assert!(matches!(v, Value::Nil));
    }

    #[test]
    fn duration_is_seconds() {
        assert_eq!(
            Duration::from_lua_value(&num(1.5)),
            Ok(Duration::from_millis(1500))
        );
        assert_eq!(Duration::from_lua_value(&num(0.0)), Ok(Duration::ZERO));
        assert_eq!(
            Duration::from_lua_value(&num(1e9)),
            Ok(Duration::from_secs(1_000_000_000))
        );
        let v = Duration::from_millis(2250)
            .to_lua_value(&Lua::new())
            .expect("to lua");
        assert!(matches!(v, Value::Number(n) if n == 2.25));
    }

    #[test]
    fn duration_rejects_negative_huge_and_non_finite() {
        assert!(Duration::from_lua_value(&num(-1.0)).is_err());
        assert!(Duration::from_lua_value(&num(1e10)).is_err());
        assert!(Duration::from_lua_value(&num(f64::NAN)).is_err());
        assert!(Duration::from_lua_value(&num(f64::INFINITY)).is_err());
    }

    #[test]
    fn plain_enum_round_trips_its_names() {
        let lua = Lua::new();
        assert_eq!(
            Shade::from_lua_value(&text(&lua, b"light")),
            Ok(Shade::Light)
        );
        let v = Shade::Dark.to_lua_value(&lua).expect("to lua");
        assert_eq!(String::from_lua_value(&v), Ok("dark".to_string()));
    }

    #[test]
    fn plain_enum_rejects_unknown_names_and_lists_the_allowed_ones() {
        let lua = Lua::new();
        let err = Shade::from_lua_value(&text(&lua, b"dusk")).expect_err("unknown");
        let message = err.to_string();
        assert!(
            message.contains("\"light\"") && message.contains("\"dark\""),
            "{message}"
        );
        assert!(Shade::from_lua_value(&num(1.0)).is_err());
    }
}
