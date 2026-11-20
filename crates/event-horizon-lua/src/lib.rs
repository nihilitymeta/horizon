//! Event Horizon's Lua runtime: one built-in host mod that runs .hlua mods (sandboxed Luau) on the mod API
#![forbid(unsafe_code)]
mod attachments;
#[cfg(test)]
mod attachments_tests;
#[cfg(test)]
mod block_state_tests;
pub mod budget;
pub mod builtin;
pub mod commands;
mod data_maps;
#[cfg(test)]
mod data_maps_tests;
pub mod declare;
pub mod discover;
pub mod dispatch;
#[cfg(test)]
mod entity_tests;
#[cfg(test)]
mod fluid_tests;
pub mod generated;
mod goal;
mod goal_actions;
mod goal_budget;
#[cfg(test)]
mod goal_tests;
pub mod handles;
pub mod host;
pub mod hostcall;
#[cfg(test)]
mod menu_tests;
pub mod package;
pub mod quarantine;
pub mod ratelimit;
pub mod stacks;
pub mod state;
