//! The developer tools' commands (docs/dev-tools.md) as a built-in mod's: `/inspect`, `/goals`,
//! `/senses`, `/nav`, `/watch` and `/mobai log` are in the server's command tree and run from the
//! mod's hooks. The bodies keep the old messages, counts and store calls, on the shared
//! [`event_horizon_observe`] store the windows read. `/lua` runs a chunk in the console's sandboxed
//! VM ([`event_horizon_lua::console`]); `/console` opens, closes or clears the console window
//! ([`crate::console`]), whose queued chunks the server tick runs. `/luareload <id>` reloads a dev-path
//! Lua mod's scripts ([`event_horizon_lua::reload`]); `/luareload` alone lists the ids that reload.
//!
//! The owner's rule: every one of these commands is the world's owner's, strictly. The sender's
//! uuid must be the one the host names as the single-player owner; any other player (a guest of
//! any level, an operator) is refused, and on a dedicated server (no owner) everyone is. The
//! runner is never reached for a refused sender.
use crate::console;
use event_horizon_lua::console as runner;
use event_horizon_observe::{self as observe, BlockKey, View};
use horizon_client::{data_cmd, lang, nbt_path::NbtPath};
use mod_api::text::TextStyle;
use mod_api::{
    CommandArgumentValue, CommandInvocation, CommandSource, PlayerRef, ServerHost, Text,
};

/// The most printed lines a `/lua` reply shows: the last ones, then a line that counts the rest.
const LUA_LINES: usize = 20;
/// The longest line a reply shows, in characters; a longer one is cut and ends in an ellipsis.
const LUA_LINE_CHARS: usize = 256;

/// A translated line: the language file's `key`, its arguments as plain text.
fn tr(key: &str, args: &[String]) -> Text {
    Text::translate(key, args.iter().map(Text::literal).collect())
}

/// `line` in red, as a command's failure is.
fn red(line: Text) -> Text {
    let mut style = TextStyle::default();
    style.color = Some("red".to_owned());
    line.with_style(style)
}

/// The red line that says a command failed, from its language key.
fn failure(key: &str) -> Text {
    red(tr(key, &[]))
}

/// The usages the tools register in the server's tree. Every player may run them: the owner's
/// rule is in the handlers, not in the tree. The argument `rest` is the line's remainder, which
/// the handlers read the way the old console line was read (`parse`).
pub const USAGES: &[&str] = &[
    "inspect",
    "inspect <rest:text>",
    "goals <rest:text>",
    "senses <rest:text>",
    "nav <rest:text>",
    "watch <rest:text>",
    "mobai log <rest:text>",
    "lua",
    "lua <rest:text>",
    "console",
    "console close",
    "console clear",
    "luareload",
    "luareload <rest:text>",
    "blockinfo",
    "blockinfo clear",
    "blockinfo <rest:text>",
];

/// One of the tools' commands, as the line names it.
#[derive(Debug, PartialEq)]
enum Cmd<'a> {
    /// `/lua <code>`: a chunk for the console's VM; empty `code` replies the usage.
    Lua { code: &'a str },
    /// `/console`, `/console close`, `/console clear`: the console window.
    Console { action: console::Action },
    /// `/luareload <id>`: reloads a dev mod's scripts; empty `id` lists the reloadable ids.
    Reload { id: &'a str },
    /// `/inspect` alone: what is watched.
    List,
    /// `/inspect|goals|senses|nav <targets>` and `… clear`.
    View {
        view: View,
        clear: bool,
        targets: &'a str,
    },
    /// `/watch <targets> <path>` and `/watch clear`.
    Watch {
        clear: bool,
        targets: &'a str,
        path: &'a str,
    },
    /// `/mobai log <targets>` and `/mobai log stop`.
    MobaiLog { stop: bool, targets: &'a str },
    /// `/blockinfo`, `/blockinfo clear`, `/blockinfo <x> <y> <z>`.
    Block { action: BlockAction },
}

/// What `/blockinfo` does with its line.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BlockAction {
    /// `/blockinfo` alone: the watched blocks.
    List,
    /// `/blockinfo clear`: every block window closes.
    Clear,
    /// `/blockinfo <x> <y> <z>`: a block window for that block, in the owner's dimension.
    Watch([Coord; 3]),
    /// The coordinates do not parse: the usage line.
    Usage,
}

/// One coordinate of `/blockinfo`: a block coordinate, or one counted from the owner's block
/// (`~` is `~0`).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Coord {
    Abs(i32),
    Rel(i32),
}

/// Parses `<x> <y> <z>`, each an integer, `~` or `~n`. None for anything else.
pub fn parse_coords(text: &str) -> Option<[Coord; 3]> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let [x, y, z] = words.as_slice() else {
        return None;
    };
    Some([parse_coord(x)?, parse_coord(y)?, parse_coord(z)?])
}

/// One coordinate word: an integer, `~`, or `~` and an integer offset.
fn parse_coord(word: &str) -> Option<Coord> {
    match word.strip_prefix('~') {
        Some("") => Some(Coord::Rel(0)),
        Some(offset) => offset.parse().ok().map(Coord::Rel),
        None => word.parse().ok().map(Coord::Abs),
    }
}

/// The block the coordinates name: a relative one is counted from `here`, the owner's block.
/// None when one is relative and there is no `here`, or when the sum overflows.
pub fn resolve(coords: [Coord; 3], here: Option<[i32; 3]>) -> Option<[i32; 3]> {
    let mut pos = [0; 3];
    for (i, coord) in coords.into_iter().enumerate() {
        pos[i] = match coord {
            Coord::Abs(v) => v,
            Coord::Rel(v) => here?[i].checked_add(v)?,
        };
    }
    Some(pos)
}

/// The server's reads and the command's replies, as the commands use them. The live host
/// implements it ([`Live`]); the tests implement it with fakes.
pub trait Reads {
    /// The single-player owner's uuid (`dev_server_owner`); None on a dedicated server.
    fn owner(&self) -> Option<u128>;
    /// The dimension id of the level the sender is in, when it is known.
    fn dimension(&self) -> Option<String>;
    /// The network entity ids a selector names, as the server's command source finds them.
    fn select_ids(&mut self, selector: &str) -> Vec<u32>;
    /// Whether entity `id` is a living mob that is alive.
    fn is_mob(&self, id: u32) -> bool;
    /// The name of entity `id`, when it has one.
    fn entity_name(&self, id: u32) -> Option<String>;
    /// The block the sender stands in (its entity's block position); None for the console.
    fn block_pos(&self) -> Option<[i32; 3]>;
    /// Runs `source` as a chunk in the console's VM, with the server's host.
    fn run_lua(&mut self, source: &str) -> LuaRun;
    /// Reloads the scripts of the dev-path Lua mod `id`, as the game's reload does.
    fn reload(&mut self, id: &str) -> ReloadOutcome;
    /// The ids the game's `reload` may reload (the dev-path mods), in load order.
    fn reloadable(&self) -> Vec<String>;
    /// A line of feedback to the sender.
    fn reply(&mut self, line: Text);
}

