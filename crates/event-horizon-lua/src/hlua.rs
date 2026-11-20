//! The `.hlua` package: one Lua mod as a folder in the package layout, or as a zip archive.
//!
//! This part holds the package format, the bounded reader, the typed refusals, and the readers for a
//! folder and for a zip archive. An archive is opened once through a handle, its central directory
//! is read through that handle, every entry is read to the end at open, and later reads go through
//! the same handle; the path is never opened again.
//!
//! A package comes from a player, so every byte is hostile. The reader refuses the whole package
//! on the first thing it cannot account for: it never truncates, never extracts part of a
//! package, and never trusts a size from a header. Each entry is read through a counter that
//! stops one byte past its limit, and a produced size that differs from the header refuses.
//!
//! Layout: the root holds `lua-mod.toml`, `main.luau`, `icon.png`, `LICENSE*`, `README*` and any
//! script the manifest lists in `entries`; the folders `assets/`, `data/` and `lang/` hold the
//! rest. Anything else refuses the package by name.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use zip::read::{ArchiveOffset, Config, ZipFile};
use zip::{CompressionMethod, ZipArchive};

use crate::manifest::{MANIFEST, MAX_MANIFEST, Manifest};

/// Largest `.hlua` file on disk, in bytes.
pub const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
/// Longest zip comment, in bytes.
pub const MAX_COMMENT: usize = 1024;
/// Most entries (files and folders) a package may hold.
pub const MAX_ENTRIES: usize = 4096;
/// Longest entry name, in bytes.
pub const MAX_NAME: usize = 256;
/// Longest single path segment of an entry name, in bytes.
pub const MAX_SEGMENT: usize = 128;
/// Deepest path, in segments.
pub const MAX_DEPTH: usize = 16;
/// Largest uncompressed size of one entry.
pub const MAX_ENTRY: u64 = 8 * 1024 * 1024;
/// Largest uncompressed size of all entries together.
pub const MAX_TOTAL: u64 = 64 * 1024 * 1024;
/// Largest uncompressed-to-compressed ratio, per entry and for the whole package.
pub const MAX_RATIO: u64 = 100;
/// Largest script (`entry_source`), in bytes.
pub const MAX_SCRIPT: u64 = 1024 * 1024;
/// The folders a package may hold, each with any files under it.
const FOLDERS: [&str; 3] = ["assets", "data", "lang"];
/// The end-of-central-directory record, its zip64 locator and the zip64 record, by signature and
/// length.
const EOCD_SIG: [u8; 4] = *b"PK\x05\x06";
const EOCD_LEN: usize = 22;
const CENTRAL_SIG: [u8; 4] = *b"PK\x01\x02";
const LOCATOR_SIG: [u8; 4] = *b"PK\x06\x07";
const LOCATOR_LEN: usize = 20;
const EOCD64_SIG: [u8; 4] = *b"PK\x06\x06";
const EOCD64_LEN: usize = 56;
/// The unix file-type bits of an archive entry's mode (the high 16 bits of its attributes).
const TYPE_MASK: u32 = 0o170000;
const TYPE_FILE: u32 = 0o100000;
const TYPE_DIR: u32 = 0o040000;
const TYPE_LINK: u32 = 0o120000;

/// Why an entry name is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameFault {
    Utf8,
    Control,
    Backslash,
    /// A leading `/`, a drive letter, or a `.` or `..` segment.
    Traversal,
    Colon,
    EmptySegment,
    TooLong,
    SegmentTooLong,
    TooDeep,
}

impl NameFault {
    fn key(self) -> &'static str {
        match self {
            NameFault::Utf8 => "lua_mod.refused.name_not_utf8",
            NameFault::Control => "lua_mod.refused.name_control_char",
            NameFault::Backslash => "lua_mod.refused.name_backslash",
            NameFault::Traversal => "lua_mod.refused.path_traversal",
            NameFault::Colon => "lua_mod.refused.name_colon",
            NameFault::EmptySegment => "lua_mod.refused.name_empty_segment",
            NameFault::TooLong => "lua_mod.refused.name_too_long",
            NameFault::SegmentTooLong => "lua_mod.refused.name_segment_too_long",
            NameFault::TooDeep => "lua_mod.refused.name_too_deep",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            NameFault::Utf8 => "the name is not valid UTF-8",
            NameFault::Control => "the name holds a control character",
            NameFault::Backslash => "the name holds a backslash",
            NameFault::Traversal => "the name leaves the package (absolute, a drive, or . or ..)",
            NameFault::Colon => "the name holds a colon",
            NameFault::EmptySegment => "the name holds an empty path segment",
            NameFault::TooLong => "the name is longer than 256 bytes",
            NameFault::SegmentTooLong => "a path segment is longer than 128 bytes",
            NameFault::TooDeep => "the path is deeper than 16 segments",
        }
    }
}

