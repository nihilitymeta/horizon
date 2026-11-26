//! Conversions between Lua values and the mod API's value types (the conversion layer of the Lua host)
#![forbid(unsafe_code)]

pub mod ids;
pub mod item;
pub mod json;
pub mod loot;
pub mod nbt;
#[rustfmt::skip]
pub mod generated_types;
pub mod scalar;
pub mod text;
pub mod vector;

#[cfg(test)]
mod fuzz_tests;
