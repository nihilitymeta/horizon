//! `emit-types <dir>`: the Luau type definitions of the Lua mod API (the committed
//! `horizon.d.luau` of the Lua crate, not regenerated here) and a `.luaurc` for `luau-lsp`.
//! A file that already exists with other content is refused unless `--force` is given; nothing is
//! written when either file is refused.

use std::path::Path;
use std::process::ExitCode;

/// The committed type definitions (`crates/event-horizon-lua/lua-types/horizon.d.luau`).
pub const TYPES: &str = include_str!("../../event-horizon-lua/lua-types/horizon.d.luau");

/// The `.luaurc` `luau-lsp` reads: strict mode, Luau's own lints on, and the `horizon` global the
/// mod API declares. The keys are the ones Luau documents for `.luaurc` (`languageMode`, `lint`,
/// `globals`); no `luau-lsp` is bundled to test them against.
pub const LUAURC: &str = "{\n  \"languageMode\": \"strict\",\n  \"lint\": {\n    \"*\": true\n  },\n  \"globals\": [\"horizon\"]\n}\n";

pub const TYPES_FILE: &str = "horizon.d.luau";
pub const LUAURC_FILE: &str = ".luaurc";

pub fn run(dir: &Path, force: bool) -> ExitCode {
    match emit(dir, force) {
        Ok(written) => {
            for name in written {
                println!("wrote {}", dir.join(name).display());
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("event-horizon-lua-check: {message}");
            ExitCode::from(1)
        }
    }
}

/// Writes both files into `dir`, returning the names written. A file that differs and is not
/// `force`d is an error, and then neither file is written.
pub fn emit(dir: &Path, force: bool) -> Result<Vec<&'static str>, String> {
    let files = [(TYPES_FILE, TYPES), (LUAURC_FILE, LUAURC)];
    let mut to_write = Vec::new();
    for (name, content) in files {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(existing) if existing == content => {}
            Ok(_) if !force => {
                return Err(format!(
                    "{} exists with other content; pass --force to overwrite it",
                    path.display()
                ));
            }
            Ok(_) => to_write.push((name, content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                to_write.push((name, content));
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut written = Vec::new();
    for (name, content) in to_write {
        let path = dir.join(name);
        std::fs::write(&path, content).map_err(|error| format!("{}: {error}", path.display()))?;
        written.push(name);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_luaurc_is_json_with_the_horizon_global() {
        let value: serde_json::Value = serde_json::from_str(LUAURC).expect("valid JSON");
        assert_eq!(value["languageMode"], "strict");
        assert_eq!(value["globals"][0], "horizon");
    }

    #[test]
    fn a_second_emit_with_other_content_is_refused_until_forced() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(emit(dir.path(), false).unwrap().len(), 2);
        // Identical content is fine again.
        assert_eq!(emit(dir.path(), false).unwrap().len(), 0);
        std::fs::write(dir.path().join(LUAURC_FILE), "{}\n").unwrap();
        assert!(emit(dir.path(), false).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(LUAURC_FILE)).unwrap(),
            "{}\n"
        );
        assert_eq!(emit(dir.path(), true).unwrap(), vec![LUAURC_FILE]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(LUAURC_FILE)).unwrap(),
            LUAURC
        );
    }
}
