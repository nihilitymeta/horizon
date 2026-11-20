//! Attachments a Lua mod keeps on entities, players and block entities (issue 74, C1, C2 and A1):
//! `horizon.attachment_get`, `horizon.attachment_set` and `horizon.attachment_remove`. The values are saved world data that
//! untrusted Lua output writes, so every limit refuses the one call that breaks it, with the reason,
//! and nothing is cut short or saved in part. A refused call changes nothing.
//!
//! A block entity holder (A1) takes the same caps, with the same `#index`. It has no sync and no
//! copy_on_death: both are refused by name, since a block entity needs chunk-level sync and does not
//! respawn. Its handle must be the current one of the server's level (a removed, replaced or unloaded
//! block entity raises, as the Part state of a block entity does). A client turn cannot read it: the
//! server does not send block entity attachments to clients. A process-wide guard counts the
//! block-entity keys each mod creates ([`MAX_BLOCK_ENTITY_NEW_KEYS`]); it is a runaway guard, not an
//! accounting, and resets when the process starts.
//!
//! C2 adds two policies, both off by default, given as `horizon.attachment_set(h, name, value, opts)`
//! with `opts = { sync = bool, copy_on_death = bool }`: `sync` sends the value to the clients that see
//! the holder (the store queues the send on a write), `copy_on_death` copies it to a respawned player.
//! A synced or copied value is at most [`MAX_FLAGGED_VALUE_BYTES`]; a mod has at most
//! [`MAX_SYNC_KEYS`] synced and [`MAX_COPY_KEYS`] copied keys per holder; copy_on_death needs a player
//! holder (an entity does not respawn). A client turn reads a value the server synced
//! (`attachment_get` through the client's copy); it never sets or removes one. The mod's `#index` is
//! never synced or copied.
//!
//! A value is kept through the host's attachment store (`ModStateHost::attachments`), the one native
//! and wasm mods reach, under the mod's own namespace: a name `<mod>:<path>` is the store's key
//! `(namespace = <mod>, name = <path>)`. The store cannot list a holder's keys, so the mod's keys and
//! their encoded sizes are kept in one more attachment of the same namespace, `#index`. `#` is outside
//! the game's id charset, so no Lua name can reach it. Every write of a value and of the index goes
//! through the same two calls, so the index and the stored values agree.
//!
//! Costs: get 1 unit of the turn's budget, set and remove 10. Get, set and remove need the server host
//! of a server turn: a client turn or a turn with no host raises `not available in this hook`. A
//! holder must be resident in the level the server host is in; a stale handle raises.
//!
//! What a write does when the turn later fails: a host call is not part of the turn's outcome. A
//! handler that errors after a set keeps the value, as the host keeps any write it made.

use std::sync::Arc;

