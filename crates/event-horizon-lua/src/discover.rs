//! Finds the Lua mods: the packages in `<game_dir>/lua-mods/` (folders and `.hlua` archives), the
//! dev mods named by the caller, and the enabled-state file that turns some of them off.
//!
//! Every package is opened through `hlua::Package`, which is the only reader of untrusted bytes.
//! A package that does not pass is refused with its translation key and arguments (`Fault`), and
//! the others still load. The result is ordered: a dependency loads before the mods that need it
//! (`load_order`), and the order is the same on every run.
//!
//! Rules, in order: a symlink that names a package or points at a folder is refused by name; other
//! entries of `lua-mods/` are ignored; at most `MAX_DEV_PATHS` dev paths and `MAX_MODS` candidates
//! are read, the rest refused; a dev mod replaces an installed one with its id; the first of two
//! mods with one id by file name is kept; a disabled installed mod is listed, not loaded.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use crate::manifest::{MANIFEST, MAX_MANIFEST, Side};

use crate::enabled;
use crate::hlua::{Package, Refusal};
use crate::load_order::{self, Dep, Entry, Presence};
use crate::manifest::Power;
use crate::powers;

/// The folder under the game folder that holds the Lua mods.
pub const DIR: &str = "lua-mods";
/// The script a package runs when its manifest lists none.
pub const ENTRY: &str = "main.luau";
/// The most dev paths read; the rest are refused.
pub const MAX_DEV_PATHS: usize = 8;
/// The most mods read in one scan (dev paths and installed packages together); the rest refused.
pub const MAX_MODS: usize = 1024;

/// The translation key of a client-only mod a dedicated server does not load. It is a notice, not
/// a refusal: the mod is correct, it just has no side to run on here.
pub const SKIPPED_CLIENT_ONLY: &str = "lua_mod.skipped.client_only_on_server";
/// The extension of a `.hlua` archive (case-sensitive).
pub const ARCHIVE_EXT: &str = "hlua";

/// A package that passed: its id, version and side as its manifest states them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovered {
    /// The package's folder, or its `.hlua` file.
    pub dir: PathBuf,
    pub id: String,
    pub version: String,
    pub side: Side,
}

/// Why something was refused: a package reader refusal, or a rule of the scan. Each has a
/// translation key (`key`), its arguments by name (`args`) and the English text for a log (`reason`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Package(Refusal),
    /// An installed mod whose id an earlier mod (by file name) already has.
    DuplicateId {
        id: String,
        first: String,
    },
    /// An installed mod whose id a dev mod takes.
    ReplacedByDev {
        id: String,
    },
    DevPathMissing,
    /// A dev path that is a file and not a `.hlua` archive.
    DevPathNotHlua,
    DevPathsTooMany {
        max: usize,
    },
    TooManyMods {
        max: usize,
    },
    DependencyMissing {
        id: String,
        requirement: String,
    },
    DependencyVersion {
        id: String,
        requirement: String,
        found: String,
    },
    DependencyDisabled {
        id: String,
    },
    DependencyRefused {
        id: String,
    },
    DependencyCycle,
}

