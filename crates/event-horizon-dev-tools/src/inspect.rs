//! The developer tools' server half (#31, docs/dev-tools.md): the inspect
//! stick's pages, the watched entities' data and the guests' windows, once
//! a server tick after every level ticked ([`tick`], the mod's
//! `server_tick` hook). It lives on the engine's generic host helpers only
//! (`entity_data_with_attachments`, `entity_name_at`, `entity_is_mob`,
//! `block_info`, `server_players`, `player_sneaking`, `server_owner`,
//! `send_payload`, `action_bar`) and on the shared
//! [`event_horizon_observe`] store, which is also where the engine's two
//! packet feeds and the commands hand it their input: a guest's client's
//! Hello/Stop/Interval through `push_server_command`, the stick's uses and
//! the windows' log buttons through the requests, the commands' targets
//! through `remember`. Nothing here runs in the engine's process half
//! without the mod installed.
use event_horizon_observe as observe;
use event_horizon_observe::wire::{self as observe_wire, BlockPage, EntityPage, Wire};
use event_horizon_observe::{BlockKey, BlockSnapshot, Request, StickTarget, View};
use horizon_client::game::NativeBlockInfo;
use horizon_client::mod_api::{Host, NativeServerPlayer};
use horizon_client::nbt_path::NbtPath;
use horizon_client::text::Text;
use horizon_client::{data_cmd, lang};
use horizon_world::nbt::Tag;
use horizon_world::snbt_text::to_snbt;
use std::sync::Mutex;

/// The inspect stick, by player: what it showed last, its page, and the
/// window it opened, which the next use replaces.
static STICK: Mutex<Vec<Stick>> = Mutex::new(Vec::new());

/// The guests whose clients said they show the windows (`Wire::Hello`).
static SHOWS: Mutex<Vec<u128>> = Mutex::new(Vec::new());
/// The window each such guest's stick opened, sent to their client.
static REMOTE: Mutex<Vec<RemoteShown>> = Mutex::new(Vec::new());
/// Server ticks the tools counted, for the guests' intervals.
static CLOCK: Mutex<u64> = Mutex::new(0);
/// Ticks since the tools first read anything: the x axis of `/watch`.
static TICKS: Mutex<u64> = Mutex::new(0);

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
    /// An entity by its id (a player's is its network entity id), read in
    /// the guest's dimension.
    Entity(u32, View),
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

