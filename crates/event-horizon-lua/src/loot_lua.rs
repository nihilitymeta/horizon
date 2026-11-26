//! The `loot_drops` hook's context as a Lua table (`event.context`): what dropped the items. The
//! view is borrowed and call-scoped, so it is converted field by field. Its block state and its
//! held tool are stacks and states, which convert through the turn's host (`stacks`), so a host
//! that has no id for one of them does not deliver the hook (see [`carried`]).

use event_horizon_lua_convert::scalar::ToLuaValue;
use mlua::{Lua, Table, Value};
use mod_api::{BlockState, Host, LootDrops};

use crate::stacks;

/// `Some` when every block state and stack in `drops` has an id in `host`. A hook whose context
/// has one without an id is not delivered (the generated pre-check).
pub(crate) fn carried(host: &dyn Host, drops: &LootDrops<'_>) -> Option<()> {
    if let Some((_, state)) = drops.block {
        stacks::state_in(host, Some(&state))?;
    }
    stacks::stack_in(host, drops.tool)?;
    Some(())
}

/// The context as Lua: `table`, `dimension`, `origin` (`{x=, y=, z=}`, numbers),
/// `block` (`{pos, state}` or nil), `entity` (`{id, type}` or nil), `player` (a UUID string or nil),
/// `damage_source` (the `HurtSource` table or nil), `tool` (a stack table or nil), `explosion`
/// (a number or nil) and `used` (a boolean).
pub(crate) fn loot_drops_lua(
    lua: &Lua,
    drops: &LootDrops<'_>,
    host: &dyn Host,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set("table", drops.table.to_string().to_lua_value(lua)?)?;
    table.raw_set("dimension", drops.dimension.to_string().to_lua_value(lua)?)?;
    table.raw_set("origin", drops.origin.to_lua_value(lua)?)?;
    table.raw_set("block", block_lua(lua, drops.block, host)?)?;
    table.raw_set("entity", entity_lua(lua, drops.entity)?)?;
    table.raw_set("player", drops.player.to_lua_value(lua)?)?;
    table.raw_set("damage_source", drops.damage_source.to_lua_value(lua)?)?;
    table.raw_set("tool", tool_lua(lua, drops.tool, host)?)?;
    table.raw_set("explosion", drops.explosion.to_lua_value(lua)?)?;
    table.raw_set("used", drops.used)?;
    Ok(table)
}

fn block_lua(
    lua: &Lua,
    block: Option<([i32; 3], BlockState)>,
    host: &dyn Host,
) -> mlua::Result<Value> {
    let Some((pos, state)) = block else {
        return Ok(Value::Nil);
    };
    let table = lua.create_table()?;
    table.raw_set("pos", pos.to_lua_value(lua)?)?;
    let spec = stacks::state_in(host, Some(&state))
        .ok_or_else(|| mlua::Error::runtime("a carried value has no id"))?;
    table.raw_set("state", spec.to_lua_value(lua)?)?;
    Ok(Value::Table(table))
}

fn entity_lua(lua: &Lua, entity: Option<(u32, &str)>) -> mlua::Result<Value> {
    let Some((id, kind)) = entity else {
        return Ok(Value::Nil);
    };
    let table = lua.create_table()?;
    table.raw_set("id", id)?;
    table.raw_set("type", kind.to_string().to_lua_value(lua)?)?;
    Ok(Value::Table(table))
}

fn tool_lua(lua: &Lua, tool: Option<&mod_api::ItemStack>, host: &dyn Host) -> mlua::Result<Value> {
    let spec = stacks::stack_in(host, tool)
        .ok_or_else(|| mlua::Error::runtime("a carried value has no id"))?;
    spec.to_lua_value(lua)
}
