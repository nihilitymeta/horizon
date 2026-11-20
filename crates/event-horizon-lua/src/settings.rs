//! Per-mod settings: the schema a mod declares in `[[settings]]` of its manifest, the values a player
//! set in `<game_dir>/config/<mod_id>/settings.toml`, and the provider API the Mods screen calls to
//! read and change them.
//!
//! Load never refuses a mod. A file that is absent is the defaults. A file that cannot be read, is a
//! symlink, is over 64 KiB, is not UTF-8 or not TOML gives the defaults with one warning for the
//! file. An unknown key is ignored with one warning for the file. A value of the wrong type, or out
//! of its range or off its step, gives that key's default with one warning for the key.
//!
//! `set` and `reset` check the value against the schema, then rewrite the whole file through a
//! temporary file that is synced and renamed over it, so a reader sees the old text or the new text.
//! Writers in this process are serialised by a mutex and each re-reads the file under it, so two
//! changes at once keep both. A file that cannot be read as TOML is not overwritten. Memory changes
//! only after the file has.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::host::warn_once;
use crate::manifest::{MAX_SETTINGS, valid_id};

/// The folder under the game directory that holds one folder per mod.
pub const CONFIG_DIR: &str = "config";
/// The file's name, under the mod's folder.
pub const FILE: &str = "settings.toml";
/// The largest file that is read; a bigger one is ignored, and so is a bigger write.
pub const MAX_FILE: u64 = 64 * 1024;
/// The longest key, in bytes. Keys match `[a-z][a-z0-9_]*`.
pub const MAX_KEY: usize = 32;
pub const MAX_LABEL: usize = 64;
pub const MAX_DESCRIPTION: usize = 256;
pub const MAX_GROUP: usize = 32;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 32;
/// The longest choice option, in bytes.
pub const MAX_OPTION: usize = 32;
/// The most steps a slider may have: `(max - min) / step` at most this.
pub const MAX_STEPS: f64 = 100_000.0;
/// The `max_length` of a text setting when it gives none.
pub const DEFAULT_TEXT_LENGTH: usize = 64;
pub const MAX_TEXT_LENGTH: usize = 256;
/// The longest keybind name, in bytes. Keybind names are checked against `[A-Za-z0-9_+ ]` only.
pub const MAX_KEYBIND: usize = 32;

/// The fields every setting may have.
const COMMON_FIELDS: [&str; 7] = [
    "key",
    "type",
    "label",
    "description",
    "default",
    "side",
    "group",
];

/// Which side of the connection a setting belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingSide {
    Client,
    Server,
    Both,
}

/// The process a `Settings` belongs to. A server-side setting is not editable from a pure client;
/// the process cannot yet tell a pure client from an integrated game, so `builtin` passes
/// `ServerRuns` for every process, and only a caller that knows it is a pure client passes
/// `PureClient`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    ServerRuns,
    PureClient,
}

/// A typed setting value. A slider that is an integer holds `Int`; a float slider holds `Float`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

/// What a setting accepts.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
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

/// One declared setting, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub key: String,
    pub label: String,
    pub description: String,
    pub group: String,
    pub side: SettingSide,
    pub kind: Kind,
    pub default: Value,
}

/// The checked schema of one mod, in declaration order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schema {
    settings: Vec<Setting>,
}

impl Schema {
    /// Checks the `[[settings]]` tables of a manifest. The first problem refuses the mod.
    pub fn parse(tables: &[toml::Table]) -> Result<Schema, SettingFault> {
        let mut settings: Vec<Setting> = Vec::with_capacity(tables.len().min(MAX_SETTINGS));
        for table in tables {
            let setting = Setting::parse(table)?;
            if settings.iter().any(|other| other.key == setting.key) {
                return Err(fault(Reason::DuplicateKey, &setting.key, ""));
            }
            settings.push(setting);
        }
        Ok(Schema { settings })
    }

    pub fn settings(&self) -> &[Setting] {
        &self.settings
    }

    pub fn get(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|setting| setting.key == key)
    }
}

