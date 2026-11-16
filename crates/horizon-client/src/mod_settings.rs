//! The Lua mods' settings, as the game's own screens see them (#64): every loaded mod that declares
//! a `settings.json`, its entries with their current values, and a write or a reset of one value.
//!
//! The schemas and the files live in `event-horizon-lua`, which this crate does not depend on. The
//! executable installs a [`Provider`] once, at start, the way `native_client_hooks` takes its table:
//! [`list`], [`set`] and [`reset`] call it, and without one `list` is empty and a write is refused.
//! A write runs the mod's `config_changed` hook through the executable's provider.

use serde_json::Value;
use std::sync::OnceLock;

/// The kind of a setting, as its screen draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Text,
    Choice,
}

/// The side whose config file holds a setting's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Server,
    Client,
}

/// One declared setting of a mod, with the value `horizon.config()` gives for it now.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingEntry {
    pub key: String,
    pub kind: Kind,
    pub side: Side,
    pub group: Option<String>,
    pub order: Option<String>,
    /// Kept for a reader that wants it; a screen leaves it out of its list.
    pub hidden: bool,
    /// Whether this game may write the value. The provider sets it; a screen gates on the game's
    /// own state (a remote server's server-side values are not the player's to change).
    pub editable: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Vec<String>,
    pub default: Value,
    /// The effective value: the stored one when it fits the schema, else the default.
    pub value: Value,
}

/// A mod that declares a settings schema, with its entries in display order (group, order, key).
#[derive(Clone, Debug, PartialEq)]
pub struct ModSettings {
    pub mod_id: String,
    pub entries: Vec<SettingEntry>,
}

/// The executable's side of the seam: how to list, write and reset the mods' settings.
pub struct Provider {
    pub list: fn() -> Vec<ModSettings>,
    pub set: fn(&str, Side, &str, Value) -> Result<(), String>,
    pub reset: fn(&str, Side, &str) -> Result<(), String>,
}

static PROVIDER: OnceLock<Provider> = OnceLock::new();

/// Installs the provider. The first one stays; a later call gets its own back.
pub fn install(provider: Provider) -> Result<(), Provider> {
    PROVIDER.set(provider)
}

/// Whether a provider is installed (the game has mods with settings to offer).
pub fn installed() -> bool {
    PROVIDER.get().is_some()
}

/// Every loaded mod that declares settings; empty without a provider.
pub fn list() -> Vec<ModSettings> {
    PROVIDER.get().map_or_else(Vec::new, |p| (p.list)())
}

/// Writes setting `key` of `mod_id`'s `side` to `value`; the provider says why it refuses.
pub fn set(mod_id: &str, side: Side, key: &str, value: Value) -> Result<(), String> {
    let provider = PROVIDER.get().ok_or_else(no_provider)?;
    (provider.set)(mod_id, side, key, value)
}

/// Removes the stored value of setting `key`, so the mod gets its default again.
pub fn reset(mod_id: &str, side: Side, key: &str) -> Result<(), String> {
    let provider = PROVIDER.get().ok_or_else(no_provider)?;
    (provider.reset)(mod_id, side, key)
}

fn no_provider() -> String {
    "no mod settings".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Vec<ModSettings> {
        vec![ModSettings {
            mod_id: "ruby".to_owned(),
            entries: Vec::new(),
        }]
    }

    fn accept(mod_id: &str, side: Side, key: &str, value: Value) -> Result<(), String> {
        if (mod_id, side, key, value.clone()) == ("ruby", Side::Client, "volume", json!(7)) {
            Ok(())
        } else {
            Err(format!("refused {mod_id} {key} {value}"))
        }
    }

    fn accept_reset(mod_id: &str, side: Side, key: &str) -> Result<(), String> {
        accept(mod_id, side, key, json!(7))
    }

    #[test]
    fn mod_settings_without_a_provider_lists_nothing_and_refuses_a_write() {
        crate::mod_api::in_own_process(
            "mod_settings::tests::mod_settings_without_a_provider_lists_nothing_and_refuses_a_write",
            || {
                assert!(!installed());
                assert!(list().is_empty());
                assert_eq!(
                    set("ruby", Side::Client, "volume", json!(7)).unwrap_err(),
                    "no mod settings"
                );
                assert_eq!(
                    reset("ruby", Side::Client, "volume").unwrap_err(),
                    "no mod settings"
                );
            },
        );
    }

    #[test]
    fn mod_settings_with_a_provider_lists_and_writes_through_it() {
        crate::mod_api::in_own_process(
            "mod_settings::tests::mod_settings_with_a_provider_lists_and_writes_through_it",
            || {
                let provider = Provider {
                    list: sample,
                    set: accept,
                    reset: accept_reset,
                };
                assert!(install(provider).is_ok());
                assert!(
                    install(Provider {
                        list: Vec::new,
                        set: accept,
                        reset: accept_reset,
                    })
                    .is_err(),
                    "the first provider stays"
                );
                assert!(installed());
                assert_eq!(list(), sample());
                assert_eq!(set("ruby", Side::Client, "volume", json!(7)), Ok(()));
                assert_eq!(
                    set("ruby", Side::Client, "volume", json!(8)).unwrap_err(),
                    "refused ruby volume 8"
                );
                assert_eq!(reset("ruby", Side::Client, "volume"), Ok(()));
                assert_eq!(
                    reset("ruby", Side::Server, "volume").unwrap_err(),
                    "refused ruby volume 7"
                );
            },
        );
    }
}
