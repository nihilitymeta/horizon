//! The enabled-state file of the Lua mods: `<game_dir>/lua-mods/enabled.toml`.
//!
//! It holds a `[mods]` table, `id = true` or `id = false`. It is a deny list in practice: a mod with
//! no entry is enabled. A file that is absent, a symlink, over 64 KiB, not UTF-8 or not valid TOML
//! is read as "every mod enabled"; the reader says why in one notice, and the game still starts.
//! Entries that are not a boolean for a valid id are ignored the same way, with one notice for the
//! file. An id that names no installed mod is simply ignored.
//!
//! A `[grants]` table, `id = ["power", ...]`, holds the powers the owner granted to each mod (see
//! `powers.rs`). It is an allow list and fails closed: a file, or an entry, that does not read gives
//! no grant. Unknown power names and wrong types are ignored, a list of more than 16 names is refused
//! as a whole, and at most 64 mods may hold grants; each problem is counted in the same one notice.
//! A `[grants]` that is not a table ignores the grants only: the `[mods]` states still apply.
//!
//! A `[display]` table, `errors = "owner"`, asks for one chat line to the owner per mod with a problem
//! (see `display.rs`). `"off"` is the default; an absent, invalid or unknown value reads as `"off"` and
//! counts in the same notice.
//!
//! `write` changes one entry of `[mods]`, and `grant` one power of `[grants]`. It writes a temporary file beside the file with `create_new`, syncs it,
//! and renames it over the file, so a reader sees the old text or the new text, never a part. A
//! symlinked file is never written through. Writers in this process are serialised by a mutex, so
//! two changes at once keep both.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::discover::DIR;
use crate::display::Errors;
use crate::manifest::{Power, valid_id};

/// The file's name, under the `lua-mods` folder.
pub const FILE: &str = "enabled.toml";
/// The largest file that is read; a bigger one is ignored, and so is a bigger write.
pub const MAX_ENABLED: u64 = 64 * 1024;
/// The table that holds the entries.
const TABLE: &str = "mods";
/// The table that holds the grants: `id = ["power", ...]`.
const GRANTS: &str = "grants";
/// The table that holds the display settings: `errors = "off" | "owner"` (see `display.rs`).
const DISPLAY: &str = "display";
/// The most mods that may hold grants in one file; the rest are ignored (and counted).
pub const MAX_GRANT_MODS: usize = 64;
/// The most powers one mod may be granted in the file; a longer list is refused as a whole.
pub const MAX_GRANTS_PER_MOD: usize = 16;

/// What the file said. `states` holds only the valid boolean entries; `grants` only the valid grants,
/// each list in `Power::rank` order without repeats; `notice` is set when anything was ignored, for one
/// warning in the log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Enabled {
    pub states: HashMap<String, bool>,
    pub grants: HashMap<String, Vec<Power>>,
    /// The `[display] errors` setting; `Errors::Off` unless the file asks for `"owner"`.
    pub display: Errors,
    pub notice: Option<String>,
}

impl Enabled {
    /// Whether the mod `id` is enabled: true unless the file says `false` for it.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.states.get(id).copied().unwrap_or(true)
    }

    /// The powers the file grants to the mod `id`: none unless the file names some.
    pub fn granted(&self, id: &str) -> Vec<Power> {
        self.grants.get(id).cloned().unwrap_or_default()
    }

    fn ignored(what: &str) -> Enabled {
        Enabled {
            states: HashMap::new(),
            grants: HashMap::new(),
            display: Errors::Off,
            notice: Some(format!("enabled.toml {what}; every mod is enabled")),
        }
    }
}

