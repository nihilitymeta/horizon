//! The Lua host's generated bindings: `hooks.rs` (the hook table) and `hooks_impl.rs` (the
//! `Hooks` methods). Both files are written by `event-horizon-lua-gen`; do not edit them.
//! They are not run through rustfmt: the generator writes them in its own layout, and the
//! drift check (`event-horizon-lua-gen --check`) compares them byte for byte.

#[rustfmt::skip]
pub mod hooks;
#[rustfmt::skip]
mod hooks_impl;
