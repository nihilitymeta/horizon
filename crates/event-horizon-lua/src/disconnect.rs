//! `PlayerRef::disconnect` under the `players` power (write powers v2, C9). The power check and the
//! conversion are generated; this module adds the rules the generator cannot know. They run after the
//! power check and before the host is called, and a refusal costs one unit, as a guard refusal does:
//!
//! - the target's permission level (`PlayerRef::permission_level`) must be below 2. A player at level 2
//!   (game masters) or above is refused with `horizon: disconnect refused: that player cannot be removed
//!   by a mod`. A level that cannot be read is refused too: the host is not called;
//! - the reason must be one plain Lua string (a component table is refused: the prefix below must be
//!   applied to the whole text). The reason is sent as `[<mod id>] <reason>`, cut to
//!   [`MAX_REASON_CHARS`] characters; the mod's own text is truncated, never refused;
//! - a turn may make at most `limits.kicks_per_turn` disconnects (default 1, ceiling 8). The next one is
//!   refused with `horizon: disconnect refused: at most <n> per turn`, and a refused call is not counted.
//!
//! Limits of this check, which the docs state: in a singleplayer world without cheats the host player is at
//! permission level 0, so a mod holding `players` can remove that player. That is how Bukkit's `kick` works
//! too, and it is why the power is an explicit grant of the owner.

use mlua::{AnyUserData, Error, Lua, MultiValue, Value};
use mod_api::PlayerRef;

use crate::handles::{read_of, receiver};
use crate::hostcall::TurnHost;
use crate::powers::{Limits, TurnCounts};

/// The op this module checks.
pub(crate) const DISCONNECT_OP: &str = "PlayerRef::disconnect";

/// The longest reason the host receives, in characters, the mod's prefix included.
pub const MAX_REASON_CHARS: usize = 200;

/// The lowest permission level a disconnect may remove: a player at this level or above is refused.
pub const PROTECTED_LEVEL: u8 = 2;

/// Checks one `disconnect` call. `Ok(None)` for every other op. `Ok(Some(args))` is the call's arguments
/// with the reason rewritten (prefixed and cut), which the host then receives. `Err` is the refusal.
pub(crate) fn check(
    op: &str,
    this: &AnyUserData,
    args: &MultiValue,
    host: &TurnHost<'_>,
    owner: &str,
    counts: &mut TurnCounts,
    limits: &Limits,
    lua: &Lua,
) -> mlua::Result<Option<MultiValue>> {
    if op != DISCONNECT_OP {
        return Ok(None);
    }
    let player: PlayerRef = receiver(this, op)?;
    match player.permission_level(read_of(host)?) {
        Some(level) if level < PROTECTED_LEVEL => {}
        Some(_) => {
            return Err(refusal("that player cannot be removed by a mod"));
        }
        None => {
            return Err(refusal("the player's permission level cannot be read"));
        }
    }
    let reason = match args.front() {
        Some(Value::String(text)) if args.len() == 1 => {
            text.to_str().ok().map(|text| text.to_owned())
        }
        _ => None,
    };
    let Some(reason) = reason else {
        return Err(refusal("the reason must be one plain string"));
    };
    if counts.kicks >= limits.kicks_per_turn {
        return Err(refusal(&format!(
            "at most {} per turn",
            limits.kicks_per_turn
        )));
    }
    counts.kicks += 1;
    let text: String = format!("[{owner}] {reason}")
        .chars()
        .take(MAX_REASON_CHARS)
        .collect();
    Ok(Some(MultiValue::from_vec(vec![Value::String(
        lua.create_string(&text)?,
    )])))
}

/// The refusal text of `disconnect`: the method and the reason, never the arguments.
fn refusal(reason: &str) -> Error {
    Error::runtime(format!("horizon: disconnect refused: {reason}"))
}
