//! The raw NBT merge under the `nbt` power (write powers v2, C5): `EntityRef::merge_nbt` and
//! `BlockEntityRef::merge_data`. The power check and the conversion are generated (`generated/powers.rs`,
//! `generated/handles.rs`); this module adds the two rules the generator cannot know:
//!
//! - a player is never the target of a raw merge: `merge_nbt` on a player is refused with
//!   `horizon: merge_nbt refused: players cannot be changed with raw NBT` (a player's private data is
//!   `horizon.attachment_*`, not NBT);
//! - the top-level keys `UUID`, `id`, `Pos`, `Passengers`, `Inventory`, `EnderItems` and `horizon:attachments` are
//!   refused, not stripped, when the table carries any of them: `horizon: <method> refused: key <name>
//!   cannot be merged`. The match is case-sensitive and looks at top-level keys only.
//!
//! A merge adds and changes keys; it cannot remove one. An empty table therefore changes nothing: a
//! merge of `{}` does not clear a tag (the NBT's own empty-compound behaviour, documented in
//! `docs/lua-modding.md`). A non-table argument is left to the converter, which refuses it.
//!
//! Runs after the power check and before the host is called; a refusal costs one unit, as a guard
//! refusal does.

use mlua::{AnyUserData, Error, MultiValue, Value};
use mod_api::EntityRef;

use crate::handles::{read_of, receiver};
use crate::hostcall::TurnHost;

/// The top-level keys a raw merge may not carry. `horizon:attachments` is the reserved compound that holds
/// every mod's attachments on an entity or block entity (`ATTACHMENTS_KEY` in `horizon-mod-runtime`); a raw
/// merge could replace another mod's values or the store's index, so it is refused.
pub(crate) const PROTECTED_KEYS: &[&str] = &[
    "UUID",
    "id",
    "Pos",
    "Passengers",
    "Inventory",
    "EnderItems",
    "horizon:attachments",
];

/// Keys a block entity's data carries for its position: a merge that moved them would save the block
/// entity at the wrong place, so a block entity merge refuses them as well.
const BLOCK_ENTITY_POSITION_KEYS: &[&str] = &["x", "y", "z"];

const ENTITY_MERGE: &str = "EntityRef::merge_nbt";
const BLOCK_ENTITY_MERGE: &str = "BlockEntityRef::merge_data";

/// Refuses a raw merge the rules forbid, before the host sees it. `Ok` for every other operation.
pub(crate) fn check(
    op: &str,
    this: &AnyUserData,
    args: &MultiValue,
    host: &TurnHost<'_>,
) -> mlua::Result<()> {
    if op != ENTITY_MERGE && op != BLOCK_ENTITY_MERGE {
        return Ok(());
    }
    let method = op.rsplit("::").next().unwrap_or(op);
    if op == ENTITY_MERGE {
        let entity = receiver::<EntityRef>(this, op)?;
        if is_player(entity, host)? {
            return Err(Error::runtime(format!(
                "horizon: {method} refused: players cannot be changed with raw NBT"
            )));
        }
    }
    if let Some(Value::Table(table)) = args.front() {
        for pair in table.pairs::<Value, Value>() {
            let (key, _) = pair.map_err(|_| Error::runtime("cannot read the NBT table"))?;
            if let Value::String(text) = key {
                let name = text.to_str()?;
                if PROTECTED_KEYS.contains(&&*name)
                    || (op == BLOCK_ENTITY_MERGE && BLOCK_ENTITY_POSITION_KEYS.contains(&&*name))
                {
                    return Err(Error::runtime(format!(
                        "horizon: {method} refused: key {name} cannot be merged"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Whether an entity is a player. A stale handle has no kind, so it is not a player here: the host
/// refuses it. When the entity-type registry is not loaded, a kind cannot be compared with the player's,
/// so the merge is refused rather than allowed.
fn is_player(entity: EntityRef, host: &TurnHost<'_>) -> mlua::Result<bool> {
    let Some(kind) = entity.kind(read_of(host)?) else {
        return Ok(false);
    };
    match mod_api::__private::resolved_entity_type_key("minecraft:player") {
        Some(player) => Ok(kind == player),
        None => Err(Error::runtime(
            "horizon: merge_nbt refused: the entity types are not loaded, so the target cannot be checked",
        )),
    }
}
