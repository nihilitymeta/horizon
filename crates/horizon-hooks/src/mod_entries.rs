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
    /// Whether the mod requests at least one power (`Provider::powers` lists them).
    pub powers: bool,
    /// The mod's icon as PNG bytes, when it has one that is readable and at most 256 KiB.
    pub icon_png: Option<Vec<u8>>,
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
    /// Found, and not loaded. `key` is the translation key of the refusal with its `args` in
    /// order (empty when the refusal has no key); `reason` is the English text the log also has.
    Refused {
        key: String,
        args: Vec<(String, String)>,
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

/// One power a mod requests, as the Mods screen shows it. The texts are translation keys:
/// `label_key` and `description_key` name the screen's own entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerView {
    /// The power's name in the manifest and the enabled file, e.g. `"world"`.
    pub name: String,
    pub label_key: String,
    pub description_key: String,
    /// Always `true` for a listed power: only requested powers are listed.
    pub requested: bool,
    /// Whether the enabled file grants it to the mod.
    pub granted: bool,
    /// Whether this build has at least one write behind the power, so a grant does something.
    pub available: bool,
    /// Whether the power is reserved: it may be requested and granted, but it is never effective.
    pub reserved: bool,
}

/// What a setting accepts. Colour and keybind values are text (`#rrggbb` or a key name).
///
/// The value contract, for each kind: which [`ViewValue`] variant carries its `value` and its
/// `default` (the provider sends both), and what `set` accepts. The provider's contract test
/// (`contract_each_kind_carries_its_variant` in `event-horizon-lua`) asserts this table.
///
/// | Kind        | `value` / `default` variant | Notes                                              |
/// |-------------|-----------------------------|----------------------------------------------------|
/// | `Toggle`    | `Bool`                      |                                                    |
/// | `Choice`    | `Text(option)`              | `option` is one of `options`; a stored value that is not one is shown as given |
/// | `Slider`    | `Int` when `integer`, else `Float` |                                             |
/// | `Colour`    | `Text("#rrggbb")`           | `#rrggbbaa` is also accepted by the store          |
/// | `Text`      | `Text`                      | at most `max_length` characters                    |
/// | `Keybind`   | `Text(key name)`            | `[A-Za-z0-9_+ ]`                                   |
#[derive(Clone, Debug, PartialEq)]
pub enum ViewKind {
    /// A switch: `Bool`.
    Toggle,
    /// One of `options`, as `Text(option)`. A click moves to the next option, wrapping. A stored
    /// value that is not in `options` is shown as given, and a click moves to the first option.
    Choice { options: Vec<String> },
    /// A number on a track: `Int` when `integer`, else `Float`.
    Slider {
        min: f64,
        max: f64,
        step: f64,
        integer: bool,
    },
    /// A colour as `Text("#rrggbb")`.
    Colour,
    /// Text of at most `max_length` characters, as `Text`.
    Text { max_length: usize },
    /// A key name, as `Text`.
    Keybind,
}

/// A setting's value. Which variant it is follows the setting's [`ViewKind`] (see its table).
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
    pub const POWER_UNKNOWN: &str = "mods.provider.power_unknown";
    pub const POWER_NOT_REQUESTED: &str = "mods.provider.power_not_requested";
    pub const POWER_UNAVAILABLE: &str = "mods.provider.power_unavailable";
    pub const POWER_NOT_EDITABLE: &str = "mods.provider.power_not_editable";
}

/// What the Lua host offers the Mods screen. Implementations run on the game's thread and must
/// return quickly.
pub trait Provider: Sync {
    /// Every mod: loaded ones in load order, then the disabled, then the refused.
    fn entries(&self) -> Vec<Entry>;
    /// The id and the name of entry `index` in [`Provider::entries`] order, `None` past the end.
    /// A provider that holds icons should override it: this default clones every entry.
    fn lookup(&self, index: usize) -> Option<(String, String)> {
        self.entries()
            .into_iter()
            .nth(index)
            .map(|entry| (entry.id, entry.name))
    }
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
    /// The powers mod `id` requests, in rank order; empty for an unknown mod or one that requests
    /// none. Only requested powers are listed.
    fn powers(&self, _id: &str) -> Vec<PowerView> {
        Vec::new()
    }
    /// Grants (`granted`) or revokes `power` for mod `id`. A grant needs a power the mod requests,
    /// and one this build can use. Nothing changes when this returns a [`Fault`]. The change takes
    /// effect at the next start.
    fn set_power(&self, _id: &str, _power: &str, _granted: bool) -> Result<(), Fault> {
        Err(fault(keys::POWER_NOT_EDITABLE, &[]))
    }
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

    /// The defaults: no power is listed, and a grant is refused with the power-specific key (not the
    /// settings one), nothing changed.
    #[test]
    fn the_default_power_methods_list_nothing_and_refuse_a_grant() {
        assert!(Empty.powers("any").is_empty());
        assert_eq!(
            Empty.set_power("any", "world", true),
            Err(Fault {
                key: keys::POWER_NOT_EDITABLE,
                args: Vec::new(),
            })
        );
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
