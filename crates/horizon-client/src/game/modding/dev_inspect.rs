//! The developer tools' four read-only reads (`DevInspectServerHost`, the fork-owned host area):
//! the single-player world's owner, the entities a command selector names, a level's dimension
//! id, and an entity's saved data. They change nothing.
use super::Game;
use super::commands::Target;
use super::native_entities::target_id;
use crate::game::persist::write_attachments;
use horizon_client::command::{Reader, selector};
use horizon_core::nbt::{Compound, Tag};
use horizon_mod_runtime::attachments::ATTACHMENTS_KEY;
use mod_api::{EntityRef, LevelRef};

/// Sets `horizon:attachments` to `map`, or removes it when the map is empty (the save's rule).
fn put_attachments(tag: &mut Tag, map: &Compound) {
    if let Tag::Compound(root) = tag {
        if map.is_empty() {
            root.remove(&ATTACHMENTS_KEY.into());
        } else {
            root.insert(ATTACHMENTS_KEY.into(), Tag::Compound(map.clone()));
        }
    }
}

impl Game<'_> {
    /// The single-player world's owner, strictly: the profile the window's own world was opened
    /// for (`IntegratedServer`'s owner, kept on the hub as `singleplayer_owner`). None where no
    /// single player owns the world: a dedicated server, a client, and a world no integrated
    /// server runs. A world opened to LAN from single player has its owner (`lan open`).
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
    /// without `id`), plus the mods' attachments under `horizon:attachments`: the map the save
    /// writes for that kind of target (a player's or guest's own, else the entity's). None for a
    /// stale or foreign handle, and for an entity that is not saved.
    pub(super) fn dev_entity_nbt(&mut self, entity: EntityRef) -> Option<Compound> {
        let mut tag = Tag::Compound(self.entity_nbt(entity)?);
        match self.native_entity_target(entity)? {
            // The player whose turn it is, or the host's own: the map `attachment_slot` gives it.
            Target::Player => put_attachments(&mut tag, &self.player_state.attachments.to_nbt()),
            Target::Guest(conn) if self.lan_acting == Some(conn) => {
                put_attachments(&mut tag, &self.player_state.attachments.to_nbt());
            }
            Target::Guest(conn) => {
                let map = self.lan.as_mut().and_then(|h| h.hub.guests.attachments_mut(conn));
                if let Some(map) = map {
                    put_attachments(&mut tag, &map.to_nbt());
                }
            }
            other => {
                if let Some(id) = target_id(other) {
                    write_attachments(&mut tag, id, &self.entity_attachments);
                }
            }
        }
        match tag {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::native_guest_turn_tests::with_guests;
    use horizon_core::nbt::Tag;
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
    fn the_dev_read_shows_a_mobs_attachments_with_no_key_when_it_has_none() {
        use horizon_mod_runtime::attachments::{ATTACHMENTS_KEY, Attachments};
        with_guests(|game| {
            let kind = game.res.mob_types().get("minecraft:cow").unwrap().clone();
            game.mobs.push(horizon_client::mobs::Mob::new(
                41,
                kind,
                horizon_core::geometry::Vec3::new(0.5, 70.0, 0.5),
                0.0,
            ));
            let cow = game.entity(41).unwrap();
            let plain = game.dev_entity_nbt(cow).expect("a spawned cow is saved");
            assert!(!plain.contains_key(&ATTACHMENTS_KEY.into()));

            let stored: horizon_core::nbt::Compound =
                [("demo:mark".into(), Tag::Int(7))].into();
            game.entity_attachments
                .insert(41, Attachments::from_nbt(stored.clone()));
            let dev = game.dev_entity_nbt(cow).expect("a spawned cow is saved");
            assert_eq!(dev.get(&ATTACHMENTS_KEY.into()), Some(&Tag::Compound(stored)));
        });
    }

    fn stored(value: i32) -> horizon_core::nbt::Compound {
        [("demo:mark".into(), Tag::Int(value))].into()
    }

    #[test]
    #[ignore = "daily"]
    fn the_host_players_attachments_are_in_the_dev_read() {
        use horizon_client::motion::PlayerMotion;
        use horizon_mod_runtime::attachments::ATTACHMENTS_KEY;
        with_guests(|game| {
            game.player = Some(PlayerMotion::new(game.motion, horizon_core::geometry::Vec3::ZERO).unwrap());
            crate::game::native_entity_refs::tests::bind_local_player(game, 20, 123);
            let me = game.entity(20).unwrap();
            let empty = game.dev_entity_nbt(me).expect("the host player is saved");
            assert!(!empty.contains_key(&ATTACHMENTS_KEY.into()));

            game.player_state.attachments.read_saved_over(stored(7));
            let dev = game.dev_entity_nbt(me).expect("the host player is saved");
            assert_eq!(dev.get(&ATTACHMENTS_KEY.into()), Some(&Tag::Compound(stored(7))));
            // The vanilla read of the player (`/data get entity` of it) has no attachments: the
            // dev read is where a mod's player map shows.
            let vanilla = game.entity_nbt(me).expect("the host player is saved");
            assert!(!vanilla.contains_key(&ATTACHMENTS_KEY.into()));
        });
    }

    #[test]
    #[ignore = "daily"]
    fn a_guests_attachments_are_in_the_dev_read_inside_and_outside_its_turn() {
        use horizon_mod_runtime::attachments::ATTACHMENTS_KEY;
        with_guests(|game| {
            let guest = game.player(12).unwrap().entity();
            assert!(!game.dev_entity_nbt(guest).expect("guest 12 is saved").contains_key(&ATTACHMENTS_KEY.into()));

            // Not acting: the guest row's own map.
            game.lan.as_mut().unwrap().hub.guests.attachments_mut(12).unwrap().read_saved_over(stored(8));
            let outside = game.dev_entity_nbt(guest).expect("guest 12 is saved");
            assert_eq!(outside.get(&ATTACHMENTS_KEY.into()), Some(&Tag::Compound(stored(8))));
            let vanilla = game.entity_nbt(guest).expect("guest 12 is saved");
            assert!(!vanilla.contains_key(&ATTACHMENTS_KEY.into()));

            // Acting: its turn moves the map into the player's, and the dev read still has it.
            let inside = game
                .as_guest(12, |game| game.dev_entity_nbt(guest))
                .unwrap()
                .expect("guest 12 is saved during its turn");
            assert_eq!(inside.get(&ATTACHMENTS_KEY.into()), Some(&Tag::Compound(stored(8))));
            // After the turn the map is back on the row.
            let after = game.dev_entity_nbt(guest).expect("guest 12 is saved");
            assert_eq!(after.get(&ATTACHMENTS_KEY.into()), Some(&Tag::Compound(stored(8))));

            // Another guest with no attachments has no key.
            let other = game.player(13).unwrap().entity();
            assert!(!game.dev_entity_nbt(other).expect("guest 13 is saved").contains_key(&ATTACHMENTS_KEY.into()));
        });
    }

    #[test]
    #[ignore = "daily"]
    fn a_guest_swapped_out_by_a_nested_turn_has_no_attachments_in_the_dev_read() {
        use horizon_mod_runtime::attachments::ATTACHMENTS_KEY;
        with_guests(|game| {
            let outer = game.player(12).unwrap().entity();
            let inner = game.player(13).unwrap().entity();
            game.lan.as_mut().unwrap().hub.guests.attachments_mut(12).unwrap().read_saved_over(stored(9));
            game.lan.as_mut().unwrap().hub.guests.attachments_mut(13).unwrap().read_saved_over(stored(10));
            game.as_guest(12, |game| {
                // Guest 12 is the acting one: its map is the player's.
                let own = game.dev_entity_nbt(outer).unwrap();
                assert_eq!(own.get(&ATTACHMENTS_KEY.into()), Some(&Tag::Compound(stored(9))));
                game.as_guest(13, |game| {
                    // Guest 12 is swapped out, its map held by the outer turn: its handle does not
                    // resolve here, so neither read gives its data (and so no attachments).
                    assert_eq!(game.entity_nbt(outer), None);
                    assert_eq!(game.dev_entity_nbt(outer), None);
                    // Guest 13 is acting now.
                    let acting = game.dev_entity_nbt(inner).unwrap();
                    assert_eq!(acting.get(&ATTACHMENTS_KEY.into()), Some(&Tag::Compound(stored(10))));
                })
                .unwrap();
            })
            .unwrap();
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
