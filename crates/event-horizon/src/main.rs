//! Event Horizon: native Rust client for Minecraft Java Edition 26.3 gameplay.
#![forbid(unsafe_code)]

#[cfg(not(feature = "system-allocator"))]
#[global_allocator]
static ALLOCATOR: horizon_threads::GameAllocator = horizon_threads::GAME_ALLOCATOR;

fn main() {
    // The marks, `--check-mod`, `--list-mods` and the built-in mods all live in the
    // library's `run`, which the launcher's generated executable shares.
    event_horizon::run(Vec::new());
}