/// What a `/lua` chunk gave back: the lines it printed, its returned value and its error, as text.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LuaRun {
    pub lines: Vec<String>,
    pub value: Option<String>,
    pub error: Option<String>,
}

/// What a `/luareload` gave back, in the tools' own terms: the live host maps the Lua crate's result.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// The mod's scripts were swapped; `scripts` is how many the new state was built from.
    Reloaded { id: String, scripts: usize },
    /// No loaded Lua mod has the id.
    #[default]
    NotLoaded,
    /// The mod was not started from a dev path.
    NotADevMod,
    /// Another thread held the mod's turn for too long.
    Busy,
    /// The new scripts could not be read or failed to run or declare; the text is the error.
    Failed(String),
    /// The fresh declaration differs; the names of what changed.
    Refused(Vec<String>),
}

/// The live server's reads for one sender: the host, and the replies collected for the hook to
/// deliver (as command feedback, or as the chat lines of `player_command`).
pub struct Live<'a> {
    host: &'a mut dyn ServerHost,
    /// The player who sent the command; None for the console, which has no level of its own.
    sender: Option<PlayerRef>,
    /// The replies in order.
    pub replies: Vec<Text>,
}

impl<'a> Live<'a> {
    pub fn new(host: &'a mut dyn ServerHost, sender: Option<PlayerRef>) -> Self {
        Self {
            host,
            sender,
            replies: Vec::new(),
        }
    }
}

impl Reads for Live<'_> {
    fn owner(&self) -> Option<u128> {
        self.host.dev_server_owner()
    }
    fn dimension(&self) -> Option<String> {
        let level = self.sender?.level(&*self.host)?;
        self.host.dev_level_dimension(level)
    }
    fn select_ids(&mut self, selector: &str) -> Vec<u32> {
        self.host.dev_select_ids(selector)
    }
    fn is_mob(&self, id: u32) -> bool {
        let Some(entity) = self.host.entity(id) else {
            return false;
        };
        let Some(mob) = self.host.mob(entity) else {
            return false;
        };
        mob.living().alive(&*self.host) == Some(true)
    }
    fn entity_name(&self, id: u32) -> Option<String> {
        self.host.entity(id)?.name(&*self.host)
    }
    fn block_pos(&self) -> Option<[i32; 3]> {
        let entity = self.sender?.entity();
        mod_api::__private::backend_read(&*self.host, |backend| {
            backend.entity_block_position(entity)
        })
        .flatten()
    }
    fn run_lua(&mut self, source: &str) -> LuaRun {
        let run = runner::console().run_chunk(&mut *self.host, "lua", source);
        LuaRun {
            lines: run.lines,
            value: run.value,
            error: run.error,
        }
    }
    fn reload(&mut self, id: &str) -> ReloadOutcome {
        use event_horizon_lua::reload::ReloadError as E;
        match event_horizon_lua::reload::reload(id) {
            Ok(done) => ReloadOutcome::Reloaded {
                id: done.id,
                scripts: done.scripts,
            },
            Err(E::NotLoaded) => ReloadOutcome::NotLoaded,
            Err(E::NotADevMod) => ReloadOutcome::NotADevMod,
            Err(E::Busy) => ReloadOutcome::Busy,
            Err(E::Declaration(what)) => ReloadOutcome::Refused(what),
            Err(other) => ReloadOutcome::Failed(other.to_string()),
        }
    }
    fn reloadable(&self) -> Vec<String> {
        event_horizon_lua::reload::reloadable()
    }
    fn reply(&mut self, line: Text) {
        self.replies.push(line);
    }
}

/// Whether `sender` is the world's owner: the only player the tools' commands serve. None (a
/// dedicated server, or a world no single player owns) serves nobody.
pub fn is_owner(owner: Option<u128>, sender: u128) -> bool {
    owner == Some(sender)
}

/// The command line a tree usage's words and remainder make, as the chat line reads it.
pub fn join_line(literals: &[String], rest: Option<&str>) -> String {
    let mut line = literals.join(" ");
    if let Some(rest) = rest.filter(|r| !r.is_empty()) {
        line.push(' ');
        line.push_str(rest);
    }
    line
}

/// The line a tree invocation was typed as.
pub fn invocation_line(invocation: &CommandInvocation) -> String {
    let rest = match invocation.argument("rest") {
        Some(CommandArgumentValue::String(rest)) => Some(rest.as_str()),
        _ => None,
    };
    join_line(&invocation.literals, rest)
}

/// Serves `line` from `sender`: the owner's command runs, anyone else is refused with the owner's
/// reply. Returns false when `line` is not one of the tools' commands (nothing is said then).
pub fn serve(reads: &mut dyn Reads, sender: u128, line: &str) -> bool {
    let Some(cmd) = parse(line) else {
        return false;
    };
    if is_owner(reads.owner(), sender) {
        run(reads, cmd);
    } else {
        reads.reply(failure("horizon.dev_tools.owner_only"));
    }
    true
}

/// The command the line names, as the tree's words: the name, then what follows it. None for a
/// line that is not one of the tools' commands.
fn parse(line: &str) -> Option<Cmd<'_>> {
    let (name, rest) = first_word(line.trim());
    match name {
        "inspect" | "goals" | "senses" | "nav" => {
            let view = match name {
                "inspect" => View::Data,
                "goals" => View::Goals,
                "senses" => View::Senses,
                _ => View::Nav,
            };
            match rest {
                // `/inspect` alone lists what is watched.
                "" if name == "inspect" => Some(Cmd::List),
                "" => None,
                "clear" => Some(Cmd::View {
                    view,
                    clear: true,
                    targets: "",
                }),
                targets => Some(Cmd::View {
                    view,
                    clear: false,
                    targets,
                }),
            }
        }
        "watch" => match rest {
            "" => None,
            "clear" => Some(Cmd::Watch {
                clear: true,
                targets: "",
                path: "",
            }),
            rest => {
                // The path is the last word, the targets the rest.
                let (targets, path) = rest.rsplit_once(char::is_whitespace)?;
                Some(Cmd::Watch {
                    clear: false,
                    targets: targets.trim(),
                    path: path.trim(),
                })
            }
        },
        "mobai" => {
            let (word, rest) = first_word(rest);
            if word != "log" {
                return None;
            }
            match rest {
                "" => None,
                "stop" => Some(Cmd::MobaiLog {
                    stop: true,
                    targets: "",
                }),
                targets => Some(Cmd::MobaiLog {
                    stop: false,
                    targets,
                }),
            }
        }
        "blockinfo" => Some(Cmd::Block {
            action: match rest {
                "" => BlockAction::List,
                "clear" => BlockAction::Clear,
                text => parse_coords(text).map_or(BlockAction::Usage, BlockAction::Watch),
            },
        }),
        "lua" => Some(Cmd::Lua { code: rest }),
        "luareload" => Some(Cmd::Reload { id: rest }),
        "console" => match rest {
            "" => Some(Cmd::Console {
                action: console::Action::Open,
            }),
            "close" => Some(Cmd::Console {
                action: console::Action::Close,
            }),
            "clear" => Some(Cmd::Console {
                action: console::Action::Clear,
            }),
            _ => None,
        },
        _ => None,
    }
}

