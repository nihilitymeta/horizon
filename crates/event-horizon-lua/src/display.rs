//! Owner-only error lines: the one chat line a mod's first problem sends the owner, when
//! `lua-mods/enabled.toml` says `[display] errors = "owner"` (see `enabled.rs`).
//!
//! The default, `Errors::Off`, sends nothing: every problem stays in the log (`host::warn_once`).
//! With `Owner`, the first runtime failure or power refusal of a mod in a session queues one line,
//! `[Lua] <mod>: 1 problem, e.g. "<first message>". See the log.`. Further problems of that mod send
//! nothing, and at most `MAX_LINES` lines go in a session. The lines are queued where the problem is
//! logged (which may be outside a hook turn, with no host at hand) and sent at the start of the next
//! server hook turn, where the owner (`dev_server_owner`, the single-player or LAN host's player) is
//! reachable. On a dedicated server there is no owner: the queue is dropped and nothing is sent.

use std::collections::BTreeSet;
use std::sync::Mutex;

use mod_api::{ServerHost, Text};

/// What `enabled.toml`'s `[display] errors` says: the default `Off`, or `Owner`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Errors {
    #[default]
    Off,
    Owner,
}

/// The most lines one session sends the owner, over all mods.
pub const MAX_LINES: usize = 5;
/// How many characters of a problem's first message the line shows.
const SHOWN_CHARS: usize = 80;

/// What the owner reads of a logged problem: its first line, without the mod's id, and for a script error
/// without the file path: `alpha: tick: runtime error: /game/lua-mods/alpha/main.luau:2: boom` shows as
/// `tick: boom`. The log line has the rest.
fn readable(source: &str, line: &str) -> String {
    let first = line.lines().next().unwrap_or("");
    // The log escapes a newline as `\u{a}`: the traceback after it is not for the owner.
    let first = first.split("\\u{a}").next().unwrap_or(first);
    let text = first.strip_prefix(&format!("{source}: ")).unwrap_or(first);
    if let Some((hook, rest)) = text.split_once(": ")
        && let Some(at) = rest.find(".luau:")
        && let Some((_, message)) = rest[at..].split_once(": ")
    {
        return format!("{hook}: {message}");
    }
    text.to_owned()
}

/// The session's state: the mode, the mods that already have their line, the lines counted, and
/// the lines queued for the next turn.
#[derive(Debug)]
pub struct Session {
    mode: Errors,
    mods: BTreeSet<String>,
    lines: usize,
    pending: Vec<String>,
}

/// The one place a line can go: the owner's uuid, and a chat line to that player. `tell` is true when
/// the line reached the player (they are online).
pub trait Owner {
    fn owner(&self) -> Option<u128>;
    fn tell(&mut self, uuid: u128, line: &str) -> bool;
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub const fn new() -> Session {
        Session {
            mode: Errors::Off,
            mods: BTreeSet::new(),
            lines: 0,
            pending: Vec::new(),
        }
    }

    pub fn set_mode(&mut self, mode: Errors) {
        self.mode = mode;
    }

    /// Notes a problem of mod `source`. Queues its line when the mode is `Owner`, the mod has none yet
    /// and the session has room; returns whether it did.
    pub fn problem(&mut self, source: &str, line: &str) -> bool {
        if self.mode != Errors::Owner || self.lines >= MAX_LINES || self.mods.contains(source) {
            return false;
        }
        self.mods.insert(source.to_owned());
        self.lines += 1;
        let shown: String = readable(source, line)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(SHOWN_CHARS)
            .collect();
        self.pending.push(format!(
            "[Lua] {source}: 1 problem, e.g. \"{shown}\". See the log."
        ));
        true
    }

    /// Sends the queued lines to the owner. With no owner the queue is dropped; a line the owner
    /// cannot take (not online) stays queued for a later turn.
    pub fn flush(&mut self, owner: &mut dyn Owner) {
        if self.pending.is_empty() {
            return;
        }
        let Some(uuid) = owner.owner() else {
            self.pending.clear();
            return;
        };
        self.pending.retain(|line| !owner.tell(uuid, line));
    }
}

/// The process's session. Its lock is held only for the bookkeeping, never across a host call that
/// could log a problem.
static SESSION: Mutex<Session> = Mutex::new(Session::new());

fn session() -> std::sync::MutexGuard<'static, Session> {
    SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sets the mode from a read of `enabled.toml`.
pub fn set_mode(mode: Errors) {
    session().set_mode(mode);
}

/// Notes a mod's runtime problem or refusal for the owner's line (see [`Session::problem`]).
pub(crate) fn problem(source: &str, line: &str) {
    session().problem(source, line);
}

/// Sends the queued lines through a server hook's host, at the start of its turn.
pub(crate) fn flush_turn(host: &mut dyn ServerHost) {
    let mut session = session();
    if !session.pending.is_empty() {
        session.flush(&mut ServerOwner(host));
    }
}

/// The owner as a server host knows it: its uuid, and its chat.
struct ServerOwner<'a>(&'a mut dyn ServerHost);

