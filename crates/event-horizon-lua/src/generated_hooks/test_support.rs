//! Shared test doubles for the generated-hook smoke tests: the mod loader,
//! the status-row assertion, a game host and a level. Used by the generated
//! model tests (`mod.rs`) and the hand-specified spike tests
//! (`spike_tests.rs`) alike.

use crate::lua_mods::LuaMod;
use horizon_client::dev_input::mods::{ModStatus, set_status, status};
use horizon_client::mod_api::{
    AttachmentError, Attachments, BlockStateId, Compound, EntityRef, Holder, Hooks, Host,
    LegacyRandom, LevelHost, LevelRandomHost, NativeServerPlayer, NativeWorldEpoch, Text,
};
use horizon_mods::Package;
use mlua::Lua;
use std::path::PathBuf;

/// One mod per name, so duo tests loading two mods get two status rows:
/// the manifest id is the test's name, and every status lookup keys on it.
fn manifest_text(name: &str) -> String {
    format!(
        "id = \"{name}\"\nname = \"Ruby\"\nversion = \"0.1.0\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"scripts\"]\nexperimental = [\"scripts\"]\n"
    )
}

pub fn hmod(name: &str, lua: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("horizon-lua-gen-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ruby.hmod");
    std::fs::write(
        &path,
        Package::write_files(
            &manifest_text(name),
            None,
            &[("scripts/init.lua", lua.as_bytes())],
        )
        .unwrap(),
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

/// Event tables the generated dispatch arms built on this thread since
/// `reset_event_tables`: an unheard hook must build none, a heard one at
/// least one. Unlike the host/level counters (which only move when a seed
/// reads through them), this also pins hooks whose seeds read nothing.
pub fn event_tables() -> usize {
    crate::generated_runtime::event_table_count()
}

pub fn reset_event_tables() {
    crate::generated_runtime::reset_event_tables();
}

pub fn drop_dir(dir: PathBuf) {
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An `error` row for `id`: the handler failed loudly, as a duo test's
/// erroring mod must (its failure must not hide the other mod's result).
pub fn has_error(id: &str) {
    assert!(
        status()
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.error)
            .is_some(),
        "expected an error row for {id}"
    );
}

/// Two mods in load order, for the duo tests: the native dispatch hears
/// both, in order.
pub fn duo_boxed(a: LuaMod, b: LuaMod) -> Vec<Box<dyn Hooks>> {
    vec![Box::new(a) as Box<dyn Hooks>, Box::new(b) as Box<dyn Hooks>]
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
pub struct FakeHost {
    /// Command answers (`host.feedback`): the command tests read these
    /// back. Uncounted: the command cost tests pin the heard dispatch at
    /// zero host reads.
    feedback: std::sync::Mutex<Vec<String>>,
    /// The acting player's held item (`host.held`): `None` is an empty
    /// hand, `Some` what one held test seeds. Uncounted, like `feedback`.
    held: Option<(String, u32, bool)>,
}
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
    count_host_read();
}

thread_local! {
    static HOST_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LEVEL_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
/// Every `Host` call the fake host answered since `reset_host_reads`:
/// an unregistered hook must make none.
pub fn host_reads() -> usize {
    HOST_READS.with(|reads| reads.get())
}
pub fn reset_host_reads() {
    HOST_READS.with(|reads| reads.set(0));
}
fn count_host_read() {
    HOST_READS.with(|reads| reads.set(reads.get() + 1));
}
/// Every `LevelHost` call the fake level answered since
/// `reset_level_reads`: an unregistered hook must make none.
pub fn level_reads() -> usize {
    LEVEL_READS.with(|reads| reads.get())
}
pub fn reset_level_reads() {
    LEVEL_READS.with(|reads| reads.set(0));
}
fn count_level_read() {
    LEVEL_READS.with(|reads| reads.set(reads.get() + 1));
}
impl FakeHost {
    pub fn empty() -> Self {
        Self {
            feedback: Default::default(),
            held: None,
        }
    }

    /// A host whose acting player holds `item` (`count` of it): the held
    /// test seeds this, so `player.held` is set.
    pub fn holding(item: &str, count: u32, plain: bool) -> Self {
        Self {
            feedback: Default::default(),
            held: Some((item.to_owned(), count, plain)),
        }
    }

    /// The command answers the host was given, in order.
    pub fn feedback_lines(&self) -> Vec<String> {
        self.feedback.lock().unwrap().clone()
    }
}
impl Host for FakeHost {
    fn level_players(&self) -> Vec<NativeServerPlayer> {
        count_host_read();
        Vec::new()
    }

    fn state_name(&self, state: BlockStateId) -> String {
        count_host_read();
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

    /// The acting player's held item, or an empty hand: most tests seed
    /// none, and assert the seeded `held` is nil. Uncounted: the command
    /// cost tests pin the heard dispatch at zero host reads.
    fn held(&self) -> Option<(String, u32, bool)> {
        self.held.clone()
    }

    /// A command answer: kept for the tests to read back, like the mob
    /// host's lines. Uncounted, like `held` above.
    fn feedback(&mut self, line: Text) {
        self.feedback.lock().unwrap().push(line.plain());
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
        count_level_read();
        BlockStateId(7)
    }
    fn is_client_side(&self) -> bool {
        count_level_read();
        false
    }
    fn state_with_property(&self, _: BlockStateId, _: &str, _: &str) -> Option<BlockStateId> {
        count_level_read();
        None
    }
    fn mod_block_entity_data(&self, _: [i32; 3]) -> Option<Compound> {
        count_level_read();
        None
    }
    fn state_block(&self, state: BlockStateId) -> Option<String> {
        count_level_read();
        Some(stone_or_state(state))
    }
    fn state_properties(&self, _: BlockStateId) -> Vec<(String, String)> {
        count_level_read();
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
