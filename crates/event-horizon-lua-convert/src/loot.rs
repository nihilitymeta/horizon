//! The loot context's leaf values in Lua, read-only: an item key is its id string, and an item
//! stack is its id, count and component patch (the same table as [`ItemStackSpec`], with the
//! damage in the patch as the host writes it). Both read the process-global tables, since the
//! context's conversion has no host to ask; a key or a stack the tables do not name is an error.

use mlua::{Lua, Value};
use mod_api::{Compound, ItemKey, ItemStack, Tag};

use crate::item::ItemStackSpec;
use crate::scalar::ToLuaValue;

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
