//! The developer tools' commands (docs/dev-tools.md): `/inspect`, `/goals`,
//! `/senses`, `/nav` and `/watch` name entities for the egui windows, `/mobai
//! log` writes a mob's AI snapshot to a file each tick. Each server tick the
//! data `/data get entity` prints goes to `mobs::observe` for every watched
//! entity, read with the same `data_of`, and the block window's facts for
//! every watched block; nothing is written back. The inspect stick (the
//! modkit's item, `horizon.dev.inspect`) opens the same windows on what it
//! is used on, a page at a time: on the host's screen, or for a guest whose
//! Horizon client shows the windows, sent to that client every so often
//! (`mobs::observe_wire`, docs/dev-tools.md "A guest's windows").
use super::{Arg, Context, Game, R, Target, data::DataRef, err};
use horizon_client::{
    data_cmd,
    mobs::observe::{self, BlockKey, BlockSnapshot, Request, StickTarget, View},
    mobs::observe_wire::{self, BlockPage, EntityPage, Wire},
    nbt_path::NbtPath,
    text::Text,
};
use horizon_world::nbt::Tag;
use std::io::Write;
use std::sync::Mutex;

/// The entities watched, as the selectors found them, with the ids the lib's
/// store holds them by (a player's is its network entity id). One list a
/// process: single player's server thread writes the data its window shows.
static WATCH: Mutex<Vec<(Target, u32)>> = Mutex::new(Vec::new());
/// The inspect stick, by player: what it showed last, its page, and the
/// window it opened, which the next use replaces.
static STICK: Mutex<Vec<Stick>> = Mutex::new(Vec::new());

/// The guests whose clients said they show the windows (`Wire::Hello`).
static SHOWS: Mutex<Vec<u128>> = Mutex::new(Vec::new());
/// The window each such guest's stick opened, sent to their client.
static REMOTE: Mutex<Vec<RemoteShown>> = Mutex::new(Vec::new());
/// Server ticks the tools counted, for the guests' intervals.
static CLOCK: Mutex<u64> = Mutex::new(0);
/// How far from a guest the thing their window shows may be.
const REMOTE_RANGE: f64 = 64.0;

#[derive(Clone)]
struct RemoteShown {
    player: u128,
    what: RemoteWhat,
    /// Milliseconds between two pages, at least `observe_wire::FLOOR`.
    interval: u32,
    /// The tick the next page goes at.
    next: u64,
}

#[derive(Clone, PartialEq)]
enum RemoteWhat {
    Entity(Target, u32, View),
    Block(BlockKey, [i32; 3]),
}

struct Stick {
    player: u128,
    last: Option<StickTarget>,
    page: View,
    opened: Option<Opened>,
}

/// A window the stick opened.
enum Opened {
    Entity(u32, View),
    Block(BlockKey),
}

/// The pages of an entity, in the stick's order: every entity has its data,
/// a mob its goals, senses and path too.
fn pages(t: Target) -> &'static [View] {
    if matches!(t, Target::Mob(_)) {
        &[View::Data, View::Goals, View::Senses, View::Nav]
    } else {
        &[View::Data]
    }
}

/// The page the stick shows next: the same entity again turns the page
/// (back when sneaking), another keeps the page when it has it, else its data.
fn next_page(pages: &[View], page: View, same: bool, back: bool) -> View {
    let Some(i) = pages.iter().position(|p| *p == page) else {
        return View::Data;
    };
    if !same {
        return page;
    }
    let n = pages.len();
    pages[if back { (i + n - 1) % n } else { (i + 1) % n }]
}

/// A page's name, as its command is named.
fn page_name(view: View) -> &'static str {
    match view {
        View::Data => "inspect",
        View::Goals => "goals",
        View::Senses => "senses",
        View::Nav => "nav",
    }
}
/// `/mobai log`: the mobs logged, each with its open file.
static LOGS: Mutex<Vec<(u32, std::fs::File)>> = Mutex::new(Vec::new());
/// Ticks since the tools first read anything: the x axis of `/watch`.
static TICKS: Mutex<u64> = Mutex::new(0);

/// Where the tools keep their files, under the game folder.
pub const DIR: &str = "dev-tools";