/// Why a package (or one of its entries, or its manifest) is refused. Every variant has a
/// translation key (`key`) and its arguments (`args`); `reason` is the English text for logs.
/// Text that came from the package is quoted with `{:?}`, which escapes control characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Symlink,
    NotPlain,
    Unreadable,
    Changed,
    ArchiveTooLarge,
    NotZip,
    TrailingData,
    CommentTooLong,
    MultiDisk,
    TooManyEntries,
    BadName { name: String, fault: NameFault },
    Duplicate { name: String },
    Collision { name: String, other: String },
    NotAllowed { name: String },
    EntrySymlink { name: String },
    EntrySpecial { name: String },
    KindMismatch { name: String },
    Encrypted { name: String },
    Method { name: String },
    EntryTooLarge { name: String, max: u64 },
    TotalTooLarge { max: u64 },
    Ratio { name: String },
    TotalRatio,
    SizeMismatch { name: String },
    Corrupt { name: String },
    EntryMissing { name: String },
    NotAnEntry { name: String },
    NotAnAsset { name: String },
    ScriptNotUtf8 { name: String },
    ManifestMissing,
    ManifestTooLarge,
    ManifestNotUtf8,
    ManifestNotToml,
    UnknownKey { key: String },
    MissingKey { key: String },
    WrongType { key: String },
    TextTooLong { field: String, max: u64 },
    BadText { field: String },
    TooMany { field: String, max: u64 },
    BadId,
    ReservedId,
    BadVersion,
    BadSide,
    UnknownPower { name: String },
    DuplicatePower { name: String },
    NoEntries,
    BadEntry { value: String },
    BadDependency { id: String },
    BadRequirement { id: String },
    BadIcon,
    AssetsReserved,
    SettingsTooDeep,
}

