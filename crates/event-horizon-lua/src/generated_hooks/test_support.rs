//! Shared test doubles for the generated-hook smoke tests: the mod loader,
//! the status-row assertion, a game host and a level. Used by the generated
//! model tests (`mod.rs`) and the hand-specified spike tests
//! (`spike_tests.rs`) alike.

use crate::lua_mods::LuaMod;
use horizon_client::dev_input::mods::{ModStatus, set_status, status};
use horizon_client::mod_api::{
    AttachmentError, Attachments, BlockStateId, Compound, EntityRef, Holder, Hooks, Host,
    LegacyRandom, LevelHost, LevelRandomHost, NativeServerPlayer, NativeWorldEpoch,
};
use horizon_mods::Package;
use mlua::Lua;
use std::path::PathBuf;

pub const MANIFEST_TEXT: &str = "id = \"ruby\"\nname = \"Ruby\"\nversion = \"0.1.0\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"scripts\"]\nexperimental = [\"scripts\"]\n";

pub fn hmod(name: &str, lua: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("horizon-lua-gen-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ruby.hmod");
    std::fs::write(
        &path,
        Package::write_files(MANIFEST_TEXT, None, &[("scripts/init.lua", lua.as_bytes())]).unwrap(),
    )
    .unwrap();
    (dir, path)
}

/// Load a mod running `lua`, with a clean status row: `no_error` then tells
/// a handler's `assert` from silence.
pub fn loaded(name: &str, lua: &str) -> (LuaMod, String, PathBuf) {
    let (dir, path) = hmod(name, lua);
    let m = LuaMod::load(&path, &[]).unwrap();
    let id = m.id().to_owned();
    set_status(ModStatus {
        id: id.clone(),
        ..Default::default()
    });
    (m, id, dir)
}

pub fn boxed(m: LuaMod) -> Vec<Box<dyn Hooks>> {
    vec![Box::new(m) as Box<dyn Hooks>]
}

/// Load a mod and keep its Lua state beside it: the generated (and spike)
/// tests read the `__witness` counter their handlers bump, proving the
/// handler ran. An `assert` that never fires leaves no error row, so
/// `no_error` alone cannot tell silence from success.
pub fn loaded_tracked(name: &str, lua: &str) -> (LuaMod, String, PathBuf, Lua) {
    let (m, id, dir) = loaded(name, lua);
    let state = m.lua.clone();
    (m, id, dir, state)
}

/// Handler firings since load: the generated handlers start with
/// `__witness = (__witness or 0) + 1`, so 1 means the handler ran for this
/// firing and 0 means it stayed quiet.
pub fn witness(lua: &Lua) -> i64 {
    match lua.globals().get::<mlua::Value>("__witness") {
        Ok(mlua::Value::Integer(i)) => i,
        _ => 0,
    }
}

pub fn drop_dir(dir: PathBuf) {
    std::fs::remove_dir_all(&dir).unwrap();
}

/// No `error` row for `id`: every `assert` in its handlers held.
pub fn no_error(id: &str) {
    assert!(
        status()
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.error)
            .is_none(),
        "a handler failed: {:?}",
        status()
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.error)
    );
}

// A game host with no players on it: `player_by_uuid` answers the uuid and
// the name the hook told, as for a player out of reach (the engine's
// `NativeServerPlayer` is `#[non_exhaustive]`, so a test outside its crate
// cannot build the full table). Block states name themselves by id, except
// id 7, which is stone (the spike's sample). Entity 11 is a zombie, 13 a
// skeleton, anything else resolves to nothing.
pub struct FakeHost;
thread_local! {
    static ENTITY_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
/// Entity lookups (`Host::entity`, `Host::entity_kind`) the fake host answered on this thread since
/// `reset_entity_reads`: a hook nobody listens to must not make any.
pub fn entity_reads() -> usize {
    ENTITY_READS.with(|reads| reads.get())
}
pub fn reset_entity_reads() {
    ENTITY_READS.with(|reads| reads.set(0));
}
fn count_entity_read() {
    ENTITY_READS.with(|reads| reads.set(reads.get() + 1));
}
impl FakeHost {
    pub fn empty() -> Self {
        Self
    }
}
impl Host for FakeHost {
    fn level_players(&self) -> Vec<NativeServerPlayer> {
        Vec::new()
    }

    fn state_name(&self, state: BlockStateId) -> String {
        stone_or_state(state)
    }

    fn entity(&self, id: u32) -> Option<EntityRef> {
        count_entity_read();
        Some(EntityRef::new(NativeWorldEpoch::new(0), id))
    }

    fn entity_kind(&self, entity: EntityRef) -> Option<String> {
        count_entity_read();
        match entity.id() {
            11 => Some("minecraft:zombie".to_owned()),
            13 => Some("minecraft:skeleton".to_owned()),
            _ => None,
        }
    }
}

/// Id 7 is stone (the spike's sample); every other id names itself, so the
/// smoke tests tell two states apart.
fn stone_or_state(state: BlockStateId) -> String {
    if state == BlockStateId(7) {
        "minecraft:stone".to_owned()
    } else {
        format!("test:state{}", state.0)
    }
}

/// A level of states named by id (7 is stone, as above).
pub struct FakeLevel;
impl LevelHost for FakeLevel {
    fn block_state(&self, _: [i32; 3]) -> BlockStateId {
        BlockStateId(7)
    }
    fn is_client_side(&self) -> bool {
        false
    }
    fn state_with_property(&self, _: BlockStateId, _: &str, _: &str) -> Option<BlockStateId> {
        None
    }
    fn mod_block_entity_data(&self, _: [i32; 3]) -> Option<Compound> {
        None
    }
    fn state_block(&self, state: BlockStateId) -> Option<String> {
        Some(stone_or_state(state))
    }
    fn state_properties(&self, _: BlockStateId) -> Vec<(String, String)> {
        Vec::new()
    }
}
impl LevelRandomHost for FakeLevel {
    fn attachments(&mut self, _: Holder) -> Result<&mut Attachments, AttachmentError> {
        Err(AttachmentError::MissingHolder)
    }
    fn random(&mut self) -> Option<&mut LegacyRandom> {
        None
    }
}
