//! The tag reads a Lua mod makes in a turn: `horizon.tag_contains(kind, tag_id, entry_id)` (is the
//! entry in the tag) and `horizon.tag_entries(kind, tag_id)` (the tag's members, in the game's order).
//!
//! They read the merged item, block and fluid tags through the turn's tag service. A server or
//! client hook reads it through its host; a loot condition and a loot function read the one their
//! event carries (`TurnHost::Tags`), since those hooks have no host. A hook with neither raises
//! `not available in this hook`. Ids are checked before the game sees them, and a malformed one is
//! refused by name without echoing it.

use mlua::{Error, Function, Lua, MultiValue, UserData, Value};
use mod_api::__private::{block_key, block_key_id, fluid_id, fluid_key, item_key, item_key_id};
use mod_api::{Identifier, Tags};

use crate::hostcall::{NOT_HERE, TurnHost, host_call};

/// The host call a `horizon.tag_contains` read makes.
pub(crate) const CONTAINS_OP: &str = "tag_contains";
/// The host call a `horizon.tag_entries` read makes.
pub(crate) const ENTRIES_OP: &str = "tag_entries";
/// What one tag read spends of the turn's budget.
pub(crate) const COST: u32 = 1;
/// The longest tag or entry id, in bytes.
pub const MAX_ID_BYTES: usize = 256;
/// The most entries one `horizon.tag_entries` call returns.
pub const MAX_ENTRIES: usize = 1024;

/// The userdata a tag read passes to the turn's host. It holds nothing.
pub(crate) struct TagReader;

impl UserData for TagReader {}

/// Whether `op` is one of the tag reads.
pub(crate) fn is_op(op: &str) -> bool {
    op == CONTAINS_OP || op == ENTRIES_OP
}

/// What a tag read costs, or `None` for another operation.
pub(crate) fn cost(op: &str) -> Option<u32> {
    is_op(op).then_some(COST)
}

/// The `horizon.tag_contains` function of a mod's runtime table: a host call of the turn.
pub(crate) fn contains_function(lua: &Lua) -> mlua::Result<Function> {
    let reader = lua.create_userdata(TagReader)?;
    lua.create_function(move |lua, args: MultiValue| {
        host_call(lua, CONTAINS_OP, reader.clone(), args)
    })
}

/// The `horizon.tag_entries` function of a mod's runtime table: a host call of the turn.
pub(crate) fn entries_function(lua: &Lua) -> mlua::Result<Function> {
    let reader = lua.create_userdata(TagReader)?;
    lua.create_function(move |lua, args: MultiValue| {
        host_call(lua, ENTRIES_OP, reader.clone(), args)
    })
}

/// Runs a tag read in its turn. `op` is one of [`is_op`]'s.
pub(crate) fn run(
    lua: &Lua,
    host: &TurnHost<'_>,
    op: &str,
    args: MultiValue,
) -> mlua::Result<Value> {
    if op == CONTAINS_OP {
        contains(host, args)
    } else {
        entries(lua, host, args)
    }
}

/// The kinds of tag the game's tag service reads.
#[derive(Clone, Copy)]
enum Kind {
    Item,
    Block,
    Fluid,
}

fn kind_of(name: &str, call: &str) -> mlua::Result<Kind> {
    match name {
        "item" => Ok(Kind::Item),
        "block" => Ok(Kind::Block),
        "fluid" => Ok(Kind::Fluid),
        _ => Err(refused(call, "kind must be item, block or fluid")),
    }
}

/// A refusal of one call, with the call's name and a reason that never echoes the input.
fn refused(call: &str, reason: &str) -> Error {
    Error::runtime(format!("horizon.{call}: {reason}"))
}

/// The arguments of a call: exactly `N` strings.
fn strings<const N: usize>(call: &str, args: MultiValue) -> mlua::Result<[String; N]> {
    let values: Vec<Value> = args.into_iter().collect();
    if values.len() != N {
        return Err(refused(call, &format!("needs {N} strings")));
    }
    let mut out: [String; N] = std::array::from_fn(|_| String::new());
    for (slot, value) in out.iter_mut().zip(values) {
        let Value::String(text) = value else {
            return Err(refused(call, &format!("needs {N} strings")));
        };
        *slot = text.to_str()?.to_string();
    }
    Ok(out)
}