/// Reads `<game_dir>/lua-mods/enabled.toml`. Never fails: a problem with the file is a notice.
pub fn read(game_dir: &Path) -> Enabled {
    let path = game_dir.join(DIR).join(FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Enabled::default(),
        Err(_) => return Enabled::ignored("cannot be read"),
        Ok(meta) if meta.file_type().is_symlink() => {
            return Enabled::ignored("is a symlink and is not read");
        }
        Ok(meta) if !meta.is_file() => return Enabled::ignored("is not a file"),
        Ok(_) => {}
    }
    let Ok(file) = File::open(&path) else {
        return Enabled::ignored("cannot be read");
    };
    let mut bytes = Vec::new();
    if file.take(MAX_ENABLED + 1).read_to_end(&mut bytes).is_err() {
        return Enabled::ignored("cannot be read");
    }
    if bytes.len() as u64 > MAX_ENABLED {
        return Enabled::ignored("is over 64 KiB");
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Enabled::ignored("is not UTF-8");
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return Enabled::ignored("is not valid TOML");
    };
    parse(&table)
}

/// The entries of a parsed file: the valid booleans, and one count of what was ignored.
fn parse(table: &toml::Table) -> Enabled {
    let mut states = HashMap::new();
    let mut grants = HashMap::new();
    let mut display = Errors::Off;
    let mut ignored = 0usize;
    for (key, value) in table {
        if key == GRANTS {
            match value {
                toml::Value::Table(mods) => ignored += parse_grants(mods, &mut grants),
                _ => ignored += 1,
            }
            continue;
        }
        if key == DISPLAY {
            match value {
                toml::Value::Table(settings) => ignored += parse_display(settings, &mut display),
                _ => ignored += 1,
            }
            continue;
        }
        if key != TABLE {
            ignored += 1;
            continue;
        }
        let toml::Value::Table(mods) = value else {
            return Enabled::ignored("has a [mods] that is not a table");
        };
        for (id, state) in mods {
            match state {
                toml::Value::Boolean(on) if valid_id(id) => {
                    states.insert(id.clone(), *on);
                }
                _ => ignored += 1,
            }
        }
    }
    let notice = (ignored > 0).then(|| {
        format!("enabled.toml has {ignored} entries that are not valid; they are ignored")
    });
    Enabled {
        states,
        grants,
        display,
        notice,
    }
}

/// The `[display]` table's `errors` setting into `out`; returns how many entries were ignored. Only
/// `"owner"` changes the setting: `"off"` is the default, and any other value or key is ignored.
fn parse_display(settings: &toml::Table, out: &mut Errors) -> usize {
    let mut ignored = 0;
    for (key, value) in settings {
        match (key.as_str(), value.as_str()) {
            ("errors", Some("owner")) => *out = Errors::Owner,
            ("errors", Some("off")) => *out = Errors::Off,
            _ => ignored += 1,
        }
    }
    ignored
}

/// The valid grants of a `[grants]` table, into `out`; returns how many entries were ignored. A mod
/// whose list has more than `MAX_GRANTS_PER_MOD` names is refused as a whole; unknown names are dropped.
fn parse_grants(mods: &toml::Table, out: &mut HashMap<String, Vec<Power>>) -> usize {
    let mut ignored = 0;
    for (id, value) in mods {
        let toml::Value::Array(names) = value else {
            ignored += 1;
            continue;
        };
        if !valid_id(id) || names.len() > MAX_GRANTS_PER_MOD || out.len() >= MAX_GRANT_MODS {
            ignored += 1;
            continue;
        }
        let mut powers: Vec<Power> = Vec::new();
        for name in names {
            match name.as_str().and_then(Power::from_name) {
                Some(power) if !powers.contains(&power) => powers.push(power),
                Some(_) => {}
                None => ignored += 1,
            }
        }
        if !powers.is_empty() {
            powers.sort_by_key(|p| p.rank());
            out.insert(id.clone(), powers);
        }
    }
    ignored
}

/// Serialises the writers of `enabled.toml` in this process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// Makes each temporary file's name unique within this process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Sets mod `id` to `enabled` in `<game_dir>/lua-mods/enabled.toml`, keeping every other entry.
///
/// Refused, and nothing is written, when the id is not valid, when `lua-mods` or the file is a
/// symlink or not a plain file, or when the file that is there cannot be read as TOML (it is not
/// overwritten: fix or remove it first). The error text is English, for a log.
pub fn write(game_dir: &Path, id: &str, enabled: bool) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("{id:?} is not a valid mod id"));
    }
    edit(game_dir, |table| {
        let mods = table_mut(table, TABLE)?;
        mods.insert(id.to_owned(), toml::Value::Boolean(enabled));
        Ok(())
    })
}

