//! Mod-to-mod events for server-side Lua mods (issue 63): a mod names an
//! event under its own id and hands it a JSON-like table (`horizon.emit`),
//! and every mod listening for that name hears it (`horizon.on_event`).
//!
//! Delivery is queued, never inline: the emit pushes onto this queue and the
//! end of the server tick (`drain_mod_events`) fires `native_mod_event` once
//! per event, so a listener runs after the handler that emitted, never inside
//! it. A listener that emits queues for the same drain; each event carries the
//! depth it was emitted at, and one past [`MAX_DEPTH`] is dropped (and logged
//! once a tick), so a listener that re-emits forever stops instead of holding
//! the tick.
//!
//! The queue holds each payload as its JSON text, not as a Lua table: every
//! listening state decodes its own copy, so no two states share a table, and
//! the size cap is measured on the serialized text.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use serde_json::Value;

use super::Game;

/// How large an event's payload may be, serialized as JSON (`horizon.emit`):
/// an emit past it returns false and queues nothing.
pub(super) const MAX_PAYLOAD: usize = 64 * 1024;
/// How deeply one tick's events may chain: an event a listener emits is one
/// deeper than the event it heard, and one past this is dropped, so a listener
/// that re-emits forever stops instead of holding the tick.
pub(super) const MAX_DEPTH: u32 = 32;

/// One queued event: its name (`<mod>:<name>`), its payload as JSON text, and
/// the depth it was emitted at.
pub(super) struct Queued {
    depth: u32,
    pub(super) event: String,
    pub(super) payload: String,
}

/// The events the server tick has queued but not delivered.
#[derive(Default)]
pub(super) struct Queue {
    pending: RefCell<VecDeque<Queued>>,
    /// The depth of the event being delivered (0 outside the drain): an emit
    /// during the drain queues one deeper.
    depth: Cell<u32>,
    /// Whether the drain is running: only then does an emit chain deeper.
    draining: Cell<bool>,
    /// Whether a depth refusal was already logged this tick, so a loop logs
    /// once instead of once per dropped event.
    capped: Cell<bool>,
}

impl Queue {
    /// Queues `payload` as the event `mod_id` names by `event` (`true` when
    /// queued): the name is the mod's own (`<mod_id>:<event>`, or the event
    /// already prefixed with `mod_id`), the payload's JSON text must fit
    /// [`MAX_PAYLOAD`], and the emit must not chain past [`MAX_DEPTH`].
    pub(super) fn emit(&self, mod_id: &str, event: &str, payload: &Value) -> bool {
        let Some(event) = own_event(mod_id, event) else {
            return false;
        };
        let Ok(payload) = serde_json::to_string(payload) else {
            return false;
        };
        if payload.len() > MAX_PAYLOAD {
            return false;
        }
        let depth = if self.draining.get() {
            self.depth.get() + 1
        } else {
            0
        };
        if depth > MAX_DEPTH {
            if !self.capped.replace(true) {
                log::warn!(
                    "mod events: an event emitted deeper than {MAX_DEPTH} in one tick; \
                     the rest are dropped"
                );
            }
            return false;
        }
        self.pending.borrow_mut().push_back(Queued {
            depth,
            event,
            payload,
        });
        true
    }

    /// The drain starts: later emits chain one deeper than the event they
    /// hear, and this tick's refusal log starts over.
    fn begin(&self) {
        self.draining.set(true);
        self.depth.set(0);
        self.capped.set(false);
    }

    /// The next queued event, oldest first.
    fn next(&self) -> Option<Queued> {
        self.pending.borrow_mut().pop_front()
    }

    /// The depth a listener of the event in hand emits at.
    fn set_depth(&self, depth: u32) {
        self.depth.set(depth);
    }

    fn end(&self) {
        self.draining.set(false);
        self.depth.set(0);
    }
}

/// The full name of `event` as `mod_id` may emit it: its own name prefixed
/// with its id, or the name already so prefixed; `None` for an empty name or
/// one another mod's id prefixed.
pub(super) fn own_event(mod_id: &str, event: &str) -> Option<String> {
    if event.is_empty() {
        return None;
    }
    match event.split_once(':') {
        Some((prefix, name)) if !name.is_empty() => (prefix == mod_id).then(|| event.to_owned()),
        Some(_) => None,
        None => Some(format!("{mod_id}:{event}")),
    }
}

