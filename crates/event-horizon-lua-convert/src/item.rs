//! Item stacks and block states between Lua tables and Rust, as validated specs.
//!
//! A block state is `{id = "minecraft:oak_stairs", props = {facing = "north"}}`. `props` is
//! optional and holds string values only. An item stack is
//! `{id = "minecraft:diamond_sword", count = 1, nbt = {...}}`: `count` defaults to 1 and
//! `nbt` is an optional component patch. A stack with count 0 is the empty stack.
//!
//! The mod API names items and block states by process-local keys (`ItemKey`, `BlockKey`)
//! that only the game's registry creates, and a converter has no registry. So this module
//! reads and writes the canonical id as a checked string, next to the counts, properties and
//! component patch. The host turns an id into its key and builds the mod API value.
//!
//! Ids follow [`mod_api::Identifier::parse`]: a missing namespace is `minecraft`, the
//! namespace is `[a-z0-9_.-]` and the path is `[a-z0-9_./-]`. The id is stored in its
//! canonical `namespace:path` form. Whether the id exists in the registry is not checked here.
//!
//! Limits: an id is at most [`MAX_ID`] bytes, a block state has at most [`MAX_PROPS`]
//! properties, each name and value is `[a-z0-9_]` and at most [`MAX_PROP_LEN`] bytes, and a
//! stack's count is at most [`MAX_COUNT`]. The type's own maximum stack size is not checked
//! here: it depends on the item, so the registry check at the write boundary applies it.

use mlua::{Lua, Table, Value};
use mod_api::{Compound, Identifier, StateConstraint};

use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};

/// The longest id, in bytes.
pub const MAX_ID: usize = 256;
/// The most properties in one block state.
pub const MAX_PROPS: usize = 64;
/// The longest property name or value, in bytes.
pub const MAX_PROP_LEN: usize = 64;
/// The largest count a Lua item stack may name.
pub const MAX_COUNT: u32 = 99;

/// A block state's id and properties, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStateSpec {
    /// Canonical `namespace:path`.
    pub id: String,
    /// Property constraints, sorted by name, each name and value checked.
    pub props: Vec<StateConstraint>,
}

/// An item stack's id, count and component patch. A count of 0 is the empty stack.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemStackSpec {
    /// Canonical `namespace:path`.
    pub id: String,
    /// Items in the stack, from 0 (empty) to [`MAX_COUNT`].
    pub count: u32,
    /// The component patch, read as a compound; `None` when the table has no `nbt`.
    pub nbt: Option<Compound>,
}

impl ItemStackSpec {
    /// Whether this spec is the empty stack (count 0).
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

impl FromLuaValue for BlockStateSpec {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let table = table_of(value, "block state")?;
        let id = read_id(table)?;
        let props = read_props(table)?;
        Ok(Self { id, props })
    }
}

impl ToLuaValue for BlockStateSpec {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let table = lua.create_table()?;
        table.raw_set("id", self.id.to_lua_value(lua)?)?;
        let props = lua.create_table()?;
        for constraint in &self.props {
            props.raw_set(constraint.name.as_str(), constraint.value.as_str())?;
        }
        table.raw_set("props", props)?;
        Ok(Value::Table(table))
    }
}

impl FromLuaValue for ItemStackSpec {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        let table = table_of(value, "item stack")?;
        let id = read_id(table)?;
        let count = read_count(table)?;
        let nbt = read_nbt(table)?;
        Ok(Self { id, count, nbt })
    }
}

impl ToLuaValue for ItemStackSpec {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let table = lua.create_table()?;
        table.raw_set("id", self.id.to_lua_value(lua)?)?;
        table.raw_set("count", self.count.to_lua_value(lua)?)?;
        if let Some(nbt) = &self.nbt {
            table.raw_set("nbt", nbt.to_lua_value(lua)?)?;
        }
        Ok(Value::Table(table))
    }
}

/// The table a spec is read from, or an error naming what was expected.
fn table_of<'a>(value: &'a Value, expected: &str) -> Result<&'a Table, ConvertError> {
    match value {
        Value::Table(table) => Ok(table),
        other => Err(ConvertError::expected(expected, other)),
    }
}

