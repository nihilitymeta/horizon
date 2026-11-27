//! The actions a goal's handler may take on its mob (#74, B1 round 2): `e.ctx`, a userdata with the five
//! operations of `GoalContext` (`move_to`, `stop_navigation`, `look_at`, `set_target`, `jump`).
//!
//! The userdata holds only the number of the goal call it was made for. A call's actions are queued
//! by `run_op` in the turn's `TurnHost::Goal` (each costs 10 units of the turn's budget, like a handle
//! write) and applied to the mob's `GoalContext` after the handler returns, so a handler's reads and
//! its actions never borrow the same context at once. Each action is validated when it is made: the
//! handler's error names the refusal, and nothing is queued. A kept `e.ctx` that is used in a later
//! call, or outside a goal call, raises "not available outside this goal call".
//!
//! Actions queued before a handler error stay applied (as a handle write before an error does).

use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{AnyUserData, Error, Lua, MultiValue, UserData, UserDataMethods, Value};
use mod_api::{EntityRef, GoalContext, LivingRef, MobRef};

use crate::handles::Args;
use crate::host::warn_once;
use crate::hostcall::{TurnHost, contained};

/// The actions one goal call may make.
pub const MAX_ACTIONS: usize = 64;
/// What one action spends of its turn's budget, as a handle write does.
pub const ACTION_COST: u32 = 10;
/// The most a `move_to` speed may be.
pub const MAX_SPEED: f64 = 3.0;

const MOVE_TO: &str = "GoalCtx::move_to";
const STOP_NAVIGATION: &str = "GoalCtx::stop_navigation";
const LOOK_AT: &str = "GoalCtx::look_at";
const SET_TARGET: &str = "GoalCtx::set_target";
const JUMP: &str = "GoalCtx::jump";

/// The message of an `e.ctx` used outside the goal call it was made for.
pub const NOT_THIS_CALL: &str = "not available outside this goal call";

/// The goal calls made so far in this process; 0 is no call.
static CALLS: AtomicU64 = AtomicU64::new(1);

/// A new goal call's number.
pub(crate) fn next_call() -> u64 {
    CALLS.fetch_add(1, Ordering::Relaxed)
}

/// One `GoalContext` operation a goal's handler asked for, its arguments converted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GoalAction {
    MoveTo { at: [f64; 3], speed: f64 },
    StopNavigation,
    LookAt { at: [f64; 3] },
    SetTarget(Option<LivingRef>),
    Jump,
}

/// The userdata `e.ctx`: nothing but the goal call it belongs to.
pub struct GoalCtx {
    call: u64,
}

impl GoalCtx {
    /// `e.ctx` for the goal call `call`.
    pub(crate) fn new(call: u64) -> GoalCtx {
        GoalCtx { call }
    }
}

impl GoalCtx {
    /// The userdata `e.ctx` of the goal call `call`, as the turn runner makes it.
    pub(crate) fn userdata(lua: &Lua, call: u64) -> mlua::Result<AnyUserData> {
        lua.create_userdata(GoalCtx::new(call))
    }
}

impl UserData for GoalCtx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // A colon call passes `e.ctx` as the first argument, so each method takes it as a userdata.
        methods.add_function(
            "move_to",
            |lua, (this, x, y, z, speed): (AnyUserData, Value, Value, Value, Value)| {
                enter(lua, this, MOVE_TO, vec![x, y, z, speed])
            },
        );
        methods.add_function("stop_navigation", |lua, this: AnyUserData| {
            enter(lua, this, STOP_NAVIGATION, Vec::new())
        });
        methods.add_function(
            "look_at",
            |lua, (this, x, y, z): (AnyUserData, Value, Value, Value)| {
                enter(lua, this, LOOK_AT, vec![x, y, z])
            },
        );
        methods.add_function("set_target", |lua, (this, target): (AnyUserData, Value)| {
            enter(lua, this, SET_TARGET, vec![target])
        });
        methods.add_function("jump", |lua, this: AnyUserData| {
            enter(lua, this, JUMP, Vec::new())
        });
    }
}

/// Whether `op` is one of the five `e.ctx` operations.
pub(crate) fn is_op(op: &str) -> bool {
    matches!(op, MOVE_TO | STOP_NAVIGATION | LOOK_AT | SET_TARGET | JUMP)
}

/// What an `e.ctx` operation costs: the turn's units, as a handle write.
pub(crate) fn cost(op: &str) -> Option<u32> {
    is_op(op).then_some(ACTION_COST)
}