/// Why a schema is refused. `key` is the setting's key as far as it was read (sanitised), `field`
/// the field or type it concerns, empty when none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingFault {
    pub reason: Reason,
    pub key: String,
    pub field: String,
}

/// The kinds of schema refusal. Each has its own translation key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    BadKey,
    DuplicateKey,
    UnknownField,
    PatternUnsupported,
    UnknownType,
    MissingField,
    WrongType,
    BadText,
    BadSide,
    BadOptions,
    BadRange,
    IntegerMismatch,
    BadDefault,
    BadColour,
    BadTextLength,
    BadKeybind,
}

impl SettingFault {
    /// The translation key of the refusal's text.
    pub fn key(&self) -> &'static str {
        match self.reason {
            Reason::BadKey => "lua_mod.refused.setting_bad_key",
            Reason::DuplicateKey => "lua_mod.refused.setting_duplicate_key",
            Reason::UnknownField => "lua_mod.refused.setting_unknown_field",
            Reason::PatternUnsupported => "lua_mod.refused.setting_pattern",
            Reason::UnknownType => "lua_mod.refused.setting_unknown_type",
            Reason::MissingField => "lua_mod.refused.setting_missing_field",
            Reason::WrongType => "lua_mod.refused.setting_wrong_type",
            Reason::BadText => "lua_mod.refused.setting_bad_text",
            Reason::BadSide => "lua_mod.refused.setting_bad_side",
            Reason::BadOptions => "lua_mod.refused.setting_bad_options",
            Reason::BadRange => "lua_mod.refused.setting_bad_range",
            Reason::IntegerMismatch => "lua_mod.refused.setting_integer_mismatch",
            Reason::BadDefault => "lua_mod.refused.setting_bad_default",
            Reason::BadColour => "lua_mod.refused.setting_bad_colour",
            Reason::BadTextLength => "lua_mod.refused.setting_bad_text_length",
            Reason::BadKeybind => "lua_mod.refused.setting_bad_keybind",
        }
    }

    /// The English text for a log.
    pub fn reason(&self) -> String {
        let (key, field) = (&self.key, &self.field);
        match self.reason {
            Reason::BadKey => format!("setting key {key:?} is not [a-z][a-z0-9_]* up to 32 bytes"),
            Reason::DuplicateKey => format!("setting key {key:?} is declared twice"),
            Reason::UnknownField => format!("setting {key:?} has an unknown field {field:?}"),
            Reason::PatternUnsupported => format!("setting {key:?} has a pattern; not supported"),
            Reason::UnknownType => format!("setting {key:?} has an unknown type {field:?}"),
            Reason::MissingField => format!("setting {key:?} lacks the field {field:?}"),
            Reason::WrongType => format!("setting {key:?} has a wrong type for {field:?}"),
            Reason::BadText => format!("setting {key:?} has a bad {field:?} text"),
            Reason::BadSide => {
                format!("setting {key:?} has a side that is not client, server or both")
            }
            Reason::BadOptions => format!("setting {key:?} has bad options"),
            Reason::BadRange => format!("setting {key:?} has a range or step that is not valid"),
            Reason::IntegerMismatch => {
                format!("setting {key:?} is integer, but min, max and step are not all integers")
            }
            Reason::BadDefault => format!("setting {key:?} has a default that is not valid for it"),
            Reason::BadColour => {
                format!("setting {key:?} has a colour that is not #rrggbb or #rrggbbaa")
            }
            Reason::BadTextLength => format!("setting {key:?} has a max_length outside 1..=256"),
            Reason::BadKeybind => format!("setting {key:?} has a keybind name that is not valid"),
        }
    }

    /// The arguments the translation takes, by name.
    pub fn args(&self) -> Vec<(&'static str, String)> {
        vec![("key", self.key.clone()), ("field", self.field.clone())]
    }
}

/// Builds a fault. Text from the mod is cut to 64 characters and its control characters replaced,
/// so a refusal line cannot be broken by it.
fn fault(reason: Reason, key: &str, field: &str) -> SettingFault {
    SettingFault {
        reason,
        key: clean(key),
        field: clean(field),
    }
}

