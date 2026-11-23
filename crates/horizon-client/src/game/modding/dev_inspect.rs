//! The developer tools' four read-only reads (`DevInspectServerHost`, the fork-owned host area):
//! the single-player world's owner, the entities a command selector names, a level's dimension
//! id, and an entity's saved data. They change nothing.
use super::Game;
use horizon_client::command::{Reader, selector};
use horizon_core::nbt::Compound;
use mod_api::{EntityRef, LevelRef};

impl Game<'_> {
    /// The single-player world's owner, strictly: the profile the window's own world was opened
    /// for (`IntegratedServer`'s owner, kept on the hub as `singleplayer_owner`). None where no
    /// single player owns the world: a dedicated server, a client, and a LAN host's world.
    pub(super) fn dev_server_owner(&self) -> Option<u128> {
        self.lan.as_ref()?.hub.singleplayer_owner
    }

    /// The network entity ids of the entities a selector names, as the server's own command
    /// source finds them (`Game::select`). A player's is its network entity id. A selector that
    /// does not parse gives an empty list.
    pub(super) fn dev_select_ids(&mut self, text: &str) -> Vec<u32> {
        let mut reader = Reader::new(text);
        let Ok(sel) = selector::parse(&mut reader, &self.ui.registries, true) else {
            return Vec::new();
        };
        self.select(&sel)
            .into_iter()
            .filter_map(|t| u32::try_from(self.network_entity_id(t)?).ok())
            .collect()
    }

    /// The dimension id of a server level the handle names (`minecraft:overworld`, ...). None for
    /// a stale or foreign handle, or where no server level is open.
    pub(super) fn dev_level_dimension(&self, level: LevelRef) -> Option<String> {
        self.native_level_world(level)?;
        self.dimension_key_id(level.dimension())
    }

    /// The entity's saved data, as the engine's `entity_nbt` gives it (`/data get entity`
    /// without `id`). None for a stale or foreign handle, and for an entity that is not saved.
    pub(super) fn dev_entity_nbt(&mut self, entity: EntityRef) -> Option<Compound> {
        self.entity_nbt(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::super::native_guest_turn_tests::with_guests;
    use mod_api::prelude::*;

    #[test]
    #[ignore = "daily"]
    fn owner_is_the_hub_single_player_owner_only() {
        with_guests(|game| {
            assert_eq!(game.dev_server_owner(), None);
            game.lan.as_mut().unwrap().hub.singleplayer_owner = Some(12);
            assert_eq!(game.dev_server_owner(), Some(12));
            game.lan.as_mut().unwrap().hub.singleplayer_owner = None;
            assert_eq!(game.dev_server_owner(), None);
        });
    }

    #[test]
    #[ignore = "daily"]
    fn selector_ids_are_empty_when_it_does_not_parse_or_names_nothing() {
        with_guests(|game| {
            assert!(game.dev_select_ids("@e[").is_empty());
            assert!(game.dev_select_ids("@e[type=minecraft:pig,tag=no_such_tag]").is_empty());
        });
    }

    #[test]
    #[ignore = "daily"]
    fn a_spawned_mobs_saved_data_is_read_and_a_stale_handle_gives_none() {
        with_guests(|game| {
            let kind = game.res.mob_types().get("minecraft:cow").unwrap().clone();
            game.mobs.push(horizon_client::mobs::Mob::new(
                41,
                kind,
                horizon_core::geometry::Vec3::new(0.5, 70.0, 0.5),
                0.0,
            ));
            let cow = game.entity(41).unwrap();
            let saved = game.dev_entity_nbt(cow).expect("a spawned cow is saved");
            assert!(saved.contains_key(&"Health".into()));
            assert!(!saved.contains_key(&"id".into()));
            game.mobs.clear();
            assert_eq!(game.dev_entity_nbt(cow), None);
        });
    }

    #[test]
    #[ignore = "daily"]
    fn level_dimension_is_the_overworld_id_of_a_loaded_level() {
        with_guests(|game| {
            let level = game.list_levels()[0];
            assert_eq!(game.dev_level_dimension(level).as_deref(), Some("minecraft:overworld"));
        });
    }
}