use event_horizon_lua_convert::scalar::{FromLuaValue, ToLuaValue};
use mlua::{AnyUserData, Error, Lua, MultiValue, Table, UserData, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

use mod_api::{
    AttachmentError, AttachmentForm, AttachmentPolicy, AttachmentStore, AttachmentValue,
    AttachmentWrite, BlockEntityRef, Compound, EntityRef, EntityResidency, Holder, Host,
    PartStateError, PartStateHolder, PlayerRef, Tag,
};

use crate::declare::check_path;
use crate::handles::{Args, refusal_text, server_of};
use crate::host::warn_once;
use crate::hostcall::{TurnHost, Void, host_call};

/// The longest name, `<mod>:<path>`, in bytes.
pub const MAX_NAME_BYTES: usize = 256;
/// The largest encoded value, in bytes, the NBT of its entries without the root tag's header.
pub const MAX_VALUE_BYTES: usize = 64 * 1024;
/// The keys one mod may keep on one holder.
pub const MAX_KEYS: usize = 16;
/// The encoded bytes of all of one mod's values on one holder.
pub const MAX_HOLDER_BYTES: usize = 256 * 1024;
/// The largest encoded value of a synced or copied attachment, in bytes (`MAX_VALUE_BYTES` otherwise).
pub const MAX_FLAGGED_VALUE_BYTES: usize = 4 * 1024;
/// The synced keys one mod may keep on one holder.
pub const MAX_SYNC_KEYS: usize = 4;
/// The copied-on-death keys one mod may keep on one holder.
pub const MAX_COPY_KEYS: usize = 4;
/// The block-entity keys one mod may create by new-key sets in one process lifetime. A runaway guard,
/// not an accounting: an overwrite never counts, a removal does not give a key back, and the count
/// starts at zero each time the process starts.
#[cfg(not(test))]
pub const MAX_BLOCK_ENTITY_NEW_KEYS: usize = 4096;
/// The same guard with a small limit, so the test that reaches it stays fast.
#[cfg(test)]
pub const MAX_BLOCK_ENTITY_NEW_KEYS: usize = 16;
/// The synced writes one mod may make, or try, in one turn. The host has no server tick counter, so the
/// cap is per turn (see `hostcall::Void`): a mod that runs many turns in a tick is limited per turn only.
pub const MAX_SYNC_WRITES: u32 = 16;
/// The store key of the mod's own index: the keys it holds on a holder and their sizes. Outside the
/// game's id charset, so no name a Lua mod gives reaches it.
const INDEX: &str = "#index";
pub(crate) const GET_OP: &str = "attachment_get";
pub(crate) const SET_OP: &str = "attachment_set";
pub(crate) const REMOVE_OP: &str = "attachment_remove";
const GET_COST: u32 = 1;
const WRITE_COST: u32 = 10;

/// The userdata a `horizon.attachment_*` function passes to the turn's host: the mod's id, the
/// namespace every name must have.
pub(crate) struct Owner(String);

impl UserData for Owner {}

/// Installs `horizon.attachment_get`, `horizon.attachment_set` and `horizon.attachment_remove` for
/// the mod `owner`. Each is a host call of the turn, so it is answered only while a turn is open.
pub(crate) fn install(lua: &Lua, horizon: &Table, owner: &str) -> mlua::Result<()> {
    let this = lua.create_userdata(Owner(owner.to_owned()))?;
    for (name, op) in [
        ("attachment_get", GET_OP),
        ("attachment_set", SET_OP),
        ("attachment_remove", REMOVE_OP),
    ] {
        let this = this.clone();
        let function = lua
            .create_function(move |lua, args: MultiValue| host_call(lua, op, this.clone(), args))?;
        horizon.raw_set(name, function)?;
    }
    Ok(())
}

/// What an attachment operation costs, or `None` when it is not one.
pub(crate) fn cost(op: &str) -> Option<u32> {
    match op {
        GET_OP => Some(GET_COST),
        SET_OP | REMOVE_OP => Some(WRITE_COST),
        _ => None,
    }
}

/// Runs an attachment operation, of a `cost` that is `Some`, against the turn's host. `turn` counts the
/// synced writes of the turn.
pub(crate) fn run(
    lua: &Lua,
    host: &mut TurnHost<'_>,
    op: &str,
    this: &AnyUserData,
    args: MultiValue,
    turn: &Void,
) -> mlua::Result<Value> {
    let owner = this.borrow::<Owner>()?.0.clone();
    let (method, count) = match op {
        GET_OP => ("horizon.attachment_get", 2),
        SET_OP => ("horizon.attachment_set", 4),
        REMOVE_OP => ("horizon.attachment_remove", 2),
        _ => return Err(Error::runtime(format!("unknown host call {op}"))),
    };
    let args = Args::new(method, args, count)?;
    let target = target_arg(&args, method)?;
    let name: String = args.get(1)?;
    let key = key_of(&owner, &name).map_err(|why| refuse(&owner, method, &why))?;
    let fail = |why: String| refuse(&owner, method, &why);
    if op == GET_OP {
        // A client turn reads its copy of what the server synced; a server turn reads its own store.
        let found = match host {
            TurnHost::Client(_) if matches!(target, Target::BlockEntity(_)) => {
                return Err(refuse(
                    &owner,
                    method,
                    "a client turn cannot read a block entity's attachments: the server does not send them",
                ));
            }
            TurnHost::Client(client) => {
                let holder = holder_of(&**client, target).map_err(fail)?;
                read(client.attachments(), holder, &owner, key).map_err(fail)?
            }
            _ => {
                let server = server_of(host)?;
                let holder = holder_of(&*server, target).map_err(fail)?;
                read(server.attachments(), holder, &owner, key).map_err(fail)?
            }
        };
        return match found {
            Some(found) => found.to_lua_value(lua),
            None => Ok(Value::Nil),
        };
    }
    let write = if op == SET_OP {
        let compound = compound_of(&args.value(2)).map_err(|why| refuse(&owner, method, &why))?;
        let len = payload_len(&compound).map_err(|why| refuse(&owner, method, &why))?;
        let flags = flags_of(&args.value(3)).map_err(|why| refuse(&owner, method, &why))?;
        let limit = if flags.sync || flags.copy_on_death {
            MAX_FLAGGED_VALUE_BYTES
        } else {
            MAX_VALUE_BYTES
        };
        if len > limit {
            let kind = if flags.sync {
                "synced"
            } else if flags.copy_on_death {
                "copied"
            } else {
                ""
            };
            let why = if kind.is_empty() {
                format!("the value is {len} bytes, more than {limit}")
            } else {
                format!("the {kind} value is {len} bytes, more than {limit}")
            };
            return Err(refuse(&owner, method, &why));
        }
        Some((compound, len, flags))
    } else {
        None
    };
    let server = server_of(host)?;
    let holder = holder_of(&*server, target).map_err(fail)?;
    let asked = write
        .as_ref()
        .map_or(Flags::default(), |(_, _, flags)| *flags);
    holder_allows(target, asked).map_err(fail)?;
    let store = server.attachments();
    match write {
        Some((compound, len, flags)) => {
            if flags.sync && !turn.take_synced_write() {
                let why = format!(
                    "a mod may make {MAX_SYNC_WRITES} synced writes in one turn, and this turn has made them"
                );
                return Err(refuse(&owner, method, &why));
            }
            set(store, holder, &owner, key, &compound, len, flags).map_err(fail)?;
            Ok(Value::Boolean(true))
        }
        None => Ok(Value::Boolean(
            remove(store, holder, &owner, key).map_err(fail)?,
        )),
    }
}

/// The policies a set asks for. Both are false unless the options say otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Flags {
    pub(crate) sync: bool,
    pub(crate) copy_on_death: bool,
}