fn clean(text: &str) -> String {
    text.chars()
        .take(64)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

impl Setting {
    fn parse(table: &toml::Table) -> Result<Setting, SettingFault> {
        let key = match table.get("key") {
            Some(toml::Value::String(text)) if valid_key(text) => text.clone(),
            Some(toml::Value::String(text)) => return Err(fault(Reason::BadKey, text, "")),
            Some(_) => return Err(fault(Reason::WrongType, "", "key")),
            None => return Err(fault(Reason::MissingField, "", "key")),
        };
        let kind_name = match table.get("type") {
            Some(toml::Value::String(text)) => text.as_str(),
            Some(_) => return Err(fault(Reason::WrongType, &key, "type")),
            None => return Err(fault(Reason::MissingField, &key, "type")),
        };
        let extra: &[&str] = match kind_name {
            "toggle" | "colour" | "keybind" => &[],
            "choice" => &["options"],
            "slider" => &["min", "max", "step", "integer"],
            "text" => &["max_length"],
            _ => return Err(fault(Reason::UnknownType, &key, kind_name)),
        };
        for field in table.keys() {
            if COMMON_FIELDS.contains(&field.as_str()) || extra.contains(&field.as_str()) {
                continue;
            }
            if field == "pattern" {
                return Err(fault(Reason::PatternUnsupported, &key, field));
            }
            return Err(fault(Reason::UnknownField, &key, field));
        }

        let label = required_text(table, &key, "label", MAX_LABEL)?;
        let description = optional_text(table, &key, "description", MAX_DESCRIPTION)?;
        let group = optional_text(table, &key, "group", MAX_GROUP)?;
        let side = match optional_str(table, &key, "side")? {
            None | Some("both") => SettingSide::Both,
            Some("client") => SettingSide::Client,
            Some("server") => SettingSide::Server,
            Some(_) => return Err(fault(Reason::BadSide, &key, "side")),
        };
        let kind = Kind::parse(kind_name, table, &key)?;
        let raw = table
            .get("default")
            .ok_or_else(|| fault(Reason::MissingField, &key, "default"))?;
        let default = from_toml(raw)
            .and_then(|value| kind.check(&value).ok())
            .ok_or_else(|| fault(Reason::BadDefault, &key, "default"))?;
        Ok(Setting {
            key,
            label,
            description,
            group,
            side,
            kind,
            default,
        })
    }

    /// The value `value` takes for this setting, or why it is not one (the English reason, for a
    /// log). A float slider takes an integer and gives a float.
    pub fn check(&self, value: &Value) -> Result<Value, String> {
        self.kind.check(value)
    }
}

impl Kind {
    fn parse(name: &str, table: &toml::Table, key: &str) -> Result<Kind, SettingFault> {
        Ok(match name {
            "toggle" => Kind::Toggle,
            "colour" => Kind::Colour,
            "keybind" => Kind::Keybind,
            "choice" => Kind::Choice {
                options: options(table, key)?,
            },
            "text" => {
                let max_length = match table.get("max_length") {
                    None => DEFAULT_TEXT_LENGTH,
                    Some(toml::Value::Integer(n)) if (1..=MAX_TEXT_LENGTH as i64).contains(n) => {
                        *n as usize
                    }
                    Some(toml::Value::Integer(_)) => {
                        return Err(fault(Reason::BadTextLength, key, "max_length"));
                    }
                    Some(_) => return Err(fault(Reason::WrongType, key, "max_length")),
                };
                Kind::Text { max_length }
            }
            _ => slider(table, key)?,
        })
    }

    /// The value `value` takes for this kind, or the English reason it is refused.
    pub fn check(&self, value: &Value) -> Result<Value, String> {
        match (self, value) {
            (Kind::Toggle, Value::Bool(on)) => Ok(Value::Bool(*on)),
            (Kind::Choice { options }, Value::Str(text)) => {
                if options.contains(text) {
                    Ok(Value::Str(text.clone()))
                } else {
                    Err("is not one of its options".into())
                }
            }
            (
                Kind::Slider {
                    min,
                    max,
                    step,
                    integer,
                },
                Value::Int(n),
            ) => {
                let v = *n as f64;
                check_grid(v, *min, *max, *step)?;
                Ok(if *integer {
                    Value::Int(*n)
                } else {
                    Value::Float(v)
                })
            }
            (
                Kind::Slider {
                    min,
                    max,
                    step,
                    integer: false,
                },
                Value::Float(v),
            ) => {
                if !v.is_finite() {
                    return Err("is not a finite number".into());
                }
                check_grid(*v, *min, *max, *step)?;
                Ok(Value::Float(*v))
            }
            (Kind::Colour, Value::Str(text)) => {
                if valid_colour(text) {
                    Ok(Value::Str(text.clone()))
                } else {
                    Err("is not #rrggbb or #rrggbbaa".into())
                }
            }
            (Kind::Text { max_length }, Value::Str(text)) => {
                if text.chars().count() > *max_length {
                    Err(format!("is longer than {max_length} characters"))
                } else if text.chars().any(char::is_control) {
                    Err("holds a control character".into())
                } else {
                    Ok(Value::Str(text.clone()))
                }
            }
            (Kind::Keybind, Value::Str(text)) => {
                if valid_keybind(text) {
                    Ok(Value::Str(text.clone()))
                } else {
                    Err("is not a key name of [A-Za-z0-9_+ ]".into())
                }
            }
            _ => Err("has the wrong type".into()),
        }
    }
}

/// Parses a slider's bounds. `integer = true` needs all three to be integers, and then the values
/// are integers too; otherwise the slider holds floats.
fn slider(table: &toml::Table, key: &str) -> Result<Kind, SettingFault> {
    let integer = match table.get("integer") {
        None => false,
        Some(toml::Value::Boolean(on)) => *on,
        Some(_) => return Err(fault(Reason::WrongType, key, "integer")),
    };
    let min = number(table, key, "min")?;
    let max = number(table, key, "max")?;
    let step = number(table, key, "step")?;
    if integer && !(is_int(min) && is_int(max) && is_int(step)) {
        return Err(fault(Reason::IntegerMismatch, key, "integer"));
    }
    let (min, max, step) = (min.as_f64(), max.as_f64(), step.as_f64());
    if !(min.is_finite() && max.is_finite() && step.is_finite()) || min >= max || step <= 0.0 {
        return Err(fault(Reason::BadRange, key, "min"));
    }
    if (max - min) / step > MAX_STEPS {
        return Err(fault(Reason::BadRange, key, "step"));
    }
    Ok(Kind::Slider {
        min,
        max,
        step,
        integer,
    })
}

/// The options of a choice: 2 to 32 distinct non-empty strings, each at most 32 bytes, no control
/// characters.
fn options(table: &toml::Table, key: &str) -> Result<Vec<String>, SettingFault> {
    let Some(value) = table.get("options") else {
        return Err(fault(Reason::MissingField, key, "options"));
    };
    let toml::Value::Array(items) = value else {
        return Err(fault(Reason::WrongType, key, "options"));
    };
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&items.len()) {
        return Err(fault(Reason::BadOptions, key, "options"));
    }
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let toml::Value::String(text) = item else {
            return Err(fault(Reason::BadOptions, key, "options"));
        };
        let bad = text.is_empty()
            || text.len() > MAX_OPTION
            || text.chars().any(char::is_control)
            || out.contains(text);
        if bad {
            return Err(fault(Reason::BadOptions, key, "options"));
        }
        out.push(text.clone());
    }
    Ok(out)
}

