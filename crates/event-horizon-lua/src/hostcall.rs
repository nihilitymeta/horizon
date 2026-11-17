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
//! A `None` turn opens no scope, so every host call raises `no host call outside a hook`.
//!
//! The operations themselves are the handles' job (the next chunk). Until then the only operations
//! are the H0 stub's, which exist under `cfg(test)` and drive the real `EntityRef` methods.
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

use mlua::{Error, Lua, Value};
use mod_api::{ClientHost, Host, HostSide, ServerHost};

use crate::host::{LOG_TARGET, LuaMod};

/// The units one turn may spend on host calls.
pub const TURN_BUDGET: u32 = 10_000;
/// The hook calls that may be open at once on one thread. A call past this is refused.
pub const MAX_DEPTH: u32 = 8;
/// The Lua registry key of the turn's scoped function. Only [`scoped`] sets it, and only in a turn.
const SLOT: &str = "event_horizon.host_call";
/// Raised when a handle is called with no turn open, or by a turn with no host at all.
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

/// Runs `body` for one mod's turn with `host` open to its handles. For a host turn, a scope holds
/// the scoped function under [`SLOT`] while `body` runs. A `None` turn runs `body` with no scope.
/// Returns what `body` returned and whether the turn's budget was spent, which voids the turn even
/// when a `pcall` hid the raise. An error from the scope itself is returned as is.
pub(crate) fn scoped<R>(
    lua: &Lua,
    host: &mut TurnHost<'_>,
    body: impl FnOnce(&Lua) -> R,
) -> mlua::Result<(R, bool)> {
    if let TurnHost::None = host {
        return Ok((body(lua), false));
    }
    let mut turn_host = host.reborrow();
    let spent_cell = Cell::new(false);
    let spent_out = &spent_cell;
    let out = lua.scope(|scope| {
        let mut spent = 0u32;
        let calls = scope.create_function_mut(
            move |lua: &Lua, (op, id, epoch, arg): (String, u32, u32, Value)| {
                if spent_out.get() {
                    return Err(Error::runtime(SPENT));
                }
                let cost = op_cost(&op)?;
                if spent + cost > TURN_BUDGET {
                    spent_out.set(true);
                    return Err(Error::runtime(SPENT));
                }
                spent += cost;
                run_op(lua, &mut turn_host, &op, id, epoch, arg)
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
/// operation's name and the handle's id and epoch. Outside a host turn there is none.
pub fn host_call(
    lua: &Lua,
    op: &'static str,
    id: u32,
    epoch: u32,
    arg: Value,
) -> mlua::Result<Value> {
    let slot: Option<mlua::Function> = lua.named_registry_value(SLOT)?;
    match slot {
        Some(slot) => slot.call((op, id, epoch, arg)),
        None => Err(Error::runtime(OUTSIDE)),
    }
}

/// The operations the scoped function knows. None exist until the handles do (the next chunk), so
/// outside tests every call is an unknown operation; the H0 stub's table is under `cfg(test)`.
#[cfg(not(test))]
fn op_cost(op: &str) -> mlua::Result<u32> {
    Err(Error::runtime(format!("unknown host call {op}")))
}

#[cfg(not(test))]
fn run_op(
    _: &Lua,
    _: &mut TurnHost<'_>,
    op: &str,
    _: u32,
    _: u32,
    _: Value,
) -> mlua::Result<Value> {
    Err(Error::runtime(format!("unknown host call {op}")))
}

#[cfg(test)]
use stub::{op_cost, run_op};

#[cfg(test)]
mod stub {
    //! The H0 stub: a handle `Entity` with `position`, `heavy` and `set_velocity`, which reach the
    //! real `EntityRef` methods through the turn's host. Test-only: the handles are the next chunk.

    use event_horizon_lua_convert::scalar::FromLuaValue;
    use mlua::{Error, Lua, UserData, UserDataMethods, Value};
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
            methods.add_method("position", |lua, this, ()| {
                host_call(lua, "position", this.id, this.epoch, Value::Nil)
            });
            methods.add_method("heavy", |lua, this, ()| {
                host_call(lua, "heavy", this.id, this.epoch, Value::Nil)
            });
            methods.add_method("set_velocity", |lua, this, velocity: Value| {
                host_call(lua, "set_velocity", this.id, this.epoch, velocity)
            });
        }
    }

    pub fn op_cost(op: &str) -> mlua::Result<u32> {
        match op {
            "position" => Ok(READ),
            "heavy" => Ok(HEAVY),
            "set_velocity" => Ok(WRITE),
            other => Err(Error::runtime(format!("unknown host call {other}"))),
        }
    }

    /// Runs one operation against the turn's host. Each operation asks for the host kind it needs,
    /// so a read-only turn refuses a write with the same error a `None` turn gives a read.
    pub fn run_op(
        lua: &Lua,
        host: &mut TurnHost<'_>,
        op: &str,
        id: u32,
        epoch: u32,
        arg: Value,
    ) -> mlua::Result<Value> {
        let entity = EntityRef::new(WorldEpoch::new(u64::from(epoch)), id);
        match op {
            "position" | "heavy" => {
                let Some(read) = host.read() else {
                    return Err(Error::runtime(NOT_HERE));
                };
                match EntityRef::position(entity, read) {
                    Some([x, y, z]) => {
                        let point = lua.create_table()?;
                        point.raw_set("x", x)?;
                        point.raw_set("y", y)?;
                        point.raw_set("z", z)?;
                        Ok(Value::Table(point))
                    }
                    None => Ok(Value::Nil),
                }
            }
            _ => {
                let Some(server) = host.server() else {
                    return Err(Error::runtime(NOT_HERE));
                };
                let velocity = vector(&arg)?;
                Ok(Value::Boolean(entity.set_velocity(server, velocity)))
            }
        }
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
    use mod_api::prelude::{EntitiesBackend, EntitiesBackendMut};
    use mod_api::{
        Backend, BackendMut, BackendMutRef, BackendRef, ClientHost, EntityRef, Host, ServerHost,
    };

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
            mod_api::impl_backend_areas!($ty, except(entities));
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

    /// A host that knows `ME` at `at`, counts its writes and keeps the last velocity.
    struct Fake {
        at: [f64; 3],
        velocity: Option<[f64; 3]>,
        writes: u32,
    }

    impl Fake {
        fn at(at: [f64; 3]) -> Fake {
            Fake {
                at,
                velocity: None,
                writes: 0,
            }
        }
    }

    host_double!(Fake);

    impl EntitiesBackend for Fake {
        fn entity_position(&self, entity: EntityRef) -> Option<[f64; 3]> {
            is_me(entity).then_some(self.at)
        }
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
            ("none", TurnHost::None, ["OUTSIDE", "OUTSIDE"]),
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
    fn counted_reads(method: &str) -> String {
        let mut source = String::new();
        for _ in 0..10 {
            source.push_str(&format!(
                "horizon.on(\"block_receive_game\", function(event)\n\
                   local e = event.entity\n\
                   for i = 1, 1000 do e:{method}() end\n\
                 end)\n"
            ));
        }
        source.push_str(&format!(
            "horizon.on(\"block_receive_game\", function(event)\n\
               local e = event.entity\n\
               for i = 1, event.n do e:{method}() end\n\
             end)\n"
        ));
        source
    }

    #[test]
    fn ten_thousand_reads_fit_the_budget_and_the_next_one_voids_the_turn() {
        let root = tempfile::tempdir().unwrap();
        let m = load(root.path(), "hc_budget", &counted_reads("position"));
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
                counted_reads("position")
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
}