/// From a Lua method: the call must be inside a turn (else it is not available), and the turn checks
/// the call's number and queues the action.
pub(crate) fn enter(
    lua: &Lua,
    this: AnyUserData,
    op: &'static str,
    args: Vec<Value>,
) -> mlua::Result<Value> {
    let inside = lua
        .named_registry_value::<Option<mlua::Function>>(crate::hostcall::SLOT)?
        .is_some();
    if !inside {
        return Err(Error::runtime(NOT_THIS_CALL));
    }
    crate::hostcall::host_call(lua, op, this, MultiValue::from_vec(args))
}

/// Runs one `e.ctx` operation in its turn: the turn must be the goal call the userdata was made for,
/// the arguments must be valid, and the call must have room (`MAX_ACTIONS`).
pub(crate) fn run(
    host: &mut TurnHost<'_>,
    op: &str,
    this: &AnyUserData,
    args: MultiValue,
) -> mlua::Result<Value> {
    let call = this.borrow::<GoalCtx>()?.call;
    let TurnHost::Goal {
        actions,
        call: current,
        ..
    } = host
    else {
        return Err(Error::runtime(NOT_THIS_CALL));
    };
    if *current != call {
        return Err(Error::runtime(NOT_THIS_CALL));
    }
    let action = parse(op, args)?;
    crate::mob_turn::queue(actions, *current, call, action, NOT_THIS_CALL)?;
    Ok(Value::Nil)
}

/// The action an operation and its arguments name, validated.
fn parse(op: &str, args: MultiValue) -> mlua::Result<GoalAction> {
    let name = match op {
        MOVE_TO => "e.ctx:move_to",
        STOP_NAVIGATION => "e.ctx:stop_navigation",
        LOOK_AT => "e.ctx:look_at",
        SET_TARGET => "e.ctx:set_target",
        _ => "e.ctx:jump",
    };
    let args = Args::new(name, args, 4)?;
    Ok(match op {
        MOVE_TO => GoalAction::MoveTo {
            at: position(&args, name)?,
            speed: speed(&args, name)?,
        },
        LOOK_AT => GoalAction::LookAt {
            at: position(&args, name)?,
        },
        SET_TARGET => GoalAction::SetTarget(target(&args, name)?),
        STOP_NAVIGATION => GoalAction::StopNavigation,
        _ => GoalAction::Jump,
    })
}

/// The finite number at `index`, named `what` in a refusal.
pub(crate) fn finite(args: &Args, index: usize, name: &str, what: &str) -> mlua::Result<f64> {
    let value: f64 = args.get(index)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(Error::runtime(format!(
            "{name}: {what} is not a finite number"
        )))
    }
}

/// The position of arguments 1 to 3 (`x`, `y`, `z`), each finite.
fn position(args: &Args, name: &str) -> mlua::Result<[f64; 3]> {
    Ok([
        finite(args, 0, name, "x")?,
        finite(args, 1, name, "y")?,
        finite(args, 2, name, "z")?,
    ])
}

/// The `move_to` speed (argument 4): finite, from 0.0 to `MAX_SPEED`.
pub(crate) fn speed(args: &Args, name: &str) -> mlua::Result<f64> {
    let speed = finite(args, 3, name, "speed")?;
    if (0.0..=MAX_SPEED).contains(&speed) {
        Ok(speed)
    } else {
        Err(Error::runtime(format!(
            "{name}: speed {speed} is outside 0.0 to {MAX_SPEED}"
        )))
    }
}

/// The `set_target` argument: nil (no target), or a mob or an entity handle.
pub(crate) fn target(args: &Args, name: &str) -> mlua::Result<Option<LivingRef>> {
    if args.value(0).is_nil() {
        return Ok(None);
    }
    if let Ok(mob) = args.handle::<MobRef>(0) {
        return Ok(Some(mob.living()));
    }
    if let Ok(entity) = args.handle::<EntityRef>(0) {
        return Ok(Some(LivingRef::new(entity)));
    }
    Err(Error::runtime(format!(
        "{name}: argument 1: must be a MobRef, an EntityRef or nil"
    )))
}

/// Applies a goal call's actions to its mob, in order, after the call's turn. Each one is contained:
/// a panic in the game's context logs its line once, quarantines the hook (through `contained`), and
/// ends the rest of the call's actions.
pub(crate) fn apply(cx: &mut GoalContext<'_>, actions: Vec<GoalAction>, owner: &str, hook: &str) {
    for action in actions {
        let done = contained(owner, hook, || match action {
            GoalAction::MoveTo { at, speed } => {
                cx.move_to(at, speed);
            }
            GoalAction::StopNavigation => cx.stop_navigation(),
            GoalAction::LookAt { at } => cx.look_at(at),
            GoalAction::SetTarget(target) => {
                cx.set_target(target);
            }
            GoalAction::Jump => cx.jump(),
        });
        if done.is_none() {
            warn_once(
                owner,
                &format!("{owner}: {hook}: an action of the goal ended the call's actions"),
            );
            return;
        }
    }
}