/// A number from the schema: a TOML integer or float.
#[derive(Clone, Copy)]
enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn as_f64(self) -> f64 {
        match self {
            Num::Int(n) => n as f64,
            Num::Float(v) => v,
        }
    }
}

fn is_int(num: Num) -> bool {
    matches!(num, Num::Int(_))
}

fn number(table: &toml::Table, key: &str, field: &str) -> Result<Num, SettingFault> {
    match table.get(field) {
        None => Err(fault(Reason::MissingField, key, field)),
        Some(toml::Value::Integer(n)) => Ok(Num::Int(*n)),
        Some(toml::Value::Float(v)) => Ok(Num::Float(*v)),
        Some(_) => Err(fault(Reason::WrongType, key, field)),
    }
}

/// Checks that `v` is one of the points `min + i * step` that are at most `max`.
fn check_grid(v: f64, min: f64, max: f64, step: f64) -> Result<(), String> {
    if v < min || v > max {
        return Err("is outside the slider's range".into());
    }
    let steps = (v - min) / step;
    if (steps - steps.round()).abs() > 1e-9 {
        return Err("is not on the slider's step".into());
    }
    Ok(())
}

/// The value of a TOML scalar, or `None` for a table, an array or a date.
fn from_toml(raw: &toml::Value) -> Option<Value> {
    match raw {
        toml::Value::Boolean(on) => Some(Value::Bool(*on)),
        toml::Value::Integer(n) => Some(Value::Int(*n)),
        toml::Value::Float(v) => Some(Value::Float(*v)),
        toml::Value::String(text) => Some(Value::Str(text.clone())),
        _ => None,
    }
}

