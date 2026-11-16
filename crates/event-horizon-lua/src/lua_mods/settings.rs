//! A mod's settings schema: `settings.json` at the mod's root (docs/lua-bindings.md, "Settings
//! schema"). It is read at load, without running Lua. `horizon.config()` fills in the declared
//! defaults and refuses a stored value that does not fit; `LuaMod::set_setting` and
//! `LuaMod::reset_setting` write one value and say what changed, through the same checks.
//!
//! A mod without the file has no schema, and its `horizon.config()` is what it was before.

use super::LuaSide;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The schema's file, at the mod's root.
pub const FILE: &str = "settings.json";

#[cfg(test)]
mod tests;

/// The kinds a setting can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Str,
    Choice,
}

impl Kind {
    fn describe(self) -> &'static str {
        match self {
            Kind::Bool => "a bool",
            Kind::Int => "an int",
            Kind::Float => "a float",
            Kind::Str => "a string",
            Kind::Choice => "one of its choices",
        }
    }
}

/// One declared setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub kind: Kind,
    pub default: Value,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Vec<String>,
    pub group: Option<String>,
    pub order: Option<String>,
    pub hidden: bool,
    /// The side whose config file holds the value: `server` unless declared `client`.
    pub side: LuaSide,
}

impl Entry {
    /// Whether `value` is one this setting accepts; the error says why not.
    pub fn check(&self, value: &Value) -> Result<(), String> {
        let fits = match self.kind {
            Kind::Bool => value.is_boolean(),
            Kind::Str => value.is_string(),
            Kind::Choice => match value.as_str() {
                Some(s) if self.choices.iter().any(|c| c == s) => true,
                Some(_) => return Err("is not one of its choices".to_owned()),
                None => false,
            },
            Kind::Int => value.is_i64() || value.is_u64() || value.as_f64().is_some_and(whole),
            Kind::Float => value.is_number(),
        };
        if !fits {
            return Err(format!("takes {}", self.kind.describe()));
        }
        if let Some(n) = value
            .as_f64()
            .filter(|_| matches!(self.kind, Kind::Int | Kind::Float))
            && (self.min.is_some_and(|min| n < min) || self.max.is_some_and(|max| n > max))
        {
            return Err(format!(
                "is outside {}..{}",
                self.min.map_or("-inf".to_owned(), |m| m.to_string()),
                self.max.map_or("inf".to_owned(), |m| m.to_string())
            ));
        }
        Ok(())
    }
}

fn whole(f: f64) -> bool {
    f.is_finite() && f.fract() == 0.0
}

/// The schema of `text`, a `settings.json`: its valid entries in the order declared. A file that
/// is not JSON, or has no `settings` list, is an error. A bad entry is skipped and logged with the
/// mod id and its key; the good ones stay.
pub fn parse(text: &str, mod_id: &str) -> Result<Vec<Entry>, String> {
    let json: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let list = json
        .get("settings")
        .and_then(Value::as_array)
        .ok_or_else(|| "no \"settings\" list".to_owned())?;
    let mut out: Vec<Entry> = Vec::new();
    for item in list {
        match entry(item, &out) {
            Ok(e) => out.push(e),
            Err((key, why)) => {
                log::warn!("[lua:{mod_id}] settings.json: setting {key:?} skipped: {why}");
            }
        }
    }
    Ok(out)
}