impl Owner for ServerOwner<'_> {
    fn owner(&self) -> Option<u128> {
        self.0.dev_server_owner()
    }

    fn tell(&mut self, uuid: u128, line: &str) -> bool {
        let Some(player) = self.0.player(uuid) else {
            return false;
        };
        matches!(player.tell(self.0, Text::literal(line)), Ok(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An owner who is online, and every line it was sent.
    struct Online {
        uuid: Option<u128>,
        present: bool,
        sent: Vec<String>,
    }

    impl Owner for Online {
        fn owner(&self) -> Option<u128> {
            self.uuid
        }

        fn tell(&mut self, _uuid: u128, line: &str) -> bool {
            if self.present {
                self.sent.push(line.to_owned());
            }
            self.present
        }
    }

    fn owned() -> Session {
        let mut session = Session::new();
        session.set_mode(Errors::Owner);
        session
    }

    fn sent_to(session: &mut Session, owner: &mut Online) {
        session.flush(owner);
    }

    #[test]
    fn off_queues_nothing_for_any_problem() {
        let mut session = Session::new();
        assert!(!session.problem("alpha", "alpha: hook: boom"));
        let mut owner = Online {
            uuid: Some(1),
            present: true,
            sent: vec![],
        };
        sent_to(&mut session, &mut owner);
        assert!(owner.sent.is_empty());
    }

    #[test]
    fn a_mod_gets_one_line_and_a_second_problem_of_it_is_refused() {
        let mut session = owned();
        assert!(session.problem("alpha", "alpha: tick: boom"));
        assert!(!session.problem("alpha", "alpha: tick: boom again"));
        assert!(!session.problem("alpha", "alpha: world refused"));
        let mut owner = Online {
            uuid: Some(1),
            present: true,
            sent: vec![],
        };
        sent_to(&mut session, &mut owner);
        assert_eq!(
            owner.sent,
            vec!["[Lua] alpha: 1 problem, e.g. \"tick: boom\". See the log.".to_owned()]
        );
    }

    #[test]
    fn another_mod_gets_its_own_first_line_and_the_session_stops_at_five() {
        let mut session = owned();
        for id in ["a", "b", "c", "d", "e"] {
            assert!(session.problem(id, "x"));
        }
        assert!(
            !session.problem("f", "x"),
            "a sixth mod is over the session's five lines"
        );
        assert!(!session.problem("a", "x"));
    }

    #[test]
    fn no_owner_sends_nothing_and_drops_the_queue() {
        let mut session = owned();
        assert!(session.problem("alpha", "boom"));
        let mut dedicated = Online {
            uuid: None,
            present: true,
            sent: vec![],
        };
        sent_to(&mut session, &mut dedicated);
        assert!(dedicated.sent.is_empty());
        assert!(session.pending.is_empty());
    }

    #[test]
    fn a_line_for_an_offline_owner_waits_for_a_later_turn() {
        let mut session = owned();
        assert!(session.problem("alpha", "boom"));
        let mut away = Online {
            uuid: Some(1),
            present: false,
            sent: vec![],
        };
        sent_to(&mut session, &mut away);
        assert_eq!(session.pending.len(), 1);
        let mut back = Online {
            uuid: Some(1),
            present: true,
            sent: vec![],
        };
        sent_to(&mut session, &mut back);
        assert_eq!(back.sent.len(), 1);
        assert!(session.pending.is_empty());
    }

    #[test]
    fn one_refusal_delivers_one_line_to_the_owner_and_off_delivers_none() {
        let mut on = owned();
        assert!(on.problem("alpha", "alpha: set_weather: world power refused"));
        let mut owner = Online {
            uuid: Some(7),
            present: true,
            sent: vec![],
        };
        sent_to(&mut on, &mut owner);
        assert_eq!(owner.sent.len(), 1);

        let mut off = Session::new();
        assert!(!off.problem("alpha", "alpha: set_weather: world power refused"));
        let mut owner = Online {
            uuid: Some(7),
            present: true,
            sent: vec![],
        };
        sent_to(&mut off, &mut owner);
        assert!(owner.sent.is_empty());
    }

    #[test]
    fn a_script_error_shows_the_hook_and_message_without_the_path_or_traceback() {
        assert_eq!(
            readable(
                "alpha",
                "alpha: loot_drops: runtime error: /game/lua-mods/alpha/main.luau:2: boom\\u{a}stack traceback:\\u{a}\\t[C]: in ?"
            ),
            "loot_drops: boom"
        );
        assert_eq!(
            readable(
                "alpha",
                "alpha: a host call needs the \"world\" power; refused"
            ),
            "a host call needs the \"world\" power; refused"
        );
    }

    #[test]
    fn control_characters_and_length_are_bounded_in_the_line() {
        let mut session = owned();
        let long = format!("first\nline {}", "x".repeat(200));
        assert!(session.problem("alpha", &long));
        let line = &session.pending[0];
        assert!(!line.contains('\n'));
        assert!(line.chars().count() < 80 + 60);
    }
}
