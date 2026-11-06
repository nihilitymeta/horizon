//! An entity's saved data, tags and name for native server mods, through an
//! `EntityRef` (docs/native-mods.md, "Entity and level handles", #7387): what
//! `/data` and `/tag` read and change. They return `None` (or `false`) for a
//! stale handle or on a client.
use super::{Dim, Game, commands::Target};
use horizon_client::mod_api::EntityRef;
use horizon_world::nbt::{Compound, Tag};

/// The most tags an entity holds (`Entity.addTag`).
const MAX_TAGS: usize = 1024;

/// The level `dimension` names, when the game knows it.
fn dimension_of(dimension: &str) -> Option<Dim> {
    let dim = Dim::from_id(dimension);
    (dim.id() == dimension).then_some(dim)
}

/// Whether the entity a target names is a living mob.
fn entity_is_mob(target: Target, alive: bool) -> bool {
    matches!(target, Target::Mob(_)) && alive
}

impl Game<'_> {
    /// The entity's data as `/data get entity` shows it
    /// (`Entity.saveWithoutId`): its compound without `id`. An entity with no
    /// UUID yet gets the one it will be saved with. API 386.
    pub(super) fn native_entity_nbt(&mut self, entity: EntityRef) -> Option<Compound> {
        let t = self.native_entity_target(entity)?;
        match self.entity_nbt(t)? {
            Tag::Compound(mut c) => {
                c.remove(&"id".into());
                Some(c)
            }
            _ => None,
        }
    }
    /// Merges `nbt` into the entity's data and loads the entity from it, as
    /// `/data merge entity` does (`EntityDataAccessor.setData`): true when it
    /// changed. False for a player (vanilla refuses them too), a kind Horizon
    /// can't reload yet (only mobs, dropped items, armor stands and item
    /// frames), data nested too deep, or data that changes nothing. API
    /// 386.
    pub(super) fn native_entity_merge_nbt(&mut self, entity: EntityRef, nbt: Compound) -> bool {
        let Some(t) = self.native_entity_target(entity) else {
            return false;
        };
        let Some(old) = self.native_entity_nbt(entity).map(Tag::Compound) else {
            return false;
        };
        let nbt = Tag::Compound(nbt);
        if horizon_client::nbt_path::too_deep(&nbt, 0) {
            return false;
        }
        let merged = horizon_client::data_cmd::merged(&old, &nbt);
        merged != old && self.apply_entity_nbt(t, merged).is_ok()
    }
    /// The entity's tags (`Entity.getTags`), as `/tag <entity> list` shows
    /// them. API 386.
    pub(super) fn native_entity_tags(&self, entity: EntityRef) -> Option<Vec<String>> {
        Some(self.tags_of(self.native_entity_target(entity)?))
    }
    /// Adds tag `tag` (`Entity.addTag`): false when it has it already, holds
    /// 1024, its kind can't store tags yet, or the handle is stale. API 386.
    pub(super) fn native_entity_add_tag(&mut self, entity: EntityRef, tag: &str) -> bool {
        let Some(t) = self.native_entity_target(entity) else {
            return false;
        };
        let mut tags = self.tags_of(t);
        if tags.len() >= MAX_TAGS || tags.iter().any(|x| x == tag) {
            return false;
        }
        tags.push(tag.to_owned());
        // False, not a silent no-op, for kinds whose tags aren't stored yet
        // (ominous item spawners, falling blocks, rockets).
        self.set_entity_tags(t, &tags)
    }
    /// Removes tag `tag` (`Entity.removeTag`): false when it didn't have it.
    /// API 386.
    pub(super) fn native_entity_remove_tag(&mut self, entity: EntityRef, tag: &str) -> bool {
        let Some(t) = self.native_entity_target(entity) else {
            return false;
        };
        let mut tags = self.tags_of(t);
        let before = tags.len();
        tags.retain(|x| x != tag);
        tags.len() != before && self.set_entity_tags(t, &tags)
    }
    /// The entity's name as plain text (`Entity.getName`): its custom name,
    /// a player's name, else its type's name. API 386.
    pub(super) fn native_entity_name(&self, entity: EntityRef) -> Option<String> {
        Some(self.target_name(self.native_entity_target(entity)?).plain())
    }
}