impl Refusal {
    /// The translation key of the refusal's text.
    pub fn key(&self) -> &'static str {
        match self {
            Refusal::Symlink => "lua_mod.refused.symlink",
            Refusal::NotPlain => "lua_mod.refused.not_plain",
            Refusal::Unreadable => "lua_mod.refused.unreadable",
            Refusal::Changed => "lua_mod.refused.changed",
            Refusal::ArchiveTooLarge => "lua_mod.refused.archive_too_large",
            Refusal::NotZip => "lua_mod.refused.not_zip",
            Refusal::TrailingData => "lua_mod.refused.trailing_data",
            Refusal::CommentTooLong => "lua_mod.refused.comment_too_long",
            Refusal::MultiDisk => "lua_mod.refused.multi_disk",
            Refusal::TooManyEntries => "lua_mod.refused.too_many_entries",
            Refusal::BadName { fault, .. } => fault.key(),
            Refusal::Duplicate { .. } => "lua_mod.refused.duplicate_name",
            Refusal::Collision { .. } => "lua_mod.refused.name_case_collision",
            Refusal::NotAllowed { .. } => "lua_mod.refused.entry_not_allowed",
            Refusal::EntrySymlink { .. } => "lua_mod.refused.entry_symlink",
            Refusal::EntrySpecial { .. } => "lua_mod.refused.entry_special_file",
            Refusal::KindMismatch { .. } => "lua_mod.refused.entry_kind_mismatch",
            Refusal::Encrypted { .. } => "lua_mod.refused.entry_encrypted",
            Refusal::Method { .. } => "lua_mod.refused.entry_method",
            Refusal::EntryTooLarge { .. } => "lua_mod.refused.entry_too_large",
            Refusal::TotalTooLarge { .. } => "lua_mod.refused.total_too_large",
            Refusal::Ratio { .. } => "lua_mod.refused.entry_ratio",
            Refusal::TotalRatio => "lua_mod.refused.total_ratio",
            Refusal::SizeMismatch { .. } => "lua_mod.refused.size_mismatch",
            Refusal::Corrupt { .. } => "lua_mod.refused.entry_corrupt",
            Refusal::EntryMissing { .. } => "lua_mod.refused.entry_missing",
            Refusal::NotAnEntry { .. } => "lua_mod.refused.entry_not_listed",
            Refusal::NotAnAsset { .. } => "lua_mod.refused.not_an_asset",
            Refusal::ScriptNotUtf8 { .. } => "lua_mod.refused.script_not_utf8",
            Refusal::ManifestMissing => "lua_mod.refused.manifest_missing",
            Refusal::ManifestTooLarge => "lua_mod.refused.manifest_too_large",
            Refusal::ManifestNotUtf8 => "lua_mod.refused.manifest_not_utf8",
            Refusal::ManifestNotToml => "lua_mod.refused.manifest_not_toml",
            Refusal::UnknownKey { .. } => "lua_mod.refused.unknown_key",
            Refusal::MissingKey { .. } => "lua_mod.refused.missing_key",
            Refusal::WrongType { .. } => "lua_mod.refused.wrong_type",
            Refusal::TextTooLong { .. } => "lua_mod.refused.text_too_long",
            Refusal::BadText { .. } => "lua_mod.refused.bad_text",
            Refusal::TooMany { .. } => "lua_mod.refused.too_many",
            Refusal::BadId => "lua_mod.refused.bad_id",
            Refusal::ReservedId => "lua_mod.refused.reserved_id",
            Refusal::BadVersion => "lua_mod.refused.bad_version",
            Refusal::BadSide => "lua_mod.refused.bad_side",
            Refusal::UnknownPower { .. } => "lua_mod.refused.unknown_power",
            Refusal::DuplicatePower { .. } => "lua_mod.refused.duplicate_power",
            Refusal::NoEntries => "lua_mod.refused.no_entries",
            Refusal::BadEntry { .. } => "lua_mod.refused.bad_entry",
            Refusal::BadDependency { .. } => "lua_mod.refused.bad_dependency",
            Refusal::BadRequirement { .. } => "lua_mod.refused.bad_requirement",
            Refusal::BadIcon => "lua_mod.refused.bad_icon",
            Refusal::AssetsReserved => "lua_mod.refused.assets_reserved",
            Refusal::SettingsTooDeep => "lua_mod.refused.settings_too_deep",
        }
    }

    /// The arguments the translation of `key` takes, by name.
    pub fn args(&self) -> Vec<(&'static str, String)> {
        match self {
            Refusal::BadName { name, .. }
            | Refusal::Duplicate { name }
            | Refusal::NotAllowed { name }
            | Refusal::EntrySymlink { name }
            | Refusal::EntrySpecial { name }
            | Refusal::KindMismatch { name }
            | Refusal::Encrypted { name }
            | Refusal::Method { name }
            | Refusal::Ratio { name }
            | Refusal::SizeMismatch { name }
            | Refusal::Corrupt { name }
            | Refusal::EntryMissing { name }
            | Refusal::NotAnEntry { name }
            | Refusal::NotAnAsset { name }
            | Refusal::ScriptNotUtf8 { name }
            | Refusal::UnknownPower { name }
            | Refusal::DuplicatePower { name } => vec![("name", name.clone())],
            Refusal::Collision { name, other } => {
                vec![("name", name.clone()), ("other", other.clone())]
            }
            Refusal::EntryTooLarge { name, max } => {
                vec![("name", name.clone()), ("max", max.to_string())]
            }
            Refusal::TotalTooLarge { max } => vec![("max", max.to_string())],
            Refusal::TooMany { field, max } | Refusal::TextTooLong { field, max } => {
                vec![("field", field.clone()), ("max", max.to_string())]
            }
            Refusal::UnknownKey { key }
            | Refusal::MissingKey { key }
            | Refusal::WrongType { key } => vec![("key", key.clone())],
            Refusal::BadText { field } => vec![("field", field.clone())],
            Refusal::BadDependency { id } | Refusal::BadRequirement { id } => {
                vec![("id", id.clone())]
            }
            Refusal::BadEntry { value } => vec![("value", value.clone())],
            _ => Vec::new(),
        }
    }

    /// The English text, for a log line. Escape it with `host::Escaped` before it is logged.
    pub fn reason(&self) -> String {
        match self {
            Refusal::Symlink => "a symlink is not followed".into(),
            Refusal::NotPlain => "it is not a plain file or folder".into(),
            Refusal::Unreadable => "it cannot be read".into(),
            Refusal::Changed => "it changed while it was opened".into(),
            Refusal::ArchiveTooLarge => "the archive is over 64 MiB".into(),
            Refusal::NotZip => {
                "it is not a readable zip archive, or its end record is damaged".into()
            }
            Refusal::TrailingData => "there are bytes before or after the archive".into(),
            Refusal::CommentTooLong => "the zip comment is over 1 KiB".into(),
            Refusal::MultiDisk => "the archive spans several disks".into(),
            Refusal::TooManyEntries => "the archive holds more than 4096 entries".into(),
            Refusal::BadName { name, fault } => format!("{:?}: {}", name, fault.reason()),
            Refusal::Duplicate { name } => format!("{name:?} is in the package twice"),
            Refusal::Collision { name, other } => {
                format!("{name:?} and {other:?} differ only by case")
            }
            Refusal::NotAllowed { name } => format!("{name:?} is not allowed in a package"),
            Refusal::EntrySymlink { name } => format!("{name:?} is a symlink"),
            Refusal::EntrySpecial { name } => format!("{name:?} is not a file or folder"),
            Refusal::KindMismatch { name } => format!("{name:?} is a folder and a file at once"),
            Refusal::Encrypted { name } => format!("{name:?} is encrypted"),
            Refusal::Method { name } => {
                format!("{name:?} uses a compression method other than stored or deflate")
            }
            Refusal::EntryTooLarge { name, max } => format!("{name:?} is over {max} bytes"),
            Refusal::TotalTooLarge { max } => format!("the entries together are over {max} bytes"),
            Refusal::Ratio { name } => format!("{name:?} compresses more than 100 to 1"),
            Refusal::TotalRatio => "the package compresses more than 100 to 1".into(),
            Refusal::SizeMismatch { name } => format!("{name:?} is not the size its header says"),
            Refusal::Corrupt { name } => format!("{name:?} cannot be decompressed"),
            Refusal::EntryMissing { name } => format!("{name:?} is listed but missing"),
            Refusal::NotAnEntry { name } => format!("{name:?} is not a listed entry"),
            Refusal::NotAnAsset { name } => format!("{name:?} is not an asset"),
            Refusal::ScriptNotUtf8 { name } => format!("{name:?} is not UTF-8"),
            Refusal::ManifestMissing => "lua-mod.toml is missing".into(),
            Refusal::ManifestTooLarge => "lua-mod.toml is over 64 KiB".into(),
            Refusal::ManifestNotUtf8 => "lua-mod.toml is not UTF-8".into(),
            Refusal::ManifestNotToml => "lua-mod.toml is not valid TOML".into(),
            Refusal::UnknownKey { key } => format!("lua-mod.toml has an unknown key {key:?}"),
            Refusal::MissingKey { key } => format!("lua-mod.toml has no {key:?}"),
            Refusal::WrongType { key } => format!("{key:?} has the wrong type"),
            Refusal::TextTooLong { field, max } => format!("{field} is over {max} bytes"),
            Refusal::BadText { field } => format!("{field} holds a control character"),
            Refusal::TooMany { field, max } => format!("{field} holds more than {max} items"),
            Refusal::BadId => "the id is not a valid mod id".into(),
            Refusal::ReservedId => "the id is reserved".into(),
            Refusal::BadVersion => "the version is not a semver version".into(),
            Refusal::BadSide => "side must be client, server or both".into(),
            Refusal::UnknownPower { name } => format!("the power {name:?} is not known"),
            Refusal::DuplicatePower { name } => format!("the power {name:?} is listed twice"),
            Refusal::NoEntries => "entries lists no script".into(),
            Refusal::BadEntry { value } => {
                format!("the entry {value:?} is not a relative .luau path")
            }
            Refusal::BadDependency { id } => format!("the dependency {id:?} is not a valid mod id"),
            Refusal::BadRequirement { id } => format!("the requirement of {id:?} is not supported"),
            Refusal::BadIcon => "icon must be a relative .png path".into(),
            Refusal::AssetsReserved => "assets is reserved in this version".into(),
            Refusal::SettingsTooDeep => "settings nests deeper than 4 levels".into(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

/// One entry of a package, as the reader accounts for it.
#[derive(Clone, Debug)]
struct Entry {
    /// The name as written in the package. A folder ends with `/`.
    name: String,
    dir: bool,
    /// The uncompressed size the header (or the file's length) claims.
    size: u64,
    /// The compressed size the header claims (a folder's file: its length).
    compressed: u64,
    /// The archive's index of the entry (0 in a folder, where it is not used).
    index: usize,
}

/// Where a package's bytes come from.
enum Source {
    /// A folder; each entry is opened by its path under the folder, checked on every read.
    Folder(PathBuf),
    /// An archive, opened once. Entries are read through this handle and nothing else.
    Archive(Mutex<ZipArchive<File>>),
}

/// A checked `.hlua` package, or a folder with the same layout.
pub struct Package {
    manifest: Manifest,
    entries: Vec<Entry>,
    by_name: HashMap<String, usize>,
    source: Source,
}

impl Package {
    /// Opens a folder in the package layout. Symlinks and special files are refused, and the
    /// folder is walked once, with the same rules as an archive.
    pub fn open_dir(path: &Path) -> Result<Package, Refusal> {
        let meta = std::fs::symlink_metadata(path).map_err(|_| Refusal::Unreadable)?;
        if meta.file_type().is_symlink() {
            return Err(Refusal::Symlink);
        }
        if !meta.is_dir() {
            return Err(Refusal::NotPlain);
        }
        let entries = walk(path)?;
        finish(Source::Folder(path.to_path_buf()), entries)
    }

    /// Opens a `.hlua` archive. The file is opened once, through a handle that must be a plain
    /// file and the file the path names. Its end record is checked, the central directory is read
    /// through that handle, every entry's header is checked, and every file is read to its end (its
    /// CRC is checked by the zip reader) before the package is returned.
    pub fn open_archive(path: &Path) -> Result<Package, Refusal> {
        let mut file = open_plain(path)?;
        let len = file.metadata().map_err(|_| Refusal::Unreadable)?.len();
        if len > MAX_ARCHIVE {
            return Err(Refusal::ArchiveTooLarge);
        }
        let directory = check_end_record(&mut file, len)?;
        if directory.count > MAX_ENTRIES as u64 {
            return Err(Refusal::TooManyEntries);
        }
        check_directory(&mut file, &directory)?;
        // A fixed offset of zero: the zip reader does not search for the directory, so bytes
        // in front of the archive refuse it instead of being skipped.
        let config = Config {
            archive_offset: ArchiveOffset::Known(0),
        };
        let mut archive = ZipArchive::with_config(config, file).map_err(|_| Refusal::NotZip)?;
        if archive.offset() != 0 || archive.len() as u64 != directory.count {
            return Err(Refusal::NotZip);
        }
        let mut entries = Vec::with_capacity(archive.len());
        for index in 0..archive.len() {
            let raw = archive.by_index_raw(index).map_err(|_| Refusal::NotZip)?;
            entries.push(archive_entry(&raw, index)?);
        }
        check_entries(&entries)?;
        verify_contents(&mut archive, &entries)?;
        finish(Source::Archive(Mutex::new(archive)), entries)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The text of a listed script. Only a name in the manifest's `entries` is read.
    pub fn entry_source(&self, name: &str) -> Result<String, Refusal> {
        if !self.manifest.entries.iter().any(|e| e == name) {
            return Err(Refusal::NotAnEntry { name: name.into() });
        }
        let bytes = self.read_file(name, MAX_SCRIPT)?;
        String::from_utf8(bytes).map_err(|_| Refusal::ScriptNotUtf8 { name: name.into() })
    }

    /// The names of the package's files under `assets/`, in name order.
    pub fn asset_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !e.dir && e.name.starts_with("assets/"))
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        names
    }

    /// The bytes of one asset (`assets/...`), at most `MAX_ENTRY`.
    pub fn read_asset(&self, name: &str) -> Result<Vec<u8>, Refusal> {
        if !name.starts_with("assets/") {
            return Err(Refusal::NotAnAsset { name: name.into() });
        }
        self.read_file(name, MAX_ENTRY)
    }

    /// Reads a file of the package, refusing it if it is missing, a folder, or over `limit`.
    fn read_file(&self, name: &str, limit: u64) -> Result<Vec<u8>, Refusal> {
        let entry = self
            .by_name
            .get(name)
            .map(|&i| &self.entries[i])
            .filter(|e| !e.dir)
            .ok_or_else(|| Refusal::EntryMissing { name: name.into() })?;
        read_bytes(&self.source, entry, limit)
    }
}

/// Builds the package once its entries are known: names, sizes and the layout are checked, the
/// manifest is read and parsed, and the entries it lists must be there.
fn finish(source: Source, entries: Vec<Entry>) -> Result<Package, Refusal> {
    check_entries(&entries)?;
    let by_name: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.clone(), i))
        .collect();
    let manifest_entry = by_name
        .get(MANIFEST)
        .map(|&i| &entries[i])
        .filter(|e| !e.dir)
        .ok_or(Refusal::ManifestMissing)?;
    let bytes = read_bytes(&source, manifest_entry, MAX_MANIFEST).map_err(|e| match e {
        Refusal::EntryTooLarge { .. } => Refusal::ManifestTooLarge,
        other => other,
    })?;
    let manifest = Manifest::parse(&bytes)?;

    for entry in &entries {
        allowed(&entry.name, entry.dir, &manifest.entries)?;
    }
    for listed in &manifest.entries {
        match by_name.get(listed).map(|&i| &entries[i]) {
            Some(e) if !e.dir => {}
            _ => {
                return Err(Refusal::EntryMissing {
                    name: listed.clone(),
                });
            }
        }
    }
    if let Some(icon) = &manifest.icon {
        match by_name.get(icon).map(|&i| &entries[i]) {
            Some(e) if !e.dir => {}
            _ => return Err(Refusal::BadIcon),
        }
    }
    Ok(Package {
        manifest,
        entries,
        by_name,
        source,
    })
}

