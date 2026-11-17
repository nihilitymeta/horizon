//! Host calls from a Lua handler (H0 spike, issue 74): a handle that reaches the host of the hook
//! call that is running, with no `'static` host and no `unsafe`.
//!
//! A hook that has a host hands it to `LuaHost::dispatch_hosted` as a [`TurnHost`]. For each mod's
//! turn, `run_hosted` opens a `Lua::scope`, and inside it creates one scoped Lua function that
//! closes over that host. A scope may borrow for one call only, so the host never has to be
//! `'static`. The function is stored in the Lua registry under [`SLOT`]. The handle's methods are
//! ordinary `'static` callbacks: they look the slot up and call it with the operation's name. The
//! scoped function does the host work, charges the turn's budget and checks the hook's kind. The
//! slot is cleared before the scope ends, so a handle kept from an earlier fire finds no slot and
//! raises `no host call outside a hook`.
//!
//! [`HostLike`] and [`WriteHostLike`] are small traits of this file. The real `Host` and
//! `ServerHost` of mod_api are not used here: each real host kind gets a one-line adapter to these
//! traits, which is the next chunk's job.
//!
//! Re-entrancy: a host call may fire a hook again on this thread. A mod whose turn is open is
//! skipped there (one log line per mod per tick), the other mods run, and the nesting is limited to
//! [`MAX_DEPTH`] hook calls open at once.
//!
//! Budget: each turn may spend [`TURN_BUDGET`] units. A read costs 1, a write 10 and a heavy read
//! 100. A call past the budget raises, and the budget stays spent for the rest of the turn, so a
//! `pcall` cannot keep the turn going. The turn is then void, as a failing handler's is. A write
//! is not rolled back by a void turn: the host keeps it.

use std::cell::{Cell, RefCell};

use event_horizon_lua_convert::scalar::FromLuaValue;
use mlua::{Error, Function, Lua, Table, UserData, UserDataMethods, Value};

use crate::dispatch::Event;
use crate::host::{LOG_TARGET, LuaHost, LuaMod, call_handler};

/// The units one turn may spend on host calls.
pub const TURN_BUDGET: u32 = 10_000;
/// The hook calls that may be open at once on one thread. A call past this raises.
pub const MAX_DEPTH: u32 = 8;
const READ: u32 = 1;
const WRITE: u32 = 10;
const HEAVY: u32 = 100;
/// The Lua registry key of the turn's scoped function. Only the host sets it, and only inside a turn.
const SLOT: &str = "event_horizon.host_call";
const OUTSIDE: &str = "no host call outside a hook";
const NOT_HERE: &str = "not available in this hook";
const SPENT: &str = "host call budget is spent for this turn";

/// A handle to a world object, by the id and epoch the host gave it. A copy of two numbers: it
/// holds no host and says nothing about the object until a method asks the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entity {
    pub id: u32,
    pub epoch: u32,
}

impl UserData for Entity {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("id", |_, this, ()| Ok(this.id));
        methods.add_method("position", |lua, this, ()| {
            host_call(lua, "position", *this, Value::Nil)
        });
        methods.add_method("heavy", |lua, this, ()| {
            host_call(lua, "heavy", *this, Value::Nil)
        });
        methods.add_method("set_velocity", |lua, this, velocity: Value| {
            host_call(lua, "set_velocity", *this, velocity)
        });
    }
}

/// A read of the world: what a read-only hook gets, and what a writable hook gets too.
pub trait HostLike {
    /// The position of `entity`, or `None` when the handle is stale.
    fn position(&self, entity: Entity) -> Option<[f64; 3]>;
    /// A read that scans more than a position does (charged as a heavy read).
    fn heavy(&self, entity: Entity) -> Option<[f64; 3]>;
}

/// A write of the world: what a hook that may change things gets.
pub trait WriteHostLike: HostLike {
    /// Sets the velocity of `entity`. `Ok(false)` when the handle is stale; `Err` is refused and
    /// reaches the handler as a Lua error.
    fn set_velocity(&mut self, entity: Entity, velocity: [f64; 3]) -> Result<bool, String>;
}