fn to_toml(value: &Value) -> toml::Value {
    match value {
        Value::Bool(on) => toml::Value::Boolean(*on),
        Value::Int(n) => toml::Value::Integer(*n),
        Value::Float(v) => toml::Value::Float(*v),
        Value::Str(text) => toml::Value::String(text.clone()),
    }
}

/// A setting key: `[a-z][a-z0-9_]*`, at most 32 bytes.
fn valid_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    (1..=MAX_KEY).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

/// `#rrggbb` or `#rrggbbaa`, lowercase hex.
fn valid_colour(text: &str) -> bool {
    let bytes = text.as_bytes();
    (bytes.len() == 7 || bytes.len() == 9)
        && bytes[0] == b'#'
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

/// A key name: 1 to 32 bytes of ASCII letters, digits, `_`, `+` and space.
fn valid_keybind(text: &str) -> bool {
    (1..=MAX_KEYBIND).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'+' || b == b' ')
}

fn required_text(
    table: &toml::Table,
    key: &str,
    field: &str,
    max: usize,
) -> Result<String, SettingFault> {
    if !table.contains_key(field) {
        return Err(fault(Reason::MissingField, key, field));
    }
    optional_text(table, key, field, max)
}

/// A plain text field: at most `max` bytes, no control characters. Empty when absent.
fn optional_text(
    table: &toml::Table,
    key: &str,
    field: &str,
    max: usize,
) -> Result<String, SettingFault> {
    match table.get(field) {
        None => Ok(String::new()),
        Some(toml::Value::String(text)) => {
            if text.len() > max || text.chars().any(char::is_control) {
                Err(fault(Reason::BadText, key, field))
            } else {
                Ok(text.clone())
            }
        }
        Some(_) => Err(fault(Reason::WrongType, key, field)),
    }
}

fn optional_str<'a>(
    table: &'a toml::Table,
    key: &str,
    field: &str,
) -> Result<Option<&'a str>, SettingFault> {
    match table.get(field) {
        None => Ok(None),
        Some(toml::Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(fault(Reason::WrongType, key, field)),
    }
}

/// One change to a setting, for the change event: the new value and the one it replaced.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub key: String,
    pub value: Value,
    pub old: Value,
}

/// Why a `set`, `reset` or `reset_all` did nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingError {
    UnknownKey { key: String },
    NotEditable { key: String },
    Invalid { key: String, reason: String },
    File(String),
}

impl std::fmt::Display for SettingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingError::UnknownKey { key } => write!(f, "unknown setting {key:?}"),
            SettingError::NotEditable { key } => {
                write!(
                    f,
                    "setting {key:?} is server-side and cannot be changed here"
                )
            }
            SettingError::Invalid { key, reason } => write!(f, "setting {key:?} {reason}"),
            SettingError::File(text) => f.write_str(text),
        }
    }
}