/// A target's entity id; players have none here (`/data get entity @s` reads
/// the acting player, who is nobody on a tick).
pub(super) fn id_of(t: Target) -> Option<u32> {
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

/// Remember targets a command or the Lua console named, so the tick reads
/// their data: those with an id here (not players; [`remember_with_ids`]).
pub(super) fn remember(targets: &[Target]) {
    let with: Vec<(Target, u32)> = targets
        .iter()
        .filter_map(|&t| id_of(t).map(|id| (t, id)))
        .collect();
    remember_with_ids(&with);
}

/// [`remember`] with each target's id.
fn remember_with_ids(targets: &[(Target, u32)]) {
    let mut watch = WATCH.lock().unwrap_or_else(|e| e.into_inner());
    for &(t, id) in targets {
        if !watch.iter().any(|(_, i)| *i == id) {
            watch.push((t, id));
        }
    }
}

/// Forget the targets the store no longer watches or logs.
fn forget_unwatched() {
    let watched = observe::watched();
    let logged: Vec<u32> = LOGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(id, _)| *id)
        .collect();
    WATCH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(_, id)| watched.contains(id) || logged.contains(id));
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
    /// The tools act on the host: their windows open on the host's screen,
    /// `/mobai log` writes on the host's disk and the console runs as the
    /// owner. So every command of theirs is the owner's own: a guest's is
    /// refused (the owner's commands act through the hub too, as the owner;
    /// a server console has no player and passes).
    pub(super) fn dev_tools_owner_only(&self) -> R<()> {
        let owner = self.serving.as_ref().map(|s| s.owner);
        if self.lan_acting_uuid.is_some() && self.lan_acting_uuid != owner {
            return Err(err("horizon.dev_tools.owner_only", &[]));
        }
        Ok(())
    }

    /// The world closed: nothing of it stays watched. The windows' store
    /// empties, the AI logs' files close, the tick count restarts, the
    /// console closes and drops its handlers, and the tools let the pointer
    /// go, so a watch on entity 123 of this world never reads the next
    /// world's entity 123.
    pub(in crate::game) fn dev_tools_world_closed(&mut self) {
        LOGS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        WATCH.lock().unwrap_or_else(|e| e.into_inner()).clear();
        STICK.lock().unwrap_or_else(|e| e.into_inner()).clear();
        SHOWS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        REMOTE.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *CLOCK.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        *TICKS.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        observe::reset();
        super::lua_console::world_closed();
        crate::dev_tools::world_closed();
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
        if !crate::dev_tools::enabled() {
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
        remember_with_ids(&targets);
        observe::add(&ids, view);
        self.dev_inspect_tick();
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
        remember_with_ids(&targets);
        observe::add_path(&ids, &path);
        self.dev_inspect_tick();
        let line = self.opened_line("horizon.dev_tools.watch", &path, ids.len(), skipped);
        self.feedback(line);
        Ok(ids.len() as i32)
    }

    /// `/mobai log <targets>`: each tick, the mob's AI snapshot as a JSON
    /// line in `dev-tools/logs/`; `/mobai log stop` closes the files.
    pub(super) fn cmd_mobai_log(&mut self, ctx: &Context, stop: bool) -> R<i32> {
        self.dev_tools_owner_only()?;
        if stop {
            let n = LOGS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .count();
            observe::clear_logs();
            forget_unwatched();
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
        let stamp = log_stamp();
        let mut opened = Vec::new();
        for &(t, id) in &mobs {
            let name = self.target_name(t).plain();
            if let Some(path) = open_ai_log(&name, id, &stamp)
                .map_err(|e| super::CommandError::new(Text::literal(e)))?
            {
                opened.push(path);
            }
        }
        remember(&mobs.iter().map(|(t, _)| *t).collect::<Vec<_>>());
        self.feedback(if opened.is_empty() {
            Text::tr("horizon.dev_tools.already_logging", &[])
        } else {
            Text::tr("horizon.dev_tools.logging_to", &[&opened.join(", ")])
        });
        Ok(opened.len() as i32)
    }

    /// Once a server tick, after every level ticked: each watched entity's
    /// data (read where `/data get entity` reads it), the watched paths'
    /// values, and the AI logs' lines. An entity that is gone keeps its
    /// last data, which the windows mark stale.
    pub(in crate::game) fn dev_inspect_tick(&mut self) {
        self.dev_requests();
        self.dev_remote_tick();
        let logging = !LOGS.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if !observe::watching() && !logging {
            return;
        }
        let tick = {
            let mut t = TICKS.lock().unwrap_or_else(|e| e.into_inner());
            *t += 1;
            *t
        };
        self.dev_blocks_tick();
        let targets = WATCH.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for (t, id) in targets {
            let read = self.in_target_level(t, |g| {
                let name = g.target_name(t).plain();
                g.data_of(&DataRef::Entity(t)).ok().map(|data| (name, data))
            });
            let Some(Some((name, data))) = read else {
                continue;
            };
            for path in observe::paths_of(id) {
                let value = match NbtPath::parse(&path) {
                    Err(e) => Err(format!("{e:?}")),
                    Ok(p) => match data_cmd::single_tag(&p, &data) {
                        Err(_) => Err("no such path".to_string()),
                        Ok(tag) => number(&tag).ok_or_else(|| "not a number".to_string()),
                    },
                };
                observe::set_sample(id, &path, tick, value);
            }
            observe::set_data(id, name, data);
            if logging && let Some(brain) = observe::brain_of(id) {
                let mut logs = LOGS.lock().unwrap_or_else(|e| e.into_inner());
                if let Some((_, file)) = logs.iter_mut().find(|(i, _)| *i == id)
                    && let Ok(line) = serde_json::to_string(&brain)
                {
                    let _ = writeln!(file, "{line}");
                }
            }
        }
    }
}

impl Game<'_> {
    /// What the windows asked for since the last tick: the Goals window's
    /// log button starts or stops one mob's log, as `/mobai log` does.
    fn dev_requests(&mut self) {
        for request in observe::take_requests() {
            match request {
                Request::StartLog(id) => {
                    // A mob some command named: the Goals window shows only those.
                    let target = WATCH
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .map(|(t, _)| *t)
                        .find(|t| *t == Target::Mob(id));
                    let Some(t) = target else { continue };
                    let name = self
                        .in_target_level(t, |g| g.target_name(t).plain())
                        .unwrap_or_default();
                    if let Err(e) = open_ai_log(&name, id, &log_stamp()) {
                        log::warn!("dev tools: {e}");
                    }
                }
                Request::StopLog(id) => {
                    LOGS.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .retain(|(i, _)| *i != id);
                    observe::stop_log(id);
                    forget_unwatched();
                }
                Request::Stick { player, target } => self.stick_used(player, target),
            }
        }
    }

    /// The inspect stick used by `player` on `target`. The windows are the
    /// host's: another player's use is refused with a message. An entity
    /// shows its data first; the same entity again turns the page (back when
    /// sneaking), and the window replaces the one the stick opened before.
    fn stick_used(&mut self, player: u128, target: StickTarget) {
        let Some(guest) = self
            .lan
            .as_ref()
            .and_then(|l| l.hub.guests.iter().find(|g| g.uuid == player))
        else {
            return;
        };
        let (conn, back, dimension) = (guest.conn, guest.sneaking, guest.dimension.clone());
        // The owner's windows are the host's; a guest's are on their own
        // screen when their client shows them.
        let remote = self.serving.as_ref().map(|s| s.owner) != Some(player);
        if remote
            && !SHOWS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&player)
        {
            self.stick_message(conn, Text::tr("horizon.dev_tools.stick.no_windows", &[]));
            return;
        }
        let mut sticks = STICK.lock().unwrap_or_else(|e| e.into_inner());
        let i = match sticks.iter().position(|s| s.player == player) {
            Some(i) => i,
            None => {
                sticks.push(Stick {
                    player,
                    last: None,
                    page: View::Data,
                    opened: None,
                });
                sticks.len() - 1
            }
        };
        let stick = &mut sticks[i];
        let line = match &target {
            StickTarget::Entity(id) => {
                let id = *id;
                // A player by its entity id, or what the player's level holds.
                let found = self
                    .lan
                    .as_ref()
                    .and_then(|l| {
                        l.hub
                            .guests
                            .iter()
                            .find(|g| u32::try_from(g.entity_id) == Ok(id))
                    })
                    .map(|g| Target::Guest(g.conn))
                    .or_else(|| {
                        self.in_dimension(&dimension, |g| g.level_entity(id as i32))
                            .flatten()
                    });
                let Some(t) = found else {
                    return;
                };
                let pages = pages(t);
                let same = stick.last.as_ref() == Some(&target);
                let page = if stick.last.is_none() {
                    View::Data
                } else {
                    next_page(pages, stick.page, same, back)
                };
                if let Some(opened) = stick.opened.take() {
                    close(opened);
                }
                if remote {
                    remote_open(player, RemoteWhat::Entity(t, id, page));
                } else {
                    remember_with_ids(&[(t, id)]);
                    observe::add(&[id], page);
                    stick.opened = Some(Opened::Entity(id, page));
                }
                stick.page = page;
                let n = pages.iter().position(|p| *p == page).unwrap_or(0) + 1;
                Text::tr(
                    "horizon.dev_tools.stick.page",
                    &[&page_name(page), &n, &pages.len()],
                )
            }
            StickTarget::Block {
                dimension,
                pos,
                face,
            } => {
                let key = BlockKey {
                    dimension: dimension.clone(),
                    pos: *pos,
                };
                if let Some(opened) = stick.opened.take()
                    && !matches!(&opened, Opened::Block(k) if *k == key)
                {
                    close(opened);
                }
                if remote {
                    remote_open(player, RemoteWhat::Block(key, *face));
                } else {
                    observe::add_block(key.clone(), *face);
                    stick.opened = Some(Opened::Block(key));
                }
                Text::tr("horizon.dev_tools.stick.block", &[])
            }
        };
        stick.last = Some(target);
        drop(sticks);
        forget_unwatched();
        self.stick_message(conn, line);
    }

    /// What the client of guest `conn` said on the tools' channel: it shows
    /// the windows, its window closed, or another interval.
    pub(in crate::game) fn guest_dev_tools(
        &mut self,
        host: &crate::game::lan::LanHost,
        conn: u32,
        data: &[u8],
    ) {
        let Some(player) = host.hub.guest(conn).map(|g| g.uuid) else {
            return;
        };
        match observe_wire::decode(data) {
            Some(Wire::Hello) => {
                let mut shows = SHOWS.lock().unwrap_or_else(|e| e.into_inner());
                if !shows.contains(&player) {
                    shows.push(player);
                }
            }
            Some(Wire::Stop) => remote_stop(player),
            Some(Wire::Interval(ms)) => {
                let mut remote = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(r) = remote.iter_mut().find(|r| r.player == player) {
                    r.interval = observe_wire::clamp_interval(ms);
                }
            }
            _ => {}
        }
    }

    /// The guests' windows whose interval came: their pages, read now and
    /// sent; a window whose player left, or whose entity is gone or out of
    /// range, ends with `Wire::Closed`.
    fn dev_remote_tick(&mut self) {
        let now = {
            let mut c = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
            *c += 1;
            *c
        };
        let due: Vec<RemoteShown> = REMOTE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|r| r.next <= now)
            .cloned()
            .collect();
        for r in due {
            let guest = self.lan.as_ref().and_then(|l| {
                l.hub
                    .guests
                    .iter()
                    .find(|g| g.uuid == r.player)
                    .map(|g| (g.conn, g.dimension.clone(), g.position))
            });
            let Some((conn, dimension, at)) = guest else {
                remote_stop(r.player);
                continue;
            };
            let near = |p: [f64; 3]| {
                (0..3).map(|i| (p[i] - at[i]).powi(2)).sum::<f64>() <= REMOTE_RANGE * REMOTE_RANGE
            };
            let page = match &r.what {
                RemoteWhat::Entity(t, id, view) => {
                    let (t, id, view) = (*t, *id, *view);
                    self.in_dimension(&dimension, |g| {
                        if !matches!(t, Target::Guest(_)) && g.level_entity(id as i32) != Some(t) {
                            return None;
                        }
                        let data = g.data_of(&DataRef::Entity(t)).ok()?;
                        let pos = match data.get("Pos") {
                            Some(Tag::List(p)) if p.len() == 3 => {
                                let f = |v: &Tag| match v {
                                    Tag::Double(bits) => f64::from_bits(*bits),
                                    _ => 0.0,
                                };
                                [f(&p[0]), f(&p[1]), f(&p[2])]
                            }
                            _ => return None,
                        };
                        near(pos).then(|| {
                            Wire::Entity(EntityPage {
                                id,
                                view,
                                name: g.target_name(t).plain(),
                                data: Some(horizon_world::snbt_text::to_snbt(&data)),
                                brain: observe::brain_of(id),
                                history: observe::history_of(id)
                                    .into_iter()
                                    .map(|(age, names)| {
                                        (age, names.iter().map(|n| n.to_string()).collect())
                                    })
                                    .collect(),
                                interval: r.interval,
                            })
                        })
                    })
                    .flatten()
                }
                RemoteWhat::Block(key, face) => (key.dimension == dimension
                    && near(key.pos.map(|c| f64::from(c) + 0.5)))
                .then(|| {
                    self.in_dimension(&dimension, |g| {
                        Wire::Block(BlockPage {
                            key: key.clone(),
                            face: *face,
                            snapshot: g.block_snapshot(key.pos, *face),
                            interval: r.interval,
                        })
                    })
                })
                .flatten(),
            };
            match page {
                Some(wire) => {
                    self.send_dev_tools(conn, &wire);
                    let ticks = u64::from(r.interval.div_ceil(50)).max(1);
                    if let Some(shown) = REMOTE
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter_mut()
                        .find(|s| s.player == r.player)
                    {
                        shown.next = now + ticks;
                    }
                }
                None => {
                    self.send_dev_tools(conn, &Wire::Closed);
                    remote_stop(r.player);
                }
            }
        }
    }

    /// A client's tick: once joined, it tells the server it shows the windows
    /// (when it has them), and sends what its windows asked for.
    pub(in crate::game) fn dev_tools_client_tick(&mut self) {
        if !self.remote.as_ref().is_some_and(|r| r.joined) {
            return;
        }
        let mut out = observe::take_outbox();
        if crate::dev_tools::installed()
            && !crate::dev_tools::HELLO.swap(true, std::sync::atomic::Ordering::Relaxed)
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

    /// A message to guest `conn`'s client on the tools' channel.
    fn send_dev_tools(&mut self, conn: u32, wire: &Wire) {
        let Some(data) = observe_wire::encode(wire) else {
            log::warn!(
                "dev tools: a page past {} bytes is not sent",
                observe_wire::MAX
            );
            return;
        };
        if let Some(lan) = self.lan.as_ref() {
            lan.hub.send_to(
                conn,
                &horizon_protocol::packets::play::Clientbound::CustomPayload(
                    horizon_protocol::packets::configuration::CustomPayload {
                        channel: observe_wire::CHANNEL.into(),
                        data,
                    },
                ),
            );
        }
    }

    /// A line over the hotbar of the guest `conn`, as the debug stick tells
    /// what it did.
    fn stick_message(&mut self, conn: u32, line: Text) {
        if let Some(lan) = self.lan.as_ref() {
            lan.hub.send_to(
                conn,
                &horizon_protocol::packets::play::Clientbound::SystemChat(
                    horizon_protocol::packets::play::SystemChat {
                        content: horizon_client::lan::data::component_nbt(&line.component()),
                        overlay: true,
                    },
                ),
            );
        }
    }

    /// Run `f` in the level of `dimension`, putting the level in place back;
    /// None when that level is not loaded.
    fn in_dimension<R>(&mut self, dimension: &str, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if self.dim.id() == dimension {
            return Some(f(self));
        }
        let dim = crate::game::Dim::from_id(dimension);
        if dim.id() != dimension {
            return None;
        }
        let back = self.enter_command_level(dim)?;
        let r = f(self);
        self.enter_command_level(back);
        Some(r)
    }

    /// Each watched block's facts, read in its level.
    fn dev_blocks_tick(&mut self) {
        for block in observe::blocks() {
            let read = self.in_dimension(&block.key.dimension, |g| {
                g.block_snapshot(block.key.pos, block.face)
            });
            if let Some(snapshot) = read {
                observe::set_block(&block.key, snapshot);
            }
        }
    }

    /// What the block window shows of the block at `pos`, clicked on `face`.
    fn block_snapshot(&mut self, pos: [i32; 3], face: [i32; 3]) -> BlockSnapshot {
        use horizon_core::position::{BlockPos, ChunkPos};
        let [x, y, z] = pos;
        let state = self.world.block(x, y, z);
        let (name, properties) = match self.pack.assets.registry.state(state) {
            Some(s) => (
                s.name.clone(),
                s.properties
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            None => (String::new(), Vec::new()),
        };
        let entity = self.data_of(&DataRef::Block(pos)).ok();
        let light = |l: u8| [l & 15, l >> 4];
        let at = BlockPos::new(x, y, z);
        let (power, strong) = self.with_level(|b, level, ticks| {
            (
                b.best_neighbor_signal(level, ticks, at),
                b.direct_signals(level, ticks, at),
            )
        });
        let now = self.world.game_time;
        let mut scheduled = Vec::new();
        let chunk = ChunkPos {
            x: x >> 4,
            z: z >> 4,
        };
        for (kind, ticks) in [
            ("block", &self.block_ticks.blocks),
            ("fluid", &self.block_ticks.fluids),
        ] {
            if let Some(c) = ticks.chunk(chunk) {
                for t in c.queued().iter().filter(|t| t.pos == at) {
                    scheduled.push((kind.to_string(), t.trigger_tick - now, t.priority as i32));
                }
            }
        }
        let tags = horizon_client::debug_overlay::tag_lines(&self.items().block_tags, &name);
        let collision = self
            .block_collision_shape(pos)
            .boxes()
            .iter()
            .map(|b| {
                let (min, max) = (b.min_position(), b.max_position());
                [
                    f64::from(x) + min.x,
                    f64::from(y) + min.y,
                    f64::from(z) + min.z,
                    f64::from(x) + max.x,
                    f64::from(y) + max.y,
                    f64::from(z) + max.z,
                ]
            })
            .collect();
        BlockSnapshot {
            properties,
            entity,
            light: light(self.world.light(x, y, z)),
            face_light: light(self.world.light(x + face[0], y + face[1], z + face[2])),
            biome: self.biome_name_at(horizon_core::geometry::Vec3::new(
                f64::from(x) + 0.5,
                f64::from(y) + 0.5,
                f64::from(z) + 0.5,
            )),
            power,
            strong,
            random_ticks: self.res.behaviours().is_randomly_ticking(state),
            scheduled,
            tags,
            declared_by: crate::game::mods::block_declared_by(&name),
            collision,
            outline: self.outline_boxes(pos),
            taken: std::time::Instant::now(),
            name,
        }
    }
}

/// Guest `player`'s stick shows `what` on their screen now, the first page
/// at the end of this tick; a mob's AI is snapshot for it.
fn remote_open(player: u128, what: RemoteWhat) {
    let mut remote = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
    let (interval, old) = match remote.iter().position(|r| r.player == player) {
        Some(i) => {
            let r = remote.remove(i);
            (r.interval, Some(r.what))
        }
        None => (observe_wire::DEFAULT_INTERVAL, None),
    };
    if let RemoteWhat::Entity(Target::Mob(_), id, _) = &what {
        observe::set_remote(*id, true);
    }
    remote.push(RemoteShown {
        player,
        what,
        interval,
        next: 0,
    });
    unwatch_remote(&remote, old);
}

/// Guest `player`'s window ended.
fn remote_stop(player: u128) {
    let mut remote = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(i) = remote.iter().position(|r| r.player == player) else {
        return;
    };
    let r = remote.remove(i);
    unwatch_remote(&remote, Some(r.what));
}

/// A mob no guest's window shows any more is no longer snapshot for one.
fn unwatch_remote(remote: &[RemoteShown], old: Option<RemoteWhat>) {
    if let Some(RemoteWhat::Entity(_, id, _)) = old
        && !remote
            .iter()
            .any(|r| matches!(r.what, RemoteWhat::Entity(_, i, _) if i == id))
    {
        observe::set_remote(id, false);
    }
}

/// Close a window the stick opened.
fn close(opened: Opened) {
    match opened {
        Opened::Entity(id, view) => observe::remove(id, Some(view)),
        Opened::Block(key) => observe::remove_block(&key),
    }
}

/// The time a log's file name carries.
fn log_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
}

