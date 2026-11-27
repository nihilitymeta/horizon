//! Host method declarations for the developer tools (fork-owned); access defaults and game forwarders are generated.
#![allow(unused_variables)]
use crate::*;

/// The server host's read-only methods of the `dev_inspect` area, implemented by the game and reached through [`ServerHost`].
/// They exist for the developer tools' commands: they read the single-player world and its
/// command selectors, and change nothing.
#[horizon_mod_api_macros::host_area]
pub trait DevInspectServerHost {
    /// Side: server.
    /// The single-player world's owner, strictly: the profile the window's own world was opened
    /// for, not any operator or LAN guest. None on a dedicated server, on a client, and in a world
    /// no single player owns. Read-only; `player` resolves the uuid to a player.
    #[access(WorldOnly)]
    fn dev_server_owner(&self) -> Option<u128>;
    /// Side: server.
    /// The network entity ids of the entities a command selector such as `@e[type=zombie]` names,
    /// found as the server's own command source finds them. A selector that does not parse, or
    /// names nothing, gives an empty list. Read-only: it selects and changes nothing.
    #[access(WorldOnly)]
    fn dev_select_ids(&mut self, selector: &str) -> Vec<u32>;
    /// Side: server.
    /// The dimension id (`minecraft:overworld`, `minecraft:the_nether`, ...) of a level handle.
    /// None for a stale or foreign handle, or where no server level is open. Read-only.
    #[access(WorldOnly)]
    fn dev_level_dimension(&self, level: LevelRef) -> Option<String>;
    /// Side: server.
    /// The entity's saved data as `/data get entity` shows it (the engine's `entity_nbt`), without
    /// `id`, and with the mods' attachments under `horizon:attachments` when it has any. None for a
    /// stale or foreign handle, and for an entity that is not saved. Read-only.
    #[access(WorldOnly)]
    fn dev_entity_nbt(&mut self, entity: EntityRef) -> Option<Compound>;
}