/// The settings of one mod, as loaded from its file, with the schema they are checked against.
pub struct Settings {
    mod_id: String,
    game_dir: PathBuf,
    schema: Schema,
    role: Role,
    /// The valid values the file holds, by key. A key not here has its default.
    values: Mutex<BTreeMap<String, Value>>,
}

impl Settings {
    /// Reads `<game_dir>/config/<mod_id>/settings.toml` against `schema`. Never fails: every problem
    /// with the file is a warning, and the defaults are used for what it does not give.
    pub fn load(game_dir: &Path, mod_id: &str, schema: Schema, role: Role) -> Settings {
        let mut values = BTreeMap::new();
        if valid_id(mod_id) {
            let target = game_dir.join(CONFIG_DIR).join(mod_id).join(FILE);
            match read_table(&target) {
                Ok(None) => {}
                Err(problem) => warn_once(
                    mod_id,
                    &format!("settings.toml {problem}; the defaults are used"),
                ),
                Ok(Some(table)) => {
                    let unknown = table.keys().filter(|key| schema.get(key).is_none()).count();
                    if unknown > 0 {
                        warn_once(
                            mod_id,
                            &format!(
                                "settings.toml has {unknown} keys no setting declares; they are ignored"
                            ),
                        );
                    }
                    for setting in schema.settings() {
                        let Some(raw) = table.get(&setting.key) else {
                            continue;
                        };
                        let checked = from_toml(raw)
                            .ok_or_else(|| "has the wrong type".to_string())
                            .and_then(|value| setting.check(&value));
                        match checked {
                            Ok(value) => {
                                values.insert(setting.key.clone(), value);
                            }
                            Err(reason) => warn_once(
                                mod_id,
                                &format!(
                                    "settings.toml: {} {reason}; its default is used",
                                    setting.key
                                ),
                            ),
                        }
                    }
                }
            }
        } else {
            warn_once(mod_id, "settings are not read: the mod id is not valid");
        }
        Settings {
            mod_id: mod_id.to_owned(),
            game_dir: game_dir.to_owned(),
            schema,
            role,
            values: Mutex::new(values),
        }
    }

    pub fn mod_id(&self) -> &str {
        &self.mod_id
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The current value of `key`: the file's, or the default. `None` for a key no setting declares.
    pub fn get(&self, key: &str) -> Option<Value> {
        let setting = self.schema.get(key)?;
        let values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        Some(
            values
                .get(key)
                .cloned()
                .unwrap_or_else(|| setting.default.clone()),
        )
    }

    /// Sets `key` to `value`, checked against the schema, and writes the file. Returns the change.
    pub fn set(&self, key: &str, value: Value) -> Result<Change, SettingError> {
        let setting = self.editable(key)?;
        let value = setting
            .check(&value)
            .map_err(|reason| SettingError::Invalid {
                key: key.to_owned(),
                reason,
            })?;
        first(self.commit(&[(key.to_owned(), Some(value))])?)
    }

    /// Removes `key` from the file, so it takes its default again. Returns the change to the default.
    pub fn reset(&self, key: &str) -> Result<Change, SettingError> {
        self.editable(key)?;
        first(self.commit(&[(key.to_owned(), None)])?)
    }

    /// Removes every setting of this mod from the file, in one write. Returns a change for each
    /// setting whose value was not already its default.
    pub fn reset_all(&self) -> Result<Vec<Change>, SettingError> {
        let edits: Vec<(String, Option<Value>)> = self
            .schema
            .settings()
            .iter()
            .map(|setting| (setting.key.clone(), None))
            .collect();
        Ok(self
            .commit(&edits)?
            .into_iter()
            .filter(|change| change.value != change.old)
            .collect())
    }

    /// The setting `key`, if this mod declares it and this process may change it.
    fn editable(&self, key: &str) -> Result<&Setting, SettingError> {
        let setting = self
            .schema
            .get(key)
            .ok_or_else(|| SettingError::UnknownKey {
                key: key.to_owned(),
            })?;
        if setting.side == SettingSide::Server && self.role == Role::PureClient {
            return Err(SettingError::NotEditable {
                key: key.to_owned(),
            });
        }
        Ok(setting)
    }

    /// Writes `edits` to the file in one go and, only after that succeeded, updates memory. A `None`
    /// removes the key. Nothing is written when the file cannot be read as TOML or would be too big.
    fn commit(&self, edits: &[(String, Option<Value>)]) -> Result<Vec<Change>, SettingError> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = ensure_dir(&self.game_dir, &self.mod_id).map_err(SettingError::File)?;
        let target = dir.join(FILE);
        let mut table = read_table(&target)
            .map_err(|problem| {
                SettingError::File(format!("settings.toml {problem}; it is not written"))
            })?
            .unwrap_or_default();
        let mut values = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changes = Vec::with_capacity(edits.len());
        for (key, new) in edits {
            let Some(setting) = self.schema.get(key) else {
                continue;
            };
            let old = values
                .get(key)
                .cloned()
                .unwrap_or_else(|| setting.default.clone());
            match new {
                Some(value) => {
                    table.insert(key.clone(), to_toml(value));
                }
                None => {
                    table.remove(key);
                }
            }
            changes.push(Change {
                key: key.clone(),
                value: new.clone().unwrap_or_else(|| setting.default.clone()),
                old,
            });
        }
        let text = toml::to_string(&table)
            .map_err(|e| SettingError::File(format!("settings.toml: {e}")))?;
        if text.len() as u64 > MAX_FILE {
            return Err(SettingError::File(
                "settings.toml would be over 64 KiB; it is not written".into(),
            ));
        }
        write_atomic(&dir, &target, text.as_bytes()).map_err(SettingError::File)?;
        for (key, new) in edits {
            if self.schema.get(key).is_none() {
                continue;
            }
            match new {
                Some(value) => values.insert(key.clone(), value.clone()),
                None => values.remove(key),
            };
        }
        Ok(changes)
    }
}