/// The host a hook gave for one call. A `None` hook has no host: every host call is refused.
pub enum TurnHost<'a> {
    Read(&'a dyn HostLike),
    Write(&'a mut dyn WriteHostLike),
    None,
}

impl TurnHost<'_> {
    /// The same host for one mod's turn, borrowed again so each mod gets its own scope.
    fn reborrow(&mut self) -> TurnHost<'_> {
        match self {
            TurnHost::Read(host) => TurnHost::Read(*host),
            TurnHost::Write(host) => TurnHost::Write(&mut **host),
            TurnHost::None => TurnHost::None,
        }
    }
}

thread_local! {
    /// The hook calls open on this thread.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// The mods skipped for being open, in the current top-level hook call.
    static SKIPPED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Lets go of one open hook call when it ends, also when it panics.
struct OpenCall;

impl Drop for OpenCall {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
    }
}

impl LuaHost {
    /// Fires `hook` for every mod that declared it, for a hook that gets a host. Each mod's turn is
    /// run as `dispatch_turns` runs it, but its handlers can call `host` through their handles.
    /// `Err` when `MAX_DEPTH` hook calls are already open on this thread: the call is refused.
    pub fn dispatch_hosted(
        &self,
        hook: &str,
        mut host: TurnHost<'_>,
        build: &dyn Fn(&Lua) -> mlua::Result<Event>,
        commit: &mut dyn FnMut(&str, &Event) -> bool,
    ) -> Result<(), String> {
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
        let _open = OpenCall;
        self.run_turns(hook, &|| false, build, commit, Some(&mut host));
        Ok(())
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

/// Runs one mod's handlers inside a scope that carries `host`. Returns whether the turn failed: a
/// handler raised, the scope failed, or the budget ran out (even if a `pcall` hid the raise).
pub(crate) fn run_hosted(
    m: &LuaMod,
    hook: &str,
    host: &mut TurnHost<'_>,
    handlers: &[Function],
    event: &Table,
) -> bool {
    let lua = m.lua();
    let mut turn_host = host.reborrow();
    let spent_cell = Cell::new(false);
    let spent_out = &spent_cell;
    let outcome = lua.scope(|scope| {
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
                run_op(lua, &mut turn_host, &op, Entity { id, epoch }, arg)
            },
        )?;
        lua.set_named_registry_value(SLOT, calls)?;
        let mut failed = false;
        for handler in handlers {
            if let Err(err) = call_handler(lua, handler, event) {
                log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
                failed = true;
            }
        }
        // The slot goes before the scope ends, so nothing finds the function once it is invalid.
        let _ = lua.set_named_registry_value(SLOT, Value::Nil);
        Ok(failed)
    });
    match outcome {
        Ok(failed) => failed || spent_cell.get(),
        Err(err) => {
            log::warn!(target: LOG_TARGET, "{}: {hook}: {err}", m.id());
            true
        }
    }
}

/// The handle's side of a host call: find the turn's scoped function and call it. Outside a turn
/// there is none.
fn host_call(lua: &Lua, op: &'static str, entity: Entity, arg: Value) -> mlua::Result<Value> {
    let slot: Option<Function> = lua.named_registry_value(SLOT)?;
    match slot {
        Some(slot) => slot.call((op, entity.id, entity.epoch, arg)),
        None => Err(Error::runtime(OUTSIDE)),
    }
}

/// What one operation costs the turn, or an error for a name that is not an operation.
fn op_cost(op: &str) -> mlua::Result<u32> {
    match op {
        "position" => Ok(READ),
        "heavy" => Ok(HEAVY),
        "set_velocity" => Ok(WRITE),
        other => Err(Error::runtime(format!("unknown host call {other}"))),
    }
}

