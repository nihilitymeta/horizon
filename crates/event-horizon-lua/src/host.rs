//! The Lua mods as one built-in game mod.
//!
//! The game reads `Hooks::subscribed` once, at install time, and then calls only the hook methods
//! that a mod subscribed to. `LuaHost` is that one mod: it holds every loaded `LuaMod` and, when a
//! hook fires, runs each handler the mods registered for it. A mod's failure stays in that mod:
//! its error is logged, its handler is stopped by a time budget, and the others still run.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use horizon_client::save::level_mods::{self, ModRecord};
use mlua::{Error, Function, Lua, Table, Value};

use crate::budget;
use crate::declare::{
    self, BlockEntityTypeRecord, BlockRecord, ClientCommandRecord, CommandRecord, FluidRecord,
    ItemRecord, ListenerOptions, PartKind, PartRecord,
};
use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::{self, new_event};
use crate::generated::hooks::{hook_id, is_emitted};
use crate::ratelimit::{RateLimit, Verdict};
use crate::state;

/// The largest `main.luau` that is read.
pub const MAX_SOURCE: u64 = 1024 * 1024;
/// The lines kept per mod.
pub const MAX_LOG_LINES: usize = 100;
/// The characters kept per log line.
pub const MAX_LOG_LINE: usize = 1000;
/// The time one handler may run for one hook call. A handler that runs longer is stopped, so one
/// mod cannot hold up the server's tick or the handlers of the other mods.
pub const HANDLER_BUDGET: Duration = Duration::from_millis(50);
/// The log lines one mod may write at once.
pub const LOG_BURST: u32 = 20;
/// The log lines one mod may add each second, after its burst.
pub const LOG_PER_SECOND: u32 = 20;

const LATE_ON: &str = "horizon.on can only be used while the mod loads";
pub(crate) const LOG_TARGET: &str = "event_horizon_lua";

/// One loaded Lua mod: its own sandboxed state, the hooks it declared, its log lines, and the turn
/// that serialises the threads calling into its state.
pub struct LuaMod {
    id: String,
    version: String,
    side: Side,
    hooks: Vec<String>,
    options: Vec<(String, ListenerOptions)>,
    /// The blocks and items the mod declared with `horizon.block` and `horizon.item`.
    blocks: Vec<BlockRecord>,
    items: Vec<ItemRecord>,
    /// The block parts the mod declared with `horizon.block_part`, and the item parts with
    /// `horizon.item_part`.
    parts: Vec<PartRecord>,
    item_parts: Vec<PartRecord>,
    /// The block entity types the mod declared with `horizon.block_entity_type`, and the block entity
    /// parts with `horizon.block_entity_part`.
    block_entity_types: Vec<BlockEntityTypeRecord>,
    block_entity_parts: Vec<PartRecord>,
    /// The world fluids the mod declared with `horizon.world_fluid`, and the fluid parts with
    /// `horizon.fluid_part`.
    fluids: Vec<FluidRecord>,
    fluid_parts: Vec<PartRecord>,
    /// The entity parts the mod declared with `horizon.entity_part`, and the menu parts with
    /// `horizon.menu_part`.
    entity_parts: Vec<PartRecord>,
    menu_parts: Vec<PartRecord>,
    /// The client commands the mod declared with `horizon.client_command`.
    client_commands: Vec<ClientCommandRecord>,
    /// The server commands the mod declared with `horizon.command`.
    commands: Vec<CommandRecord>,
    /// The configs the mod declared with `horizon.config`, each owned by this mod's id.
    configs: Vec<mod_api::Config>,
    /// The data map types the mod declared with `horizon.data_map_type`, and the data pack files
    /// with `horizon.data_pack_file` (key and JSON text).
    data_map_types: Vec<mod_api::DataMapType>,
    data_pack_files: Vec<(String, String)>,
    /// The warnings the declaration gave (a refused data map type or file, say), as `(source, line)`
    /// for `warn_once`. They are logged by `LuaHost::report`, once the game's logger is up.
    notices: Vec<(String, String)>,
    lua: Lua,
    log: Arc<Mutex<Vec<String>>>,
    turn: Turn,
}

/// A per-mod turn: one thread at a time runs a mod's handlers. The same thread may enter again
/// (a call that re-enters the mod does not wait for itself), and another thread waits until the
/// turn is free. std has no such lock, so it is built from a mutex, its holder and a condvar.
#[derive(Default)]
pub(crate) struct Turn {
    holder: Mutex<Option<(ThreadId, usize)>>,
    free: Condvar,
}

/// Holds a mod's turn; the turn is given back when the guard drops, also when the thread panics.
pub(crate) struct TurnGuard<'a> {
    turn: &'a Turn,
}

impl Turn {
    pub(crate) fn enter(&self) -> TurnGuard<'_> {
        let me = thread::current().id();
        let mut holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match *holder {
                None => {
                    *holder = Some((me, 1));
                    break;
                }
                Some((who, depth)) if who == me => {
                    *holder = Some((me, depth + 1));
                    break;
                }
                Some(_) => {
                    holder = self
                        .free
                        .wait(holder)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        TurnGuard { turn: self }
    }
}

impl Turn {
    /// Whether the calling thread holds this turn now (it is open in an outer call of this thread).
    pub(crate) fn held_here(&self) -> bool {
        let me = thread::current().id();
        let holder = self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        matches!(*holder, Some((who, _)) if who == me)
    }
}

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        let mut holder = self
            .turn
            .holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((who, depth)) = *holder {
            *holder = (depth > 1).then_some((who, depth - 1));
        }
        self.turn.free.notify_all();
    }
}

impl LuaMod {
    /// Loads the mod in `found`: reads its `main.luau`, declares its hooks, and installs the
    /// runtime `horizon` table in place of the declaring one. Any failure is an error.
    pub fn load(found: &Discovered) -> Result<LuaMod, String> {
        let source = read_source(&found.dir)?;
        let lua = state::new_state().map_err(|e| e.to_string())?;
        let chunk_name = format!("@{}", found.dir.join(ENTRY).display());
        let (declaration, notices) = collect_load_notices(|| {
            declare::declare(&lua, &found.id, &source, &chunk_name, &|n| is_emitted(n))
        });
        let declaration = declaration?;

        let log = Arc::new(Mutex::new(Vec::new()));
        let runtime =
            runtime_table(&lua, &found.id, Arc::clone(&log)).map_err(|e| e.to_string())?;
        declare::environment(&lua)
            .and_then(|env| env.raw_set("horizon", runtime))
            .map_err(|e| e.to_string())?;

        Ok(LuaMod {
            id: found.id.clone(),
            version: found.version.clone(),
            side: found.side,
            hooks: declaration.hooks,
            options: declaration.options,
            blocks: declaration.blocks,
            items: declaration.items,
            parts: declaration.parts,
            item_parts: declaration.item_parts,
            block_entity_types: declaration.block_entity_types,
            block_entity_parts: declaration.block_entity_parts,
            fluids: declaration.fluids,
            fluid_parts: declaration.fluid_parts,
            entity_parts: declaration.entity_parts,
            menu_parts: declaration.menu_parts,
            client_commands: declaration.client_commands,
            commands: declaration.commands,
            configs: declaration.configs,
            data_map_types: declaration.data_map_types,
            data_pack_files: declaration.data_pack_files,
            notices,
            lua,
            log,
            turn: Turn::default(),
        })
    }

    /// The blocks this mod declared, in declaration order.
    pub fn blocks(&self) -> &[BlockRecord] {
        &self.blocks
    }

    /// The items this mod declared, in declaration order.
    pub fn items(&self) -> &[ItemRecord] {
        &self.items
    }

    /// The block parts this mod declared, in declaration order.
    pub fn parts(&self) -> &[PartRecord] {
        &self.parts
    }

