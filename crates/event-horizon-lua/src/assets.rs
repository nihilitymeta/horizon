//! A Lua mod's content as a content-only pack the game loads: its `assets/`, `data/` and `lang/`
//! files, published to `<game folder>/.horizon-cache/lua-mods/<pack id>-<hash>/` beside a
//! synthesized `horizon-mod.toml`.
//!
//! The package is hostile input. Nothing is copied from the `.hlua` as it is: every file is read
//! through `hlua::Package` (the bounded reader, the names checked by its 3a rules) and written
//! again, under a name taken from that validated entry. Publication is all or nothing: the files go
//! to a fresh temp folder beside the target, the pack must pass the content fence
//! (`horizon_mod_package::{Package, Content}`), and only then is the folder renamed into place. An
//! existing target is reused only when it is byte-for-byte what this run would write; anything
//! else is refused and left alone. Stale packs of the same mod are removed after a success.
//!
//! Layout of a pack: `assets/` and `data/` as in the package; `lang/<file>` becomes
//! `assets/<mod id>/lang/<file>`, the namespace the mod owns (`namespaces` in the manifest).
//! `icon.png`, the scripts and the Lua manifest are not published.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::hlua::{Package, Refusal};
use crate::manifest::Side;

/// The folder under the game folder that holds the caches the game builds (`.horizon-cache`).
pub const CACHE_DIR: &str = ".horizon-cache";
/// The folder under the cache that holds the Lua mods' packs.
pub const DIR: &str = "lua-mods";
/// The translation key of every refusal of a mod's content (its notice; the mod still loads).
pub const KEY: &str = "lua_mod.content.refused";
/// The synthesized manifest's file name.
pub const MANIFEST: &str = "horizon-mod.toml";
/// Most files a pack may hold (the package reader already caps its entries at 4096).
pub const MAX_FILES: usize = 4096;
/// Largest file a pack holds (the package reader's entry cap).
pub const MAX_FILE: u64 = 8 * 1024 * 1024;
/// Largest total of a pack's files (the package reader's total cap).
pub const MAX_TOTAL: u64 = 64 * 1024 * 1024;
/// Most stale packs of one mod removed by one publication.
pub const MAX_REMOVALS: usize = 8;
/// Longest mod id: the pack id is `lua_` and the id, and an id is at most 64 bytes.
const MAX_PACK_ID: usize = 64;
/// Hex characters of the content hash in a pack's folder name.
const HASH_LEN: usize = 16;
/// Names Windows reserves, with or without an extension, in any case.
const RESERVED: [&str; 24] = [
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why a mod's content is not published. The mod still loads; the notice carries `reason`.
#[derive(Debug)]
pub enum AssetError {
    /// A file name that cannot be a file on every system, or that leaves the pack.
    Name { name: String, why: &'static str },
    /// Two files that map to one name (`lang/x` beside `assets/<id>/lang/x`).
    Collision { name: String },
    /// More files than `MAX_FILES`.
    TooManyFiles,
    /// A file over `MAX_FILE`, or more than `MAX_TOTAL` in all.
    TooLarge { name: String },
    /// The mod id, with the `lua_` prefix, is longer than a mod id may be.
    IdTooLong,
    /// A file could not be read through the package reader.
    Package(Refusal),
    /// `.horizon-cache` or `lua-mods` is not a real folder (a symlink, or a file).
    Cache { path: String },
    /// A pack folder that is already there and is not what this run would write.
    Corrupt { path: String },
    /// The content fence (`horizon_mod_package`) refused the pack.
    Fence(String),
    /// A folder could not be created, written or renamed.
    Io(String),
}

impl AssetError {
    /// The English text, for a log line.
    pub fn reason(&self) -> String {
        match self {
            AssetError::Name { name, why } => format!("{name:?} {why}"),
            AssetError::Collision { name } => format!("{name:?} maps to a name another file has"),
            AssetError::TooManyFiles => format!("more than {MAX_FILES} files"),
            AssetError::TooLarge { name } => format!("{name:?} is over the size limit"),
            AssetError::IdTooLong => format!("its id is longer than {} bytes", MAX_PACK_ID - 4),
            AssetError::Package(refusal) => refusal.reason(),
            AssetError::Cache { path } => {
                format!("the cache folder {path:?} is not a plain folder")
            }
            AssetError::Corrupt { path } => {
                format!("the cache folder {path:?} is not what this mod publishes; remove it")
            }
            AssetError::Fence(reason) => reason.clone(),
            AssetError::Io(reason) => reason.clone(),
        }
    }
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

/// What a publication writes: the pack id, the folder name, the manifest and the files.
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub pack_id: String,
    /// `<pack id>-<hash>`: the folder's name under `lua-mods/`.
    pub target_name: String,
    pub manifest: String,
    /// The published names and their bytes, in name order.
    pub files: Vec<(String, Vec<u8>)>,
}

/// The pack id of a mod: `lua_` and its id. Its namespace is the id itself.
pub fn pack_id(id: &str) -> Option<String> {
    let pack = format!("lua_{id}");
    (pack.len() <= MAX_PACK_ID).then_some(pack)
}

/// The side of the pack. A both-side mod's registry content (`data/<ns>/horizon/`) must be on
/// every client, so the pack is `both`; everything else is local (`client`), and a server-side mod
/// stays `server` (its registry content is refused by the fence, `Content::server_only_refusal`).
pub fn pack_side(side: Side, registry: bool) -> &'static str {
    match side {
        Side::Both if registry => "both",
        Side::Server => "server",
        _ => "client",
    }
}

/// The synthesized manifest: the content API, `registries` (the capability a `data/` pack needs),
/// and the pack's own id, name, version and side. Nothing else, so the text is fixed by these.
pub fn manifest_text(pack_id: &str, id: &str, name: &str, version: &str, side: &str) -> String {
    let mut table = toml::Table::new();
    table.insert("id".into(), pack_id.into());
    table.insert("name".into(), name.into());
    table.insert("version".into(), version.into());
    table.insert("api".into(), "0.2".into());
    table.insert("side".into(), side.into());
    table.insert(
        "capabilities".into(),
        toml::Value::Array(vec!["registries".into()]),
    );
    table.insert("namespaces".into(), toml::Value::Array(vec![id.into()]));
    toml::to_string(&table).expect("a table of strings serializes")
}

/// The name a package file is published under: `lang/<file>` moves to the mod's namespace.
fn published_name(id: &str, name: &str) -> String {
    match name.strip_prefix("lang/") {
        Some(rest) => format!("assets/{id}/lang/{rest}"),
        None => name.to_string(),
    }
}

/// Whether a published name is one every system can hold: no empty, `.`, `..`, backslash, colon or
/// control part, no leading `/`, no reserved Windows name and no trailing dot or space in a part.
/// The package reader already refuses these (`hlua::check_name`); this is the same check at the
/// point of writing, so a name that reaches the disk was checked here as well.
pub fn check_target(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name.starts_with('/') {
        return Err("is not a relative path");
    }
    if name
        .chars()
        .any(|c| c.is_control() || c == '\\' || c == ':')
    {
        return Err("holds a character a file name may not have");
    }
    for part in name.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err("has an empty, . or .. part");
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err("has a part that ends in a dot or a space");
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err("names a file Windows reserves");
        }
    }
    Ok(())
}