impl Game<'_> {
    /// Queues the event `mod_id` (the calling Lua mod) names by `event`,
    /// with the JSON payload `payload` (`horizon.emit`): false when the name
    /// is another mod's, the payload has no JSON form (the bridge refused it
    /// first), or it is too large. The queue is delivered at the end of this
    /// tick (`drain_mod_events`). API @API:mod_events@.
    pub(super) fn native_emit(&self, mod_id: &str, event: &str, payload: &Value) -> bool {
        self.mod_events.emit(mod_id, event, payload)
    }

    /// Delivers the events this tick queued, oldest first, each through
    /// `native_mod_event`: the end of the server tick's tail, after the tick
    /// took every action it was going to. A listener that emits queues for
    /// this same drain.
    pub(super) fn drain_mod_events(&mut self) {
        self.mod_events.begin();
        while let Some(queued) = self.mod_events.next() {
            self.mod_events.set_depth(queued.depth);
            // The queue only holds what it serialized itself, so a decode
            // failure means the text was edited in between: say so rather
            // than deliver a hole.
            match serde_json::from_str::<Value>(&queued.payload) {
                Ok(payload) => self.native_mod_event(&queued.event, &payload),
                Err(e) => log::warn!("mod event {}: payload undecodable: {e}", queued.event),
            }
        }
        self.mod_events.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> Queue {
        Queue::default()
    }

    #[test]
    fn mod_event_emit_takes_the_mods_own_name_and_refuses_another() {
        assert_eq!(
            own_event("ruby", "hello").as_deref(),
            Some("ruby:hello"),
            "a bare name is the mod's own"
        );
        assert_eq!(
            own_event("ruby", "ruby:hello").as_deref(),
            Some("ruby:hello"),
            "the mod's own prefix is kept"
        );
        assert_eq!(
            own_event("ruby", "other:hello"),
            None,
            "another mod's event is refused"
        );
        assert_eq!(own_event("ruby", ""), None, "an empty name is refused");
        assert_eq!(
            own_event("ruby", "ruby:"),
            None,
            "an empty event is refused"
        );
    }

    #[test]
    fn mod_event_emit_queues_until_the_drain_and_keeps_the_payload() {
        let q = queue();
        let payload =
            serde_json::json!({"n": 7, "on": true, "who": "ruby", "nested": {"at": [1, 2]}});
        assert!(q.emit("ruby", "hello", &payload));
        assert!(
            q.pending.borrow().len() == 1,
            "queued, not delivered: the queue has no listener to call"
        );
        let queued = q.next().expect("the drain takes it");
        assert_eq!(queued.event, "ruby:hello");
        assert_eq!(queued.depth, 0, "an ordinary handler emits at depth 0");
        let back: Value = serde_json::from_str(&queued.payload).expect("valid JSON");
        assert_eq!(back, payload, "the payload crosses as sent");
    }

    #[test]
    fn mod_event_payload_over_the_size_cap_queues_nothing() {
        let q = queue();
        let at_cap = Value::String("x".repeat(MAX_PAYLOAD - 2));
        assert!(
            q.emit("ruby", "big", &at_cap),
            "the cap itself fits (with its quotes)"
        );
        let over = Value::String("x".repeat(MAX_PAYLOAD));
        assert!(!q.emit("ruby", "bigger", &over), "an over-cap emit refuses");
        assert!(
            q.next().is_some() && q.next().is_none(),
            "only the fitting payload queued"
        );
    }

    #[test]
    fn mod_event_a_listener_that_emits_is_delivered_in_the_same_drain() {
        let q = queue();
        // The handler that emitted ran outside the drain: its event is depth 0.
        assert!(q.emit("ruby", "first", &serde_json::json!(1)));
        q.begin();
        let mut delivered = Vec::new();
        while let Some(queued) = q.next() {
            delivered.push(queued.event.clone());
            q.set_depth(queued.depth);
            // The listener of "first" emits "second": one deeper, same drain.
            if queued.event == "ruby:first" {
                assert!(q.emit("ruby", "second", &serde_json::json!(2)));
            }
        }
        q.end();
        assert_eq!(delivered, ["ruby:first", "ruby:second"]);
    }

    #[test]
    fn mod_event_a_self_loop_stops_at_the_depth_cap() {
        let q = queue();
        assert!(q.emit("ruby", "loop", &serde_json::json!(0)));
        q.begin();
        let mut delivered = 0;
        while let Some(queued) = q.next() {
            delivered += 1;
            q.set_depth(queued.depth);
            // The listener re-emits its own event every time it hears it.
            q.emit("ruby", "loop", &serde_json::json!(delivered as u32));
        }
        q.end();
        assert_eq!(
            delivered,
            MAX_DEPTH as usize + 1,
            "depths 0 through {MAX_DEPTH} deliver, the emit after refuses"
        );
        assert!(q.capped.get(), "the refusal is logged once");
        assert!(
            q.pending.borrow().is_empty(),
            "a dropped emit queues nothing"
        );
    }
}
