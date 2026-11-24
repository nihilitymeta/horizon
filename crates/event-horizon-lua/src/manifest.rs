//! The `lua-mod.toml` manifest of a Lua mod. The same manifest is read from a folder and from a
//! `.hlua` package (`hlua.rs`); discover.rs reads only its minimal part (id, version, side).
//!
//! Nothing here runs a power. `powers` are parsed and recorded only; `settings` are checked against the
//! schema in `settings.rs` and kept as data for it.
//! Every key is known, every string, list and table is capped, and an unknown key, a reserved id,
//! an unknown power or a field of the wrong type refuses the whole manifest with a named reason.

use crate::hlua::{self, Refusal};

/// The manifest's file name.
pub const MANIFEST: &str = "lua-mod.toml";
/// The largest manifest that is read; a bigger file is refused without reading all of it.
pub const MAX_MANIFEST: u64 = 64 * 1024;
/// The script a package runs when its manifest lists no entries.
pub const DEFAULT_ENTRY: &str = "main.luau";
/// The longest mod id, in bytes (ids are ASCII).
pub const MAX_ID: usize = 64;
/// The longest name, version, license and author, in bytes.
pub const MAX_NAME: usize = 64;
pub const MAX_VERSION: usize = 64;
pub const MAX_LICENSE: usize = 64;
pub const MAX_AUTHORS: usize = 16;
pub const MAX_AUTHOR: usize = 64;
/// The longest description, in bytes; line breaks and tabs are kept, other control characters refused.
pub const MAX_DESCRIPTION: usize = 1024;
pub const MAX_DEPENDENCIES: usize = 32;
/// The longest version requirement, in bytes.
pub const MAX_REQUIREMENT: usize = 64;
/// Most comparators one requirement may hold (`>=1.2, <2`).
pub const MAX_COMPARATORS: usize = 4;
pub const MAX_ENTRIES: usize = 16;
pub const MAX_POWERS: usize = 16;
pub const MAX_SETTINGS: usize = 64;
/// Deepest nesting of containers in `settings`: the array is depth 1, its tables depth 2.
pub const MAX_SETTING_DEPTH: usize = 4;
/// The longest string or key inside `settings`, in bytes.
pub const MAX_SETTING_TEXT: usize = 256;

/// The ids no mod may take: the vanilla and client ones, and Horizon's own.
const RESERVED_IDS: [&str; 3] = ["minecraft", "c", "horizon"];
/// Every key the manifest may have at the top level. `assets` is listed only so it is refused by name.
const KEYS: [&str; 13] = [
    "id",
    "name",
    "version",
    "authors",
    "license",
    "description",
    "icon",
    "side",
    "dependencies",
    "entries",
    "powers",
    "settings",
    "assets",
];

/// The side a mod runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
    Both,
}

impl Side {
    fn parse(text: &str) -> Option<Side> {
        match text {
            "client" => Some(Side::Client),
            "server" => Some(Side::Server),
            "both" => Some(Side::Both),
            _ => None,
        }
    }
}

/// A capability a mod asks for. This chunk only records the request; nothing is unlocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Commands,
    Nbt,
    Spawn,
    Registration,
    Explosions,
    World,
    GameRules,
    Transactions,
    Network,
    Files,
}

/// The names a manifest may ask for, with their powers.
const POWERS: [(&str, Power); 10] = [
    ("commands", Power::Commands),
    ("nbt", Power::Nbt),
    ("spawn", Power::Spawn),
    ("registration", Power::Registration),
    ("explosions", Power::Explosions),
    ("world", Power::World),
    ("game_rules", Power::GameRules),
    ("transactions", Power::Transactions),
    ("network", Power::Network),
    ("files", Power::Files),
];

impl Power {
    /// The power a manifest name asks for, or `None` for a name not in the list.
    pub fn from_name(name: &str) -> Option<Power> {
        POWERS.iter().find(|(n, _)| *n == name).map(|(_, p)| *p)
    }

