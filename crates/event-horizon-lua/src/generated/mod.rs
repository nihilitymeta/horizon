//! The Lua host's generated bindings: `hooks.rs` (the hook table), `hooks_impl.rs` (the
//! `Hooks` methods), `handles.rs` (the handle families' methods, with `handles_coverage.txt`) and
//! `parts.rs` (the BlockPart methods and their handler table) and `item_parts.rs` (the ItemPart
//! methods and their handler table).
//! The generator (`event-horizon-lua-gen`) writes them; do not edit them. They are not run through
//! rustfmt: the generator writes them in its own layout, and the drift check
//! (`event-horizon-lua-gen --check`) compares them byte for byte.

#[rustfmt::skip]
pub mod hooks;
#[rustfmt::skip]
mod hooks_impl;
#[rustfmt::skip]
pub mod handles;
#[rustfmt::skip]
pub mod parts;
#[rustfmt::skip]
pub mod item_parts;
