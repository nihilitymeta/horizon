//! NBT compounds between Lua tables and Rust: a [`mod_api::Compound`], or any single [`mod_api::Tag`].
//!
//! A compound is a Lua table with string keys, a list is a sequence table (keys 1 to n), and an
//! empty table is an empty compound. Plain Lua values infer their tag: a boolean is a byte (0 or
//! 1), an integer that fits i32 is an int, any other integer is a long, a non-integral number is a
//! double and a string is a string. The tags Lua cannot infer are tagged tables
//! `{__nbt = "byte", value = n}`, made by `nbt.byte`, `nbt.short`, `nbt.long`, `nbt.float` and the
//! arrays `nbt.byte_array`, `nbt.int_array` and `nbt.long_array` (see [`install_constructors`]).
//! Output writes the same tagged tables for those tags, so a byte comes back as a byte. An integral
//! double is written as `{__nbt = "double", value = n}` (a plain number would read back as an int),
//! and an empty list as `{__nbt = "empty_list", value = {}}` (a plain empty table reads as a compound).
//!
//! The data comes from mods and ends up in saved worlds, so reading is bounded (see the `MAX_`
//! constants) and strict: a mixed list, a table that mixes string and sequence keys, or a
//! non-finite number is an error. Table fields are read raw, so no metamethod runs. Output is
//! bounded by the same limits, and refuses what Lua cannot hold.

use mlua::{Lua, Table, Value};
use mod_api::{Compound, Tag};

use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};

/// The deepest nesting of lists and compounds, the root compound counting as 1.
pub const MAX_DEPTH: usize = 16;
/// The most tags one value may hold, the root included. Array elements are not tags.
pub const MAX_ENTRIES: usize = 4096;
/// The most bytes one string or compound key may hold.
pub const MAX_STRING: usize = 32767;
/// The most elements one array may hold.
pub const MAX_ARRAY: usize = 65536;

/// The key that marks a tagged value. A compound may not hold it.
const TAG_KEY: &str = "__nbt";
const VALUE_KEY: &str = "value";
/// Tag names by id, for messages.
const NAMES: [&str; 13] = [
    "end",
    "byte",
    "short",
    "int",
    "long",
    "float",
    "double",
    "byte_array",
    "string",
    "list",
    "compound",
    "int_array",
    "long_array",
];
/// 2^63: Lua numbers at or past it are not an i64.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

/// Counts the tags of one value against [`MAX_ENTRIES`].
#[derive(Default)]
struct Budget {
    tags: usize,
}

impl Budget {
    fn tag(&mut self) -> Result<(), ConvertError> {
        self.tags += 1;
        if self.tags > MAX_ENTRIES {
            return Err(ConvertError::new(format!(
                "NBT value has more than {MAX_ENTRIES} tags"
            )));
        }
        Ok(())
    }
}

fn limit(error: ConvertError) -> mlua::Error {
    mlua::Error::runtime(error.to_string())
}

fn kind(tag: &Tag) -> &'static str {
    NAMES[usize::from(tag.id())]
}

/// The level of a list or compound directly inside a container at level `parent`.
fn level_under(parent: usize) -> Result<usize, ConvertError> {
    let level = parent + 1;
    if level > MAX_DEPTH {
        return Err(ConvertError::new(format!(
            "NBT nests deeper than {MAX_DEPTH} levels"
        )));
    }
    Ok(level)
}

/// The raw entries of a table, at most `cap` of them. No metamethod runs.
fn raw_entries(table: &Table, cap: usize, what: &str) -> Result<Vec<(Value, Value)>, ConvertError> {
    let mut entries = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let pair = pair.map_err(|_| ConvertError::new(format!("cannot read {what}")))?;
        if entries.len() == cap {
            return Err(ConvertError::new(format!(
                "{what} has more than {cap} items"
            )));
        }
        entries.push(pair);
    }
    Ok(entries)
}

/// One field of a table, read raw.
fn raw_field(table: &Table, key: &str) -> Result<Value, ConvertError> {
    table
        .raw_get::<Value>(key)
        .map_err(|_| ConvertError::new(format!("cannot read {key}")))
}

