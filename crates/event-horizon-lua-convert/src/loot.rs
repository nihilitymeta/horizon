//! The loot context's leaf values in Lua, read-only: an item key is its id string, and an item
//! stack is its id, count and component patch (the same table as [`ItemStackSpec`], with the
//! damage in the patch as the host writes it). Both read the process-global tables, since the
//! context's conversion has no host to ask; a key or a stack the tables do not name is an error.

use mlua::{Lua, Value};
use mod_api::world::snapshots::LootStack;
use mod_api::{Compound, ItemKey, ItemStack, Tag, Text};

use crate::item::ItemStackSpec;
use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};

impl ToLuaValue for ItemKey {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let id = mod_api::__private::item_key_id(*self)
            .ok_or_else(|| mlua::Error::runtime("an item key with no id"))?;
        Ok(Value::String(lua.create_string(id)?))
    }
}

impl ToLuaValue for ItemStack {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let id = mod_api::__private::item_key_id(self.item())
            .ok_or_else(|| mlua::Error::runtime("an item stack with no id"))?;
        let mut patch: Compound = self
            .components()
            .map(|patch| patch.as_nbt().clone())
            .unwrap_or_default();
        if let Some(damage) = self.damage_component() {
            let tag = i32::try_from(damage).map_or(Tag::Long(i64::from(damage)), Tag::Int);
            patch.insert("minecraft:damage".into(), tag);
        }
        let spec = ItemStackSpec {
            id: id.to_owned(),
            count: self.count(),
            nbt: (!patch.is_empty()).then_some(patch),
        };
        spec.to_lua_value(lua)
    }
}

/// The most entries one list of a loot stack may hold when it comes from Lua.
const MAX_LIST: usize = 256;

/// The most characters one name string of a loot stack may hold when it comes from Lua.
const MAX_NAME: usize = 64;

/// The keys a loot stack table may hold. Any other key is refused, so a typo does not do nothing.
const FIELDS: [&str; 11] = [
    "item",
    "count",
    "damage",
    "enchantments",
    "potion",
    "map",
    "exploration",
    "ominous_amplifier",
    "stew_effects",
    "names",
    "components",
];

/// A loot stack written by a handler, read back into the engine's candidate.
///
/// `item`, `count` are required; the rest default as the engine does (`damage` 0, lists empty,
/// `potion` and `ominous_amplifier` absent, `map` false). `exploration` is refused: a handler
/// cannot ask for a structure search. Every list holds at most [`MAX_LIST`] entries and every
/// name at most [`MAX_NAME`] characters. The engine validates the result again when it is
/// written back, so this only has to produce a well-formed stack or say what is wrong.
pub fn loot_stack_from_lua(value: &Value) -> Result<LootStack, String> {
    let Value::Table(table) = value else {
        return Err(format!(
            "expected a loot stack table, got {}",
            value.type_name()
        ));
    };
    for pair in table.pairs::<Value, Value>() {
        let (key, _) = pair.map_err(|err| format!("cannot read the loot stack: {err}"))?;
        let key = String::from_lua_value(&key)
            .map_err(|_| "a loot stack key is not a name".to_owned())?;
        if !FIELDS.contains(&key.as_str()) {
            return Err(format!("unknown loot stack key {key:?}"));
        }
    }
    let field = |key: &str| -> Result<Value, String> {
        table
            .raw_get::<Value>(key)
            .map_err(|err| format!("cannot read {key}: {err}"))
    };

    let id = name_field(&field("item")?, "item")?;
    let item =
        mod_api::__private::item_key(&id).ok_or_else(|| format!("unknown item id {id:?}"))?;
    let count = i32_field(&field("count")?, "count")?
        .ok_or_else(|| "a loot stack needs a count".to_owned())?;
    let mut stack = LootStack::new(item, count);
    stack.damage = i32_field(&field("damage")?, "damage")?.unwrap_or(0);
    stack.enchantments = list(&field("enchantments")?, "enchantments", |value| {
        let (name, level) = pair(value, "an enchantment")?;
        Ok((
            name_field(&name, "an enchantment")?,
            i32_field(&level, "an enchantment level")?.ok_or("an enchantment needs a level")?,
        ))
    })?;
    stack.potion = match field("potion")? {
        Value::Nil => None,
        value => Some(name_field(&value, "potion")?),
    };
    stack.map = match field("map")? {
        Value::Nil => false,
        value => bool::from_lua_value(&value).map_err(|err| format!("map: {err}"))?,
    };
    if !field("exploration")?.is_nil() {
        return Err("exploration maps are read-only".to_owned());
    }
    stack.ominous_amplifier = i32_field(&field("ominous_amplifier")?, "ominous_amplifier")?;
    stack.stew_effects = list(&field("stew_effects")?, "stew_effects", |value| {
        let (name, ticks) = pair(value, "a stew effect")?;
        Ok((
            name_field(&name, "a stew effect")?,
            i32_field(&ticks, "a stew effect duration")?.ok_or("a stew effect needs a duration")?,
        ))
    })?;
    stack.names = list(&field("names")?, "names", |value| {
        let (name, text) = pair(value, "a name")?;
        let text = Text::from_lua_value(&text).map_err(|err| format!("name text: {err}"))?;
        Ok((name_field(&name, "a name")?, text))
    })?;
    stack.components = list(&field("components")?, "components", |value| {
        let (name, tag) = pair(value, "a component")?;
        let name = name_field(&name, "a component")?;
        match tag {
            Value::Nil | Value::Boolean(false) => Ok((name, None)),
            tag => {
                let label = name.clone();
                Tag::from_lua_value(&tag)
                    .map(|tag| (name, Some(tag)))
                    .map_err(|err| format!("component {label:?}: {err}"))
            }
        }
    })?;
    Ok(stack)
}