/// `root` joined with a published name, refused unless the result stays under `root`.
pub(crate) fn contained(root: &Path, name: &str) -> Result<PathBuf, AssetError> {
    let refuse = |why| AssetError::Name {
        name: name.into(),
        why,
    };
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(refuse("leaves the pack"));
    }
    let joined = root.join(path);
    if !joined.starts_with(root) {
        return Err(refuse("leaves the pack"));
    }
    Ok(joined)
}

/// The plan of a package's content, or `None` when it has no `assets/`, `data/` or `lang/` file.
pub fn plan(package: &Package) -> Result<Option<Plan>, AssetError> {
    let names = package.content_files();
    if names.is_empty() {
        return Ok(None);
    }
    if names.len() > MAX_FILES {
        return Err(AssetError::TooManyFiles);
    }
    let manifest = package.manifest();
    let pack = pack_id(&manifest.id).ok_or(AssetError::IdTooLong)?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(names.len());
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for name in &names {
        let published = published_name(&manifest.id, name);
        check_target(&published).map_err(|why| AssetError::Name {
            name: published.clone(),
            why,
        })?;
        if !seen.insert(published.to_ascii_lowercase()) {
            return Err(AssetError::Collision { name: published });
        }
        let bytes = package
            .read_content_file(name)
            .map_err(AssetError::Package)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(AssetError::TooLarge { name: published });
        }
        total += bytes.len() as u64;
        if total > MAX_TOTAL {
            return Err(AssetError::TooLarge { name: published });
        }
        files.push((published, bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let registry = files.iter().any(|(name, _)| {
        let mut parts = name.split('/');
        parts.next() == Some("data") && parts.nth(1) == Some("horizon")
    });
    let side = pack_side(manifest.side, registry);
    let text = manifest_text(&pack, &manifest.id, &manifest.name, &manifest.version, side);
    let mut hasher = Sha256::new();
    hasher.update(b"horizon-lua-pack-v1\0");
    let published = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .chain([(MANIFEST, text.as_bytes())]);
    for (name, bytes) in published {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    let hash: String = hasher
        .finalize()
        .iter()
        .take(HASH_LEN / 2)
        .map(|b| format!("{b:02x}"))
        .collect();
    files.push((MANIFEST.to_string(), text.clone().into_bytes()));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(Plan {
        target_name: format!("{pack}-{hash}"),
        pack_id: pack,
        manifest: text,
        files,
    }))
}

/// Publishes the package's content: the pack folder under `game_dir`'s cache, whose name is the
/// hash of what it holds. Returns the folder, or `None` when the package has no content.
pub fn publish(game_dir: &Path, package: &Package) -> Result<Option<PathBuf>, AssetError> {
    publish_with(game_dir, package, None)
}

/// `publish`, with a test's fault: `fail_at` makes the write of the file with that index fail, after
/// the files before it were written, so the test sees a publication fail in the middle.
pub(crate) fn publish_with(
    game_dir: &Path,
    package: &Package,
    fail_at: Option<usize>,
) -> Result<Option<PathBuf>, AssetError> {
    let Some(plan) = plan(package)? else {
        return Ok(None);
    };
    let root = cache_root(game_dir)?;
    let target = root.join(&plan.target_name);
    if std::fs::symlink_metadata(&target).is_ok() {
        verify(&target, &plan)?;
        remove_stale(&root, &plan);
        return Ok(Some(target));
    }
    let temp = root.join(format!(".tmp-{}-{}", plan.target_name, std::process::id()));
    if let Err(error) = std::fs::create_dir(&temp) {
        return Err(AssetError::Io(format!("{}: {error}", temp.display())));
    }
    if let Err(error) = write_pack(&temp, &plan, fail_at) {
        let _ = std::fs::remove_dir_all(&temp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_dir_all(&temp);
        if std::fs::symlink_metadata(&target).is_ok() {
            // Another game published the same pack first: it must be what this run holds.
            verify(&target, &plan)?;
            remove_stale(&root, &plan);
            return Ok(Some(target));
        }
        return Err(AssetError::Io(format!("{}: {error}", target.display())));
    }
    remove_stale(&root, &plan);
    Ok(Some(target))
}

/// `<game_dir>/.horizon-cache/lua-mods`, created when missing. Each folder on the way must be a
/// plain folder: a symlink or a file there refuses the publication.
fn cache_root(game_dir: &Path) -> Result<PathBuf, AssetError> {
    let cache = game_dir.join(CACHE_DIR);
    let lua = cache.join(DIR);
    for dir in [&cache, &lua] {
        let plain = |dir: &Path| match std::fs::symlink_metadata(dir) {
            Ok(meta) if meta.is_dir() => Ok(true),
            Ok(_) => Err(AssetError::Cache {
                path: dir.display().to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(AssetError::Io(format!("{}: {error}", dir.display()))),
        };
        if !plain(dir)? {
            match std::fs::create_dir(dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(AssetError::Io(format!("{}: {error}", dir.display()))),
            }
            if !plain(dir)? {
                return Err(AssetError::Io(format!("{}: not created", dir.display())));
            }
        }
    }
    Ok(lua)
}

/// Writes a plan into `root` (an empty folder this run made), then checks the result with the
/// content fence: the manifest and folder rules of `horizon_mod_package::Package`, the content rules
/// of `Content`, and the side rule of a server pack. A failure leaves files behind; the caller
/// removes the folder.
fn write_pack(root: &Path, plan: &Plan, fail_at: Option<usize>) -> Result<(), AssetError> {
    for (index, (name, bytes)) in plan.files.iter().enumerate() {
        if cfg!(test) && fail_at == Some(index) {
            return Err(AssetError::Io(format!("{name}: injected failure")));
        }
        write_new(root, name, bytes)?;
    }
    let package = horizon_mod_package::Package::read(root).map_err(AssetError::Fence)?;
    let content =
        horizon_mod_package::Content::read(root, &package.manifest).map_err(AssetError::Fence)?;
    if let Some(reason) = content.server_only_refusal(&package.manifest) {
        return Err(AssetError::Fence(reason));
    }
    sync_dir(root);
    Ok(())
}

/// Creates `root/name` exclusively (it must not exist), its parents first, and writes `bytes`.
fn write_new(root: &Path, name: &str, bytes: &[u8]) -> Result<(), AssetError> {
    let path = contained(root, name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AssetError::Io(format!("{name}: {error}")))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| AssetError::Io(format!("{name}: {error}")))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| AssetError::Io(format!("{name}: {error}")))
}

/// Flushes a folder's entries where the platform allows it (not on Windows).
fn sync_dir(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

/// Checks that `target` is exactly the plan: a plain folder with the expected files and folders,
/// no symlink, no other entry, and each file's bytes equal. Nothing is changed here.
fn verify(target: &Path, plan: &Plan) -> Result<(), AssetError> {
    let corrupt = || AssetError::Corrupt {
        path: target.display().to_string(),
    };
    let meta = std::fs::symlink_metadata(target).map_err(|_| corrupt())?;
    if !meta.is_dir() {
        return Err(corrupt());
    }
    let mut files: BTreeMap<String, &[u8]> = BTreeMap::new();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for (name, bytes) in &plan.files {
        files.insert(name.clone(), bytes);
        let mut at = String::new();
        let parts: Vec<&str> = name.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            if !at.is_empty() {
                at.push('/');
            }
            at.push_str(part);
            dirs.insert(at.clone());
        }
    }
    let mut seen_files = 0usize;
    let mut seen_dirs = 0usize;
    let mut stack: Vec<(PathBuf, String)> = vec![(target.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let children = std::fs::read_dir(&dir).map_err(|_| corrupt())?;
        for child in children {
            let child = child.map_err(|_| corrupt())?;
            let name = child.file_name();
            let name = name.to_str().ok_or_else(corrupt)?;
            let rel = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            let meta = std::fs::symlink_metadata(child.path()).map_err(|_| corrupt())?;
            if meta.is_dir() {
                if !dirs.contains(&rel) {
                    return Err(corrupt());
                }
                seen_dirs += 1;
                stack.push((child.path(), rel));
            } else if meta.is_file() {
                let Some(bytes) = files.get(&rel) else {
                    return Err(corrupt());
                };
                if meta.len() != bytes.len() as u64 {
                    return Err(corrupt());
                }
                let mut held = Vec::with_capacity(bytes.len());
                std::fs::File::open(child.path())
                    .and_then(|file| file.take(bytes.len() as u64 + 1).read_to_end(&mut held))
                    .map_err(|_| corrupt())?;
                if held != *bytes {
                    return Err(corrupt());
                }
                seen_files += 1;
            } else {
                return Err(corrupt());
            }
        }
    }
    if seen_files != files.len() || seen_dirs != dirs.len() {
        return Err(corrupt());
    }
    Ok(())
}

/// Removes the packs of the same mod with another hash, at most `MAX_REMOVALS`. Only a folder named
/// `<pack id>-<16 hex>` that is a real folder (not a symlink) is removed. Returns how many went.
pub fn remove_stale(root: &Path, plan: &Plan) -> usize {
    let prefix = format!("{}-", plan.pack_id);
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if removed >= MAX_REMOVALS {
            break;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == plan.target_name {
            continue;
        }
        let Some(hash) = name.strip_prefix(&prefix) else {
            continue;
        };
        if hash.len() != HASH_LEN
            || !hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            continue;
        }
        let path = entry.path();
        // A symlink is not a real folder: its metadata says so, and it is left alone.
        let real_dir = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
        if real_dir && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}
