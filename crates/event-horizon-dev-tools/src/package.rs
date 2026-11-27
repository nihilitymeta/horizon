//! The package the game needs beside its executable for the developer tools, a built-in mod.
//!
//! The tools are compiled into the game, so this package only declares them: nothing is built
//! from it. The game refuses to start a built-in mod unless its package is there, with the same
//! id, version and side. `build.rs` of the game writes it.

use std::path::Path;

/// The mod id: the game accepts only lowercase letters, digits and underscores (`valid_mod_id`).
pub const ID: &str = "event_horizon_dev_tools";
pub const VERSION: &str = "0.1.0";
/// Server-only: the tools' hooks all run on the server, so a guest need not have them.
pub const SIDE: &str = "server";

/// The text of `horizon-mod.toml`.
pub fn manifest_text() -> String {
    format!(
        "id = '{ID}'\n\
         name = 'Event Horizon Developer Tools'\n\
         version = '{VERSION}'\n\
         api = '0.2'\n\
         side = '{SIDE}'\n\
         capabilities = ['registries']\n\
         [native]\n\
         game = '26.3'\n\
         horizon = '{PIN}'\n\
         mixins = '{PIN}'\n",
        PIN = "0".repeat(40),
    )
}

/// Writes `horizon-mod.toml`, `Cargo.toml` and `src/lib.rs` into `dir` (created if missing).
pub fn write_to(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::write(dir.join("horizon-mod.toml"), manifest_text())?;
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = 'event_horizon_dev_tools_package'\nversion = '0.1.0'\n",
    )?;
    std::fs::write(
        dir.join("src/lib.rs"),
        "// The developer tools are compiled into the game; this package only declares them.\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_written_package_is_accepted_as_a_native_package() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("horizon-native-mods").join(ID);
        write_to(&dir).unwrap();
        let read = horizon_mod_package::packages::read_native(&dir).unwrap();
        assert_eq!(read.manifest.id, ID);
        assert_eq!(read.manifest.version, VERSION);
        assert_eq!(read.manifest.name, "Event Horizon Developer Tools");
    }

    #[test]
    fn the_version_is_the_crates_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }
}