impl FromLuaValue for LootStack {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        loot_stack_from_lua(value).map_err(ConvertError::new)
    }
}

/// A name string of at most [`MAX_NAME`] characters.
fn name_field(value: &Value, what: &str) -> Result<String, String> {
    let name = String::from_lua_value(value).map_err(|err| format!("{what}: {err}"))?;
    if name.chars().count() > MAX_NAME {
        return Err(format!("{what} is longer than {MAX_NAME} characters"));
    }
    Ok(name)
}

/// An integer field, or `None` when it is absent.
fn i32_field(value: &Value, what: &str) -> Result<Option<i32>, String> {
    Option::<i32>::from_lua_value(value).map_err(|err| format!("{what}: {err}"))
}

/// A two-item array `{name, value}`. The value may be nil; a third item is refused.
fn pair(value: &Value, what: &str) -> Result<(Value, Value), String> {
    let Value::Table(table) = value else {
        return Err(format!("{what} must be a table, got {}", value.type_name()));
    };
    for entry in table.pairs::<Value, Value>() {
        let (key, _) = entry.map_err(|err| format!("cannot read {what}: {err}"))?;
        if !matches!(key, Value::Integer(1 | 2)) {
            return Err(format!("{what} has an item past the second"));
        }
    }
    let first: Value = table
        .raw_get(1)
        .map_err(|err| format!("cannot read {what}: {err}"))?;
    let second: Value = table
        .raw_get(2)
        .map_err(|err| format!("cannot read {what}: {err}"))?;
    if first.is_nil() {
        return Err(format!("{what} has no name"));
    }
    Ok((first, second))
}

/// A Lua array of at most [`MAX_LIST`] items, each read by `each`. Like [`Vec`]'s conversion it
/// refuses holes and non-index keys; nil is the empty list.
fn list<T>(
    value: &Value,
    what: &str,
    each: impl Fn(&Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let table = match value {
        Value::Nil => return Ok(Vec::new()),
        Value::Table(table) => table,
        other => return Err(format!("{what} must be a list, got {}", other.type_name())),
    };
    let mut entries: Vec<(i64, Value)> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, item) = pair.map_err(|err| format!("cannot read {what}: {err}"))?;
        let index = match key {
            Value::Integer(i) => i,
            Value::Number(n) if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 => n as i64,
            _ => return Err(format!("{what} has a key that is not an index")),
        };
        if entries.len() == MAX_LIST {
            return Err(format!("{what} is over the cap of {MAX_LIST} items"));
        }
        entries.push((index, item));
    }
    entries.sort_unstable_by_key(|(index, _)| *index);
    entries
        .iter()
        .enumerate()
        .map(|(position, (index, item))| {
            if *index != position as i64 + 1 {
                return Err(format!("{what} has a hole at item {}", position + 1));
            }
            each(item).map_err(|err| format!("{what} item {}: {err}", position + 1))
        })
        .collect()
}