/// The checks that do not need the manifest: names, duplicates, collisions and sizes.
fn check_entries(entries: &[Entry]) -> Result<(), Refusal> {
    if entries.len() > MAX_ENTRIES {
        return Err(Refusal::TooManyEntries);
    }
    let mut seen: HashMap<String, &str> = HashMap::with_capacity(entries.len());
    for entry in entries {
        check_name(&entry.name).map_err(|fault| Refusal::BadName {
            name: entry.name.clone(),
            fault,
        })?;
        match seen.insert(entry.name.to_ascii_lowercase(), &entry.name) {
            None => {}
            Some(prev) if prev == entry.name => {
                return Err(Refusal::Duplicate {
                    name: entry.name.clone(),
                });
            }
            Some(prev) => {
                return Err(Refusal::Collision {
                    name: entry.name.clone(),
                    other: prev.to_string(),
                });
            }
        }
    }
    let (mut total, mut total_compressed) = (0u64, 0u64);
    for entry in entries {
        if entry.size > MAX_ENTRY {
            return Err(Refusal::EntryTooLarge {
                name: entry.name.clone(),
                max: MAX_ENTRY,
            });
        }
        if entry.size > entry.compressed.saturating_mul(MAX_RATIO) {
            return Err(Refusal::Ratio {
                name: entry.name.clone(),
            });
        }
        total = total.saturating_add(entry.size);
        total_compressed = total_compressed.saturating_add(entry.compressed);
    }
    if total > MAX_TOTAL {
        return Err(Refusal::TotalTooLarge { max: MAX_TOTAL });
    }
    if total > total_compressed.saturating_mul(MAX_RATIO) {
        return Err(Refusal::TotalRatio);
    }
    Ok(())
}