/// One field of a table, read raw: `__index` is not run.
fn field(table: &Table, key: &str) -> Result<Value, ConvertError> {
    table
        .raw_get::<Value>(key)
        .map_err(|_| ConvertError::new(format!("cannot read {key}")))
}

/// Prefixes an error with the key it came from.
fn named(key: &str, error: ConvertError) -> ConvertError {
    ConvertError::new(format!("{key}: {error}"))
}

/// The `id` field, checked: present, a string, and a valid identifier in canonical form.
fn read_id(table: &Table) -> Result<String, ConvertError> {
    let value = field(table, "id")?;
    if value.is_nil() {
        return Err(ConvertError::new("missing id"));
    }
    let id = String::from_lua_value(&value).map_err(|e| named("id", e))?;
    canonical_id(&id).map_err(|e| named("id", e))
}

/// The canonical `namespace:path` for an id, or an error that does not echo the input.
pub fn canonical_id(id: &str) -> Result<String, ConvertError> {
    if id.len() > MAX_ID {
        return Err(ConvertError::new(format!("longer than {MAX_ID} bytes")));
    }
    let parsed = Identifier::parse(id).map_err(|_| ConvertError::new("not a valid identifier"))?;
    if parsed.path().is_empty() {
        return Err(ConvertError::new("the path is empty"));
    }
    Ok(format!("{}:{}", parsed.namespace(), parsed.path()))
}

/// The `props` field: nil is no properties; otherwise a table of string names to string values.
fn read_props(table: &Table) -> Result<Vec<StateConstraint>, ConvertError> {
    let value = field(table, "props")?;
    let props = match &value {
        Value::Nil => return Ok(Vec::new()),
        Value::Table(props) => props,
        other => return Err(named("props", ConvertError::expected("table", other))),
    };
    let mut constraints = Vec::new();
    for pair in props.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|_| ConvertError::new("props: cannot read"))?;
        if constraints.len() == MAX_PROPS {
            return Err(ConvertError::new(format!(
                "props: more than {MAX_PROPS} properties"
            )));
        }
        let name = String::from_lua_value(&key).map_err(|e| named("props", e))?;
        let text = String::from_lua_value(&value).map_err(|e| named("props", e))?;
        check_word("property name", &name)?;
        check_word("property value", &text)?;
        constraints.push(StateConstraint::new(name, text));
    }
    constraints.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(constraints)
}

/// A property name or value: nonempty, at most [`MAX_PROP_LEN`] bytes, of `[a-z0-9_]`.
fn check_word(what: &str, word: &str) -> Result<(), ConvertError> {
    let ok = !word.is_empty()
        && word.len() <= MAX_PROP_LEN
        && word
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
    if ok {
        Ok(())
    } else {
        Err(ConvertError::new(format!(
            "{what} must be [a-z0-9_], 1 to {MAX_PROP_LEN} bytes"
        )))
    }
}

/// The `count` field: nil is 1; otherwise an integer from 0 to [`MAX_COUNT`].
fn read_count(table: &Table) -> Result<u32, ConvertError> {
    let value = field(table, "count")?;
    if value.is_nil() {
        return Ok(1);
    }
    let count = u32::from_lua_value(&value).map_err(|e| named("count", e))?;
    if count > MAX_COUNT {
        return Err(ConvertError::new(format!(
            "count: expected 0 to {MAX_COUNT}, got {count}"
        )));
    }
    Ok(count)
}