/// Opens mob `id`'s AI log in `dev-tools/logs/`, named after the mob, and
/// marks it logged: the file's path, or None when it is logged already. The
/// snapshots come from the AI step, which only watches what the store
/// lists: a log is a watch without a window.
fn open_ai_log(name: &str, id: u32, stamp: &str) -> Result<Option<String>, String> {
    let mut logs = LOGS.lock().unwrap_or_else(|e| e.into_inner());
    if logs.iter().any(|(i, _)| *i == id) {
        return Ok(None);
    }
    let dir = std::path::Path::new(DIR).join("logs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let path = dir.join(format!("mobai-{name}-{id}-{stamp}.jsonl"));
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    logs.push((id, file));
    let shown = path.display().to_string();
    observe::add_log(&[id]);
    observe::set_log_file(id, &shown);
    Ok(Some(shown))
}

/// A numeric tag as a float, for the chart.
fn number(tag: &Tag) -> Option<f64> {
    Some(match tag {
        Tag::Byte(v) => f64::from(*v),
        Tag::Short(v) => f64::from(*v),
        Tag::Int(v) => f64::from(*v),
        Tag::Long(v) => *v as f64,
        Tag::Float(bits) => f64::from(f32::from_bits(*bits)),
        Tag::Double(bits) => f64::from_bits(*bits),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Audio, Game, Options, Progress, Settings, load_resources};

    #[test]
    fn the_stick_turns_a_mobs_pages_and_keeps_its_page_on_another() {
        let mob = pages(Target::Mob(1));
        // The same mob again: forward, wrapping; sneaking: back, wrapping.
        assert_eq!(next_page(mob, View::Data, true, false), View::Goals);
        assert_eq!(next_page(mob, View::Nav, true, false), View::Data);
        assert_eq!(next_page(mob, View::Data, true, true), View::Nav);
        assert_eq!(next_page(mob, View::Senses, true, true), View::Goals);
        // Another mob keeps the page; an entity without it shows its data.
        assert_eq!(next_page(mob, View::Senses, false, false), View::Senses);
        let frame = pages(Target::Frame(2));
        assert_eq!(next_page(frame, View::Senses, false, false), View::Data);
        assert_eq!(next_page(frame, View::Data, true, false), View::Data);
        assert_eq!(pages(Target::Guest(3)), [View::Data]);
    }

    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn the_goals_windows_log_button_opens_and_closes_one_mobs_log() {
        let root = std::path::PathBuf::from(std::env::var_os("HORIZON_TEST_RESOURCES").unwrap());
        let settings = Settings::default();
        let loaded = load_resources(
            root.clone(),
            None,
            &root,
            &settings,
            std::time::Instant::now(),
            false,
            true,
            &Progress::new(),
            &mut |_, _| unreachable!(),
        )
        .unwrap();
        let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
        options.resources = root.clone();
        options.execution_role = crate::game::execution_role::ExecutionRole::IntegratedServer;
        let mut game = Game::new(
            &loaded.resources,
            loaded.pack,
            loaded.gui.unwrap(),
            options,
            settings,
            Audio::silent(&root),
            None,
        )
        .unwrap();
        let kind = game
            .res
            .mob_types()
            .get("minecraft:zombie")
            .unwrap()
            .clone();
        for id in [42, 43] {
            game.mobs.push(horizon_client::mobs::Mob::new(
                id,
                kind.clone(),
                horizon_core::geometry::Vec3::new(0.0, 0.0, 0.0),
                0.0,
            ));
        }
        game.build_registries();
        game.set_allow_commands(true);
        horizon_client::command::vanilla::set_dev_tools_commands(true);
        observe::reset();
        assert!(game.run_command("goals @e[type=minecraft:zombie]"));
        // The button asks; the game opens the file at its next tick, where
        // /mobai log opens it, and the window names it.
        observe::request(Request::StartLog(42));
        assert!(observe::snapshots().iter().all(|w| w.log_file.is_none()));
        game.dev_inspect_tick();
        let file = |id: u32| {
            observe::snapshots()
                .into_iter()
                .find(|w| w.id == id)
                .and_then(|w| w.log_file)
        };
        let path = file(42).expect("42 logged");
        assert!(
            path.starts_with("dev-tools/logs/mobai-") && path.ends_with(".jsonl"),
            "{path}"
        );
        assert!(std::path::Path::new(&path).is_file());
        assert_eq!(file(43), None);
        // A mob no command named is not the window's to log.
        observe::request(Request::StartLog(7));
        game.dev_inspect_tick();
        assert!(!observe::watched().contains(&7));
        // Stop closes that log only; the Goals window stays.
        assert!(game.run_command("mobai log @e[type=minecraft:zombie]"));
        let other = file(43).expect("43 logged");
        observe::request(Request::StopLog(42));
        game.dev_inspect_tick();
        assert_eq!(file(42), None);
        assert_eq!(file(43).as_deref(), Some(other.as_str()));
        assert!(
            observe::snapshots()
                .iter()
                .any(|w| w.id == 42 && w.views.contains(&View::Goals))
        );
        assert!(LOGS.lock().unwrap().iter().all(|(id, _)| *id != 42));
        observe::reset();
        LOGS.lock().unwrap().clear();
        for p in [path, other] {
            std::fs::remove_file(p).unwrap();
        }
        // The folders only when the test made them (nothing else is in them).
        let _ = std::fs::remove_dir(std::path::Path::new(DIR).join("logs"));
        let _ = std::fs::remove_dir(DIR);
    }
}
