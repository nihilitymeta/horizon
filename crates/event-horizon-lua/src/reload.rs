//! Hot reload of a dev-path Lua mod's scripts (5f-c2): `reload(id)` re-reads the mod's folder through
//! the same scan the load made, declares the scripts again in a fresh state, and swaps that state in
//! when the fresh declaration equals the one the mod had. The mod's handlers then run the new code.
//!
//! What a reload does NOT do, by design:
//! - It re-runs the scripts' top-level declarations and replaces the handlers. Hooks, blocks, items,
//!   parts, commands, client commands, goals, behaviors, configs, data map types, data pack files and
//!   the settings schema are fixed until the game restarts: a change to any of them is refused with
//!   the names of what changed (`ReloadError::Declaration`), and the old state keeps running.
//! - Timers the old scripts set with `horizon.after` do not survive: their state is dropped with it.
//! - Lifecycle hooks the mod already received (a world started, say) are not fired again.
//! - Installed `.hlua` archives are not reloaded: only mods started from a dev path (`EH_LUA_MODS`).
//! - The content packs (assets, data, lang) the mod published at its load are not republished.
//!
//! Only the mod's scripts are read again. A reload that fails at any step (a read error, a script
//! error, a refused declaration) leaves the old state in place and reports why.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::discover;
use crate::host::{LOG_TARGET, LuaMod};

/// How long a reload waits for the mod's turn (a handler running on another thread) before it gives
/// up with `ReloadError::Busy`. The mod's handlers are short; a longer wait would stall the caller.
const TURN_WAIT: Duration = Duration::from_millis(250);

/// What a reload reads the mods through: the loaded mods, and the arguments the load's scan was given.
pub(crate) struct Context {
    mods: Arc<[LuaMod]>,
    game_dir: PathBuf,
    dev_paths: Vec<PathBuf>,
    dedicated: bool,
}

impl Context {
    pub(crate) fn new(
        mods: Arc<[LuaMod]>,
        game_dir: &Path,
        dev_paths: &[PathBuf],
        dedicated: bool,
    ) -> Context {
        Context {
            mods,
            game_dir: game_dir.to_path_buf(),
            dev_paths: dev_paths.to_vec(),
            dedicated,
        }
    }
}

/// The context of the game's Lua mods, set once by `builtin` (the first call wins).
static CONTEXT: OnceLock<Context> = OnceLock::new();

/// Publishes the loaded mods and the scan's arguments for `reload`. A second call changes nothing.
pub(crate) fn publish(
    mods: Arc<[LuaMod]>,
    game_dir: &Path,
    dev_paths: &[PathBuf],
    dedicated: bool,
) {
    let _ = CONTEXT.set(Context::new(mods, game_dir, dev_paths, dedicated));
}

/// A reload that took effect: the mod's id and how many scripts the new state was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reloaded {
    pub id: String,
    pub scripts: usize,
}

/// Why a reload did not take effect. The old state keeps running in every case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadError {
    /// No loaded Lua mod has that id.
    NotLoaded,
    /// The mod was not started from a dev path (`EH_LUA_MODS`); only those reload.
    NotADevMod,
    /// Another thread ran the mod's handlers for longer than the wait; try again.
    Busy,
    /// The mod's folder no longer reads as a mod, or its scripts could not be read; the text says why.
    Unreadable(String),
    /// The fresh declaration differs from the one the mod loaded with: the names of what changed.
    Declaration(Vec<String>),
    /// The new scripts failed to run or to declare (a syntax error, say); the text is the error.
    Failed(String),
}

impl fmt::Display for ReloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReloadError::NotLoaded => f.write_str("no Lua mod with that id is loaded"),
            ReloadError::NotADevMod => f.write_str(
                "the mod was not started from a dev path (EH_LUA_MODS), so only the mods started that way reload",
            ),
            ReloadError::Busy => f.write_str("the mod is busy running a handler"),
            ReloadError::Unreadable(why) => write!(f, "its scripts could not be read: {why}"),
            ReloadError::Declaration(what) => write!(
                f,
                "its declarations changed, restart the game to change them: {}",
                what.join("; ")
            ),
            ReloadError::Failed(why) => write!(f, "its new scripts failed: {why}"),
        }
    }
}

/// Reloads the scripts of the dev-path Lua mod `id`, see the module doc. Returns what was reloaded,
/// or why it was not.
pub fn reload(id: &str) -> Result<Reloaded, ReloadError> {
    let context = CONTEXT.get().ok_or(ReloadError::NotLoaded)?;
    reload_in(context, id)
}

/// The ids of the loaded mods a `reload` may reload: the ones started from a dev path, in load order.
pub fn reloadable() -> Vec<String> {
    CONTEXT.get().map(reloadable_in).unwrap_or_default()
}

pub(crate) fn reloadable_in(context: &Context) -> Vec<String> {
    context
        .mods
        .iter()
        .filter(|m| m.is_dev())
        .map(|m| m.id().to_owned())
        .collect()
}

/// `reload` against `context`: the steps of the module doc, in order.
pub(crate) fn reload_in(context: &Context, id: &str) -> Result<Reloaded, ReloadError> {
    let m = context
        .mods
        .iter()
        .find(|m| m.id() == id)
        .ok_or(ReloadError::NotLoaded)?;
    if !m.is_dev() {
        return Err(ReloadError::NotADevMod);
    }
    let _turn = m.turn().enter_timeout(TURN_WAIT).ok_or(ReloadError::Busy)?;
    // The same scan the load made: the dev paths and the game's folder, through the same reader.
    let scan = discover::scan_on(&context.game_dir, &context.dev_paths, context.dedicated);
    let scanned = scan
        .loaded
        .iter()
        .find(|s| s.dev && s.found.id == id)
        .ok_or_else(|| ReloadError::Unreadable("its folder no longer loads as a mod".to_owned()))?;
    let scripts = scanned
        .package
        .entry_sources()
        .map_err(|reason| ReloadError::Unreadable(reason.reason()))?;
    let (fresh, fingerprint) = m
        .rebuild_state(&scanned.found, &scripts)
        .map_err(ReloadError::Failed)?;
    m.swap_state(fresh, fingerprint)
        .map_err(|refused| ReloadError::Declaration(refused.what))?;
    log::info!(
        target: LOG_TARGET,
        "{id}: reloaded {} scripts; declarations are unchanged",
        scripts.len()
    );
    Ok(Reloaded {
        id: id.to_owned(),
        scripts: scripts.len(),
    })
}
