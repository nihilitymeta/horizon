//! The developer tools' windows on a guest's own screen (docs/dev-tools.md,
//! "A guest's windows"): what the server and a Horizon client with the
//! windows say to each other. The server sends the page a guest's inspect
//! stick opened, every so often while it is open; the client fills its own
//! `observe` store with it, so its windows draw it as they draw the host's.
//! A message is a custom payload of its own channel, `horizon:dev_tools`,
//! in JSON; only a client that said it shows the windows (`Wire::Hello`) is
//! sent one.
use super::store::{BlockKey, BlockSnapshot, BrainSnapshot, View};
use serde::{Deserialize, Serialize};

/// The channel.
pub const CHANNEL: &str = "horizon:dev_tools";
/// The update intervals a window offers, in milliseconds, and the one a
/// window starts with.
pub const INTERVALS: [u32; 5] = [250, 500, 1000, 2000, 5000];
pub const DEFAULT_INTERVAL: u32 = 1000;
/// The shortest interval the server keeps, whatever a client asks.
pub const FLOOR: u32 = 250;

/// What one side tells the other.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Wire {
    /// Client to server, once it joined: this client shows the windows.
    Hello,
    /// Client to server: its window closed.
    Stop,
    /// Client to server: send the page every this many milliseconds.
    Interval(u32),
    /// Server to client: an entity's page.
    Entity(EntityPage),
    /// Server to client: a block window.
    Block(BlockPage),
    /// Server to client: the window ended (the entity is gone, or the player
    /// went out of range or to another level).
    Closed,
}

/// An entity's page as the server read it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityPage {
    pub id: u32,
    pub view: View,
    pub name: String,
    /// Its data as `/data get entity` prints it, in SNBT.
    pub data: Option<String>,
    pub brain: Option<BrainSnapshot>,
    /// The goals that ran at each AI step (the Goals page's history).
    pub history: Vec<(u32, Vec<String>)>,
    /// The interval the server sends at.
    pub interval: u32,
}

/// A block window as the server read it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockPage {
    pub key: BlockKey,
    pub face: [i32; 3],
    pub snapshot: BlockSnapshot,
    pub interval: u32,
}

/// The interval the server keeps for one a client asked for: at least the floor.
pub fn clamp_interval(ms: u32) -> u32 {
    ms.max(FLOOR)
}

/// The most a clientbound custom payload holds (`ClientboundCustomPayloadPacket`'s 1 MiB).
pub const MAX: usize = 1 << 20;

/// `wire` as a payload of the channel: JSON. None past [`MAX`] (an entity
/// with more data than that is not sent).
pub fn encode(wire: &Wire) -> Option<Vec<u8>> {
    serde_json::to_vec(wire).ok().filter(|b| b.len() <= MAX)
}

/// A payload of the channel, or None for one that is not a message.
pub fn decode(data: &[u8]) -> Option<Wire> {
    serde_json::from_slice(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_comes_back_as_it_was_sent() {
        let page = Wire::Entity(EntityPage {
            id: 7,
            view: View::Goals,
            name: "Pig".into(),
            data: Some("{Health:10.0f}".into()),
            brain: None,
            history: vec![(3, vec!["Float".into()])],
            interval: 1000,
        });
        let Some(Wire::Entity(back)) = decode(&encode(&page).unwrap()) else {
            panic!("not a page");
        };
        assert_eq!((back.id, back.view, back.interval), (7, View::Goals, 1000));
        assert_eq!(back.history, vec![(3, vec!["Float".to_string()])]);
        assert!(decode(b"nonsense").is_none());
        // More than a payload holds is not sent.
        let huge = Wire::Entity(EntityPage {
            data: Some("x".repeat(MAX)),
            ..match page {
                Wire::Entity(p) => p,
                _ => unreachable!(),
            }
        });
        assert!(encode(&huge).is_none());
    }

    #[test]
    fn the_server_keeps_no_interval_under_its_floor() {
        assert_eq!(clamp_interval(10), FLOOR);
        assert_eq!(clamp_interval(250), 250);
        assert_eq!(clamp_interval(5000), 5000);
        assert!(INTERVALS.iter().all(|&i| clamp_interval(i) == i));
    }
}