/// The options table of a set: nil for none, else `{ sync = bool, copy_on_death = bool }`. An unknown
/// key is refused by name, and so is a value that is not a boolean.
fn flags_of(value: &Value) -> Result<Flags, String> {
    let table = match value {
        Value::Nil => return Ok(Flags::default()),
        Value::Table(table) => table,
        other => {
            return Err(format!(
                "the options must be a table, not {}",
                other.type_name()
            ));
        }
    };
    let mut flags = Flags::default();
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|err| format!("the options are refused: {err}"))?;
        let name = match &key {
            Value::String(name) => name
                .to_str()
                .map_err(|err| format!("the options are refused: {err}"))?
                .to_owned(),
            other => {
                return Err(format!(
                    "the options key must be a name, not {}",
                    other.type_name()
                ));
            }
        };
        match name.as_str() {
            "sync" => flags.sync = flag_of(&value, &name)?,
            "copy_on_death" => flags.copy_on_death = flag_of(&value, &name)?,
            _ => {
                return Err(format!(
                    "the option '{name}' is unknown; the options are sync and copy_on_death"
                ));
            }
        }
    }
    Ok(flags)
}

fn flag_of(value: &Value, name: &str) -> Result<bool, String> {
    match value {
        Value::Boolean(flag) => Ok(*flag),
        other => Err(format!(
            "the option '{name}' must be true or false, not {}",
            other.type_name()
        )),
    }
}

