//! The developer tools' commands (docs/dev-tools.md): `/inspect`, `/goals`,
//! `/senses`, `/nav` and `/watch` name entities for the egui windows, and
//! `/mobai log` writes a mob's AI snapshot to a file each tick. The tools'
//! server half (the stick's pages, the per-tick reads, the guests' windows)
//! lives in `event-horizon-dev-tools` (issue #31); these commands stay here
//! until #34 registers them from that crate, and talk to the shared
//! `event_horizon_observe` store the windows read.
use super::{Arg, Context, Game, R, Target, err};
use crate::dev_input::console;
use crate::mobs::observe::{self, View};
use crate::mobs::observe_wire::{self, Wire};
use crate::nbt_path::NbtPath;
use crate::{data_cmd, text::Text};
use std::sync::atomic::Ordering;

/// A target's entity id; players have none here (`/data get entity @s` reads
/// the acting player, who is nobody on a tick).
pub(in crate::game) fn id_of(t: Target) -> Option<u32> {
    match t {
        Target::Player | Target::Guest(_) => None,
        Target::Mob(id)
        | Target::Item(id)
        | Target::Orb(id)
        | Target::Projectile(id)
        | Target::Stand(id)
        | Target::Frame(id)
        | Target::Painting(id)
        | Target::Boat(id)
        | Target::Cart(id)
        | Target::Other(id)
        | Target::Crystal(id)
        | Target::Dragon(id)
        | Target::Tnt(id)
        | Target::Falling(id)
        | Target::Rocket(id)
        | Target::Trident(id)
        | Target::Spawner(id)
        | Target::Fangs(id)
        | Target::Cloud(id) => Some(id),
    }
}

/// Remember the entities with their ids, so the tick reads their data:
/// those read in `dimension`.
fn remember_with_ids(targets: &[(u32, Option<String>)]) {
    observe::remember(targets);
}