/// Whether an entry may be in the package: a file or folder under `assets/`, `data/` or `lang/`,
/// a root file the layout names, or a script the manifest lists. Anything else is refused.
fn allowed(name: &str, dir: bool, listed: &[String]) -> Result<(), Refusal> {
    let path = name.strip_suffix('/').unwrap_or(name);
    let first = path.split('/').next().unwrap_or("");
    let root = !path.contains('/');
    if FOLDERS.contains(&first) {
        // A file named like a folder would collide with the folder's own entries.
        if root && !dir {
            return Err(Refusal::NotAllowed { name: name.into() });
        }
        return Ok(());
    }
    if dir {
        // A folder is allowed only as the ancestor of a listed script (it holds no data itself).
        let prefix = format!("{path}/");
        if listed.iter().any(|l| l.starts_with(&prefix)) {
            return Ok(());
        }
        return Err(Refusal::NotAllowed { name: name.into() });
    }
    let root_file = matches!(path, "lua-mod.toml" | "main.luau" | "icon.png") || is_document(path);
    if (root && root_file) || listed.iter().any(|l| l == path) {
        return Ok(());
    }
    Err(Refusal::NotAllowed { name: name.into() })
}

/// `LICENSE`, `LICENSE.txt`, `README-en.md` and the like, in any case.
fn is_document(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ["license", "readme"].iter().any(|stem| {
        lower == *stem
            || lower
                .strip_prefix(*stem)
                .is_some_and(|rest| rest.starts_with(['.', '-', '_']))
    })
}