/// The values of a sequence (keys 1 to n, in order), or an error if the keys are anything else.
fn sequence(entries: Vec<(Value, Value)>) -> Result<Vec<Value>, ConvertError> {
    let len = entries.len();
    let mut slots: Vec<Option<Value>> = vec![None; len];
    for (key, value) in entries {
        let index = i64::from_lua_value(&key)
            .ok()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| (1..=len).contains(i))
            .ok_or_else(|| {
                ConvertError::new("keys must be strings (a compound) or 1 to n (a list)")
            })?;
        slots[index - 1] = Some(value);
    }
    slots
        .into_iter()
        .collect::<Option<Vec<Value>>>()
        .ok_or_else(|| ConvertError::new("keys must be 1 to n"))
}

/// A Lua string as a Java string: UTF-8 and at most [`MAX_STRING`] bytes.
fn read_string(value: &Value) -> Result<String, ConvertError> {
    let Value::String(s) = value else {
        return Err(ConvertError::expected("string", value));
    };
    let bytes = s.as_bytes();
    if bytes.len() > MAX_STRING {
        return Err(ConvertError::new(format!(
            "string is longer than {MAX_STRING} bytes"
        )));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ConvertError::new("expected UTF-8 string, got invalid bytes"))?;
    Ok(text.to_owned())
}

/// An integer Lua value as an int when it fits, else a long.
fn integral_tag(i: i64) -> Tag {
    match i32::try_from(i) {
        Ok(int) => Tag::Int(int),
        Err(_) => Tag::Long(i),
    }
}

/// A Lua number: finite; integral ones are an int or a long, others a double.
fn number_tag(n: f64) -> Result<Tag, ConvertError> {
    if !n.is_finite() {
        return Err(ConvertError::new(format!(
            "expected finite number, got {n}"
        )));
    }
    if n.fract() != 0.0 {
        return Ok(Tag::Double(n.to_bits()));
    }
    if !(-TWO_POW_63..TWO_POW_63).contains(&n) {
        return Err(ConvertError::new(format!(
            "integer {n} is out of range for an NBT long"
        )));
    }
    Ok(integral_tag(n as i64))
}

/// The tag of a value: a plain Lua value is inferred, a table is a compound, a list or a tagged
/// value. `parent` is the level of the container holding it (0 at the root).
fn read_value(value: &Value, parent: usize, budget: &mut Budget) -> Result<Tag, ConvertError> {
    budget.tag()?;
    match value {
        Value::Boolean(b) => Ok(Tag::Byte(i8::from(*b))),
        Value::Integer(i) => Ok(integral_tag(*i)),
        Value::Number(n) => number_tag(*n),
        Value::String(_) => read_string(value).map(|text| Tag::String(text.as_str().into())),
        Value::Table(table) => read_table(table, parent, budget),
        other => Err(ConvertError::expected("NBT value", other)),
    }
}

fn read_table(table: &Table, parent: usize, budget: &mut Budget) -> Result<Tag, ConvertError> {
    if !raw_field(table, TAG_KEY)?.is_nil() {
        return read_tagged(table);
    }
    let level = level_under(parent)?;
    let entries = raw_entries(table, MAX_ENTRIES, "table")?;
    let strings = entries
        .iter()
        .filter(|(key, _)| matches!(key, Value::String(_)))
        .count();
    if strings == entries.len() {
        let mut map = Compound::new();
        for (key, value) in entries {
            let key = read_string(&key)?;
            map.insert(key.as_str().into(), read_value(&value, level, budget)?);
        }
        return Ok(Tag::Compound(map));
    }
    if strings > 0 {
        return Err(ConvertError::new(
            "a table cannot mix string keys and sequence keys",
        ));
    }
    let mut list: Vec<Tag> = Vec::new();
    for (index, value) in sequence(entries)?.iter().enumerate() {
        let tag = read_value(value, level, budget)
            .map_err(|e| ConvertError::new(format!("[{}]: {e}", index + 1)))?;
        if let Some(first) = list.first().filter(|first| first.id() != tag.id()) {
            return Err(ConvertError::new(format!(
                "a list cannot mix {} and {}",
                kind(first),
                kind(&tag)
            )));
        }
        list.push(tag);
    }
    Ok(Tag::List(list))
}

/// A tagged value: exactly `__nbt` (a tag name) and `value`.
fn read_tagged(table: &Table) -> Result<Tag, ConvertError> {
    let entries = raw_entries(table, 2, "tagged NBT value")?;
    let name = raw_field(table, TAG_KEY)?;
    let value = raw_field(table, VALUE_KEY)?;
    if entries.len() != 2 || value.is_nil() {
        return Err(ConvertError::new(
            "a tagged NBT value holds exactly __nbt and value",
        ));
    }
    let name = String::from_lua_value(&name)?;
    tagged_from(&name, &value)
}