    /// The records of the parts of `kind` this mod declared, in declaration order.
    fn part_records(&self, kind: PartKind) -> &[PartRecord] {
        match kind {
            PartKind::Block => &self.parts,
            PartKind::Item => &self.item_parts,
            PartKind::BlockEntity => &self.block_entity_parts,
            PartKind::Fluid => &self.fluid_parts,
            PartKind::Entity => &self.entity_parts,
            PartKind::Menu => &self.menu_parts,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The mod's sandboxed state. Callers hold `turn()` while they use it.
    pub(crate) fn lua(&self) -> &Lua {
        &self.lua
    }

    /// The turn that serialises the threads calling into this mod's state.
    pub(crate) fn turn(&self) -> &Turn {
        &self.turn
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// Whether the mod declared a server command named `name` with `horizon.command`: the mod's
    /// `run_mod_command` handler hears only the invocations of its own commands (`commands::delivers_to`).
    pub(crate) fn declares_command(&self, name: &str) -> bool {
        self.commands.iter().any(|command| command.name() == name)
    }

    /// Whether the mod declared any command with `horizon.command`.
    pub(crate) fn declares_any_command(&self) -> bool {
        !self.commands.is_empty()
    }

    /// Whether the mod runs on a client: a client or a both-sides mod.
    fn runs_on_client(&self) -> bool {
        self.side != Side::Server
    }

    /// Whether the mod runs on a server: a server or a both-sides mod.
    fn runs_on_server(&self) -> bool {
        self.side != Side::Client
    }

    /// Whether the mod's config is read where the mod runs. A `client` config is read by the window
    /// only, a `server` one by the server only, and a `common` one by both. The side is the manifest's.
    fn contributes_config(&self, config: &mod_api::Config) -> bool {
        match config.kind {
            mod_api::ConfigKind::Client => self.runs_on_client(),
            mod_api::ConfigKind::Server => self.runs_on_server(),
            mod_api::ConfigKind::Common => true,
            _ => false,
        }
    }

    pub fn hooks(&self) -> &[String] {
        &self.hooks
    }

    /// The options this mod declared for `hook`, or the defaults when it did not declare the hook.
    pub fn options(&self, hook: &str) -> ListenerOptions {
        self.options
            .iter()
            .find(|(name, _)| name == hook)
            .map(|(_, options)| *options)
            .unwrap_or_default()
    }

    /// The lines `horizon.log` wrote, oldest first.
    pub fn log_lines(&self) -> Vec<String> {
        lock(&self.log).clone()
    }
}

/// `existing` (the code mods already recorded, in their order), then one record for each Lua mod
/// that runs on the server (`Server` or `Both`), in load order. An id already in the list is not
/// added again, so merging the same mods twice changes nothing. A client-only mod does not
/// affect a world, so it is not recorded.
pub fn merged_records(existing: Option<Vec<ModRecord>>, mods: &[LuaMod]) -> Vec<ModRecord> {
    let mut records = existing.unwrap_or_default();
    for m in mods
        .iter()
        .filter(|m| matches!(m.side, Side::Server | Side::Both))
    {
        if !records.iter().any(|r| r.id == m.id) {
            records.push(ModRecord {
                id: m.id.clone(),
                version: m.version.clone(),
            });
        }
    }
    records
}

/// The one `Hooks` implementation the game installs: it fans each hook out to the Lua mods.
///
/// The game sees this host as ONE listener per hook, so `listeners` gives the game the highest
/// priority any Lua mod asked for and whether any of them receives cancelled events. The order
/// between the Lua mods is the host's own job: `order` holds, for each hook a mod declared, the
/// mods in the order they run.
pub struct LuaHost {
    /// The loaded mods, shared with the block parts that run their handlers.
    mods: Arc<[LuaMod]>,
    subscribed: &'static [Option<mod_api::HookId>],
    listeners: &'static [mod_api::Listener],
    order: Vec<(String, Vec<usize>)>,
    /// One line per folder or mod that did not load, from `builtin`.
    report: Vec<String>,
    /// Set once the report is written, so a second `register` writes nothing.
    reported: AtomicBool,
    /// The ids of the mods whose declared keys the game refused. Their blocks and items are not
    /// answered.
    refused_keys: Mutex<Vec<String>>,
}

impl LuaHost {
    pub fn new(mods: Vec<LuaMod>) -> LuaHost {
        // The hooks in the order the mods first declare them. Each one's mods run by priority,
        // highest first; ties keep load order.
        let mut names = Vec::<String>::new();
        for name in mods.iter().flat_map(|m| m.hooks.iter()) {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        let mut order = Vec::new();
        let mut subscribed = Vec::<Option<mod_api::HookId>>::new();
        let mut listeners = Vec::<mod_api::Listener>::new();
        for name in names {
            let declared: Vec<usize> = (0..mods.len())
                .filter(|&i| mods[i].hooks.contains(&name))
                .collect();
            let priority_of = |i: usize| mods[i].options(&name).priority;
            let mut run = declared.clone();
            run.sort_by_key(|&i| (Reverse(priority_of(i)), i));
            if let Some(id) = hook_id(&name) {
                subscribed.push(Some(id));
                listeners.push(mod_api::Listener {
                    hook: Some(id),
                    priority: declared.iter().map(|&i| priority_of(i)).max().unwrap_or(0),
                    receive_cancelled: declared
                        .iter()
                        .any(|&i| mods[i].options(&name).receive_cancelled),
                });
            }
            order.push((name, run));
        }
        // The declaration hooks have no Lua handlers: the host answers them from the records. The
        // game asks only the hooks a host subscribes to, so each one is subscribed when a mod
        // declared something for it.
        for (name, declared) in [
            ("blocks", mods.iter().any(|m| !m.blocks.is_empty())),
            ("items", mods.iter().any(|m| !m.items.is_empty())),
            (
                "register_client_commands",
                mods.iter()
                    .any(|m| m.runs_on_client() && !m.client_commands.is_empty()),
            ),
            (
                "register_configs",
                mods.iter()
                    .any(|m| m.configs.iter().any(|c| m.contributes_config(c))),
            ),
            (
                "register_commands",
                mods.iter()
                    .any(|m| m.runs_on_server() && !m.commands.is_empty()),
            ),
            (
                "data_map_types",
                mods.iter()
                    .any(|m| m.runs_on_server() && !m.data_map_types.is_empty()),
            ),
            (
                "data_pack_files",
                mods.iter()
                    .any(|m| m.runs_on_server() && !m.data_pack_files.is_empty()),
            ),
        ] {
            if let (true, Some(id)) = (declared, hook_id(name)) {
                subscribed.push(Some(id));
                listeners.push(mod_api::Listener {
                    hook: Some(id),
                    priority: 0,
                    receive_cancelled: false,
                });
            }
        }
        LuaHost {
            mods: Arc::from(mods),
            subscribed: Box::leak(subscribed.into_boxed_slice()),
            listeners: Box::leak(listeners.into_boxed_slice()),
            order,
            report: Vec::new(),
            reported: AtomicBool::new(false),
            refused_keys: Mutex::new(Vec::new()),
        }
    }

    /// The lines about the folders and mods that did not load, written by `register`.
    pub fn with_report(mut self, lines: Vec<String>) -> LuaHost {
        self.report = lines;
        self
    }

    pub fn report_lines(&self) -> &[String] {
        &self.report
    }

    /// The loaded Lua mods, in load order.
    pub(crate) fn mods(&self) -> &[LuaMod] {
        &self.mods
    }

    /// The indexes into `mods()` of the mods that declared `hook`, in the order they run. Empty
    /// for a hook no mod declared.
    pub(crate) fn run_order(&self, hook: &str) -> &[usize] {
        match self.order.iter().find(|(name, _)| name == hook) {
            Some((_, run)) => run,
            None => &[],
        }
    }

    /// Writes the loaded and refused counts and one line per refusal. The game's logger is up by
    /// `register`, unlike when the mods are read, so this is where the player sees them.
    pub(crate) fn report(&self) {
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        log::info!(
            "Lua mods: {} loaded, {} refused",
            self.mods.len(),
            self.report.len()
        );
        for line in &self.report {
            log::warn!("Lua mod refused: {}", Escaped(line));
        }
        // What the loads warned about (`collect_load_notices`) reaches the log only now, when the
        // game's logger is up. A command with no `run_mod_command` handler is declared but never
        // runs: the game fails it (the host does not answer `ran`). The declaration stays.
        for m in self.mods.iter() {
            for (source, line) in &m.notices {
                warn_once(source, line);
            }
            if !m.commands.is_empty() && !m.hooks.iter().any(|h| h == "run_mod_command") {
                warn_once(
                    &m.id,
                    &format!(
                        "{}: declares commands but has no run_mod_command handler; they will fail when run",
                        m.id
                    ),
                );
            }
        }
    }

    /// Adds the server-side Lua mods to the code mods the world records (`level_mods::set_loaded`),
    /// so a world saved with them remembers them. Nothing changes when there are no such mods
    /// and the list was never set.
    pub fn record_loaded(&self) {
        let existing = level_mods::loaded();
        let merged = merged_records(existing.clone(), &self.mods);
        if existing.is_none() && merged.is_empty() {
            return;
        }
        level_mods::set_loaded(Some(merged));
    }

    /// Calls every handler of `hook` in every mod that declared it. A client-only mod is skipped
    /// for the server hooks. A failing handler is logged and does not stop the others.
    pub fn fire(&self, hook: &str) {
        self.dispatch(
            hook,
            &|lua| new_event(lua, lua.create_table()?, &[]),
            &mut |_, _| true,
        );
    }
}

impl LuaHost {
    /// The hook ids the game subscribes to, read by the generated `Hooks::subscribed`.
    pub(crate) fn subscribed_slice(&self) -> &'static [Option<mod_api::HookId>] {
        self.subscribed
    }

    /// The listeners the game reads, generated `Hooks::listeners`: one per subscribed hook, in the
    /// same order as `subscribed_slice`.
    pub(crate) fn listeners_slice(&self) -> &'static [mod_api::Listener] {
        self.listeners
    }

    /// What `register` does: writes the report, records the loaded mods, and requests the registry
    /// keys of every declared block and item. The generated `Hooks::register` calls it.
    pub(crate) fn on_register(&self, r: &mut dyn mod_api::Registrar) {
        self.report();
        self.record_loaded();
        self.request_keys(&mut RegistrarKeys(r));
    }

    /// Requests the key of each block and item every loaded mod declared, in declaration order. A
    /// mod whose request is refused is dropped from the answers, and the others are not affected.
    ///
    /// Then each block part is attached to its block, and each item part to its item: the key is
    /// requested again (the game keeps one key per id), and the part is handed over. A part that
    /// cannot be made (its subscription is over the interning cap) refuses its mod, as a refused
    /// key does.
    ///
    /// Each mod's block entity types are declared after its blocks and items, and its block entity
    /// parts are attached last. A mod's blocks are its own namespace's (the declaration checks it),
    /// so no two Lua mods name one block.
    pub(crate) fn request_keys(&self, keys: &mut dyn DeclaredKeys) {
        for m in self.mods.iter() {
            let refusal = m
                .blocks
                .iter()
                .find_map(|block| keys.block_key(&block.id).err())
                .or_else(|| {
                    m.items
                        .iter()
                        .find_map(|item| keys.item_key(&item.id).err())
                })
                .or_else(|| {
                    m.block_entity_types
                        .iter()
                        .find_map(|ty| keys.block_entity_type(&ty.id, &ty.blocks).err())
                })
                .or_else(|| {
                    m.fluids
                        .iter()
                        .find_map(|fluid| keys.world_fluid(&fluid.id, &fluid.fluid).err())
                });
            if let Some(reason) = refusal {
                log::warn!(
                    "Lua mod {} refused its declarations: {}",
                    m.id,
                    Escaped(reason)
                );
                lock_ids(&self.refused_keys).push(m.id.clone());
            }
        }
        self.attach_parts::<BlockTag>(|id, part| keys.block_part(id, part));
        self.attach_parts::<ItemTag>(|id, part| keys.item_part(id, part));
        self.attach_parts::<BlockEntityTag>(|id, part| keys.block_entity_part(id, part));
        self.attach_parts::<FluidTag>(|id, part| keys.fluid_part(id, part));
        self.attach_parts::<EntityTag>(|id, part| keys.entity_part(id, part));
        self.attach_parts::<MenuTag>(|id, part| keys.menu_part(id, part));
    }

    /// Hands each part of kind `K` the mods declared to `attach`, with the id it attaches to. A part
    /// that cannot be made or attached refuses its mod, and the others are not affected.
    fn attach_parts<K: PartTag>(
        &self,
        mut attach: impl FnMut(&str, LuaPart<K>) -> Result<(), String>,
    ) {
        for (index, m) in self.mods.iter().enumerate() {
            if self.is_refused(m) {
                continue;
            }
            for (part, record) in m.part_records(K::KIND).iter().enumerate() {
                let attached = LuaPart::<K>::new(Arc::clone(&self.mods), index, part)
                    .and_then(|made| attach(&record.id, made));
                if let Err(reason) = attached {
                    log::warn!(
                        "Lua mod {} refused its {} parts: {}",
                        m.id,
                        K::KIND.keyword(),
                        Escaped(reason)
                    );
                    lock_ids(&self.refused_keys).push(m.id.clone());
                }
            }
        }
    }

    /// Whether `m` was refused by its key requests.
    fn is_refused(&self, m: &LuaMod) -> bool {
        lock_ids(&self.refused_keys).contains(&m.id)
    }

    /// The blocks of every loaded mod that was not refused, in load order and declaration order:
    /// the answer to the game's `blocks` hook.
    pub(crate) fn declared_blocks(&self, out: &mut Vec<mod_api::Block>) {
        for m in self.mods.iter().filter(|m| !self.is_refused(m)) {
            out.extend(m.blocks.iter().map(BlockRecord::to_block));
        }
    }

    /// The items of every loaded mod that was not refused: the answer to the game's `items` hook.
    pub(crate) fn declared_items(&self, out: &mut Vec<mod_api::Item>) {
        for m in self.mods.iter().filter(|m| !self.is_refused(m)) {
            out.extend(m.items.iter().map(ItemRecord::to_item));
        }
    }

    /// The client commands of every loaded mod that runs on a client and was not refused, in load
    /// and declaration order: the answer to the game's `register_client_commands` hook. A server-only
    /// mod's commands are left out, since the client never runs that mod.
    pub(crate) fn declared_client_commands(&self, out: &mut Vec<mod_api::Command>) {
        for m in self
            .mods
            .iter()
            .filter(|m| m.runs_on_client() && !self.is_refused(m))
        {
            out.extend(
                m.client_commands
                    .iter()
                    .map(ClientCommandRecord::to_command),
            );
        }
    }

    /// The server commands of every loaded mod that runs on a server and was not refused, in load and
    /// declaration order: the answer to the game's `register_commands` hook. A client-only mod's
    /// commands are left out, since no server runs that mod.
    pub(crate) fn declared_commands(&self, out: &mut Vec<mod_api::Command>) {
        for m in self
            .mods
            .iter()
            .filter(|m| m.runs_on_server() && !self.is_refused(m))
        {
            out.extend(m.commands.iter().map(CommandRecord::to_command));
        }
    }

    /// The data map types of every loaded mod that runs on a server and was not refused, in load and
    /// declaration order: the answer to the game's `data_map_types` hook.
    pub(crate) fn declared_data_map_types(&self, out: &mut Vec<mod_api::DataMapType>) {
        for m in self
            .mods
            .iter()
            .filter(|m| m.runs_on_server() && !self.is_refused(m))
        {
            out.extend(m.data_map_types.iter().cloned());
        }
    }

    /// The data pack files of every loaded mod that runs on a server and was not refused, in load and
    /// declaration order, each text as its bytes: the answer to the game's `data_pack_files` hook.
    pub(crate) fn declared_data_pack_files(&self, out: &mut Vec<(String, Vec<u8>)>) {
        for m in self
            .mods
            .iter()
            .filter(|m| m.runs_on_server() && !self.is_refused(m))
        {
            out.extend(
                m.data_pack_files
                    .iter()
                    .map(|(key, text)| (key.clone(), text.clone().into_bytes())),
            );
        }
    }

    /// The configs of every loaded mod that was not refused, in load and declaration order: the
    /// answer to the game's `register_configs` hook. A config is left out where its mod does not run
    /// (see `LuaMod::contributes_config`). Each one's `mod_id` is the host's id for the mod.
    pub(crate) fn declared_configs(&self, out: &mut Vec<mod_api::Config>) {
        for m in self.mods.iter().filter(|m| !self.is_refused(m)) {
            out.extend(
                m.configs
                    .iter()
                    .filter(|config| m.contributes_config(config))
                    .cloned(),
            );
        }
    }
}

/// The registry keys `register` requests for the declared blocks and items. The game's Registrar
/// answers them (`RegistrarKeys`); a test double records them.
pub(crate) trait DeclaredKeys {
    /// Requests the key of block `id`. An `Err` refuses the mod that declared it.
    fn block_key(&mut self, id: &str) -> Result<(), String>;
    /// Requests the key of item `id`. An `Err` refuses the mod that declared it.
    fn item_key(&mut self, id: &str) -> Result<(), String>;
    /// Attaches `part` to block `id`. An `Err` refuses the mod that declared the part. The game's
    /// Registrar cannot refuse one mod's part (its refusals are game-wide), so `RegistrarKeys` never
    /// returns one; a test double does, to check the refusal path.
    fn block_part(&mut self, id: &str, part: LuaBlockPart) -> Result<(), String>;
    /// Attaches `part` to item `id`: as [`DeclaredKeys::block_part`].
    fn item_part(&mut self, id: &str, part: LuaItemPart) -> Result<(), String>;
    /// Declares the block entity type `id` in the blocks `blocks`. An `Err` refuses the mod that
    /// declared it.
    fn block_entity_type(&mut self, id: &str, blocks: &[String]) -> Result<(), String>;
    /// Attaches `part` to the block entity type `id`. An `Err` refuses the mod that declared the part;
    /// the game's refusal of a native type's part is reported as one.
    fn block_entity_part(&mut self, id: &str, part: LuaBlockEntityPart) -> Result<(), String>;
    /// Declares the world fluid `id` with `fluid`. An `Err` refuses the mod that declared it.
    fn world_fluid(&mut self, id: &str, fluid: &mod_api::WorldFluid) -> Result<(), String>;
    /// Attaches `part` to the world fluid `id`: as [`DeclaredKeys::block_part`].
    fn fluid_part(&mut self, id: &str, part: LuaFluidPart) -> Result<(), String>;
    /// Attaches `part` to the entity type `id` (any namespace). The game's Registrar checks no type when a
    /// part is attached, so an unknown type is never asked and never refuses the mod.
    fn entity_part(&mut self, id: &str, part: LuaEntityPart) -> Result<(), String>;
    /// Attaches `part` to the mod menu type `id` (any namespace). The game never checks a menu id when a
    /// part is attached: a part on a menu that does not exist is never asked, and never refuses the mod.
    fn menu_part(&mut self, id: &str, part: LuaMenuPart) -> Result<(), String>;
}

/// The game's Registrar as the keys' requester.
struct RegistrarKeys<'a>(&'a mut dyn mod_api::Registrar);

impl DeclaredKeys for RegistrarKeys<'_> {
    fn block_key(&mut self, id: &str) -> Result<(), String> {
        // The game checks every request after all declarations and reports a bad one itself,
        // through its own refusal, so the key is not an answer this host can refuse with.
        let _key = mod_api::BlocksRegistrar::block_key(&mut *self.0, id);
        Ok(())
    }

