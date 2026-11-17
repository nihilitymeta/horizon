//! Event Horizon's game executable: the platform's game, started with Event Horizon's built-in
//! mods (the Lua host, which runs the mods in the game folder's `lua-mods/`).
#![forbid(unsafe_code)]

/// Identifies the executable as the game for launcher imports, as `horizon-official` does.
static GAME_IDENTITY: [u8; horizon_game::GAME_IDENTITY.len()] = horizon_game::GAME_IDENTITY;

fn main() {
    horizon_guest_stack::prepare_main();
    std::hint::black_box(&GAME_IDENTITY);
    horizon_mod_host::exit_if_compile_child();
    let startup = horizon_client::entry::prepare();
    std::hint::black_box(&horizon_mod_package::holder::CAPABILITY);
    let lua = event_horizon_lua::builtin::builtin(&startup.launch().context.game_dir);
    horizon_mod_host::main_with(
        vec![lua.entry],
        &[],
        event_horizon_lua::builtin::BUILT,
        startup,
    );
}
