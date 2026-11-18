//! Host calls from a Lua handler (issue 74): a handle that reaches the host of the hook call that is
//! running, with no `'static` host and no `unsafe`.
//!
//! A hook hands its host to the dispatch functions as a [`TurnHost`]: the real `mod_api` host kind
//! its method was given (`ServerHost`, `ClientHost`, `Host`), or [`TurnHost::None`] for a hook that
//! gets none. For each mod's turn, [`scoped`] opens a `Lua::scope` and creates one scoped Lua
//! function that closes over that host. A scope may borrow for one call only, so the host never has
//! to be `'static`. The function is stored in the Lua registry under [`SLOT`] and cleared before the
//! scope ends, so a handle kept from an earlier fire finds no slot and raises
//! `no host call outside a hook`.
//!
//! A handle's method is an ordinary `'static` callback: it finds the slot and calls it with the
//! operation's name. The scoped function charges the turn's budget and runs the operation against
//! the turn's host, which must be of the kind the operation needs: a read needs [`TurnHost::read`],
//! a write needs [`TurnHost::server`]. A turn without that kind raises `not available in this hook`.
//! A `None` turn opens a scope too, with no host: its calls raise `not available in this hook`,
//! and a call outside any hook raises `no host call outside a hook`.
//!
//! The operations are the generated handle methods ([`crate::generated::handles`]): `run` converts
//! the arguments, calls the game's method through the turn's host, and converts the result. A
//! handle's userdata passes itself as the first argument, so the operation finds the handle it
//! names. The H0 stub's operations exist under `cfg(test)` only.
//!
//! Re-entrancy: a host call may fire a hook again on this thread. A mod whose turn is open is
//! skipped there (one log line per mod per top-level call), the other mods run, and the nesting is
//! limited to [`MAX_DEPTH`] hook calls open at once. A call past that is refused to its caller.
//!
//! Budget: each turn may spend [`TURN_BUDGET`] units. A read costs 1, a write 10 and a heavy read
//! 100. A call past the budget raises, and the budget stays spent for the rest of the turn, so a
//! `pcall` cannot keep the turn going. The turn is then void, as a failing handler's is. A write is
//! not rolled back by a void turn: the host keeps it.

use std::cell::{Cell, RefCell};

use event_horizon_lua_convert::scalar::ToLuaValue;
use mlua::{AnyUserData, Error, Lua, MultiValue, Table, Value};
use mod_api::{BlockClick, ClientHost, Host, HostSide, ServerHost};

use crate::host::{LOG_TARGET, LuaMod};

/// A right click as a Lua handler reads it (`click` in `use_without_item` and `use_item_on`): its
/// side, dimension, block position, hit, hand, player, the held stack (`stack`, already converted by
/// the generated code: nil for an empty hand) and the player's entity id.
pub(crate) fn block_click_lua(
    lua: &Lua,
    click: &BlockClick<'_>,
    stack: Value,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set("side", click.side.to_lua_value(lua)?)?;
    table.raw_set("dimension", click.dimension.to_lua_value(lua)?)?;
    table.raw_set("pos", click.pos.to_lua_value(lua)?)?;
    table.raw_set("hit", click.hit.to_lua_value(lua)?)?;
    table.raw_set("hand", click.hand.to_lua_value(lua)?)?;
    table.raw_set("stack", stack)?;
    table.raw_set("player", click.player.to_lua_value(lua)?)?;
    table.raw_set("player_entity_id", click.player_entity_id)?;
    Ok(table)
}

/// The units one turn may spend on host calls.
pub const TURN_BUDGET: u32 = 10_000;
/// The hook calls that may be open at once on one thread. A call past this is refused.
pub const MAX_DEPTH: u32 = 8;
/// The Lua registry key of the turn's scoped function. Only [`scoped`] sets it, and only in a turn.
const SLOT: &str = "event_horizon.host_call";
/// Raised when a handle is called with no turn open.
pub const OUTSIDE: &str = "no host call outside a hook";
/// Raised when the turn's host is not of the kind the operation needs.
pub const NOT_HERE: &str = "not available in this hook";
const SPENT: &str = "host call budget is spent for this turn";