    fn item_key(&mut self, id: &str) -> Result<(), String> {
        let _key = mod_api::ItemsRegistrar::item_key(&mut *self.0, id);
        Ok(())
    }

    fn block_part(&mut self, id: &str, part: LuaBlockPart) -> Result<(), String> {
        let key = mod_api::BlocksRegistrar::block_key(&mut *self.0, id);
        self.0.block_part(key, part);
        Ok(())
    }

    fn item_part(&mut self, id: &str, part: LuaItemPart) -> Result<(), String> {
        let key = mod_api::ItemsRegistrar::item_key(&mut *self.0, id);
        self.0.item_part(key, part);
        Ok(())
    }

    fn block_entity_type(&mut self, id: &str, blocks: &[String]) -> Result<(), String> {
        // As the block keys: the game reports a bad type itself, and the type's key is requested again
        // by its part (the game keeps one key per id).
        let blocks: Vec<&str> = blocks.iter().map(String::as_str).collect();
        let _key = self.0.block_entity_type(id).blocks(&blocks).build();
        Ok(())
    }

    fn block_entity_part(&mut self, id: &str, part: LuaBlockEntityPart) -> Result<(), String> {
        let key = mod_api::BlockEntitiesRegistrar::block_entity_type_key(&mut *self.0, id);
        self.0
            .block_entity_part(&key, part)
            .map_err(|error| error.to_string())
    }

    fn world_fluid(&mut self, id: &str, fluid: &mod_api::WorldFluid) -> Result<(), String> {
        // As the block keys: the game checks the declaration itself (name, namespace, the numbers)
        // and reports a bad one through its own refusal.
        let _key = mod_api::FluidsRegistrar::declare_world_fluid(&mut *self.0, id, *fluid);
        Ok(())
    }

    fn fluid_part(&mut self, id: &str, part: LuaFluidPart) -> Result<(), String> {
        self.0.fluid_part(id, part);
        Ok(())
    }

    fn entity_part(&mut self, id: &str, part: LuaEntityPart) -> Result<(), String> {
        self.0
            .attach_entity_part(id, std::any::type_name::<LuaEntityPart>(), Arc::new(part));
        Ok(())
    }

    fn menu_part(&mut self, id: &str, part: LuaMenuPart) -> Result<(), String> {
        self.0
            .attach_menu_part(id, std::any::type_name::<LuaMenuPart>(), Arc::new(part));
        Ok(())
    }
}

/// The most distinct sets of part handler names one process interns, per kind of part. Each set's
/// subscription is leaked once, because the game reads it as `'static`.
pub const MAX_PART_SETS: usize = 64;

