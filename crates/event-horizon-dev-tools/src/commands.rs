//! The developer tools' commands (docs/dev-tools.md) as a native mod's
//! (#34): `/inspect`, `/goals`, `/senses`, `/nav`, `/watch`, `/mobai log`
//! and `/lua` are registered in the server's command tree and run from the
//! mod's hooks, so the engine keeps no command of its own for them.
//!
//! The bodies are the ones the engine had (`game/commands/inspect.rs`
//! before #34): the same messages, the same counts and the same store
//! calls, on the shared [`event_horizon_observe`] store the windows read.
//! The owner's rule is the same too: the tools' commands are the world's
//! owner's, and a guest of any level is refused.
use event_horizon_observe as observe;
use event_horizon_observe::View;
use horizon_client::dev_input;
use horizon_client::dev_input::console;
use horizon_client::mod_api::{
    CommandsEvent, Flow, Host, NativeCommand, PerformCommandEvent, PerformConsoleCommandEvent,
    PlayerCommandEvent, RegisterCommandsEvent,
};
use horizon_client::nbt_path::NbtPath;
use horizon_client::text::{RED, Text};
use horizon_client::{data_cmd, lang};
use std::sync::atomic::Ordering;

/// The usages the tools register in the server's tree. Every player may
/// run them: the owner's rule is in the handlers, not in the tree.
pub const USAGES: &[&str] = &[
    "inspect",
    "inspect <targets:entities>",
    "inspect clear",
    "goals <targets:entities>",
    "goals clear",
    "senses <targets:entities>",
    "senses clear",
    "nav <targets:entities>",
    "nav clear",
    "watch <targets:entities> <path:word>",
    "watch clear",
    "mobai log <targets:entities>",
    "mobai log stop",
    "lua [code:text]",
];

/// The commands' names, for the chat box: a client completes them as it
/// does a mod's commands, before the server sends its own tree.
pub const NAMES: &[&str] = &["inspect", "goals", "senses", "nav", "watch", "mobai", "lua"];

/// One of the tools' commands, as the line names it.
#[derive(Debug)]
enum Cmd<'a> {
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
    /// `/lua` and `/lua <code>`.
    Lua { code: Option<&'a str> },
}

/// The commands the tools add to the server's tree: with the tools on
/// only, for they run the owner's commands and Lua.
pub fn register(event: &mut RegisterCommandsEvent<'_>) {
    if !dev_input::enabled() {
        return;
    }
    for usage in USAGES {
        event.commands.push(NativeCommand::new(usage));
    }
}

/// The commands' names, for the chat box (`commands` hook).
pub fn names(event: &mut CommandsEvent<'_>) {
    if !dev_input::enabled() {
        return;
    }
    for name in NAMES {
        if !event.commands.contains(name) {
            event.commands.push(name);
        }
    }
}

/// A player's command on the server: the tools' own, run for the world's
/// owner. A guest is refused, as every tool's command is.
pub fn perform(host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
    if !dev_input::enabled() {
        return Flow::Continue;
    }
    let Some(cmd) = parse(event.command) else {
        return Flow::Continue;
    };
    if host.server_owner() != Some(event.player.uuid) {
        host.feedback(failure("horizon.dev_tools.owner_only"));
        return Flow::Cancel;
    }
    run(host, cmd);
    Flow::Cancel
}

/// The window's own player's command, as the LAN host's: the host is the
/// owner of its world, so the command is its own. A guest's command on
/// the same game is left to the server's own path, which runs it with the
/// player's state in place.
pub fn player(host: &mut dyn Host, event: &mut PlayerCommandEvent<'_>) -> Flow {
    if !dev_input::enabled() {
        return Flow::Continue;
    }
    if !is_window_player(host, event.player) {
        return Flow::Continue;
    }
    let Some(cmd) = parse(event.command) else {
        return Flow::Continue;
    };
    run(host, cmd);
    Flow::Cancel
}

/// Whether the player `uuid` is the window's own player, and not a guest of
/// the hub: the game knows them (the window's own player has a level of
/// their own) and no guest of the hub is them.
fn is_window_player(host: &dyn Host, uuid: u128) -> bool {
    host.player_permission(uuid).is_some() && !host.server_players().iter().any(|p| p.uuid == uuid)
}

