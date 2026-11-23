//! The developer tools' three read-only reads (`DevInspectServerHost`, the fork-owned host area):
//! the single-player world's owner, the entities a command selector names, and a level's
//! dimension id. They change nothing.
use super::Game;
use horizon_client::command::{Reader, selector};
use mod_api::LevelRef;

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
    fn level_dimension_is_the_overworld_id_of_a_loaded_level() {
        with_guests(|game| {
            let level = game.list_levels()[0];
            assert_eq!(game.dev_level_dimension(level).as_deref(), Some("minecraft:overworld"));
        });
    }
}