/// The tag a tagged value named `name` holds, with the value checked for it.
fn tagged_from(name: &str, value: &Value) -> Result<Tag, ConvertError> {
    match name {
        "byte" => Ok(Tag::Byte(i8::from_lua_value(value)?)),
        "short" => Ok(Tag::Short(i16::from_lua_value(value)?)),
        "long" => Ok(Tag::Long(i64::from_lua_value(value)?)),
        "float" => Ok(Tag::Float(f32::from_lua_value(value)?.to_bits())),
        "double" => Ok(Tag::Double(f64::from_lua_value(value)?.to_bits())),
        // A list tag holds no element type, so an empty list is the same whatever it was in Java.
        "empty_list" => match value {
            Value::Table(table) => {
                raw_entries(table, 0, "empty_list value")?;
                Ok(Tag::List(Vec::new()))
            }
            other => Err(ConvertError::expected("table", other)),
        },
        // Java bytes are signed; they are stored as their bit patterns.
        "byte_array" => array(value, i8::from_lua_value)
            .map(|v| Tag::ByteArray(v.into_iter().map(|b| b as u8).collect())),
        "int_array" => array(value, i32::from_lua_value).map(Tag::IntArray),
        "long_array" => array(value, i64::from_lua_value).map(Tag::LongArray),
        other => Err(ConvertError::new(format!("unknown NBT tag {other:?}"))),
    }
}

/// The elements of an array table, each converted, at most [`MAX_ARRAY`].
fn array<T>(
    value: &Value,
    convert: impl Fn(&Value) -> Result<T, ConvertError>,
) -> Result<Vec<T>, ConvertError> {
    let Value::Table(table) = value else {
        return Err(ConvertError::expected("array table", value));
    };
    let values = sequence(raw_entries(table, MAX_ARRAY, "array")?)?;
    values.iter().map(convert).collect()
}

impl FromLuaValue for Tag {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        read_value(value, 0, &mut Budget::default())
    }
}

impl FromLuaValue for Compound {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        match Tag::from_lua_value(value)? {
            Tag::Compound(map) => Ok(map),
            other => Err(ConvertError::new(format!(
                "expected compound, got {}",
                kind(&other)
            ))),
        }
    }
}

/// A Java string as UTF-8 (it must have no unpaired surrogate) of at most [`MAX_STRING`] bytes.
fn utf8(text: Result<String, std::string::FromUtf16Error>) -> mlua::Result<String> {
    let text = text.map_err(|_| mlua::Error::runtime("NBT string has an unpaired surrogate"))?;
    if text.len() > MAX_STRING {
        return Err(mlua::Error::runtime(format!(
            "NBT string is longer than {MAX_STRING} bytes"
        )));
    }
    Ok(text)
}

/// `{__nbt = name, value = value}`.
fn tagged(lua: &Lua, name: &str, value: Value) -> mlua::Result<Value> {
    let table = lua.create_table_with_capacity(0, 2)?;
    table.raw_set(TAG_KEY, name)?;
    table.raw_set(VALUE_KEY, value)?;
    Ok(Value::Table(table))
}

/// A Lua sequence of the elements, at most [`MAX_ARRAY`].
fn array_value<T: ToLuaValue>(lua: &Lua, items: &[T]) -> mlua::Result<Value> {
    if items.len() > MAX_ARRAY {
        return Err(mlua::Error::runtime(format!(
            "NBT array has more than {MAX_ARRAY} elements"
        )));
    }
    let table = lua.create_table_with_capacity(items.len(), 0)?;
    for (index, item) in items.iter().enumerate() {
        table.raw_set(index + 1, item.to_lua_value(lua)?)?;
    }
    Ok(Value::Table(table))
}

/// A compound as a table. Its own tag is counted by the caller.
fn write_map(lua: &Lua, map: &Compound, parent: usize, budget: &mut Budget) -> mlua::Result<Table> {
    let level = level_under(parent).map_err(limit)?;
    let table = lua.create_table()?;
    for (key, value) in map {
        let name = utf8(key.to_string())?;
        if name == TAG_KEY {
            return Err(mlua::Error::runtime(format!("{TAG_KEY} is a reserved key")));
        }
        table.raw_set(name.as_str(), write_tag(lua, value, level, budget)?)?;
    }
    Ok(table)
}

