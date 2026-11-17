//! Finds the Lua mods a player has put in `<game_dir>/lua-mods/`.
//!
//! Each mod is a folder with a `lua-mod.toml` manifest and a `main.luau` script. The folders are
//! untrusted input: a symlink is never followed, the manifest is read up to a fixed size, and a
//! folder that does not pass is reported with a reason instead of being loaded. Only the minimal
//! manifest (id, version and side) is read here.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The folder under the game folder that holds the Lua mods.
pub const DIR: &str = "lua-mods";
/// The manifest every mod folder has.
pub const MANIFEST: &str = "lua-mod.toml";
/// The script every mod folder has.
pub const ENTRY: &str = "main.luau";
/// The largest manifest that is read; a bigger file is refused without reading all of it.
pub const MAX_MANIFEST: u64 = 64 * 1024;

/// The side a mod runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
    Both,
}

/// A mod folder that passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovered {
    pub dir: PathBuf,
    pub id: String,
    pub version: String,
    pub side: Side,
}

/// A folder that was refused, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub dir: PathBuf,
    pub reason: String,
}

/// What a scan found. Both lists are in folder-name order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub mods: Vec<Discovered>,
    pub refused: Vec<Refused>,
}

/// Scans `<game_dir>/lua-mods/`. A missing folder is not an error: the result is empty.
pub fn scan(game_dir: &Path) -> Found {
    let Ok(entries) = std::fs::read_dir(game_dir.join(DIR)) else {
        return Found::default();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    dirs.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut found = Found::default();
    // The id of each accepted mod, and the folder name that took it first.
    let mut taken: HashMap<String, String> = HashMap::new();
    for dir in dirs {
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            found.refused.push(Refused {
                dir,
                reason: "a symlink is not followed".into(),
            });
        } else if meta.is_dir() {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match check(&dir) {
                Ok(m) => match taken.get(&m.id) {
                    Some(first) => found.refused.push(Refused {
                        reason: format!("id {} is already used by {first}", m.id),
                        dir,
                    }),
                    None => {
                        taken.insert(m.id.clone(), name);
                        found.mods.push(m);
                    }
                },
                Err(reason) => found.refused.push(Refused { dir, reason }),
            }
        }
    }
    found
}

/// Checks one mod folder and returns what it declares, or why it is refused.
fn check(dir: &Path) -> Result<Discovered, String> {
    let manifest = read_manifest(&dir.join(MANIFEST))?;
    let id = text(&manifest, "id")?;
    if !valid_id(id) {
        return Err("id is not a valid mod id".into());
    }
    let version = text(&manifest, "version")?;
    if !(1..=64).contains(&version.chars().count()) {
        return Err("version must be 1 to 64 characters".into());
    }
    let side = match text(&manifest, "side")? {
        "client" => Side::Client,
        "server" => Side::Server,
        "both" => Side::Both,
        _ => return Err("side must be client, server or both".into()),
    };
    let entry_is_file = std::fs::symlink_metadata(dir.join(ENTRY)).is_ok_and(|m| m.is_file());
    if !entry_is_file {
        return Err(format!("no {ENTRY}"));
    }
    Ok(Discovered {
        dir: dir.to_path_buf(),
        id: id.to_string(),
        version: version.to_string(),
        side,
    })
}

/// Reads and parses the manifest, reading at most `MAX_MANIFEST + 1` bytes.
fn read_manifest(path: &Path) -> Result<toml::Table, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| format!("no {MANIFEST}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{MANIFEST} is a symlink"));
    }
    if !meta.is_file() {
        return Err(format!("no {MANIFEST}"));
    }
    let file = std::fs::File::open(path).map_err(|_| format!("{MANIFEST} cannot be read"))?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{MANIFEST} cannot be read"))?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(format!("{MANIFEST} is over {} KiB", MAX_MANIFEST / 1024));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("{MANIFEST} is not UTF-8"))?;
    text.parse::<toml::Table>()
        .map_err(|_| format!("{MANIFEST} is not valid TOML"))
}

/// A string key of the manifest; a missing or wrongly typed key is refused by name.
fn text<'a>(manifest: &'a toml::Table, key: &str) -> Result<&'a str, String> {
    match manifest.get(key) {
        Some(toml::Value::String(s)) => Ok(s),
        _ => Err(format!("{key} is missing or not a string")),
    }
}

