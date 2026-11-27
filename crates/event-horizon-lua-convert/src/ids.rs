//! Identifier-like values between Lua and Rust: UUIDs (`u128`), `usize`, and mod
//! identifiers (`mod_api::Identifier`), and a block entity type key as its id string.
//!
//! A player or entity id is a 128-bit UUID, which a Lua number cannot hold, so it
//! travels as the canonical lowercase hyphenated string
//! `"123e4567-e89b-12d3-a456-426614174000"`. Input accepts either case; output is
//! always lowercase. A `usize` follows the rules of `u64`. An identifier is the string
//! `"namespace:path"`: a missing namespace is `minecraft` and output is always the full
//! form, as [`item::canonical_id`] states.
//!
//! The values come from mods, so every conversion is strict and nothing panics.

use mlua::{Lua, Value};
use mod_api::Identifier;

use crate::item;
use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};

/// Where the four hyphens of a UUID string sit, by byte index.
const HYPHENS: [usize; 4] = [8, 13, 18, 23];
/// The length of a UUID string: 32 hex digits and four hyphens.
const UUID_LEN: usize = 36;

/// The canonical lowercase UUID string for a 128-bit value (its big-endian bytes).
pub fn uuid_string(v: u128) -> String {
    let h = format!("{v:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The 128-bit value of a UUID string in `8-4-4-4-12` hex form, either case.
pub fn parse_uuid(s: &str) -> Result<u128, ConvertError> {
    parse_uuid_bytes(s.as_bytes())
}

fn parse_uuid_bytes(bytes: &[u8]) -> Result<u128, ConvertError> {
    let refuse = || ConvertError::new("expected uuid, got a string not in 8-4-4-4-12 hex form");
    if bytes.len() != UUID_LEN {
        return Err(refuse());
    }
    let mut value: u128 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if HYPHENS.contains(&i) {
            if b != b'-' {
                return Err(refuse());
            }
            continue;
        }
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(refuse()),
        };
        value = (value << 4) | u128::from(digit);
    }
    Ok(value)
}

impl FromLuaValue for u128 {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        match value {
            Value::String(s) => parse_uuid_bytes(&s.as_bytes()),
            other => Err(ConvertError::expected("uuid", other)),
        }
    }
}

impl ToLuaValue for u128 {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::String(lua.create_string(uuid_string(*self))?))
    }
}

impl FromLuaValue for usize {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let wide = u64::from_lua_value(value)?;
        usize::try_from(wide).map_err(|_| ConvertError::new(format!("expected usize, got {wide}")))
    }
}

impl ToLuaValue for usize {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let wide = u64::try_from(*self)
            .map_err(|_| mlua::Error::runtime(format!("usize {self} does not fit a u64")))?;
        wide.to_lua_value(lua)
    }
}

/// A block entity type key is the canonical id of its type (`"mymod:press"`), the string the mod
/// wrote. A key with no name in the load-owned registry is an error, never an empty string.
impl ToLuaValue for mod_api::BlockEntityTypeKey {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let name = mod_api::__private::block_entity_type_name(*self)
            .ok_or_else(|| mlua::Error::runtime("unknown block entity type key"))?;
        Ok(Value::String(lua.create_string(name)?))
    }
}

impl FromLuaValue for Identifier {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let text = String::from_lua_value(value)?;
        let canonical = item::canonical_id(&text)?;
        Identifier::parse(&canonical).map_err(|_| ConvertError::new("not a valid identifier"))
    }
}