/// Checks one entry name: UTF-8 and control characters were checked by the caller, the rest here.
/// A name is relative, uses `/`, has no `.`, `..`, empty or colon-bearing segment, and stays
/// within the length and depth caps. A trailing `/` marks a folder.
pub fn check_name(name: &str) -> Result<(), NameFault> {
    if name.is_empty() {
        return Err(NameFault::EmptySegment);
    }
    if name.len() > MAX_NAME {
        return Err(NameFault::TooLong);
    }
    if name.chars().any(char::is_control) {
        return Err(NameFault::Control);
    }
    if name.contains('\\') {
        return Err(NameFault::Backslash);
    }
    let bytes = name.as_bytes();
    if bytes[0] == b'/' || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return Err(NameFault::Traversal);
    }
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.len() > MAX_DEPTH {
        return Err(NameFault::TooDeep);
    }
    for segment in segments {
        if segment.is_empty() {
            return Err(NameFault::EmptySegment);
        }
        if segment == "." || segment == ".." {
            return Err(NameFault::Traversal);
        }
        if segment.contains(':') {
            return Err(NameFault::Colon);
        }
        if segment.len() > MAX_SEGMENT {
            return Err(NameFault::SegmentTooLong);
        }
    }
    Ok(())
}

/// Walks a folder in the package layout. Each name goes through `check_name` before the walk
/// descends into it, so the recursion is bounded by `MAX_DEPTH`.
fn walk(root: &Path) -> Result<Vec<Entry>, Refusal> {
    let mut out: Vec<Entry> = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        // Sorted, so a refusal names the same entry every time (read_dir order is arbitrary).
        let mut children = std::fs::read_dir(&dir)
            .map_err(|_| Refusal::Unreadable)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Refusal::Unreadable)?;
        children.sort_by_key(|child| child.file_name());
        for child in children {
            let file_name = child.file_name();
            let part = file_name.to_str().ok_or_else(|| Refusal::BadName {
                name: format!("{prefix}{}", file_name.to_string_lossy()),
                fault: NameFault::Utf8,
            })?;
            let meta = std::fs::symlink_metadata(child.path()).map_err(|_| Refusal::Unreadable)?;
            let dir_entry = meta.is_dir();
            let mut name = format!("{prefix}{part}");
            if dir_entry {
                name.push('/');
            }
            if meta.file_type().is_symlink() {
                return Err(Refusal::EntrySymlink { name });
            }
            if !meta.is_file() && !dir_entry {
                return Err(Refusal::EntrySpecial { name });
            }
            check_name(&name).map_err(|fault| Refusal::BadName {
                name: name.clone(),
                fault,
            })?;
            if out.len() >= MAX_ENTRIES {
                return Err(Refusal::TooManyEntries);
            }
            let size = if dir_entry { 0 } else { meta.len() };
            out.push(Entry {
                name: name.clone(),
                dir: dir_entry,
                size,
                compressed: size,
                index: 0,
            });
            if dir_entry {
                stack.push((child.path(), name));
            }
        }
    }
    Ok(out)
}

/// Reads an entry's bytes through the package's source, refusing more than `limit`.
fn read_bytes(source: &Source, entry: &Entry, limit: u64) -> Result<Vec<u8>, Refusal> {
    match source {
        Source::Folder(root) => {
            let file = open_within(root, &entry.name)?;
            read_capped(file, entry, limit)
        }
        Source::Archive(archive) => {
            let mut archive = archive.lock().map_err(|_| Refusal::Unreadable)?;
            let reader = archive
                .by_index(entry.index)
                .map_err(|_| Refusal::Corrupt {
                    name: entry.name.clone(),
                })?;
            read_capped(reader, entry, limit)
        }
    }
}

/// The account of one archive entry, from its central-directory record. Nothing is read.
/// Encrypted entries, other methods, symlinks and special files are refused by name.
fn archive_entry(raw: &ZipFile<'_, File>, index: usize) -> Result<Entry, Refusal> {
    let name = match std::str::from_utf8(raw.name_raw()) {
        Ok(name) => name.to_string(),
        Err(_) => {
            return Err(Refusal::BadName {
                name: String::from_utf8_lossy(raw.name_raw()).into_owned(),
                fault: NameFault::Utf8,
            });
        }
    };
    check_name(&name).map_err(|fault| Refusal::BadName {
        name: name.clone(),
        fault,
    })?;
    if raw.encrypted() {
        return Err(Refusal::Encrypted { name });
    }
    if !matches!(
        raw.compression(),
        CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
        return Err(Refusal::Method { name });
    }
    let dir = name.ends_with('/');
    if let Some(mode) = raw.unix_mode() {
        let wanted = if dir { TYPE_DIR } else { TYPE_FILE };
        match mode & TYPE_MASK {
            0 => {}
            TYPE_LINK => return Err(Refusal::EntrySymlink { name }),
            kind if kind == wanted => {}
            TYPE_FILE | TYPE_DIR => return Err(Refusal::KindMismatch { name }),
            _ => return Err(Refusal::EntrySpecial { name }),
        }
    }
    Ok(Entry {
        name,
        dir,
        size: raw.size(),
        compressed: raw.compressed_size(),
        index,
    })
}