/// The holder kinds a policy may name: sync for entities and players, copy_on_death for players only.
/// An entity does not respawn, so copy_on_death refuses for one. A block entity takes neither: its
/// clients need chunk-level sync, which is not in this version, and it does not respawn.
fn holder_allows(target: Target, flags: Flags) -> Result<(), String> {
    match target {
        Target::BlockEntity(_) if flags.sync => Err(
            "sync does not apply to block entities: their clients need chunk-level sync, not in this version"
                .to_owned(),
        ),
        Target::BlockEntity(_) if flags.copy_on_death => Err(
            "copy_on_death does not apply to block entities: a block entity does not respawn".to_owned(),
        ),
        Target::Entity(_) if flags.copy_on_death => {
            Err("copy_on_death needs a player holder: an entity does not respawn".to_owned())
        }
        Target::Entity(_) | Target::Player(_) | Target::BlockEntity(_) => Ok(()),
    }
}

/// Logs a refused call through `warn_once` (a mod that loops on refusals cannot flood the log) and
/// returns the Lua error the mod sees: the method and the reason, clipped like other refusals.
fn refuse(owner: &str, method: &str, why: &str) -> Error {
    let reason = refusal_text(why);
    warn_once(owner, &format!("{owner}: {method} refused: {reason}"));
    Error::runtime(format!("{method}: {reason}"))
}

/// The holder a call names: an entity, a player or a block entity handle.
#[derive(Clone, Copy)]
enum Target {
    Entity(EntityRef),
    Player(PlayerRef),
    BlockEntity(BlockEntityRef),
}

fn target_arg(args: &Args, method: &str) -> mlua::Result<Target> {
    if let Ok(player) = args.handle::<PlayerRef>(0) {
        return Ok(Target::Player(player));
    }
    if let Ok(entity) = args.handle::<EntityRef>(0) {
        return Ok(Target::Entity(entity));
    }
    args.handle::<BlockEntityRef>(0)
        .map(Target::BlockEntity)
        .map_err(|_| {
            Error::runtime(format!(
                "{method}: argument 1 must be an EntityRef, PlayerRef or BlockEntityRef"
            ))
        })
}

/// The store's holder for a handle, when the handle names a resident holder of the server's level.
/// A player's attachments are kept under its UUID, on its entity's residency.
fn holder_of(host: &dyn Host, target: Target) -> Result<Holder, String> {
    let (entity, holder) = match target {
        Target::Entity(entity) => (entity, Holder::Entity(entity.id())),
        Target::Player(player) => (player.entity(), Holder::Player(player.uuid())),
        // The Part state's own check: the handle's level is the host's, and its identity is current.
        Target::BlockEntity(block) => {
            return block.part_state_holder(host).map_err(|why| match why {
                PartStateError::OtherLevel => "the holder is in another level".to_owned(),
                _ => "the holder is gone, or its handle is stale".to_owned(),
            });
        }
    };
    match host.entity_residency(entity) {
        EntityResidency::InPlace => Ok(holder),
        EntityResidency::Busy => Err("the holder is busy in this tick".to_owned()),
        EntityResidency::OtherLevel => Err("the holder is in another level".to_owned()),
        EntityResidency::Gone => Err("the holder is gone, or its handle is stale".to_owned()),
        _ => Err("the holder is not resident".to_owned()),
    }
}

/// The store key a name maps to: its path, when the name is `<owner>:<path>` in the game's charset
/// and at most [`MAX_NAME_BYTES`] bytes. Any other namespace, `minecraft:` included, is refused.
fn key_of<'a>(owner: &str, name: &'a str) -> Result<&'a str, String> {
    if name.len() > MAX_NAME_BYTES {
        return Err(format!(
            "the name is {} bytes, more than {MAX_NAME_BYTES}",
            name.len()
        ));
    }
    let Some((namespace, path)) = name.split_once(':') else {
        return Err(format!("needs a namespaced name such as {owner}:name"));
    };
    if namespace != owner {
        return Err(format!("is outside this mod's namespace '{owner}'"));
    }
    check_path(name, path)?;
    Ok(path)
}