/// A list as a sequence table, all its elements of one tag. Its own tag is counted by the caller.
fn write_list(lua: &Lua, items: &[Tag], parent: usize, budget: &mut Budget) -> mlua::Result<Table> {
    let level = level_under(parent).map_err(limit)?;
    let mixed = items.first().and_then(|first| {
        items
            .iter()
            .find(|item| item.id() != first.id())
            .map(|other| (first, other))
    });
    if let Some((first, other)) = mixed {
        return Err(mlua::Error::runtime(format!(
            "a list cannot mix {} and {}",
            kind(first),
            kind(other)
        )));
    }
    let table = lua.create_table_with_capacity(items.len(), 0)?;
    for (index, item) in items.iter().enumerate() {
        table.raw_set(index + 1, write_tag(lua, item, level, budget)?)?;
    }
    Ok(table)
}

/// A tag as a Lua value, counting it against the budget.
fn write_tag(lua: &Lua, tag: &Tag, parent: usize, budget: &mut Budget) -> mlua::Result<Value> {
    budget.tag().map_err(limit)?;
    match tag {
        Tag::End => Err(mlua::Error::runtime("NBT end has no Lua value")),
        Tag::Byte(b) => tagged(lua, "byte", b.to_lua_value(lua)?),
        Tag::Short(s) => tagged(lua, "short", s.to_lua_value(lua)?),
        Tag::Int(i) => i.to_lua_value(lua),
        Tag::Long(l) => tagged(lua, "long", l.to_lua_value(lua)?),
        Tag::Float(bits) => tagged(lua, "float", f32::from_bits(*bits).to_lua_value(lua)?),
        Tag::Double(bits) => {
            let n = f64::from_bits(*bits);
            let plain = n.to_lua_value(lua)?;
            if n.trunc() == n {
                tagged(lua, "double", plain)
            } else {
                Ok(plain)
            }
        }
        Tag::String(s) => Ok(Value::String(lua.create_string(utf8(s.to_string())?)?)),
        Tag::ByteArray(bytes) => {
            let signed: Vec<i8> = bytes.iter().map(|b| *b as i8).collect();
            tagged(lua, "byte_array", array_value(lua, &signed)?)
        }
        Tag::IntArray(ints) => tagged(lua, "int_array", array_value(lua, ints)?),
        Tag::LongArray(longs) => tagged(lua, "long_array", array_value(lua, longs)?),
        // The element type of an empty list is not preserved: `Tag::List` holds none, so it reads
        // back as the empty list of the one type it has.
        Tag::List(items) if items.is_empty() => {
            tagged(lua, "empty_list", Value::Table(lua.create_table()?))
        }
        Tag::List(items) => write_list(lua, items, parent, budget).map(Value::Table),
        Tag::Compound(map) => write_map(lua, map, parent, budget).map(Value::Table),
    }
}

impl ToLuaValue for Tag {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        write_tag(lua, self, 0, &mut Budget::default())
    }
}

impl ToLuaValue for Compound {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let mut budget = Budget::default();
        budget.tag().map_err(limit)?;
        write_map(lua, self, 0, &mut budget).map(Value::Table)
    }
}

