//! Bulk block edits for server mods: a helper on the level being ticked, as `native_set_block` is, that makes many
//! runs of blocks up columns at once, straight into the chunks.
use super::Game;
use horizon_world::registry::BlockStateId;

/// Which of a column's heights (`Heightmap.Types`) a run's `y` counts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeHeightmap {
    WorldSurface,
    MotionBlocking,
    MotionBlockingNoLeaves,
    OceanFloor,
}
/// A run: `height` blocks up a column from `y` (from the column's `surface` height when one is
/// given), set to palette entry `block`, only where filter `only` matches when one is given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeBlockRun {
    pub x: i32,
    pub z: i32,
    pub y: i32,
    pub height: u32,
    pub surface: Option<NativeHeightmap>,
    pub block: u32,
    pub only: Option<u32>,
}
/// A bulk edit: block states and block predicates as commands write them (without NBT), and the runs naming them
/// by index.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NativeBlockEdit {
    pub palette: Vec<String>,
    pub filters: Vec<String>,
    pub runs: Vec<NativeBlockRun>,
}

#[allow(dead_code)]
impl Game<'_> {
    /// For a server mod: the runs made at once, straight into the chunks of the authoritative level `dimension`
    /// (the one being ticked, as for `native_set_block`), with what `fill ... strict` does for block entities (a
    /// block replaced by another loses its block entity, and a new block gets the one `native_set_block` would
    /// give it, a mod block's included); no neighbour is told, and the chunks changed go to the players whole.
    /// Runs in chunks not loaded, and the parts of runs outside the column, are left out. The blocks changed, or
    /// why the edit was left out, with nothing changed: another dimension, a client of a server, a palette entry
    /// or filter that doesn't read or carries NBT (refused), a run naming a palette entry or filter past the end,
    /// or more than 65536 filters. API 126.
    pub(super) fn native_edit_blocks(
        &mut self,
        dimension: &str,
        edit: &NativeBlockEdit,
    ) -> Result<u64, String> {
        if !self.native_level_open(dimension) {
            return Err(format!("{dimension} is not the level being ticked"));
        }
        let started = std::time::Instant::now();
        let changed = self.edit_blocks(edit)?;
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        if self.options.spikes.is_some_and(|threshold| ms > threshold) {
            log::info!(
                "Horizon: slow mod block edit {ms:.1} ms ({} runs, {changed} blocks)",
                edit.runs.len()
            );
        }
        Ok(changed)
    }
    /// Whether `dimension` is the authoritative level being ticked: where a server mod's world helpers act.
    /// The block state `name` names, as a command reads one
    /// (`minecraft:oak_log[axis=x]`; the properties left out are the
    /// block's defaults): what `native_set_block` and
    /// `native_spawn_falling_block` take. None for a block the game doesn't
    /// have, a property it hasn't, or block NBT (`{...}`, not read here).
    /// API 365.
    pub(super) fn native_state_named(&self, name: &str) -> Option<BlockStateId> {
        use horizon_client::command::{Reader, block};
        let mut reader = Reader::new(name);
        let b = block::parse(&mut reader, &self.ui.registries, false).ok()?;
        if reader.can_read() || b.nbt.is_some() {
            return None;
        }
        self.block_state(&b).ok()
    }
    /// The block state `state` names, as a command writes one
    /// (`minecraft:oak_log[axis=x]`; `minecraft:air` for a state the game
    /// doesn't have): the reverse of `native_state_named`, for a hook that
    /// hands a state (`native_loot_drops`'s block). API 369.
    pub(super) fn native_state_name(&self, state: BlockStateId) -> String {
        let Some(s) = self.pack.assets.registry.state(state) else {
            return "minecraft:air".to_owned();
        };
        let mut name = s.name.to_string();
        if !s.properties.is_empty() {
            let mut props: Vec<String> = s
                .properties
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            props.sort();
            name.push('[');
            name.push_str(&props.join(","));
            name.push(']');
        }
        name
    }
    pub(super) fn native_level_open(&self, dimension: &str) -> bool {
        (self.serving.is_some() || self.options.dedicated.is_some()) && dimension == self.dim.id()
    }
}