/// A mod id: 1 to 64 bytes of lowercase ASCII letters, digits and underscores.
fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, version: &str, side: &str) -> String {
        format!("id = \"{id}\"\nversion = \"{version}\"\nside = \"{side}\"\n")
    }

    /// Writes `<root>/lua-mods/<name>/` with the manifest text and a `main.luau`.
    fn mod_folder(root: &Path, name: &str, manifest_text: &str) -> PathBuf {
        let dir = root.join(DIR).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest_text).unwrap();
        std::fs::write(dir.join(ENTRY), "return {}\n").unwrap();
        dir
    }

    fn reasons(found: &Found) -> Vec<String> {
        found.refused.iter().map(|r| r.reason.clone()).collect()
    }

    #[test]
    fn a_valid_mod_folder_is_found() {
        let root = tempfile::tempdir().unwrap();
        let dir = mod_folder(
            root.path(),
            "alpha",
            &manifest("alpha_mod", "1.0.0", "both"),
        );
        let found = scan(root.path());
        assert!(found.refused.is_empty());
        assert_eq!(
            found.mods,
            vec![Discovered {
                dir,
                id: "alpha_mod".into(),
                version: "1.0.0".into(),
                side: Side::Both,
            }]
        );
    }

    #[test]
    fn a_missing_lua_mods_folder_is_empty() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(scan(root.path()), Found::default());
    }

    #[test]
    fn a_folder_without_a_manifest_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIR).join("nomanifest");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), "return {}\n").unwrap();
        let found = scan(root.path());
        assert!(found.mods.is_empty());
        assert_eq!(reasons(&found), vec!["no lua-mod.toml"]);
    }

    #[test]
    fn a_bad_id_is_refused() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(root.path(), "a_upper", &manifest("Bad-Id", "1", "both"));
        mod_folder(root.path(), "b_empty", &manifest("", "1", "both"));
        mod_folder(
            root.path(),
            "c_long",
            &manifest(&"a".repeat(65), "1", "both"),
        );
        mod_folder(
            root.path(),
            "d_max",
            &manifest(&"a".repeat(64), "1", "both"),
        );
        let found = scan(root.path());
        assert_eq!(found.mods.len(), 1);
        assert_eq!(found.mods[0].id.len(), 64);
        assert_eq!(reasons(&found), vec!["id is not a valid mod id"; 3]);
    }

    #[test]
    fn a_bad_side_and_a_missing_key_are_refused() {
        let root = tempfile::tempdir().unwrap();
        mod_folder(root.path(), "a_side", &manifest("side_mod", "1", "Both"));
        mod_folder(
            root.path(),
            "b_nover",
            "id = \"nover_mod\"\nside = \"both\"\n",
        );
        mod_folder(
            root.path(),
            "c_int",
            "id = \"int_mod\"\nversion = 1\nside = \"both\"\n",
        );
        let found = scan(root.path());
        assert!(found.mods.is_empty());
        assert_eq!(
            reasons(&found),
            vec![
                "side must be client, server or both",
                "version is missing or not a string",
                "version is missing or not a string",
            ]
        );
    }

    #[test]
    fn a_manifest_over_the_cap_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let text = format!(
            "{}# {}\n",
            manifest("big_mod", "1", "both"),
            "x".repeat(70 * 1024)
        );
        mod_folder(root.path(), "big", &text);
        let found = scan(root.path());
        assert!(found.mods.is_empty());
        assert_eq!(reasons(&found), vec!["lua-mod.toml is over 64 KiB"]);
    }

    #[test]
    fn a_folder_without_main_luau_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(DIR).join("noentry");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest("noentry", "1", "both")).unwrap();
        let found = scan(root.path());
        assert!(found.mods.is_empty());
        assert_eq!(reasons(&found), vec!["no main.luau"]);
    }

    #[test]
    fn a_duplicate_id_is_refused_and_the_first_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let first = mod_folder(root.path(), "a", &manifest("same", "1", "both"));
        let second = mod_folder(root.path(), "b", &manifest("same", "2", "client"));
        let found = scan(root.path());
        assert_eq!(found.mods.len(), 1);
        assert_eq!(found.mods[0].dir, first);
        assert_eq!(found.mods[0].version, "1");
        assert_eq!(
            found.refused,
            vec![Refused {
                dir: second,
                reason: "id same is already used by a".into(),
            }]
        );
    }

    #[test]
    fn files_are_ignored_and_the_order_is_by_name() {
        let root = tempfile::tempdir().unwrap();
        let lua_mods = root.path().join(DIR);
        std::fs::create_dir_all(&lua_mods).unwrap();
        std::fs::write(lua_mods.join("plain.txt"), "hello").unwrap();
        std::fs::write(lua_mods.join("x.hlua"), "return {}").unwrap();
        mod_folder(root.path(), "b", &manifest("bee", "1", "both"));
        mod_folder(root.path(), "a", &manifest("ay", "1", "server"));
        let found = scan(root.path());
        assert!(found.refused.is_empty());
        let ids: Vec<&str> = found.mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["ay", "bee"]);
        assert_eq!(found.mods[0].side, Side::Server);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let target = mod_folder(root.path(), "real", &manifest("real_mod", "1", "both"));
        let lua_mods = root.path().join(DIR);
        std::fs::create_dir_all(&lua_mods).unwrap();
        std::os::unix::fs::symlink(&target, lua_mods.join("link")).unwrap();
        let found = scan(root.path());
        assert_eq!(found.mods.len(), 1);
        assert_eq!(found.mods[0].dir, target);
        assert_eq!(found.refused.len(), 1);
        assert_eq!(found.refused[0].dir, lua_mods.join("link"));
        assert_eq!(found.refused[0].reason, "a symlink is not followed");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_main_luau_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside.luau");
        std::fs::write(&outside, "return {}\n").unwrap();
        let dir = root.path().join(DIR).join("linked");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest("linked_mod", "1", "both")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(ENTRY)).unwrap();
        let found = scan(root.path());
        assert!(found.mods.is_empty());
        assert_eq!(reasons(&found), vec!["no main.luau"]);
    }

    #[test]
    fn a_deeply_nested_manifest_is_refused_without_crashing() {
        // 30,000 levels is under the 64 KiB cap: the TOML parser must refuse it, not overflow.
        let root = tempfile::tempdir().unwrap();
        let nested = format!("x = {}{}\n", "[".repeat(30_000), "]".repeat(30_000));
        assert!(nested.len() < MAX_MANIFEST as usize);
        mod_folder(root.path(), "deep", &nested);
        let found = scan(root.path());
        assert!(found.mods.is_empty());
        assert_eq!(found.refused.len(), 1);
    }
}
