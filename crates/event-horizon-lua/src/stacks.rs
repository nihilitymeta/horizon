//! Item stacks and block states between the game and Lua: the conversions the generated hooks use.
//!
//! Each conversion has two halves. The host half reads a game value into a checked spec, or a
//! spec into a game value, through [`StackHost`] and [`StateHost`]. The spec half is
//! `event-horizon-lua-convert`'s `ItemStackSpec` and `BlockStateSpec`, plain data with the rules
//! about ids, counts and properties. The host half is a thin layer over the mod API, so the rules
//! are tested here with a double whose stacks and states are strings.
//!
//! A value the host cannot name (a stack with no id) is not delivered: the hook's caller checks
//! with [`stack_in`] or [`state_in`] before it fires, logs once with [`undelivered`], and gives
//! no answer. A value a mod writes is refused whole when it does not convert, and the previous
//! value is kept.

use event_horizon_lua_convert::item::{BlockStateSpec, ItemStackSpec, MAX_COUNT};
use event_horizon_lua_convert::scalar::FromLuaValue;
use mlua::Value;
use mod_api::prelude::{BlockStatesHost, ItemsHost};
use mod_api::{
    BlockState, Compound, Host, HostSide, Identifier, ItemStack, LevelHost, LevelSide,
    StateConstraint, Tag,
};

use crate::host::warn_once;

/// What a stack conversion needs from a host: the stack's NBT escape (`id`, `count`,
/// `components`) both ways, and the item's maximum count.
pub trait StackHost {
    /// The game's stack type.
    type Stack;
    /// The stack's NBT escape; None when the host has no id for it.
    fn stack_nbt(&self, stack: &Self::Stack) -> Option<Compound>;
    /// The stack an NBT escape names; None for an unknown id, a zero count or a bad patch.
    fn stack_from_nbt(&self, nbt: &Compound) -> Option<Self::Stack>;
    /// The item's maximum count; None when the host has none to give.
    fn stack_max(&self, stack: &Self::Stack) -> Option<u32>;
}

/// What a block state conversion needs from a host: the state's text both ways.
pub trait StateHost {
    /// The game's block state type.
    type State;
    /// The state as `/setblock` writes it (`minecraft:oak_stairs[facing=east]`).
    fn state_text(&self, state: &Self::State) -> Option<String>;
    /// The state a `/setblock`-style text names; None for an unknown block, property or value.
    fn state_parse(&self, text: &str) -> Option<Self::State>;
}

impl StackHost for dyn Host + '_ {
    type Stack = ItemStack;

    fn stack_nbt(&self, stack: &ItemStack) -> Option<Compound> {
        stack.to_nbt(self)
    }

    fn stack_from_nbt(&self, nbt: &Compound) -> Option<ItemStack> {
        ItemStack::from_nbt(self, nbt)
    }

    fn stack_max(&self, stack: &ItemStack) -> Option<u32> {
        ItemsHost::max_stack_size(self, stack.item())
    }
}

impl StateHost for dyn Host + '_ {
    type State = BlockState;

    fn state_text(&self, state: &BlockState) -> Option<String> {
        BlockStatesHost::block_state_string(self, *state)
    }

    fn state_parse(&self, text: &str) -> Option<BlockState> {
        BlockStatesHost::block_state_id(self, text)
    }
}

impl StateHost for dyn LevelHost + '_ {
    type State = BlockState;

    fn state_text(&self, state: &BlockState) -> Option<String> {
        LevelHost::state_string(self, *state)
    }

    fn state_parse(&self, text: &str) -> Option<BlockState> {
        LevelHost::block_state_id(self, text)
    }
}

/// The host of a hook with no host parameter, for stacks: the process-global item table the mod API
/// installs when a world is entered. It gives the host's NBT shape for the id and components. Two
/// limits, both because the table is not the host:
/// - A stack has no item maximum here, so only [`MAX_COUNT`] is checked. The game's write boundary
///   still refuses a count above the item's maximum.
/// - Block states come from the same installed tables: a state's block is found by its id, and its
///   properties come from the block's definition. Before the tables are installed every stack and
///   state conversion is None, so the hook is not delivered.
pub struct GlobalHost;

impl StackHost for GlobalHost {
    type Stack = ItemStack;

    fn stack_nbt(&self, stack: &ItemStack) -> Option<Compound> {
        let id = mod_api::__private::item_key_id(stack.item())?;
        Some(stack_nbt_of(
            id,
            stack.count(),
            stack.damage_component(),
            stack.components().map(|patch| patch.as_nbt()),
        ))
    }

