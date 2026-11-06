//! The developer tools' engine half (docs/dev-tools.md): the tools' window
//! state the game reaches for, the console's completions and the two
//! packet feeds between a guest's client and the server. The commands
//! (`/inspect`, `/goals`, `/senses`, `/nav`, `/watch`, `/mobai log`,
//! `/lua`) and the windows' server half live in `event-horizon-dev-tools`
//! (#31, #34); what is left here talks to the shared
//! `event_horizon_observe` store, which both halves read.
use super::Game;
use super::commands::Target;
use crate::dev_input::console;
use crate::mobs::observe;
use crate::mobs::observe_wire::{self, Wire};

/// A target's entity id; players have none here (`/data get entity @s` reads
/// the acting player, who is nobody on a tick).
pub(in crate::game) fn id_of(t: Target) -> Option<u32> {
    match t {
        Target::Player | Target::Guest(_) => None,
        Target::Mob(id)
        | Target::Item(id)
        | Target::Orb(id)
        | Target::Projectile(id)
        | Target::Stand(id)
        | Target::Frame(id)
        | Target::Painting(id)
        | Target::Boat(id)
        | Target::Cart(id)
        | Target::Other(id)
        | Target::Crystal(id)
        | Target::Dragon(id)
        | Target::Tnt(id)
        | Target::Falling(id)
        | Target::Rocket(id)
        | Target::Trident(id)
        | Target::Spawner(id)
        | Target::Fangs(id)
        | Target::Cloud(id) => Some(id),
    }
}

impl Game<'_> {
    /// The world closed: nothing of it stays watched. The windows' store
    /// empties, the AI logs' files close, the tick count restarts, and
    /// the tools let the pointer go, so a watch on entity 123 of this world
    /// never reads the next world's entity 123. The console closes and
    /// drops its handlers in the console mod's `world_lifecycle` hook.
    pub(in crate::game) fn dev_tools_world_closed(&mut self) {
        // The stick's own statics are reset by the mod's `world_lifecycle`
        // hook on the server; this half empties the shared store.
        observe::reset();
        crate::dev_input::world_closed();
    }

    /// Once a frame on the window's game: the completions the console asked
    /// for. Inside `mc.cmd("…")` they are the chat box's own, from the
    /// tree the server sent; elsewhere the `mc` names.
    pub(in crate::game) fn dev_console_complete(&mut self) {
        let Some((text, cursor)) = console::take_completion_request() else {
            return;
        };
        let tree_completions = |start: usize, command: &str, skip: usize| {
            // `skip` bytes of `command` are a prefix the line does not hold.
            let tree = crate::game::chat::chat_tree(&self.remote);
            let parse = tree.parse(command, &self.ui.registries);
            let (range, list) = tree.client_suggestions(&parse, command.len(), &self.ui.registries);
            let map = |i: usize| start + i.saturating_sub(skip);
            console::Completions {
                text: text.clone(),
                cursor,
                range: (map(range.0), map(range.1)),
                items: list.into_iter().map(|s| (s.text, s.tooltip)).collect(),
                hint: console::call_context(&text, cursor)
                    .and_then(|c| console::signature_hint(&c.name)),
            }
        };
        let completions = if let Some((start, command)) = console::command_context(&text, cursor) {
            tree_completions(start, &command, 0)
        } else if let Some((start, partial)) = console::entity_context(&text, cursor) {
            // An entity argument completes as /tp's target does.
            tree_completions(start, &format!("tp {partial}"), 3)
        } else {
            console::lua_completions(&text, cursor)
        };
        console::set_completions(completions);
    }

    /// A guest's client said `data` on the tools' channel: read at the next
    /// server tick by `event-horizon-dev-tools` through the store.
    pub(in crate::game) fn guest_dev_tools(
        &mut self,
        host: &crate::game::lan::LanHost,
        conn: u32,
        data: &[u8],
    ) {
        let Some(player) = host.hub.guest(conn).map(|g| g.uuid) else {
            return;
        };
        if let Some(wire) = observe_wire::decode(data) {
            observe::push_server_command(player, wire);
        }
    }

    /// The client's half of the tools' channel: what the store queued for
    /// the server, and `Hello` once when this client joined.
    pub(in crate::game) fn dev_tools_client_tick(&mut self) {
        if !self.remote.as_ref().is_some_and(|r| r.joined) {
            return;
        }
        let mut out: Vec<Wire> = observe::take_outbox();
        if crate::dev_input::installed()
            && !crate::dev_input::HELLO.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            out.insert(0, Wire::Hello);
        }
        for wire in out {
            if let Some(data) = observe_wire::encode(&wire) {
                self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                    horizon_protocol::packets::configuration::CustomPayload {
                        channel: observe_wire::CHANNEL.into(),
                        data,
                    },
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_without_an_id_is_not_watched() {
        assert_eq!(id_of(Target::Player), None);
        assert_eq!(id_of(Target::Mob(7)), Some(7));
    }
}