impl Fault {
    /// The translation key of the text.
    pub fn key(&self) -> &'static str {
        match self {
            Fault::Package(refusal) => refusal.key(),
            Fault::DuplicateId { .. } => "lua_mod.refused.duplicate_id",
            Fault::ReplacedByDev { .. } => "lua_mod.refused.replaced_by_dev",
            Fault::DevPathMissing => "lua_mod.refused.dev_path_missing",
            Fault::DevPathNotHlua => "lua_mod.refused.dev_path_not_hlua",
            Fault::DevPathsTooMany { .. } => "lua_mod.refused.dev_paths_too_many",
            Fault::TooManyMods { .. } => "lua_mod.refused.too_many_mods",
            Fault::DependencyMissing { .. } => "lua_mod.refused.dependency_missing",
            Fault::DependencyVersion { .. } => "lua_mod.refused.dependency_version",
            Fault::DependencyDisabled { .. } => "lua_mod.refused.dependency_disabled",
            Fault::DependencyRefused { .. } => "lua_mod.refused.dependency_refused",
            Fault::DependencyCycle => "lua_mod.refused.dependency_cycle",
        }
    }

    /// The arguments the translation of `key` takes, by name.
    pub fn args(&self) -> Vec<(&'static str, String)> {
        match self {
            Fault::Package(refusal) => refusal.args(),
            Fault::DuplicateId { id, first } => {
                vec![("id", id.clone()), ("first", first.clone())]
            }
            Fault::ReplacedByDev { id }
            | Fault::DependencyDisabled { id }
            | Fault::DependencyRefused { id } => vec![("id", id.clone())],
            Fault::DevPathsTooMany { max } | Fault::TooManyMods { max } => {
                vec![("max", max.to_string())]
            }
            Fault::DependencyMissing { id, requirement } => {
                vec![("id", id.clone()), ("requirement", requirement.clone())]
            }
            Fault::DependencyVersion {
                id,
                requirement,
                found,
            } => vec![
                ("id", id.clone()),
                ("requirement", requirement.clone()),
                ("found", found.clone()),
            ],
            Fault::DevPathMissing | Fault::DevPathNotHlua | Fault::DependencyCycle => Vec::new(),
        }
    }

    /// The English text, for a log line. Escape it before it is logged.
    pub fn reason(&self) -> String {
        match self {
            Fault::Package(refusal) => refusal.reason(),
            Fault::DuplicateId { id, first } => {
                format!("the id {id:?} is already used by {first:?}")
            }
            Fault::ReplacedByDev { id } => format!("a dev mod with the id {id:?} replaces it"),
            Fault::DevPathMissing => "the dev path does not exist".into(),
            Fault::DevPathNotHlua => "a dev file must be a .hlua archive or a folder".into(),
            Fault::DevPathsTooMany { max } => format!("more than {max} dev paths are given"),
            Fault::TooManyMods { max } => format!("more than {max} mods are scanned"),
            Fault::DependencyMissing { id, requirement } => {
                format!("it needs {id:?} {requirement:?}, which is not installed")
            }
            Fault::DependencyVersion {
                id,
                requirement,
                found,
            } => format!("it needs {id:?} {requirement:?}, but {found:?} is installed"),
            Fault::DependencyDisabled { id } => format!("it needs {id:?}, which is disabled"),
            Fault::DependencyRefused { id } => format!("it needs {id:?}, which was refused"),
            Fault::DependencyCycle => "it is part of a dependency cycle".into(),
        }
    }
}

/// A package the scan did not load, with the path it came from and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub path: PathBuf,
    pub fault: Fault,
}

/// A package that loads: its id, version and side, the package itself (read through `Package`),
/// whether it is a dev mod, and its position in the load order. Its powers are the manifest's
/// `requested` list, the `granted` list the enabled-state file gives its id, and `powers`, the
/// effective set (`powers::effective`: requested and granted, minus the reserved ones). Only
/// `powers` is applied; `requested` and `granted` are kept for the Mods screen.
pub struct ScannedMod {
    pub found: Discovered,
    pub dev: bool,
    pub package: Package,
    pub index: usize,
    pub requested: Vec<Power>,
    pub granted: Vec<Power>,
    pub powers: Vec<Power>,
    /// The owner's limits on the gated writes (`[limits]`, `powers::Limits`).
    pub limits: crate::powers::Limits,
    /// The owner's `[command_allow]` list for the mod (`command_guard.rs`); `None` for the default set.
    pub command_allow: Option<Vec<String>>,
}

/// What a scan found. `loaded` is in load order. `disabled` lists the installed mods the enabled
/// file turned off, in name order. `skipped` lists the client-only mods a dedicated server does not
/// load (`scan_on`), in name order. `notices` are the warnings about the enabled file (one per problem).
#[derive(Default)]
pub struct Scan {
    pub loaded: Vec<ScannedMod>,
    pub refused: Vec<Refused>,
    pub disabled: Vec<Discovered>,
    pub skipped: Vec<Discovered>,
    pub notices: Vec<String>,
    /// The powers each mod's manifest requests, by id, for every mod whose manifest was read: loaded,
    /// disabled and refused by the plan. The Mods screen lists them; the granted ones are read from the
    /// file when it asks, so they are not kept here.
    pub requested: HashMap<String, Vec<Power>>,
}