/// The server console's command: the console is the server's owner.
pub fn console(host: &mut dyn Host, event: &mut PerformConsoleCommandEvent<'_>) -> Flow {
    if !dev_input::enabled() {
        return Flow::Continue;
    }
    let Some(cmd) = parse(event.command) else {
        return Flow::Continue;
    };
    run(host, cmd);
    Flow::Cancel
}

/// The command the line names, as the tree's words: the name, then what
/// follows it. None for a line that is not one of the tools' commands.
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
        "lua" => Some(Cmd::Lua {
            code: (!rest.is_empty()).then_some(rest),
        }),
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
fn run(host: &mut dyn Host, cmd: Cmd<'_>) {
    match cmd {
        Cmd::List => list(host),
        Cmd::View {
            view,
            clear,
            targets,
        } => open_view(host, view, clear, targets),
        Cmd::Watch {
            clear,
            targets,
            path,
        } => watch(host, clear, targets, path),
        Cmd::MobaiLog { stop, targets } => mobai_log(host, stop, targets),
        Cmd::Lua { code } => lua(host, code),
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

/// `key` with `args`, or `key.one` when `n` is one: the two forms of a
/// counted line, as the language file gives them.
fn counted(key: &str, n: usize, args: &[&dyn std::fmt::Display]) -> Text {
    if n == 1 {
        Text::tr(&format!("{key}.one"), args)
    } else {
        Text::tr(key, args)
    }
}

/// A line that says the command failed, in red, as a command's failure is.
fn failure(key: &str) -> Text {
    Text::tr(key, &[]).with_color(RED)
}

/// `line`, and a note that this game shows no window when the tools are
/// off on it (a server's, or one started without them).
fn with_tools_note(mut line: Text) -> Text {
    if !dev_input::enabled() {
        line.push(" ");
        line.push(Text::tr("horizon.dev_tools.no_window", &[]));
    }
    line
}

/// The line a window opens with: how many entities it watches, and the
/// players left out when some were.
fn opened(key: &str, what: &str, n: usize, skipped: usize) -> Text {
    let mut line = counted(key, n, &[&what, &n]);
    if skipped > 0 {
        line.push(" ");
        line.push(counted(
            "horizon.dev_tools.players_left_out",
            skipped,
            &[&skipped],
        ));
    }
    with_tools_note(line)
}

/// The ids the selector names, as the owner's command finds them: an
/// entity's id, a player's network entity id. None when it names nothing.
fn target_ids(host: &mut dyn Host, selector: &str) -> Option<Vec<u32>> {
    let ids = host.select_ids(selector);
    (!ids.is_empty()).then_some(ids)
}

/// `/inspect|goals|senses|nav <targets>`: open that window for them;
/// `… clear` closes it everywhere.
fn open_view(host: &mut dyn Host, view: View, clear: bool, targets: &str) {
    let name = view_name(view);
    if clear {
        observe::clear(Some(view));
        observe::forget_unwatched();
        host.feedback(Text::tr("horizon.dev_tools.closed", &[&name]));
        return;
    }
    let Some(ids) = target_ids(host, targets) else {
        host.feedback(failure("argument.entity.notfound.entity"));
        return;
    };
    let dim = host.level_dimension();
    let entries: Vec<(u32, Option<String>)> =
        ids.iter().map(|&id| (id, Some(dim.clone()))).collect();
    observe::remember(&entries);
    observe::add(&ids, view);
    host.feedback(opened("horizon.dev_tools.opened", name, ids.len(), 0));
}

/// `/inspect` alone: what is watched, and how to use the tools.
fn list(host: &mut dyn Host) {
    let watched = observe::snapshots();
    if watched.is_empty() {
        host.feedback(Text::tr("horizon.dev_tools.watching_nothing", &[]));
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
    host.feedback(Text::tr("horizon.dev_tools.watching", &[&names.join("; ")]));
}

/// `/watch <targets> <path>`: a numeric NBT path over time; `/watch clear`.
fn watch(host: &mut dyn Host, clear: bool, targets: &str, path: &str) {
    if clear {
        observe::clear_paths();
        observe::forget_unwatched();
        host.feedback(Text::tr("horizon.dev_tools.closed", &[&"watch"]));
        return;
    }
    if let Err(e) = NbtPath::parse(path) {
        let e = data_cmd::path_error(e);
        host.feedback(e.message.with_color(RED));
        return;
    }
    let Some(ids) = target_ids(host, targets) else {
        host.feedback(failure("argument.entity.notfound.entity"));
        return;
    };
    let dim = host.level_dimension();
    let entries: Vec<(u32, Option<String>)> =
        ids.iter().map(|&id| (id, Some(dim.clone()))).collect();
    observe::remember(&entries);
    observe::add_path(&ids, path);
    host.feedback(opened("horizon.dev_tools.watch", path, ids.len(), 0));
}

/// `/mobai log <targets>`: each tick, the mob's AI snapshot as a JSON
/// line in `dev-tools/logs/`; `/mobai log stop` closes the files.
fn mobai_log(host: &mut dyn Host, stop: bool, targets: &str) {
    if stop {
        let n = observe::stop_logs();
        observe::forget_unwatched();
        host.feedback(counted("horizon.dev_tools.logs_closed", n, &[&n]));
        return;
    }
    let dim = host.level_dimension();
    let ids = host.select_ids(targets);
    let mobs: Vec<u32> = ids
        .into_iter()
        .filter(|&id| host.entity_is_mob(&dim, id))
        .collect();
    if mobs.is_empty() {
        host.feedback(failure("argument.entity.notfound.entity"));
        return;
    }
    let mut opened = Vec::new();
    for &id in &mobs {
        let name = host
            .entity_name_in_place(id)
            .unwrap_or_else(|| lang::get("horizon.dev_tools.entity"));
        match observe::open_log(&name, id) {
            Ok(Some(path)) => opened.push(path),
            Ok(None) => {}
            Err(e) => {
                host.feedback(Text::literal(e).with_color(RED));
                return;
            }
        }
    }
    let entries: Vec<(u32, Option<String>)> =
        mobs.iter().map(|&id| (id, Some(dim.clone()))).collect();
    observe::remember(&entries);
    host.feedback(if opened.is_empty() {
        Text::tr("horizon.dev_tools.already_logging", &[])
    } else {
        Text::tr("horizon.dev_tools.logging_to", &[&opened.join(", ")])
    });
}

/// `/lua`: the console window; `/lua <code>` runs a line in it.
fn lua(host: &mut dyn Host, code: Option<&str>) {
    match code {
        None => {
            let open = !console::OPEN.load(Ordering::Relaxed);
            if open {
                console::open_from_command();
            } else {
                console::OPEN.store(false, Ordering::Relaxed);
            }
            let line = Text::tr(
                if open {
                    "horizon.dev_tools.lua.open"
                } else {
                    "horizon.dev_tools.lua.closed"
                },
                &[],
            );
            host.feedback(with_tools_note(line));
        }
        Some(code) => {
            console::open_from_command();
            console::submit(format!("> {code}"), code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines the tree's words name, as the commands read them.
    fn parse_line(line: &str) -> Option<Cmd<'_>> {
        parse(line)
    }

    #[test]
    fn the_lines_of_the_tree_are_the_tools_commands() {
        assert!(matches!(parse_line("inspect"), Some(Cmd::List)));
        assert!(matches!(
            parse_line("inspect clear"),
            Some(Cmd::View { clear: true, .. })
        ));
        assert!(matches!(
            parse_line("inspect @e[type=zombie]"),
            Some(Cmd::View { clear: false, .. })
        ));
        assert!(matches!(
            parse_line("mobai log stop"),
            Some(Cmd::MobaiLog { stop: true, .. })
        ));
        assert!(matches!(
            parse_line("mobai log @e"),
            Some(Cmd::MobaiLog { stop: false, .. })
        ));
        assert!(matches!(parse_line("lua"), Some(Cmd::Lua { code: None })));
        assert!(matches!(
            parse_line("lua print(1)"),
            Some(Cmd::Lua {
                code: Some("print(1)")
            })
        ));
    }

    #[test]
    fn the_lines_of_other_commands_are_not_ours() {
        for line in ["", "give", "inspectx", "mobai", "mobai logs", "watch"] {
            assert!(parse_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn a_watch_line_ends_with_its_path() {
        match parse_line("watch @e[type=zombie] Health") {
            Some(Cmd::Watch {
                clear: false,
                targets,
                path,
            }) => {
                assert_eq!(targets, "@e[type=zombie]");
                assert_eq!(path, "Health");
            }
            other => panic!("{other:?}"),
        }
    }
}