// Native mods use these; nothing in Horizon does.
#[allow(dead_code)]
impl Game<'_> {
    /// The entity `id`'s data in `dimension`, as `/data get entity` prints it
    /// (`Entity.saveWithoutId`): its compound without `id`. A player is
    /// named by their network entity id; the window's own player counts only
    /// in the dimension it is in. None for an unknown dimension and for no
    /// such entity. A client holds only the level in place, so another
    /// dimension answers None.
    /// API @API:dev_helpers@.
    pub(super) fn native_entity_data(&mut self, dimension: &str, id: u32) -> Option<Compound> {
        // The entity is found and read in the level in place: a target names
        // an entity by its id alone, so reading it after the level was put
        // back would read another level's entity of that id.
        let own = self.dim.id();
        self.native_in_dimension(dimension, |g| g.entity_data_in_level(id, own))
            .flatten()
    }

    /// The entity `id`'s data in the level in place.
    fn entity_data_in_level(&mut self, id: u32, own_dimension: &str) -> Option<Compound> {
        let t = self.native_target_in_level(id, own_dimension)?;
        match self.entity_nbt(t)? {
            Tag::Compound(mut c) => {
                c.remove(&"id".into());
                Some(c)
            }
            _ => None,
        }
    }

    /// The entity `id`'s name in `dimension`, as plain text
    /// (`Entity.getName`): its custom name, a player's name, else its type's
    /// name. The window's own player counts only in the dimension it is in.
    /// None for an unknown dimension and for no such entity.
    /// API @API:dev_helpers@.
    pub(super) fn native_entity_name_at(&mut self, dimension: &str, id: u32) -> Option<String> {
        let own = self.dim.id();
        self.native_in_dimension(dimension, |g| g.entity_name_in_level(id, own))
            .flatten()
    }

    /// The entity `id`'s name in the level in place.
    fn entity_name_in_level(&self, id: u32, own_dimension: &str) -> Option<String> {
        let t = self.native_target_in_level(id, own_dimension)?;
        Some(self.target_name(t).plain())
    }

    /// Whether the entity `id` in `dimension` is a living mob: true for a mob
    /// that is alive and not removed, false for an item, a player and any
    /// other entity. API @API:dev_helpers@.
    pub(super) fn native_entity_is_mob(&mut self, dimension: &str, id: u32) -> bool {
        let own = self.dim.id();
        self.native_in_dimension(dimension, |g| g.entity_is_mob_in_level(id, own))
            .unwrap_or(false)
    }

    /// Whether the entity `id` in the level in place is a living mob.
    fn entity_is_mob_in_level(&self, id: u32, own_dimension: &str) -> bool {
        let Some(t) = self.native_target_in_level(id, own_dimension) else {
            return false;
        };
        let alive = self
            .mobs
            .iter()
            .any(|m| m.id == id && !m.removed && m.alive());
        entity_is_mob(t, alive)
    }

    /// The entity `id` in the level in place: a player by their network
    /// entity id, else the entity of that level. None for no such entity.
    fn native_target_in_level(&self, id: u32, own_dimension: &str) -> Option<Target> {
        if let Some(t) = self.native_player_target(id, own_dimension) {
            return Some(t);
        }
        let id = i32::try_from(id).ok()?;
        self.level_entity(id)
    }

    /// The player `id` in the level in place: a guest of the hub by their
    /// network entity id, else the window's own player, which is in
    /// `own_dimension` (the dimension the window was in) and so counts only
    /// in that dimension.
    fn native_player_target(&self, id: u32, own_dimension: &str) -> Option<Target> {
        let dimension = self.dim.id();
        let guest = self.lan.as_ref().and_then(|host| {
            host.hub.guests.iter().find(|g| {
                g.placed && g.dimension == dimension && u32::try_from(g.entity_id).ok() == Some(id)
            })
        });
        if let Some(g) = guest {
            return Some(Target::Guest(g.conn));
        }
        if dimension == own_dimension
            && self
                .player
                .as_ref()
                .and_then(|p| u32::try_from(p.entity_id).ok())
                == Some(id)
            // While a guest acts, the window's own player is not in place:
            // the guest's state is.
            && self.lan_acting_uuid.is_none()
        {
            return Some(Target::Player);
        }
        None
    }

    /// Run `f` in the level of `dimension`, putting the level back; None when
    /// the dimension is unknown. A known dimension is opened when nobody is
    /// in it; a client holds only the level in place.
    pub(in crate::game) fn native_in_dimension<R>(
        &mut self,
        dimension: &str,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        if self.dim.id() == dimension {
            return Some(f(self));
        }
        let dim = dimension_of(dimension)?;
        let back = self.enter_command_level(dim)?;
        let r = f(self);
        self.enter_command_level(back);
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated `Host` trait offers the new helpers: the generator
    /// reads the pending fragment and the game's own methods.
    #[allow(dead_code)]
    fn the_host_offers_the_new_helpers(host: &mut dyn horizon_client::mod_api::Host) {
        let _ = host.entity_data("minecraft:overworld", 1);
        let _ = host.entity_name_at("minecraft:overworld", 1);
        let _ = host.entity_is_mob("minecraft:overworld", 1);
        let _ = host.block_info("minecraft:overworld", [0, 64, 0], [0, 1, 0]);
        let _ = host.send_payload(1, "horizon:dev_tools", &[0]);
        let _ = host.player_sneaking(1);
        let _ = host.player_permission(1);
    }

    #[test]
    fn the_generator_gives_the_host_the_new_helpers() {
        // The check is the type check of the call above: the trait's methods
        // are the ones the fragment names.
        let _ = the_host_offers_the_new_helpers;
    }

    #[test]
    fn only_the_dimensions_the_game_knows_are_levels() {
        assert_eq!(dimension_of("minecraft:overworld"), Some(Dim::Overworld));
        assert_eq!(dimension_of("minecraft:the_nether"), Some(Dim::Nether));
        assert_eq!(dimension_of("minecraft:the_end"), Some(Dim::End));
        assert_eq!(dimension_of("minecraft:the_void"), None);
        assert_eq!(dimension_of(""), None);
    }

    #[test]
    fn only_a_living_mob_is_a_mob() {
        assert!(entity_is_mob(Target::Mob(7), true));
        assert!(!entity_is_mob(Target::Mob(7), false));
        assert!(!entity_is_mob(Target::Item(7), true));
        assert!(!entity_is_mob(Target::Player, true));
        assert!(!entity_is_mob(Target::Guest(1), true));
    }

    /// A game with the prepared resources and an integrated server.
    fn game_with_resources() -> super::super::Game<'static> {
        use super::super::{
            Audio, Game, Options, Progress, Settings, execution_role::ExecutionRole, load_resources,
        };

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
            &mut |_, _| unreachable!("no menu handoff"),
        )
        .unwrap();
        let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
        options.resources = root.clone();
        options.execution_role = ExecutionRole::IntegratedServer;
        let res: &'static _ = Box::leak(Box::new(loaded.resources));
        Game::new(
            res,
            loaded.pack,
            loaded.gui.unwrap(),
            options,
            settings,
            Audio::silent(&root),
            None,
        )
        .unwrap()
    }

    /// An entity is read in the level that holds it: the same id in the level
    /// in place is another entity, and the entity of another level is not
    /// read at all.
    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn an_entity_is_read_in_the_level_that_holds_it() {
        use super::super::Dim;
        use horizon_client::mobs::Mob;
        use horizon_core::geometry::Vec3;

        let mut game = game_with_resources();
        let kind = game.res.mob_types().get("minecraft:cow").unwrap().clone();
        // A cow of the level in place, with the id the other level's cow has.
        game.mobs
            .push(Mob::new(42, kind.clone(), Vec3::new(100.0, 64.0, 0.0), 0.0));
        // The other level: a cow of its own, with the same id.
        let Some(back) = game.enter_command_level(Dim::Nether) else {
            panic!("the nether opens");
        };
        game.mobs
            .push(Mob::new(42, kind.clone(), Vec3::new(0.0, 64.0, 0.0), 0.0));
        game.enter_command_level(back);

        let nether = game.native_entity_data("minecraft:the_nether", 42).unwrap();
        let pos_x = |tag: &horizon_world::nbt::Compound| match tag.get(&"Pos".into()) {
            Some(Tag::List(list)) => match list.first() {
                Some(Tag::Double(bits)) => Some(f64::from_bits(*bits)),
                _ => None,
            },
            _ => None,
        };
        assert_eq!(
            pos_x(&nether),
            Some(0.0),
            "the nether's cow is read, not the level in place's: {nether:?}"
        );
        let overworld = game.native_entity_data("minecraft:overworld", 42).unwrap();
        assert_eq!(
            pos_x(&overworld),
            Some(100.0),
            "the level in place's cow is read: {overworld:?}"
        );
        // A dimension the game does not know holds no entity.
        assert!(game.native_entity_data("minecraft:the_void", 42).is_none());
        // An id no level holds is no entity.
        assert!(game.native_entity_data("minecraft:overworld", 43).is_none());
        assert!(!game.native_entity_is_mob("minecraft:the_void", 42));
    }

    /// The window's own player is read in the dimension it is in only: a
    /// request for another dimension does not find it there.
    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn the_own_player_is_read_in_its_own_dimension_only() {
        use super::super::Dim;
        use horizon_client::motion::PlayerMotion;
        use horizon_core::geometry::Vec3;

        let mut game = game_with_resources();
        game.player = Some(PlayerMotion::new(game.motion, Vec3::new(10.5, 64.0, -3.25)).unwrap());
        game.player.as_mut().unwrap().entity_id = 7;
        // The nether is open, so a request for it reads that level and not
        // a level that is not there.
        let Some(back) = game.enter_command_level(Dim::Nether) else {
            panic!("the nether opens");
        };
        game.enter_command_level(back);

        assert!(
            game.native_entity_data("minecraft:overworld", 7).is_some(),
            "the own player is read in the dimension it is in"
        );
        assert!(
            game.native_entity_name_at("minecraft:overworld", 7)
                .is_some(),
            "the own player has a name in the dimension it is in"
        );
        assert!(
            !game.native_entity_is_mob("minecraft:overworld", 7),
            "a player is not a mob"
        );
        assert!(
            game.native_entity_data("minecraft:the_nether", 7).is_none(),
            "the own player is not in the nether"
        );
        assert!(
            game.native_entity_name_at("minecraft:the_nether", 7)
                .is_none(),
            "the own player has no name in the nether"
        );
        assert!(
            !game.native_entity_is_mob("minecraft:the_nether", 7),
            "the nether holds no mob of that id"
        );
    }
}
