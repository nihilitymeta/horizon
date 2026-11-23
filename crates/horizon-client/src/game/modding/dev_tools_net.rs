//! The developer tools' game-side plumbing (docs/dev-tools.md) on the `horizon:dev_tools` channel:
//! the shared store empties when a world leaves, a guest's client tells the server what its
//! windows want, and the server's pages come back to this client's windows. The commands and the
//! windows' server half live in `event-horizon-dev-tools`; the store is `mobs::observe`.
use super::Game;
use horizon_client::mobs::{observe, observe_wire};
use horizon_protocol::packets::{configuration::CustomPayload, play::Serverbound as S};

/// Empties the store: nothing of a world that left stays watched, so a watch on entity 123 of
/// that world never reads the next world's entity 123.
fn close_store() {
    observe::reset();
}

/// A guest's message on the channel, as the player `uuid`'s client said it: read at the server's
/// next tick. A payload that is not a message is dropped.
fn queue_guest_message(uuid: u128, data: &[u8]) {
    if let Some(wire) = observe_wire::decode(data) {
        observe::push_server_command(uuid, wire);
    }
}

/// The server's page on the channel, for this client's windows.
fn show_received(data: &[u8]) {
    if let Some(wire) = observe_wire::decode(data) {
        observe::received(wire);
    }
}

impl Game<'_> {
    /// A world left (`leave_server`): the store empties. The windows' own state is the
    /// `dev_input` half, not here.
    pub(in crate::game) fn dev_tools_world_closed(&mut self) {
        close_store();
    }

    /// A serverbound payload on `horizon:dev_tools` from the guest `conn`.
    pub(in crate::game) fn guest_dev_tools(&mut self, conn: u32, data: &[u8]) {
        let Some(uuid) = self
            .lan
            .as_ref()
            .and_then(|host| host.hub.guest(conn))
            .map(|guest| guest.uuid)
        else {
            return;
        };
        queue_guest_message(uuid, data);
    }

    /// Once a client tick, joined: what the store queued for the server goes out on the channel.
    pub(in crate::game) fn dev_tools_client_tick(&mut self) {
        if !self.remote.as_ref().is_some_and(|r| r.joined) {
            return;
        }
        for wire in observe::take_outbox() {
            if let Some(data) = observe_wire::encode(&wire) {
                self.send(S::CustomPayload(CustomPayload {
                    channel: observe_wire::CHANNEL.into(),
                    data,
                }));
            }
        }
    }

    /// A clientbound payload on `horizon:dev_tools`: the server's page for this client's windows.
    pub(in crate::game) fn remote_dev_tools(&mut self, data: &[u8]) {
        show_received(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use observe::View;

    const PLAYER: u128 = 0x0077_5db2_0000_0000_0000_0000_0000_0001;
    const WATCHED: u32 = 9_077_501;

    /// The store is process-wide and `reset` clears every part of it, so the three checks run in
    /// one test, in order: a client's message reaches the server for its player only when it is a
    /// message, and leaving a world empties the store.
    #[test]
    fn the_channel_feeds_the_server_and_leaving_a_world_empties_the_store() {
        let bytes = observe_wire::encode(&observe_wire::Wire::Hello).expect("a hello fits");
        queue_guest_message(PLAYER, &bytes);
        queue_guest_message(PLAYER, b"\x00not json");
        let queued = observe::take_server_commands();
        let mine: Vec<_> = queued.iter().filter(|(uuid, _)| *uuid == PLAYER).collect();
        assert_eq!(mine.len(), 1, "one message, one queued command: {queued:?}");
        assert_eq!(observe_wire::encode(&mine[0].1), Some(bytes));

        observe::add(&[WATCHED], View::Data);
        assert!(observe::watched().contains(&WATCHED));
        close_store();
        assert!(!observe::watched().contains(&WATCHED));
    }
}
