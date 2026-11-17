//! Event Horizon's game executable: the platform's game, started with Event Horizon's built-in
//! mods (none yet; a later chunk passes them to `horizon_mod_host::main_with`).
#![forbid(unsafe_code)]

/// Identifies the executable as the game for launcher imports, as `horizon-official` does.
static GAME_IDENTITY: [u8; horizon_game::GAME_IDENTITY.len()] = horizon_game::GAME_IDENTITY;

fn main() {
    horizon_guest_stack::prepare_main();
    std::hint::black_box(&GAME_IDENTITY);
    horizon_mod_host::exit_if_compile_child();
    let startup = horizon_client::entry::prepare();
    std::hint::black_box(&horizon_mod_package::holder::CAPABILITY);
    // Built-in mods are added here in a later chunk; until then the lists are empty.
    horizon_mod_host::main_with(Vec::new(), &[], &[], startup);
}