/// One entry of the list, or the key it names (empty when none) and why it is skipped.
fn entry(item: &Value, earlier: &[Entry]) -> Result<Entry, (String, String)> {
    let key = item
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let fail = |why: &str| (key.clone(), why.to_owned());
    let key_ok = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b));
    if !key_ok {
        return Err(fail("the key is missing or not [a-z0-9_.-]"));
    }
    if earlier.iter().any(|e| e.key == key) {
        return Err(fail("a second setting with this key"));
    }
    let kind = match item.get("type").and_then(Value::as_str) {
        Some("bool") => Kind::Bool,
        Some("int") => Kind::Int,
        Some("float") => Kind::Float,
        Some("string") => Kind::Str,
        Some("choice") => Kind::Choice,
        _ => return Err(fail("the type is not bool, int, float, string or choice")),
    };
    let default = item
        .get("default")
        .cloned()
        .ok_or_else(|| fail("no default"))?;
    let number = |name: &str| -> Result<Option<f64>, (String, String)> {
        match item.get(name) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .map(Some)
                .ok_or_else(|| fail(&format!("{name} is not a number"))),
        }
    };
    let (min, max) = (number("min")?, number("max")?);
    if (min.is_some() || max.is_some()) && !matches!(kind, Kind::Int | Kind::Float) {
        return Err(fail("min and max go on int and float settings only"));
    }
    if min.zip(max).is_some_and(|(lo, hi)| lo > hi) {
        return Err(fail("min is above max"));
    }
    let choices = match item.get("choices") {
        None if kind == Kind::Choice => return Err(fail("a choice has no choices list")),
        None => Vec::new(),
        Some(_) if kind != Kind::Choice => {
            return Err(fail("choices go on a choice setting only"));
        }
        Some(list) => {
            let values: Option<Vec<String>> = list
                .as_array()
                .map(|l| {
                    l.iter()
                        .filter_map(|c| c.get("value").and_then(Value::as_str).map(str::to_owned))
                        .collect()
                })
                .filter(|v: &Vec<String>| !v.is_empty());
            values.ok_or_else(|| fail("choices is not a non-empty list of {\"value\": string}"))?
        }
    };
    let text_field = |name: &str| -> Result<Option<String>, (String, String)> {
        match item.get(name) {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(|s| Some(s.to_owned()))
                .ok_or_else(|| fail(&format!("{name} is not a string"))),
        }
    };
    let side = match item.get("side").and_then(Value::as_str) {
        None | Some("server") => LuaSide::Server,
        Some("client") => LuaSide::Client,
        Some(_) => return Err(fail("side is not server or client")),
    };
    let out = Entry {
        key: key.clone(),
        kind,
        default,
        min,
        max,
        choices,
        group: text_field("group")?,
        order: text_field("order")?,
        hidden: item.get("hidden").and_then(Value::as_bool).unwrap_or(false),
        side,
    };
    out.check(&out.default)
        .map_err(|why| (key.clone(), format!("its default {why}")))?;
    Ok(out)
}

/// The schema of the mod at `path`: `settings.json` at a mod folder's root. None when there is no
/// such file, or it is not a valid one (logged once). A packaged `.hmod` has its file read by
/// nothing yet, so it has no schema.
pub fn read(path: &Path, mod_id: &str) -> Option<Vec<Entry>> {
    if !path.is_dir() {
        return None;
    }
    let text = match std::fs::read_to_string(path.join(FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::error!("[lua:{mod_id}] settings.json: {e}; the mod has no settings schema");
            return None;
        }
    };
    match parse(&text, mod_id) {
        Ok(entries) => Some(entries),
        Err(e) => {
            log::error!("[lua:{mod_id}] settings.json: {e}; the mod has no settings schema");
            None
        }
    }
}

/// The file a side's values live in, under the game folder's `config/`: `<id>.json` on the
/// server, `<id>.client.json` on the client.
pub fn file_name(mod_id: &str, side: LuaSide) -> String {
    match side {
        LuaSide::Server => format!("{mod_id}.json"),
        LuaSide::Client => format!("{mod_id}.client.json"),
    }
}

/// A change to a setting's stored value, as the `config_changed` hook is told it.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub mod_id: String,
    pub key: String,
    /// What `horizon.config()` gave before the write, and gives after it.
    pub old: Value,
    pub new: Value,
}

/// A mod's schema with the config folder its values are saved in.
pub struct Settings {
    mod_id: String,
    entries: Vec<Entry>,
    config_dir: Option<PathBuf>,
    /// The keys whose stored value `horizon.config()` refused, each logged once.
    refused: Mutex<HashSet<String>>,
}

impl Settings {
    pub fn new(mod_id: &str, entries: Vec<Entry>, config_dir: Option<PathBuf>) -> Self {
        Self {
            mod_id: mod_id.to_owned(),
            entries,
            config_dir,
            refused: Mutex::default(),
        }
    }

    pub fn mod_id(&self) -> &str {
        &self.mod_id
    }