/// Reads every file of an archive once through the bounded reader, so a damaged entry (a bad CRC,
/// bytes that disagree with the header, a bomb) refuses the package before it is returned.
fn verify_contents(archive: &mut ZipArchive<File>, entries: &[Entry]) -> Result<(), Refusal> {
    for entry in entries.iter().filter(|e| !e.dir) {
        let reader = archive
            .by_index(entry.index)
            .map_err(|_| Refusal::Corrupt {
                name: entry.name.clone(),
            })?;
        read_capped(reader, entry, MAX_ENTRY)?;
    }
    Ok(())
}

/// Where an archive's central directory is, and how many records it declares.
struct Directory {
    /// The offset of the first record.
    start: u64,
    /// The offset just past the last record.
    end: u64,
    count: u64,
}

/// The end of an archive: the end record must end the file, name one disk, and sit directly
/// after the central directory, so bytes before the directory or after the record refuse the
/// archive. A comment over `MAX_COMMENT` is refused. The record is searched for in the last
/// `u16::MAX + 22` bytes only, where the zip format puts it.
fn check_end_record(file: &mut File, len: u64) -> Result<Directory, Refusal> {
    let base = len.saturating_sub(EOCD_LEN as u64 + u64::from(u16::MAX));
    let tail = read_at(file, base, len)?;
    let mut found = None;
    let mut any_signature = false;
    for pos in (0..tail.len().saturating_sub(EOCD_LEN - 1)).rev() {
        if tail[pos..pos + 4] != EOCD_SIG {
            continue;
        }
        any_signature = true;
        if pos + EOCD_LEN + usize::from(le16(&tail, pos + 20)) == tail.len() {
            found = Some(pos);
            break;
        }
    }
    let Some(pos) = found else {
        return Err(if any_signature {
            Refusal::TrailingData
        } else {
            Refusal::NotZip
        });
    };
    if usize::from(le16(&tail, pos + 20)) > MAX_COMMENT {
        return Err(Refusal::CommentTooLong);
    }
    let end_record = base + pos as u64;
    let disk = le16(&tail, pos + 4);
    let cd_disk = le16(&tail, pos + 6);
    let disk_entries = le16(&tail, pos + 8);
    let entries = le16(&tail, pos + 10);
    let cd_size = u64::from(le32(&tail, pos + 12));
    let cd_at = u64::from(le32(&tail, pos + 16));
    let zip64 = [disk, cd_disk, disk_entries, entries].contains(&u16::MAX)
        || cd_size == u64::from(u32::MAX)
        || cd_at == u64::from(u32::MAX);
    if !zip64 {
        if disk != 0 || cd_disk != 0 || disk_entries != entries {
            return Err(Refusal::MultiDisk);
        }
        if cd_at.checked_add(cd_size) != Some(end_record) {
            return Err(Refusal::TrailingData);
        }
        return Ok(Directory {
            start: cd_at,
            end: end_record,
            count: u64::from(entries),
        });
    }
    check_zip64(file, &tail, pos, base)
}

/// A zip64 archive: the locator sits just before the end record and names the zip64 record,
/// which must say the archive is on one disk and must end where the central directory ends.
fn check_zip64(file: &mut File, tail: &[u8], pos: usize, base: u64) -> Result<Directory, Refusal> {
    if pos < LOCATOR_LEN || tail[pos - LOCATOR_LEN..pos - LOCATOR_LEN + 4] != LOCATOR_SIG {
        return Err(Refusal::NotZip);
    }
    let locator = pos - LOCATOR_LEN;
    if le32(tail, locator + 4) != 0 || le32(tail, locator + 16) != 1 {
        return Err(Refusal::MultiDisk);
    }
    let record_at = le64(tail, locator + 8);
    let locator_at = base + locator as u64;
    match record_at.checked_add(EOCD64_LEN as u64) {
        Some(end) if end <= locator_at => {}
        _ => return Err(Refusal::NotZip),
    }
    let record = read_at(file, record_at, record_at + EOCD64_LEN as u64)?;
    if record[..4] != EOCD64_SIG {
        return Err(Refusal::NotZip);
    }
    if le32(&record, 16) != 0 || le32(&record, 20) != 0 || le64(&record, 24) != le64(&record, 32) {
        return Err(Refusal::MultiDisk);
    }
    let cd_size = le64(&record, 40);
    let cd_at = le64(&record, 48);
    if cd_at.checked_add(cd_size) != Some(record_at) {
        return Err(Refusal::TrailingData);
    }
    Ok(Directory {
        start: cd_at,
        end: record_at,
        count: le64(&record, 32),
    })
}

