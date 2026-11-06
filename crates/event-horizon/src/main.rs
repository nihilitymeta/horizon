//! Event Horizon: native Rust client for Minecraft Java Edition 26.3 gameplay.
#![forbid(unsafe_code)]

#[cfg(not(any(feature = "system-allocator", feature = "alloc-census")))]
#[global_allocator]
static ALLOCATOR: horizon_threads::Allocator = horizon_threads::ALLOCATOR;
#[cfg(all(feature = "alloc-census", not(feature = "system-allocator")))]
#[global_allocator]
static ALLOCATOR: horizon_threads::CensusAllocator = horizon_threads::CensusAllocator;

/// The mod API level this game serves, kept in the program for the launcher
/// to read without running it (`horizon_mods::api_level::MARK`). `main`
/// reads it, so the linker keeps it.
static MOD_API_LEVEL: [u8; 27] = horizon_mods::api_level::MARK;
/// The feature registry this game answers `--list-mods` with, kept the same way
/// (`horizon_mods::feature_registry::MARK`).
static FEATURE_REGISTRY: [u8; 27] = horizon_mods::feature_registry::MARK;

fn main() {
    std::hint::black_box(&MOD_API_LEVEL);
    std::hint::black_box(&FEATURE_REGISTRY);

    // `--check-mod <path> [--dev-mod <library>]... [--resources <folder>]`:
    // load a scripted mod headlessly (and with `--resources`, layer its
    // content as the game would), report, and leave; no window, no world.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--check-mod") {
        let Some(path) = args.get(i + 1) else {
            eprintln!("--check-mod <path to a mod folder or .hmod>");
            std::process::exit(2);
        };
        let dev: Vec<std::path::PathBuf> = args
            .windows(2)
            .filter(|w| w[0] == "--dev-mod")
            .map(|w| std::path::PathBuf::from(&w[1]))
            .collect();
        let resources = args
            .windows(2)
            .find(|w| w[0] == "--resources")
            .map(|w| std::path::PathBuf::from(&w[1]));
        match horizon_client::lua_mods::check_mod(
            std::path::Path::new(path),
            &dev,
            resources.as_deref(),
        ) {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    // `--list-mods` asks for the build's feature registry and nothing else
    // (`main_with` prints it and returns): the launcher runs it at every
    // version selection, so no mod is discovered or loaded for it, no
    // `mods` folder scanned, and the developer tools stay off.
    if list_mods_only(&args) {
        horizon_client::entry::main_with(Default::default());
        return;
    }
    horizon_client::entry::main_with(event_horizon::builtin_mods(args.into_iter()).into());
}

/// Whether the command line is a `--list-mods` query, as `main_with` reads
/// it: among the game's arguments, before any `--dedicated`.
fn list_mods_only(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| *a != "--dedicated")
        .any(|a| a == "--list-mods")
}
