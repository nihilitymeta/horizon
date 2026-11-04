//! The mark a game program carries when it has a feature registry (the client's `mod_registry`)
//! and answers `--list-mods`, for the launcher to read from the program's bytes before running
//! it: a program from before the registry would start the game on that flag instead. The literal
//! `--list-mods` itself cannot be searched for, the compiler folds short string comparisons into
//! instructions, so the program keeps this array alive (`std::hint::black_box` in its `main`).

/// `horizon-feature-registry:1;`: the registry's format version after the colon.
pub const MARK: [u8; 27] = *b"horizon-feature-registry:1;";

/// Whether `program`'s bytes carry the [`MARK`] (any format version).
pub fn has_mark(program: &[u8]) -> bool {
    let prefix = &MARK[..25];
    program
        .windows(MARK.len())
        .any(|w| w.starts_with(prefix) && w[26] == b';')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_is_found_amid_other_bytes_and_not_by_its_prefix_alone() {
        let mut program = b"...horizon-mod-api-level:0001;...".to_vec();
        assert!(!has_mark(&program));
        program.extend_from_slice(&MARK);
        program.extend_from_slice(b"...");
        assert!(has_mark(&program));
        assert!(has_mark(b"horizon-feature-registry:2;"));
        assert!(!has_mark(b"horizon-feature-registry:1"));
    }
}
