//! The `/lua` console's generic host helpers (#32, docs/dev-tools.md): the
//! reads and writes its `mc` table needs, on the world in place, for the
//! console mod of `event-horizon-lua`. They are thin: the engine's own
//! commands do the work, so the console reads and writes what `/data` and
//! the owner's commands do.
use super::Game;
use super::commands::id_of;
use super::commands::{DataRef, Target};
use horizon_client::command::{Reader, selector};
use horizon_world::nbt::{Compound, Tag};

impl Game<'_> {
    /// The ids of the entities a selector names, as the owner's command
    /// would find them (`Game::select`): an entity's id, a player's network
    /// entity id. An empty list for a selector that names nothing.
    /// API @API:lua_console_helpers@.
    pub(super) fn native_console_select_ids(&mut self, selector: &str) -> Vec<u32> {
        let mut reader = Reader::new(selector);
        let Ok(sel) = selector::parse(&mut reader, &self.ui.registries, true) else {
            return Vec::new();
        };
        self.select(&sel)
            .iter()
            .filter_map(|t| self.console_id(*t))
            .collect()
    }

    /// The entity `id`'s data in the level in place, as `/data get entity`
    /// prints it (`Entity.saveWithoutId`): its compound without `id`. None
    /// for no such entity.
    /// API @API:lua_console_helpers@.
    pub(super) fn native_console_entity_data(&mut self, id: u32) -> Option<Compound> {
        let t = self.console_target(id)?;
        let data = self
            .in_target_level(t, |g| g.data_of(&DataRef::Entity(t)).ok())
            .flatten()?;
        match data {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }

    /// Writes the entity `id`'s data in the level in place, as `/data
    /// modify entity` does: false for no such entity and for a write the
    /// entity refuses.
    /// API @API:lua_console_helpers@.
    pub(super) fn native_console_set_entity_data(&mut self, id: u32, tag: Compound) -> bool {
        let Some(t) = self.console_target(id) else {
            return false;
        };
        self.in_target_level(t, |g| {
            g.set_data_of(&DataRef::Entity(t), Tag::Compound(tag)).ok()
        })
        .flatten()
        .is_some()
    }

    /// The entity `id`'s name in the level in place, as plain text
    /// (`Entity.getName`): its custom name, a player's name, else its
    /// type's name. None for no such entity.
    /// API @API:lua_console_helpers@.
    pub(super) fn native_console_entity_name(&mut self, id: u32) -> Option<String> {
        let t = self.console_target(id)?;
        Some(
            self.in_target_level(t, |g| g.target_name(t).plain())
                .unwrap_or_else(|| "gone".to_string()),
        )
    }

    /// Runs a command as the world's owner, in the owner's level, at the
    /// owner's permission level, and returns the chat replies.
    /// API @API:lua_console_helpers@.
    pub(super) fn native_console_run_command(&mut self, line: &str) -> Vec<String> {
        self.owner_console_command(line)
    }

    /// The dimension of the level in place.
    /// API @API:lua_console_helpers@.
    pub(super) fn native_console_dimension(&self) -> String {
        self.dim.id().to_owned()
    }

    /// The id a target has in the console: an entity's id, a player's
    /// network entity id (a player is no entity of the level's list).
    fn console_id(&self, t: Target) -> Option<u32> {
        match t {
            Target::Player => self
                .player
                .as_ref()
                .and_then(|p| u32::try_from(p.entity_id).ok()),
            Target::Guest(conn) => self
                .command_guest(conn)
                .and_then(|g| u32::try_from(g.entity_id).ok()),
            other => id_of(other),
        }
    }

    /// The target an id names, in the level in place: a player by their
    /// network entity id, else the entity of that id. None for no such
    /// entity.
    fn console_target(&self, id: u32) -> Option<Target> {
        if let Some(p) = self.player.as_ref()
            && u32::try_from(p.entity_id).ok() == Some(id)
            // While a guest acts, the window's own player is not in place:
            // the guest's state is.
            && self.lan_acting_uuid.is_none()
        {
            return Some(Target::Player);
        }
        let guest = self.lan.as_ref().and_then(|host| {
            host.hub
                .guests
                .iter()
                .find(|g| g.placed && u32::try_from(g.entity_id).ok() == Some(id))
        });
        if let Some(g) = guest {
            return Some(Target::Guest(g.conn));
        }
        self.level_entity(i32::try_from(id).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated `Host` trait offers the console's helpers: the
    /// generator reads the pending fragment and the game's own methods.
    #[allow(dead_code)]
    fn the_host_offers_the_console_helpers(host: &mut dyn horizon_client::mod_api::Host) {
        let _ = host.select_ids("@e[type=pig]");
        let _ = host.entity_data_in_place(1);
        let _ = host.set_entity_data(1, Compound::new());
        let _ = host.entity_name_in_place(1);
        let _ = host.run_command_as_owner("time set day");
        let _ = host.level_dimension();
    }

    #[test]
    fn the_generator_gives_the_host_the_console_helpers() {
        // The check is the type check of the call above: the trait's methods
        // are the ones the fragment names.
        let _ = the_host_offers_the_console_helpers;
    }
}