/// The value a set stores: a table the converter reads (its depth, entries and strings bounded).
fn compound_of(value: &Value) -> Result<Compound, String> {
    if !matches!(value, Value::Table(_)) {
        return Err(format!(
            "the value must be a table (a compound), not {}",
            value.type_name()
        ));
    }
    Compound::from_lua_value(value).map_err(|err| format!("the value is refused: {err}"))
}

/// The bytes of a compound's entries, as the save writes them: each entry's tag, name and payload,
/// and the end tag. The root tag's own header is not counted, so the number is the value's size.
/// Errors on a string over the NBT limit of 65535 bytes, or a list of mixed kinds, which the save
/// could not write.
fn payload_len(map: &Compound) -> Result<usize, String> {
    let mut total = 1;
    for (name, tag) in map {
        total += 3 + text_len(&name.0)? + tag_len(tag)?;
    }
    Ok(total)
}

/// The bytes of one tag's payload.
fn tag_len(tag: &Tag) -> Result<usize, String> {
    Ok(match tag {
        Tag::End => 0,
        Tag::Byte(_) => 1,
        Tag::Short(_) => 2,
        Tag::Int(_) | Tag::Float(_) => 4,
        Tag::Long(_) | Tag::Double(_) => 8,
        Tag::ByteArray(values) => 4 + values.len(),
        Tag::String(text) => 2 + text_len(&text.0)?,
        Tag::List(items) => {
            let kind = items.first().map_or(0, |first| first.id());
            if items.iter().any(|item| item.id() != kind) {
                return Err("a list mixes its element kinds".to_owned());
            }
            let mut total = 5;
            for item in items {
                total += tag_len(item)?;
            }
            total
        }
        Tag::Compound(map) => payload_len(map)?,
        Tag::IntArray(values) => 4 + 4 * values.len(),
        Tag::LongArray(values) => 4 + 8 * values.len(),
    })
}

/// The bytes of a string in NBT's modified UTF-8, which Java writes: NUL takes two bytes, the other
/// units under U+0080 one, under U+0800 two, and the rest three (surrogates each three).
fn text_len(units: &[u16]) -> Result<usize, String> {
    let len: usize = units
        .iter()
        .map(|unit| match unit {
            0 => 2,
            1..=0x7F => 1,
            0x80..=0x7FF => 2,
            _ => 3,
        })
        .sum();
    if len > usize::from(u16::MAX) {
        return Err("a string is longer than 65535 bytes, the NBT limit".to_owned());
    }
    Ok(len)
}

/// The value under `key` on `holder`, saved or decoded, or none.
fn read(
    store: &mut dyn AttachmentStore,
    holder: Holder,
    owner: &str,
    key: &str,
) -> Result<Option<Compound>, String> {
    if let Some(value) = store.decoded(holder, owner, key).map_err(store_error)? {
        return match (*value).downcast_ref::<Compound>() {
            Some(compound) => Ok(Some(compound.clone())),
            None => Err(store_error(AttachmentError::TypeMismatch)),
        };
    }
    store.saved(holder, owner, key).map_err(store_error)
}

/// Keeps `value` under `key` on `holder`, as a value a mod set: the saved form it has is the compound
/// itself, under the store's usual envelope, with the policies the set asked for.
fn keep(
    store: &mut dyn AttachmentStore,
    holder: Holder,
    owner: &str,
    key: &str,
    value: &Compound,
    flags: Flags,
) -> Result<(), String> {
    let shared: AttachmentValue = Arc::new(value.clone());
    let form = AttachmentForm {
        encoded: Some(value.clone()),
        policy: AttachmentPolicy {
            copy_on_death: flags.copy_on_death,
            sync: flags.sync,
        },
        write: AttachmentWrite::Set,
    };
    store
        .store(holder, owner, key, shared, form)
        .map_err(store_error)
}