    fn stack_from_nbt(&self, nbt: &Compound) -> Option<ItemStack> {
        let Tag::String(id) = tag_of(nbt, "id")? else {
            return None;
        };
        let id = Identifier::parse(&id.to_string_lossy()).ok()?;
        let item = mod_api::__private::item_key_by_identifier(&id)?;
        let count = match tag_of(nbt, "count") {
            None => 1,
            Some(tag) => nbt_count(tag)?,
        };
        let mut stack = ItemStack::new(item, count)?;
        match tag_of(nbt, "components") {
            Some(Tag::Compound(patch)) => stack.set_components(patch.clone()).ok()?,
            None => {}
            Some(_) => return None,
        }
        Some(stack)
    }

    fn stack_max(&self, _stack: &ItemStack) -> Option<u32> {
        None
    }
}

/// A state from the global tables: the block's name and its definition's properties, in name order,
/// as the host path writes them. Read-only: nothing is installed or changed here.
impl StateHost for GlobalHost {
    type State = BlockState;

    fn state_text(&self, state: &BlockState) -> Option<String> {
        let tables = mod_api::__private::block_definitions()?;
        let id = mod_api::__private::block_key_id(state.block())?;
        let index = *tables.names.slots.get(id)? as usize;
        let properties = tables
            .definitions
            .get(index)?
            .properties_of(state.ordinal())
            .ok()?;
        let spec = BlockStateSpec {
            id: id.to_owned(),
            props: properties
                .into_iter()
                .map(|(name, value)| StateConstraint::new(name, value))
                .collect(),
        };
        Some(state_text(&spec))
    }

    fn state_parse(&self, text: &str) -> Option<BlockState> {
        let spec = spec_of_text(text)?;
        let tables = mod_api::__private::block_definitions()?;
        let block = mod_api::__private::block_key(&spec.id)?;
        BlockState::of(tables, block, &spec.props).ok()
    }
}

/// A stack's NBT escape as the host writes it: id, count, then the component patch with the
/// damage override put back into it (the host's `to_nbt`, from the parts the global tables give).
fn stack_nbt_of(id: &str, count: u32, damage: Option<u32>, patch: Option<&Compound>) -> Compound {
    let mut nbt = Compound::new();
    nbt.insert("id".into(), Tag::String(id.into()));
    nbt.insert("count".into(), count_tag(count));
    let mut patch = patch.cloned().unwrap_or_default();
    if let Some(damage) = damage {
        patch.insert("minecraft:damage".into(), count_tag(damage));
    }
    if !patch.is_empty() {
        nbt.insert("components".into(), Tag::Compound(patch));
    }
    nbt
}

/// A count or damage as the host writes it: an int, or a long above the int range.
fn count_tag(value: u32) -> Tag {
    i32::try_from(value).map_or_else(|_| Tag::Long(i64::from(value)), Tag::Int)
}

/// A count as the host reads it: any integer tag that is a `u32`.
fn nbt_count(tag: &Tag) -> Option<u32> {
    let value = match tag {
        Tag::Byte(value) => i64::from(*value),
        Tag::Short(value) => i64::from(*value),
        Tag::Int(value) => i64::from(*value),
        Tag::Long(value) => *value,
        _ => return None,
    };
    u32::try_from(value).ok()
}

/// The host of a `HostSide` hook as a `Host`: both of its sides are one.
pub fn side_host<'a>(side: &'a HostSide<'_>) -> &'a dyn Host {
    match side {
        HostSide::Server(host) => &**host,
        HostSide::Client(host) => &**host,
    }
}

/// The level of a `LevelSide` hook as a `LevelHost`: both of its sides are one.
pub fn side_level<'a>(side: &'a LevelSide<'_>) -> &'a dyn LevelHost {
    match side {
        LevelSide::Server(level) | LevelSide::Client(level) => *level,
    }
}

/// The spec of an optional stack for a Lua table: `Some(None)` when there is no stack, `Some(Some)`
/// with the spec, and `None` when the host has no id for the stack (the hook is not delivered).
pub fn stack_in<H: StackHost + ?Sized>(
    host: &H,
    stack: Option<&H::Stack>,
) -> Option<Option<ItemStackSpec>> {
    match stack {
        None => Some(None),
        Some(stack) => spec_of_stack(host, stack).map(Some),
    }
}

/// The specs of a list of stacks; `None` when one of them has no id.
pub fn stack_list_in<H: StackHost + ?Sized>(
    host: &H,
    stacks: &[H::Stack],
) -> Option<Vec<ItemStackSpec>> {
    stacks
        .iter()
        .map(|stack| spec_of_stack(host, stack))
        .collect()
}

