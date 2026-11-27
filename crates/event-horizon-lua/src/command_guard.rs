//! The `commands` power's rules: which command line a mod may run, and at what permission level
//! (write powers v2, `/work/eh-reports/write-powers-v2-design.md` section 4).
//!
//! `EntityRef::run_command(line, level)` is the only command write a mod reaches. [`check`] runs in
//! `powers::guard` after the power check, before the host sees the call:
//! - the level is one of `all`, `moderators`, `gamemasters`; `admins` and `owners` are refused, so a
//!   mod never runs above level 2 whatever it passes;
//! - the line is at most [`MAX_LINE_CHARS`] characters, with no newline, carriage return or NUL;
//! - the command name is the first token of the line, after trimming whitespace, one leading `/`, and
//!   a `minecraft:` prefix. The name is matched exactly and case-sensitively;
//! - a name in [`NEVER_ALLOW`] is refused whatever the owner lists, `*` included;
//! - any other name must be in the mod's allow list: the owner's `[command_allow]` entry for the mod
//!   when it has one (`*` allows every name not in [`NEVER_ALLOW`]), else [`DEFAULT_ALLOW`].
//!
//! The owner's lists come from `enabled.toml` (`enabled.rs`), and a mod never reads or changes them.

use mlua::{Error, MultiValue, Value};

/// The command names a mod runs with no `[command_allow]` entry of its own: output and effect
/// commands only. Each one that changes a player's items, the world or the server belongs to a power
/// of its own, so `commands` does not reach it.
pub const DEFAULT_ALLOW: &[&str] = &[
    "say",
    "tellraw",
    "title",
    "playsound",
    "particle",
    "effect",
    "tag",
    "bossbar",
    "scoreboard",
];

/// The command names a mod may never run: they chain into other commands or are server
/// administration. No owner's list reaches them, `*` included.
pub const NEVER_ALLOW: &[&str] = &[
    "execute",
    "function",
    "reload",
    "datapack",
    "op",
    "deop",
    "ban",
    "ban-ip",
    "pardon",
    "kick",
    "stop",
    "save-all",
    "whitelist",
    "perf",
    "debug",
];

/// The longest command line a mod may pass, in characters.
pub const MAX_LINE_CHARS: usize = 1024;
/// The longest name in an owner's allow list, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// The most names in one owner's allow list.
pub const MAX_NAMES: usize = 64;
/// The wildcard an owner's allow list may hold: every name not in [`NEVER_ALLOW`].
pub const WILDCARD: &str = "*";

/// The permission levels a mod may pass, by the names `PermissionLevel` converts from.
const ALLOWED_LEVELS: &[&str] = &["all", "moderators", "gamemasters"];

/// The owner's rule for one mod's commands: its id (named in a refusal) and its allow list, `None`
/// when the owner gave it no list, which means [`DEFAULT_ALLOW`]. Held by each turn's scope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CommandPolicy {
    /// The mod's id.
    pub owner: String,
    /// The owner's list of command names for the mod, `None` for the default.
    pub allow: Option<Vec<String>>,
}

/// The guard of `EntityRef::run_command`: `args` are the Lua arguments after the handle, the line
/// then the level. Any other call passes.
pub(crate) fn check(args: &MultiValue, policy: &CommandPolicy) -> Result<(), Error> {
    let line = string_arg(args, 0);
    let level = string_arg(args, 1);
    check_text(line.as_deref(), level.as_deref(), policy).map_err(Error::runtime)
}

/// The string at `index` of the Lua arguments, or `None` for a missing argument or another type.
fn string_arg(args: &MultiValue, index: usize) -> Option<String> {
    match args.iter().nth(index) {
        Some(Value::String(text)) => text.to_str().ok().map(|text| text.to_owned()),
        _ => None,
    }
}

/// The rules of [`check`] on the Rust text of the arguments; `Err` carries the refusal text.
pub(crate) fn check_text(
    line: Option<&str>,
    level: Option<&str>,
    policy: &CommandPolicy,
) -> Result<(), String> {
    match level {
        Some(name) if ALLOWED_LEVELS.contains(&name) => {}
        Some("admins" | "owners") => return Err(refuse("permission level is capped at 2")),
        _ => return Err(refuse("the permission level is not a level name")),
    }
    let line = line.ok_or_else(|| refuse("the command line is not valid"))?;
    if line.chars().count() > MAX_LINE_CHARS || line.contains(['\n', '\r', '\0']) {
        return Err(refuse("the command line is not valid"));
    }
    let body = line.trim();
    let body = body.strip_prefix('/').unwrap_or(body);
    let token = body.split_whitespace().next().unwrap_or("");
    let name = token.strip_prefix("minecraft:").unwrap_or(token);
    if name.is_empty() {
        return Err(refuse("no command"));
    }
    if NEVER_ALLOW.contains(&name) {
        return Err(refuse(&format!("\"{name}\" cannot be run by a mod")));
    }
    let allowed = match &policy.allow {
        None => DEFAULT_ALLOW.contains(&name),
        Some(list) => list.iter().any(|entry| entry == WILDCARD || entry == name),
    };
    if !allowed {
        return Err(refuse(&format!(
            "\"{name}\" is not an allowed command for {}; the owner may list it under [command_allow]",
            policy.owner
        )));
    }
    Ok(())
}

/// The refusal text of `run_command`, with its reason.
fn refuse(reason: &str) -> String {
    format!("horizon: run_command refused: {reason}")
}

/// Whether one name of an owner's `[command_allow]` list is valid: not empty, at most
/// [`MAX_NAME_CHARS`] characters, and no whitespace.
pub(crate) fn valid_allow_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.chars().any(char::is_whitespace)
}