/// Forget the targets the store no longer watches or logs.
fn forget_unwatched() {
    observe::forget_unwatched();
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

impl Game<'_> {
    /// The tools' commands are the owner's, as the windows are: a LAN guest
    /// of any permission level is refused.
    pub(super) fn dev_tools_owner_only(&self) -> R<()> {
        let owner = self.serving.as_ref().map(|s| s.owner);
        if self.lan_acting_uuid.is_some() && self.lan_acting_uuid != owner {
            return Err(err("horizon.dev_tools.owner_only", &[]));
        }
        Ok(())
    }

    /// The world closed: nothing of it stays watched. The windows' store
    /// empties, the AI logs' files close, the tick count restarts, and
    /// the tools let the pointer go, so a watch on entity 123 of this world
    /// never reads the next world's entity 123. The console closes and
    /// drops its handlers in the console mod's `world_lifecycle` hook.
    pub(in crate::game) fn dev_tools_world_closed(&mut self) {
        // The stick's own statics are reset by the mod's `world_lifecycle`
        // hook on the server; this half empties the shared store.
        observe::reset();
        crate::dev_input::world_closed();
    }

    /// `/lua`: the console window; `/lua <code>` runs a line in it.
    pub(super) fn cmd_lua(&mut self, code: Option<String>) -> super::R<i32> {
        // The console runs with the owner's powers and shows on the owner's
        // screen: a guest's /lua is refused, as every tool's command is.
        self.dev_tools_owner_only()?;
        match code {
            None => {
                let open = !console::OPEN.load(Ordering::Relaxed);
                if open {
                    console::open_from_command();
                } else {
                    console::OPEN.store(false, Ordering::Relaxed);
                }
                let line = horizon_client::text::Text::tr(
                    if open {
                        "horizon.dev_tools.lua.open"
                    } else {
                        "horizon.dev_tools.lua.closed"
                    },
                    &[],
                );
                let line = self.with_tools_note(line);
                self.feedback(line);
                Ok(1)
            }
            Some(code) => {
                console::open_from_command();
                console::submit(format!("> {code}"), code);
                Ok(1)
            }
        }
    }

    /// Once a frame on the window's game: the completions the console asked
    /// for. Inside `mc.cmd("…")` they are the chat box's own, from the
    /// tree the server sent; elsewhere the `mc` names.
    pub(in crate::game) fn dev_console_complete(&mut self) {
        let Some((text, cursor)) = console::take_completion_request() else {
            return;
        };
        let tree_completions = |start: usize, command: &str, skip: usize| {
            // `skip` bytes of `command` are a prefix the line does not hold.
            let tree = crate::game::chat::chat_tree(&self.remote);
            let parse = tree.parse(command, &self.ui.registries);
            let (range, list) = tree.client_suggestions(&parse, command.len(), &self.ui.registries);
            let map = |i: usize| start + i.saturating_sub(skip);
            console::Completions {
                text: text.clone(),
                cursor,
                range: (map(range.0), map(range.1)),
                items: list.into_iter().map(|s| (s.text, s.tooltip)).collect(),
                hint: console::call_context(&text, cursor)
                    .and_then(|c| console::signature_hint(&c.name)),
            }
        };
        let completions = if let Some((start, command)) = console::command_context(&text, cursor) {
            tree_completions(start, &command, 0)
        } else if let Some((start, partial)) = console::entity_context(&text, cursor) {
            // An entity argument completes as /tp's target does.
            tree_completions(start, &format!("tp {partial}"), 3)
        } else {
            console::lua_completions(&text, cursor)
        };
        console::set_completions(completions);
    }

    /// A target as the tools watch it, and its id: a player is kept by
    /// connection (the acting player in place is that guest), by its network
    /// entity id; none for a player not on the hub.
    fn watch_id(&self, t: Target) -> Option<(Target, u32)> {
        let guest = match t {
            Target::Guest(conn) => conn,
            Target::Player => self.lan_acting?,
            _ => return id_of(t).map(|id| (t, id)),
        };
        let id = u32::try_from(self.command_guest(guest)?.entity_id).ok()?;
        Some((Target::Guest(guest), id))
    }

    /// The targets of the command with their ids, and how many have none.
    fn watchable(&mut self, ctx: &Context) -> R<(Vec<(Target, u32)>, Vec<u32>, usize)> {
        let targets = self.entities(ctx, "targets", false)?;
        let watched: Vec<(Target, u32)> =
            targets.iter().filter_map(|t| self.watch_id(*t)).collect();
        if watched.is_empty() {
            return Err(err("argument.entity.notfound.entity", &[]));
        }
        let ids = watched.iter().map(|(_, id)| *id).collect();
        let skipped = targets.len() - watched.len();
        Ok((watched, ids, skipped))
    }

    /// `line`, and a note that this game shows no window when the tools
    /// are off on it (a server's, or one started without them).
    pub(super) fn with_tools_note(&self, mut line: Text) -> Text {
        if !crate::dev_input::enabled() {
            line.push(" ");
            line.push(Text::tr("horizon.dev_tools.no_window", &[]));
        }
        line
    }

    /// The count of entities opened, with the players left out when some were.
    fn opened_line(&self, key: &str, what: &str, n: usize, skipped: usize) -> Text {
        let mut line = counted(key, n, &[&what, &n]);
        if skipped > 0 {
            line.push(" ");
            line.push(counted(
                "horizon.dev_tools.players_left_out",
                skipped,
                &[&skipped],
            ));
        }
        self.with_tools_note(line)
    }

    /// `/inspect|goals|senses|nav <targets>`: open that window for them;
    /// `… clear` closes it everywhere.
    pub(super) fn cmd_view(&mut self, ctx: &Context, view: View, clear: bool) -> R<i32> {
        self.dev_tools_owner_only()?;
        let name = match view {
            View::Data => "inspect",
            View::Goals => "goals",
            View::Senses => "senses",
            View::Nav => "nav",
        };
        if clear {
            observe::clear(Some(view));
            forget_unwatched();
            self.feedback(Text::tr("horizon.dev_tools.closed", &[&name]));
            return Ok(0);
        }
        let (targets, ids, skipped) = self.watchable(ctx)?;
        let dim = self.dim.id().to_owned();
        let entries: Vec<(u32, Option<String>)> = targets
            .iter()
            .map(|(_, id)| (*id, Some(dim.clone())))
            .collect();
        remember_with_ids(&entries);
        observe::add(&ids, view);
        let line = self.opened_line("horizon.dev_tools.opened", name, ids.len(), skipped);
        self.feedback(line);
        Ok(ids.len() as i32)
    }

    /// `/inspect` alone: what is watched, and how to use the tools.
    pub(super) fn cmd_inspect_list(&mut self) -> R<i32> {
        self.dev_tools_owner_only()?;
        let watched = observe::snapshots();
        if watched.is_empty() {
            self.feedback(Text::tr("horizon.dev_tools.watching_nothing", &[]));
            return Ok(0);
        }
        let names: Vec<String> = watched
            .iter()
            .map(|w| {
                let name = w.data.as_ref().map_or_else(
                    || horizon_client::lang::get("horizon.dev_tools.entity"),
                    |d| d.name.clone(),
                );
                let views: Vec<&str> = w
                    .views
                    .iter()
                    .map(|v| match v {
                        View::Data => "inspect",
                        View::Goals => "goals",
                        View::Senses => "senses",
                        View::Nav => "nav",
                    })
                    .chain(w.series.iter().map(|s| s.path.as_str()))
                    .collect();
                format!("{name} #{} [{}]", w.id, views.join(", "))
            })
            .collect();
        self.feedback(Text::tr("horizon.dev_tools.watching", &[&names.join("; ")]));
        Ok(watched.len() as i32)
    }

    /// `/watch <targets> <path>`: a numeric NBT path over time; `/watch clear`.
    pub(super) fn cmd_watch(&mut self, ctx: &Context, clear: bool) -> R<i32> {
        self.dev_tools_owner_only()?;
        if clear {
            observe::clear_paths();
            forget_unwatched();
            self.feedback(Text::tr("horizon.dev_tools.closed", &[&"watch"]));
            return Ok(0);
        }
        let Some(Arg::Str(path)) = ctx.get("path").cloned() else {
            return Err(err("command.failed", &[]));
        };
        NbtPath::parse(&path).map_err(data_cmd::path_error)?;
        let (targets, ids, skipped) = self.watchable(ctx)?;
        let dim = self.dim.id().to_owned();
        let entries: Vec<(u32, Option<String>)> = targets
            .iter()
            .map(|(_, id)| (*id, Some(dim.clone())))
            .collect();
        remember_with_ids(&entries);
        observe::add_path(&ids, &path);
        let line = self.opened_line("horizon.dev_tools.watch", &path, ids.len(), skipped);
        self.feedback(line);
        Ok(ids.len() as i32)
    }

    /// `/mobai log <targets>`: each tick, the mob's AI snapshot as a JSON
    /// line in `dev-tools/logs/`; `/mobai log stop` closes the files.
    pub(super) fn cmd_mobai_log(&mut self, ctx: &Context, stop: bool) -> R<i32> {
        self.dev_tools_owner_only()?;
        if stop {
            let n = observe::stop_logs();
            observe::forget_unwatched();
            self.feedback(counted("horizon.dev_tools.logs_closed", n, &[&n]));
            return Ok(n as i32);
        }
        let targets = self.entities(ctx, "targets", false)?;
        let mobs: Vec<(Target, u32)> = targets
            .iter()
            .filter_map(|&t| match t {
                Target::Mob(id) => Some((t, id)),
                _ => None,
            })
            .collect();
        if mobs.is_empty() {
            return Err(err("argument.entity.notfound.entity", &[]));
        }
        let mut opened = Vec::new();
        for &(t, id) in &mobs {
            let name = self.target_name(t).plain();
            if let Some(path) = observe::open_log(&name, id)
                .map_err(|e| super::CommandError::new(Text::literal(e)))?
            {
                opened.push(path);
            }
        }
        let dim = self.dim.id().to_owned();
        let entries: Vec<(u32, Option<String>)> = mobs
            .iter()
            .map(|(_, id)| (*id, Some(dim.clone())))
            .collect();
        remember_with_ids(&entries);
        self.feedback(if opened.is_empty() {
            Text::tr("horizon.dev_tools.already_logging", &[])
        } else {
            Text::tr("horizon.dev_tools.logging_to", &[&opened.join(", ")])
        });
        Ok(opened.len() as i32)
    }

    /// A guest's client said `data` on the tools' channel: read at the next
    /// server tick by `event-horizon-dev-tools` through the store.
    pub(in crate::game) fn guest_dev_tools(
        &mut self,
        host: &crate::game::lan::LanHost,
        conn: u32,
        data: &[u8],
    ) {
        let Some(player) = host.hub.guest(conn).map(|g| g.uuid) else {
            return;
        };
        if let Some(wire) = observe_wire::decode(data) {
            observe::push_server_command(player, wire);
        }
    }

    /// The client's half of the tools' channel: what the store queued for
    /// the server, and `Hello` once when this client joined.
    pub(in crate::game) fn dev_tools_client_tick(&mut self) {
        if !self.remote.as_ref().is_some_and(|r| r.joined) {
            return;
        }
        let mut out: Vec<Wire> = observe::take_outbox();
        if crate::dev_input::installed()
            && !crate::dev_input::HELLO.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            out.insert(0, Wire::Hello);
        }
        for wire in out {
            if let Some(data) = observe_wire::encode(&wire) {
                self.send(horizon_protocol::packets::play::Serverbound::CustomPayload(
                    horizon_protocol::packets::configuration::CustomPayload {
                        channel: observe_wire::CHANNEL.into(),
                        data,
                    },
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_without_an_id_is_not_watched() {
        assert_eq!(id_of(Target::Player), None);
        assert_eq!(id_of(Target::Mob(7)), Some(7));
    }
}