fn spec_of_stack<H: StackHost + ?Sized>(host: &H, stack: &H::Stack) -> Option<ItemStackSpec> {
    let nbt = host.stack_nbt(stack)?;
    let id = match tag_of(&nbt, "id")? {
        Tag::String(id) => id.to_string_lossy(),
        _ => return None,
    };
    let count = match tag_of(&nbt, "count")? {
        Tag::Int(count) => u32::try_from(*count).ok()?,
        Tag::Long(count) => u32::try_from(*count).ok()?,
        _ => return None,
    };
    let nbt = match tag_of(&nbt, "components") {
        None => None,
        Some(Tag::Compound(patch)) => Some(patch.clone()),
        Some(_) => return None,
    };
    Some(ItemStackSpec { id, count, nbt })
}

/// The stack a mod wrote, as the game holds it. A count of 0 is the empty stack: `Ok(None)`. An
/// unknown id, a bad patch, or a count above the item's maximum is an error (the maximum is the
/// host's, or [`MAX_COUNT`] when the host gives none).
pub fn stack_from_spec<H: StackHost + ?Sized>(
    host: &H,
    spec: &ItemStackSpec,
) -> Result<Option<H::Stack>, String> {
    if spec.count == 0 {
        return Ok(None);
    }
    let mut nbt = Compound::new();
    nbt.insert("id".into(), Tag::String(spec.id.as_str().into()));
    nbt.insert("count".into(), Tag::Int(spec.count as i32));
    if let Some(patch) = &spec.nbt {
        nbt.insert("components".into(), Tag::Compound(patch.clone()));
    }
    let stack = host
        .stack_from_nbt(&nbt)
        .ok_or("an unknown item id, or a component patch the item refuses")?;
    let max = host.stack_max(&stack).unwrap_or(MAX_COUNT);
    if spec.count > max {
        return Err(format!("count {} is above the maximum {max}", spec.count));
    }
    Ok(Some(stack))
}

/// A stack a mod wrote to a stack or an `Option<ItemStack>` output. A count of 0 is refused: it is
/// not a stack. An optional output answers None with `horizon.none`, never with a count of 0.
pub fn stack_value<H: StackHost + ?Sized>(host: &H, value: &Value) -> Result<H::Stack, String> {
    stack_from_spec(host, &read_stack(value)?)?.ok_or_else(|| {
        "count 0 is not a stack: an empty answer is horizon.none on an optional output".to_owned()
    })
}

/// A list of stacks a mod wrote: each entry must be a stack, so a count of 0 refuses the list.
pub fn stack_list_value<H: StackHost + ?Sized>(
    host: &H,
    value: &Value,
) -> Result<Vec<H::Stack>, String> {
    let specs = Vec::<ItemStackSpec>::from_lua_value(value).map_err(|err| err.to_string())?;
    specs
        .iter()
        .map(|spec| {
            stack_from_spec(host, spec)?.ok_or_else(|| "a list holds no count 0 entries".to_owned())
        })
        .collect()
}

/// The most stacks one `on_sheared` answer may drop, and the most items in all of them: the drops
/// are spawned by the game in the world, so a mod's list is bounded before any stack is made.
pub const MAX_DROP_STACKS: usize = 16;
/// See [`MAX_DROP_STACKS`]: the most items one `on_sheared` answer may drop in all.
pub const MAX_DROP_ITEMS: u32 = 256;

/// The drops a mod's `on_sheared` answered: at most [`MAX_DROP_STACKS`] stacks, each a stack by
/// [`stack_list_value`]'s rules, and at most [`MAX_DROP_ITEMS`] items in all. A list over either
/// bound is refused whole, before any stack is converted past it.
pub fn drops_value<H: StackHost + ?Sized>(
    host: &H,
    value: &Value,
) -> Result<Vec<H::Stack>, String> {
    if let Value::Table(table) = value
        && table.raw_len() > MAX_DROP_STACKS
    {
        return Err(format!(
            "a drop list holds at most {MAX_DROP_STACKS} stacks"
        ));
    }
    let specs = Vec::<ItemStackSpec>::from_lua_value(value).map_err(|err| err.to_string())?;
    let items: u64 = specs.iter().map(|spec| u64::from(spec.count)).sum();
    if items > u64::from(MAX_DROP_ITEMS) {
        return Err(format!("a drop list holds at most {MAX_DROP_ITEMS} items"));
    }
    specs
        .iter()
        .map(|spec| {
            stack_from_spec(host, spec)?.ok_or_else(|| "a list holds no count 0 entries".to_owned())
        })
        .collect()
}