    /// The manifest name of this power.
    pub fn name(self) -> &'static str {
        POWERS
            .iter()
            .find(|(_, p)| *p == self)
            .map_or("", |(n, _)| n)
    }

    /// The position of this power in the table of names: the order grants are kept in.
    pub fn rank(self) -> usize {
        POWERS
            .iter()
            .position(|(_, p)| *p == self)
            .unwrap_or(usize::MAX)
    }
}

/// A mod this one depends on. `optional` dependencies may be missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub id: String,
    /// The requirement as written, checked by `valid_requirement`.
    pub requirement: String,
    pub optional: bool,
}

/// A checked manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    /// The display name; the id when the manifest gives none.
    pub name: String,
    pub version: String,
    pub authors: Vec<String>,
    pub license: String,
    pub description: String,
    /// A `.png` inside the package (its placement is checked by the package reader).
    pub icon: Option<String>,
    pub side: Side,
    /// Sorted by id.
    pub dependencies: Vec<Dependency>,
    /// The scripts the mod runs, in the order listed. `main.luau` when none is listed.
    pub entries: Vec<String>,
    pub powers: Vec<Power>,
    /// The `[[settings]]` tables, checked against the schema in `settings.rs` before this is built.
    pub settings: Vec<toml::Table>,
}

impl Manifest {
    /// Parses and checks the bytes of a `lua-mod.toml`. The caller reads at most `MAX_MANIFEST + 1`
    /// bytes; a bigger input is refused here too.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, Refusal> {
        if bytes.len() as u64 > MAX_MANIFEST {
            return Err(Refusal::ManifestTooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| Refusal::ManifestNotUtf8)?;
        let table: toml::Table = text.parse().map_err(|_| Refusal::ManifestNotToml)?;
        if let Some(key) = table.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(Refusal::UnknownKey { key: key.clone() });
        }
        if table.contains_key("assets") {
            return Err(Refusal::AssetsReserved);
        }

        let id = required_str(&table, "id")?;
        if !valid_id(id) {
            return Err(Refusal::BadId);
        }
        if reserved_id(id) {
            return Err(Refusal::ReservedId);
        }
        let name = match optional_str(&table, "name")? {
            Some(name) => bounded_text(name, "name", MAX_NAME, false)?.to_string(),
            None => id.to_string(),
        };
        let version = bounded_text(
            required_str(&table, "version")?,
            "version",
            MAX_VERSION,
            false,
        )?;
        if dep_version(version).is_err() {
            return Err(Refusal::BadVersion);
        }
        let side = Side::parse(required_str(&table, "side")?).ok_or(Refusal::BadSide)?;
        let settings = settings(&table)?;
        crate::settings::Schema::parse(&settings).map_err(Refusal::Setting)?;

        let authors = match string_list(&table, "authors", MAX_AUTHORS)? {
            Some(list) => list
                .into_iter()
                .map(|a| bounded_text(a, "authors", MAX_AUTHOR, false).map(str::to_string))
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        let license = match optional_str(&table, "license")? {
            Some(text) => bounded_text(text, "license", MAX_LICENSE, false)?.to_string(),
            None => String::new(),
        };
        let description = match optional_str(&table, "description")? {
            Some(text) => bounded_text(text, "description", MAX_DESCRIPTION, true)?.to_string(),
            None => String::new(),
        };
        let icon = match optional_str(&table, "icon")? {
            Some(path) => {
                if hlua::check_name(path).is_err() || !path.ends_with(".png") {
                    return Err(Refusal::BadIcon);
                }
                Some(path.to_string())
            }
            None => None,
        };

        Ok(Manifest {
            id: id.to_string(),
            name,
            version: version.to_string(),
            authors,
            license,
            description,
            icon,
            side,
            dependencies: dependencies(&table)?,
            entries: entries(&table)?,
            powers: powers(&table)?,
            settings,
        })
    }
}

