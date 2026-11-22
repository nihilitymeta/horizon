//! The Mods screen's view of the Lua mods, as plain data. The engine (horizon-client-render) lists
//! the mods and reads and changes their settings through [`Provider`] without naming a type of
//! `event-horizon-lua`: the Lua host (platform layer) implements it and registers it before the
//! game runs. With nothing registered, [`engine::provider`] is `None` and the screen shows no Lua
//! mods.
//!
//! Reads come from data the host already holds; nothing here calls into a Lua state. Writes are
//! checked and delivered by the host, which owns the files and the change events.
use crate::{TooLate, registry::Registry};

/// One mod in the list: loaded, turned off in the enabled file, or refused.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub authors: Vec<String>,
    pub description: String,
    pub side: Side,
    /// `false` when the enabled file turns the mod off.
    pub enabled: bool,
    pub status: Status,
    /// Whether the mod declares any setting.
    pub settings: bool,
}

/// Which side of the connection a mod or a setting belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
    Both,
}

/// Whether a mod runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Loaded,
    /// Installed, but the enabled file turns it off.
    Disabled,
    /// Found, and not loaded. `reason` is the English text the log also has.
    Refused {
        reason: String,
    },
}

/// One setting of a mod, as the screen shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingView {
    pub key: String,
    pub label: String,
    pub description: String,
    pub group: String,
    pub side: Side,
    pub kind: ViewKind,
    pub value: ViewValue,
    pub default: ViewValue,
    /// Whether `value` differs from `default`.
    pub changed: bool,
}

/// What a setting accepts. Colour and keybind values are text (`#rrggbb` or a key name).
#[derive(Clone, Debug, PartialEq)]
pub enum ViewKind {
    Toggle,
    Choice {
        options: Vec<String>,
    },
    Slider {
        min: f64,
        max: f64,
        step: f64,
        integer: bool,
    },
    Colour,
    Text {
        max_length: usize,
    },
    Keybind,
}

/// A setting's value.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

/// Why a change was refused: a translation key with its arguments by name. The screen translates
/// it; the English text for a log is in the arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

/// The keys of [`Fault`]. Their texts are not in the game's lang files yet.
pub mod keys {
    pub const UNKNOWN_MOD: &str = "mods.provider.unknown_mod";
    pub const UNKNOWN_SETTING: &str = "mods.provider.unknown_setting";
    pub const NOT_EDITABLE: &str = "mods.provider.not_editable";
    pub const INVALID_VALUE: &str = "mods.provider.invalid_value";
    pub const BUSY: &str = "mods.provider.busy";
    pub const FILE: &str = "mods.provider.file";
}

/// What the Lua host offers the Mods screen. Implementations run on the game's thread and must
/// return quickly.
pub trait Provider: Sync {
    /// Every mod: loaded ones in load order, then the disabled, then the refused.
    fn entries(&self) -> Vec<Entry>;
    /// The settings of mod `id`, in declaration order; empty for an unknown mod.
    fn settings(&self, id: &str) -> Vec<SettingView>;
    /// Sets `key` of mod `id`. Nothing changes when this returns a [`Fault`].
    fn set(&self, id: &str, key: &str, value: ViewValue) -> Result<(), Fault>;
    /// Removes `key` of mod `id` from its file, so it takes its default.
    fn reset(&self, id: &str, key: &str) -> Result<(), Fault>;
    /// Resets every setting of mod `id`.
    fn reset_all(&self, id: &str) -> Result<(), Fault>;
    /// Writes the mod's state to the enabled file. It takes effect at the next start.
    fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), Fault>;
}

/// A [`Fault`] with the key and its named arguments.
pub fn fault(key: &'static str, args: &[(&'static str, &str)]) -> Fault {
    Fault {
        key,
        args: args
            .iter()
            .map(|(name, value)| (*name, (*value).to_owned()))
            .collect(),
    }
}

static PROVIDER: Registry<dyn Provider> = Registry::new();

/// Offers the Mods screen its provider. Call it before the game starts. Only the first provider
/// is used: a later one is refused with [`TooLate`] once the list is sealed, and otherwise
/// ignored.
pub fn register(provider: &'static dyn Provider) -> Result<(), TooLate> {
    PROVIDER.register(provider)
}

/// The first registered provider of `registry`, sealing it.
fn first(registry: &'static Registry<dyn Provider>) -> Option<&'static dyn Provider> {
    registry.all().first().copied()
}

/// The engine's side. Only the engine calls it.
pub mod engine {
    /// The registered provider, sealing the registry on the first call. `None` without one.
    pub fn provider() -> Option<&'static dyn super::Provider> {
        super::first(&super::PROVIDER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Empty;
    impl Provider for Empty {
        fn entries(&self) -> Vec<Entry> {
            Vec::new()
        }
        fn settings(&self, _: &str) -> Vec<SettingView> {
            Vec::new()
        }
        fn set(&self, _: &str, _: &str, _: ViewValue) -> Result<(), Fault> {
            Ok(())
        }
        fn reset(&self, _: &str, _: &str) -> Result<(), Fault> {
            Ok(())
        }
        fn reset_all(&self, _: &str) -> Result<(), Fault> {
            Ok(())
        }
        fn set_enabled(&self, _: &str, _: bool) -> Result<(), Fault> {
            Ok(())
        }
    }

    static FIRST: Empty = Empty;
    static SECOND: Empty = Empty;

    #[test]
    fn with_nothing_registered_there_is_no_provider() {
        static EMPTY: Registry<dyn Provider> = Registry::new();
        assert!(first(&EMPTY).is_none());
    }

    /// The one test that uses the process-wide slot: the first registration wins, and a late one
    /// is refused once the list is sealed.
    #[test]
    fn the_first_registration_wins_and_a_late_one_is_refused() {
        register(&FIRST).unwrap();
        register(&SECOND).unwrap();
        let got = engine::provider().unwrap();
        assert!(std::ptr::addr_eq(got, &FIRST as &dyn Provider));
        assert_eq!(register(&SECOND), Err(TooLate));
    }
}