/// The spec of an optional block state for a Lua table, with the same three outcomes as
/// [`stack_in`].
pub fn state_in<H: StateHost + ?Sized>(
    host: &H,
    state: Option<&H::State>,
) -> Option<Option<BlockStateSpec>> {
    match state {
        None => Some(None),
        Some(state) => spec_of_state(host, state).map(Some),
    }
}

fn spec_of_state<H: StateHost + ?Sized>(host: &H, state: &H::State) -> Option<BlockStateSpec> {
    spec_of_text(&host.state_text(state)?)
}

/// The most text a state may be written as, and the most properties it may carry: a mod-written
/// state never reaches either bound, so a longer one is not a state (bounded work).
const MAX_STATE_TEXT: usize = 1024;
const MAX_STATE_PROPS: usize = 32;

/// A `/setblock`-style state text as a spec: `namespace:id`, then `[name=value,...]` when it has
/// properties. Refused: text over [`MAX_STATE_TEXT`] bytes, more than [`MAX_STATE_PROPS`]
/// properties, an empty name or value, a repeated name, a pair with no `=`, and anything after the
/// closing bracket. Which names and values exist is the host's to say.
fn spec_of_text(text: &str) -> Option<BlockStateSpec> {
    if text.len() > MAX_STATE_TEXT {
        return None;
    }
    let (id, props) = match text.split_once('[') {
        None => (text, ""),
        Some((id, rest)) => (id, rest.strip_suffix(']')?),
    };
    if id.is_empty() {
        return None;
    }
    let mut constraints: Vec<StateConstraint> = Vec::new();
    if text.contains('[') {
        if props.is_empty() {
            return None;
        }
        for pair in props.split(',') {
            let (name, value) = pair.split_once('=')?;
            if name.is_empty() || value.is_empty() || value.contains('=') {
                return None;
            }
            if constraints.iter().any(|known| known.name == name) {
                return None;
            }
            if constraints.len() == MAX_STATE_PROPS {
                return None;
            }
            constraints.push(StateConstraint::new(name, value));
        }
    }
    Some(BlockStateSpec {
        id: id.to_owned(),
        props: constraints,
    })
}

/// The `/setblock` text of a spec: its id, then its properties in name order when it has any.
pub fn state_text(spec: &BlockStateSpec) -> String {
    if spec.props.is_empty() {
        return spec.id.clone();
    }
    let props: Vec<String> = spec
        .props
        .iter()
        .map(|constraint| format!("{}={}", constraint.name, constraint.value))
        .collect();
    format!("{}[{}]", spec.id, props.join(","))
}

/// The block state a mod wrote. The spec's names and values are already checked as `[a-z0-9_]`,
/// so the text cannot carry block NBT or a second block, and a bad one never reaches the host.
pub fn state_from_spec<H: StateHost + ?Sized>(
    host: &H,
    spec: &BlockStateSpec,
) -> Result<H::State, String> {
    host.state_parse(&state_text(spec))
        .ok_or_else(|| "an unknown block, property or value".to_owned())
}

/// A block state a mod wrote, for a state slot.
pub fn state_value<H: StateHost + ?Sized>(host: &H, value: &Value) -> Result<H::State, String> {
    state_from_spec(host, &read_state(value)?)
}

/// Logs that a hook is not delivered to Lua because a stack or state in it has no id. Logged once
/// per process for each hook, however often it fires; the caller gives no answer.
pub fn undelivered(hook: &str) {
    warn_once(
        "host",
        &format!(
            "{hook}: not delivered to Lua: a stack or block state in it has no id in this game"
        ),
    );
}

fn read_stack(value: &Value) -> Result<ItemStackSpec, String> {
    ItemStackSpec::from_lua_value(value).map_err(|err| err.to_string())
}

fn read_state(value: &Value) -> Result<BlockStateSpec, String> {
    BlockStateSpec::from_lua_value(value).map_err(|err| err.to_string())
}