/// The dependencies table: `dep = "^1"` or `dep = { version = "^1", optional = true }`.
fn dependencies(table: &toml::Table) -> Result<Vec<Dependency>, Refusal> {
    let Some(value) = table.get("dependencies") else {
        return Ok(Vec::new());
    };
    let deps = value.as_table().ok_or_else(|| Refusal::WrongType {
        key: "dependencies".into(),
    })?;
    if deps.len() > MAX_DEPENDENCIES {
        return Err(too_many("dependencies", MAX_DEPENDENCIES));
    }
    let mut out = Vec::with_capacity(deps.len());
    for (id, spec) in deps {
        if !valid_id(id) || reserved_id(id) {
            return Err(Refusal::BadDependency { id: id.clone() });
        }
        let (requirement, optional) = match spec {
            toml::Value::String(req) => (req.as_str(), false),
            toml::Value::Table(inline) => {
                if let Some(k) = inline
                    .keys()
                    .find(|k| !matches!(k.as_str(), "version" | "optional"))
                {
                    return Err(Refusal::UnknownKey {
                        key: format!("dependencies.{id}.{k}"),
                    });
                }
                let optional = match inline.get("optional") {
                    None => false,
                    Some(toml::Value::Boolean(b)) => *b,
                    Some(_) => {
                        return Err(Refusal::WrongType {
                            key: format!("dependencies.{id}.optional"),
                        });
                    }
                };
                (required_str(inline, "version")?, optional)
            }
            _ => {
                return Err(Refusal::WrongType {
                    key: "dependencies".into(),
                });
            }
        };
        if !valid_requirement(requirement) {
            return Err(Refusal::BadRequirement { id: id.clone() });
        }
        out.push(Dependency {
            id: id.clone(),
            requirement: requirement.to_string(),
            optional,
        });
    }
    Ok(out)
}

/// The scripts to run: each a relative `.luau` path, unique, at most `MAX_ENTRIES`.
fn entries(table: &toml::Table) -> Result<Vec<String>, Refusal> {
    let Some(list) = string_list(table, "entries", MAX_ENTRIES)? else {
        return Ok(vec![DEFAULT_ENTRY.to_string()]);
    };
    if list.is_empty() {
        return Err(Refusal::NoEntries);
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for path in list {
        let fine = path.ends_with(".luau") && hlua::check_name(path).is_ok();
        if !fine || out.iter().any(|o| o == path) {
            return Err(Refusal::BadEntry {
                value: path.to_string(),
            });
        }
        out.push(path.to_string());
    }
    Ok(out)
}

/// The powers asked for: known names only, each once.
fn powers(table: &toml::Table) -> Result<Vec<Power>, Refusal> {
    let Some(list) = string_list(table, "powers", MAX_POWERS)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<Power> = Vec::with_capacity(list.len());
    for name in list {
        let power = Power::from_name(name).ok_or_else(|| Refusal::UnknownPower {
            name: name.to_string(),
        })?;
        if out.contains(&power) {
            return Err(Refusal::DuplicatePower {
                name: name.to_string(),
            });
        }
        out.push(power);
    }
    Ok(out)
}

/// The settings schema: an array of tables, bounded in count, depth and string length.
fn settings(table: &toml::Table) -> Result<Vec<toml::Table>, Refusal> {
    let Some(value) = table.get("settings") else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or_else(|| Refusal::WrongType {
        key: "settings".into(),
    })?;
    check_setting(value, 1)?;
    items
        .iter()
        .map(|item| {
            item.as_table().cloned().ok_or_else(|| Refusal::WrongType {
                key: "settings".into(),
            })
        })
        .collect()
}

/// Checks one value of `settings`. `depth` counts containers: the settings array is depth 1, its
/// tables depth 2, and a container deeper than `MAX_SETTING_DEPTH` is refused. Scalars are not levels.
fn check_setting(value: &toml::Value, depth: usize) -> Result<(), Refusal> {
    match value {
        toml::Value::String(text) if text.len() > MAX_SETTING_TEXT => {
            return Err(text_too_long("settings", MAX_SETTING_TEXT));
        }
        toml::Value::Array(items) => {
            if depth > MAX_SETTING_DEPTH {
                return Err(Refusal::SettingsTooDeep);
            }
            if items.len() > MAX_SETTINGS {
                return Err(too_many("settings", MAX_SETTINGS));
            }
            for item in items {
                check_setting(item, depth + 1)?;
            }
        }
        toml::Value::Table(inner) => {
            if depth > MAX_SETTING_DEPTH {
                return Err(Refusal::SettingsTooDeep);
            }
            if inner.len() > MAX_SETTINGS {
                return Err(too_many("settings", MAX_SETTINGS));
            }
            for (key, item) in inner {
                if key.len() > MAX_SETTING_TEXT {
                    return Err(text_too_long("settings", MAX_SETTING_TEXT));
                }
                check_setting(item, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A key that must be a string.
fn required_str<'a>(table: &'a toml::Table, key: &str) -> Result<&'a str, Refusal> {
    optional_str(table, key)?.ok_or_else(|| Refusal::MissingKey { key: key.into() })
}

/// A key that may be absent, and must be a string when present.
fn optional_str<'a>(table: &'a toml::Table, key: &str) -> Result<Option<&'a str>, Refusal> {
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(Refusal::WrongType { key: key.into() }),
    }
}

/// A key holding a list of strings, at most `max` long. `None` when the key is absent.
fn string_list<'a>(
    table: &'a toml::Table,
    key: &str,
    max: usize,
) -> Result<Option<Vec<&'a str>>, Refusal> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let items = value
        .as_array()
        .ok_or_else(|| Refusal::WrongType { key: key.into() })?;
    if items.len() > max {
        return Err(too_many(key, max));
    }
    items
        .iter()
        .map(|item| match item {
            toml::Value::String(text) => Ok(text.as_str()),
            _ => Err(Refusal::WrongType { key: key.into() }),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// A human-readable string: at most `max` bytes, no control characters (line breaks and tabs
/// are kept when `multiline`).
fn bounded_text<'a>(
    text: &'a str,
    field: &str,
    max: usize,
    multiline: bool,
) -> Result<&'a str, Refusal> {
    if text.len() > max {
        return Err(text_too_long(field, max));
    }
    let bad = text
        .chars()
        .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\t')));
    if bad {
        return Err(Refusal::BadText {
            field: field.into(),
        });
    }
    Ok(text)
}

