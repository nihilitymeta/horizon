//! Item stack arguments of handle reads (pw-4): an item-stack table `{id, count, nbt}` becomes the
//! game's stack through the turn's host, or it is refused with a message that does not echo the
//! table. These tests drive [`Args::stack`] and [`Args::opt_stack`] directly, with the vocab's fake
//! host and its item table; the Lua-driven refusals are in `hostcall`'s tests.

use mlua::{Lua, MultiValue, Value};
use mod_api::testing::FakeHost;
use mod_api::{Host, Identifier, Tag};

use crate::handles::Args;

/// A host that knows the two items the tests name. `minecraft:ender_pearl` has a maximum of 16.
fn host() -> FakeHost {
    let mut host = FakeHost::new();
    host.registries
        .resolve_items(
            ["minecraft:ender_pearl", "minecraft:shears"].map(|id| Identifier::parse(id).unwrap()),
        )
        .unwrap();
    host.registries.item_component(
        "minecraft:ender_pearl",
        "minecraft:max_stack_size",
        Tag::Int(16),
    );
    host
}

/// The arguments of one call with `values` as they are given, at most two of them.
fn args(values: Vec<Value>) -> Args {
    Args::new("test_op", MultiValue::from_vec(values), 2).unwrap()
}

/// `{id = id, count = count}` as a Lua table.
fn spec(lua: &Lua, id: &str, count: i64) -> Value {
    let table = lua.create_table().unwrap();
    table.raw_set("id", id).unwrap();
    table.raw_set("count", count).unwrap();
    Value::Table(table)
}

/// The message of a refused argument, with the method and argument number removed.
fn why(err: mlua::Error) -> String {
    let text = err.to_string();
    text.rsplit("argument ")
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_stack_table_becomes_the_stack_the_host_names() {
    let lua = Lua::new();
    let host = host();
    let stack = args(vec![spec(&lua, "minecraft:ender_pearl", 3)])
        .stack(0, &host as &dyn Host)
        .unwrap();
    assert_eq!(stack.count(), 3);
    assert_eq!(
        stack.item(),
        mod_api::__private::item_key("minecraft:ender_pearl").unwrap()
    );
}

#[test]
fn an_unknown_id_is_refused_with_the_named_problem_and_not_echoed() {
    let lua = Lua::new();
    let host = host();
    let err = args(vec![spec(&lua, "minecraft:no_such_item", 1)])
        .stack(0, &host as &dyn Host)
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("unknown item id"), "{text}");
    assert!(
        !text.contains("no_such_item"),
        "the table is not echoed: {text}"
    );
}

#[test]
fn a_count_of_zero_is_refused_for_a_required_stack_and_none_for_an_optional_one() {
    let lua = Lua::new();
    let host = host();
    let required = args(vec![spec(&lua, "minecraft:ender_pearl", 0)])
        .stack(0, &host as &dyn Host)
        .unwrap_err();
    assert!(why(required).contains("count of 0"), "required refusal");
    let optional = args(vec![spec(&lua, "minecraft:ender_pearl", 0)])
        .opt_stack(0, &host as &dyn Host)
        .unwrap();
    assert!(optional.is_none());
}

#[test]
fn nil_is_refused_for_a_required_stack_and_none_for_an_optional_one() {
    let host = host();
    let missing = args(vec![]).stack(0, &host as &dyn Host).unwrap_err();
    assert!(why(missing).contains("not nil"), "nil refusal");
    let explicit = args(vec![Value::Nil])
        .stack(0, &host as &dyn Host)
        .unwrap_err();
    assert!(why(explicit).contains("not nil"), "explicit nil refusal");
    assert!(
        args(vec![Value::Nil])
            .opt_stack(0, &host as &dyn Host)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_count_above_the_items_maximum_is_refused() {
    let lua = Lua::new();
    let host = host();
    let err = args(vec![spec(&lua, "minecraft:ender_pearl", 17)])
        .opt_stack(0, &host as &dyn Host)
        .unwrap_err();
    assert!(
        why(err).contains("above the maximum 16"),
        "over the maximum"
    );
}

#[test]
fn a_wrong_typed_field_or_a_non_table_is_refused() {
    let lua = Lua::new();
    let host = host();
    let table = lua.create_table().unwrap();
    table.raw_set("id", "minecraft:ender_pearl").unwrap();
    table.raw_set("count", "three").unwrap();
    let text = args(vec![Value::Table(table)])
        .stack(0, &host as &dyn Host)
        .unwrap_err()
        .to_string();
    assert!(text.contains("count"), "names the field: {text}");
    assert!(!text.contains("three"), "does not echo the value: {text}");
    let string = lua.create_string("minecraft:ender_pearl").unwrap();
    assert!(
        args(vec![Value::String(string)])
            .stack(0, &host as &dyn Host)
            .is_err()
    );
}