/// The first word of `s` and what follows it.
fn first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

/// Runs the command the line named.
fn run(reads: &mut dyn Reads, cmd: Cmd<'_>) {
    match cmd {
        Cmd::Lua { code } => lua(reads, code),
        Cmd::Reload { id } => reload(reads, id),
        Cmd::Console { action } => console::act(action),
        Cmd::List => list(reads),
        Cmd::View {
            view,
            clear,
            targets,
        } => open_view(reads, view, clear, targets),
        Cmd::Watch {
            clear,
            targets,
            path,
        } => watch(reads, clear, targets, path),
        Cmd::MobaiLog { stop, targets } => mobai_log(reads, stop, targets),
        Cmd::Block { action } => block_info(reads, action),
    }
}

/// The window's name, as the messages name it.
fn view_name(view: View) -> &'static str {
    match view {
        View::Data => "inspect",
        View::Goals => "goals",
        View::Senses => "senses",
        View::Nav => "nav",
    }
}

/// `key` with `args`, or `key.one` when `n` is one: the two forms of a counted line, as the
/// language file gives them.
fn counted(key: &str, n: usize, args: &[String]) -> Text {
    if n == 1 {
        tr(&format!("{key}.one"), args)
    } else {
        tr(key, args)
    }
}

/// `/lua <code>`: runs the chunk, then replies what it printed (the last [`LUA_LINES`], and a line
/// that counts the rest), its value as `= <value>`, and its error in red.
fn lua(reads: &mut dyn Reads, code: &str) {
    if code.is_empty() {
        reads.reply(tr("horizon.dev_tools.lua.usage", &[]));
        return;
    }
    let run = reads.run_lua(code);
    if run.lines.is_empty() && run.value.is_none() && run.error.is_none() {
        reads.reply(tr("horizon.dev_tools.lua.done", &[]));
        return;
    }
    let skipped = run.lines.len().saturating_sub(LUA_LINES);
    for line in &run.lines[skipped..] {
        reads.reply(Text::literal(cut(line)));
    }
    if skipped > 0 {
        reads.reply(tr("horizon.dev_tools.lua.more", &[skipped.to_string()]));
    }
    if let Some(value) = &run.value {
        reads.reply(Text::literal(cut(&format!("= {value}"))));
    }
    if let Some(error) = &run.error {
        reads.reply(red(Text::literal(cut(error))));
    }
}

/// `/luareload`: the ids that reload when `id` is empty, else the reload of `id`, replied as its outcome.
fn reload(reads: &mut dyn Reads, id: &str) {
    if id.is_empty() {
        let ids = reads.reloadable();
        let line = if ids.is_empty() {
            tr("horizon.dev_tools.reload.none", &[])
        } else {
            tr("horizon.dev_tools.reload.list", &[ids.join(", ")])
        };
        reads.reply(line);
        return;
    }
    let outcome = reads.reload(id);
    for line in reload_replies(id, &outcome) {
        reads.reply(line);
    }
}

/// The reply lines of a reload's outcome for `id`: the success line, or the red lines that say why
/// the scripts were not swapped.
pub fn reload_replies(id: &str, outcome: &ReloadOutcome) -> Vec<Text> {
    let id = id.to_owned();
    match outcome {
        ReloadOutcome::Reloaded { scripts, .. } => vec![tr(
            "horizon.dev_tools.reload.done",
            &[id, scripts.to_string()],
        )],
        ReloadOutcome::NotLoaded => vec![red(tr("horizon.dev_tools.reload.not_loaded", &[id]))],
        ReloadOutcome::NotADevMod => vec![red(tr("horizon.dev_tools.reload.not_dev", &[id]))],
        ReloadOutcome::Busy => vec![red(tr("horizon.dev_tools.reload.busy", &[id]))],
        ReloadOutcome::Failed(why) => {
            vec![red(tr(
                "horizon.dev_tools.reload.failed",
                &[id, why.clone()],
            ))]
        }
        ReloadOutcome::Refused(what) => vec![
            red(tr(
                "horizon.dev_tools.reload.refused",
                &[id, what.join("; ")],
            )),
            red(tr("horizon.dev_tools.reload.restart", &[])),
        ],
    }
}