fn too_many(field: &str, max: usize) -> Refusal {
    Refusal::TooMany {
        field: field.into(),
        max: max as u64,
    }
}

fn text_too_long(field: &str, max: usize) -> Refusal {
    Refusal::TextTooLong {
        field: field.into(),
        max: max as u64,
    }
}

/// A mod id: 1 to 64 bytes of lowercase ASCII letters, digits and underscores.
pub fn valid_id(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Whether an id is one no mod may take.
pub fn reserved_id(id: &str) -> bool {
    RESERVED_IDS.contains(&id)
}

/// The version of a mod, checked as the saved-world comparison checks it (`parse_version`).
fn dep_version(version: &str) -> Result<(), ()> {
    horizon_mod_package::dependencies::parse_version(version)
        .map(|_| ())
        .map_err(|_| ())
}

/// A dependency's version requirement, strictly: up to `MAX_COMPARATORS` comparators separated by
/// commas, each an optional operator (`=`, `>`, `>=`, `<`, `<=`, `~`, `^`) then one to three
/// numbers without leading zeros (`1`, `1.2`, `1.2.3`). Wildcards and pre-release tags are refused.
pub fn valid_requirement(text: &str) -> bool {
    const OPS: [&str; 7] = [">=", "<=", "=", ">", "<", "~", "^"];
    if text.is_empty() || text.len() > MAX_REQUIREMENT {
        return false;
    }
    let comparators: Vec<&str> = text.split(',').map(str::trim).collect();
    if comparators.len() > MAX_COMPARATORS {
        return false;
    }
    comparators.iter().all(|c| {
        let rest = OPS.iter().find_map(|op| c.strip_prefix(op)).unwrap_or(c);
        let parts: Vec<&str> = rest.split('.').collect();
        (1..=3).contains(&parts.len())
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.len() <= 9
                    && p.bytes().all(|b| b.is_ascii_digit())
                    && (*p == "0" || !p.starts_with('0'))
            })
    })
}