/// The pages of an entity, in the stick's order: a mob has its data, goals,
/// senses and path; every other entity its data alone.
fn pages(mob: bool) -> &'static [View] {
    if mob {
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

/// The player `uuid`'s record on the server, as the host lists the players.
fn player_record(host: &mut dyn Host, uuid: u128) -> Option<NativeServerPlayer> {
    host.server_players().into_iter().find(|g| g.uuid == uuid)
}

/// The dimension a remembered entity is read in: a player's own record
/// names it (they may have changed levels since); an entity's is the one
/// the remember carried. None when a player left.
fn dimension_of(host: &mut dyn Host, id: u32, remembered: Option<&str>) -> Option<String> {
    match remembered {
        Some(dimension) => Some(dimension.to_owned()),
        None => host
            .server_players()
            .iter()
            .find(|g| u32::try_from(g.entity_id).ok() == Some(id))
            .map(|g| g.dimension.clone()),
    }
}

/// Once a server tick, after every level ticked (the mod's `server_tick`
/// hook at `ServerTickEnd`, a world that runs): the stick's requests and
/// the guests' clients' commands, the guests' pages, then each watched
/// entity's data (read where `/data get entity` reads it), the watched
/// paths' values, each watched block's facts and the AI logs' lines. An
/// entity that is gone keeps its last data, which the windows mark stale.
pub(crate) fn tick(host: &mut dyn Host) {
    requests(host);
    server_commands();
    remote_tick(host);
    let logging = observe::is_logging();
    if !observe::watching() && !logging {
        return;
    }
    let tick = {
        let mut t = TICKS.lock().unwrap_or_else(|e| e.into_inner());
        *t += 1;
        *t
    };
    blocks_tick(host);
    for (id, remembered) in observe::watch_entries() {
        let Some(dimension) = dimension_of(host, id, remembered.as_deref()) else {
            continue;
        };
        let Some(data) = host.entity_data_with_attachments(&dimension, id) else {
            continue;
        };
        let name = host
            .entity_name_at(&dimension, id)
            .unwrap_or_else(|| lang::get("horizon.dev_tools.entity"));
        for path in observe::paths_of(id) {
            let value = match NbtPath::parse(&path) {
                Err(e) => Err(format!("{e:?}")),
                Ok(p) => match data_cmd::single_tag(&p, &Tag::Compound(data.clone())) {
                    Err(_) => Err("no such path".to_string()),
                    Ok(tag) => number(&tag).ok_or_else(|| "not a number".to_string()),
                },
            };
            observe::set_sample(id, &path, tick, value);
        }
        observe::set_data(id, name, Tag::Compound(data));
        if logging
            && let Some(brain) = observe::brain_of(id)
            && let Ok(line) = serde_json::to_string(&brain)
        {
            observe::write_log_line(id, &line);
        }
    }
}

/// What the windows and the stick asked for since the last tick: the Goals
/// window's log button starts or stops one mob's log, as `/mobai log` does;
/// the stick turns a page.
fn requests(host: &mut dyn Host) {
    for request in observe::take_requests() {
        match request {
            Request::StartLog(id) => {
                // A mob some command named: the Goals window shows only those.
                let named = observe::watch_entries().into_iter().find(|(i, dimension)| {
                    *i == id
                        && dimension
                            .as_deref()
                            .is_some_and(|d| host.entity_is_mob(d, id))
                });
                let Some((_, dimension)) = named else {
                    continue;
                };
                let name = host
                    .entity_name_at(dimension.as_deref().unwrap_or_default(), id)
                    .unwrap_or_default();
                if let Err(e) = observe::open_log(&name, id) {
                    log::warn!("dev tools: {e}");
                }
            }
            Request::StopLog(id) => {
                observe::stop_log(id);
                observe::forget_unwatched();
            }
            Request::Stick { player, target } => stick_used(host, player, target),
        }
    }
}

/// What the guests' clients said on the tools' channel since the last tick:
/// a client that shows the windows, one whose window closed, and another
/// interval for one. Pages never come from a client.
fn server_commands() {
    for (player, wire) in observe::take_server_commands() {
        match wire {
            Wire::Hello => {
                let mut shows = SHOWS.lock().unwrap_or_else(|e| e.into_inner());
                if !shows.contains(&player) {
                    shows.push(player);
                }
            }
            Wire::Stop => remote_stop(player),
            Wire::Interval(ms) => {
                let mut remote = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(r) = remote.iter_mut().find(|r| r.player == player) {
                    r.interval = observe_wire::clamp_interval(ms);
                }
            }
            Wire::Entity(_) | Wire::Block(_) | Wire::Closed => {}
        }
    }
}

/// The inspect stick used by `player` on `target`. The windows are the
/// host's: another player's use is refused with a message. An entity
/// shows its data first; the same entity again turns the page (back when
/// sneaking), and the window replaces the one the stick opened before.
fn stick_used(host: &mut dyn Host, player: u128, target: StickTarget) {
    let Some(guest) = player_record(host, player) else {
        return;
    };
    let (dimension, back) = (
        guest.dimension,
        host.player_sneaking(player).unwrap_or(false),
    );
    // The owner's windows are the host's; a guest's are on their own
    // screen when their client shows them.
    let remote = host.server_owner() != Some(player);
    if remote
        && !SHOWS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&player)
    {
        host.action_bar(player, Text::tr("horizon.dev_tools.stick.no_windows", &[]));
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
            // A player by its entity id, or an entity of the player's level.
            let is_player = host
                .server_players()
                .iter()
                .any(|g| u32::try_from(g.entity_id).ok() == Some(id));
            let found = is_player || host.entity_name_at(&dimension, id).is_some();
            if !found {
                return;
            }
            let mob = !is_player && host.entity_is_mob(&dimension, id);
            let pages = pages(mob);
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
                remote_open(player, RemoteWhat::Entity(id, page), mob);
            } else {
                observe::remember(&[(
                    id,
                    if is_player {
                        None
                    } else {
                        Some(dimension.clone())
                    },
                )]);
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
            dimension: target_dimension,
            pos,
            face,
        } => {
            let key = BlockKey {
                dimension: target_dimension.clone(),
                pos: *pos,
            };
            if let Some(opened) = stick.opened.take()
                && !matches!(&opened, Opened::Block(k) if *k == key)
            {
                close(opened);
            }
            if remote {
                remote_open(player, RemoteWhat::Block(key, *face), false);
            } else {
                observe::add_block(key.clone(), *face);
                stick.opened = Some(Opened::Block(key));
            }
            Text::tr("horizon.dev_tools.stick.block", &[])
        }
    };
    stick.last = Some(target);
    drop(sticks);
    observe::forget_unwatched();
    host.action_bar(player, line);
}