/// The read-only `nbt` table of constructors, to set as a global: `nbt.byte(n)`, `nbt.short(n)`,
/// `nbt.long(n)`, `nbt.float(n)`, `nbt.double(n)`, `nbt.empty_list()` and `nbt.byte_array(t)`, `nbt.int_array(t)`, `nbt.long_array(t)`.
/// Each checks its value now and returns a tagged table.
pub fn install_constructors(lua: &Lua) -> mlua::Result<Table> {
    let nbt = lua.create_table()?;
    for name in [
        "byte",
        "short",
        "long",
        "float",
        "double",
        "byte_array",
        "int_array",
        "long_array",
    ] {
        let constructor = lua.create_function(move |lua, value: Value| {
            tagged_from(name, &value).map_err(limit)?;
            tagged(lua, name, value)
        })?;
        nbt.raw_set(name, constructor)?;
    }
    let empty_list = lua
        .create_function(|lua, ()| tagged(lua, "empty_list", Value::Table(lua.create_table()?)))?;
    nbt.raw_set("empty_list", empty_list)?;
    nbt.set_readonly(true);
    Ok(nbt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(lua: &Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk runs")
    }

    fn read(source: &str) -> Result<Compound, ConvertError> {
        let lua = Lua::new();
        let nbt = install_constructors(&lua).expect("constructors");
        lua.globals().raw_set("nbt", nbt).expect("global");
        // A constructor that refuses its value fails the chunk: that is an error too.
        let value = lua
            .load(source)
            .eval::<Value>()
            .map_err(|e| ConvertError::new(e.to_string()))?;
        Compound::from_lua_value(&value)
    }

    /// The value under `name`, read without naming the key type.
    fn get<'a>(map: &'a Compound, name: &str) -> Option<&'a Tag> {
        map.iter()
            .find(|(key, _)| key.to_string_lossy() == name)
            .map(|(_, tag)| tag)
    }

    /// A string tag holding an unpaired surrogate, which Java strings can hold.
    fn lone_surrogate() -> Tag {
        let mut tag = Tag::String("x".into());
        if let Tag::String(text) = &mut tag {
            text.0 = vec![0xD800];
        }
        tag
    }

    /// `levels` nested compounds, the innermost holding `x = 1`.
    fn nested(lua: &Lua, levels: usize) -> Value {
        let mut inner = lua.create_table().expect("table");
        inner.raw_set("x", 1).expect("set");
        for _ in 1..levels {
            let outer = lua.create_table().expect("table");
            outer.raw_set("c", inner).expect("set");
            inner = outer;
        }
        Value::Table(inner)
    }

    #[test]
    fn scalars_infer_their_tags() {
        let map = read(
            "return {a = true, b = false, c = 5, d = 3000000000, e = 1.5, f = \"hi\", g = -2147483649}",
        )
        .expect("compound");
        assert_eq!(get(&map, "a"), Some(&Tag::Byte(1)));
        assert_eq!(get(&map, "b"), Some(&Tag::Byte(0)));
        assert_eq!(get(&map, "c"), Some(&Tag::Int(5)));
        assert_eq!(get(&map, "d"), Some(&Tag::Long(3_000_000_000)));
        assert_eq!(get(&map, "e"), Some(&Tag::Double(0x3FF8_0000_0000_0000)));
        assert_eq!(get(&map, "f"), Some(&Tag::String("hi".into())));
        assert_eq!(get(&map, "g"), Some(&Tag::Long(-2_147_483_649)));
    }

    #[test]
    fn lua_integers_are_an_int_or_a_long() {
        let lua = Lua::new();
        assert_eq!(Tag::from_lua_value(&Value::Integer(5)), Ok(Tag::Int(5)));
        assert_eq!(
            Tag::from_lua_value(&Value::Integer(3_000_000_000)),
            Ok(Tag::Long(3_000_000_000))
        );
        assert!(Tag::from_lua_value(&eval(&lua, "return 0/0")).is_err());
    }

    #[test]
    fn tagged_constructors_make_their_tags() {
        let map = read(
            "return {a = nbt.byte(7), b = nbt.short(-3), c = nbt.long(5), d = nbt.float(0.5), e = nbt.double(2), f = nbt.empty_list()}",
        )
        .expect("compound");
        assert_eq!(get(&map, "a"), Some(&Tag::Byte(7)));
        assert_eq!(get(&map, "b"), Some(&Tag::Short(-3)));
        assert_eq!(get(&map, "c"), Some(&Tag::Long(5)));
        assert_eq!(get(&map, "d"), Some(&Tag::Float(0x3F00_0000)));
        assert_eq!(get(&map, "e"), Some(&Tag::Double(0x4000_0000_0000_0000)));
        assert_eq!(get(&map, "f"), Some(&Tag::List(Vec::new())));
    }

    #[test]
    fn constructors_check_their_range_and_the_table_is_read_only() {
        assert!(read("return {a = nbt.byte(300)}").is_err());
        assert!(read("return {a = nbt.byte(1.5)}").is_err());
        assert!(read("return {a = nbt.float(1e300)}").is_err());
        let lua = Lua::new();
        let nbt = install_constructors(&lua).expect("constructors");
        lua.globals().raw_set("nbt", nbt).expect("global");
        assert!(lua.load("nbt.byte = 1").exec().is_err());
    }

    #[test]
    fn arrays_of_each_kind() {
        let map = read(
            "return {i = nbt.int_array({1, -2, 3}), b = nbt.byte_array({1, -1}), l = nbt.long_array({4, 5})}",
        )
        .expect("compound");
        assert_eq!(get(&map, "i"), Some(&Tag::IntArray(vec![1, -2, 3])));
        assert_eq!(get(&map, "b"), Some(&Tag::ByteArray(vec![1, 0xFF])));
        assert_eq!(get(&map, "l"), Some(&Tag::LongArray(vec![4, 5])));
        assert!(read("return {b = nbt.byte_array({200})}").is_err());
        assert!(read("return {i = nbt.int_array({\"x\"})}").is_err());
    }

    #[test]
    fn nested_compound_with_a_list_of_compounds() {
        let map = read("return {items = {{id = 1}, {id = 2}}, empty = {}}").expect("compound");
        let id = |n: i32| Tag::compound([("id", Tag::Int(n))]);
        assert_eq!(get(&map, "items"), Some(&Tag::List(vec![id(1), id(2)])));
        assert_eq!(get(&map, "empty"), Some(&Tag::Compound(Compound::new())));
    }

    #[test]
    fn mixed_list_is_refused() {
        assert!(read("return {l = {1, \"a\"}}").is_err());
        assert!(read("return {l = {1, 2.5}}").is_err());
    }

    #[test]
    fn string_and_sequence_keys_cannot_mix() {
        assert!(read("return {l = {1, 2, x = 3}}").is_err());
    }

    #[test]
    fn depth_sixteen_is_accepted_and_seventeen_refused() {
        let lua = Lua::new();
        assert!(Compound::from_lua_value(&nested(&lua, MAX_DEPTH)).is_ok());
        let err = Compound::from_lua_value(&nested(&lua, MAX_DEPTH + 1)).expect_err("too deep");
        assert!(err.to_string().contains("16"), "{err}");
    }

    #[test]
    fn self_referential_table_is_refused_without_overflowing() {
        let lua = Lua::new();
        let t = lua.create_table().expect("table");
        t.raw_set("c", t.clone()).expect("set");
        let err = Compound::from_lua_value(&Value::Table(t)).expect_err("cycle");
        assert!(err.to_string().contains("deeper"), "{err}");
    }

    #[test]
    fn more_than_4096_tags_are_refused_the_root_counting() {
        let lua = Lua::new();
        let table = |entries: usize| {
            let t = lua.create_table().expect("table");
            for i in 0..entries {
                t.raw_set(format!("k{i}"), 1).expect("set");
            }
            Value::Table(t)
        };
        // the root plus 4095 entries is exactly 4096 tags
        assert!(Compound::from_lua_value(&table(MAX_ENTRIES - 1)).is_ok());
        let err = Compound::from_lua_value(&table(MAX_ENTRIES)).expect_err("4097");
        assert!(err.to_string().contains("4096"), "{err}");
    }

    #[test]
    fn strings_longer_than_32767_bytes_are_refused() {
        let lua = Lua::new();
        let root = |len: usize| {
            let t = lua.create_table().expect("table");
            let s = lua.create_string("a".repeat(len)).expect("string");
            t.raw_set("s", s).expect("set");
            Value::Table(t)
        };
        assert!(Compound::from_lua_value(&root(MAX_STRING)).is_ok());
        assert!(Compound::from_lua_value(&root(MAX_STRING + 1)).is_err());
    }

    #[test]
    fn nan_and_infinity_are_refused() {
        assert!(read("return {a = 0/0}").is_err());
        assert!(read("return {a = 1/0}").is_err());
    }

    #[test]
    fn metamethods_are_not_run() {
        let lua = Lua::new();
        let t = lua.create_table().expect("table");
        t.raw_set("a", 2.5).expect("set");
        let meta = lua.create_table().expect("meta");
        let failing = lua
            .create_function(|_, ()| -> mlua::Result<Value> {
                Err(mlua::Error::runtime("metamethod ran"))
            })
            .expect("function");
        meta.raw_set("__index", failing.clone()).expect("set");
        meta.raw_set("__pairs", failing).expect("set");
        t.set_metatable(Some(meta)).expect("metatable");
        let map = Compound::from_lua_value(&Value::Table(t)).expect("raw fields only");
        assert_eq!(get(&map, "a"), Some(&Tag::Double(2.5f64.to_bits())));
    }

    #[test]
    fn tagged_value_must_hold_exactly_its_name_and_value() {
        assert!(read("return {a = {__nbt = \"byte\", value = 1, extra = 2}}").is_err());
        assert!(read("return {a = {__nbt = \"nope\", value = 1}}").is_err());
        assert!(read("return {a = {__nbt = \"byte\"}}").is_err());
        assert!(read("return {a = {__nbt = 3, value = 1}}").is_err());
    }

    #[test]
    fn round_trip_keeps_every_tag_kind() {
        let mut map = Compound::new();
        map.insert("a".into(), Tag::Byte(-3));
        map.insert("b".into(), Tag::Short(-300));
        map.insert("c".into(), Tag::Int(70_000));
        map.insert("d".into(), Tag::Long(5_000_000_000));
        map.insert("e".into(), Tag::Float(0x3F00_0000));
        map.insert("f".into(), Tag::Double(0x3FF8_0000_0000_0000));
        map.insert("g".into(), Tag::ByteArray(vec![1, 0xFF]));
        map.insert("h".into(), Tag::String("héllo".into()));
        map.insert("i".into(), Tag::List(vec![Tag::Int(1), Tag::Int(2)]));
        map.insert("j".into(), Tag::compound([("k", Tag::Int(9))]));
        map.insert("k".into(), Tag::IntArray(vec![1, 2]));
        map.insert("l".into(), Tag::LongArray(vec![6]));
        map.insert("m".into(), Tag::Double(2.0f64.to_bits()));
        map.insert("n".into(), Tag::List(Vec::new()));
        let lua = Lua::new();
        let value = map.to_lua_value(&lua).expect("to lua");
        assert_eq!(Compound::from_lua_value(&value), Ok(map));
    }

    #[test]
    fn integral_double_is_written_tagged_and_reads_back_exactly() {
        let lua = Lua::new();
        let value = Tag::Double(2.0f64.to_bits())
            .to_lua_value(&lua)
            .expect("to lua");
        let Value::Table(table) = &value else {
            panic!("a tagged table, got {value:?}")
        };
        assert_eq!(table.raw_get::<String>(TAG_KEY).expect("name"), "double");
        assert_eq!(
            Tag::from_lua_value(&value),
            Ok(Tag::Double(2.0f64.to_bits()))
        );
        let half = Tag::Double(1.5f64.to_bits())
            .to_lua_value(&lua)
            .expect("to lua");
        assert!(matches!(half, Value::Number(n) if n == 1.5));
    }

    #[test]
    fn empty_list_is_written_tagged_and_reads_back_as_an_empty_list() {
        let lua = Lua::new();
        let value = Tag::List(Vec::new()).to_lua_value(&lua).expect("to lua");
        let Value::Table(table) = &value else {
            panic!("a tagged table, got {value:?}")
        };
        assert_eq!(
            table.raw_get::<String>(TAG_KEY).expect("name"),
            "empty_list"
        );
        assert_eq!(Tag::from_lua_value(&value), Ok(Tag::List(Vec::new())));
    }

    #[test]
    fn double_and_empty_list_refuse_the_wrong_value() {
        assert!(read("return {a = {__nbt = \"double\", value = \"x\"}}").is_err());
        assert!(read("return {a = {__nbt = \"empty_list\", value = {1}}}").is_err());
        assert!(read("return {a = {__nbt = \"empty_list\", value = 3}}").is_err());
        assert!(read("return {a = {__nbt = \"double\", value = 0/0}}").is_err());
    }

    #[test]
    fn output_refuses_what_lua_cannot_hold() {
        let lua = Lua::new();
        assert!(Tag::Double(f64::NAN.to_bits()).to_lua_value(&lua).is_err());
        assert!(
            Tag::Float(f32::INFINITY.to_bits())
                .to_lua_value(&lua)
                .is_err()
        );
        assert!(Tag::End.to_lua_value(&lua).is_err());
        assert!(lone_surrogate().to_lua_value(&lua).is_err());
        let mixed = Tag::List(vec![Tag::Int(1), Tag::Double(1.5f64.to_bits())]);
        assert!(mixed.to_lua_value(&lua).is_err());
        let reserved = Tag::compound([(TAG_KEY, Tag::Int(1))]);
        assert!(reserved.to_lua_value(&lua).is_err());
    }
}