/// The interned subscriptions of one kind, by their sorted handler names.
type Subscriptions = Vec<(Vec<String>, &'static [Option<mod_api::HookId>])>;

/// The block parts' subscriptions and the item parts' ones: a separate table for each kind.
static BLOCK_SUBSCRIPTIONS: Mutex<Subscriptions> = Mutex::new(Vec::new());
static ITEM_SUBSCRIPTIONS: Mutex<Subscriptions> = Mutex::new(Vec::new());
static BLOCK_ENTITY_SUBSCRIPTIONS: Mutex<Subscriptions> = Mutex::new(Vec::new());
static FLUID_SUBSCRIPTIONS: Mutex<Subscriptions> = Mutex::new(Vec::new());
static ENTITY_SUBSCRIPTIONS: Mutex<Subscriptions> = Mutex::new(Vec::new());
static MENU_SUBSCRIPTIONS: Mutex<Subscriptions> = Mutex::new(Vec::new());

/// The subscription of a set of handler names (sorted, no repeats) in `table`: the same slice for
/// the same set, a new one for a new set, and an error once `MAX_PART_SETS` sets are interned.
fn intern_in(
    table: &mut Subscriptions,
    names: &[String],
) -> Result<&'static [Option<mod_api::HookId>], String> {
    if let Some((_, known)) = table.iter().find(|(known, _)| known == names) {
        return Ok(known);
    }
    if table.len() >= MAX_PART_SETS {
        return Err(format!(
            "a process may hold at most {MAX_PART_SETS} different sets of part handlers of one kind"
        ));
    }
    let ids = names
        .iter()
        .map(|name| {
            hook_id(name)
                .map(Some)
                .ok_or_else(|| format!("'{name}' has no hook id"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let slice: &'static [Option<mod_api::HookId>] = Box::leak(ids.into_boxed_slice());
    table.push((names.to_vec(), slice));
    Ok(slice)
}

/// The marker of a Lua part's kind: [`LuaBlockPart`] and [`LuaItemPart`] are [`LuaPart`] with one
/// marker each, so the game's traits get a distinct type for each.
pub trait PartTag: 'static {
    /// The kind of part the marker stands for.
    const KIND: PartKind;
}

/// The marker of a BlockPart.
pub struct BlockTag;

/// The marker of an ItemPart.
pub struct ItemTag;

/// The marker of a BlockEntityPart.
pub struct BlockEntityTag;

/// The marker of a FluidPart.
pub struct FluidTag;

/// The marker of an EntityPart.
pub struct EntityTag;

/// The marker of a MenuPart.
pub struct MenuTag;

impl PartTag for MenuTag {
    const KIND: PartKind = PartKind::Menu;
}

impl PartTag for EntityTag {
    const KIND: PartKind = PartKind::Entity;
}

impl PartTag for BlockTag {
    const KIND: PartKind = PartKind::Block;
}

impl PartTag for ItemTag {
    const KIND: PartKind = PartKind::Item;
}

impl PartTag for BlockEntityTag {
    const KIND: PartKind = PartKind::BlockEntity;
}

impl PartTag for FluidTag {
    const KIND: PartKind = PartKind::Fluid;
}

/// One block part of a loaded mod: the BlockPart the game calls for a block the mod attached to.
pub type LuaBlockPart = LuaPart<BlockTag>;

/// One item part of a loaded mod: the ItemPart the game calls for an item the mod attached to.
pub type LuaItemPart = LuaPart<ItemTag>;

/// One block entity part of a loaded mod: the BlockEntityPart the game calls for a block entity of a
/// type the mod declared.
pub type LuaBlockEntityPart = LuaPart<BlockEntityTag>;

/// One fluid part of a loaded mod: the FluidPart the game calls for a world fluid the mod declared.
pub type LuaFluidPart = LuaPart<FluidTag>;

/// One entity part of a loaded mod: the EntityPart the game calls for an entity type the mod attached to.
pub type LuaEntityPart = LuaPart<EntityTag>;

/// One menu part of a loaded mod: the MenuPart the game calls for a mod menu type the mod attached to.
pub type LuaMenuPart = LuaPart<MenuTag>;

/// One part of a loaded mod (a block or an item part, by `K`). It runs that mod's handlers for the
/// hooks the part declared, and no other mod's.
pub struct LuaPart<K: PartTag> {
    mods: Arc<[LuaMod]>,
    index: usize,
    id: String,
    hooks: Vec<String>,
    subscribed: &'static [Option<mod_api::HookId>],
    kind: std::marker::PhantomData<K>,
}

impl<K: PartTag> LuaPart<K> {
    /// Part `part` (an index into the mod's records of this kind) of the mod at `index` in `mods`.
    pub(crate) fn new(mods: Arc<[LuaMod]>, index: usize, part: usize) -> Result<Self, String> {
        let (id, hooks) = {
            let record = &mods[index].part_records(K::KIND)[part];
            (record.id.clone(), record.hooks.clone())
        };
        let subscribed = intern_in(
            &mut lock_subscriptions(K::KIND),
            &subscribed_names(K::KIND, &hooks),
        )?;
        Ok(LuaPart {
            mods,
            index,
            id,
            hooks,
            subscribed,
            kind: std::marker::PhantomData,
        })
    }

    /// The hook ids the part subscribes to: the generated `subscribed` of its trait.
    pub(crate) fn subscribed(&self) -> &'static [Option<mod_api::HookId>] {
        self.subscribed
    }

    /// Whether the part has a handler for `hook` that its mod runs (see `dispatch::side_runs`).
    pub(crate) fn has_handlers(&self, hook: &str) -> bool {
        self.hooks.iter().any(|name| name == hook)
            && dispatch::side_runs(&self.mods[self.index], hook)
    }

    /// The generated methods' turn: the part's handler, as `LuaHost::dispatch_turns` runs a hook.
    pub(crate) fn dispatch_turns(
        &self,
        hook: &str,
        host: crate::hostcall::TurnHost<'_>,
        is_cancelled: &dyn Fn() -> bool,
        build: &dyn Fn(
            &Lua,
            &crate::hostcall::TurnHost<'_>,
        ) -> mlua::Result<crate::dispatch::Event>,
        commit: &mut dyn FnMut(
            &str,
            &crate::dispatch::Event,
            &crate::hostcall::TurnHost<'_>,
        ) -> bool,
    ) -> Result<(), String> {
        let key = declare::part_handler_key(K::KIND, &self.id, hook);
        dispatch::turns_in(
            &self.mods,
            &[self.index],
            hook,
            &key,
            host,
            &|_| true,
            is_cancelled,
            build,
            commit,
        )
    }

    /// The generated methods' replace turn, as `LuaHost::dispatch_replace` runs a replace hook.
    pub(crate) fn dispatch_replace(
        &self,
        hook: &str,
        host: crate::hostcall::TurnHost<'_>,
        build: &dyn Fn(
            &Lua,
            &crate::hostcall::TurnHost<'_>,
        ) -> mlua::Result<crate::dispatch::Event>,
        answer: &mut dyn FnMut(&str, Value, &crate::hostcall::TurnHost<'_>) -> bool,
    ) -> Result<(), String> {
        let key = declare::part_handler_key(K::KIND, &self.id, hook);
        dispatch::replace_in(&self.mods, &[self.index], hook, &key, host, build, answer)
    }
}

/// The hook names a part of `kind` subscribes to, from its handler names. An entity part's plain
/// methods (`is_shearable` and the rest) are no hook and have no hook id: the game calls them
/// directly, and the part's handler table decides which ones run. So only the names that are hooks
/// are interned for an entity part; for the other kinds every handler name is a hook.
fn subscribed_names(kind: PartKind, hooks: &[String]) -> Vec<String> {
    match kind {
        PartKind::Entity => hooks
            .iter()
            .filter(|name| hook_id(name).is_some())
            .cloned()
            .collect(),
        _ => hooks.to_vec(),
    }
}

/// The table of the subscriptions of parts of `kind`.
fn subscriptions_of(kind: PartKind) -> &'static Mutex<Subscriptions> {
    match kind {
        PartKind::Block => &BLOCK_SUBSCRIPTIONS,
        PartKind::Item => &ITEM_SUBSCRIPTIONS,
        PartKind::BlockEntity => &BLOCK_ENTITY_SUBSCRIPTIONS,
        PartKind::Fluid => &FLUID_SUBSCRIPTIONS,
        PartKind::Entity => &ENTITY_SUBSCRIPTIONS,
        PartKind::Menu => &MENU_SUBSCRIPTIONS,
    }
}

fn lock_subscriptions(kind: PartKind) -> MutexGuard<'static, Subscriptions> {
    subscriptions_of(kind)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn lock_ids(ids: &Mutex<Vec<String>>) -> MutexGuard<'_, Vec<String>> {
    ids.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs one handler under `HANDLER_BUDGET`. The budget keeps failing after the deadline, so a
/// `pcall` inside the handler cannot swallow the stop.
pub(crate) fn call_handler(lua: &Lua, handler: &Function, event: &Table) -> mlua::Result<()> {
    budget::within_with(
        lua,
        HANDLER_BUDGET,
        "the handler ran over its time budget",
        || handler.call::<()>(event.clone()),
    )
}

/// Runs one handler under `HANDLER_BUDGET` and returns what it returned: its first value, or `nil`
/// when it returned nothing. A replace hook's answer is this value.
pub(crate) fn call_handler_value(
    lua: &Lua,
    handler: &Function,
    event: &Table,
) -> mlua::Result<Value> {
    budget::within_with(
        lua,
        HANDLER_BUDGET,
        "the handler ran over its time budget",
        || handler.call::<Value>(event.clone()),
    )
}

/// The `horizon` table a mod's handlers see: `log`, and `none` (the sentinel that answers None on an
/// `Option` output). Any other field errors, and `on` only refuses, because hooks are declared while
/// the mod loads.
fn runtime_table(lua: &Lua, id: &str, log: Arc<Mutex<Vec<String>>>) -> mlua::Result<Table> {
    let horizon = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set(
        "__index",
        lua.create_function(|_, (_, field): (Value, Value)| -> mlua::Result<Value> {
            let name = match field {
                Value::String(s) => s.to_string_lossy(),
                other => other.type_name().to_string(),
            };
            Err(Error::runtime(format!(
                "horizon.{name} is not available yet"
            )))
        })?,
    )?;
    horizon.set_metatable(Some(meta))?;
    horizon.raw_set(
        "on",
        lua.create_function(|_, _: mlua::MultiValue| -> mlua::Result<()> {
            Err(Error::runtime(LATE_ON))
        })?,
    )?;
    horizon.raw_set("data_map", crate::data_maps::data_map_function(lua)?)?;
    crate::attachments::install(lua, &horizon, id)?;
    let id = id.to_string();
    let limit = Mutex::new(RateLimit::new(LOG_BURST, LOG_PER_SECOND));
    horizon.raw_set(
        "log",
        lua.create_function(move |_, message: Value| -> mlua::Result<()> {
            let Value::String(message) = message else {
                return Err(Error::runtime("horizon.log needs the message as a string"));
            };
            // A line over the rate is dropped without an error: the mod is not told.
            let verdict = limit
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .allow(Instant::now());
            match verdict {
                Verdict::Dropped => return Ok(()),
                Verdict::AllowedAfter(n) => {
                    log::warn!(target: LOG_TARGET, "{id}: {n} log lines were dropped");
                }
                Verdict::Allowed => {}
            }
            let line = escape_capped(&message.to_string_lossy(), MAX_LOG_LINE);
            keep_line(&log, line.clone());
            log::info!(target: LOG_TARGET, "{id}: {line}");
            Ok(())
        })?,
    )?;
    horizon.raw_set("none", lua.create_userdata(crate::dispatch::NoneAnswer)?)?;
    horizon.set_readonly(true);
    Ok(horizon)
}

/// Reads a mod's `main.luau`, reading at most `MAX_SOURCE + 1` bytes. A symlink is refused.
///
/// The file is opened, then the opened file and the path are checked, and only then read, so a
/// path swapped for a symlink or another file in between is refused (see `same_file`). The path
/// is also checked before the open, so a FIFO is not opened in the normal case: opening one
/// blocks until a writer appears.
fn read_source(dir: &Path) -> Result<String, String> {
    let dir_meta = std::fs::symlink_metadata(dir).map_err(|_| format!("no {ENTRY}"))?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Err("mod folder is not a plain folder".to_string());
    }
    let path = dir.join(ENTRY);
    let before = std::fs::symlink_metadata(&path).map_err(|_| format!("no {ENTRY}"))?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err(format!("{ENTRY} is not a plain file"));
    }
    let file = std::fs::File::open(&path).map_err(|_| format!("{ENTRY} cannot be read"))?;
    let opened = file
        .metadata()
        .map_err(|_| format!("{ENTRY} cannot be read"))?;
    if !opened.is_file() {
        return Err(format!("{ENTRY} is not a plain file"));
    }
    if !same_file(&file, &path) {
        return Err(format!("{ENTRY} changed while it was opened"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{ENTRY} cannot be read"))?;
    if bytes.len() as u64 > MAX_SOURCE {
        return Err(format!("{ENTRY} is over {} KiB", MAX_SOURCE / 1024));
    }
    String::from_utf8(bytes).map_err(|_| format!("{ENTRY} is not UTF-8"))
}

/// Whether `path` still names the file that `opened` was opened from. A path that is now a
/// symlink, or not a plain file, never does.
fn same_file(opened: &std::fs::File, path: &Path) -> bool {
    let Ok(now) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if now.file_type().is_symlink() || !now.is_file() {
        return false;
    }
    same_inode(opened, &now)
}

/// Whether the opened file and the path's current metadata are the same device and inode.
#[cfg(unix)]
fn same_inode(opened: &std::fs::File, now: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    opened
        .metadata()
        .is_ok_and(|o| o.dev() == now.dev() && o.ino() == now.ino())
}

/// Other platforms have no inode to compare: the symlink and plain-file checks are all there is.
#[cfg(not(unix))]
fn same_inode(_opened: &std::fs::File, _now: &std::fs::Metadata) -> bool {
    true
}

/// One character as it is written in a log line: a control character (a newline, a carriage return,
/// an escape, ...) as its `\u{..}` escape text, so a mod cannot start a line of its own or forge the
/// game's own prefix. Any other character is itself.
fn log_piece(c: char) -> String {
    if c.is_control() {
        format!("\\u{{{:x}}}", u32::from(c))
    } else {
        c.to_string()
    }
}

/// `text` as one log line, with its control characters escaped (`log_piece`) and at most `cap`
/// characters counted after escaping. An escape is never cut: a piece that would pass the cap is
/// left out whole.
pub(crate) fn escape_capped(text: &str, cap: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in text.chars() {
        let piece = log_piece(c);
        let width = piece.chars().count();
        if count + width > cap {
            break;
        }
        out.push_str(&piece);
        count += width;
    }
    out
}

/// Any text a mod can shape (a Lua error's message, a refusal's reason, a name it chose), written into
/// a log message with its control characters escaped, as `horizon.log` writes its line. Use it in
/// `log::` arguments wherever the text came from a mod.
pub(crate) struct Escaped<T: std::fmt::Display>(pub T);

impl<T: std::fmt::Display> std::fmt::Display for Escaped<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0
            .to_string()
            .chars()
            .try_for_each(|c| f.write_str(&log_piece(c)))
    }
}

/// The most distinct sources (mods, and "host" for lines with no mod) that [`warn_once`] tracks in
/// one process. A new source past it is counted and dropped, and one line says so.
const MAX_SOURCES: usize = 256;

/// The most distinct lines one source may log through [`warn_once`]. Past it that source's further
/// lines are counted and dropped, and one line says so; other sources are not affected.
const MAX_LINES_PER_SOURCE: usize = 16;

/// The lines [`warn_once`] has logged for one source, and how many it has dropped past
/// [`MAX_LINES_PER_SOURCE`].
#[derive(Default)]
struct SourceWarnings {
    seen: std::collections::BTreeSet<String>,
    suppressed: u64,
}

/// The warnings [`warn_once`] has logged, per source, and how many it has dropped past the caps.
#[derive(Default)]
pub(crate) struct Warned {
    sources: std::collections::BTreeMap<String, SourceWarnings>,
    dropped_sources: u64,
}

impl Warned {
    /// The text to log for `line` from `source`: `line` itself the first time, nothing again. Past
    /// the source's cap, one "{source}: further warnings suppressed" line; a new source past
    /// [`MAX_SOURCES`] is counted and dropped, with one global line.
    pub(crate) fn once(&mut self, source: &str, line: &str) -> Option<String> {
        if !self.sources.contains_key(source) && self.sources.len() >= MAX_SOURCES {
            self.dropped_sources += 1;
            return (self.dropped_sources == 1).then(|| {
                format!("further warnings suppressed: more than {MAX_SOURCES} sources this process")
            });
        }
        let entry = self.sources.entry(source.to_owned()).or_default();
        if entry.seen.contains(line) {
            return None;
        }
        if entry.seen.len() < MAX_LINES_PER_SOURCE {
            entry.seen.insert(line.to_owned());
            return Some(line.to_owned());
        }
        entry.suppressed += 1;
        (entry.suppressed == 1).then(|| format!("{source}: further warnings suppressed"))
    }
}

static WARNED: Mutex<Warned> = Mutex::new(Warned {
    sources: std::collections::BTreeMap::new(),
    dropped_sources: 0,
});

thread_local! {
    /// The warnings a mod's load gives on this thread, while `collect_load_notices` runs. `None`
    /// outside a load.
    static LOAD_NOTICES: RefCell<Option<Vec<(String, String)>>> = const { RefCell::new(None) };
}

/// Runs `load`, a mod's declaration, and returns what it gave with the warnings it gave: each one
/// is kept, not logged. A load runs before the game's logger is up (`builtin` reads the mods, and
/// `register` writes the report), so the warning would be lost. `LuaHost::report` logs them then,
/// through `warn_once`, with the same text.
pub(crate) fn collect_load_notices<T>(load: impl FnOnce() -> T) -> (T, Vec<(String, String)>) {
    /// Puts back the collector of the outer load, if there was one, when this load ends.
    struct Restore(Option<Vec<(String, String)>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            LOAD_NOTICES.with(|notices| *notices.borrow_mut() = self.0.take());
        }
    }
    let outer = LOAD_NOTICES.with(|notices| notices.borrow_mut().replace(Vec::new()));
    let _restore = Restore(outer);
    let out = load();
    let notices = LOAD_NOTICES
        .with(|notices| notices.borrow_mut().as_mut().map(std::mem::take))
        .unwrap_or_default();
    (out, notices)
}

