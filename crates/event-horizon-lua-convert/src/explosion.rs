//! Explosion settings and game rule values between Lua tables and Rust, read strictly.
//!
//! `ExplosionSettings` is a table whose keys are the six field names, every one optional:
//! `{damage_type, calculator, small_particle, large_particle, block_particles, sound}`. An
//! absent or nil key is `None`; any other key is refused by name. A calculator is
//! `{explodes_blocks, damages_entities, knockback_multiplier?, immune_blocks?}` and a block
//! particle is `{particle, scaling, speed, weight}`: the exact shapes the generated
//! `ToLuaValue` writes. A game rule value is `{tag = "bool", true}` or `{tag = "int", 7}`, or,
//! as a convenience, a bare boolean or a bare whole number.
//!
//! Limits: an identifier is 1 to [`MAX_IDENTIFIER`] characters with no control characters, and
//! a blast has at most [`MAX_BLOCK_PARTICLES`] block particles. The engine checks the ids
//! (registered damage types, sounds, particles, tags) and the weights and scales again at the
//! write boundary; this module only produces a well-formed value or says what is wrong.

use mlua::{Table, Value};
use mod_api::{ExplosionBlockParticle, ExplosionCalculator, ExplosionSettings, GameRuleValue};

use crate::scalar::{ConvertError, FromLuaValue};

/// The longest identifier, in characters.
pub const MAX_IDENTIFIER: usize = 128;
/// The most block particles one blast may name.
pub const MAX_BLOCK_PARTICLES: usize = 64;

const SETTINGS_FIELDS: &[&str] = &[
    "damage_type",
    "calculator",
    "small_particle",
    "large_particle",
    "block_particles",
    "sound",
];
const CALCULATOR_FIELDS: &[&str] = &[
    "explodes_blocks",
    "damages_entities",
    "knockback_multiplier",
    "immune_blocks",
];
const PARTICLE_FIELDS: &[&str] = &["particle", "scaling", "speed", "weight"];

impl FromLuaValue for ExplosionSettings {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let table = table_of(value, "explosion settings")?;
        check_keys(table, SETTINGS_FIELDS, "explosion settings")?;
        Ok(Self {
            damage_type: optional(table, "damage_type", identifier)?,
            calculator: optional(table, "calculator", ExplosionCalculator::from_lua_value)?,
            small_particle: optional(table, "small_particle", identifier)?,
            large_particle: optional(table, "large_particle", identifier)?,
            block_particles: optional(table, "block_particles", block_particles)?,
            sound: optional(table, "sound", identifier)?,
        })
    }
}

impl FromLuaValue for ExplosionCalculator {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let table = table_of(value, "explosion calculator")?;
        check_keys(table, CALCULATOR_FIELDS, "explosion calculator")?;
        Ok(Self {
            explodes_blocks: required(table, "explodes_blocks", bool::from_lua_value)?,
            damages_entities: required(table, "damages_entities", bool::from_lua_value)?,
            knockback_multiplier: optional(table, "knockback_multiplier", f32::from_lua_value)?,
            immune_blocks: optional(table, "immune_blocks", identifier)?,
        })
    }
}

impl FromLuaValue for ExplosionBlockParticle {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let table = table_of(value, "block particle")?;
        check_keys(table, PARTICLE_FIELDS, "block particle")?;
        Ok(Self {
            particle: required(table, "particle", identifier)?,
            scaling: required(table, "scaling", f32::from_lua_value)?,
            speed: required(table, "speed", f32::from_lua_value)?,
            weight: required(table, "weight", u32::from_lua_value)?,
        })
    }
}

impl FromLuaValue for GameRuleValue {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        match value {
            Value::Boolean(flag) => Ok(Self::Bool(*flag)),
            Value::Integer(_) | Value::Number(_) => game_rule_int(value).map(Self::Int),
            Value::Table(table) => tagged_game_rule(table),
            other => Err(ConvertError::new(format!(
                "game rule value: expected a boolean, an integer or a tagged table, got {}",
                other.type_name()
            ))),
        }
    }
}

/// The table a value is read from, or an error naming what was expected.
fn table_of<'a>(value: &'a Value, expected: &str) -> Result<&'a Table, ConvertError> {
    match value {
        Value::Table(table) => Ok(table),
        other => Err(ConvertError::expected(expected, other)),
    }
}