/// The `nbt` field: nil is no patch; otherwise a compound.
fn read_nbt(table: &Table) -> Result<Option<Compound>, ConvertError> {
    let value = field(table, "nbt")?;
    Option::<Compound>::from_lua_value(&value).map_err(|e| named("nbt", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::Tag;

    fn eval(lua: &Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk runs")
    }

    fn block(lua: &Lua, source: &str) -> Result<BlockStateSpec, ConvertError> {
        BlockStateSpec::from_lua_value(&eval(lua, source))
    }

    fn stack(lua: &Lua, source: &str) -> Result<ItemStackSpec, ConvertError> {
        ItemStackSpec::from_lua_value(&eval(lua, source))
    }

    fn prop_pairs(spec: &BlockStateSpec) -> Vec<(&str, &str)> {
        spec.props
            .iter()
            .map(|c| (c.name.as_str(), c.value.as_str()))
            .collect()
    }

    #[test]
    fn block_state_with_props_is_read_sorted() {
        let lua = Lua::new();
        let spec = block(
            &lua,
            r#"return {id = "minecraft:oak_stairs", props = {half = "bottom", facing = "north"}}"#,
        )
        .expect("valid");
        assert_eq!(spec.id, "minecraft:oak_stairs");
        assert_eq!(
            prop_pairs(&spec),
            vec![("facing", "north"), ("half", "bottom")]
        );
    }

    #[test]
    fn block_state_without_props_has_none() {
        let lua = Lua::new();
        let spec = block(&lua, r#"return {id = "minecraft:stone"}"#).expect("valid");
        assert!(spec.props.is_empty());
        let empty = block(&lua, r#"return {id = "minecraft:stone", props = nil}"#).expect("valid");
        assert!(empty.props.is_empty());
    }

    #[test]
    fn block_id_without_namespace_is_minecraft() {
        let lua = Lua::new();
        let spec = block(&lua, r#"return {id = "oak_stairs"}"#).expect("valid");
        assert_eq!(spec.id, "minecraft:oak_stairs");
        let spec = block(&lua, r#"return {id = ":oak_stairs"}"#).expect("valid");
        assert_eq!(spec.id, "minecraft:oak_stairs");
    }

    #[test]
    fn id_path_may_contain_slashes_and_dots() {
        let lua = Lua::new();
        let spec = block(&lua, r#"return {id = "my_mod:blocks/ruby.ore-1"}"#).expect("valid");
        assert_eq!(spec.id, "my_mod:blocks/ruby.ore-1");
    }

    #[test]
    fn bad_ids_are_refused() {
        let lua = Lua::new();
        for source in [
            r#"return {id = "minecraft:Stone"}"#,
            r#"return {id = "a:b:c"}"#,
            r#"return {id = "minecraft:oak stairs"}"#,
            r#"return {id = ""}"#,
            r#"return {id = "minecraft:"}"#,
            r#"return {id = ":"}"#,
            r#"return {id = "minecraft:a\\"}"#,
        ] {
            let err = block(&lua, source).expect_err(source);
            assert!(err.to_string().starts_with("id: "), "{source}: {err}");
        }
    }

    #[test]
    fn id_of_256_bytes_is_accepted_and_257_refused() {
        let lua = Lua::new();
        let path = "a".repeat(MAX_ID - "minecraft:".len());
        let ok = format!(r#"return {{id = "minecraft:{path}"}}"#);
        assert_eq!(block(&lua, &ok).expect("256 bytes").id.len(), MAX_ID);
        let long = format!(r#"return {{id = "minecraft:{path}a"}}"#);
        assert!(block(&lua, &long).is_err());
    }

    #[test]
    fn missing_or_mistyped_id_is_an_error() {
        let lua = Lua::new();
        assert_eq!(
            block(&lua, "return {props = {}}")
                .expect_err("missing")
                .to_string(),
            "missing id"
        );
        let err = block(&lua, "return {id = 5}").expect_err("number");
        assert!(err.to_string().starts_with("id: expected string"), "{err}");
    }

    #[test]
    fn props_limits_are_enforced() {
        let lua = Lua::new();
        let props = |n: usize| {
            (0..n)
                .map(|i| format!("p{i} = \"v\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let ok = format!(
            r#"return {{id = "minecraft:x", props = {{{}}}}}"#,
            props(64)
        );
        assert_eq!(block(&lua, &ok).expect("64 props").props.len(), 64);
        let too_many = format!(
            r#"return {{id = "minecraft:x", props = {{{}}}}}"#,
            props(65)
        );
        let err = block(&lua, &too_many).expect_err("65 props");
        assert!(err.to_string().contains("more than 64"), "{err}");
    }

    #[test]
    fn prop_words_are_checked() {
        let lua = Lua::new();
        let refused = [
            r#"props = {facing = "north west"}"#,
            r#"props = {Facing = "north"}"#,
            r#"props = {facing = "north-west"}"#,
            r#"props = {facing = ""}"#,
            r#"props = {[""] = "north"}"#,
            r#"props = {facing = "]"}"#,
        ];
        for props in refused {
            let source = format!(r#"return {{id = "minecraft:x", {props}}}"#);
            assert!(block(&lua, &source).is_err(), "{props}");
        }
        let long = "a".repeat(MAX_PROP_LEN + 1);
        let source = format!(r#"return {{id = "minecraft:x", props = {{k = "{long}"}}}}"#);
        assert!(block(&lua, &source).is_err());
        let exact = "a".repeat(MAX_PROP_LEN);
        let source = format!(r#"return {{id = "minecraft:x", props = {{k = "{exact}"}}}}"#);
        assert!(block(&lua, &source).is_ok());
    }

    #[test]
    fn props_must_be_a_table_with_string_names() {
        let lua = Lua::new();
        let err = block(&lua, r#"return {id = "minecraft:x", props = "facing"}"#)
            .expect_err("string props");
        assert!(
            err.to_string().starts_with("props: expected table"),
            "{err}"
        );
        let err = block(&lua, r#"return {id = "minecraft:x", props = {"north"}}"#)
            .expect_err("sequence props");
        assert!(
            err.to_string().starts_with("props: expected string"),
            "{err}"
        );
    }

    #[test]
    fn stack_with_count_and_nbt_is_read() {
        let lua = Lua::new();
        let spec = stack(
            &lua,
            r#"return {id = "minecraft:diamond_sword", count = 3, nbt = {damage = 7}}"#,
        )
        .expect("valid");
        assert_eq!(spec.id, "minecraft:diamond_sword");
        assert_eq!(spec.count, 3);
        assert!(!spec.is_empty());
        let nbt = spec.nbt.expect("nbt present");
        assert_eq!(nbt.len(), 1);
        assert!(matches!(nbt.get(&"damage".into()), Some(Tag::Int(7))));
    }

    #[test]
    fn stack_count_defaults_to_one_and_nbt_to_none() {
        let lua = Lua::new();
        let spec = stack(&lua, r#"return {id = "minecraft:stone"}"#).expect("valid");
        assert_eq!(spec.count, 1);
        assert!(spec.nbt.is_none());
    }

    #[test]
    fn empty_stack_is_count_zero() {
        let lua = Lua::new();
        let spec = stack(&lua, r#"return {id = "minecraft:air", count = 0}"#).expect("valid");
        assert!(spec.is_empty());
        assert_eq!(spec.count, 0);
        assert!(
            Option::<ItemStackSpec>::from_lua_value(&Value::Nil)
                .expect("nil")
                .is_none()
        );
    }

    #[test]
    fn stack_count_limits_are_enforced() {
        let lua = Lua::new();
        let count = |n: &str| format!(r#"return {{id = "minecraft:stone", count = {n}}}"#);
        assert_eq!(stack(&lua, &count("99")).expect("99").count, 99);
        let err = stack(&lua, &count("100")).expect_err("100");
        assert!(
            err.to_string().contains("expected 0 to 99, got 100"),
            "{err}"
        );
        assert!(stack(&lua, &count("-1")).is_err());
        assert!(stack(&lua, &count("1.5")).is_err());
        assert!(stack(&lua, &count("\"5\"")).is_err());
        let err = stack(&lua, &count("1.5")).expect_err("fraction");
        assert!(err.to_string().starts_with("count: "), "{err}");
    }

    #[test]
    fn stack_nbt_must_be_a_compound() {
        let lua = Lua::new();
        let err =
            stack(&lua, r#"return {id = "minecraft:stone", nbt = "x"}"#).expect_err("string nbt");
        assert!(err.to_string().starts_with("nbt: "), "{err}");
    }

    #[test]
    fn non_table_is_an_error_naming_the_kind() {
        let lua = Lua::new();
        let err = stack(&lua, r#"return "minecraft:stone""#).expect_err("string");
        assert_eq!(err.to_string(), "expected item stack, got string");
        let err = block(&lua, "return 3").expect_err("number");
        assert_eq!(err.to_string(), "expected block state, got integer");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let lua = Lua::new();
        let spec = stack(&lua, r#"return {id = "minecraft:stone", color = "red"}"#).expect("valid");
        assert_eq!(spec.count, 1);
    }

    #[test]
    fn metamethods_are_not_run() {
        let lua = Lua::new();
        let source = r#"
            return setmetatable({}, {__index = function() error("ran") end})
        "#;
        let err = stack(&lua, source).expect_err("no id");
        assert_eq!(err.to_string(), "missing id");
        let source = r#"
            return setmetatable({}, {__index = {id = "minecraft:stone", count = 5}})
        "#;
        assert_eq!(
            stack(&lua, source).expect_err("inherited id").to_string(),
            "missing id"
        );
        let source = r#"
            return {id = "minecraft:x", props = setmetatable({}, {__index = {facing = "north"}})}
        "#;
        assert!(block(&lua, source).expect("valid").props.is_empty());
    }

    #[test]
    fn block_state_round_trips() {
        let lua = Lua::new();
        let spec = block(
            &lua,
            r#"return {id = "minecraft:oak_stairs", props = {facing = "north", half = "top"}}"#,
        )
        .expect("valid");
        let value = spec.to_lua_value(&lua).expect("to lua");
        assert_eq!(BlockStateSpec::from_lua_value(&value), Ok(spec));
    }

    #[test]
    fn item_stack_round_trips_with_nbt_and_empty() {
        let lua = Lua::new();
        let spec = stack(
            &lua,
            r#"return {id = "minecraft:diamond_sword", count = 2, nbt = {damage = 7}}"#,
        )
        .expect("valid");
        let value = spec.to_lua_value(&lua).expect("to lua");
        assert_eq!(ItemStackSpec::from_lua_value(&value), Ok(spec));

        let empty = stack(&lua, r#"return {id = "minecraft:air", count = 0}"#).expect("valid");
        let value = empty.to_lua_value(&lua).expect("to lua");
        assert_eq!(ItemStackSpec::from_lua_value(&value), Ok(empty));
    }

    #[test]
    fn written_block_state_has_the_documented_shape() {
        let lua = Lua::new();
        let spec = block(&lua, r#"return {id = "stone", props = {a = "b"}}"#).expect("valid");
        let Value::Table(table) = spec.to_lua_value(&lua).expect("to lua") else {
            panic!("expected a table");
        };
        assert_eq!(
            table.raw_get::<String>("id").expect("id"),
            "minecraft:stone"
        );
        let props: Table = table.raw_get("props").expect("props");
        assert_eq!(props.raw_get::<String>("a").expect("a"), "b");
    }

    /// An equipment slot reads from its lowercase name, and the names written for a slot read back.
    #[test]
    fn equipment_slot_reads_its_lowercase_name_only() {
        let lua = Lua::new();
        for (source, slot) in [
            ("'main_hand'", mod_api::EquipmentSlot::MainHand),
            ("'off_hand'", mod_api::EquipmentSlot::OffHand),
            ("'head'", mod_api::EquipmentSlot::Head),
            ("'saddle'", mod_api::EquipmentSlot::Saddle),
        ] {
            let read =
                <mod_api::EquipmentSlot as FromLuaValue>::from_lua_value(&eval(&lua, source));
            assert!(read.is_ok(), "{source}");
            assert_eq!(read.ok(), Some(slot), "{source}");
        }
        for source in ["'MainHand'", "'hand'", "'Head'", "7", "nil"] {
            let read =
                <mod_api::EquipmentSlot as FromLuaValue>::from_lua_value(&eval(&lua, source));
            assert!(read.is_err(), "{source} must be refused");
        }
    }
}