/// One path the scan reads: a dev path (canonical) or an entry of `lua-mods/`.
struct Candidate {
    path: PathBuf,
    dev: bool,
}

/// A package that opened and whose id is not taken by an earlier one.
struct Kept {
    found: Discovered,
    dev: bool,
    package: Package,
}

/// Scans `<game_dir>/lua-mods/` and the dev paths. A missing folder is not an error. `dev_paths` are
/// folders or `.hlua` files; a dev mod is always enabled, is listed first among equals, and replaces
/// an installed mod with its id.
pub fn scan(game_dir: &Path, dev_paths: &[PathBuf]) -> Scan {
    scan_on(game_dir, dev_paths, false)
}

/// `scan`, for the game's role: on a dedicated server (`dedicated`) a client-only mod is skipped,
/// as `Scan::skipped`, and counts as disabled for the mods that depend on it.
pub fn scan_on(game_dir: &Path, dev_paths: &[PathBuf], dedicated: bool) -> Scan {
    let mut out = Scan::default();
    let states = enabled::read(game_dir);
    out.notices.extend(states.notice.clone());
    crate::display::set_mode(states.display);

    let mut candidates = Vec::new();
    for (k, path) in dev_paths.iter().enumerate() {
        if k >= MAX_DEV_PATHS {
            out.refused.push(Refused {
                path: path.clone(),
                fault: Fault::DevPathsTooMany { max: MAX_DEV_PATHS },
            });
            continue;
        }
        match std::fs::canonicalize(path) {
            Ok(canonical) => candidates.push(Candidate {
                path: canonical,
                dev: true,
            }),
            Err(_) => out.refused.push(Refused {
                path: path.clone(),
                fault: Fault::DevPathMissing,
            }),
        }
    }
    for path in installed(game_dir, &mut out.refused) {
        candidates.push(Candidate { path, dev: false });
    }

    // The opened packages, in candidate order: dev paths first, then installed by file name.
    let mut kept: Vec<Kept> = Vec::new();
    let mut taken: HashMap<String, String> = HashMap::new();
    let mut dev_ids: HashSet<String> = HashSet::new();
    for (index, candidate) in candidates.into_iter().enumerate() {
        if index >= MAX_MODS {
            out.refused.push(Refused {
                path: candidate.path,
                fault: Fault::TooManyMods { max: MAX_MODS },
            });
            continue;
        }
        let package = match open(&candidate.path) {
            Ok(package) => package,
            Err(fault) => {
                out.refused.push(Refused {
                    path: candidate.path,
                    fault,
                });
                continue;
            }
        };
        let manifest = package.manifest();
        let id = manifest.id.clone();
        let found = Discovered {
            dir: candidate.path.clone(),
            id: id.clone(),
            version: manifest.version.clone(),
            side: manifest.side,
        };
        let name = file_name(&candidate.path);
        let fault = if candidate.dev {
            match taken.get(&id) {
                Some(first) => Some(Fault::DuplicateId {
                    id: id.clone(),
                    first: first.clone(),
                }),
                None => {
                    dev_ids.insert(id.clone());
                    None
                }
            }
        } else if dev_ids.contains(&id) {
            Some(Fault::ReplacedByDev { id: id.clone() })
        } else {
            taken.get(&id).map(|first| Fault::DuplicateId {
                id: id.clone(),
                first: first.clone(),
            })
        };
        match fault {
            Some(fault) => out.refused.push(Refused {
                path: candidate.path,
                fault,
            }),
            None => {
                taken.entry(id).or_insert(name);
                kept.push(Kept {
                    found,
                    dev: candidate.dev,
                    package,
                });
            }
        }
    }

    // What the planner sees: each enabled mod as an entry, each disabled one as its id.
    let mut entries: Vec<Entry> = Vec::new();
    let mut entry_kept: Vec<usize> = Vec::new();
    let mut presence: HashMap<String, Presence> = HashMap::new();
    for (k, item) in kept.iter().enumerate() {
        let id = &item.found.id;
        if !item.dev && !states.is_enabled(id) {
            out.requested
                .insert(id.clone(), item.package.manifest().powers.clone());
            out.disabled.push(item.found.clone());
            presence.insert(id.clone(), Presence::Disabled);
            continue;
        }
        if dedicated && item.found.side == Side::Client {
            out.skipped.push(item.found.clone());
            presence.insert(id.clone(), Presence::Disabled);
            continue;
        }
        presence.insert(id.clone(), Presence::Enabled(entries.len()));
        entries.push(Entry {
            id: id.clone(),
            dev: item.dev,
            version: item.found.version.clone(),
            deps: item
                .package
                .manifest()
                .dependencies
                .iter()
                .map(|d| Dep {
                    id: d.id.clone(),
                    requirement: d.requirement.clone(),
                    optional: d.optional,
                })
                .collect(),
        });
        entry_kept.push(k);
    }
    out.disabled
        .sort_by(|a, b| a.dir.file_name().cmp(&b.dir.file_name()));
    out.skipped
        .sort_by(|a, b| a.dir.file_name().cmp(&b.dir.file_name()));

    let plan = load_order::plan(&entries, &presence);
    let mut slots: Vec<Option<Kept>> = kept.into_iter().map(Some).collect();
    for (i, fault) in plan.refused {
        if let Some(item) = slots[entry_kept[i]].take() {
            out.requested.insert(
                item.found.id.clone(),
                item.package.manifest().powers.clone(),
            );
            out.refused.push(Refused {
                path: item.found.dir,
                fault,
            });
        }
    }
    for (index, i) in plan.order.into_iter().enumerate() {
        if let Some(item) = slots[entry_kept[i]].take() {
            let requested = item.package.manifest().powers.clone();
            let granted = states.granted(&item.found.id);
            let powers = powers::effective(&requested, &granted);
            let limits = states.limits;
            let command_allow = states.command_allow(&item.found.id);
            out.requested
                .insert(item.found.id.clone(), requested.clone());
            out.loaded.push(ScannedMod {
                found: item.found,
                dev: item.dev,
                package: item.package,
                index,
                requested,
                granted,
                powers,
                limits,
                command_allow,
            });
        }
    }
    out
}