/// The guests' windows whose interval came: their pages, read now and
/// sent; a window whose player left, or whose entity is gone or out of
/// range, ends with `Wire::Closed`.
fn remote_tick(host: &mut dyn Host) {
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
        let Some(guest) = player_record(host, r.player) else {
            remote_stop(r.player);
            continue;
        };
        let (dimension, at) = (guest.dimension, guest.position);
        let near = |p: [f64; 3]| {
            (0..3).map(|i| (p[i] - at[i]).powi(2)).sum::<f64>() <= REMOTE_RANGE * REMOTE_RANGE
        };
        let page = match &r.what {
            RemoteWhat::Entity(id, view) => {
                let (id, view) = (*id, *view);
                host.entity_data_with_attachments(&dimension, id)
                    .zip(host.entity_name_at(&dimension, id))
                    .and_then(|(data, name)| {
                        let pos = match data.get(&"Pos".into()) {
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
                                name,
                                data: Some(to_snbt(&Tag::Compound(data))),
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
            }
            RemoteWhat::Block(key, face) => (key.dimension == dimension
                && near(key.pos.map(|c| f64::from(c) + 0.5)))
            .then(|| {
                host.block_info(&key.dimension, key.pos, *face).map(|info| {
                    Wire::Block(BlockPage {
                        key: key.clone(),
                        face: *face,
                        snapshot: block_snapshot(info),
                        interval: r.interval,
                    })
                })
            })
            .flatten(),
        };
        match page {
            Some(wire) => {
                send_page(host, r.player, &wire);
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
                send_page(host, r.player, &Wire::Closed);
                remote_stop(r.player);
            }
        }
    }
}

/// A page on the tools' channel to the client of guest `player`.
fn send_page(host: &mut dyn Host, player: u128, wire: &Wire) {
    let Some(data) = observe_wire::encode(wire) else {
        log::warn!(
            "dev tools: a page past {} bytes is not sent",
            observe_wire::MAX
        );
        return;
    };
    host.send_payload(player, observe_wire::CHANNEL, &data);
}

/// Each watched block's facts, read in its level.
fn blocks_tick(host: &mut dyn Host) {
    for block in observe::blocks() {
        if let Some(info) = host.block_info(&block.key.dimension, block.key.pos, block.face) {
            observe::set_block(&block.key, block_snapshot(info));
        }
    }
}

/// What the block window shows of a block the host read.
fn block_snapshot(info: NativeBlockInfo) -> BlockSnapshot {
    BlockSnapshot {
        name: info.name,
        properties: info.properties,
        entity: info.entity,
        light: info.light,
        face_light: info.face_light,
        biome: info.biome,
        power: info.power,
        strong: info.strong,
        random_ticks: info.random_ticks,
        scheduled: info.scheduled,
        tags: info.tags,
        declared_by: info.declared_by,
        collision: info.collision,
        outline: info.outline,
        taken: std::time::Instant::now(),
    }
}

/// The world closed: the stick's windows, the guests' pages and the tick
/// counts go, and the store's watches, logs, requests and snapshots with
/// them. Runs before the engine's own world-closed reset; both reset the
/// store, which is idempotent.
pub fn world_closed() {
    STICK.lock().unwrap_or_else(|e| e.into_inner()).clear();
    SHOWS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    REMOTE.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *CLOCK.lock().unwrap_or_else(|e| e.into_inner()) = 0;
    *TICKS.lock().unwrap_or_else(|e| e.into_inner()) = 0;
    observe::reset();
}

/// Guest `player`'s stick shows `what` on their screen now, the first page
/// at the end of this tick; a mob's AI is snapshot for it.
fn remote_open(player: u128, what: RemoteWhat, mob: bool) {
    let mut remote = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
    let (interval, old) = match remote.iter().position(|r| r.player == player) {
        Some(i) => {
            let r = remote.remove(i);
            (r.interval, Some(r.what))
        }
        None => (observe_wire::DEFAULT_INTERVAL, None),
    };
    if let RemoteWhat::Entity(id, _) = &what
        && mob
    {
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
    if let Some(RemoteWhat::Entity(id, _)) = old
        && !remote
            .iter()
            .any(|r| matches!(r.what, RemoteWhat::Entity(i, _) if i == id))
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

    #[test]
    fn the_stick_turns_a_mobs_pages_and_keeps_its_page_on_another() {
        let mob = pages(true);
        // The same mob again: forward, wrapping; sneaking: back, wrapping.
        assert_eq!(next_page(mob, View::Data, true, false), View::Goals);
        assert_eq!(next_page(mob, View::Nav, true, false), View::Data);
        assert_eq!(next_page(mob, View::Data, true, true), View::Nav);
        assert_eq!(next_page(mob, View::Senses, true, true), View::Goals);
        // Another mob keeps the page; an entity without it shows its data.
        assert_eq!(next_page(mob, View::Senses, false, false), View::Senses);
        let frame = pages(false);
        assert_eq!(next_page(frame, View::Senses, false, false), View::Data);
        assert_eq!(next_page(frame, View::Data, true, false), View::Data);
        assert_eq!(pages(false), [View::Data]);
    }

    /// The host the log-button test needs: two zombies by their ids, nothing
    /// else. Every other hook keeps `Host`'s default.
    struct LogHost;
    impl Host for LogHost {
        fn entity_name_at(&mut self, _dimension: &str, id: u32) -> Option<String> {
            Some(format!("Zombie {id}"))
        }
        fn entity_is_mob(&mut self, _dimension: &str, id: u32) -> bool {
            id == 42 || id == 43
        }
    }

    /// The Goals window's log button asks; the mod's tick opens the file at
    /// the same place `/mobai log` does, one mob at a time, and Stop closes
    /// only the one it names.
    #[test]
    fn the_goals_windows_log_button_opens_and_closes_one_mobs_log() {
        observe::reset();
        observe::remember(&[
            (42, Some("overworld".into())),
            (43, Some("overworld".into())),
        ]);
        observe::add(&[42, 43], View::Goals);
        observe::request(Request::StartLog(42));
        requests(&mut LogHost);
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
        requests(&mut LogHost);
        assert!(!observe::watched().contains(&7));
        // Stop closes that log only; the Goals window stays.
        observe::request(Request::StopLog(42));
        requests(&mut LogHost);
        assert_eq!(file(42), None);
        assert!(
            observe::snapshots()
                .iter()
                .any(|w| w.id == 42 && w.views.contains(&View::Goals))
        );
        observe::reset();
        let _ = std::fs::remove_file(&path);
        // The folders only when this test made them (nothing else is in them).
        let _ = std::fs::remove_dir(std::path::Path::new(observe::DIR).join("logs"));
        let _ = std::fs::remove_dir(observe::DIR);
    }
}