/// `line` cut to [`LUA_LINE_CHARS`] characters: the first ones and an ellipsis when it is longer.
fn cut(line: &str) -> String {
    if line.chars().count() <= LUA_LINE_CHARS {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(LUA_LINE_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The ids the selector names, as the owner's command finds them. None when it names nothing.
fn target_ids(reads: &mut dyn Reads, selector: &str) -> Option<Vec<u32>> {
    let ids = reads.select_ids(selector);
    (!ids.is_empty()).then_some(ids)
}

/// The store's entries for `ids`, each with the sender's dimension.
fn entries(ids: &[u32], dimension: &Option<String>) -> Vec<(u32, Option<String>)> {
    ids.iter().map(|&id| (id, dimension.clone())).collect()
}

/// `/inspect|goals|senses|nav <targets>`: open that window for them; `… clear` closes it everywhere.
fn open_view(reads: &mut dyn Reads, view: View, clear: bool, targets: &str) {
    let name = view_name(view);
    if clear {
        observe::clear(Some(view));
        observe::forget_unwatched();
        reads.reply(tr("horizon.dev_tools.closed", &[name.to_owned()]));
        return;
    }
    let Some(ids) = target_ids(reads, targets) else {
        reads.reply(failure("argument.entity.notfound.entity"));
        return;
    };
    let dimension = reads.dimension();
    observe::remember(&entries(&ids, &dimension));
    observe::add(&ids, view);
    reads.reply(counted(
        "horizon.dev_tools.opened",
        ids.len(),
        &[name.to_owned(), ids.len().to_string()],
    ));
}

/// `/inspect` alone: what is watched, and how to use the tools.
fn list(reads: &mut dyn Reads) {
    let watched = observe::snapshots();
    if watched.is_empty() {
        reads.reply(tr("horizon.dev_tools.watching_nothing", &[]));
        return;
    }
    let names: Vec<String> = watched
        .iter()
        .map(|w| {
            let name = w
                .data
                .as_ref()
                .map_or_else(|| lang::get("horizon.dev_tools.entity"), |d| d.name.clone());
            let views: Vec<&str> = w
                .views
                .iter()
                .map(|v| view_name(*v))
                .chain(w.series.iter().map(|s| s.path.as_str()))
                .collect();
            format!("{name} #{} [{}]", w.id, views.join(", "))
        })
        .collect();
    reads.reply(tr("horizon.dev_tools.watching", &[names.join("; ")]));
}

/// `/watch <targets> <path>`: a numeric NBT path over time; `/watch clear`.
fn watch(reads: &mut dyn Reads, clear: bool, targets: &str, path: &str) {
    if clear {
        observe::clear_paths();
        observe::forget_unwatched();
        reads.reply(tr("horizon.dev_tools.closed", &["watch".to_owned()]));
        return;
    }
    if let Err(e) = NbtPath::parse(path) {
        let message = data_cmd::path_error(e).message.plain();
        reads.reply(red(Text::literal(message)));
        return;
    }
    let Some(ids) = target_ids(reads, targets) else {
        reads.reply(failure("argument.entity.notfound.entity"));
        return;
    };
    let dimension = reads.dimension();
    observe::remember(&entries(&ids, &dimension));
    observe::add_path(&ids, path);
    reads.reply(counted(
        "horizon.dev_tools.watch",
        ids.len(),
        &[path.to_owned(), ids.len().to_string()],
    ));
}

/// `/mobai log <targets>`: each tick, the mob's AI snapshot as a JSON line in `dev-tools/logs/`;
/// `/mobai log stop` closes the files.
fn mobai_log(reads: &mut dyn Reads, stop: bool, targets: &str) {
    if stop {
        let n = observe::stop_logs();
        observe::forget_unwatched();
        reads.reply(counted(
            "horizon.dev_tools.logs_closed",
            n,
            &[n.to_string()],
        ));
        return;
    }
    let dimension = reads.dimension();
    let mobs: Vec<u32> = reads
        .select_ids(targets)
        .into_iter()
        .filter(|&id| reads.is_mob(id))
        .collect();
    if mobs.is_empty() {
        reads.reply(failure("argument.entity.notfound.entity"));
        return;
    }
    let mut opened = Vec::new();
    for &id in &mobs {
        let name = reads
            .entity_name(id)
            .unwrap_or_else(|| lang::get("horizon.dev_tools.entity"));
        match observe::open_log(&name, id) {
            Ok(Some(path)) => opened.push(path),
            Ok(None) => {}
            Err(e) => {
                reads.reply(red(Text::literal(e)));
                return;
            }
        }
    }
    observe::remember(&entries(&mobs, &dimension));
    reads.reply(if opened.is_empty() {
        tr("horizon.dev_tools.already_logging", &[])
    } else {
        tr("horizon.dev_tools.logging_to", &[opened.join(", ")])
    });
}

/// `/blockinfo`: the block windows, as `action` says. A new window is in the owner's dimension.
fn block_info(reads: &mut dyn Reads, action: BlockAction) {
    match action {
        BlockAction::List => {
            let watched = observe::blocks();
            if watched.is_empty() {
                reads.reply(tr("horizon.dev_tools.block.none", &[]));
                return;
            }
            let names: Vec<String> = watched
                .iter()
                .map(|b| {
                    format!(
                        "{} in {}",
                        crate::windows::block_label(b.key.pos),
                        b.key.dimension
                    )
                })
                .collect();
            reads.reply(tr("horizon.dev_tools.block.watching", &[names.join("; ")]));
        }
        BlockAction::Clear => {
            for block in observe::blocks() {
                observe::remove_block(&block.key);
            }
            observe::forget_unwatched();
            reads.reply(tr("horizon.dev_tools.block.closed", &[]));
        }
        BlockAction::Usage => reads.reply(tr("horizon.dev_tools.block.usage", &[])),
        BlockAction::Watch(coords) => {
            let Some(pos) = resolve(coords, reads.block_pos()) else {
                reads.reply(failure("horizon.dev_tools.block.no_position"));
                return;
            };
            let Some(dimension) = reads.dimension() else {
                reads.reply(failure("horizon.dev_tools.block.no_level"));
                return;
            };
            let label = crate::windows::block_label(pos);
            observe::add_block(
                BlockKey {
                    dimension: dimension.clone(),
                    pos,
                },
                [0, 1, 0],
            );
            reads.reply(tr("horizon.dev_tools.block.opened", &[label, dimension]));
        }
    }
}

/// A tree usage's line from `source`: a player's is served as `serve` serves it; a console's
/// (the dedicated server or RCON) is refused, and the runner is never reached.
fn serve_from(reads: &mut dyn Reads, source: &CommandSource, line: &str) {
    match source {
        CommandSource::Player { player, .. } => {
            serve(reads, player.uuid(), line);
        }
        _ => reads.reply(failure("horizon.dev_tools.owner_only")),
    }
}

/// The hook side: a command a tree usage ran, from its sender. The owner's rule is in `serve`.
pub fn run_tree_command(
    host: &mut dyn ServerHost,
    source: &CommandSource,
    invocation: &CommandInvocation,
) -> Vec<Text> {
    let sender = match source {
        CommandSource::Player { player, .. } => Some(*player),
        _ => None,
    };
    let mut live = Live::new(host, sender);
    serve_from(&mut live, source, &invocation_line(invocation));
    live.replies
}

/// The hook side: a player's chat command. Only the owner's own commands are served here; any
/// other command goes on to the tree, which refuses a guest.
pub fn player_command(host: &mut dyn ServerHost, sender: u128, line: &str) -> Option<Vec<Text>> {
    if !is_owner(host.dev_server_owner(), sender) {
        return None;
    }
    let player = host.player(sender)?;
    let mut live = Live::new(host, Some(player));
    if !serve(&mut live, sender, line) {
        return None;
    }
    Some(live.replies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_of_the_tree_are_the_tools_commands() {
        assert_eq!(parse("inspect"), Some(Cmd::List));
        assert!(matches!(
            parse("inspect clear"),
            Some(Cmd::View { clear: true, .. })
        ));
        assert!(matches!(
            parse("inspect @e[type=zombie]"),
            Some(Cmd::View { clear: false, .. })
        ));
        assert!(matches!(
            parse("goals @e[type=cow]"),
            Some(Cmd::View {
                view: View::Goals,
                ..
            })
        ));
        assert!(matches!(
            parse("mobai log stop"),
            Some(Cmd::MobaiLog { stop: true, .. })
        ));
        assert!(matches!(
            parse("mobai log @e"),
            Some(Cmd::MobaiLog { stop: false, .. })
        ));
    }

    #[test]
    fn the_lines_of_other_commands_are_not_ours() {
        for line in [
            "",
            "give",
            "inspectx",
            "mobai",
            "mobai logs",
            "watch",
            "goals",
            "luau",
        ] {
            assert!(parse(line).is_none(), "{line}");
        }
    }

    #[test]
    fn a_lua_line_is_its_code_and_keeps_the_code_as_typed() {
        assert_eq!(parse("lua print(1)"), Some(Cmd::Lua { code: "print(1)" }));
        assert_eq!(
            parse("lua print(\"a\");  print(\"b\")"),
            Some(Cmd::Lua {
                code: "print(\"a\");  print(\"b\")"
            }),
            "the code keeps its inner spacing"
        );
        assert_eq!(parse("lua"), Some(Cmd::Lua { code: "" }));
    }

    #[test]
    fn a_watch_line_ends_with_its_path() {
        assert_eq!(
            parse("watch @e[type=zombie] Health"),
            Some(Cmd::Watch {
                clear: false,
                targets: "@e[type=zombie]",
                path: "Health",
            })
        );
    }

    #[test]
    fn the_tree_usage_is_its_words_and_its_remainder() {
        assert_eq!(join_line(&["inspect".to_owned()], None), "inspect");
        assert_eq!(
            join_line(
                &["mobai".to_owned(), "log".to_owned()],
                Some("@e[type=zombie]")
            ),
            "mobai log @e[type=zombie]"
        );
        assert_eq!(
            join_line(&["inspect".to_owned()], Some("")),
            "inspect",
            "an empty remainder adds nothing"
        );
    }

    #[test]
    fn only_the_owner_is_the_owner() {
        assert!(is_owner(Some(7), 7));
        assert!(!is_owner(Some(7), 8), "a guest or an operator is refused");
        assert!(!is_owner(None, 7), "a dedicated server refuses everyone");
        assert!(!is_owner(None, 0));
    }

    #[test]
    fn the_usages_register_every_command() {
        for word in ["inspect", "goals", "senses", "nav", "watch", "mobai"] {
            assert!(
                USAGES.iter().any(|u| u.starts_with(word)),
                "{word} is registered"
            );
        }
        assert!(USAGES.contains(&"lua"));
        assert!(USAGES.contains(&"lua <rest:text>"));
        assert!(USAGES.contains(&"console"));
        assert!(USAGES.contains(&"console close"));
        assert!(USAGES.contains(&"console clear"));
        assert!(USAGES.contains(&"blockinfo"));
        assert!(USAGES.contains(&"blockinfo clear"));
        assert!(USAGES.contains(&"blockinfo <rest:text>"));
    }

    #[test]
    fn a_console_line_is_open_close_or_clear_and_nothing_else() {
        assert_eq!(
            parse("console"),
            Some(Cmd::Console {
                action: console::Action::Open
            })
        );
        assert_eq!(
            parse("console close"),
            Some(Cmd::Console {
                action: console::Action::Close
            })
        );
        assert_eq!(
            parse("console clear"),
            Some(Cmd::Console {
                action: console::Action::Clear
            })
        );
        assert!(parse("console print(1)").is_none());
    }
}

#[cfg(test)]
mod block_lines {
    use super::*;

    #[test]
    fn coordinates_are_integers_negatives_tildes_and_tilde_offsets() {
        assert_eq!(
            parse_coords("1 -2 3"),
            Some([Coord::Abs(1), Coord::Abs(-2), Coord::Abs(3)])
        );
        assert_eq!(
            parse_coords("~ ~5 ~-2"),
            Some([Coord::Rel(0), Coord::Rel(5), Coord::Rel(-2)])
        );
        assert_eq!(
            parse_coords("  ~1   64  ~ "),
            Some([Coord::Rel(1), Coord::Abs(64), Coord::Rel(0)]),
            "extra spaces do not matter"
        );
    }

    #[test]
    fn bad_coordinates_are_refused() {
        for text in ["", "1 2", "1 2 3 4", "1 x 3", "~x 1 1", "1.5 2 3", "~~ 1 1"] {
            assert_eq!(parse_coords(text), None, "{text:?}");
        }
    }

    #[test]
    fn relative_coordinates_count_from_the_owner_block() {
        assert_eq!(
            resolve(
                [Coord::Rel(0), Coord::Rel(-1), Coord::Rel(0)],
                Some([1, 2, 3])
            ),
            Some([1, 1, 3])
        );
        assert_eq!(
            resolve([Coord::Abs(5), Coord::Abs(6), Coord::Abs(7)], None),
            Some([5, 6, 7]),
            "absolute ones need no owner"
        );
        assert_eq!(
            resolve([Coord::Rel(0), Coord::Abs(1), Coord::Abs(1)], None),
            None,
            "a relative one needs the owner's block"
        );
        assert_eq!(
            resolve(
                [Coord::Rel(1), Coord::Abs(0), Coord::Abs(0)],
                Some([i32::MAX, 0, 0])
            ),
            None,
            "an overflowing sum is refused"
        );
    }

    #[test]
    fn the_blockinfo_line_is_list_clear_watch_or_usage() {
        assert_eq!(
            parse("blockinfo"),
            Some(Cmd::Block {
                action: BlockAction::List
            })
        );
        assert_eq!(
            parse("blockinfo clear"),
            Some(Cmd::Block {
                action: BlockAction::Clear
            })
        );
        assert_eq!(
            parse("blockinfo ~ ~-1 ~"),
            Some(Cmd::Block {
                action: BlockAction::Watch([Coord::Rel(0), Coord::Rel(-1), Coord::Rel(0)])
            })
        );
        assert_eq!(
            parse("blockinfo 1 two 3"),
            Some(Cmd::Block {
                action: BlockAction::Usage
            }),
            "bad input is the usage, not an unknown line"
        );
    }
}

#[cfg(test)]
mod flows {
    use super::*;

    /// The server's reads as a test sets them: the owner, the level's dimension, what a selector
    /// names and which of those are mobs.
    #[derive(Default)]
    struct Fake {
        /// Held for the test: the store is process-wide, and the inspect tests share it.
        _store: Option<std::sync::MutexGuard<'static, ()>>,
        owner: Option<u128>,
        dimension: Option<String>,
        selected: Vec<u32>,
        mobs: Vec<u32>,
        replies: Vec<Text>,
        /// What the runner gives back for any chunk.
        lua_run: LuaRun,
        /// The chunks the runner was called with, in order.
        lua_sources: Vec<String>,
        /// The ids `reload` was called with, in order; what it answers; the ids it lists.
        reloads: Vec<String>,
        reload_result: ReloadOutcome,
        reload_ids: Vec<String>,
        /// The owner's block, for `~` coordinates.
        block_pos: Option<[i32; 3]>,
    }

    impl Reads for Fake {
        fn owner(&self) -> Option<u128> {
            self.owner
        }
        fn dimension(&self) -> Option<String> {
            self.dimension.clone()
        }
        fn select_ids(&mut self, _selector: &str) -> Vec<u32> {
            self.selected.clone()
        }
        fn is_mob(&self, id: u32) -> bool {
            self.mobs.contains(&id)
        }
        fn entity_name(&self, id: u32) -> Option<String> {
            Some(format!("Zombie{id}"))
        }
        fn block_pos(&self) -> Option<[i32; 3]> {
            self.block_pos
        }
        fn run_lua(&mut self, source: &str) -> LuaRun {
            self.lua_sources.push(source.to_owned());
            self.lua_run.clone()
        }
        fn reload(&mut self, id: &str) -> ReloadOutcome {
            self.reloads.push(id.to_owned());
            self.reload_result.clone()
        }
        fn reloadable(&self) -> Vec<String> {
            self.reload_ids.clone()
        }
        fn reply(&mut self, line: Text) {
            self.replies.push(line);
        }
    }

    /// The text of a reply, as the chat shows it: a literal's content, with its colour if any.
    fn plain(line: &Text) -> &str {
        match &line.content {
            mod_api::text::TextContent::Literal(text) => text,
            other => panic!("a reply is plain text, not {other:?}"),
        }
    }

    fn printed(lines: &[&str]) -> LuaRun {
        LuaRun {
            lines: lines.iter().map(|l| (*l).to_owned()).collect(),
            ..LuaRun::default()
        }
    }

    fn owned_by(owner: Option<u128>, selected: Vec<u32>) -> Fake {
        let store = crate::inspect::store_guard();
        observe::reset();
        Fake {
            _store: Some(store),
            owner,
            dimension: Some("minecraft:overworld".to_owned()),
            selected,
            ..Fake::default()
        }
    }

    #[test]
    fn the_owner_watches_a_path_and_the_store_holds_it() {
        let mut fake = owned_by(Some(7), vec![3, 4]);
        assert!(serve(&mut fake, 7, "watch @e[type=zombie] Health"));
        assert_eq!(observe::paths_of(3), vec!["Health".to_owned()]);
        assert_eq!(observe::paths_of(4), vec!["Health".to_owned()]);
        assert_eq!(
            fake.replies,
            vec![Text::translate(
                "horizon.dev_tools.watch",
                vec![Text::literal("Health"), Text::literal("2")]
            )]
        );
    }

    #[test]
    fn the_owner_opens_one_window_and_the_one_entity_line_is_its_reply() {
        let mut fake = owned_by(Some(7), vec![5]);
        assert!(serve(&mut fake, 7, "inspect @e[limit=1]"));
        assert!(observe::watched().contains(&5));
        assert_eq!(
            fake.replies,
            vec![Text::translate(
                "horizon.dev_tools.opened.one",
                vec![Text::literal("inspect"), Text::literal("1")]
            )]
        );
    }

    #[test]
    fn a_guest_or_an_operator_is_refused_and_the_store_is_untouched() {
        let mut fake = owned_by(Some(7), vec![3]);
        assert!(serve(&mut fake, 8, "watch @e Health"));
        assert!(observe::watched().is_empty());
        assert!(observe::paths_of(3).is_empty());
        assert_eq!(fake.replies, vec![failure("horizon.dev_tools.owner_only")]);
    }

    #[test]
    fn a_dedicated_server_refuses_everyone() {
        let mut fake = owned_by(None, vec![3]);
        assert!(serve(&mut fake, 7, "inspect @e"));
        assert!(observe::watched().is_empty());
        assert_eq!(fake.replies, vec![failure("horizon.dev_tools.owner_only")]);
    }

    #[test]
    fn a_selector_naming_nothing_says_so_and_watches_nothing() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "goals @e[type=nothing]"));
        assert!(observe::watched().is_empty());
        assert_eq!(
            fake.replies,
            vec![failure("argument.entity.notfound.entity")]
        );
    }

    #[test]
    fn a_bad_nbt_path_is_refused_in_red_and_watches_nothing() {
        let mut fake = owned_by(Some(7), vec![3]);
        assert!(serve(&mut fake, 7, "watch @e Health["));
        assert!(observe::watched().is_empty());
        assert_eq!(fake.replies.len(), 1);
    }

    #[test]
    fn stopping_no_logs_says_how_many_it_closed() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "mobai log stop"));
        assert_eq!(
            fake.replies,
            vec![Text::translate(
                "horizon.dev_tools.logs_closed",
                vec![Text::literal("0")]
            )]
        );
    }

    #[test]
    fn the_owner_watches_a_block_in_the_owners_dimension() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "blockinfo 4 -64 9"));
        assert_eq!(
            observe::blocks()
                .iter()
                .map(|b| b.key.clone())
                .collect::<Vec<_>>(),
            vec![BlockKey {
                dimension: "minecraft:overworld".to_owned(),
                pos: [4, -64, 9],
            }]
        );
        assert_eq!(
            fake.replies,
            vec![Text::translate(
                "horizon.dev_tools.block.opened",
                vec![
                    Text::literal("4 -64 9"),
                    Text::literal("minecraft:overworld")
                ]
            )]
        );
    }

    #[test]
    fn a_relative_block_is_counted_from_the_owner() {
        let mut fake = owned_by(Some(7), Vec::new());
        fake.block_pos = Some([10, 64, -3]);
        assert!(serve(&mut fake, 7, "blockinfo ~ ~-1 ~5"));
        assert_eq!(observe::blocks()[0].key.pos, [10, 63, 2]);
    }

    #[test]
    fn a_relative_block_with_no_owner_position_says_so_and_watches_nothing() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "blockinfo ~ 1 1"));
        assert!(observe::blocks().is_empty());
        assert_eq!(
            fake.replies,
            vec![failure("horizon.dev_tools.block.no_position")]
        );
    }

    #[test]
    fn bad_coordinates_reply_the_usage_and_watch_nothing() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "blockinfo 1 two 3"));
        assert!(observe::blocks().is_empty());
        assert_eq!(
            fake.replies,
            vec![Text::translate("horizon.dev_tools.block.usage", vec![])]
        );
    }

    #[test]
    fn blockinfo_alone_lists_the_watched_blocks_and_clear_closes_them() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "blockinfo"));
        assert_eq!(
            fake.replies,
            vec![Text::translate("horizon.dev_tools.block.none", vec![])]
        );
        assert!(serve(&mut fake, 7, "blockinfo 1 2 3"));
        assert!(serve(&mut fake, 7, "blockinfo 4 5 6"));
        fake.replies.clear();
        assert!(serve(&mut fake, 7, "blockinfo"));
        assert_eq!(
            fake.replies,
            vec![Text::translate(
                "horizon.dev_tools.block.watching",
                vec![Text::literal(
                    "1 2 3 in minecraft:overworld; 4 5 6 in minecraft:overworld"
                )]
            )]
        );
        fake.replies.clear();
        assert!(serve(&mut fake, 7, "blockinfo clear"));
        assert!(observe::blocks().is_empty());
        assert_eq!(
            fake.replies,
            vec![Text::translate("horizon.dev_tools.block.closed", vec![])]
        );
    }

    #[test]
    fn a_guest_or_a_dedicated_server_never_watches_a_block() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 8, "blockinfo 1 2 3"));
        assert!(observe::blocks().is_empty(), "a guest is refused");
        assert_eq!(fake.replies, vec![failure("horizon.dev_tools.owner_only")]);

        fake.owner = None;
        assert!(serve(&mut fake, 7, "blockinfo 1 2 3"));
        assert!(
            observe::blocks().is_empty(),
            "a dedicated server refuses everyone"
        );
    }

    #[test]
    fn a_console_source_never_watches_a_block() {
        let mut fake = owned_by(Some(7), Vec::new());
        let console = CommandSource::Console {
            permission: mod_api::PermissionLevel::Admins,
        };
        serve_from(&mut fake, &console, "blockinfo 1 2 3");
        assert!(observe::blocks().is_empty());
        assert_eq!(fake.replies, vec![failure("horizon.dev_tools.owner_only")]);
    }

    #[test]
    fn a_line_of_another_command_is_not_served() {
        let mut fake = owned_by(Some(7), vec![3]);
        assert!(!serve(&mut fake, 7, "give @s apple"));
        assert!(fake.replies.is_empty());
    }

    #[test]
    fn the_owner_runs_a_lua_chunk_once_and_a_guest_never_reaches_the_runner() {
        // One fake: `owned_by` holds the process-wide store lock, so a second one would deadlock.
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "lua print(1+1)"));
        assert_eq!(fake.lua_sources, vec!["print(1+1)".to_owned()]);

        assert!(serve(&mut fake, 8, "lua print(1+1)"));
        assert_eq!(
            fake.lua_sources.len(),
            1,
            "the guest's chunk never reaches the runner"
        );
        assert_eq!(
            fake.replies.last(),
            Some(&failure("horizon.dev_tools.owner_only"))
        );
    }

    #[test]
    fn the_owner_reloads_by_id_and_a_guest_never_reaches_the_reloader() {
        let mut fake = owned_by(Some(7), Vec::new());
        fake.reload_result = ReloadOutcome::Reloaded {
            id: "mod_a".to_owned(),
            scripts: 2,
        };
        assert!(serve(&mut fake, 7, "luareload mod_a"));
        assert_eq!(fake.reloads, ["mod_a"]);
        assert_eq!(
            fake.replies,
            vec![Text::translate(
                "horizon.dev_tools.reload.done",
                vec![Text::literal("mod_a"), Text::literal("2")]
            )]
        );

        assert!(serve(&mut fake, 8, "luareload mod_a"));
        assert_eq!(fake.reloads.len(), 1, "the guest's reload never runs");
        assert_eq!(
            fake.replies.last(),
            Some(&failure("horizon.dev_tools.owner_only"))
        );
    }

    #[test]
    fn a_dedicated_server_and_a_console_source_never_reload() {
        // One fake: `owned_by` holds the process-wide store lock, so a second one would deadlock.
        let mut fake = owned_by(None, Vec::new());
        assert!(serve(&mut fake, 7, "luareload mod_a"));
        assert!(fake.reloads.is_empty());
        assert_eq!(fake.replies, vec![failure("horizon.dev_tools.owner_only")]);

        fake.owner = Some(7);
        fake.replies.clear();
        let console = CommandSource::Console {
            permission: mod_api::PermissionLevel::Admins,
        };
        serve_from(&mut fake, &console, "luareload mod_a");
        assert!(fake.reloads.is_empty(), "the console is refused, as before");
    }

    #[test]
    fn reload_alone_lists_the_reloadable_ids_or_says_there_are_none() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "luareload"));
        assert_eq!(
            fake.replies,
            vec![Text::translate("horizon.dev_tools.reload.none", vec![])]
        );
        assert!(fake.reloads.is_empty());

        fake.replies.clear();
        fake.reload_ids = vec!["mod_a".to_owned(), "mod_b".to_owned()];
        assert!(serve(&mut fake, 7, "luareload"));
        assert_eq!(
            fake.replies,
            vec![Text::translate(
                "horizon.dev_tools.reload.list",
                vec![Text::literal("mod_a, mod_b")]
            )]
        );
        assert!(fake.reloads.is_empty());
    }

    #[test]
    fn each_reload_outcome_has_its_reply_and_the_failures_are_red() {
        let done = reload_replies(
            "mod_a",
            &ReloadOutcome::Reloaded {
                id: "mod_a".to_owned(),
                scripts: 1,
            },
        );
        assert_eq!(
            done,
            vec![Text::translate(
                "horizon.dev_tools.reload.done",
                vec![Text::literal("mod_a"), Text::literal("1")]
            )]
        );
        assert_eq!(done[0].style.color, None, "a success is not red");

        let cases = [
            (
                ReloadOutcome::NotLoaded,
                "horizon.dev_tools.reload.not_loaded",
            ),
            (
                ReloadOutcome::NotADevMod,
                "horizon.dev_tools.reload.not_dev",
            ),
            (ReloadOutcome::Busy, "horizon.dev_tools.reload.busy"),
            (
                ReloadOutcome::Failed("boom".to_owned()),
                "horizon.dev_tools.reload.failed",
            ),
        ];
        for (outcome, key) in cases {
            let lines = reload_replies("mod_a", &outcome);
            assert_eq!(lines.len(), 1, "{key}");
            assert_eq!(lines[0].style.color.as_deref(), Some("red"), "{key}");
            assert!(
                matches!(&lines[0].content, mod_api::text::TextContent::Translatable { key: k, .. } if k == key),
                "{key}: {:?}",
                lines[0].content
            );
        }

        let refused = reload_replies(
            "mod_a",
            &ReloadOutcome::Refused(vec!["commands: added `mod_a hello`".to_owned()]),
        );
        assert_eq!(refused.len(), 2);
        assert!(
            refused
                .iter()
                .all(|l| l.style.color.as_deref() == Some("red"))
        );
        assert_eq!(
            refused[1].content,
            Text::translate("horizon.dev_tools.reload.restart", vec![]).content,
            "the restart line says what to do"
        );
    }

    #[test]
    fn a_dedicated_server_never_runs_a_lua_chunk() {
        let mut fake = owned_by(None, Vec::new());
        assert!(serve(&mut fake, 7, "lua print(1)"));
        assert!(fake.lua_sources.is_empty());
        assert_eq!(fake.replies, vec![failure("horizon.dev_tools.owner_only")]);
    }

    #[test]
    fn a_console_source_never_runs_a_lua_chunk() {
        let mut fake = owned_by(Some(7), Vec::new());
        let console = CommandSource::Console {
            permission: mod_api::PermissionLevel::Admins,
        };
        serve_from(&mut fake, &console, "lua print(1)");
        assert!(
            fake.lua_sources.is_empty(),
            "the console is refused, as before"
        );
        assert_eq!(fake.replies, vec![failure("horizon.dev_tools.owner_only")]);
    }

    #[test]
    fn lua_alone_replies_the_usage_and_runs_nothing() {
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "lua"));
        assert!(fake.lua_sources.is_empty());
        assert_eq!(
            fake.replies,
            vec![Text::translate("horizon.dev_tools.lua.usage", vec![])]
        );
    }

    #[test]
    fn three_printed_lines_and_a_value_make_four_replies() {
        let mut fake = owned_by(Some(7), Vec::new());
        fake.lua_run = LuaRun {
            value: Some("2".to_owned()),
            ..printed(&["a", "b", "c"])
        };
        serve(&mut fake, 7, "lua print('a')");
        let shown: Vec<&str> = fake.replies.iter().map(plain).collect();
        assert_eq!(shown, vec!["a", "b", "c", "= 2"]);
    }

    #[test]
    fn twenty_five_printed_lines_show_the_last_twenty_and_count_five_more() {
        let mut fake = owned_by(Some(7), Vec::new());
        let lines: Vec<String> = (1..=25).map(|n| format!("line {n}")).collect();
        fake.lua_run = LuaRun {
            lines: lines.clone(),
            ..LuaRun::default()
        };
        serve(&mut fake, 7, "lua loop()");
        assert_eq!(fake.replies.len(), 21);
        assert_eq!(plain(&fake.replies[0]), "line 6");
        assert_eq!(plain(&fake.replies[19]), "line 25");
        assert_eq!(
            fake.replies[20],
            Text::translate("horizon.dev_tools.lua.more", vec![Text::literal("5")])
        );
    }

    #[test]
    fn an_error_is_one_red_line() {
        let mut fake = owned_by(Some(7), Vec::new());
        fake.lua_run = LuaRun {
            error: Some("lua:1: boom".to_owned()),
            ..LuaRun::default()
        };
        serve(&mut fake, 7, "lua error(\"boom\")");
        assert_eq!(fake.replies.len(), 1);
        assert_eq!(plain(&fake.replies[0]), "lua:1: boom");
        assert_eq!(fake.replies[0].style.color.as_deref(), Some("red"));
    }

    #[test]
    fn a_chunk_that_prints_nothing_and_returns_nil_says_ran() {
        let mut fake = owned_by(Some(7), Vec::new());
        serve(&mut fake, 7, "lua local x = 1");
        assert_eq!(
            fake.replies,
            vec![Text::translate("horizon.dev_tools.lua.done", vec![])]
        );
    }

    #[test]
    fn a_300_character_line_is_cut_to_256_and_ends_in_an_ellipsis() {
        let mut fake = owned_by(Some(7), Vec::new());
        let long = "x".repeat(300);
        fake.lua_run = printed(&[long.as_str()]);
        serve(&mut fake, 7, "lua print(string.rep('x', 300))");
        let shown = plain(&fake.replies[0]);
        assert_eq!(shown.chars().count(), 256);
        assert!(shown.ends_with('…'));
        assert_eq!(&shown[..255], "x".repeat(255));
    }

    /// The console window's state, held for one test: the process's state, reset before and after.
    fn console_state() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::console::test_guard();
        *crate::console::state() = crate::console::State::default();
        guard
    }

    #[test]
    fn the_owner_opens_the_console_window_and_asks_for_the_pointer() {
        let _console = console_state();
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "console"));
        assert!(crate::console::state().open);
        assert!(crate::console::state().want_pointer);
        assert!(fake.replies.is_empty(), "opening says nothing");
        *crate::console::state() = crate::console::State::default();
    }

    #[test]
    fn a_guest_never_opens_the_console_and_the_window_state_is_untouched() {
        let _console = console_state();
        let before = crate::console::state().open;
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 8, "console clear"));
        assert!(serve(&mut fake, 8, "console"));
        assert_eq!(crate::console::state().open, before);
        assert!(!crate::console::state().want_pointer);
        assert_eq!(
            fake.replies,
            vec![failure("horizon.dev_tools.owner_only"); 2]
        );
        *crate::console::state() = crate::console::State::default();
    }

    #[test]
    fn a_dedicated_server_and_a_console_source_never_open_the_console() {
        let _console = console_state();
        // One fake only: `owned_by` takes the store lock, which a test may take once.
        let mut fake = owned_by(None, Vec::new());
        assert!(serve(&mut fake, 7, "console"));
        assert!(
            !crate::console::state().open,
            "a dedicated server refuses everyone"
        );
        fake.owner = Some(7);
        let console = CommandSource::Console {
            permission: mod_api::PermissionLevel::Admins,
        };
        serve_from(&mut fake, &console, "console");
        assert!(!crate::console::state().open, "a console source is refused");
        assert_eq!(
            fake.replies,
            vec![failure("horizon.dev_tools.owner_only"); 2]
        );
        *crate::console::state() = crate::console::State::default();
    }

    #[test]
    fn the_owner_clears_the_console_output() {
        let _console = console_state();
        crate::console::state().push(crate::console::Kind::Out, "old");
        let mut fake = owned_by(Some(7), Vec::new());
        assert!(serve(&mut fake, 7, "console clear"));
        assert!(crate::console::state().lines.is_empty());
        *crate::console::state() = crate::console::State::default();
    }
}