/// A tag id: at most [`MAX_ID_BYTES`] bytes, and a well-formed `namespace:path`.
fn tag_id(call: &str, text: &str) -> mlua::Result<Identifier> {
    if text.len() > MAX_ID_BYTES {
        return Err(refused(call, "the tag id is longer than 256 bytes"));
    }
    Identifier::parse(text).map_err(|_| refused(call, "the tag id is malformed"))
}

/// An entry id: at most [`MAX_ID_BYTES`] bytes, and a well-formed `namespace:path`.
fn entry_id(call: &str, text: &str) -> mlua::Result<()> {
    if text.len() > MAX_ID_BYTES {
        return Err(refused(call, "the entry id is longer than 256 bytes"));
    }
    Identifier::parse(text)
        .map(|_| ())
        .map_err(|_| refused(call, "the entry id is malformed"))
}

/// The tag service of a turn: the host's, or the one a loot hook's event carries.
fn service<'t>(host: &'t TurnHost<'_>, call: &str) -> mlua::Result<&'t dyn Tags> {
    let tags: Option<&'t dyn Tags> = match host {
        TurnHost::Tags(tags) => Some(*tags),
        other => other.read().map(|read| read.tags()),
    };
    tags.ok_or_else(|| refused(call, NOT_HERE))
}

fn unloaded(call: &str) -> Error {
    refused(call, "the tags are not loaded yet")
}

/// `horizon.tag_contains(kind, tag_id, entry_id)`: whether the entry is in the tag. An entry the game
/// does not know is not in any tag.
fn contains(host: &TurnHost<'_>, args: MultiValue) -> mlua::Result<Value> {
    const CALL: &str = "tag_contains";
    let [kind, tag, entry] = strings::<3>(CALL, args)?;
    let kind = kind_of(&kind, CALL)?;
    let tag = tag_id(CALL, &tag)?;
    entry_id(CALL, &entry)?;
    let tags = service(host, CALL)?;
    let Some(key) = tags.key(&tag) else {
        return Ok(Value::Boolean(false));
    };
    let found = match kind {
        Kind::Item => match item_key(&entry) {
            Some(item) => tags.is_in_tag(item, key),
            None => Some(false),
        },
        Kind::Block => match block_key(&entry) {
            Some(block) => tags.blocks(key).map(|members| members.contains(&block)),
            None => Some(false),
        },
        Kind::Fluid => match fluid_key(&entry) {
            Some(fluid) => tags.fluids(key).map(|members| members.contains(&fluid)),
            None => Some(false),
        },
    };
    found.map(Value::Boolean).ok_or_else(|| unloaded(CALL))
}

/// `horizon.tag_entries(kind, tag_id)`: the tag's members in the game's order, as ids. A tag with
/// more than [`MAX_ENTRIES`] members raises rather than returning part of it.
fn entries(lua: &Lua, host: &TurnHost<'_>, args: MultiValue) -> mlua::Result<Value> {
    const CALL: &str = "tag_entries";
    let [kind, tag] = strings::<2>(CALL, args)?;
    let kind = kind_of(&kind, CALL)?;
    let tag = tag_id(CALL, &tag)?;
    let tags = service(host, CALL)?;
    let Some(key) = tags.key(&tag) else {
        return Ok(Value::Table(lua.create_table()?));
    };
    let ids: Vec<String> = match kind {
        Kind::Item => tags
            .items(key)
            .ok_or_else(|| unloaded(CALL))?
            .into_iter()
            .filter_map(item_key_id)
            .map(str::to_owned)
            .collect(),
        Kind::Block => tags
            .blocks(key)
            .ok_or_else(|| unloaded(CALL))?
            .into_iter()
            .filter_map(block_key_id)
            .map(str::to_owned)
            .collect(),
        Kind::Fluid => tags
            .fluids(key)
            .ok_or_else(|| unloaded(CALL))?
            .into_iter()
            .filter_map(fluid_id)
            .map(str::to_owned)
            .collect(),
    };
    if ids.len() > MAX_ENTRIES {
        return Err(refused(CALL, "the tag has more than 1024 entries"));
    }
    Ok(Value::Table(lua.create_sequence_from(ids)?))
}