/// Logs `line` as a warning from `source` (a mod's id, or "host"), once per process for each
/// distinct line: a refusal or a failure a mod repeats on every call is logged at its first
/// occurrence only. Each source has its own cap, so one mod's errors cannot silence another's.
/// Inside a mod's load (`collect_load_notices`) the line is kept for the report instead.
pub(crate) fn warn_once(source: &str, line: &str) {
    let kept = LOAD_NOTICES.with(|notices| match notices.borrow_mut().as_mut() {
        Some(notices) => {
            notices.push((source.to_owned(), line.to_owned()));
            true
        }
        None => false,
    });
    if kept {
        return;
    }
    let text = WARNED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .once(source, line);
    if let Some(text) = text {
        log::warn!(target: LOG_TARGET, "{text}");
    }
}

/// Appends `line` to a mod's buffer, dropping the oldest lines past `MAX_LOG_LINES`.
fn keep_line(log: &Mutex<Vec<String>>, line: String) {
    let mut lines = lock(log);
    lines.push(line);
    if lines.len() > MAX_LOG_LINES {
        let excess = lines.len() - MAX_LOG_LINES;
        lines.drain(..excess);
    }
}

/// The log buffer's lock. A poisoned lock still holds the lines, so it is taken anyway.
fn lock(log: &Mutex<Vec<String>>) -> MutexGuard<'_, Vec<String>> {
    log.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Instant;

    /// A server hook the generated impl has, with a cancel answer: the tests fire it by name.
    const HOOK: &str = "block_receive_game";

    /// A mod folder with `source` as its main.luau; returns what discovery would have reported.
    fn mod_folder(root: &Path, name: &str, source: &str, side: Side) -> Discovered {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), source).unwrap();
        Discovered {
            dir,
            id: name.to_string(),
            version: "1".to_string(),
            side,
        }
    }

    fn load_one(source: &str) -> Result<LuaMod, String> {
        let root = tempfile::tempdir().unwrap();
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        LuaMod::load(&found)
    }

    #[test]
    fn the_part_subscription_table_interns_each_set_once_and_refuses_the_65th() {
        let names: Vec<String> = crate::generated::hooks::HOOKS
            .iter()
            .map(|hook| hook.name.to_owned())
            .collect();
        let mut table = Vec::new();
        for name in names.iter().take(MAX_PART_SETS) {
            intern_in(&mut table, std::slice::from_ref(name)).unwrap();
        }
        let first = intern_in(&mut table, &names[..1]).unwrap();
        assert!(std::ptr::eq(first, table[0].1));
        let err = match intern_in(&mut table, &names[MAX_PART_SETS..=MAX_PART_SETS]) {
            Err(err) => err,
            Ok(_) => panic!("the 65th set was interned"),
        };
        assert!(err.contains("at most 64 different sets"), "got {err}");
    }

    #[test]
    fn each_part_kind_interns_its_subscriptions_in_a_separate_table() {
        let kinds = [
            PartKind::Block,
            PartKind::Item,
            PartKind::BlockEntity,
            PartKind::Fluid,
            PartKind::Menu,
        ];
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                assert!(!std::ptr::eq(subscriptions_of(*a), subscriptions_of(*b)));
            }
        }
    }

    #[test]
    fn a_loaded_mod_declares_its_hooks() {
        let root = tempfile::tempdir().unwrap();
        let found = mod_folder(
            root.path(),
            "mod_a",
            "horizon.on(\"block_receive_game\", function() horizon.log(\"tick\") end)",
            Side::Both,
        );
        let loaded = LuaMod::load(&found).unwrap();
        assert_eq!(loaded.hooks(), ["block_receive_game"]);
        assert_eq!(loaded.id(), "mod_a");
        assert_eq!(loaded.side(), Side::Both);
    }

    #[test]
    fn the_host_subscribes_to_the_union_once() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", source, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", source, Side::Server)).unwrap();
        let host = LuaHost::new(vec![a, b]);
        let subscribed = mod_api::Hooks::subscribed(&host);
        assert_eq!(subscribed.len(), 1);
        assert_eq!(
            subscribed[0].map(|id| id as usize),
            Some(mod_api::HookId::BlockReceiveGame as usize)
        );
    }

    #[test]
    fn fire_runs_the_handlers() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() horizon.log(\"tick\") end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(HOOK);
        assert_eq!(lock(&log).clone(), ["tick"]);
        host.fire(HOOK);
        assert_eq!(lock(&log).clone(), ["tick", "tick"]);
    }

    #[test]
    fn an_unknown_hook_in_main_luau_refuses_the_mod() {
        let err = load_one("horizon.on(\"nope\", function() end)")
            .err()
            .unwrap();
        assert!(err.contains("unknown hook 'nope'"), "got {err}");
    }

    #[test]
    fn a_hook_without_a_generated_method_is_refused() {
        // `play_sound` writes back (`sound: &mut Option<SoundPlay>`), so it has no generated method.
        let err = load_one("horizon.on(\"play_sound\", function() end)")
            .err()
            .unwrap();
        assert!(err.contains("unknown hook 'play_sound'"), "got {err}");
    }

    #[test]
    fn a_server_tick_listener_is_declared_now_that_the_phase_converts() {
        let loaded = load_one("horizon.on(\"server_tick\", function() end)").unwrap();
        assert_eq!(loaded.hooks(), ["server_tick"]);
    }

    /// The host argument of a server hook: it has no state the Lua hooks read.
    struct NoHost;
    impl mod_api::Host for NoHost {}
    mod_api::impl_host_areas!(NoHost);
    impl mod_api::ServerHost for NoHost {}
    mod_api::impl_server_host_areas!(NoHost);

    /// A block the game received, as the generated `block_receive_game` gets it.
    fn received(cancelled: bool) -> mod_api::BlockReceiveGameEvent<'static> {
        mod_api::BlockReceiveGameEvent {
            dimension: "minecraft:overworld",
            pos: [1, 64, 2],
            block: "minecraft:stone",
            event: "minecraft:block_event",
            cancelled,
        }
    }

    #[test]
    fn a_generated_cancel_hook_returns_cancel_when_a_mod_cancels() {
        let m = load_one(
            "horizon.on(\"block_receive_game\", function(event) event.cancelled = true end)",
        )
        .unwrap();
        let host = LuaHost::new(vec![m]);
        let mut event = received(false);
        let flow = mod_api::Hooks::block_receive_game(&host, &mut NoHost, &mut event);
        assert!(matches!(flow, mod_api::Flow::Cancel));
    }

    #[test]
    fn a_generated_cancel_hook_returns_continue_when_no_mod_cancels() {
        let m = load_one(
            "horizon.on(\"block_receive_game\", function(event) horizon.log(event.block) end)",
        )
        .unwrap();
        let host = LuaHost::new(vec![m]);
        let mut event = received(false);
        let flow = mod_api::Hooks::block_receive_game(&host, &mut NoHost, &mut event);
        assert!(matches!(flow, mod_api::Flow::Continue));
        assert_eq!(host.mods()[0].log_lines(), ["minecraft:stone"]);
    }

    #[test]
    fn a_client_mod_is_not_fired_for_a_server_hook() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() horizon.log(\"tick\") end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Client);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(HOOK);
        assert!(lock(&log).is_empty());
    }

    #[test]
    fn a_failing_handler_does_not_stop_the_others() {
        let root = tempfile::tempdir().unwrap();
        let failing = "horizon.on(\"block_receive_game\", function() error(\"boom\") end)";
        let logging =
            "horizon.on(\"block_receive_game\", function() horizon.log(\"still here\") end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", failing, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", logging, Side::Both)).unwrap();
        let b_log = Arc::clone(&b.log);
        let host = LuaHost::new(vec![a, b]);
        host.fire(HOOK);
        assert_eq!(lock(&b_log).clone(), ["still here"]);
    }

    #[test]
    fn a_runaway_handler_is_stopped_and_the_others_run() {
        let root = tempfile::tempdir().unwrap();
        let runaway = "horizon.on(\"block_receive_game\", function() while true do end end)";
        let logging = "horizon.on(\"block_receive_game\", function() horizon.log(\"ran\") end)";
        let a = LuaMod::load(&mod_folder(root.path(), "mod_a", runaway, Side::Both)).unwrap();
        let b = LuaMod::load(&mod_folder(root.path(), "mod_b", logging, Side::Both)).unwrap();
        let b_log = Arc::clone(&b.log);
        let host = LuaHost::new(vec![a, b]);
        let started = Instant::now();
        host.fire(HOOK);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(lock(&b_log).clone(), ["ran"]);
    }

    #[test]
    fn horizon_log_is_not_available_while_the_mod_loads() {
        let err = load_one("horizon.log(\"x\")").err().unwrap();
        assert!(
            err.contains("not available while the mod loads"),
            "got {err}"
        );
    }

    #[test]
    fn a_runtime_field_that_does_not_exist_errors() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function()\n\
                      local ok = pcall(function() return horizon.world end)\n\
                      horizon.log(tostring(ok))\n\
                      end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let loaded = LuaMod::load(&found).unwrap();
        let log = Arc::clone(&loaded.log);
        let host = LuaHost::new(vec![loaded]);
        host.fire(HOOK);
        assert_eq!(lock(&log).clone(), ["false"]);
    }

    #[test]
    fn an_oversized_source_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let source = "-".repeat(MAX_SOURCE as usize + 1);
        let found = mod_folder(root.path(), "mod_a", &source, Side::Both);
        let err = LuaMod::load(&found).err().unwrap();
        assert!(err.contains("over"), "got {err}");
    }

    #[test]
    fn the_log_keeps_the_newest_lines() {
        // The rate limit lets a burst through, so the cap is checked on the buffer itself.
        let log = Mutex::new(Vec::new());
        for i in 1..=MAX_LOG_LINES + 49 {
            keep_line(&log, format!("line {i}"));
        }
        let lines = lock(&log).clone();
        assert_eq!(lines.len(), MAX_LOG_LINES);
        assert_eq!(lines[0], "line 50");
        assert_eq!(
            lines[MAX_LOG_LINES - 1],
            format!("line {}", MAX_LOG_LINES + 49)
        );
    }

    #[test]
    fn a_long_line_is_cut() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function()\n\
                      horizon.log(string.rep(\"x\", 2000))\n\
                      end)";
        let found = mod_folder(root.path(), "mod_a", source, Side::Both);
        let host = LuaHost::new(vec![LuaMod::load(&found).unwrap()]);
        host.fire(HOOK);
        let lines = host.mods[0].log_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].chars().count(), MAX_LOG_LINE);
    }

    /// Runs a mod whose `block_receive_game` handler is `body` once, and returns its log lines.
    fn logged(root: &Path, body: &str) -> Vec<String> {
        let source = format!("horizon.on(\"block_receive_game\", function()\n{body}\nend)");
        let found = mod_folder(root, "mod_log", &source, Side::Both);
        let host = LuaHost::new(vec![LuaMod::load(&found).unwrap()]);
        host.fire(HOOK);
        host.mods[0].log_lines()
    }

    #[test]
    fn control_characters_are_logged_escaped_on_one_line() {
        // A newline cannot start a line of the game's own, and an escape is shown, not sent.
        let root = tempfile::tempdir().unwrap();
        let lines = logged(root.path(), r#"horizon.log("x\ny\rz\27[0m")"#);
        assert_eq!(lines, [r"x\u{a}y\u{d}z\u{1b}[0m"]);

        let root = tempfile::tempdir().unwrap();
        let forged = logged(
            root.path(),
            r#"horizon.log("x\n[12:00:00] [Server thread/INFO]: <Admin> ok")"#,
        );
        assert_eq!(forged.len(), 1, "got {forged:?}");
        assert_eq!(
            forged[0],
            r"x\u{a}[12:00:00] [Server thread/INFO]: <Admin> ok"
        );
    }

    #[test]
    fn a_message_of_4096_characters_is_capped_at_max_log_line() {
        let root = tempfile::tempdir().unwrap();
        let lines = logged(root.path(), r#"horizon.log(string.rep("x", 4096))"#);
        assert_eq!(lines[0].chars().count(), MAX_LOG_LINE);
        assert!(lines[0].chars().all(|c| c == 'x'));
    }

    #[test]
    fn the_cap_counts_escaped_characters_and_never_cuts_an_escape() {
        // A newline is five characters once escaped: 600 of them fill the cap exactly (200 * 5 = 1000).
        let root = tempfile::tempdir().unwrap();
        let lines = logged(root.path(), r#"horizon.log(string.rep("\n", 600))"#);
        assert_eq!(lines[0].chars().count(), MAX_LOG_LINE);
        assert_eq!(lines[0], r"\u{a}".repeat(200));

        // 997 characters leave 3: the newline's escape (5) would pass the cap, so it is left out whole.
        let root = tempfile::tempdir().unwrap();
        let lines = logged(
            root.path(),
            r#"horizon.log(string.rep("x", 997) .. "\n" .. "yyy")"#,
        );
        assert_eq!(lines[0], "x".repeat(997));
    }

    #[test]
    fn an_ordinary_message_is_logged_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let lines = logged(root.path(), r#"horizon.log("hello, wörld: 3 + 4 = 7")"#);
        assert_eq!(lines, ["hello, wörld: 3 + 4 = 7"]);
    }

    #[test]
    fn escaped_text_in_a_host_message_has_no_control_characters() {
        assert_eq!(format!("{}", Escaped("a\nb\u{1b}")), r"a\u{a}b\u{1b}");
        assert_eq!(escape_capped("a\u{7f}b", 100), r"a\u{7f}b");
    }

    #[test]
    fn a_missing_main_luau_is_an_error() {
        let found = Discovered {
            dir: PathBuf::from("/nonexistent/eh-lua-mod"),
            id: "mod_a".into(),
            version: "1".into(),
            side: Side::Both,
        };
        assert!(LuaMod::load(&found).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_main_luau_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside.luau");
        std::fs::write(
            &outside,
            "horizon.on(\"block_receive_game\", function() end)",
        )
        .unwrap();
        let dir = root.path().join("mod_a");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(ENTRY)).unwrap();
        let err = read_source(&dir).err().unwrap();
        assert!(err.contains("not a plain file"), "got {err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_mod_folder_is_refused_at_load() {
        let root = tempfile::tempdir().unwrap();
        let real = mod_folder(
            root.path(),
            "real",
            "horizon.on(\"block_receive_game\", function() end)",
            Side::Both,
        );
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(&real.dir, &link).unwrap();
        let found = Discovered {
            dir: link,
            id: "linked".into(),
            version: "1".into(),
            side: Side::Both,
        };
        let err = LuaMod::load(&found).err().unwrap();
        assert!(err.contains("not a plain folder"), "got {err}");
    }

    #[test]
    fn a_file_swapped_after_open_is_detected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(ENTRY);
        std::fs::write(&path, "first").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        assert!(same_file(&file, &path));

        // The open handle keeps the old inode alive, so the new file cannot reuse its number.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "second").unwrap();
        assert!(!same_file(&file, &path));

        #[cfg(unix)]
        {
            let outside = root.path().join("outside.luau");
            std::fs::write(&outside, "third").unwrap();
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert!(!same_file(&file, &path));
        }
    }

    #[test]
    fn pcall_cannot_swallow_a_handlers_budget() {
        // The interrupt keeps failing after the deadline, so catching one error does not help.
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function() while true do pcall(function() while true do end end) end end)";
        let m = LuaMod::load(&mod_folder(root.path(), "stubborn", source, Side::Both)).unwrap();
        let host = LuaHost::new(vec![m]);
        let started = Instant::now();
        host.fire(HOOK);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A loaded mod with `id`, `version` and `side`, with no hooks.
    fn lua_mod(root: &Path, id: &str, version: &str, side: Side) -> LuaMod {
        let mut found = mod_folder(root, id, "", side);
        found.version = version.to_string();
        LuaMod::load(&found).unwrap()
    }

    fn record(id: &str, version: &str) -> ModRecord {
        ModRecord {
            id: id.into(),
            version: version.into(),
        }
    }

    #[test]
    fn server_and_both_mods_are_added_and_client_mods_are_not() {
        let root = tempfile::tempdir().unwrap();
        let mods = [
            lua_mod(root.path(), "client_mod", "1.0.0", Side::Client),
            lua_mod(root.path(), "server_mod", "2.0.0", Side::Server),
            lua_mod(root.path(), "both_mod", "3.0.0", Side::Both),
        ];
        assert_eq!(
            merged_records(None, &mods),
            [record("server_mod", "2.0.0"), record("both_mod", "3.0.0")]
        );
    }

    #[test]
    fn existing_records_come_first_and_ids_are_not_repeated() {
        let root = tempfile::tempdir().unwrap();
        let mods = [
            lua_mod(root.path(), "alpha_mod", "1.0.0", Side::Both),
            lua_mod(root.path(), "beta_mod", "2.0.0", Side::Server),
        ];
        let existing = Some(vec![
            record("native_mod", "0.1.0"),
            record("alpha_mod", "1.0.0"),
        ]);
        assert_eq!(
            merged_records(existing, &mods),
            [
                record("native_mod", "0.1.0"),
                record("alpha_mod", "1.0.0"),
                record("beta_mod", "2.0.0"),
            ]
        );
    }

    #[test]
    fn merging_twice_changes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let mods = [
            lua_mod(root.path(), "alpha_mod", "1.0.0", Side::Both),
            lua_mod(root.path(), "beta_mod", "2.0.0", Side::Server),
        ];
        let first = merged_records(None, &mods);
        assert_eq!(merged_records(Some(first.clone()), &mods), first);
    }

    /// The loaded list is the process's, so this runs under nextest (one process per test), where
    /// no other test sees it.
    #[test]
    fn record_loaded_updates_the_process_list() {
        let root = tempfile::tempdir().unwrap();
        let previous = level_mods::set_loaded(None);
        let host = LuaHost::new(vec![lua_mod(root.path(), "both_mod", "3.0.0", Side::Both)]);
        host.record_loaded();
        let loaded = level_mods::loaded().unwrap();
        assert!(loaded.contains(&record("both_mod", "3.0.0")));
        level_mods::set_loaded(previous);
    }

    #[test]
    fn a_second_record_loaded_is_a_no_op() {
        let root = tempfile::tempdir().unwrap();
        let previous = level_mods::set_loaded(None);
        let host = LuaHost::new(vec![lua_mod(root.path(), "both_mod", "3.0.0", Side::Both)]);
        host.record_loaded();
        host.record_loaded();
        let loaded = level_mods::loaded().unwrap();
        assert_eq!(loaded.iter().filter(|m| m.id == "both_mod").count(), 1);
        level_mods::set_loaded(previous);
    }

    /// Adds a handler for `name` to the mod, as a second hook the mod could declare. Only the
    /// game's own hooks can be declared through `horizon.on`, so the test registers the hook here.
    #[test]
    fn two_threads_calling_one_state_each_keep_their_own_limit() {
        let root = tempfile::tempdir().unwrap();
        let runaway = "horizon.on(\"block_receive_game\", function() while true do end end)";
        // The other hook counts in a Lua global rather than logging: log lines are rate limited.
        let source =
            format!("{runaway}\nhorizon.on(\"animal_tame\", function() n = (n or 0) + 1 end)");
        let m = LuaMod::load(&mod_folder(root.path(), "mod_a", &source, Side::Both)).unwrap();
        let host = LuaHost::new(vec![m]);
        let started = Instant::now();
        thread::scope(|s| {
            let a = s.spawn(|| {
                let began = Instant::now();
                host.fire(HOOK);
                began.elapsed()
            });
            s.spawn(|| {
                for _ in 0..50 {
                    host.fire("animal_tame");
                }
            });
            let a_elapsed = a.join().unwrap();
            assert!(
                a_elapsed < HANDLER_BUDGET * 5,
                "the runaway was not stopped in time: {a_elapsed:?}"
            );
        });
        assert!(started.elapsed() < Duration::from_secs(10));
        let env = declare::environment(&host.mods[0].lua).unwrap();
        let others: i64 = env.get("n").unwrap();
        assert_eq!(others, 50, "every other call must have run");
    }

    #[test]
    fn the_turn_lock_is_reentrant_and_exclusive() {
        let root = tempfile::tempdir().unwrap();
        let m = lua_mod(root.path(), "mod_a", "", Side::Both);
        let turn = Arc::new(m.turn);
        // Re-entry on one thread must not deadlock: a watchdog fails the test if it does.
        let (done_tx, done_rx) = mpsc::channel();
        let reentrant = Arc::clone(&turn);
        thread::spawn(move || {
            let _outer = reentrant.enter();
            let _inner = reentrant.enter();
            let _deepest = reentrant.enter();
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("re-entering the turn on one thread deadlocked");

        // Another thread waits while the turn is held, and gets it once it is free.
        let holder = turn.enter();
        let (entered_tx, entered_rx) = mpsc::channel();
        let waiter = Arc::clone(&turn);
        let handle = thread::spawn(move || {
            let _turn = waiter.enter();
            let _ = entered_tx.send(());
        });
        assert!(
            entered_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "a second thread entered a held turn"
        );
        drop(holder);
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the waiting thread never got the free turn");
        handle.join().unwrap();
    }

    #[test]
    fn a_panic_inside_a_turn_releases_it() {
        let root = tempfile::tempdir().unwrap();
        let m = lua_mod(root.path(), "mod_a", "", Side::Both);
        let turn = Arc::new(m.turn);
        let panicking = Arc::clone(&turn);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _turn = panicking.enter();
            panic!("a handler panicked");
        }));
        assert!(caught.is_err());
        // Another thread enters at once only if the panic gave the turn back.
        let (entered_tx, entered_rx) = mpsc::channel();
        let after = Arc::clone(&turn);
        thread::spawn(move || {
            let _turn = after.enter();
            let _ = entered_tx.send(());
        });
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the panicking thread kept the turn");
    }

    /// Every line the process logs, for the tests that check what is reported.
    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct Capture;

    static CAPTURE: Capture = Capture;

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            lock_captured().push(record.args().to_string());
        }

        fn flush(&self) {}
    }

    fn lock_captured() -> MutexGuard<'static, Vec<String>> {
        CAPTURED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Installs the capturing logger once for the process.
    pub(crate) fn install_capture() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = log::set_logger(&CAPTURE);
            log::set_max_level(log::LevelFilter::Info);
        });
    }

    pub(crate) fn captured_with_prefix(prefix: &str) -> Vec<String> {
        install_capture();
        lock_captured()
            .iter()
            .filter(|line| line.starts_with(prefix))
            .cloned()
            .collect()
    }

    #[test]
    fn a_tight_loop_cannot_flood_the_log() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function()\n\
                      for i = 1, 10000 do horizon.log(\"line \" .. i) end\n\
                      end)";
        let found = mod_folder(root.path(), "flood_mod", source, Side::Both);
        let host = LuaHost::new(vec![LuaMod::load(&found).unwrap()]);
        host.fire(HOOK);
        let lines = host.mods[0].log_lines();
        assert!(
            lines.len() >= LOG_BURST as usize,
            "the loop did not run: {} lines",
            lines.len()
        );
        assert!(
            lines.len() <= LOG_BURST as usize + 3,
            "{} lines got through",
            lines.len()
        );
        // A dropped line is not an error: the handler ran to its end.
        assert!(captured_with_prefix("flood_mod: server_tick").is_empty());
    }

    #[test]
    fn the_report_is_emitted_once_at_register() {
        install_capture();
        let host = LuaHost::new(Vec::new()).with_report(vec!["report_mod: nope".into()]);
        host.report();
        host.report();
        // Other tests' reports log their own counts into the same capture: pick this host's line.
        let counts: Vec<String> = captured_with_prefix("Lua mods: ")
            .into_iter()
            .filter(|line| line.ends_with("0 loaded, 1 refused"))
            .collect();
        assert_eq!(counts, ["Lua mods: 0 loaded, 1 refused"]);
        assert_eq!(
            captured_with_prefix("Lua mod refused: "),
            ["Lua mod refused: report_mod: nope"]
        );
    }

    /// A mod `id` that declares `hook` with `options` (a Lua table literal, or "" for none).
    fn ordered_mod(root: &Path, id: &str, hook: &str, options: &str) -> LuaMod {
        let source = if options.is_empty() {
            format!("horizon.on(\"{hook}\", function() end)")
        } else {
            format!("horizon.on(\"{hook}\", function() end, {options})")
        };
        let found = mod_folder(root, id, &source, Side::Both);
        LuaMod::load(&found).unwrap()
    }

    /// The ids of the mods that ran for `HOOK`, in the order they ran.
    fn ran_order(host: &LuaHost) -> Vec<String> {
        let mut ids = Vec::new();
        host.dispatch(
            HOOK,
            &|lua| new_event(lua, lua.create_table()?, &[]),
            &mut |id, _| {
                ids.push(id.to_string());
                true
            },
        );
        ids
    }

    #[test]
    fn mods_run_by_priority_highest_first_and_ties_in_load_order() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            ordered_mod(root.path(), "a", HOOK, "{priority = 0}"),
            ordered_mod(root.path(), "b", HOOK, "{priority = 100}"),
            ordered_mod(root.path(), "c", HOOK, "{priority = 100}"),
        ]);
        assert_eq!(ran_order(&host), ["b", "c", "a"]);
    }

    #[test]
    fn a_negative_priority_runs_last() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            ordered_mod(root.path(), "a", HOOK, "{priority = -5}"),
            ordered_mod(root.path(), "b", HOOK, ""),
            ordered_mod(root.path(), "c", HOOK, "{priority = 1}"),
        ]);
        assert_eq!(ran_order(&host), ["c", "b", "a"]);
    }

    #[test]
    fn a_mod_that_fails_keeps_the_order_and_the_later_mods_run() {
        let root = tempfile::tempdir().unwrap();
        let failing = mod_folder(
            root.path(),
            "a",
            "horizon.on(\"block_receive_game\", function() error(\"boom\") end, {priority = 10})",
            Side::Both,
        );
        let host = LuaHost::new(vec![
            LuaMod::load(&failing).unwrap(),
            ordered_mod(root.path(), "b", HOOK, ""),
        ]);
        assert_eq!(ran_order(&host), ["a", "b"]);
        assert_eq!(host.run_order(HOOK), [0, 1]);
    }

    #[test]
    fn a_mod_with_no_options_for_a_hook_gets_the_defaults() {
        let root = tempfile::tempdir().unwrap();
        let m = ordered_mod(
            root.path(),
            "a",
            HOOK,
            "{priority = 4, receive_cancelled = true}",
        );
        assert_eq!(
            m.options(HOOK),
            ListenerOptions {
                priority: 4,
                receive_cancelled: true
            }
        );
        assert_eq!(m.options("animal_tame"), ListenerOptions::default());
    }

    #[test]
    fn the_game_gets_one_listener_per_subscribed_hook_with_the_max_priority_and_any_receive() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![
            ordered_mod(root.path(), "a", HOOK, "{priority = 3}"),
            ordered_mod(
                root.path(),
                "b",
                HOOK,
                "{priority = 7, receive_cancelled = true}",
            ),
            ordered_mod(root.path(), "c", "animal_tame", "{priority = -2}"),
        ]);
        let subscribed = mod_api::Hooks::subscribed(&host);
        let listeners = mod_api::Hooks::listeners(&host);
        assert_eq!(listeners.len(), subscribed.len());
        assert_eq!(listeners.len(), 2);
        for (listener, hook) in listeners.iter().zip(subscribed) {
            assert_eq!(
                listener.hook.map(|id| id as usize),
                hook.map(|id| id as usize)
            );
        }
        assert_eq!(listeners[0].priority, 7);
        assert!(listeners[0].receive_cancelled);
        assert_eq!(listeners[1].priority, -2);
        assert!(!listeners[1].receive_cancelled);
    }

    #[test]
    fn a_host_with_no_mods_gives_the_game_no_listeners() {
        let host = LuaHost::new(Vec::new());
        assert!(mod_api::Hooks::listeners(&host).is_empty());
    }

    /// The client host the generated `compute_fov` gets: it has no state the Lua hooks read.
    struct NoClientHost;
    impl mod_api::Host for NoClientHost {}
    mod_api::impl_host_areas!(NoClientHost);
    impl mod_api::ClientHost for NoClientHost {}
    mod_api::impl_client_host_areas!(NoClientHost);

    /// A field of view the game is about to use, as the generated `compute_fov` gets it.
    fn fov(degrees: f32) -> mod_api::ComputeFovEvent {
        mod_api::ComputeFovEvent {
            partial: 0.5,
            camera: [0.0, 64.0, 0.0],
            degrees,
        }
    }

    #[test]
    fn a_generated_number_output_is_the_value_the_mods_leave() {
        let a = load_one("horizon.on(\"compute_fov\", function(event) event.degrees = 2.0 end)")
            .unwrap();
        let b = load_one(
            "horizon.on(\"compute_fov\", function(event) event.degrees = event.degrees * 3 end)",
        )
        .unwrap();
        let host = LuaHost::new(vec![a, b]);
        let mut event = fov(1.0);
        mod_api::Hooks::compute_fov(&host, &NoClientHost, &mut event);
        assert_eq!(event.degrees, 6.0);
    }

    #[test]
    fn a_refused_number_keeps_the_game_value_and_the_other_mods_answer() {
        let a =
            load_one("horizon.on(\"compute_fov\", function(event) event.degrees = \"wide\" end)")
                .unwrap();
        let b = load_one("horizon.on(\"compute_fov\", function(event) event.degrees = 90.0 end)")
            .unwrap();
        let host = LuaHost::new(vec![a, b]);
        let mut event = fov(70.0);
        mod_api::Hooks::compute_fov(&host, &NoClientHost, &mut event);
        assert_eq!(event.degrees, 90.0);
    }

    #[test]
    fn a_cancelled_event_is_uncancelled_by_a_receiving_mod_and_the_flow_continues() {
        let root = tempfile::tempdir().unwrap();
        let cancel =
            "horizon.on(\"block_receive_game\", function(event) event.cancelled = true end)";
        let plain = "horizon.on(\"block_receive_game\", function() horizon.log(\"plain\") end)";
        let receive = "horizon.on(\"block_receive_game\", function(event)\n\
                       horizon.log(tostring(event.cancelled))\n\
                       event.cancelled = false\n\
                       end, {receive_cancelled = true})";
        let host = LuaHost::new(vec![
            LuaMod::load(&mod_folder(root.path(), "mod_a", cancel, Side::Both)).unwrap(),
            LuaMod::load(&mod_folder(root.path(), "mod_b", plain, Side::Both)).unwrap(),
            LuaMod::load(&mod_folder(root.path(), "mod_c", receive, Side::Both)).unwrap(),
        ]);
        let mut event = received(false);
        let flow = mod_api::Hooks::block_receive_game(&host, &mut NoHost, &mut event);
        assert!(matches!(flow, mod_api::Flow::Continue));
        assert!(!event.cancelled);
        assert!(host.mods()[1].log_lines().is_empty());
        assert_eq!(host.mods()[2].log_lines(), ["true"]);
    }

    #[test]
    fn a_failing_turn_that_cancelled_does_not_cancel() {
        let host = LuaHost::new(vec![
            load_one(
                "horizon.on(\"block_receive_game\", function(event)\n\
                      event.cancelled = true\n\
                      error(\"boom\")\n\
                      end)",
            )
            .unwrap(),
        ]);
        let mut event = received(false);
        let flow = mod_api::Hooks::block_receive_game(&host, &mut NoHost, &mut event);
        assert!(matches!(flow, mod_api::Flow::Continue));
        assert!(!event.cancelled);
    }
}