impl ToLuaValue for Identifier {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::String(lua.create_string(self.to_string())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIXED: &str = "123e4567-e89b-12d3-a456-426614174000";
    const MIXED_VALUE: u128 = 0x123e4567_e89b_12d3_a456_426614174000;

    fn text(lua: &Lua, s: &str) -> Value {
        Value::String(lua.create_string(s).expect("string"))
    }

    /// `s` with the byte at `i` replaced by `c` (both ASCII here).
    fn with_byte(s: &str, i: usize, c: char) -> String {
        let mut bytes = s.as_bytes().to_vec();
        bytes[i] = c as u8;
        String::from_utf8(bytes).expect("ascii")
    }

    #[test]
    fn uuid_nil_and_max_map_to_zero_and_all_ones() {
        let lua = Lua::new();
        assert_eq!(
            u128::from_lua_value(&text(&lua, "00000000-0000-0000-0000-000000000000")),
            Ok(0)
        );
        assert_eq!(
            u128::from_lua_value(&text(&lua, "ffffffff-ffff-ffff-ffff-ffffffffffff")),
            Ok(u128::MAX)
        );
        assert_eq!(uuid_string(0), "00000000-0000-0000-0000-000000000000");
        assert_eq!(
            uuid_string(u128::MAX),
            "ffffffff-ffff-ffff-ffff-ffffffffffff"
        );
    }

    #[test]
    fn uuid_round_trips_in_lowercase() {
        let lua = Lua::new();
        assert_eq!(u128::from_lua_value(&text(&lua, MIXED)), Ok(MIXED_VALUE));
        let v = MIXED_VALUE.to_lua_value(&lua).expect("to lua");
        assert_eq!(String::from_lua_value(&v), Ok(MIXED.to_string()));
        assert_eq!(uuid_string(MIXED_VALUE), MIXED);
    }

    #[test]
    fn uuid_accepts_uppercase_and_writes_lowercase() {
        let lua = Lua::new();
        let upper = MIXED.to_uppercase();
        assert_eq!(u128::from_lua_value(&text(&lua, &upper)), Ok(MIXED_VALUE));
        assert_eq!(parse_uuid(&upper), Ok(MIXED_VALUE));
        assert_eq!(uuid_string(MIXED_VALUE), MIXED);
    }

    #[test]
    fn uuid_refuses_wrong_length() {
        let lua = Lua::new();
        let short = &MIXED[..35];
        let long = format!("{MIXED}0");
        assert!(u128::from_lua_value(&text(&lua, short)).is_err());
        assert!(u128::from_lua_value(&text(&lua, &long)).is_err());
        assert!(u128::from_lua_value(&text(&lua, "")).is_err());
    }

    #[test]
    fn uuid_refuses_missing_or_misplaced_hyphens() {
        let lua = Lua::new();
        // A hyphen at 8 replaced by a hex digit: still 36 bytes, three hyphens.
        let missing = with_byte(MIXED, 8, '0');
        assert!(u128::from_lua_value(&text(&lua, &missing)).is_err());
        // The hyphen moved from 8 to 9.
        let misplaced = with_byte(&with_byte(MIXED, 8, '0'), 9, '-');
        assert!(u128::from_lua_value(&text(&lua, &misplaced)).is_err());
        let no_hyphens = MIXED.replace('-', "0");
        assert!(u128::from_lua_value(&text(&lua, &no_hyphens)).is_err());
    }

    #[test]
    fn uuid_refuses_non_hex_braces_urn_spaces_and_numbers() {
        let lua = Lua::new();
        assert!(u128::from_lua_value(&text(&lua, &with_byte(MIXED, 0, 'g'))).is_err());
        assert!(u128::from_lua_value(&text(&lua, &with_byte(MIXED, 3, ' '))).is_err());
        assert!(u128::from_lua_value(&text(&lua, &format!("{{{MIXED}}}"))).is_err());
        assert!(u128::from_lua_value(&text(&lua, &format!("urn:uuid:{MIXED}"))).is_err());
        assert!(u128::from_lua_value(&text(&lua, "12345678901234567890123456789012")).is_err());
        assert!(u128::from_lua_value(&Value::Number(1.0)).is_err());
        assert!(u128::from_lua_value(&Value::Integer(5)).is_err());
        assert!(u128::from_lua_value(&Value::Nil).is_err());
    }

    #[test]
    fn usize_matches_u64_rules() {
        let lua = Lua::new();
        assert_eq!(usize::from_lua_value(&Value::Integer(0)), Ok(0));
        assert_eq!(usize::from_lua_value(&Value::Integer(1)), Ok(1));
        assert_eq!(
            usize::from_lua_value(&Value::Number(9_007_199_254_740_992.0)),
            Ok(1 << 53)
        );
        // Lua integers are exact, so 2^53 + 1 survives as an integer.
        assert_eq!(
            usize::from_lua_value(&Value::Integer((1 << 53) + 1)),
            Ok((1 << 53) + 1)
        );
        assert_eq!(
            usize::from_lua_value(&Value::Integer(i64::MAX)),
            Ok(i64::MAX as usize)
        );
        assert!(usize::from_lua_value(&Value::Integer(-1)).is_err());
        assert!(usize::from_lua_value(&Value::Number(1.5)).is_err());
        assert!(usize::from_lua_value(&Value::Number(18_446_744_073_709_551_616.0)).is_err());
        assert!(usize::from_lua_value(&text(&lua, "1")).is_err());
    }

    #[test]
    fn usize_to_lua_refuses_values_a_lua_number_cannot_hold() {
        let lua = Lua::new();
        let v = usize::to_lua_value(&(1usize << 53), &lua).expect("exact");
        assert_eq!(usize::from_lua_value(&v), Ok(1 << 53));
        assert!(usize::to_lua_value(&((1usize << 53) + 1), &lua).is_err());
        assert!(usize::to_lua_value(&usize::MAX, &lua).is_err());
    }

    #[test]
    fn identifier_gets_the_namespace_and_writes_the_full_form() {
        let lua = Lua::new();
        let stone = Identifier::from_lua_value(&text(&lua, "minecraft:stone")).expect("full");
        assert_eq!(stone.to_string(), "minecraft:stone");
        let bare = Identifier::from_lua_value(&text(&lua, "stone")).expect("bare");
        assert_eq!(bare.to_string(), "minecraft:stone");
        let path = Identifier::from_lua_value(&text(&lua, "mymod:a/b.c")).expect("path");
        assert_eq!(path.namespace(), "mymod");
        assert_eq!(path.path(), "a/b.c");
        let v = path.to_lua_value(&lua).expect("to lua");
        assert_eq!(String::from_lua_value(&v), Ok("mymod:a/b.c".to_string()));
    }

    #[test]
    fn identifier_refuses_what_the_parser_refuses_and_the_length_cap() {
        let lua = Lua::new();
        for bad in [
            "",
            "mymod:",
            "Stone",
            "minecraft:Stone",
            "a b",
            "a:b:c",
            "a/b:c",
        ] {
            assert!(
                Identifier::from_lua_value(&text(&lua, bad)).is_err(),
                "accepted {bad:?}"
            );
        }
        let at_cap = format!("mymod:{}", "a".repeat(256 - "mymod:".len()));
        assert_eq!(at_cap.len(), 256);
        assert!(Identifier::from_lua_value(&text(&lua, &at_cap)).is_ok());
        let over = format!("{at_cap}a");
        assert!(Identifier::from_lua_value(&text(&lua, &over)).is_err());
        assert!(Identifier::from_lua_value(&Value::Number(1.0)).is_err());
    }
}
