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
    // Dev mods come from EH_LUA_MODS only: the game's own argument parser reads the command line
    // itself, so a `--lua-mod` flag cannot be taken out here (see the report of chunk 3b).
    let dev_paths =
        event_horizon_lua::builtin::env_dev_paths(std::env::var_os("EH_LUA_MODS").as_deref());
    // A dedicated server is the server installation: a client-only Lua mod is not loaded there.
    let dedicated =
        startup.launch().installation_side() == horizon_mod_package::manifest::Side::Server;
    let lua = event_horizon_lua::builtin::builtin(
        &startup.launch().context.game_dir,
        &dev_paths,
        dedicated,
    );
    horizon_mod_host::main_with(
        vec![lua.entry],
        &[],
        event_horizon_lua::builtin::BUILT,
        startup,
    );
}