    /// Every declared setting, both sides, in the order declared.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The side's values as `horizon.config()` gives them now: its saved file read and resolved
    /// (see [`Settings::resolve`]). A file that cannot be read gives the defaults, logged.
    pub fn values(&self, side: LuaSide) -> Map<String, Value> {
        let saved = match self.file(side) {
            Ok(path) => read_map(&path).unwrap_or_else(|e| {
                log::warn!("[lua:{}] settings: {e}", self.mod_id);
                Map::new()
            }),
            Err(_) => Map::new(),
        };
        match self.resolve(side, Value::Object(saved)) {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    /// `json` (the side's saved object) with the side's declared defaults filled in for missing
    /// keys, and a refused stored value replaced by its default. Keys the schema does not name
    /// stay as they are.
    pub fn resolve(&self, side: LuaSide, mut json: Value) -> Value {
        let Some(map) = json.as_object_mut() else {
            return json;
        };
        for e in self.entries.iter().filter(|e| e.side == side) {
            let stored = map.get(&e.key).cloned();
            if let Some(why) = stored.as_ref().and_then(|v| e.check(v).err()) {
                self.refuse(&e.key, &why);
            }
            let value = effective(e, stored.as_ref());
            map.insert(e.key.clone(), value);
        }
        json
    }

    fn refuse(&self, key: &str, why: &str) {
        let mut seen = self.refused.lock().unwrap_or_else(|e| e.into_inner());
        if seen.insert(key.to_owned()) {
            log::warn!(
                "[lua:{}] settings: the saved {key} {why}; using its default",
                self.mod_id
            );
        }
    }

    /// Writes `value` as setting `key` of `side`: only that key changes in the side's file, and
    /// `notify` is told once, when the stored value changes. Errors name the reason and write
    /// nothing.
    pub fn set(
        &self,
        side: LuaSide,
        key: &str,
        value: Value,
        notify: &mut dyn FnMut(&Change),
    ) -> Result<(), String> {
        let entry = self.declared(side, key)?;
        entry.check(&value).map_err(|why| format!("{key} {why}"))?;
        let path = self.file(side)?;
        let mut map = read_map(&path)?;
        let stored = map.get(key).cloned();
        if stored.as_ref() == Some(&value) {
            return Ok(());
        }
        let old = effective(entry, stored.as_ref());
        map.insert(key.to_owned(), value.clone());
        write_map(&path, map)?;
        notify(&Change {
            mod_id: self.mod_id.clone(),
            key: key.to_owned(),
            old,
            new: value,
        });
        Ok(())
    }

    /// Removes setting `key` from `side`'s file, so `horizon.config()` gives its default again.
    /// `notify` is told once, when there was a stored value to remove.
    pub fn reset(
        &self,
        side: LuaSide,
        key: &str,
        notify: &mut dyn FnMut(&Change),
    ) -> Result<(), String> {
        let entry = self.declared(side, key)?;
        let path = self.file(side)?;
        let mut map = read_map(&path)?;
        let Some(stored) = map.remove(key) else {
            return Ok(());
        };
        let old = effective(entry, Some(&stored));
        write_map(&path, map)?;
        notify(&Change {
            mod_id: self.mod_id.clone(),
            key: key.to_owned(),
            old,
            new: entry.default.clone(),
        });
        Ok(())
    }

    fn declared(&self, side: LuaSide, key: &str) -> Result<&Entry, String> {
        self.entries
            .iter()
            .find(|e| e.side == side && e.key == key)
            .ok_or_else(|| format!("no setting {key} on the {} side", side.as_str()))
    }

    fn file(&self, side: LuaSide) -> Result<PathBuf, String> {
        self.config_dir
            .as_ref()
            .map(|d| d.join(file_name(&self.mod_id, side)))
            .ok_or_else(|| "no config folder".to_owned())
    }
}

/// The value `horizon.config()` gives for `e` with `stored` saved: the stored one when it fits,
/// the default otherwise.
fn effective(e: &Entry, stored: Option<&Value>) -> Value {
    match stored {
        Some(v) if e.check(v).is_ok() => v.clone(),
        _ => e.default.clone(),
    }
}

/// The side's saved object: empty when there is no file yet.
fn read_map(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{}: not a JSON object", path.display())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes the object as `save_config` does: pretty JSON and a final newline.
fn write_map(path: &Path, map: Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(map)).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

impl super::LuaMod {
    /// Writes a setting of the mod's schema to `side`'s config file (see [`Settings::set`]). A mod
    /// without a schema has no settings.
    pub fn set_setting(
        &self,
        side: LuaSide,
        key: &str,
        value: Value,
        notify: &mut dyn FnMut(&Change),
    ) -> Result<(), String> {
        self.settings
            .as_ref()
            .ok_or_else(|| format!("{} declares no settings", self.id))?
            .set(side, key, value, notify)
    }

    /// Removes a setting of the mod's schema from `side`'s config file (see [`Settings::reset`]).
    pub fn reset_setting(
        &self,
        side: LuaSide,
        key: &str,
        notify: &mut dyn FnMut(&Change),
    ) -> Result<(), String> {
        self.settings
            .as_ref()
            .ok_or_else(|| format!("{} declares no settings", self.id))?
            .reset(side, key, notify)
    }
}