/// Walks the central directory record by record, from its first to its last byte, and refuses
/// an exact duplicate by name. The zip reader keeps one of two equal names and drops the other,
/// so the walk is what names it. The records must fill the directory exactly.
fn check_directory(file: &mut File, directory: &Directory) -> Result<(), Refusal> {
    let mut seen: HashSet<Vec<u8>> = HashSet::with_capacity(directory.count as usize);
    let mut at = directory.start;
    for _ in 0..directory.count {
        if at.checked_add(46).is_none_or(|end| end > directory.end) {
            return Err(Refusal::NotZip);
        }
        let header = read_at(file, at, at + 46)?;
        if header[..4] != CENTRAL_SIG {
            return Err(Refusal::NotZip);
        }
        let name_len = u64::from(le16(&header, 28));
        let extra_len = u64::from(le16(&header, 30));
        let comment_len = u64::from(le16(&header, 32));
        let next = at + 46 + name_len + extra_len + comment_len;
        if next > directory.end {
            return Err(Refusal::NotZip);
        }
        let name = read_at(file, at + 46, at + 46 + name_len)?;
        if !seen.insert(name.clone()) {
            return Err(Refusal::Duplicate {
                name: String::from_utf8_lossy(&name).into_owned(),
            });
        }
        at = next;
    }
    if at != directory.end {
        return Err(Refusal::TrailingData);
    }
    Ok(())
}

/// Reads the bytes `start..end` of a file, through its handle.
fn read_at(file: &mut File, start: u64, end: u64) -> Result<Vec<u8>, Refusal> {
    let mut bytes = vec![0u8; (end - start) as usize];
    file.seek(SeekFrom::Start(start))
        .map_err(|_| Refusal::Unreadable)?;
    file.read_exact(&mut bytes)
        .map_err(|_| Refusal::Unreadable)?;
    Ok(bytes)
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(b)
}

/// Reads at most `limit + 1` bytes, so a stream that runs past its limit is caught by its size
/// and not by its header. The produced size must then be the size the header claimed.
fn read_capped<R: Read>(reader: R, entry: &Entry, limit: u64) -> Result<Vec<u8>, Refusal> {
    read_counted(reader, &entry.name, entry.size, limit)
}

/// Reads `reader` to its end, at most `limit + 1` bytes, and checks what it produced: over `limit`
/// is refused, and so is a length other than the `size` the header declared. `pub(crate)` for the
/// tests, which feed it streams that lie.
pub(crate) fn read_counted<R: Read>(
    reader: R,
    name: &str,
    size: u64,
    limit: u64,
) -> Result<Vec<u8>, Refusal> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refusal::Corrupt { name: name.into() })?;
    let produced = bytes.len() as u64;
    if produced > limit {
        return Err(Refusal::EntryTooLarge {
            name: name.into(),
            max: limit,
        });
    }
    if produced != size {
        return Err(Refusal::SizeMismatch { name: name.into() });
    }
    Ok(bytes)
}

/// Opens a folder's file by its entry name. Every folder on the way is checked too, so a folder
/// swapped for a symlink after the walk is refused.
fn open_within(root: &Path, name: &str) -> Result<File, Refusal> {
    let parts: Vec<&str> = name.split('/').collect();
    let mut path = root.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        path.push(part);
        let meta = std::fs::symlink_metadata(&path).map_err(|_| Refusal::Changed)?;
        if meta.file_type().is_symlink() {
            return Err(Refusal::Symlink);
        }
        if !meta.is_dir() {
            return Err(Refusal::Changed);
        }
    }
    path.push(parts[parts.len() - 1]);
    open_plain(&path)
}

/// Opens a regular file. A symlink is refused, and the opened handle must name the same file
/// the path names now (device and inode), so a swap between the check and the open is caught.
fn open_plain(path: &Path) -> Result<File, Refusal> {
    let before = std::fs::symlink_metadata(path).map_err(|_| Refusal::Unreadable)?;
    if before.file_type().is_symlink() {
        return Err(Refusal::Symlink);
    }
    if !before.is_file() {
        return Err(Refusal::NotPlain);
    }
    let file = File::open(path).map_err(|_| Refusal::Unreadable)?;
    if !file.metadata().map_err(|_| Refusal::Unreadable)?.is_file() {
        return Err(Refusal::NotPlain);
    }
    if !same_file(&file, path) {
        return Err(Refusal::Changed);
    }
    Ok(file)
}

/// Whether `path` still names the file `opened` was opened from. A path that is now a symlink,
/// or not a plain file, never does.
pub(crate) fn same_file(opened: &File, path: &Path) -> bool {
    let Ok(now) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if now.file_type().is_symlink() || !now.is_file() {
        return false;
    }
    same_inode(opened, &now)
}

#[cfg(unix)]
fn same_inode(opened: &File, now: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    opened
        .metadata()
        .is_ok_and(|o| o.dev() == now.dev() && o.ino() == now.ino())
}

/// Other platforms have no inode to compare: the symlink and plain-file checks are all there is.
#[cfg(not(unix))]
fn same_inode(_opened: &File, _now: &std::fs::Metadata) -> bool {
    true
}
