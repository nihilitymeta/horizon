//! The mark a game program carries when it has a feature registry (the client's `mod_registry`)
//! and answers `--list-mods`, for the launcher to read from the program's bytes before running
//! it: a program from before the registry would start the game on that flag instead. The literal
//! `--list-mods` itself cannot be searched for, the compiler folds short string comparisons into
//! instructions, so the program keeps this array alive (`std::hint::black_box` in its `main`).

/// `horizon-feature-registry:1;`: the registry's format version after the colon.
pub const MARK: [u8; 27] = *b"horizon-feature-registry:1;";
