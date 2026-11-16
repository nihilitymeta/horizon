//! The Options page's Lua mod settings, from the game's side (#64): the provider's list turned into
//! the render crate's snapshot (`mod_settings_ui`), the writes and resets the page asks for, and
//! the reading again after each of them.

use super::Game;
use crate::mod_settings::{self as provider, Kind, ModSettings, SettingEntry, Side};
use crate::mod_settings_ui::{Entry, Kind as RKind, ModConfig, Side as RSide, Snapshot};

/// The page's snapshot from the provider's list. `remote`: this game is a client of a server,
/// whose server-side settings the page shows but does not change.
pub(super) fn config_snapshot(list: Vec<ModSettings>, remote: bool) -> Snapshot {
    let mods = list
        .into_iter()
        .map(|m| ModConfig {
            mod_id: m.mod_id,
            entries: m.entries.into_iter().map(entry_of).collect(),
        })
        .collect();
    Snapshot { mods, remote }
}

fn entry_of(e: SettingEntry) -> Entry {
    Entry {
        key: e.key,
        kind: match e.kind {
            Kind::Bool => RKind::Bool,
            Kind::Int => RKind::Int,
            Kind::Float => RKind::Float,
            Kind::Text => RKind::Text,
            Kind::Choice => RKind::Choice,
        },
        side: match e.side {
            Side::Server => RSide::Server,
            Side::Client => RSide::Client,
        },
        group: e.group,
        hidden: e.hidden,
        editable: e.editable,
        min: e.min,
        max: e.max,
        choices: e.choices,
        default: e.default,
        value: e.value,
    }
}

/// The provider's side for the page's side.
pub(super) fn side_of(side: RSide) -> Side {
    match side {
        RSide::Server => Side::Server,
        RSide::Client => Side::Client,
    }
}

/// Writes a setting the Options page changed, through the provider; it says why it refused.
pub(super) fn write_mod_setting(
    mod_id: &str,
    side: RSide,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    provider::set(mod_id, side_of(side), key, value)
}

impl Game<'_> {
    /// Reads the Lua mods' settings again into the Options page. The remote flag is the game's own
    /// state: a client of a server, but not single player on its server thread, nor a host.
    pub(super) fn refresh_mod_config(&mut self) {
        let remote = self.remote.is_some() && self.integrated.is_none();
        self.settings.mods.config = config_snapshot(provider::list(), remote);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_settings::Provider;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicI64, Ordering};

    /// The provider's one stored value: `ruby`'s volume, 0 to 10, 5 by default.
    static VOLUME: AtomicI64 = AtomicI64::new(5);

    fn list() -> Vec<ModSettings> {
        vec![ModSettings {
            mod_id: "ruby".to_owned(),
            entries: vec![SettingEntry {
                key: "volume".to_owned(),
                kind: Kind::Int,
                side: Side::Client,
                group: None,
                order: None,
                hidden: false,
                editable: true,
                min: Some(0.0),
                max: Some(10.0),
                choices: Vec::new(),
                default: json!(5),
                value: json!(VOLUME.load(Ordering::SeqCst)),
            }],
        }]
    }

    fn set(mod_id: &str, side: Side, key: &str, value: Value) -> Result<(), String> {
        if (mod_id, side, key) != ("ruby", Side::Client, "volume") {
            return Err(format!("no setting {mod_id} {key}"));
        }
        let whole = value
            .as_i64()
            .ok_or_else(|| format!("{value} is not whole"))?;
        if (0..=10).contains(&whole) {
            VOLUME.store(whole, Ordering::SeqCst);
            Ok(())
        } else {
            Err(format!("{whole} is out of range"))
        }
    }

    fn reset(mod_id: &str, side: Side, key: &str) -> Result<(), String> {
        set(mod_id, side, key, json!(5))
    }

    /// A write goes through the provider and the page reads it back; a refused one leaves the
    /// stored value, which the page then shows.
    #[test]
    fn mod_settings_write_reaches_the_provider_and_a_refused_one_leaves_the_stored_value() {
        crate::mod_api::in_own_process(
            "game::mod_settings_glue::tests::mod_settings_write_reaches_the_provider_and_a_refused_one_leaves_the_stored_value",
            || {
                assert!(provider::install(Provider { list, set, reset }).is_ok());
                let page = |remote| config_snapshot(provider::list(), remote);
                let volume = |snapshot: &Snapshot| snapshot.mods[0].entries[0].value.clone();
                assert_eq!(volume(&page(false)), json!(5));
                let written = provider::set("ruby", Side::Client, "volume", json!(7));
                assert_eq!(written, Ok(()));
                assert_eq!(volume(&page(false)), json!(7));
                let refused = provider::set("ruby", Side::Client, "volume", json!(42));
                assert_eq!(refused.unwrap_err(), "42 is out of range");
                assert_eq!(volume(&page(false)), json!(7));
                assert_eq!(provider::reset("ruby", Side::Client, "volume"), Ok(()));
                assert_eq!(volume(&page(false)), json!(5));
            },
        );
    }

    /// A client of a remote server sees its server's settings as read-only; the flag is kept.
    #[test]
    fn mod_settings_snapshot_marks_a_remote_clients_server_settings_as_not_writable() {
        let mut server = list();
        server[0].entries[0].side = Side::Server;
        let snapshot = config_snapshot(server, true);
        assert!(snapshot.remote);
        assert!(!snapshot.mods[0].entries[0].writable(snapshot.remote));
        let snapshot = config_snapshot(list(), false);
        assert!(!snapshot.remote);
        assert!(snapshot.mods[0].entries[0].writable(false));
    }
}
