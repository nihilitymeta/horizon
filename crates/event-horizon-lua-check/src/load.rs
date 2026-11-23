//! The headless load: a mod goes through the Lua host's own entry (`builtin`), so the check sees
//! what the game would refuse. A process runs one load: `builtin` registers process-wide state.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use event_horizon_lua::builtin::builtin;

/// An empty scratch game folder, removed when dropped.
pub struct ScratchGame {
    path: PathBuf,
}

impl ScratchGame {
    pub fn new() -> std::io::Result<ScratchGame> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path =
            std::env::temp_dir().join(format!("eh-lua-check-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(ScratchGame { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchGame {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// What the host made of a game folder and its dev paths: how many Lua mods loaded, and one line
/// per folder or mod that did not, with its translation key after the reason.
pub struct Outcome {
    pub loaded: usize,
    pub refused: Vec<String>,
}

/// Loads the mods under `game_dir` and the dev paths through the host.
pub fn load(game_dir: &Path, dev_paths: &[PathBuf]) -> Outcome {
    let built = builtin(game_dir, dev_paths, false);
    Outcome {
        loaded: built.loaded,
        refused: built.refused,
    }
}