/// The generated BlockEntityPart methods' turns, run on a loaded part with no host: the handler sees
/// the converted argument, another mod's handler does not run, and an error voids the turn.
#[cfg(test)]
mod block_entity_turn_tests {
    use super::*;
    use crate::host::tests::{captured_with_prefix, install_capture};
    use crate::hostcall::TurnHost;
    use event_horizon_lua_convert::scalar::ToLuaValue;

    /// A mod named `name` with `source` as its main.luau, loaded.
    fn load(root: &Path, name: &str, source: &str) -> LuaMod {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), source).unwrap();
        let found = Discovered {
            dir,
            id: name.to_string(),
            version: "1".to_string(),
            side: Side::Both,
        };
        LuaMod::load(&found).unwrap()
    }

    /// A block and a block entity type `ns:press` on it, with one part whose handlers are `handlers`.
    fn source(ns: &str, handlers: &str) -> String {
        format!(
            "horizon.block{{id = \"{ns}:press_block\", template = \"minecraft:stone\"}}\n\
             horizon.block_entity_type{{id = \"{ns}:press\", blocks = {{\"{ns}:press_block\"}}}}\n\
             horizon.block_entity_part(\"{ns}:press\", {{{handlers}}})\n"
        )
    }

    /// The block entity part of the mod at `index` (each mod here declares one).
    fn part(mods: &Arc<[LuaMod]>, index: usize) -> LuaBlockEntityPart {
        LuaPart::new(mods.clone(), index, 0).unwrap()
    }

    fn event(lua: &Lua, data: Table) -> mlua::Result<crate::dispatch::Event> {
        new_event(lua, data, &[])
    }

    #[test]
    fn a_block_entity_on_load_handler_sees_the_id_and_no_other_mod_runs() {
        let root = tempfile::tempdir().unwrap();
        let mods: Arc<[LuaMod]> = Arc::from(vec![
            load(
                root.path(),
                "first_mod",
                &source(
                    "first_mod",
                    "block_entity_on_load = function(event) horizon.log(event.id) end",
                ),
            ),
            load(
                root.path(),
                "second_mod",
                &source(
                    "second_mod",
                    "block_entity_on_load = function() horizon.log(\"second ran\") end",
                ),
            ),
        ]);
        let build = |lua: &Lua, _: &TurnHost<'_>| {
            let data = lua.create_table()?;
            data.raw_set("id", "first_mod:press".to_owned().to_lua_value(lua)?)?;
            event(lua, data)
        };
        let mut committed = 0;
        part(&mods, 0)
            .dispatch_turns(
                "block_entity_on_load",
                TurnHost::None,
                &|| false,
                &build,
                &mut |_, _, _| {
                    committed += 1;
                    true
                },
            )
            .unwrap();
        assert_eq!(committed, 1);
        assert_eq!(mods[0].log_lines(), ["first_mod:press"]);
        assert!(mods[1].log_lines().is_empty());
    }

    #[test]
    fn a_failing_block_entity_on_load_handler_voids_its_turn() {
        let root = tempfile::tempdir().unwrap();
        let mods: Arc<[LuaMod]> = Arc::from(vec![load(
            root.path(),
            "first_mod",
            &source(
                "first_mod",
                "block_entity_on_load = function() error(\"boom\") end",
            ),
        )]);
        let build = |lua: &Lua, _: &TurnHost<'_>| event(lua, lua.create_table()?);
        let mut committed = 0;
        part(&mods, 0)
            .dispatch_turns(
                "block_entity_on_load",
                TurnHost::None,
                &|| false,
                &build,
                &mut |_, _, _| {
                    committed += 1;
                    true
                },
            )
            .unwrap();
        assert_eq!(committed, 0);
    }

    #[test]
    fn a_screen_click_handler_sees_the_slice_as_an_array_and_its_answer_is_taken() {
        let root = tempfile::tempdir().unwrap();
        let mods: Arc<[LuaMod]> = Arc::from(vec![
            load(
                root.path(),
                "first_mod",
                &source(
                    "first_mod",
                    "screen_click = function(event)\n\
                     if #event.data == 3 and event.data[1] == 1 and event.data[3] == 3 then return 7 end\n\
                     end",
                ),
            ),
            load(
                root.path(),
                "second_mod",
                &source(
                    "second_mod",
                    "screen_click = function() horizon.log(\"second ran\") return 9 end",
                ),
            ),
        ]);
        let build = |lua: &Lua, _: &TurnHost<'_>| {
            let data = lua.create_table()?;
            data.raw_set("data", [1_i32, 2, 3].as_slice().to_lua_value(lua)?)?;
            event(lua, data)
        };
        let mut answer = None;
        part(&mods, 0)
            .dispatch_replace(
                "screen_click",
                TurnHost::None,
                &build,
                &mut |_, value, _| {
                    match value {
                        Value::Integer(found) => answer = Some(found),
                        Value::Number(found) => answer = Some(found as i64),
                        _ => {}
                    }
                    true
                },
            )
            .unwrap();
        assert_eq!(answer, Some(7));
        assert!(mods[1].log_lines().is_empty());
    }

    #[test]
    fn a_panic_in_a_part_build_ends_the_part_turn_for_both_hook_kinds() {
        install_capture();
        let root = tempfile::tempdir().unwrap();
        let mods: Arc<[LuaMod]> = Arc::from(vec![load(
            root.path(),
            "panic_part",
            &source(
                "panic_part",
                "block_entity_on_load = function() horizon.log(\"ran\") end,\n\
                 screen_click = function() return 1 end",
            ),
        )]);
        let build = |_: &Lua, _: &TurnHost<'_>| -> mlua::Result<crate::dispatch::Event> {
            panic!("the part build broke")
        };
        let mut committed = 0;
        part(&mods, 0)
            .dispatch_turns(
                "block_entity_on_load",
                TurnHost::None,
                &|| false,
                &build,
                &mut |_, _, _| {
                    committed += 1;
                    true
                },
            )
            .unwrap();
        assert_eq!(committed, 0);
        assert!(mods[0].log_lines().is_empty());
        assert_eq!(
            captured_with_prefix("panic_part: block_entity_on_load: the game's host panicked"),
            [
                "panic_part: block_entity_on_load: the game's host panicked: the part build broke (this mod no longer receives this hook)"
            ]
        );
        let mut answered = 0;
        part(&mods, 0)
            .dispatch_replace("screen_click", TurnHost::None, &build, &mut |_, _, _| {
                answered += 1;
                true
            })
            .unwrap();
        assert_eq!(answered, 0);
        assert_eq!(
            captured_with_prefix("panic_part: screen_click: the game's host panicked"),
            [
                "panic_part: screen_click: the game's host panicked: the part build broke (this mod no longer receives this hook)"
            ]
        );
        assert!(!mods[0].turn().held_here());
    }

    #[test]
    fn a_failing_screen_click_handler_gives_no_answer() {
        let root = tempfile::tempdir().unwrap();
        let mods: Arc<[LuaMod]> = Arc::from(vec![load(
            root.path(),
            "first_mod",
            &source("first_mod", "screen_click = function() error(\"boom\") end"),
        )]);
        let build = |lua: &Lua, _: &TurnHost<'_>| event(lua, lua.create_table()?);
        let mut answered = false;
        part(&mods, 0)
            .dispatch_replace("screen_click", TurnHost::None, &build, &mut |_, _, _| {
                answered = true;
                true
            })
            .unwrap();
        assert!(!answered);
    }
}