fn first(changes: Vec<Change>) -> Result<Change, SettingError> {
    changes
        .into_iter()
        .next()
        .ok_or_else(|| SettingError::File("no setting was changed".into()))
}

/// Serialises the writers of `settings.toml` in this process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// Makes each temporary file's name unique within this process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// The mod's folder under `config`, created when absent. Refuses a `config` or mod folder that is
/// a symlink or not a folder.
fn ensure_dir(game_dir: &Path, mod_id: &str) -> Result<PathBuf, String> {
    let config = game_dir.join(CONFIG_DIR);
    let dir = config.join(mod_id);
    for path in [&config, &dir] {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(format!("{} is not a plain folder", path.display()));
            }
            Ok(_) => {}
            Err(_) => {
                std::fs::create_dir_all(path).map_err(|e| format!("config: {e}"))?;
            }
        }
    }
    Ok(dir)
}

/// The table the file holds: `None` when there is no file. Refuses, with the reason, a symlink, a
/// non-file, an unreadable or oversized file, text that is not UTF-8 or not TOML.
fn read_table(target: &Path) -> Result<Option<toml::Table>, String> {
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err("cannot be read".into()),
        Ok(meta) if meta.file_type().is_symlink() => Err("is a symlink".into()),
        Ok(meta) if !meta.is_file() => Err("is not a file".into()),
        Ok(_) => {
            let file = File::open(target).map_err(|_| "cannot be read".to_string())?;
            let mut bytes = Vec::new();
            file.take(MAX_FILE + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "cannot be read".to_string())?;
            if bytes.len() as u64 > MAX_FILE {
                return Err("is over 64 KiB".into());
            }
            let text = std::str::from_utf8(&bytes).map_err(|_| "is not UTF-8".to_string())?;
            text.parse::<toml::Table>()
                .map(Some)
                .map_err(|_| "is not valid TOML".to_string())
        }
    }
}

/// Writes `bytes` to a new file beside `target`, syncs it, and renames it over `target`.
fn write_atomic(dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), String> {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{FILE}.{}.{seq}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, target)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("settings.toml cannot be written: {error}"));
    }
    if let Ok(folder) = File::open(dir) {
        let _ = folder.sync_all();
    }
    Ok(())
}