/// The mod's index on `holder`: each key it holds, with the key's encoded size and policies.
fn index_of(
    store: &mut dyn AttachmentStore,
    holder: Holder,
    owner: &str,
) -> Result<Compound, String> {
    Ok(read(store, holder, owner, INDEX)?.unwrap_or_default())
}

/// Writes the index, or drops it when the mod holds no key on the holder. The index is never synced
/// or copied: it is stored with no policy.
fn write_index(
    store: &mut dyn AttachmentStore,
    holder: Holder,
    owner: &str,
    index: &Compound,
) -> Result<(), String> {
    if index.is_empty() {
        return store
            .discard(holder, owner, INDEX, false)
            .map_err(store_error);
    }
    keep(store, holder, owner, INDEX, index, Flags::default())
}

/// Whether a compound key is `key`: the same UTF-16 units, so no lossy comparison.
fn is_key(stored: &[u16], key: &str) -> bool {
    stored.iter().copied().eq(key.encode_utf16())
}

/// One index entry as the mod's keys are indexed: the encoded size and the policies. A plain key is an
/// int, the size alone (the format C1 wrote); a key with a policy is a compound with the size and the
/// flags that are set.
fn entry_tag(size: usize, flags: Flags) -> Tag {
    let size = i32::try_from(size).unwrap_or(i32::MAX);
    if flags == Flags::default() {
        return Tag::Int(size);
    }
    let mut entry = Compound::new();
    entry.insert("size".into(), Tag::Int(size));
    if flags.sync {
        entry.insert("sync".into(), Tag::Byte(1));
    }
    if flags.copy_on_death {
        entry.insert("copy_on_death".into(), Tag::Byte(1));
    }
    Tag::Compound(entry)
}

/// The size and policies of an index entry, read from either form of [`entry_tag`].
fn entry_of(tag: &Tag) -> (usize, Flags) {
    match tag {
        Tag::Int(size) => (usize::try_from(*size).unwrap_or(0), Flags::default()),
        Tag::Compound(entry) => {
            let size = match entry.get(&"size".into()) {
                Some(Tag::Int(size)) => usize::try_from(*size).unwrap_or(0),
                _ => 0,
            };
            let flag = |name: &str| matches!(entry.get(&name.into()), Some(Tag::Byte(1)));
            (
                size,
                Flags {
                    sync: flag("sync"),
                    copy_on_death: flag("copy_on_death"),
                },
            )
        }
        _ => (0, Flags::default()),
    }
}

/// The index entry of `key`, when the mod holds it on the holder.
fn entry_at(index: &Compound, key: &str) -> Option<(usize, Flags)> {
    index
        .iter()
        .find(|(stored, _)| is_key(&stored.0, key))
        .map(|(_, tag)| entry_of(tag))
}

/// How many of the mod's other keys on the holder have the policy `pick` names.
fn others_with(index: &Compound, key: &str, pick: fn(Flags) -> bool) -> usize {
    index
        .iter()
        .filter(|(stored, tag)| !is_key(&stored.0, key) && pick(entry_of(tag).1))
        .count()
}

/// The encoded bytes the index counts for all of a mod's keys on one holder.
fn total_of(index: &Compound) -> usize {
    index.values().map(|tag| entry_of(tag).0).sum()
}