/// Refuses any key that is not one of `allowed`, naming it. Keys are read raw.
fn check_keys(table: &Table, allowed: &[&str], what: &str) -> Result<(), ConvertError> {
    for pair in table.pairs::<Value, Value>() {
        let (key, _) =
            pair.map_err(|err| ConvertError::new(format!("cannot read {what}: {err}")))?;
        let name = String::from_lua_value(&key)
            .map_err(|_| ConvertError::new(format!("{what} has a key that is not a name")))?;
        if !allowed.contains(&name.as_str()) {
            return Err(ConvertError::new(format!(
                "{what}: unknown key {:?}",
                ConvertError::clip(&name)
            )));
        }
    }
    Ok(())
}

/// The field `key` read raw, or `None` when it is absent or nil.
fn optional<T>(
    table: &Table,
    key: &str,
    read: impl Fn(&Value) -> Result<T, ConvertError>,
) -> Result<Option<T>, ConvertError> {
    let value = raw_field(table, key)?;
    if value.is_nil() {
        return Ok(None);
    }
    read(&value).map(Some).map_err(|err| named(key, err))
}

/// The field `key` read raw; absent or nil is an error naming the key.
fn required<T>(
    table: &Table,
    key: &str,
    read: impl Fn(&Value) -> Result<T, ConvertError>,
) -> Result<T, ConvertError> {
    let value = raw_field(table, key)?;
    if value.is_nil() {
        return Err(ConvertError::new(format!("{key} is required")));
    }
    read(&value).map_err(|err| named(key, err))
}

fn raw_field(table: &Table, key: &str) -> Result<Value, ConvertError> {
    table
        .raw_get::<Value>(key)
        .map_err(|_| ConvertError::new(format!("cannot read {key}")))
}

/// Prefixes an error with the key it came from.
fn named(key: &str, error: ConvertError) -> ConvertError {
    ConvertError::new(format!("{key}: {error}"))
}

