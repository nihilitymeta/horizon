//! Event Horizon's game executable: the platform's game, started with Event Horizon's built-in
//! mods (the Lua host, which runs the mods in the game folder's `lua-mods/`).
#![forbid(unsafe_code)]

/// Identifies the executable as the game for launcher imports, as `horizon-official` does.
static GAME_IDENTITY: [u8; horizon_game::GAME_IDENTITY.len()] = horizon_game::GAME_IDENTITY;

/// The built-in mods with the developer tools on: the Lua host, then the tools.
const BUILT_WITH_DEV_TOOLS: &[mod_api::__private::native::BuiltMod] = &[
    event_horizon_lua::builtin::BUILT[0],
    event_horizon_dev_tools::BUILT[0],
];

fn main() {
    horizon_guest_stack::prepare_main();
    std::hint::black_box(&GAME_IDENTITY);
    horizon_mod_host::exit_if_compile_child();
    let startup = horizon_client::entry::prepare();
    std::hint::black_box(&horizon_mod_package::holder::CAPABILITY);
    // Dev mods: `--lua-mod PATH` (repeatable) on the command line, then `EH_LUA_MODS`.
    let dev_paths = event_horizon_lua::builtin::dev_paths(
        event_horizon_lua::builtin::flag_dev_paths(std::env::args_os().skip(1)),
        std::env::var_os("EH_LUA_MODS").as_deref(),
    );
    // A dedicated server is the server installation: a client-only Lua mod is not loaded there.
    let dedicated =
        startup.launch().installation_side() == horizon_mod_package::manifest::Side::Server;
    let lua = event_horizon_lua::builtin::builtin(
        &startup.launch().context.game_dir,
        &dev_paths,
        dedicated,
    );
    // The developer tools join the game only when asked for (`HORIZON_DEV_TOOLS=1` or
    // `--dev-tools`): then their mod and its package are listed, and otherwise neither is.
    let mut mods = vec![lua.entry];
    let built = if event_horizon_dev_tools::requested(
        std::env::var_os(event_horizon_dev_tools::ENV).as_deref(),
        std::env::args_os(),
    ) {
        mods.extend(event_horizon_dev_tools::mods());
        BUILT_WITH_DEV_TOOLS
    } else {
        event_horizon_lua::builtin::BUILT
    };
    horizon_mod_host::main_with(mods, &[], built, startup);
}