/// Runs one operation against the hook's host. The host kind is checked here, so a read-only hook
/// refuses a write with the same error a `None` hook gives.
fn run_op(
    lua: &Lua,
    host: &mut TurnHost<'_>,
    op: &str,
    entity: Entity,
    arg: Value,
) -> mlua::Result<Value> {
    match op {
        "position" | "heavy" => {
            let reader: &dyn HostLike = match host {
                TurnHost::Read(reader) => *reader,
                TurnHost::Write(writer) => &**writer,
                TurnHost::None => return Err(Error::runtime(NOT_HERE)),
            };
            let found = if op == "position" {
                reader.position(entity)
            } else {
                reader.heavy(entity)
            };
            match found {
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
            let TurnHost::Write(writer) = host else {
                return Err(Error::runtime(NOT_HERE));
            };
            let velocity = vector(&arg)?;
            let moved = writer
                .set_velocity(entity, velocity)
                .map_err(Error::runtime)?;
            Ok(Value::Boolean(moved))
        }
    }
}

/// Reads `{x, y, z}` strictly: each axis must be a finite number. The error names the argument.
fn vector(value: &Value) -> mlua::Result<[f64; 3]> {
    let bad = |why: String| Error::runtime(format!("set_velocity: argument 1 (velocity) {why}"));
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::declare;
    use crate::discover::{Discovered, ENTRY, Side};
    use crate::dispatch::new_event;
    use crate::host::tests::{captured_with_prefix, install_capture};

    const HOOK: &str = "block_receive_game";
    const ME: Entity = Entity { id: 1, epoch: 1 };
    /// The same id with an older epoch: a stale handle, which the host does not know.
    const STRANGER: Entity = Entity { id: 1, epoch: 0 };

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

    impl HostLike for Fake {
        fn position(&self, entity: Entity) -> Option<[f64; 3]> {
            (entity == ME).then_some(self.at)
        }
        fn heavy(&self, entity: Entity) -> Option<[f64; 3]> {
            self.position(entity)
        }
    }

    impl WriteHostLike for Fake {
        fn set_velocity(&mut self, entity: Entity, velocity: [f64; 3]) -> Result<bool, String> {
            self.writes += 1;
            if entity != ME {
                return Ok(false);
            }
            self.velocity = Some(velocity);
            Ok(true)
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
    fn build(n: u32) -> impl Fn(&Lua) -> mlua::Result<Event> {
        move |lua| {
            let data = lua.create_table()?;
            data.raw_set("entity", lua.create_userdata(ME)?)?;
            data.raw_set("stranger", lua.create_userdata(STRANGER)?)?;
            data.raw_set("n", n)?;
            new_event(lua, data, &["cancelled", "result"])
        }
    }

    /// Fires `HOOK` with `host` and `n`; returns how many turns committed.
    fn fire(host: &LuaHost, turn: TurnHost<'_>, n: u32) -> u32 {
        let mut commits = 0;
        host.dispatch_hosted(HOOK, turn, &build(n), &mut |_, _| {
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
        fire(&host, TurnHost::Write(&mut fake), 0);
        assert_eq!(fake.velocity, Some([0.5, -1.0, 2.0]));
        assert_eq!(host.mods()[0].log_lines(), ["true"]);
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
        fire(&host, TurnHost::Write(&mut fake), 0);
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
            fire(&host, TurnHost::Write(&mut fake), 0),
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
    /// `dispatch_hosted` again with a new `Nesting` host, while `fires` is left.
    struct Nesting<'a> {
        host: &'a LuaHost,
        fires: &'a Cell<u32>,
        refused: &'a Cell<u32>,
        deepest: &'a Cell<u32>,
    }

    impl HostLike for Nesting<'_> {
        fn position(&self, _: Entity) -> Option<[f64; 3]> {
            None
        }
        fn heavy(&self, _: Entity) -> Option<[f64; 3]> {
            None
        }
    }

    impl WriteHostLike for Nesting<'_> {
        fn set_velocity(&mut self, _: Entity, velocity: [f64; 3]) -> Result<bool, String> {
            if velocity[0] != 999.0 || self.fires.get() == 0 {
                return Ok(true);
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
            match self.host.dispatch_hosted(
                HOOK,
                TurnHost::Write(&mut next),
                &build(0),
                &mut |_, _| true,
            ) {
                Ok(()) => Ok(true),
                Err(err) => {
                    self.refused.set(self.refused.get() + 1);
                    Err(err)
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
            let commits = fire(&host, TurnHost::Write(&mut turn), 0);
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
            fire(&host, TurnHost::Write(&mut turn), 0);
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