/// The value under `key` in an NBT compound, by its text.
fn tag_of<'a>(nbt: &'a Compound, key: &str) -> Option<&'a Tag> {
    nbt.iter()
        .find(|(name, _)| name.to_string_lossy() == key)
        .map(|(_, tag)| tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_horizon_lua_convert::scalar::ToLuaValue;
    use std::cell::Cell;

    /// A stack in the double: its id, count and component patch.
    #[derive(Clone, Debug, PartialEq)]
    struct Fake {
        id: String,
        count: u32,
        patch: Option<Compound>,
    }

    /// A host double: knows two items with maxima, and counts its calls.
    struct Items {
        calls: Cell<usize>,
    }

    const MAX: [(&str, u32); 2] = [("minecraft:apple", 64), ("minecraft:egg", 16)];

    impl StackHost for Items {
        type Stack = Fake;

        fn stack_nbt(&self, stack: &Fake) -> Option<Compound> {
            self.calls.set(self.calls.get() + 1);
            let mut nbt = Compound::new();
            nbt.insert("id".into(), Tag::String(stack.id.as_str().into()));
            nbt.insert("count".into(), Tag::Int(stack.count as i32));
            if let Some(patch) = &stack.patch {
                nbt.insert("components".into(), Tag::Compound(patch.clone()));
            }
            Some(nbt)
        }

        fn stack_from_nbt(&self, nbt: &Compound) -> Option<Fake> {
            self.calls.set(self.calls.get() + 1);
            let id = match tag_of(nbt, "id")? {
                Tag::String(id) => id.to_string_lossy(),
                _ => return None,
            };
            if !MAX.iter().any(|(known, _)| *known == id) {
                return None;
            }
            let count = match tag_of(nbt, "count")? {
                Tag::Int(count) => u32::try_from(*count).ok()?,
                _ => return None,
            };
            if count == 0 {
                return None;
            }
            let patch = match tag_of(nbt, "components") {
                None => None,
                Some(Tag::Compound(patch)) => Some(patch.clone()),
                Some(_) => return None,
            };
            Some(Fake { id, count, patch })
        }

        fn stack_max(&self, stack: &Fake) -> Option<u32> {
            MAX.iter()
                .find(|(known, _)| *known == stack.id)
                .map(|(_, max)| *max)
        }
    }

    /// A state double: states are their text, and `state_parse` knows two of them.
    struct States {
        calls: Cell<usize>,
    }

    impl StateHost for States {
        type State = String;

        fn state_text(&self, state: &String) -> Option<String> {
            Some(state.clone())
        }

        fn state_parse(&self, text: &str) -> Option<String> {
            self.calls.set(self.calls.get() + 1);
            matches!(
                text,
                "minecraft:oak_stairs[facing=east]" | "minecraft:stone"
            )
            .then(|| text.to_owned())
        }
    }

    fn apple(count: u32) -> Fake {
        Fake {
            id: "minecraft:apple".into(),
            count,
            patch: None,
        }
    }

    fn lua_value(lua: &mlua::Lua, source: &str) -> Value {
        lua.load(source).eval().expect("the Lua chunk evaluates")
    }

    #[test]
    fn a_stack_goes_to_a_table_with_its_id_count_and_nbt() {
        let host = Items {
            calls: Cell::new(0),
        };
        let mut patch = Compound::new();
        patch.insert("minecraft:custom_name".into(), Tag::String("x".into()));
        let stack = Fake {
            id: "minecraft:apple".into(),
            count: 3,
            patch: Some(patch),
        };
        let spec = stack_in(&host, Some(&stack))
            .expect("the id is known")
            .expect("a stack");
        assert_eq!(spec.id, "minecraft:apple");
        assert_eq!(spec.count, 3);
        assert_eq!(spec.nbt.as_ref().map(|nbt| nbt.len()), Some(1));

        let lua = mlua::Lua::new();
        let table = spec.to_lua_value(&lua).expect("the table builds");
        let Value::Table(table) = table else {
            panic!("a table")
        };
        assert_eq!(table.get::<String>("id").unwrap(), "minecraft:apple");
        assert_eq!(table.get::<u32>("count").unwrap(), 3);
        assert!(table.get::<Value>("nbt").unwrap().is_table());
    }

    #[test]
    fn a_stack_round_trips_through_a_table_and_the_host() {
        let host = Items {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        let spec = stack_in(&host, Some(&apple(5))).unwrap().unwrap();
        let table = spec.to_lua_value(&lua).unwrap();
        let back = stack_value(&host, &table).unwrap();
        assert_eq!(back, apple(5));
    }

    #[test]
    fn a_stack_with_no_id_is_not_delivered() {
        let host = Items {
            calls: Cell::new(0),
        };
        let missing = Fake {
            id: String::new(),
            count: 1,
            patch: None,
        };
        // The double's id is always present, so the check is on the host's answer.
        struct NoId;
        impl StackHost for NoId {
            type Stack = Fake;
            fn stack_nbt(&self, _: &Fake) -> Option<Compound> {
                None
            }
            fn stack_from_nbt(&self, _: &Compound) -> Option<Fake> {
                None
            }
            fn stack_max(&self, _: &Fake) -> Option<u32> {
                None
            }
        }
        assert_eq!(stack_in(&NoId, Some(&missing)), None);
        assert_eq!(stack_in(&NoId, None), Some(None));
        assert_eq!(host.calls.get(), 0);
    }

    #[test]
    fn an_unknown_id_and_a_count_0_are_refused() {
        let host = Items {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        let unknown = lua_value(&lua, r#"return { id = "minecraft:nothing", count = 1 }"#);
        assert!(stack_value(&host, &unknown).is_err());
        let empty = lua_value(&lua, r#"return { id = "minecraft:apple", count = 0 }"#);
        let error = stack_value(&host, &empty).expect_err("count 0 is not a stack");
        assert!(error.contains("horizon.none"), "{error}");
    }

    #[test]
    fn an_optional_stack_output_is_cleared_only_by_horizon_none() {
        let host = Items {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        let slot = std::cell::RefCell::new(Some(apple(3)));
        let empty = lua_value(&lua, r#"return { id = "minecraft:apple", count = 0 }"#);
        let refused = crate::dispatch::accept_opt_by(&slot, "m", "h", "o", &empty, |value| {
            stack_value(&host, value)
        });
        assert!(refused);
        assert_eq!(slot.borrow().clone(), Some(apple(3)));
        let none = Value::UserData(lua.create_userdata(crate::dispatch::NoneAnswer).unwrap());
        let refused = crate::dispatch::accept_opt_by(&slot, "m", "h", "o", &none, |value| {
            stack_value(&host, value)
        });
        assert!(!refused);
        assert_eq!(slot.into_inner(), None);
    }

    #[test]
    fn a_count_above_the_item_maximum_is_refused() {
        let host = Items {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        let egg = lua_value(&lua, r#"return { id = "minecraft:egg", count = 17 }"#);
        let error = stack_value(&host, &egg).expect_err("17 eggs is over the maximum");
        assert!(error.contains("above the maximum 16"), "{error}");
        let apples = lua_value(&lua, r#"return { id = "minecraft:apple", count = 64 }"#);
        assert_eq!(stack_value(&host, &apples), Ok(apple(64)));
        let hundred = lua_value(&lua, r#"return { id = "minecraft:apple", count = 100 }"#);
        assert!(
            stack_value(&host, &hundred).is_err(),
            "100 is above the Lua limit"
        );
    }

    #[test]
    fn a_bad_patch_is_refused() {
        let host = Items {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        let spec = ItemStackSpec {
            id: "minecraft:apple".into(),
            count: 1,
            nbt: None,
        };
        assert!(stack_from_spec(&host, &spec).unwrap().is_some());
        let bad = lua_value(
            &lua,
            r#"return { id = "minecraft:apple", count = 1, nbt = "not a table" }"#,
        );
        assert!(stack_value(&host, &bad).is_err());
    }

    #[test]
    fn a_list_with_an_empty_entry_is_refused_whole() {
        let host = Items {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        let good = lua_value(
            &lua,
            r#"return { { id = "minecraft:apple", count = 2 }, { id = "minecraft:egg", count = 1 } }"#,
        );
        assert_eq!(stack_list_value(&host, &good).map(|v| v.len()), Ok(2));
        let with_empty = lua_value(
            &lua,
            r#"return { { id = "minecraft:apple", count = 2 }, { id = "minecraft:egg", count = 0 } }"#,
        );
        assert!(stack_list_value(&host, &with_empty).is_err());
    }

    #[test]
    fn a_block_state_goes_to_a_table_with_its_id_and_props() {
        let host = States {
            calls: Cell::new(0),
        };
        let state = "minecraft:oak_stairs[facing=east]".to_owned();
        let spec = state_in(&host, Some(&state)).unwrap().unwrap();
        assert_eq!(spec.id, "minecraft:oak_stairs");
        assert_eq!(spec.props.len(), 1);
        assert_eq!(spec.props[0].name, "facing");
        assert_eq!(spec.props[0].value, "east");
    }

    #[test]
    fn a_block_state_written_by_a_mod_is_parsed_and_checked() {
        let host = States {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        let good = lua_value(
            &lua,
            r#"return { id = "minecraft:oak_stairs", props = { facing = "east" } }"#,
        );
        assert_eq!(
            state_value(&host, &good),
            Ok("minecraft:oak_stairs[facing=east]".to_owned())
        );
        let unknown = lua_value(
            &lua,
            r#"return { id = "minecraft:stone", props = { x = "y" } }"#,
        );
        assert!(state_value(&host, &unknown).is_err());
        assert_eq!(host.calls.get(), 2);
    }

    #[test]
    fn a_prop_with_a_bracket_or_brace_is_refused_before_the_host_is_called() {
        let host = States {
            calls: Cell::new(0),
        };
        let lua = mlua::Lua::new();
        for value in ["a]b", "{x}", "A"] {
            let table = lua.create_table().unwrap();
            table.set("id", "minecraft:stone").unwrap();
            let props = lua.create_table().unwrap();
            props.set("facing", value).unwrap();
            table.set("props", props).unwrap();
            assert!(state_value(&host, &Value::Table(table)).is_err(), "{value}");
        }
        assert_eq!(host.calls.get(), 0, "the host was called");
    }

    #[test]
    fn a_global_stack_writes_the_host_nbt_shape() {
        // Hand-written: id, count, then the patch with the damage override put back in it.
        let mut patch = Compound::new();
        patch.insert("minecraft:custom_name".into(), Tag::String("x".into()));
        let nbt = stack_nbt_of("minecraft:apple", 3, Some(5), Some(&patch));
        assert_eq!(
            tag_of(&nbt, "id"),
            Some(&Tag::String("minecraft:apple".into()))
        );
        assert_eq!(tag_of(&nbt, "count"), Some(&Tag::Int(3)));
        let Some(Tag::Compound(components)) = tag_of(&nbt, "components") else {
            panic!("components")
        };
        assert_eq!(tag_of(components, "minecraft:damage"), Some(&Tag::Int(5)));
        assert_eq!(
            tag_of(components, "minecraft:custom_name"),
            Some(&Tag::String("x".into()))
        );
        // No damage and no patch: no components key at all.
        let plain = stack_nbt_of("minecraft:apple", 1, None, None);
        assert_eq!(tag_of(&plain, "components"), None);
        assert_eq!(tag_of(&plain, "count"), Some(&Tag::Int(1)));
    }

    #[test]
    fn global_counts_read_and_write_like_the_host() {
        assert_eq!(count_tag(64), Tag::Int(64));
        assert_eq!(count_tag(3_000_000_000), Tag::Long(3_000_000_000));
        assert_eq!(nbt_count(&Tag::Byte(3)), Some(3));
        assert_eq!(nbt_count(&Tag::Short(7)), Some(7));
        assert_eq!(nbt_count(&Tag::Long(-1)), None);
        assert_eq!(nbt_count(&Tag::String("3".into())), None);
    }

    #[test]
    fn without_installed_tables_global_conversions_give_none() {
        // The tables are installed at world entry; no test installs them, so every lookup is None.
        assert!(
            GlobalHost
                .stack_from_nbt(&stack_nbt_of("minecraft:apple", 1, None, None))
                .is_none()
        );
        assert!(GlobalHost.state_parse("minecraft:stone").is_none());
        assert!(
            GlobalHost
                .state_parse("minecraft:oak_stairs[facing=east]")
                .is_none()
        );
        let lua = mlua::Lua::new();
        let table = lua_value(&lua, r#"return { id = "minecraft:apple", count = 1 }"#);
        assert!(stack_value(&GlobalHost, &table).is_err());
        let state = lua_value(&lua, r#"return { id = "minecraft:stone" }"#);
        assert!(state_value(&GlobalHost, &state).is_err());
    }

    #[test]
    fn an_output_list_with_a_bad_stack_is_refused_whole_and_kept() {
        // A stack list output with one stack the tables cannot give is refused, and the value kept.
        let lua = mlua::Lua::new();
        // The slot holds no stacks yet; the refusal must leave it that way.
        let slot = std::cell::RefCell::new(Vec::<ItemStack>::new());
        let list = lua_value(
            &lua,
            r#"return { { id = "minecraft:apple", count = 2 }, { id = "minecraft:nothing", count = 1 } }"#,
        );
        let refused = crate::dispatch::accept_by(&slot, "m", "h", "items", &list, |value| {
            stack_list_value(&GlobalHost, value)
        });
        assert!(refused);
        assert!(slot.borrow().is_empty());
    }

    /// A state text of `count` properties, `a0=x`, `a1=x`, ... (names need not exist for the parser).
    fn text_with_props(count: usize) -> String {
        let props: Vec<String> = (0..count).map(|i| format!("a{i}=x")).collect();
        format!("minecraft:stone[{}]", props.join(","))
    }

    #[test]
    fn a_state_text_is_split_into_its_id_and_properties_within_the_bounds() {
        // Hand-written expectations: the id, then each pair in the order written.
        let spec = spec_of_text("minecraft:probe_slab[facing=east,half=top]").unwrap();
        assert_eq!(spec.id, "minecraft:probe_slab");
        let pairs: Vec<(&str, &str)> = spec
            .props
            .iter()
            .map(|p| (p.name.as_str(), p.value.as_str()))
            .collect();
        assert_eq!(pairs, [("facing", "east"), ("half", "top")]);
        assert_eq!(spec_of_text("minecraft:stone").unwrap().props.len(), 0);
        // 32 properties are the most; 33 are refused.
        assert_eq!(
            spec_of_text(&text_with_props(MAX_STATE_PROPS))
                .unwrap()
                .props
                .len(),
            MAX_STATE_PROPS
        );
        assert!(spec_of_text(&text_with_props(MAX_STATE_PROPS + 1)).is_none());
        // Text over 1024 bytes is refused, and so is a 2 KiB one.
        let long = format!("minecraft:stone[a={}]", "x".repeat(MAX_STATE_TEXT));
        assert!(spec_of_text(&long).is_none());
        let two_kib = format!("minecraft:stone[a={}]", "x".repeat(2048));
        assert!(spec_of_text(&two_kib).is_none());
    }

    #[test]
    fn a_malformed_state_text_is_refused_before_any_table_is_read() {
        for text in [
            "",
            "[facing=east]",
            "minecraft:stone[]",
            "minecraft:stone[=east]",
            "minecraft:stone[facing=]",
            "minecraft:stone[facing]",
            "minecraft:stone[facing=east=north]",
            "minecraft:stone[facing=east,facing=north]",
            "minecraft:stone[facing=east]x",
            "minecraft:stone[facing=east",
        ] {
            assert!(spec_of_text(text).is_none(), "{text}");
        }
    }

    #[test]
    fn global_state_text_and_parse_follow_the_installed_tables() {
        crate::block_state_tests::registry();
        let stone = mod_api::__private::block_key("minecraft:stone").unwrap();
        let no_props =
            BlockState::of(mod_api::__private::block_definitions().unwrap(), stone, &[]).unwrap();
        // A block with no properties is its id alone.
        assert_eq!(
            GlobalHost.state_text(&no_props),
            Some("minecraft:stone".to_owned())
        );
        assert_eq!(GlobalHost.state_parse("minecraft:stone"), Some(no_props));
        // Two properties come out in name order, as the host path writes them.
        let slab = mod_api::__private::block_key("minecraft:probe_slab").unwrap();
        let top = BlockState::of(
            mod_api::__private::block_definitions().unwrap(),
            slab,
            &[
                StateConstraint::new("half", "top"),
                StateConstraint::new("facing", "east"),
            ],
        )
        .unwrap();
        assert_eq!(
            GlobalHost.state_text(&top),
            Some("minecraft:probe_slab[facing=east,half=top]".to_owned())
        );
        assert_eq!(
            GlobalHost.state_parse("minecraft:probe_slab[half=top,facing=east]"),
            Some(top),
            "the order a mod writes does not matter"
        );
        // The refusals: unknown block, unknown property, bad value, repeated key, a bad bound.
        for text in [
            "minecraft:no_such_block",
            "minecraft:probe_slab[color=red]",
            "minecraft:probe_slab[facing=up]",
            "minecraft:probe_slab[half=top,half=top]",
            "minecraft:probe_slab[facing=east,half=top]x",
            "minecraft:stone[facing=east]",
            &text_with_props(MAX_STATE_PROPS + 1),
        ] {
            assert_eq!(GlobalHost.state_parse(text), None, "{text}");
        }
    }

    #[test]
    fn a_spec_table_holds_only_text() {
        let lua = mlua::Lua::new();
        let spec = BlockStateSpec {
            id: "minecraft:stone".into(),
            props: vec![StateConstraint::new("facing", "east")],
        };
        let Value::Table(table) = spec.to_lua_value(&lua).unwrap() else {
            panic!("a table")
        };
        assert!(matches!(
            table.get::<Value>("id").unwrap(),
            Value::String(_)
        ));
        let Value::Table(props) = table.get::<Value>("props").unwrap() else {
            panic!("a props table")
        };
        assert_eq!(props.get::<String>("facing").unwrap(), "east");
    }

    #[test]
    fn an_undelivered_hook_is_logged_once_however_often_it_fires() {
        crate::host::tests::install_capture();
        let hook = "stacks_undelivered_once_hook";
        for _ in 0..5 {
            undelivered(hook);
        }
        assert_eq!(crate::host::tests::captured_with_prefix(hook).len(), 1);
    }
}
