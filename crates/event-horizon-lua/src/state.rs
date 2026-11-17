//! The Luau state a mod runs in: sandboxed, with no files, no processes and no loader.
//!
//! Each mod gets its own state. Nothing a mod's chunk writes reaches another state, and the
//! libraries here cannot read or write files, start processes or read the environment.

use mlua::{Lua, LuaOptions, StdLib};

/// The libraries a mod's state gets. No PACKAGE (its `require` reads files), no IO, no DEBUG,
/// no JIT and no FFI. A function, not a constant: mlua's `StdLib` union is not `const`.
pub fn libraries() -> StdLib {
    StdLib::COROUTINE
        | StdLib::TABLE
        | StdLib::OS
        | StdLib::STRING
        | StdLib::UTF8
        | StdLib::BIT
        | StdLib::MATH
        | StdLib::BUFFER
        | StdLib::VECTOR
}

/// A fresh sandboxed state for one mod: the libraries above, then Luau's sandbox mode, which makes
/// the libraries and the globals read-only and gives the chunk a local environment for its writes.
///
/// mlua installs a file-reading `require` in every Luau state, whatever the libraries are, so it
/// is removed here, before the globals become read-only.
pub fn new_state() -> mlua::Result<Lua> {
    let lua = Lua::new_with(libraries(), LuaOptions::default())?;
    lua.globals().raw_set("require", mlua::Value::Nil)?;
    lua.sandbox(true)?;
    Ok(lua)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_has_no_files_processes_or_loaders() {
        let lua = new_state().unwrap();
        let absent: bool = lua
            .load(
                "return io == nil and package == nil and debug == nil and require == nil \
                 and dofile == nil and loadfile == nil",
            )
            .eval()
            .unwrap();
        assert!(
            absent,
            "io, package, debug, require, dofile and loadfile must be nil"
        );
        let os_reduced: bool = lua
            .load(
                "return os.execute == nil and os.exit == nil and os.remove == nil \
                 and os.rename == nil and os.getenv == nil and os.tmpname == nil \
                 and os.setlocale == nil",
            )
            .eval()
            .unwrap();
        assert!(
            os_reduced,
            "the os fields that touch files, processes or the environment must be nil"
        );
    }

    #[test]
    fn the_libraries_are_read_only() {
        let lua = new_state().unwrap();
        let writes_refused: bool = lua
            .load(
                "return not pcall(function() string.format = nil end) \
                 and not pcall(function() math.floor = nil end) \
                 and not pcall(function() getmetatable(\"\").__index = {} end)",
            )
            .eval()
            .unwrap();
        assert!(
            writes_refused,
            "a library field and a builtin metatable must be read-only"
        );
    }

    #[test]
    fn a_global_written_by_a_chunk_stays_in_its_state() {
        let a = new_state().unwrap();
        let b = new_state().unwrap();
        a.load("leaked = 1").exec().unwrap();
        let in_b: Option<i64> = b.load("return leaked").eval().unwrap();
        assert_eq!(in_b, None);
        let in_a: Option<i64> = a.load("return leaked").eval().unwrap();
        assert_eq!(in_a, Some(1));
    }

    #[test]
    fn the_harmless_standard_functions_work() {
        let lua = new_state().unwrap();
        let ok: bool = lua
            .load(
                "return os.clock() >= 0 and math.floor(2.5) == 2 and string.upper(\"a\") == \"A\" \
                 and #table.pack(1, 2) == 2",
            )
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn the_garbage_collector_can_only_be_read() {
        let lua = new_state().unwrap();
        let count: f64 = lua.load("return collectgarbage(\"count\")").eval().unwrap();
        assert!(count >= 0.0);
        let collect_refused: bool = lua
            .load("return not pcall(collectgarbage, \"collect\")")
            .eval()
            .unwrap();
        assert!(
            collect_refused,
            "only the count mode of collectgarbage may be used"
        );
    }

    #[test]
    fn require_cannot_read_a_file_even_when_the_chunk_is_named_like_one() {
        // mlua's file requirer resolves a path from the calling chunk's name, and only when the
        // chunk itself calls `require`, not as a tail call and not through `pcall`. With this name an unsandboxed state
        // loads mod.lua from the folder and returns 42.
        // No leading dot in the folder name: mlua's requirer cannot resolve a chunk under a dot folder.
        let dir = tempfile::Builder::new()
            .prefix("eh-lua-")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("mod.lua"), "return 42").unwrap();
        // The requirer resets to the requiring chunk's own file, so that file must exist too.
        std::fs::write(dir.path().join("main.lua"), "").unwrap();
        let lua = new_state().unwrap();
        let loaded = lua
            .load("local module = require(\"./mod\")\nreturn module")
            .set_name(format!("@{}/main.lua", dir.path().display()))
            .eval::<i64>();
        assert!(
            loaded.is_err(),
            "require must not read a file from the mod's folder, got {loaded:?}"
        );
    }
}