/// Grants (`granted`) or revokes `power` for mod `id` in `<game_dir>/lua-mods/enabled.toml`, keeping
/// every other entry. A mod left with no power loses its key. Refused as `write` is, and also when a
/// new mod would be the 65th to hold grants.
pub fn grant(game_dir: &Path, id: &str, power: Power, granted: bool) -> Result<(), String> {
    if !valid_id(id) {
        return Err(format!("{id:?} is not a valid mod id"));
    }
    edit(game_dir, |table| {
        let grants = table_mut(table, GRANTS)?;
        let mut powers: Vec<Power> = match grants.get(id) {
            Some(toml::Value::Array(names)) => names
                .iter()
                .filter_map(|name| name.as_str().and_then(Power::from_name))
                .collect(),
            _ => Vec::new(),
        };
        if granted {
            if !powers.contains(&power) {
                if powers.is_empty() && grants.len() >= MAX_GRANT_MODS {
                    return Err(format!(
                        "enabled.toml already grants powers to {MAX_GRANT_MODS} mods"
                    ));
                }
                powers.push(power);
            }
        } else {
            powers.retain(|p| *p != power);
        }
        powers.sort_by_key(|p| p.rank());
        if powers.is_empty() {
            grants.remove(id);
        } else {
            let names = powers
                .iter()
                .map(|p| toml::Value::String(p.name().to_owned()))
                .collect();
            grants.insert(id.to_owned(), toml::Value::Array(names));
        }
        if grants.is_empty() {
            table.remove(GRANTS);
        }
        Ok(())
    })
}

/// The table `key` of `table`, made if absent. An entry of another type is refused.
fn table_mut<'a>(table: &'a mut toml::Table, key: &str) -> Result<&'a mut toml::Table, String> {
    match table
        .entry(key.to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
    {
        toml::Value::Table(inner) => Ok(inner),
        _ => Err(format!("enabled.toml has a [{key}] that is not a table")),
    }
}

/// Reads the file under the write lock, lets `change` edit its table, and writes the result atomically.
fn edit(
    game_dir: &Path,
    change: impl FnOnce(&mut toml::Table) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let dir = game_dir.join(DIR);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err("lua-mods is not a plain folder".into());
        }
        Ok(_) => {}
        Err(_) => std::fs::create_dir_all(&dir).map_err(|e| format!("lua-mods: {e}"))?,
    }
    let target = dir.join(FILE);
    let mut table = current(&target)?;
    change(&mut table)?;
    let text = toml::to_string(&table).map_err(|e| format!("enabled.toml: {e}"))?;
    if text.len() as u64 > MAX_ENABLED {
        return Err("enabled.toml would be over 64 KiB".into());
    }
    write_atomic(&dir, &target, text.as_bytes())
}

/// The table in the file now, or an empty one when there is no file. Refuses a symlink, a
/// non-file, an unreadable or oversized file, and text that is not TOML.
fn current(target: &Path) -> Result<toml::Table, String> {
    match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(error) => Err(format!("enabled.toml: {error}")),
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("enabled.toml is a symlink; it is not written".into())
        }
        Ok(meta) if !meta.is_file() => Err("enabled.toml is not a file".into()),
        Ok(_) => {
            let file = File::open(target).map_err(|e| format!("enabled.toml: {e}"))?;
            let mut bytes = Vec::new();
            file.take(MAX_ENABLED + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("enabled.toml: {e}"))?;
            if bytes.len() as u64 > MAX_ENABLED {
                return Err("enabled.toml is over 64 KiB; it is not written".into());
            }
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| "enabled.toml is not UTF-8; it is not written".to_string())?;
            text.parse::<toml::Table>().map_err(|_| {
                "enabled.toml is not valid TOML; fix or remove it before changing a mod".to_string()
            })
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
        return Err(format!("enabled.toml cannot be written: {error}"));
    }
    if let Ok(folder) = File::open(dir) {
        let _ = folder.sync_all();
    }
    Ok(())
}