/// The host a hook gave for one call, by the kind its method takes.
pub enum TurnHost<'a> {
    /// A hook that fired on the server: writes and reads.
    Server(&'a mut dyn ServerHost),
    /// A hook that fired on the client: the client's services and reads.
    Client(&'a mut dyn ClientHost),
    /// A read-only client hook.
    ClientRead(&'a dyn ClientHost),
    /// A read-only hook.
    Read(&'a dyn Host),
    /// A hook that gets no host. Its handles raise `no host call outside a hook`.
    None,
}

impl<'a> TurnHost<'a> {
    /// The turn of a hook that is given a [`HostSide`]: the side it fired on.
    pub fn of_side(side: HostSide<'a>) -> TurnHost<'a> {
        match side {
            HostSide::Server(host) => TurnHost::Server(host),
            HostSide::Client(host) => TurnHost::Client(host),
        }
    }

    /// The host for a read: every kind but `None` has one (a server or client host upcast to `Host`).
    pub fn read(&self) -> Option<&dyn Host> {
        match self {
            TurnHost::Server(host) => Some(&**host as &dyn Host),
            TurnHost::Client(host) => Some(&**host as &dyn Host),
            TurnHost::ClientRead(host) => Some(*host as &dyn Host),
            TurnHost::Read(host) => Some(*host),
            TurnHost::None => None,
        }
    }

    /// [`TurnHost::read`], or a host that refuses every read. The generated hooks convert their
    /// carried values through this inside their closures; a hook that has a host always has a read.
    pub fn read_or_unavailable(&self) -> &dyn Host {
        self.read().unwrap_or(&Unavailable)
    }

    /// The host for a write, when the turn's hook is a server hook.
    pub fn server(&mut self) -> Option<&mut dyn ServerHost> {
        match self {
            TurnHost::Server(host) => Some(&mut **host),
            _ => None,
        }
    }

    /// The host for the client's services, when the turn's hook is a writable client hook.
    pub fn client(&mut self) -> Option<&mut dyn ClientHost> {
        match self {
            TurnHost::Client(host) => Some(&mut **host),
            _ => None,
        }
    }

    /// The same host for one mod's turn, borrowed again so each mod gets its own scope.
    fn reborrow(&mut self) -> TurnHost<'_> {
        match self {
            TurnHost::Server(host) => TurnHost::Server(&mut **host),
            TurnHost::Client(host) => TurnHost::Client(&mut **host),
            TurnHost::ClientRead(host) => TurnHost::ClientRead(*host),
            TurnHost::Read(host) => TurnHost::Read(*host),
            TurnHost::None => TurnHost::None,
        }
    }
}

/// The host of a turn that has none: every read is refused, as a stale handle's is.
struct Unavailable;
mod_api::impl_host_areas!(Unavailable);
impl Host for Unavailable {}

thread_local! {
    /// The hook calls open on this thread.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// The mods skipped for being open, in the current top-level hook call.
    static SKIPPED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Lets go of one open hook call when it ends, also when it panics.
pub(crate) struct OpenCall;

impl Drop for OpenCall {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

/// Opens one hook call on this thread. Refused, with the reason, past [`MAX_DEPTH`].
pub(crate) fn open_call(hook: &str) -> Result<OpenCall, String> {
    let depth = DEPTH.with(Cell::get);
    if depth >= MAX_DEPTH {
        return Err(format!(
            "{hook} is refused: hook calls are nested more than {MAX_DEPTH} deep"
        ));
    }
    if depth == 0 {
        SKIPPED.with_borrow_mut(Vec::clear);
    }
    DEPTH.with(|open| open.set(depth + 1));
    Ok(OpenCall)
}

/// Logs a hook call that was refused, so the hook's mods do not run. The generated hooks call this
/// on the result of their dispatch.
pub fn refused(result: Result<(), String>) {
    if let Err(err) = result {
        log::warn!(target: LOG_TARGET, "{err}");
    }
}

/// Logs that a mod was skipped because its turn is open, once per mod per top-level hook call.
pub(crate) fn note_skipped(id: &str, hook: &str) {
    let first = SKIPPED.with_borrow_mut(|seen| {
        if seen.iter().any(|name| name == id) {
            false
        } else {
            seen.push(id.to_owned());
            true
        }
    });
    if first {
        log::warn!(
            target: LOG_TARGET,
            "{id}: {hook}: skipped, this mod's turn is already open"
        );
    }
}

/// Runs `body` for one mod's turn with `host` open to its handles. A scope holds the scoped
/// function under [`SLOT`] while `body` runs, for every turn, the `None` turn included (its
/// operations find no host). Returns what `body` returned and whether the turn's budget was spent,
/// which voids the turn even when a `pcall` hid the raise. An error from the scope itself is
/// returned as is.
pub(crate) fn scoped<R>(
    lua: &Lua,
    host: &mut TurnHost<'_>,
    body: impl FnOnce(&Lua) -> R,
) -> mlua::Result<(R, bool)> {
    let mut turn_host = host.reborrow();
    let spent_cell = Cell::new(false);
    let spent_out = &spent_cell;
    let out = lua.scope(|scope| {
        let mut spent = 0u32;
        let calls = scope.create_function_mut(
            move |lua: &Lua, (op, this, args): (String, AnyUserData, MultiValue)| {
                if spent_out.get() {
                    return Err(Error::runtime(SPENT));
                }
                let cost = op_cost(&op)?;
                if spent + cost > TURN_BUDGET {
                    spent_out.set(true);
                    return Err(Error::runtime(SPENT));
                }
                spent += cost;
                run_op(lua, &mut turn_host, &op, this, args)
            },
        )?;
        lua.set_named_registry_value(SLOT, calls)?;
        let out = body(lua);
        // The slot goes before the scope ends, so nothing finds the function once it is invalid.
        let _ = lua.set_named_registry_value(SLOT, Value::Nil);
        Ok(out)
    })?;
    Ok((out, spent_cell.get()))
}

/// Runs one mod's handlers in its turn with `host` open. Returns whether the turn failed: a handler
/// raised, the scope failed, or the budget ran out.
pub(crate) fn run_hosted(
    m: &LuaMod,
    hook: &str,
    host: &mut TurnHost<'_>,
    handlers: &[mlua::Function],
    event: &mlua::Table,
) -> bool {
    let outcome = scoped(m.lua(), host, |lua| {
        let mut failed = false;
        for handler in handlers {
            if let Err(err) = crate::host::call_handler(lua, handler, event) {
                log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                failed = true;
            }
        }
        failed
    });
    match outcome {
        Ok((failed, spent)) => failed || spent,
        Err(err) => {
            log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
            true
        }
    }
}

/// The handle's side of a host call: find the turn's scoped function and call it with the
/// operation's name, the handle's userdata (its first argument) and the rest of the arguments.
/// Outside a hook there is no scoped function.
pub fn host_call(
    lua: &Lua,
    op: &'static str,
    this: AnyUserData,
    args: MultiValue,
) -> mlua::Result<Value> {
    let slot: Option<mlua::Function> = lua.named_registry_value(SLOT)?;
    match slot {
        Some(slot) => slot.call((op, this, args)),
        None => Err(Error::runtime(OUTSIDE)),
    }
}

fn unknown(op: &str) -> Error {
    Error::runtime(format!("unknown host call {op}"))
}

/// What an operation costs, from the generated table of the handle families.
#[cfg(not(test))]
fn op_cost(op: &str) -> mlua::Result<u32> {
    crate::generated::handles::cost(op).ok_or_else(|| unknown(op))
}

/// Runs an operation through the generated table.
#[cfg(not(test))]
fn run_op(
    lua: &Lua,
    host: &mut TurnHost<'_>,
    op: &str,
    this: AnyUserData,
    args: MultiValue,
) -> mlua::Result<Value> {
    crate::generated::handles::run(lua, host, op, &this, args).unwrap_or_else(|| Err(unknown(op)))
}

/// The H0 stub's table first, then the generated one: the stub's names are not generated names.
#[cfg(test)]
fn op_cost(op: &str) -> mlua::Result<u32> {
    match stub::cost(op) {
        Some(cost) => Ok(cost),
        None => crate::generated::handles::cost(op).ok_or_else(|| unknown(op)),
    }
}

#[cfg(test)]
fn run_op(
    lua: &Lua,
    host: &mut TurnHost<'_>,
    op: &str,
    this: AnyUserData,
    args: MultiValue,
) -> mlua::Result<Value> {
    match stub::run(lua, host, op, &this, args.clone()) {
        Some(out) => out,
        None => crate::generated::handles::run(lua, host, op, &this, args)
            .unwrap_or_else(|| Err(unknown(op))),
    }
}

#[cfg(test)]
mod stub {
    //! The H0 stub: a handle `Entity` with `position`, `heavy` and `set_velocity`, which reach the
    //! real `EntityRef` methods through the turn's host. Test-only: the handles are the generated
    //! ones, and these names are not theirs.

    use event_horizon_lua_convert::scalar::FromLuaValue;
    use mlua::{AnyUserData, Error, Lua, MultiValue, UserData, UserDataMethods, Value};
    use mod_api::{EntityRef, WorldEpoch};

    use super::{NOT_HERE, TurnHost, host_call};

    const READ: u32 = 1;
    const WRITE: u32 = 10;
    const HEAVY: u32 = 100;

    /// A handle to a world object, by the id and epoch the host gave it. A copy of two numbers.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Entity {
        pub id: u32,
        pub epoch: u32,
    }

    impl UserData for Entity {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("id", |_, this, ()| Ok(this.id));
            methods.add_function(
                "position",
                |lua, (this, args): (AnyUserData, MultiValue)| {
                    host_call(lua, "position", this, args)
                },
            );
            methods.add_function("heavy", |lua, (this, args): (AnyUserData, MultiValue)| {
                host_call(lua, "heavy", this, args)
            });
            methods.add_function(
                "set_velocity",
                |lua, (this, args): (AnyUserData, MultiValue)| {
                    host_call(lua, "set_velocity", this, args)
                },
            );
        }
    }

    pub fn cost(op: &str) -> Option<u32> {
        match op {
            "position" => Some(READ),
            "heavy" => Some(HEAVY),
            "set_velocity" => Some(WRITE),
            _ => None,
        }
    }

    /// Runs one stub operation against the turn's host, or `None` for a name the stub does not
    /// have. Each operation asks for the host kind it needs, so a read-only turn refuses a write
    /// with the same error a `None` turn gives a read.
    pub fn run(
        lua: &Lua,
        host: &mut TurnHost<'_>,
        op: &str,
        this: &AnyUserData,
        args: MultiValue,
    ) -> Option<mlua::Result<Value>> {
        cost(op)?;
        let entity = match this.borrow::<Entity>() {
            Ok(entity) => *entity,
            Err(err) => return Some(Err(err)),
        };
        let handle = EntityRef::new(WorldEpoch::new(u64::from(entity.epoch)), entity.id);
        let arg = args.into_iter().next().unwrap_or(Value::Nil);
        Some(match op {
            "position" | "heavy" => {
                let Some(read) = host.read() else {
                    return Some(Err(Error::runtime(NOT_HERE)));
                };
                match EntityRef::position(handle, read) {
                    Some([x, y, z]) => {
                        let point = lua.create_table().and_then(|point| {
                            point.raw_set("x", x)?;
                            point.raw_set("y", y)?;
                            point.raw_set("z", z)?;
                            Ok(point)
                        });
                        point.map(Value::Table)
                    }
                    None => Ok(Value::Nil),
                }
            }
            "set_velocity" => {
                let Some(server) = host.server() else {
                    return Some(Err(Error::runtime(NOT_HERE)));
                };
                match vector(&arg) {
                    Ok(velocity) => Ok(Value::Boolean(handle.set_velocity(server, velocity))),
                    Err(err) => Err(err),
                }
            }
            _ => return None,
        })
    }

    /// Reads `{x, y, z}` strictly: each axis must be a finite number. The error names the argument.
    fn vector(value: &Value) -> mlua::Result<[f64; 3]> {
        let bad =
            |why: String| Error::runtime(format!("set_velocity: argument 1 (velocity) {why}"));
        let Value::Table(table) = value else {
            return Err(bad(format!(
                "must be a table {{x, y, z}}, not {}",
                value.type_name()
            )));
        };
        let axis = |name: &str| -> mlua::Result<f64> {
            let found: Value = table.get(name)?;
            f64::from_lua_value(&found).map_err(|err| bad(format!("field {name}: {err}")))
        };
        Ok([axis("x")?, axis("y")?, axis("z")?])
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use mod_api::__private::game::{BackendMutRefOf, BackendRefOf};
    use mod_api::prelude::{EntitiesBackend, EntitiesBackendMut, LevelsBackend};
    use mod_api::{
        Backend, BackendMut, BackendMutRef, BackendRef, BlockState, ChunkRef, ClientHost,
        DimensionKey, DisplayRef, DragonRef, EntityRef, FishingHookRef, Host, ItemEntityRef,
        LevelRef, LivingRef, MerchantRef, MinecartRef, MobRef, ProjectileRef, RaidRef, ServerHost,
        WorldEpoch,
    };

    use crate::handles::Token;

    use super::stub::Entity;
    use super::*;
    use crate::declare;
    use crate::discover::{Discovered, ENTRY, Side};
    use crate::dispatch::{Event, new_event};
    use crate::host::LuaHost;
    use crate::host::tests::{captured_with_prefix, install_capture};

    const HOOK: &str = "block_receive_game";
    const ME: Entity = Entity { id: 1, epoch: 1 };
    /// The same id with an older epoch: a stale handle, which the host does not know.
    const STRANGER: Entity = Entity { id: 1, epoch: 0 };

    /// The `Host` of a test world. Each host kind a test needs is a macro of the real traits: the
    /// generated `impl_*_areas!` give every area its defaults, and only the entity's backend is
    /// written here, as the game writes it.
    macro_rules! host_double {
        ($ty:ty) => {
            impl Host for $ty {
                fn backend(&self) -> Option<BackendRef<'_>> {
                    Some(BackendRefOf::of(self))
                }
            }
            impl Backend for $ty {}
            mod_api::impl_host_areas!($ty);
            mod_api::impl_backend_areas!($ty, except(entities, levels));
            impl ServerHost for $ty {
                fn backend_mut(&mut self) -> Option<BackendMutRef<'_>> {
                    Some(BackendMutRefOf::of(self))
                }
            }
            impl BackendMut for $ty {}
            mod_api::impl_server_host_areas!($ty);
            mod_api::impl_backend_mut_areas!($ty, except(entities));
            impl ClientHost for $ty {}
            mod_api::impl_client_host_areas!($ty);
        };
    }

    /// Whether `entity` is the test world's `ME`, by id and epoch.
    fn is_me(entity: EntityRef) -> bool {
        entity.id() == ME.id && entity.epoch().get() == u64::from(ME.epoch)
    }

    /// A host that knows `ME` at `at`, counts its writes and keeps the last velocity. `calls` counts
    /// every read the double answers, so a test can see that a refused call never reached it.
    struct Fake {
        at: [f64; 3],
        velocity: Option<[f64; 3]>,
        glowing: Option<bool>,
        writes: u32,
        calls: Cell<u32>,
    }

    impl Fake {
        fn at(at: [f64; 3]) -> Fake {
            Fake {
                at,
                velocity: None,
                glowing: None,
                writes: 0,
                calls: Cell::new(0),
            }
        }

        fn counted(&self) {
            self.calls.set(self.calls.get() + 1);
        }
    }

    host_double!(Fake);

    impl EntitiesBackend for Fake {
        fn entity_position(&self, entity: EntityRef) -> Option<[f64; 3]> {
            self.counted();
            is_me(entity).then_some(self.at)
        }
        fn entity_is_in_water(&self, entity: EntityRef) -> Option<bool> {
            self.counted();
            is_me(entity).then_some(true)
        }
        fn entity_passengers(&self, entity: EntityRef) -> Option<Vec<EntityRef>> {
            self.counted();
            is_me(entity).then(|| vec![me()])
        }
        fn entity_level(&self, entity: EntityRef) -> Option<LevelRef> {
            self.counted();
            is_me(entity).then(level)
        }
    }

    impl LevelsBackend for Fake {
        /// `None` for every position: a block state needs the game's block registry to build.
        fn level_block_state(&self, _: LevelRef, _: [i32; 3]) -> Option<BlockState> {
            self.counted();
            None
        }
    }

    /// The test world's `ME` as a handle value.
    fn me() -> EntityRef {
        EntityRef::new(WorldEpoch::new(u64::from(ME.epoch)), ME.id)
    }

    /// The test world's overworld, as a handle value.
    fn level() -> LevelRef {
        LevelRef::new(WorldEpoch::new(1), DimensionKey::OVERWORLD)
    }

    impl EntitiesBackendMut for Fake {
        fn entity_set_velocity(&mut self, entity: EntityRef, velocity: [f64; 3]) -> bool {
            self.writes += 1;
            if !is_me(entity) {
                return false;
            }
            self.velocity = Some(velocity);
            true
        }
        fn entity_set_glowing(&mut self, entity: EntityRef, glowing: bool) -> bool {
            self.writes += 1;
            if !is_me(entity) {
                return false;
            }
            self.glowing = Some(glowing);
            true
        }
        fn entity_leash_to(&mut self, _: EntityRef, _: EntityRef) -> bool {
            self.writes += 1;
            true
        }
    }

    /// Writes `source` as mod `id` and loads it.
    fn load(root: &Path, id: &str, source: &str) -> LuaMod {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ENTRY), source).unwrap();
        LuaMod::load(&Discovered {
            dir,
            id: id.to_string(),
            version: "1".to_string(),
            side: Side::Both,
        })
        .unwrap()
    }

    /// The event for one fire: the handles, and `n`, a count the mod's source reads.
    fn build(n: u32) -> impl Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event> {
        move |lua: &Lua, _: &TurnHost<'_>| {
            let data = lua.create_table()?;
            data.raw_set("entity", lua.create_userdata(ME)?)?;
            data.raw_set("stranger", lua.create_userdata(STRANGER)?)?;
            data.raw_set("level", lua.create_userdata(Token(level()))?)?;
            data.raw_set("n", n)?;
            new_event(lua, data, &["cancelled", "result"])
        }
    }

    /// Fires `HOOK` with `turn` and `n`; returns how many turns committed.
    fn fire(host: &LuaHost, turn: TurnHost<'_>, n: u32) -> u32 {
        let mut commits = 0;
        host.dispatch_turns(HOOK, turn, &|| false, &build(n), &mut |_, _, _| {
            commits += 1;
            true
        })
        .unwrap();
        commits
    }

    /// Runs `f` on its own thread; panics if it does not finish, so a deadlock fails the test.
    fn within_time<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(60))
            .expect("the hook call did not finish: deadlock?")
    }

    #[test]
    fn a_read_returns_the_position_as_a_table_and_a_stale_handle_nil() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_read",
            "horizon.on(\"block_receive_game\", function(event)\n\
               local p = event.entity:position()\n\
               horizon.log(type(p) .. \" \" .. tostring(p.x) .. \" \" .. tostring(p.y) .. \" \" .. tostring(p.z))\n\
               horizon.log(tostring(event.stranger:position()))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        fire(&host, TurnHost::Read(&Fake::at([1.0, 2.0, 3.0])), 0);
        assert_eq!(host.mods()[0].log_lines(), ["table 1 2 3", "nil"]);
    }

    #[test]
    fn a_write_reaches_the_host_with_the_exact_vector() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_write",
            "horizon.on(\"block_receive_game\", function(event)\n\
               horizon.log(tostring(event.entity:set_velocity({x = 0.5, y = -1, z = 2})))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let mut fake = Fake::at([0.0; 3]);
        fire(&host, TurnHost::Server(&mut fake), 0);
        assert_eq!(fake.velocity, Some([0.5, -1.0, 2.0]));
        assert_eq!(host.mods()[0].log_lines(), ["true"]);
    }

    /// Each turn host kind, with the handle's read and write under `pcall`: a read needs a host
    /// (a `None` turn has none), a write needs a server host.
    #[test]
    fn the_turn_host_kind_decides_what_a_handle_may_do() {
        let root = tempfile::tempdir().unwrap();
        let source = "horizon.on(\"block_receive_game\", function(event)\n\
               local ok, p = pcall(function() return event.entity:position() end)\n\
               horizon.log(tostring(ok) .. \" \" .. (ok and tostring(p.x) or tostring(p)))\n\
               local wrote, w = pcall(function() return event.entity:set_velocity({x = 1, y = 1, z = 1}) end)\n\
               horizon.log(tostring(wrote) .. \" \" .. tostring(w))\n\
             end)";
        let host = LuaHost::new(vec![load(root.path(), "hc_kinds", source)]);
        let mut server = Fake::at([1.0, 0.0, 0.0]);
        let mut client = Fake::at([1.0, 0.0, 0.0]);
        let read_only = Fake::at([1.0, 0.0, 0.0]);
        let cases: [(&str, TurnHost<'_>, [&str; 2]); 5] = [
            (
                "server",
                TurnHost::Server(&mut server),
                ["true 1", "true true"],
            ),
            (
                "client",
                TurnHost::Client(&mut client),
                ["true 1", "NOT_HERE"],
            ),
            (
                "client_read",
                TurnHost::ClientRead(&read_only),
                ["true 1", "NOT_HERE"],
            ),
            ("read", TurnHost::Read(&read_only), ["true 1", "NOT_HERE"]),
            ("none", TurnHost::None, ["NOT_HERE", "NOT_HERE"]),
        ];
        for (kind, turn, [read, write]) in cases {
            fire(&host, turn, 0);
            let lines = host.mods()[0].log_lines();
            let [got_read, got_write] = [
                lines[lines.len() - 2].clone(),
                lines[lines.len() - 1].clone(),
            ];
            let want = |expect: &'static str| -> &'static str {
                match expect {
                    "NOT_HERE" => NOT_HERE,
                    "OUTSIDE" => OUTSIDE,
                    other => other,
                }
            };
            if read.starts_with("true") {
                assert_eq!(got_read, read, "{kind}: read");
            } else {
                assert!(got_read.contains(want(read)), "{kind}: read: {got_read}");
            }
            if write.starts_with("true") {
                assert_eq!(got_write, write, "{kind}: write");
            } else {
                assert!(
                    got_write.contains(want(write)),
                    "{kind}: write: {got_write}"
                );
            }
        }
        assert_eq!(server.velocity, Some([1.0, 1.0, 1.0]));
        assert_eq!(client.velocity, None);
        assert_eq!(client.writes, 0);
    }

    #[test]
    fn a_bad_vector_is_refused_naming_the_argument_and_the_host_is_not_called() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_bad_vector",
            "horizon.on(\"block_receive_game\", function(event)\n\
               local e = event.entity\n\
               local cases = { {x = 1, y = 2}, {x = 0 / 0, y = 0, z = 0}, \"1,2,3\" }\n\
               for _, v in ipairs(cases) do\n\
                 local ok, err = pcall(function() e:set_velocity(v) end)\n\
                 horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
               end\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let mut fake = Fake::at([0.0; 3]);
        fire(&host, TurnHost::Server(&mut fake), 0);
        let lines = host.mods()[0].log_lines();
        assert_eq!(lines.len(), 3, "{lines:?}");
        for line in &lines {
            assert!(line.starts_with("false "), "{line}");
            assert!(
                line.contains("set_velocity: argument 1 (velocity)"),
                "{line}"
            );
        }
        assert_eq!(fake.writes, 0);
        assert_eq!(fake.velocity, None);
    }

    #[test]
    fn a_handle_kept_from_one_fire_uses_the_host_of_the_next_fire() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_kept",
            "horizon.on(\"block_receive_game\", function(event)\n\
               if event.n == 0 then kept = event.entity end\n\
               if event.n == 1 then horizon.log(tostring(kept:position().x)) end\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        fire(&host, TurnHost::Read(&Fake::at([1.0, 0.0, 0.0])), 0);
        fire(&host, TurnHost::Read(&Fake::at([7.0, 0.0, 0.0])), 1);
        assert_eq!(host.mods()[0].log_lines(), ["7"]);
    }

    #[test]
    fn a_handle_called_between_fires_says_there_is_no_host() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_between",
            "horizon.on(\"block_receive_game\", function(event) kept = event.entity end)",
        );
        let host = LuaHost::new(vec![m]);
        fire(&host, TurnHost::Read(&Fake::at([1.0, 0.0, 0.0])), 0);
        // The chunk runs in the mod's own environment, where `kept` lives.
        let lua = host.mods()[0].lua();
        let err = lua
            .load("return kept:position()")
            .set_environment(declare::environment(lua).unwrap())
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains(OUTSIDE), "{err}");
    }

    /// A handler that reads `n` times, split over ten handlers of 1,000 reads so each one stays
    /// inside its own time budget. The eleventh does `n` more.
    fn counted_reads(call: &str) -> String {
        let mut source = String::new();
        for _ in 0..10 {
            source.push_str(&format!(
                "horizon.on(\"block_receive_game\", function(event)\n\
                   local e = event.entity\n\
                   for i = 1, 1000 do e:{call} end\n\
                 end)\n"
            ));
        }
        source.push_str(&format!(
            "horizon.on(\"block_receive_game\", function(event)\n\
               local e = event.entity\n\
               for i = 1, event.n do e:{call} end\n\
             end)\n"
        ));
        source
    }

    #[test]
    fn ten_thousand_reads_fit_the_budget_and_the_next_one_voids_the_turn() {
        let root = tempfile::tempdir().unwrap();
        let m = load(root.path(), "hc_budget", &counted_reads("position()"));
        let host = LuaHost::new(vec![m]);
        let fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire(&host, TurnHost::Read(&fake), 0),
            1,
            "10,000 reads are fine"
        );
        assert_eq!(
            fire(&host, TurnHost::Read(&fake), 1),
            0,
            "the 10,001st read raises and the turn is void"
        );
        assert_eq!(
            fire(&host, TurnHost::Read(&fake), 0),
            1,
            "the next fire starts at zero"
        );
    }

    #[test]
    fn a_pcall_cannot_keep_a_spent_turn_going() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_pcall",
            &format!(
                "{}\n\
                 horizon.on(\"block_receive_game\", function(event)\n\
                   local e = event.entity\n\
                   for i = 1, 5 do pcall(e.position, e) end\n\
                 end)",
                counted_reads("position()")
            ),
        );
        let host = LuaHost::new(vec![m]);
        let fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire(&host, TurnHost::Read(&fake), 0),
            0,
            "the spent budget makes the pcall'd reads fail, so the turn is void"
        );
    }

    #[test]
    fn a_hundred_heavy_reads_fit_and_one_hundred_and_one_raise() {
        let root = tempfile::tempdir().unwrap();
        let mut source = String::new();
        for _ in 0..10 {
            source.push_str(
                "horizon.on(\"block_receive_game\", function(event)\n\
                   local e = event.entity\n\
                   for i = 1, 10 do e:heavy() end\n\
                 end)\n",
            );
        }
        source.push_str(
            "horizon.on(\"block_receive_game\", function(event)\n\
               local e = event.entity\n\
               for i = 1, event.n do e:heavy() end\n\
             end)\n",
        );
        let m = load(root.path(), "hc_heavy", &source);
        let host = LuaHost::new(vec![m]);
        let fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire(&host, TurnHost::Read(&fake), 0),
            1,
            "100 heavy reads fit"
        );
        assert_eq!(fire(&host, TurnHost::Read(&fake), 1), 0, "the 101st raises");
    }

    #[test]
    fn a_write_is_kept_when_a_later_error_voids_the_turn() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_keep",
            "horizon.on(\"block_receive_game\", function(event)\n\
               event.entity:set_velocity({x = 3, y = 4, z = 5})\n\
               error(\"a later failure\")\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let mut fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire(&host, TurnHost::Server(&mut fake), 0),
            0,
            "the turn is void"
        );
        assert_eq!(fake.velocity, Some([3.0, 4.0, 5.0]));
    }

    #[test]
    fn a_write_in_a_read_only_hook_is_refused_and_a_read_still_works() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_readonly",
            "horizon.on(\"block_receive_game\", function(event)\n\
               local ok, err = pcall(function() event.entity:set_velocity({x = 1, y = 1, z = 1}) end)\n\
               horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
               horizon.log(tostring(event.entity:position().x))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let fake = Fake::at([9.0, 0.0, 0.0]);
        fire(&host, TurnHost::Read(&fake), 0);
        let lines = host.mods()[0].log_lines();
        assert!(lines[0].contains(NOT_HERE), "{lines:?}");
        assert_eq!(lines[1], "9");
        assert_eq!(fake.writes, 0);
    }

    /// Fires the same hook again from its handlers: a write whose vector has x = 999 fires
    /// `dispatch_turns` again with a new `Nesting` host, while `fires` is left.
    struct Nesting<'a> {
        host: &'a LuaHost,
        fires: &'a Cell<u32>,
        refused: &'a Cell<u32>,
        deepest: &'a Cell<u32>,
    }

    impl LevelsBackend for Nesting<'_> {
        fn level_block_state(&self, _: LevelRef, _: [i32; 3]) -> Option<BlockState> {
            None
        }
    }

    host_double!(Nesting<'_>);

    impl EntitiesBackend for Nesting<'_> {
        fn entity_position(&self, _: EntityRef) -> Option<[f64; 3]> {
            None
        }
    }

    impl EntitiesBackendMut for Nesting<'_> {
        fn entity_set_velocity(&mut self, _: EntityRef, velocity: [f64; 3]) -> bool {
            if velocity[0] != 999.0 || self.fires.get() == 0 {
                return true;
            }
            self.fires.set(self.fires.get() - 1);
            self.deepest
                .set(self.deepest.get().max(DEPTH.with(Cell::get)));
            let mut next = Nesting {
                host: self.host,
                fires: self.fires,
                refused: self.refused,
                deepest: self.deepest,
            };
            match self.host.dispatch_turns(
                HOOK,
                TurnHost::Server(&mut next),
                &|| false,
                &build(0),
                &mut |_, _, _| true,
            ) {
                Ok(()) => true,
                Err(_) => {
                    self.refused.set(self.refused.get() + 1);
                    false
                }
            }
        }
    }

    const NESTING_A: &str = "horizon.on(\"block_receive_game\", function(event)\n\
           event.entity:set_velocity({x = 999, y = 0, z = 0})\n\
         end)\n\
         horizon.on(\"block_receive_game\", function(event) horizon.log(\"a again\") end)";
    const NESTING_B: &str = "horizon.on(\"block_receive_game\", function(event)\n\
           horizon.log(\"b ran\")\n\
           event.entity:set_velocity({x = 999, y = 0, z = 0})\n\
         end)";

    #[test]
    fn a_hook_fired_from_inside_a_turn_skips_the_open_mod_once_and_nothing_deadlocks() {
        let (commits, a, b, a_skips, b_skips) = within_time(|| {
            install_capture();
            let root = tempfile::tempdir().unwrap();
            let host = LuaHost::new(vec![
                load(root.path(), "hc_reentry_a", NESTING_A),
                load(root.path(), "hc_reentry_b", NESTING_B),
            ]);
            let (fires, refused, deepest) = (Cell::new(2), Cell::new(0), Cell::new(0));
            let mut turn = Nesting {
                host: &host,
                fires: &fires,
                refused: &refused,
                deepest: &deepest,
            };
            let commits = fire(&host, TurnHost::Server(&mut turn), 0);
            let a_skips = captured_with_prefix("hc_reentry_a: block_receive_game: skipped").len();
            let b_skips = captured_with_prefix("hc_reentry_b: block_receive_game: skipped").len();
            (
                commits,
                host.mods()[0].log_lines(),
                host.mods()[1].log_lines(),
                a_skips,
                b_skips,
            )
        });
        assert_eq!(commits, 2, "both mods' top-level turns committed");
        assert_eq!(a, ["a again"]);
        assert_eq!(b, ["b ran", "b ran"]);
        assert_eq!(a_skips, 1, "one skip line for mod a in this tick");
        assert_eq!(b_skips, 1, "one skip line for mod b in this tick");
    }

    #[test]
    fn a_nesting_deeper_than_the_limit_is_refused_to_the_caller() {
        let (refused, deepest) = within_time(|| {
            let root = tempfile::tempdir().unwrap();
            let mods = (0..10)
                .map(|i| {
                    load(
                        root.path(),
                        &format!("hc_depth_{i}"),
                        "horizon.on(\"block_receive_game\", function(event)\n\
                           local ok, err = pcall(function() event.entity:set_velocity({x = 999, y = 0, z = 0}) end)\n\
                           if not ok then horizon.log(tostring(err)) end\n\
                         end)",
                    )
                })
                .collect();
            let host = LuaHost::new(mods);
            let (fires, refused, deepest) = (Cell::new(100), Cell::new(0), Cell::new(0));
            let mut turn = Nesting {
                host: &host,
                fires: &fires,
                refused: &refused,
                deepest: &deepest,
            };
            fire(&host, TurnHost::Server(&mut turn), 0);
            (refused.get(), deepest.get())
        });
        assert!(refused >= 1, "a call past the depth limit is refused");
        assert_eq!(deepest, MAX_DEPTH, "the deepest call is the limit");
    }

    #[test]
    fn the_handle_is_locked_and_cannot_be_built_or_extended_from_lua() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_sandbox",
            "horizon.on(\"block_receive_game\", function(event)\n\
               local e = event.entity\n\
               horizon.log(tostring(getmetatable(e)))\n\
               horizon.log(tostring(e.foo))\n\
               horizon.log(tostring(Entity))\n\
               horizon.log(tostring(pcall(function() e.foo = 1 end)))\n\
               horizon.log(tostring(pcall(function() e.position = nil end)))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        fire(&host, TurnHost::Read(&Fake::at([0.0; 3])), 0);
        assert_eq!(
            host.mods()[0].log_lines(),
            ["false", "nil", "nil", "false", "false"]
        );
    }

    /// The event of a mod that holds real handles: `entity` (`ME`), `level`, `stranger` (a stale
    /// handle) and `n`.
    fn build_real(n: u32) -> impl Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event> {
        move |lua: &Lua, _: &TurnHost<'_>| {
            let data = lua.create_table()?;
            data.raw_set("entity", lua.create_userdata(Token(me()))?)?;
            data.raw_set("level", lua.create_userdata(Token(level()))?)?;
            data.raw_set(
                "stranger",
                lua.create_userdata(Token(EntityRef::new(WorldEpoch::new(0), ME.id)))?,
            )?;
            data.raw_set("n", n)?;
            new_event(lua, data, &["cancelled", "result"])
        }
    }

    /// Fires `HOOK` with real handles and `turn`; returns how many turns committed.
    fn fire_real(host: &LuaHost, turn: TurnHost<'_>, n: u32) -> u32 {
        let mut commits = 0;
        host.dispatch_turns(HOOK, turn, &|| false, &build_real(n), &mut |_, _, _| {
            commits += 1;
            true
        })
        .unwrap();
        commits
    }

    /// A one-hook mod with `body` as its handler, loaded under a fresh folder.
    fn one_handler(body: &str) -> LuaMod {
        let root = tempfile::tempdir().unwrap();
        let source = format!("horizon.on(\"block_receive_game\", function(event)\n{body}\nend)");
        load(root.path(), "hc_gen", &source)
    }

    #[test]
    fn a_generated_read_returns_its_values_and_a_stale_handle_is_nil() {
        let host = LuaHost::new(vec![one_handler(
            "local p = event.entity:position()\n\
             horizon.log(tostring(p.x == 1 and p.y == 2 and p.z == 3))\n\
             horizon.log(tostring(event.entity:is_in_water()))\n\
             horizon.log(tostring(event.stranger:position()))",
        )]);
        fire_real(&host, TurnHost::Read(&Fake::at([1.0, 2.0, 3.0])), 0);
        assert_eq!(host.mods()[0].log_lines(), ["true", "true", "nil"]);
    }

    #[test]
    fn a_list_of_handles_is_a_table_of_usable_userdata_and_a_level_has_its_methods() {
        let host = LuaHost::new(vec![one_handler(
            "local p = event.entity:passengers()\n\
             horizon.log(tostring(#p) .. \" \" .. tostring(p[1]:is_in_water()))\n\
             local lv = event.entity:level()\n\
             horizon.log(tostring(lv:block_state({x = 1, y = 2, z = 3})) .. \" \" .. tostring(lv))\n\
             horizon.log(tostring(event.entity) .. \" \" .. tostring(event.entity == event.entity))",
        )]);
        fire_real(&host, TurnHost::Read(&Fake::at([1.0, 2.0, 3.0])), 0);
        assert_eq!(
            host.mods()[0].log_lines(),
            ["1 true", "nil LevelRef", "EntityRef(1) true"]
        );
    }

    #[test]
    fn a_bad_position_raises_before_the_host_is_called() {
        let host = LuaHost::new(vec![one_handler(
            "local bad = {\n\
               function() return event.level:block_state({x = 1, y = 2}) end,\n\
               function() return event.level:block_state({x = \"a\", y = 2, z = 3}) end,\n\
               function() return event.level:block_state({x = 0 / 0, y = 2, z = 3}) end,\n\
             }\n\
             for _, f in ipairs(bad) do\n\
               local ok, err = pcall(f)\n\
               horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
             end",
        )]);
        let fake = Fake::at([1.0, 2.0, 3.0]);
        fire_real(&host, TurnHost::Read(&fake), 0);
        let lines = host.mods()[0].log_lines();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert!(
                line.starts_with("false ") && line.contains("LevelRef::block_state: argument 1"),
                "{line}"
            );
        }
        assert_eq!(fake.calls.get(), 0, "the host was not called");
    }

    #[test]
    fn a_handle_of_the_wrong_family_raises_and_the_host_is_not_called() {
        let host = LuaHost::new(vec![one_handler(
            "local ok, err = pcall(function()\n\
               return event.entity:is_passenger_of_same_vehicle(event.level)\n\
             end)\n\
             horizon.log(tostring(ok) .. \" \" .. tostring(err))",
        )]);
        let fake = Fake::at([1.0, 2.0, 3.0]);
        fire_real(&host, TurnHost::Read(&fake), 0);
        let line = &host.mods()[0].log_lines()[0];
        assert!(
            line.starts_with("false ")
                && line.contains(
                    "EntityRef::is_passenger_of_same_vehicle: argument 1: must be a EntityRef"
                ),
            "{line}"
        );
        assert_eq!(fake.calls.get(), 0);
    }

    #[test]
    fn a_read_spends_one_unit_so_ten_thousand_fit_and_the_next_voids_the_turn() {
        let root = tempfile::tempdir().unwrap();
        let host = LuaHost::new(vec![load(
            root.path(),
            "hc_gen_read",
            &counted_reads("position()"),
        )]);
        let fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire_real(&host, TurnHost::Read(&fake), 0),
            1,
            "10,000 reads fit"
        );
        assert_eq!(
            fire_real(&host, TurnHost::Read(&fake), 1),
            0,
            "the 10,001st voids"
        );
    }

    #[test]
    fn a_heavy_method_spends_one_hundred_units_so_one_hundred_fit_and_the_next_voids() {
        let host = LuaHost::new(vec![one_handler(
            "local e = event.entity\nfor i = 1, event.n do e:passengers() end",
        )]);
        let fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire_real(&host, TurnHost::Read(&fake), 100),
            1,
            "100 x 100 = 10,000 fit"
        );
        assert_eq!(
            fire_real(&host, TurnHost::Read(&fake), 101),
            0,
            "the 101st voids"
        );
    }

    #[test]
    fn a_generated_handle_called_outside_a_hook_says_there_is_no_host() {
        let host = LuaHost::new(vec![one_handler("kept = event.entity")]);
        fire_real(&host, TurnHost::Read(&Fake::at([1.0, 2.0, 3.0])), 0);
        let lua = host.mods()[0].lua();
        let err = lua
            .load("return kept:position()")
            .set_environment(declare::environment(lua).unwrap())
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains(OUTSIDE), "{err}");
    }

    #[test]
    fn a_none_turn_says_the_operation_is_not_available_in_this_hook() {
        let host = LuaHost::new(vec![one_handler(
            "local ok, err = pcall(function() return event.entity:is_in_water() end)\n\
             horizon.log(tostring(ok) .. \" \" .. tostring(err))",
        )]);
        fire_real(&host, TurnHost::None, 0);
        let line = &host.mods()[0].log_lines()[0];
        assert!(
            line.starts_with("false ") && line.contains(NOT_HERE),
            "{line}"
        );
    }

    /// The event of a mod that holds one handle of each new family (H1c): the entity families made
    /// from `ME`, a chunk and a raid of the test level, and `entity` (`ME`) and `level`.
    fn build_families() -> impl Fn(&Lua, &TurnHost<'_>) -> mlua::Result<Event> {
        move |lua: &Lua, _: &TurnHost<'_>| {
            let data = lua.create_table()?;
            data.raw_set("entity", lua.create_userdata(Token(me()))?)?;
            data.raw_set("level", lua.create_userdata(Token(level()))?)?;
            data.raw_set("living", lua.create_userdata(Token(LivingRef::new(me())))?)?;
            data.raw_set("mob", lua.create_userdata(Token(MobRef::new(me())))?)?;
            data.raw_set("dragon", lua.create_userdata(Token(DragonRef::new(me())))?)?;
            data.raw_set(
                "display",
                lua.create_userdata(Token(DisplayRef::new(me())))?,
            )?;
            data.raw_set(
                "minecart",
                lua.create_userdata(Token(MinecartRef::new(me())))?,
            )?;
            data.raw_set(
                "merchant",
                lua.create_userdata(Token(MerchantRef::new(me())))?,
            )?;
            data.raw_set(
                "item",
                lua.create_userdata(Token(ItemEntityRef::new(me())))?,
            )?;
            data.raw_set(
                "projectile",
                lua.create_userdata(Token(ProjectileRef::new(me())))?,
            )?;
            data.raw_set(
                "hook",
                lua.create_userdata(Token(FishingHookRef::new(me())))?,
            )?;
            data.raw_set(
                "chunk",
                lua.create_userdata(Token(ChunkRef::new(level(), [3, -4])))?,
            )?;
            data.raw_set(
                "raid",
                lua.create_userdata(Token(RaidRef::new(level(), 9)))?,
            )?;
            new_event(lua, data, &["cancelled", "result"])
        }
    }

    /// Fires `HOOK` with the handles of [`build_families`] and `turn`.
    fn fire_families(host: &LuaHost, turn: TurnHost<'_>) -> u32 {
        let mut commits = 0;
        host.dispatch_turns(HOOK, turn, &|| false, &build_families(), &mut |_, _, _| {
            commits += 1;
            true
        })
        .unwrap();
        commits
    }

    #[test]
    fn each_entity_family_reaches_its_entity_and_the_entity_answers() {
        let host = LuaHost::new(vec![one_handler(
            "for _, name in ipairs({\"living\", \"mob\", \"dragon\", \"display\", \"minecart\",\n\
               \"merchant\", \"item\", \"projectile\", \"hook\"}) do\n\
               horizon.log(name .. \" \" .. tostring(event[name]:entity():is_in_water()))\n\
             end",
        )]);
        let fake = Fake::at([0.0; 3]);
        fire_families(&host, TurnHost::Read(&fake));
        let lines = host.mods()[0].log_lines();
        assert_eq!(lines.len(), 9, "{lines:?}");
        for line in &lines {
            assert!(line.ends_with(" true"), "{line}");
        }
    }

    #[test]
    fn a_handle_returned_across_families_keeps_its_methods() {
        let host = LuaHost::new(vec![one_handler(
            "local living = event.dragon:mob():living()\n\
             horizon.log(tostring(living))\n\
             local p = living:entity():position()\n\
             horizon.log(tostring(p.x) .. \" \" .. tostring(p.y) .. \" \" .. tostring(p.z))",
        )]);
        let fake = Fake::at([1.0, 2.0, 3.0]);
        fire_families(&host, TurnHost::Read(&fake));
        assert_eq!(host.mods()[0].log_lines(), ["LivingRef(1)", "1 2 3"]);
    }

    #[test]
    fn a_new_family_read_that_needs_no_host_returns_its_value() {
        let host = LuaHost::new(vec![one_handler(
            "local pos = event.chunk:pos()\n\
             horizon.log(tostring(pos.x) .. \" \" .. tostring(pos.y))\n\
             horizon.log(tostring(event.raid:id()))",
        )]);
        let fake = Fake::at([0.0; 3]);
        fire_families(&host, TurnHost::Read(&fake));
        assert_eq!(host.mods()[0].log_lines(), ["3 -4", "9"]);
    }

    #[test]
    fn a_handle_of_a_new_family_passed_where_another_is_wanted_is_refused_and_the_host_is_not_called()
     {
        let host = LuaHost::new(vec![one_handler(
            "local ok, err = pcall(function()\n\
               return event.entity:dismount_location(event.entity)\n\
             end)\n\
             horizon.log(tostring(ok) .. \" \" .. tostring(err))",
        )]);
        let fake = Fake::at([1.0, 2.0, 3.0]);
        fire_families(&host, TurnHost::Read(&fake));
        let line = &host.mods()[0].log_lines()[0];
        assert!(
            line.starts_with("false ")
                && line.contains("EntityRef::dismount_location: argument 1: must be a LivingRef"),
            "{line}"
        );
        assert_eq!(fake.calls.get(), 0, "the host was not called");
    }

    #[test]
    fn a_bool_write_reaches_the_double_with_its_converted_argument() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_glow",
            "horizon.on(\"block_receive_game\", function(event)\n\
               horizon.log(tostring(event.entity:set_glowing(true)))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let mut fake = Fake::at([0.0; 3]);
        fire_real(&host, TurnHost::Server(&mut fake), 0);
        assert_eq!(fake.glowing, Some(true));
        assert_eq!(host.mods()[0].log_lines(), ["true"]);
    }

    #[test]
    fn a_bad_argument_to_a_write_raises_and_the_host_is_not_called() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_bad_write",
            "horizon.on(\"block_receive_game\", function(event)\n\
               local e = event.entity\n\
               local ok, err = pcall(function() e:leash_to(event.level) end)\n\
               horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
               ok, err = pcall(function() e:set_glowing(\"yes\") end)\n\
               horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let mut fake = Fake::at([0.0; 3]);
        fire_real(&host, TurnHost::Server(&mut fake), 0);
        let lines = host.mods()[0].log_lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("false ")
                && lines[0].contains("leash_to: argument 1: must be a EntityRef"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].starts_with("false ") && lines[1].contains("set_glowing: argument 1"),
            "{}",
            lines[1]
        );
        assert_eq!(fake.writes, 0);
        assert_eq!(fake.glowing, None);
    }

    #[test]
    fn a_write_on_a_turn_without_a_server_host_is_refused_before_its_arguments() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_nohost_write",
            "horizon.on(\"block_receive_game\", function(event)\n\
               local ok, err = pcall(function() event.entity:set_glowing(\"not a bool\") end)\n\
               horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let read = Fake::at([0.0; 3]);
        let mut client = Fake::at([0.0; 3]);
        let turns: [TurnHost<'_>; 4] = [
            TurnHost::Read(&read),
            TurnHost::ClientRead(&read),
            TurnHost::Client(&mut client),
            TurnHost::None,
        ];
        for turn in turns {
            fire_real(&host, turn, 0);
            let line = host.mods().last().unwrap().log_lines().pop().unwrap();
            assert!(
                line.starts_with("false ") && line.contains(NOT_HERE),
                "{line}"
            );
            assert!(!line.contains("argument 1"), "{line}");
        }
        assert_eq!(read.writes, 0);
        assert_eq!(client.writes, 0);
    }

    #[test]
    fn a_thousand_writes_fit_the_budget_and_the_next_one_voids_the_turn() {
        let root = tempfile::tempdir().unwrap();
        // Ten handlers of 100 writes (1,000 writes, 10,000 units), then one more that writes `n`.
        let mut source = String::new();
        for _ in 0..10 {
            source.push_str(
                "horizon.on(\"block_receive_game\", function(event)\n\
                   local e = event.entity\n\
                   for i = 1, 100 do e:set_glowing(true) end\n\
                 end)\n",
            );
        }
        source.push_str(
            "horizon.on(\"block_receive_game\", function(event)\n\
               for i = 1, event.n do event.entity:set_glowing(true) end\n\
             end)",
        );
        let m = load(root.path(), "hc_wbudget", &source);
        let host = LuaHost::new(vec![m]);
        let mut fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire_real(&host, TurnHost::Server(&mut fake), 0),
            1,
            "1,000 writes are 10,000 units"
        );
        assert_eq!(fake.writes, 1000);
        assert_eq!(
            fire_real(&host, TurnHost::Server(&mut fake), 1),
            0,
            "the 1,001st write raises and the turn is void"
        );
    }

    #[test]
    fn reads_and_writes_share_one_budget() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_mixed",
            "horizon.on(\"block_receive_game\", function(event)\n\
               local e = event.entity\n\
               for i = 1, 500 do e:position() end\n\
               for i = 1, 950 do e:set_glowing(true) end\n\
             end)\n\
             horizon.on(\"block_receive_game\", function(event)\n\
               for i = 1, event.n do event.entity:position() end\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let mut fake = Fake::at([0.0; 3]);
        assert_eq!(
            fire_real(&host, TurnHost::Server(&mut fake), 0),
            1,
            "500 reads and 950 writes are exactly 10,000 units"
        );
        assert_eq!(
            fire_real(&host, TurnHost::Server(&mut fake), 1),
            0,
            "one more read past the budget voids the turn"
        );
    }
    /// The generated `Hooks` method of `entity_struck_by_lightning`, a server hook with two
    /// `EntityRef` inputs, fired with `fake` as its server host. `entity` and `bolt` are `ME`.
    fn strike(host: &LuaHost, fake: &mut Fake) {
        use mod_api::Hooks;
        let mut event = mod_api::EntityStruckByLightningEvent {
            entity: me(),
            bolt: me(),
            cancelled: false,
        };
        let _ = host.entity_struck_by_lightning(fake, &mut event);
    }

    #[test]
    fn a_carried_entity_handle_is_usable_in_a_server_turn() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_carried",
            "horizon.on(\"entity_struck_by_lightning\", function(event)\n\
               horizon.log(tostring(event.entity:position().x))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        strike(&host, &mut Fake::at([1.0, 2.0, 3.0]));
        assert_eq!(host.mods()[0].log_lines(), ["1"]);
    }

    #[test]
    fn a_carried_handle_kept_in_a_global_uses_the_next_fires_host_and_none_outside_a_turn() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_carried_kept",
            "horizon.on(\"entity_struck_by_lightning\", function(event)\n\
               if kept == nil then kept = event.entity\n\
               else horizon.log(tostring(kept:position().x)) end\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        strike(&host, &mut Fake::at([1.0, 0.0, 0.0]));
        strike(&host, &mut Fake::at([7.0, 0.0, 0.0]));
        assert_eq!(host.mods()[0].log_lines(), ["7"]);
        let lua = host.mods()[0].lua();
        let err = lua
            .load("return kept:position()")
            .set_environment(declare::environment(lua).unwrap())
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains(OUTSIDE), "{err}");
    }

    #[test]
    fn a_carried_option_handle_is_nil_for_none_and_a_userdata_for_some() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_carried_option",
            "horizon.on(\"living_visibility\", function(event)\n\
               horizon.log(tostring(event.looking == nil))\n\
               if event.looking ~= nil then horizon.log(tostring(event.looking:position().x)) end\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        let fake = Fake::at([1.0, 2.0, 3.0]);
        let mut none = mod_api::LivingVisibilityEvent {
            entity: LivingRef::new(me()),
            looking: None,
            visibility: 0.5,
        };
        mod_api::Hooks::living_visibility(&host, &fake, &mut none);
        let mut some = mod_api::LivingVisibilityEvent {
            looking: Some(me()),
            ..none
        };
        mod_api::Hooks::living_visibility(&host, &fake, &mut some);
        assert_eq!(host.mods()[0].log_lines(), ["true", "false", "1"]);
    }

    #[test]
    fn a_carried_handle_cannot_be_written_back() {
        let root = tempfile::tempdir().unwrap();
        let m = load(
            root.path(),
            "hc_carried_write",
            "horizon.on(\"entity_struck_by_lightning\", function(event)\n\
               local ok, err = pcall(function() event.entity = 1 end)\n\
               horizon.log(tostring(ok) .. \" \" .. tostring(err))\n\
             end)",
        );
        let host = LuaHost::new(vec![m]);
        strike(&host, &mut Fake::at([1.0, 2.0, 3.0]));
        let line = &host.mods()[0].log_lines()[0];
        assert!(
            line.starts_with("false ") && line.contains("event.entity is read-only"),
            "{line}"
        );
    }

    #[test]
    fn the_coverage_file_lists_every_hook_and_keeps_the_handle_outputs_skipped() {
        let coverage = include_str!("generated/hooks_coverage.txt");
        let lines: Vec<&str> = coverage
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .filter(|line| {
                !line.starts_with("block_part.")
                    && !line.starts_with("item_part.")
                    && !line.starts_with("block_entity_part.")
            })
            .collect();
        assert_eq!(lines.len(), 749);
        // The BlockPart methods: one line each, emitted or with its reason.
        let parts: Vec<&str> = coverage
            .lines()
            .filter(|line| line.starts_with("block_part."))
            .collect();
        assert_eq!(parts.len(), 81);
        assert_eq!(
            parts
                .iter()
                .filter(|line| line.ends_with(" emitted"))
                .count(),
            23
        );
        assert!(parts.contains(&"block_part.can_survive emitted"));
        assert!(parts.contains(&"block_part.use_without_item emitted"));
        assert!(
            parts.contains(
                &"block_part.block_render_visibility skipped: hot path (needs measurement)"
            )
        );
        assert!(parts.contains(&"block_part.rotate skipped: worldgen worker"));
        // The ItemPart methods: 40 lines, 16 emitted; the nine hot ones and a block-only type skipped.
        let items: Vec<&str> = coverage
            .lines()
            .filter(|line| line.starts_with("item_part."))
            .collect();
        assert_eq!(items.len(), 40);
        assert_eq!(
            items
                .iter()
                .filter(|line| line.ends_with(" emitted"))
                .count(),
            16
        );
        assert!(items.contains(&"item_part.use_item emitted"));
        assert!(items.contains(&"item_part.use_on emitted"));
        assert!(items.contains(&"item_part.tooltip skipped: hot path (needs measurement)"));
        assert_eq!(
            items
                .iter()
                .filter(|line| line.ends_with("hot path (needs measurement)"))
                .count(),
            9
        );
        // The BlockEntityPart methods: 20 lines, 2 emitted; the nine hot ones, the two NBT ones and the
        // attachment one skipped.
        let block_entities: Vec<&str> = coverage
            .lines()
            .filter(|line| line.starts_with("block_entity_part."))
            .collect();
        assert_eq!(block_entities.len(), 20);
        assert_eq!(
            block_entities
                .iter()
                .filter(|line| line.ends_with(" emitted"))
                .count(),
            2
        );
        assert!(block_entities.contains(&"block_entity_part.on_chunk_unloaded emitted"));
        assert!(block_entities.contains(&"block_entity_part.block_entity_data_received emitted"));
        assert!(block_entities.contains(
            &"block_entity_part.block_entity_tick skipped: hot path (needs measurement)"
        ));
        assert!(
            block_entities.contains(
                &"block_entity_part.block_entity skipped: NBT output (Compound is denied)"
            )
        );
        assert_eq!(
            block_entities
                .iter()
                .filter(|line| line.ends_with("hot path (needs measurement)"))
                .count(),
            9
        );
        let emitted = lines
            .iter()
            .filter(|line| line.ends_with(" emitted"))
            .count();
        assert_eq!(emitted, crate::generated::hooks::EMITTED.len());
        assert!(coverage.contains("# hooks: 749, emitted: 397, skipped: 352\n"));
        assert!(lines.contains(&"right_click_block emitted"));
        // The declaration hooks are answered from the records, and no mod can subscribe to them.
        assert!(lines.contains(&"blocks emitted (host records)"));
        assert!(lines.contains(&"items emitted (host records)"));
        assert!(!crate::generated::hooks::EMITTED.contains(&"blocks"));
        assert!(lines.contains(&"explosion_detonate skipped: handle output"));
        assert!(lines.contains(&"chunk_sent emitted"));
        assert!(lines.contains(&"play_sound_source emitted"));
        assert!(lines.contains(
            &"block_drops skipped: handle input: host `&dyn LevelHost` is not carried by a turn"
        ));
    }
}