/// Stores `value` under `key` with `flags`, after checking the key count and the holder's total.
/// Nothing is written when a limit refuses. A value written before an index write that fails is put
/// back as it was, so the index and the stored values keep agreeing. Turning sync off queues the
/// clients' removal of the value, in the store.
fn set(
    store: &mut dyn AttachmentStore,
    holder: Holder,
    owner: &str,
    key: &str,
    value: &Compound,
    len: usize,
    flags: Flags,
) -> Result<(), String> {
    let mut index = index_of(store, holder, owner)?;
    let before = entry_at(&index, key);
    let (before_len, before_flags) = before.unwrap_or((0, Flags::default()));
    if before.is_none() && index.len() >= MAX_KEYS {
        return Err(format!(
            "a holder keeps at most {MAX_KEYS} attachments of one mod, and this holder has {MAX_KEYS}"
        ));
    }
    let block_entity = matches!(holder, Holder::BlockEntity(_));
    if block_entity && before.is_none() && new_block_entity_keys(owner) >= MAX_BLOCK_ENTITY_NEW_KEYS
    {
        return Err(format!(
            "a mod may create at most {MAX_BLOCK_ENTITY_NEW_KEYS} block-entity attachment keys in one run of the game, and it has created them"
        ));
    }
    if flags.sync && !before_flags.sync && others_with(&index, key, |f| f.sync) >= MAX_SYNC_KEYS {
        return Err(format!(
            "a holder keeps at most {MAX_SYNC_KEYS} synced attachments of one mod, and this holder has {MAX_SYNC_KEYS}"
        ));
    }
    if flags.copy_on_death
        && !before_flags.copy_on_death
        && others_with(&index, key, |f| f.copy_on_death) >= MAX_COPY_KEYS
    {
        return Err(format!(
            "a holder keeps at most {MAX_COPY_KEYS} copied-on-death attachments of one mod, and this holder has {MAX_COPY_KEYS}"
        ));
    }
    let total = total_of(&index).saturating_sub(before_len) + len;
    if total > MAX_HOLDER_BYTES {
        return Err(format!(
            "the holder's values would be {total} bytes, more than {MAX_HOLDER_BYTES}"
        ));
    }
    let previous = read(store, holder, owner, key)?;
    keep(store, holder, owner, key, value, flags)?;
    index.retain(|stored, _| !is_key(&stored.0, key));
    index.insert(key.into(), entry_tag(len, flags));
    if let Err(why) = write_index(store, holder, owner, &index) {
        // The holder refused the index: put the value back as it was. The rollback is the same
        // store on the same holder, so if it refuses too the holder has gone and the value with it.
        let _ = match previous {
            Some(earlier) => keep(store, holder, owner, key, &earlier, before_flags),
            None => store
                .discard(holder, owner, key, flags.sync)
                .map_err(store_error),
        };
        return Err(why);
    }
    if block_entity && before.is_none() {
        count_block_entity_key(owner);
    }
    Ok(())
}

/// The block-entity keys each mod has created by new-key sets in this process, by mod id. A runaway
/// guard only: nothing is saved, so it starts at zero when the process starts.
static BLOCK_ENTITY_NEW_KEYS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

/// How many block-entity keys `owner` has created in this process.
fn new_block_entity_keys(owner: &str) -> usize {
    let counts = BLOCK_ENTITY_NEW_KEYS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    counts.get(owner).copied().unwrap_or(0)
}

/// Counts one block-entity key `owner` has created.
fn count_block_entity_key(owner: &str) {
    let mut counts = BLOCK_ENTITY_NEW_KEYS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(owner.to_owned()).or_insert(0) += 1;
}

/// Removes `key` from `holder`: true when a value was there, false when not. The index drops the key.
/// A synced key's removal is queued in the store, so the clients drop their copy.
fn remove(
    store: &mut dyn AttachmentStore,
    holder: Holder,
    owner: &str,
    key: &str,
) -> Result<bool, String> {
    if !store.holds(holder, owner, key).map_err(store_error)? {
        return Ok(false);
    }
    let mut index = index_of(store, holder, owner)?;
    let flags = entry_at(&index, key).map_or(Flags::default(), |(_, flags)| flags);
    store
        .discard(holder, owner, key, flags.sync)
        .map_err(store_error)?;
    index.retain(|stored, _| !is_key(&stored.0, key));
    write_index(store, holder, owner, &index)?;
    Ok(true)
}

/// The store's refusal as the text a mod sees, clipped like the other handle refusals.
fn store_error(err: AttachmentError) -> String {
    format!("the attachment store refused it: {err:?}")
}
