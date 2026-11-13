//! The window's own player and a clock that never freezes (docs/native-mods.md, "A client
//! mod's view"): what a HUD draw hook reads live about the player watching it, and a wall
//! clock to animate a HUD on while `/tick freeze` holds the game clock.
use super::Game;
use horizon_client::mod_api::{NativeCrosshair, NativeLocalPlayer, NativeRealClock};

/// `Entity.getMaxAirSupply` for a player: the game's own constant, which this version has no
/// `max_air` attribute for (mobs overrides it, players do not).
const PLAYER_MAX_AIR: i32 = 300;

// Native mods use these; nothing in Horizon calls them.
impl Game<'_> {
    /// The window's own player: health, maximum health, absorption, food and saturation, air,
    /// experience, the armour attribute, position and view, the selected hotbar slot and the
    /// crosshair's target. The server sends these even in single player (the server thread), so
    /// they are the state the HUD shows. `max_health` and `armor` are the `minecraft:max_health`
    /// and `minecraft:armor` attributes, recomputed when read so a modifier the last attribute
    /// packet applied shows at once (20 and 0 when the player has no such attribute); `air` is
    /// the last synched `DATA_AIR_SUPPLY_ID`, 300 at full air.
    /// None with no player: the title screen. API @API:local_player@.
    pub(super) fn native_local_player(&self) -> Option<NativeLocalPlayer> {
        let player = self.player.as_ref()?;
        Some(NativeLocalPlayer {
            health: player.health.health(),
            max_health: player
                .attributes
                .computed_value_of("minecraft:max_health")
                .unwrap_or(20.0) as f32,
            absorption: player.health.absorption(),
            food: player.health.food.food,
            saturation: player.health.food.saturation,
            air: player.air,
            max_air: PLAYER_MAX_AIR,
            xp_level: self.experience.level,
            xp_progress: self.experience.progress,
            xp_total: self.experience.total,
            armor: player
                .attributes
                .computed_value_of("minecraft:armor")
                .unwrap_or(0.0),
            position: [player.position.x, player.position.y, player.position.z],
            yaw: self.yaw,
            pitch: self.pitch,
            selected_slot: self.selected,
            crosshair: self.native_crosshair(),
        })
    }

    /// A clock a HUD animation can rely on: seconds since the game started (monotonic) and the
    /// wall time the last frame took, in seconds. Neither freezes: they come from the wall clock
    /// and the frame's own delta, not from the game clock `native_clock` reports, which `/tick
    /// freeze` holds like vanilla. API @API:real_clock@.
    pub(super) fn native_real_clock(&self) -> NativeRealClock {
        NativeRealClock {
            seconds: self.started.elapsed().as_secs_f64(),
            delta: self.frame_delta,
        }
    }

    /// The crosshair's target: the entity the view meets before any block (the client's own
    /// pick, as attacks and uses make it), else the first block along the view.
    fn native_crosshair(&self) -> Option<NativeCrosshair> {
        self.player.as_ref()?;
        if let Some((distance, id)) = self.pick_entity_hit_at() {
            let pos = id
                .and_then(|id| self.crosshair_entity_block_pos(id))
                .or_else(|| self.crosshair_view_block(distance));
            if let Some(pos) = pos {
                return Some(NativeCrosshair {
                    kind: "entity",
                    distance: distance as f32,
                    pos,
                    entity: id.and_then(|id| u32::try_from(id).ok()),
                });
            }
        }
        let (distance, hit) = self.native_view_hit()?;
        Some(NativeCrosshair {
            kind: "block",
            distance,
            pos: [hit.block_x, hit.block_y, hit.block_z],
            entity: None,
        })
    }

    /// The block position of the entity an entity hit names: the feet of a mob, another object's
    /// own position (a boat, minecart, armor stand, cushion, item frame, painting, interaction
    /// box); None for one of the kinds `object_target` does not know.
    fn crosshair_entity_block_pos(&self, id: i32) -> Option<[i32; 3]> {
        let mob = u32::try_from(id)
            .ok()
            .and_then(|id| self.mobs.iter().find(|m| m.id == id));
        let at = match mob {
            Some(m) => [m.position.x, m.position.y, m.position.z],
            None => self.object_target(id)?.2,
        };
        Some(at.map(|v| v.floor() as i32))
    }

    /// The block the view is in at `distance`: where an entity hit has no position of its own.
    fn crosshair_view_block(&self, distance: f64) -> Option<[i32; 3]> {
        let (eye, dir) = self.aim()?;
        Some([0, 1, 2].map(|a| (eye[a] + dir[a] * distance).floor() as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::execution_role::ExecutionRole;
    use crate::game::{Audio, Options, Progress, Settings, load_resources};
    use horizon_client::chunk::Chunk;
    use horizon_client::mobs::Mob;
    use horizon_core::geometry::Vec3;
    use std::sync::Arc;

    fn game() -> Game<'static> {
        let root = std::path::PathBuf::from(
            std::env::var_os("HORIZON_TEST_RESOURCES")
                .expect("set HORIZON_TEST_RESOURCES to prepared resources"),
        );
        let settings = Settings::default();
        let loaded = load_resources(
            root.clone(),
            None,
            &root,
            &settings,
            std::time::Instant::now(),
            false,
            true,
            &Progress::new(),
            &mut |_, _| unreachable!(),
        )
        .unwrap();
        let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
        options.resources = root.clone();
        options.execution_role = ExecutionRole::IntegratedServer;
        let res: &'static _ = Box::leak(Box::new(loaded.resources));
        let mut game = Game::new(
            res,
            loaded.pack,
            loaded.gui.unwrap(),
            options,
            settings,
            Audio::silent(&root),
            None,
        )
        .unwrap();
        game.build_registries();
        game
    }

    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn the_window_player_reads_every_field_where_it_is() {
        let mut game = game();
        assert!(
            game.native_local_player().is_none(),
            "the title screen has no player"
        );
        let p = horizon_client::motion::PlayerMotion::new(game.motion, Vec3::new(12.5, 64.0, -3.5))
            .unwrap();
        game.player = Some(p);
        {
            let p = game.player.as_mut().unwrap();
            // Distinct known values: a swapped field fails below.
            p.set_health(13.0, false).unwrap();
            p.internal_set_absorption(4.0).unwrap();
            p.health.food.food = 17;
            p.health.food.saturation = 3.5;
            p.air = 123;
            // The window's `UpdateAttributes` path: the packet's snapshots go
            // through `lan::attributes::apply`, which only dirties the instances.
            // Nothing below calls `value`, so a stale cache read shows here.
            let values: Vec<_> = [
                ("minecraft:max_health", 20.0, "test:max_health", 10.0),
                ("minecraft:armor", 0.0, "test:armor", 7.0),
            ]
            .into_iter()
            .map(|(name, base, id, amount)| {
                let attribute = p.attributes.key(name).unwrap().attribute().registry_id();
                horizon_protocol::packets::play::AttributeSnapshot {
                    attribute,
                    base,
                    modifiers: vec![horizon_protocol::packets::play::AttributeModifier {
                        id: id.into(),
                        amount,
                        operation: 0,
                    }],
                }
            })
            .collect();
            horizon_client::lan::attributes::apply(&mut p.attributes, &values);
        }
        game.experience.level = 7;
        game.experience.progress = 0.25;
        game.experience.total = 321;
        game.yaw = 33.0;
        game.pitch = -12.0;
        game.selected = 4;

        let player = game.native_local_player().unwrap();
        assert_eq!(player.health, 13.0);
        assert_eq!(player.max_health, 30.0, "the max_health attribute");
        assert_eq!(player.absorption, 4.0);
        assert_eq!(player.food, 17);
        assert_eq!(player.saturation, 3.5);
        assert_eq!(player.air, 123);
        assert_eq!(player.max_air, 300);
        assert_eq!(player.xp_level, 7);
        assert_eq!(player.xp_progress, 0.25);
        assert_eq!(player.xp_total, 321);
        assert_eq!(player.armor, 7.0, "the armor attribute");
        assert_eq!(player.position, [12.5, 64.0, -3.5]);
        assert_eq!(player.yaw, 33.0);
        assert_eq!(player.pitch, -12.0);
        assert_eq!(player.selected_slot, 4);
        assert_eq!(
            player.crosshair, None,
            "no loaded block and no entity in reach"
        );
    }

    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn the_crosshair_names_a_block_or_the_entity_before_it() {
        let mut game = game();
        game.world
            .chunks
            .insert((0, 0), Arc::new(Chunk::empty(0, 0)));
        let registry = game.pack.assets.registry;
        let air = registry.default_state("minecraft:air").unwrap();
        let stone = registry.default_state("minecraft:stone").unwrap();
        game.player = Some(
            horizon_client::motion::PlayerMotion::new(game.motion, Vec3::new(0.5, 64.0, 0.5))
                .unwrap(),
        );
        // Eye at y 65.62, looking along +z (yaw 0, pitch 0).
        game.yaw = 0.0;
        game.pitch = 0.0;
        game.world.set(0, 65, 2, stone).unwrap();
        let crosshair = game.native_local_player().unwrap().crosshair.unwrap();
        assert_eq!(crosshair.kind, "block");
        assert_eq!(crosshair.pos, [0, 65, 2]);
        assert_eq!(crosshair.entity, None);
        assert!(
            (1.0..2.0).contains(&crosshair.distance),
            "the stone's near face is 1.5 along the view: {}",
            crosshair.distance
        );

        // The same view with the stone gone meets a zombie instead.
        game.world.set(0, 65, 2, air).unwrap();
        let kind = game.res.mob_types()["minecraft:zombie"].clone();
        game.mobs
            .push(Mob::new(325, kind, Vec3::new(0.5, 64.0, 2.5), 0.0));
        let crosshair = game.native_local_player().unwrap().crosshair.unwrap();
        assert_eq!(crosshair.kind, "entity");
        assert_eq!(crosshair.entity, Some(325));
        assert_eq!(crosshair.pos, [0, 64, 2], "the zombie's feet, floored");
        assert!(
            (0.5..2.0).contains(&crosshair.distance),
            "the zombie's near face is about 1.7 along the view: {}",
            crosshair.distance
        );
    }

    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn the_real_clock_moves_while_the_game_clock_is_frozen() {
        let mut game = game();
        // A sentinel `elapsed` cannot be: a `seconds` that returned the frame
        // delta would report 5.0 below, not the wall clock.
        game.frame_delta = 5.0;
        let first = game.native_real_clock();
        assert_eq!(first.delta, 5.0, "the last frame's delta");

        // `/tick freeze` holds the game clock; the real one reads the wall clock
        // and the frame's own delta, so it does not stop.
        game.clock.set_frozen(true);
        assert!(game.clock.frozen());
        let before = game.started.elapsed().as_secs_f64();
        let second = game.native_real_clock();
        let after = game.started.elapsed().as_secs_f64();
        assert_eq!(second.delta, 5.0, "the last frame's delta");
        assert_ne!(
            second.seconds, second.delta,
            "seconds is the wall clock, not the frame delta"
        );
        assert!(
            (before..=after).contains(&second.seconds),
            "the wall clock: {before} <= {} <= {after}",
            second.seconds
        );
        assert!(
            second.seconds > first.seconds,
            "the wall clock does not stop for /tick freeze"
        );
        game.clock.set_frozen(false);
    }
}