/// The entries of `<game_dir>/lua-mods/` that may be packages, in file name order. A symlink that
/// names a package, or points at a folder or a package, is refused; any other entry is ignored.
fn installed(game_dir: &Path, refused: &mut Vec<Refused>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(game_dir.join(DIR)) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    let mut out = Vec::new();
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::canonicalize(&path).ok();
            let names_package = is_archive(&path);
            let points_at_package = target
                .as_deref()
                .is_some_and(|t| t.is_dir() || is_archive(t));
            if names_package || points_at_package {
                refused.push(Refused {
                    path,
                    fault: Fault::Package(Refusal::Symlink),
                });
            }
        } else if meta.is_dir() || (meta.is_file() && is_archive(&path)) {
            out.push(path);
        }
    }
    out
}

/// Whether a path ends in `.hlua` (its extension, case-sensitive).
fn is_archive(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == ARCHIVE_EXT)
}

/// Opens a folder or a `.hlua` archive as a package. Anything else is refused.
fn open(path: &Path) -> Result<Package, Fault> {
    let meta = std::fs::metadata(path).map_err(|_| Fault::Package(Refusal::Unreadable))?;
    if meta.is_dir() {
        Package::open_dir(path).map_err(Fault::Package)
    } else if is_archive(path) {
        Package::open_archive(path).map_err(Fault::Package)
    } else {
        Err(Fault::DevPathNotHlua)
    }
}

/// The last component of a path as text, for the duplicate's "first" argument.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