#[cfg(test)]
mod warn_once_tests {
    use super::*;

    #[test]
    fn a_repeated_line_is_logged_once() {
        let mut warned = Warned::default();
        let line = "mod_a: compute_fov: output `fov` refused, the previous value is kept: bad";
        let logged: Vec<String> = (0..50).filter_map(|_| warned.once("mod_a", line)).collect();
        assert_eq!(logged, [line]);
    }

    #[test]
    fn two_mods_or_two_reasons_are_two_lines() {
        let mut warned = Warned::default();
        assert!(warned.once("mod_a", "mod_a: hook: refused, x").is_some());
        assert!(warned.once("mod_b", "mod_b: hook: refused, x").is_some());
        assert!(warned.once("mod_a", "mod_a: hook: refused, y").is_some());
        assert!(warned.once("mod_a", "mod_a: hook: refused, x").is_none());
    }

    #[test]
    fn a_mod_with_100_distinct_errors_logs_16_and_one_suppression_line() {
        let mut warned = Warned::default();
        let logged: Vec<String> = (0..100)
            .filter_map(|i| warned.once("mod_a", &format!("mod_a: hook: error {i}")))
            .collect();
        assert_eq!(logged.len(), MAX_LINES_PER_SOURCE + 1);
        assert_eq!(logged[0], "mod_a: hook: error 0");
        assert_eq!(
            logged[MAX_LINES_PER_SOURCE],
            "mod_a: further warnings suppressed"
        );
        assert_eq!(
            warned.sources["mod_a"].suppressed,
            100 - MAX_LINES_PER_SOURCE as u64
        );
    }