/// An identifier string: 1 to [`MAX_IDENTIFIER`] characters, no control characters. The
/// message does not echo the input.
fn identifier(value: &Value) -> Result<String, ConvertError> {
    let text = String::from_lua_value(value)?;
    if text.is_empty() {
        return Err(ConvertError::new("must not be empty"));
    }
    if text.chars().count() > MAX_IDENTIFIER {
        return Err(ConvertError::new(format!(
            "longer than {MAX_IDENTIFIER} characters"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(ConvertError::new("contains a control character"));
    }
    Ok(text)
}

/// A sequence of at most [`MAX_BLOCK_PARTICLES`] block particles. Holes and non-index keys are
/// refused. An empty table is an empty list, which scatters no particles.
fn block_particles(value: &Value) -> Result<Vec<ExplosionBlockParticle>, ConvertError> {
    let table = table_of(value, "a list of block particles")?;
    let mut entries: Vec<(i64, Value)> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, item) =
            pair.map_err(|err| ConvertError::new(format!("cannot read the list: {err}")))?;
        let index = match key {
            Value::Integer(i) => i,
            Value::Number(n) if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 => n as i64,
            _ => {
                return Err(ConvertError::new(
                    "block_particles has a key that is not an index",
                ));
            }
        };
        if entries.len() == MAX_BLOCK_PARTICLES {
            return Err(ConvertError::new(format!(
                "block_particles is over the cap of {MAX_BLOCK_PARTICLES} entries"
            )));
        }
        entries.push((index, item));
    }
    entries.sort_unstable_by_key(|(index, _)| *index);
    entries
        .iter()
        .enumerate()
        .map(|(position, (index, item))| {
            if *index != position as i64 + 1 {
                return Err(ConvertError::new(format!(
                    "block_particles has a hole at item {}",
                    position + 1
                )));
            }
            ExplosionBlockParticle::from_lua_value(item).map_err(|err| {
                ConvertError::new(format!("block_particles item {}: {err}", position + 1))
            })
        })
        .collect()
}

/// A game rule integer: an `i32`, whole and in range.
fn game_rule_int(value: &Value) -> Result<i32, ConvertError> {
    i32::from_lua_value(value).map_err(|err| ConvertError::new(format!("game rule integer: {err}")))
}

/// `{tag = "bool", value}` or `{tag = "int", value}`: only the keys `tag` and `1` are allowed.
fn tagged_game_rule(table: &Table) -> Result<GameRuleValue, ConvertError> {
    for pair in table.pairs::<Value, Value>() {
        let (key, _) =
            pair.map_err(|err| ConvertError::new(format!("cannot read the game rule: {err}")))?;
        match &key {
            Value::Integer(1) => {}
            Value::Number(n) if *n == 1.0 => {}
            Value::String(_) => {
                let name = String::from_lua_value(&key)?;
                if name != "tag" {
                    return Err(ConvertError::new(format!(
                        "game rule value: unknown key {:?}",
                        ConvertError::clip(&name)
                    )));
                }
            }
            _ => {
                return Err(ConvertError::new(
                    "game rule value has a key other than tag and 1",
                ));
            }
        }
    }
    let tag = match raw_field(table, "tag")? {
        Value::Nil => return Err(ConvertError::new("game rule value needs a tag")),
        value => String::from_lua_value(&value).map_err(|err| named("tag", err))?,
    };
    let payload: Value = table
        .raw_get(1)
        .map_err(|_| ConvertError::new("cannot read the game rule value"))?;
    match tag.as_str() {
        "bool" => match payload {
            Value::Boolean(flag) => Ok(GameRuleValue::Bool(flag)),
            other => Err(ConvertError::new(format!(
                "bool game rule: expected boolean, got {}",
                other.type_name()
            ))),
        },
        "int" => game_rule_int(&payload).map(GameRuleValue::Int),
        other => Err(named(
            "tag",
            ConvertError::unknown_variant(other, &["bool", "int"]),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scalar::ToLuaValue;

    fn eval(lua: &mlua::Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk runs")
    }

    fn settings(lua: &mlua::Lua, source: &str) -> Result<ExplosionSettings, ConvertError> {
        ExplosionSettings::from_lua_value(&eval(lua, source))
    }

    fn rule(lua: &mlua::Lua, source: &str) -> Result<GameRuleValue, ConvertError> {
        GameRuleValue::from_lua_value(&eval(lua, source))
    }

    #[test]
    fn settings_with_every_field_read_back() {
        let lua = mlua::Lua::new();
        let read = settings(
            &lua,
            r#"return {
                damage_type = "mymod:fuel_tank",
                calculator = {explodes_blocks = false, damages_entities = true,
                    knockback_multiplier = 0.5, immune_blocks = "minecraft:logs"},
                small_particle = "minecraft:explosion",
                large_particle = "minecraft:flash",
                block_particles = {{particle = "minecraft:flame", scaling = 0.5, speed = 1.5, weight = 3}},
                sound = "mymod:fuel_tank_burst",
            }"#,
        )
        .expect("valid settings");
        let expected = ExplosionSettings {
            damage_type: Some("mymod:fuel_tank".into()),
            calculator: Some(ExplosionCalculator {
                explodes_blocks: false,
                damages_entities: true,
                knockback_multiplier: Some(0.5),
                immune_blocks: Some("minecraft:logs".into()),
            }),
            small_particle: Some("minecraft:explosion".into()),
            large_particle: Some("minecraft:flash".into()),
            block_particles: Some(vec![ExplosionBlockParticle {
                particle: "minecraft:flame".into(),
                scaling: 0.5,
                speed: 1.5,
                weight: 3,
            }]),
            sound: Some("mymod:fuel_tank_burst".into()),
        };
        assert_eq!(read, expected);
    }

    #[test]
    fn calculator_round_trips_through_its_lua_form() {
        let lua = mlua::Lua::new();
        for calculator in [
            ExplosionCalculator::default(),
            ExplosionCalculator {
                explodes_blocks: false,
                damages_entities: false,
                knockback_multiplier: Some(0.25),
                immune_blocks: Some("minecraft:logs".into()),
            },
        ] {
            let value = calculator.to_lua_value(&lua).expect("to lua");
            assert_eq!(ExplosionCalculator::from_lua_value(&value), Ok(calculator));
        }
    }

    #[test]
    fn block_particle_round_trips_through_its_lua_form() {
        let lua = mlua::Lua::new();
        let particle = ExplosionBlockParticle {
            particle: "minecraft:flame".into(),
            scaling: 0.5,
            speed: 1.5,
            weight: 3,
        };
        let value = particle.to_lua_value(&lua).expect("to lua");
        assert_eq!(ExplosionBlockParticle::from_lua_value(&value), Ok(particle));
    }

    #[test]
    fn empty_table_is_all_none() {
        let lua = mlua::Lua::new();
        assert_eq!(
            settings(&lua, "return {}").expect("valid"),
            ExplosionSettings::default()
        );
        let nils = settings(&lua, "return {damage_type = nil, sound = nil}").expect("valid");
        assert_eq!(nils, ExplosionSettings::default());
    }

    #[test]
    fn unknown_key_is_refused_by_name() {
        let lua = mlua::Lua::new();
        let err = settings(&lua, r#"return {damage_typ = "x"}"#).expect_err("unknown key");
        assert!(err.to_string().contains("damage_typ"), "{err}");
    }

    #[test]
    fn identifiers_are_bounded_and_clean() {
        let lua = mlua::Lua::new();
        let long = settings(&lua, r#"return {sound = string.rep("a", 129)}"#).expect_err("long");
        assert!(long.to_string().starts_with("sound:"), "{long}");
        let exact = settings(&lua, r#"return {sound = string.rep("a", 128)}"#).expect("128 ok");
        assert_eq!(exact.sound.map(|s| s.len()), Some(128));
        let control = settings(&lua, r#"return {small_particle = "bad\n"}"#).expect_err("control");
        assert!(
            control.to_string().starts_with("small_particle:"),
            "{control}"
        );
        let empty = settings(&lua, r#"return {large_particle = ""}"#).expect_err("empty");
        assert!(empty.to_string().starts_with("large_particle:"), "{empty}");
    }

    #[test]
    fn block_particles_are_capped_at_64() {
        let lua = mlua::Lua::new();
        let fill = |count: usize| {
            format!(
                r#"local t = {{}}
                for i = 1, {count} do t[i] = {{particle = "minecraft:flame", scaling = 1, speed = 1, weight = 1}} end
                return {{block_particles = t}}"#
            )
        };
        let full = settings(&lua, &fill(64)).expect("64 entries are allowed");
        assert_eq!(full.block_particles.map(|list| list.len()), Some(64));
        let err = settings(&lua, &fill(65)).expect_err("65 entries");
        assert!(err.to_string().contains("64"), "{err}");
        let holes = settings(
            &lua,
            r#"return {block_particles = {[2] = {particle = "minecraft:flame", scaling = 1, speed = 1, weight = 1}}}"#,
        )
        .expect_err("hole");
        assert!(holes.to_string().contains("hole"), "{holes}");
    }

    #[test]
    fn game_rule_values_parse_in_every_shape() {
        let lua = mlua::Lua::new();
        assert_eq!(
            rule(&lua, r#"return {tag = "bool", true}"#),
            Ok(GameRuleValue::Bool(true))
        );
        assert_eq!(
            rule(&lua, r#"return {tag = "int", 7}"#),
            Ok(GameRuleValue::Int(7))
        );
        assert_eq!(rule(&lua, "return false"), Ok(GameRuleValue::Bool(false)));
        assert_eq!(rule(&lua, "return 7"), Ok(GameRuleValue::Int(7)));
        let written = GameRuleValue::Int(-3).to_lua_value(&lua).expect("to lua");
        assert_eq!(
            GameRuleValue::from_lua_value(&written),
            Ok(GameRuleValue::Int(-3))
        );
    }

    #[test]
    fn game_rule_values_refuse_out_of_range_fraction_and_unknown_tag() {
        let lua = mlua::Lua::new();
        let big = rule(&lua, r#"return {tag = "int", 2147483648}"#).expect_err("too big");
        assert!(big.to_string().contains("i32"), "{big}");
        let fraction = rule(&lua, "return 1.5").expect_err("fraction");
        assert!(fraction.to_string().contains("integer"), "{fraction}");
        let bogus = rule(&lua, r#"return {tag = "bogus", 1}"#).expect_err("bogus tag");
        assert!(bogus.to_string().contains("bogus"), "{bogus}");
    }
}
