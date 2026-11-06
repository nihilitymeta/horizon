//! A block's facts for native mods: what the developer tools' block window
//! shows (docs/native-mods.md, "Generic host helpers"), read in the level
//! that holds the block. The window's own bookkeeping (when it was read)
//! stays with the window.
use super::{Game, commands::DataRef};
use horizon_core::position::BlockPos;
use horizon_core::ticks::LevelTicks;
use horizon_world::nbt::Tag;

/// A block as a native mod reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeBlockInfo {
    /// `minecraft:repeater`, and its state properties.
    pub name: String,
    pub properties: Vec<(String, String)>,
    /// Its block entity's data, as `/data get block` prints it.
    pub entity: Option<Tag>,
    /// Block light and sky light at the block, and at the clicked face.
    pub light: [u8; 2],
    pub face_light: [u8; 2],
    pub biome: Option<String>,
    /// `getBestNeighborSignal`: the power it receives.
    pub power: i32,
    /// `getDirectSignal(pos, direction)`, by `Direction::ALL`: the strong
    /// signal it gives each way.
    pub strong: [i32; 6],
    pub random_ticks: bool,
    /// Its pending scheduled ticks: what, in how many ticks, and the priority.
    pub scheduled: Vec<(String, i64, i32)>,
    /// The block tags it is in, `#minecraft:mineable/pickaxe`.
    pub tags: Vec<String>,
    /// A content mod's block: the mod that declares it, and its template.
    pub declared_by: Option<(String, String)>,
    /// Its collision and outline boxes, in world coordinates (min x, y, z,
    /// max x, y, z).
    pub collision: Vec<[f64; 6]>,
    pub outline: Vec<[f64; 6]>,
}

/// The light of a block: the block light and the sky light.
fn light_pair(light: u8) -> [u8; 2] {
    [light & 15, light >> 4]
}

/// The scheduled ticks of `at` in one tick table: what, in how many ticks,
/// and the priority.
fn scheduled_ticks(
    ticks: &LevelTicks,
    kind: &str,
    at: BlockPos,
    now: i64,
) -> Vec<(String, i64, i32)> {
    let Some(chunk) = ticks.chunk(at.chunk()) else {
        return Vec::new();
    };
    chunk
        .queued()
        .iter()
        .filter(|t| t.pos == at)
        .map(|t| (kind.to_owned(), t.trigger_tick - now, t.priority as i32))
        .collect()
}

impl Game<'_> {
    /// The block at `pos` in `dimension`, clicked on `face`: its state and
    /// properties, its block entity's data, the light on the block and on the
    /// face, its biome, the power it receives and gives, its scheduled
    /// ticks, its tags, the mod that declares it and its boxes. None for an
    /// unknown dimension. A known dimension is opened when nobody is in it;
    /// a client holds only the level in place, so another dimension answers
    /// None.
    /// API @API:dev_helpers@.
    pub(super) fn native_block_info(
        &mut self,
        dimension: &str,
        pos: [i32; 3],
        face: [i32; 3],
    ) -> Option<NativeBlockInfo> {
        self.native_in_dimension(dimension, |g| g.block_info_at(pos, face))
    }

    /// The block at `pos`, clicked on `face`, in the level in place.
    pub(in crate::game) fn block_info_at(
        &mut self,
        pos: [i32; 3],
        face: [i32; 3],
    ) -> NativeBlockInfo {
        let [x, y, z] = pos;
        let state = self.world.block(x, y, z);
        let (name, properties) = match self.pack.assets.registry.state(state) {
            Some(s) => (
                s.name.clone(),
                s.properties
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            None => (String::new(), Vec::new()),
        };
        let entity = self.data_of(&DataRef::Block(pos)).ok();
        let at = BlockPos::new(x, y, z);
        let (power, strong) = self.with_level(|b, level, ticks| {
            (
                b.best_neighbor_signal(level, ticks, at),
                b.direct_signals(level, ticks, at),
            )
        });
        let now = self.world.game_time;
        let mut scheduled = Vec::new();
        for (kind, ticks) in [
            ("block", &self.block_ticks.blocks),
            ("fluid", &self.block_ticks.fluids),
        ] {
            scheduled.extend(scheduled_ticks(ticks, kind, at, now));
        }
        let tags = horizon_client::debug_overlay::tag_lines(&self.items().block_tags, &name);
        let collision = self
            .block_collision_shape(pos)
            .boxes()
            .iter()
            .map(|b| {
                let (min, max) = (b.min_position(), b.max_position());
                [
                    f64::from(x) + min.x,
                    f64::from(y) + min.y,
                    f64::from(z) + min.z,
                    f64::from(x) + max.x,
                    f64::from(y) + max.y,
                    f64::from(z) + max.z,
                ]
            })
            .collect();
        let declared_by = crate::game::mods::block_declared_by(&name);
        NativeBlockInfo {
            name,
            properties,
            entity,
            light: light_pair(self.world.light(x, y, z)),
            face_light: light_pair(self.world.light(x + face[0], y + face[1], z + face[2])),
            biome: self.biome_name_at(horizon_core::geometry::Vec3::new(
                f64::from(x) + 0.5,
                f64::from(y) + 0.5,
                f64::from(z) + 0.5,
            )),
            power,
            strong,
            random_ticks: self.res.behaviours().is_randomly_ticking(state),
            scheduled,
            tags,
            declared_by,
            collision,
            outline: self.outline_boxes(pos),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::ticks::{ChunkTicks, ScheduledTick, TickPriority, TickTypeId};

    #[test]
    fn the_light_of_a_block_is_its_block_light_and_its_sky_light() {
        assert_eq!(light_pair(0), [0, 0]);
        assert_eq!(light_pair(0x0f), [15, 0]);
        assert_eq!(light_pair(0xf0), [0, 15]);
        assert_eq!(light_pair(0xa5), [5, 10]);
    }

    #[test]
    fn the_scheduled_ticks_of_a_block_are_what_is_left_to_tick() {
        let at = BlockPos::new(3, 64, -5);
        let tick = |pos, trigger, priority| ScheduledTick {
            kind: TickTypeId(1),
            pos,
            trigger_tick: trigger,
            priority,
            sub_tick_order: 0,
        };
        let mut blocks = LevelTicks::default();
        blocks.add_chunk(at.chunk(), ChunkTicks::default()).unwrap();
        blocks.schedule(tick(at, 120, TickPriority::Normal));
        blocks.schedule(tick(BlockPos::new(4, 64, -5), 130, TickPriority::Normal));
        let mut fluids = LevelTicks::default();
        fluids.add_chunk(at.chunk(), ChunkTicks::default()).unwrap();
        fluids.schedule(tick(at, 140, TickPriority::High));

        assert_eq!(
            scheduled_ticks(&blocks, "block", at, 100),
            vec![("block".to_owned(), 20, 0)]
        );
        assert_eq!(
            scheduled_ticks(&fluids, "fluid", at, 100),
            vec![("fluid".to_owned(), 40, -1)]
        );
        // Another chunk holds no tick of this block.
        let elsewhere = BlockPos::new(100, 64, 100);
        assert!(scheduled_ticks(&blocks, "block", elsewhere, 100).is_empty());
    }
}