    #[test]
    fn a_second_mod_still_logs_after_the_first_is_exhausted() {
        let mut warned = Warned::default();
        for i in 0..100 {
            warned.once("mod_a", &format!("mod_a: hook: error {i}"));
        }
        assert!(warned.once("mod_a", "mod_a: hook: error 500").is_none());
        assert_eq!(
            warned.once("mod_b", "mod_b: hook: first"),
            Some("mod_b: hook: first".to_owned())
        );
    }

    #[test]
    fn the_global_cap_of_sources_holds_and_one_global_line_is_logged() {
        let mut warned = Warned::default();
        let logged: Vec<String> = (0..300)
            .filter_map(|i| warned.once(&format!("mod_{i}"), &format!("mod_{i}: hook: reason")))
            .collect();
        assert_eq!(logged.len(), MAX_SOURCES + 1);
        assert_eq!(
            logged[MAX_SOURCES],
            "further warnings suppressed: more than 256 sources this process"
        );
        assert_eq!(warned.sources.len(), MAX_SOURCES);
        assert_eq!(warned.dropped_sources, 300 - MAX_SOURCES as u64);
        assert!(warned.once("mod_0", "mod_0: hook: reason").is_none());
    }
}

#[cfg(test)]
mod load_notice_tests {
    use super::tests::{captured_with_prefix, install_capture};
    use super::*;
    use crate::data_maps::MAX_JSON_BYTES;
    use crate::discover::{Discovered, ENTRY, Side};

    fn load(root: &Path, id: &str, source: &str) -> LuaMod {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), source).unwrap();
        LuaMod::load(&Discovered {
            dir,
            id: id.to_owned(),
            version: "1".to_owned(),
            side: Side::Server,
        })
        .unwrap()
    }

    #[test]
    fn a_declaration_refusal_is_kept_at_load_and_logged_once_by_the_report() {
        install_capture();
        let root = tempfile::tempdir().unwrap();
        let source = format!(
            "horizon.data_pack_file{{ key = \"data_maps/ln_big:item/big\", json = \"[\" .. string.rep(\" \", {}) .. \"]\" }}",
            MAX_JSON_BYTES
        );
        let host = LuaHost::new(vec![load(root.path(), "ln_big", &source)]);
        assert!(
            captured_with_prefix("ln_big: ").is_empty(),
            "a load logs nothing: the logger is not up yet"
        );
        host.report();
        host.report();
        let expected = format!(
            "ln_big: data pack file 'data_maps/ln_big:item/big' refused: its JSON is {} bytes, more than {MAX_JSON_BYTES}",
            MAX_JSON_BYTES + 2
        );
        assert_eq!(captured_with_prefix("ln_big: "), [expected]);
    }

    #[test]
    fn a_command_without_a_handler_is_warned_by_the_report_not_by_the_load() {
        install_capture();
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![load(
            root.path(),
            "ln_cmd",
            "horizon.command{usage = \"ln_cmd run\"}\n",
        )]);
        assert!(captured_with_prefix("ln_cmd: ").is_empty());
        host.report();
        assert_eq!(
            captured_with_prefix("ln_cmd: "),
            [
                "ln_cmd: declares commands but has no run_mod_command handler; they will fail when run"
            ]
        );
    }
}
