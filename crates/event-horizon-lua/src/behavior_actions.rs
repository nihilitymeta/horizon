//! The actions a brain behavior's handler may take on its mob (#74, B2b): `e.ctx`, a userdata with the two
//! writes `BehaviorCxBackend` offers, `set_walk_target` and `set_brain_target`. The rules are goal_actions'
//! (see there): the userdata holds only its call's number, each action is validated when it is made, costs
//! [`ACTION_COST`] of the turn's budget, is applied after the handler returns, and a `ctx` kept past its call
//! raises. The finite-number, speed and target checks are goal_actions' own helpers.
//!
//! The walk target takes whole block coordinates (`BehaviorCx::set_walk_target` takes `[i32; 3]`), a speed from
//! 0.0 to `MAX_SPEED` and a `close` that is a whole number from 0 to [`MAX_CLOSE`].

use mlua::{AnyUserData, Error, Lua, MultiValue, UserData, UserDataMethods, Value};
use mod_api::{BehaviorCx, LivingRef};

use crate::goal_actions::{self, ACTION_COST, finite};
use crate::handles::Args;
use crate::host::warn_once;
use crate::hostcall::{TurnHost, contained};
use crate::mob_turn::queue;

/// The most a `set_walk_target` `close` may be.
pub const MAX_CLOSE: i32 = 64;

const SET_WALK_TARGET: &str = "BehaviorCtx::set_walk_target";
const SET_BRAIN_TARGET: &str = "BehaviorCtx::set_brain_target";

/// The message of an `e.ctx` used outside the behavior call it was made for.
pub const NOT_THIS_CALL: &str = "not available outside this behavior call";

/// One `BehaviorCx` write a behavior's handler asked for, its arguments converted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BehaviorAction {
    SetWalkTarget {
        at: [i32; 3],
        speed: f64,
        close: i32,
    },
    SetBrainTarget(Option<LivingRef>),
}

/// The userdata `e.ctx` of a behavior: nothing but the call it belongs to.
pub struct BehaviorCtx {
    call: u64,
}

impl BehaviorCtx {
    /// The userdata `e.ctx` of the behavior call `call`, as the turn runner makes it.
    pub(crate) fn userdata(lua: &Lua, call: u64) -> mlua::Result<AnyUserData> {
        lua.create_userdata(BehaviorCtx { call })
    }
}

impl UserData for BehaviorCtx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function(
            "set_walk_target",
            |lua, (this, x, y, z, speed, close): (AnyUserData, Value, Value, Value, Value, Value)| {
                goal_actions::enter(lua, this, SET_WALK_TARGET, vec![x, y, z, speed, close])
            },
        );
        methods.add_function(
            "set_brain_target",
            |lua, (this, target): (AnyUserData, Value)| {
                goal_actions::enter(lua, this, SET_BRAIN_TARGET, vec![target])
            },
        );
    }
}

/// Whether `op` is one of the two `e.ctx` operations.
pub(crate) fn is_op(op: &str) -> bool {
    matches!(op, SET_WALK_TARGET | SET_BRAIN_TARGET)
}

/// What an operation costs: the turn's units, as a handle write.
pub(crate) fn cost(op: &str) -> Option<u32> {
    is_op(op).then_some(ACTION_COST)
}

/// Runs one `e.ctx` operation in its turn: the turn must be the behavior call the userdata was made for, the
/// arguments must be valid, and the call must have room.
pub(crate) fn run(
    host: &mut TurnHost<'_>,
    op: &str,
    this: &AnyUserData,
    args: MultiValue,
) -> mlua::Result<Value> {
    let call = this.borrow::<BehaviorCtx>()?.call;
    let TurnHost::Behavior {
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
    queue(actions, *current, call, action, NOT_THIS_CALL)?;
    Ok(Value::Nil)
}

/// The action an operation and its arguments name, validated.
fn parse(op: &str, args: MultiValue) -> mlua::Result<BehaviorAction> {
    let name = if op == SET_WALK_TARGET {
        "e.ctx:set_walk_target"
    } else {
        "e.ctx:set_brain_target"
    };
    let args = Args::new(name, args, 5)?;
    Ok(match op {
        SET_WALK_TARGET => BehaviorAction::SetWalkTarget {
            at: [
                block(&args, 0, name, "x")?,
                block(&args, 1, name, "y")?,
                block(&args, 2, name, "z")?,
            ],
            speed: goal_actions::speed(&args, name)?,
            close: close(&args, name)?,
        },
        _ => BehaviorAction::SetBrainTarget(goal_actions::target(&args, name)?),
    })
}

/// The whole block coordinate at `index`, named `what` in a refusal: finite, whole, and within `i32`.
fn block(args: &Args, index: usize, name: &str, what: &str) -> mlua::Result<i32> {
    let value = finite(args, index, name, what)?;
    if value.fract() != 0.0 || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(Error::runtime(format!(
            "{name}: {what} {value} is not a whole block coordinate"
        )));
    }
    Ok(value as i32)
}

/// The walk target's `close` (argument 5): a whole number from 0 to `MAX_CLOSE`.
fn close(args: &Args, name: &str) -> mlua::Result<i32> {
    let close = block(args, 4, name, "close")?;
    if (0..=MAX_CLOSE).contains(&close) {
        Ok(close)
    } else {
        Err(Error::runtime(format!(
            "{name}: close {close} is outside 0 to {MAX_CLOSE}"
        )))
    }
}

/// Applies a behavior call's actions to its mob, in order, after the call's turn. Each is contained: a panic
/// in the game's context logs its line once, quarantines the hook, and ends the rest of the call's actions.
pub(crate) fn apply(
    cx: &mut BehaviorCx<'_>,
    actions: Vec<BehaviorAction>,
    owner: &str,
    hook: &str,
) {
    for action in actions {
        let done = contained(owner, hook, || match action {
            BehaviorAction::SetWalkTarget { at, speed, close } => {
                cx.set_walk_target(at, speed, close)
            }
            BehaviorAction::SetBrainTarget(target) => {
                cx.set_brain_target(target);
            }
        });
        if done.is_none() {
            warn_once(
                owner,
                &format!("{owner}: {hook}: an action of the behavior ended the call's actions"),
            );
            return;
        }
    }
}
