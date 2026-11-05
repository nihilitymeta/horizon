//! Read-only observation of server entities for the developer tools
//! (docs/dev-tools.md). The commands name what to watch and how
//! ([`View`]): `/inspect` the data `/data get entity` prints, `/goals` a
//! mob's goal selectors with their history, `/senses` what a mob perceives,
//! `/nav` its path, `/watch` a numeric NBT path over time. Each server tick
//! the game stores the data ([`set_data`]) and the watched values
//! ([`set_sample`]), the AI step snapshots a watched mob after its goals
//! ticked ([`observe`]), and the egui windows in the executable read it all
//! ([`snapshots`]). Nothing here changes an entity. Process-wide statics,
//! because the writers (the server thread) and the reader (the window's
//! render thread) share a process in single player; a dedicated server fills
//! snapshots nobody reads, and a client of a remote server sees none.
use super::{Mob, MobWorld};
use horizon_world::nbt::Tag;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// A window of the tools, by the command that opens it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum View {
    /// `/inspect`: the entity's data as a tree.
    Data,
    /// `/goals`: the goal selectors and their history.
    Goals,
    /// `/senses`: target, sight, nearby players.
    Senses,
    /// `/nav`: the path, drawn in the world too.
    Nav,
}

/// Ticks of goal history kept per mob (a minute at 20 a second).
pub const HISTORY_TICKS: usize = 1200;
/// Samples kept per watched path.
pub const SERIES_SAMPLES: usize = 1200;

/// A name a server sent, as the `&'static str` the snapshots hold: kept
/// once each, up to a bound, so a server can't fill the client's memory.
pub fn received_name(name: &str) -> &'static str {
    use std::collections::HashSet;
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(known) = names.get(name) {
        return known;
    }
    if names.len() >= 1024 || name.len() > 96 {
        return "?";
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// A goal row as a server sends it: the name owned.
#[derive(Deserialize)]
struct GoalRowWire {
    priority: i32,
    name: String,
    running: bool,
    flags: u8,
}

impl From<GoalRowWire> for GoalRow {
    fn from(w: GoalRowWire) -> Self {
        Self {
            priority: w.priority,
            name: received_name(&w.name),
            running: w.running,
            flags: w.flags,
        }
    }
}

impl<'de> Deserialize<'de> for GoalRow {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        GoalRowWire::deserialize(d).map(Self::from)
    }
}

/// One goal of a selector, as the panel lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GoalRow {
    pub priority: i32,
    pub name: &'static str,
    pub running: bool,
    /// `Goal.getFlags`: MOVE 1, LOOK 2, JUMP 4, TARGET 8.
    pub flags: u8,
}

/// A mob's path, as far as the panel shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathSnapshot {
    pub nodes: Vec<[i32; 3]>,
    pub next: usize,
    pub target: [i32; 3],
    pub reached: bool,
}

/// What a watched mob looked like after its last AI step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrainSnapshot {
    pub id: u32,
    /// `minecraft:zombie`.
    pub kind: String,
    pub species: String,
    pub custom_name: Option<String>,
    /// The level's game time at the step.
    pub game_time: i64,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub health: f32,
    pub max_health: f32,
    pub age: u32,
    pub alive: bool,
    pub no_ai: bool,
    pub target: Option<String>,
    pub last_hurt_by: Option<String>,
    pub last_hurt_by_time: i32,
    pub aggressive: bool,
    pub panicking: bool,
    pub no_action_time: i32,
    pub home: Option<([i32; 3], i32)>,
    /// `follow_range`: how far it looks for targets.
    pub follow_range: f64,
    /// The sight checks made this tick (`Sensing`): who, and whether seen.
    pub seen: Vec<(String, bool)>,
    /// The nearest player, and how far.
    pub nearest_player: Option<([f64; 3], f64)>,
    pub nav_kind: String,
    pub nav_target: Option<[i32; 3]>,
    pub nav_speed: f64,
    pub nav_stuck: bool,
    pub path: Option<PathSnapshot>,
    pub move_operation: String,
    pub move_wanted: [f64; 3],
    /// `xxa`, `yya`, `zza`: the step's movement input.
    pub input: [f32; 3],
    pub speed: f32,
    pub goals: Vec<GoalRow>,
    pub targets: Vec<GoalRow>,
    /// When it was taken; the panel shows a stale one as such. One a
    /// server sent was taken when it came.
    #[serde(skip, default = "Instant::now")]
    pub taken: Instant,
}

/// An entity's data as `/data get entity` prints it (`Entity.saveWithoutId`).
#[derive(Clone, Debug)]
pub struct DataSnapshot {
    pub id: u32,
    /// The entity's name, as the command's answer names it.
    pub name: String,
    pub data: Tag,
    pub taken: Instant,
}

/// The goals running at one AI step: the mob's age, and their names.
pub type HistoryTick = (u32, Vec<&'static str>);

/// A watched numeric NBT path and its samples (tick, value).
#[derive(Clone, Debug)]
pub struct Series {
    pub path: String,
    pub samples: VecDeque<(u64, f64)>,
    /// Why the last tick gave no number, if it did not.
    pub error: Option<String>,
}

/// One watched entity: what is wanted of it and what has been stored.
#[derive(Clone, Debug)]
pub struct Inspected {
    pub id: u32,
    pub views: Vec<View>,
    pub data: Option<DataSnapshot>,
    pub brain: Option<BrainSnapshot>,
    pub history: Vec<HistoryTick>,
    pub series: Vec<Series>,
    /// The file its AI log goes to, while `/mobai log` or the Goals window's
    /// log button logs it.
    pub log_file: Option<String>,
}

/// What a window asks of the game, done at its next server tick
/// ([`take_requests`]): what only the game can do, such as opening a file
/// where `/mobai log` opens it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Log the mob's AI, as `/mobai log` does.
    StartLog(u32),
    /// Stop that mob's log and close its file.
    StopLog(u32),
    /// The inspect stick, used by the player with this UUID on an entity or
    /// a block: the game checks who may, and opens or turns its page.
    Stick { player: u128, target: StickTarget },
}

/// What the inspect stick was used on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StickTarget {
    /// An entity, by its id (a player's too).
    Entity(u32),
    /// A block, and the face clicked (a unit vector, out of the block).
    Block {
        dimension: String,
        pos: [i32; 3],
        face: [i32; 3],
    },
}

/// A block the block window shows: where it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockKey {
    pub dimension: String,
    pub pos: [i32; 3],
}

/// A block as the block window shows it, read once a server tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockSnapshot {
    /// `minecraft:repeater`, and its state properties.
    pub name: String,
    pub properties: Vec<(String, String)>,
    /// Its block entity's data, as `/data get block` prints it (in SNBT on
    /// the wire).
    #[serde(with = "snbt_tag")]
    pub entity: Option<Tag>,
    /// Block light and sky light at the block, and at the clicked face.
    pub light: [u8; 2],
    pub face_light: [u8; 2],
    pub biome: Option<String>,
    /// `getBestNeighborSignal`: the power it receives.
    pub power: i32,
    /// `getDirectSignal(pos, direction)`, by `Direction::ALL`: the strong
    /// signal it gives each way.
    pub strong: [i32; 6],
    pub random_ticks: bool,
    /// Its pending scheduled ticks: what, in how many ticks, and the priority.
    pub scheduled: Vec<(String, i64, i32)>,
    /// The block tags it is in, `#minecraft:mineable/pickaxe`.
    pub tags: Vec<String>,
    /// A content mod's block: the mod that declares it, and its template.
    pub declared_by: Option<(String, String)>,
    /// Its collision and outline boxes, in world coordinates (min x, y, z,
    /// max x, y, z), drawn in the world while the window is open.
    pub collision: Vec<[f64; 6]>,
    pub outline: Vec<[f64; 6]>,
    #[serde(skip, default = "Instant::now")]
    pub taken: Instant,
}

/// A tag as SNBT, for the wire.
mod snbt_tag {
    use horizon_world::nbt::Tag;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(tag: &Option<Tag>, s: S) -> Result<S::Ok, S::Error> {
        tag.as_ref()
            .map(horizon_world::snbt_text::to_snbt)
            .serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Tag>, D::Error> {
        Ok(Option::<String>::deserialize(d)?
            .and_then(|t| horizon_world::nbt::parse_snbt(&t).ok().map(|(tag, _)| tag)))
    }
}

/// The window a server shows this client, a guest's (docs/dev-tools.md, "A
/// guest's windows"): which, and the interval the server sends it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Remote {
    Entity(u32, View),
    Block(BlockKey),
}

static REMOTE: Mutex<Option<(Remote, u32)>> = Mutex::new(None);
/// What this client's windows tell the server, sent at the client's next tick.
static OUTBOX: Mutex<Vec<super::observe_wire::Wire>> = Mutex::new(Vec::new());

/// The window the server shows this client, and its interval.
pub fn remote() -> Option<(Remote, u32)> {
    REMOTE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether the entity window of `id` is the server's.
pub fn is_remote_entity(id: u32) -> bool {
    matches!(remote(), Some((Remote::Entity(i, _), _)) if i == id)
}

/// Whether the block window at `key` is the server's.
pub fn is_remote_block(key: &BlockKey) -> bool {
    matches!(remote(), Some((Remote::Block(k), _)) if k == *key)
}

/// Tell the server, at the client's next tick.
pub fn send_to_server(wire: super::observe_wire::Wire) {
    OUTBOX.lock().unwrap_or_else(|e| e.into_inner()).push(wire);
}

/// What waits for the server.
pub fn take_outbox() -> Vec<super::observe_wire::Wire> {
    std::mem::take(&mut *OUTBOX.lock().unwrap_or_else(|e| e.into_inner()))
}

/// What the server sent this client: its page in this client's store, in
/// place of the last one it sent, so the windows show it.
pub fn received(wire: super::observe_wire::Wire) {
    use super::observe_wire::Wire;
    match wire {
        Wire::Entity(page) => {
            let shown = Remote::Entity(page.id, page.view);
            replace_remote(Some((shown, page.interval)));
            add(&[page.id], page.view);
            let data = page
                .data
                .and_then(|t| horizon_world::nbt::parse_snbt(&t).ok().map(|(tag, _)| tag));
            if let Some(data) = data {
                set_data(page.id, page.name, data);
            }
            let mut s = state();
            if let Some(mut brain) = page.brain {
                brain.id = page.id;
                brain.taken = Instant::now();
                match s.snapshots.iter_mut().find(|b| b.id == page.id) {
                    Some(old) => *old = brain,
                    None => s.snapshots.push(brain),
                }
            }
            let history: VecDeque<HistoryTick> = page
                .history
                .into_iter()
                .map(|(age, names)| (age, names.iter().map(|n| received_name(n)).collect()))
                .collect();
            match s.history.iter_mut().find(|(id, _)| *id == page.id) {
                Some((_, h)) => *h = history,
                None => s.history.push((page.id, history)),
            }
        }
        Wire::Block(page) => {
            replace_remote(Some((Remote::Block(page.key.clone()), page.interval)));
            add_block(page.key.clone(), page.face);
            let mut snapshot = page.snapshot;
            snapshot.taken = Instant::now();
            set_block(&page.key, snapshot);
        }
        Wire::Closed => replace_remote(None),
        Wire::Hello | Wire::Stop | Wire::Interval(_) => {}
    }
}

/// The server's window is now `new`: the one before closes.
fn replace_remote(new: Option<(Remote, u32)>) {
    let old = {
        let mut r = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
        let old = r.take();
        *r = new.clone();
        old
    };
    match old {
        Some((was, _)) if new.as_ref().map(|(n, _)| n) != Some(&was) => match was {
            Remote::Entity(id, view) => remove(id, Some(view)),
            Remote::Block(key) => remove_block(&key),
        },
        _ => {}
    }
}

/// The server's window closed here: tell the server to stop sending it.
pub fn close_remote() {
    if remote().is_some() {
        replace_remote(None);
        send_to_server(super::observe_wire::Wire::Stop);
    }
}

/// A window of this client asks the server for another interval.
pub fn set_remote_interval(ms: u32) {
    let mut r = REMOTE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, interval)) = r.as_mut() {
        *interval = ms;
        drop(r);
        send_to_server(super::observe_wire::Wire::Interval(ms));
    }
}

/// One watched block: where, the face it was clicked on, and what is stored.
#[derive(Clone, Debug)]
pub struct InspectedBlock {
    pub key: BlockKey,
    pub face: [i32; 3],
    pub snapshot: Option<BlockSnapshot>,
}

static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());

/// Ask the game for `request`, at its next server tick.
pub fn request(request: Request) {
    let mut r = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    if !r.contains(&request) {
        r.push(request);
    }
}

/// The requests made since the last call, in order.
pub fn take_requests() -> Vec<Request> {
    std::mem::take(&mut *REQUESTS.lock().unwrap_or_else(|e| e.into_inner()))
}

struct Watch {
    id: u32,
    views: Vec<View>,
    paths: Vec<String>,
    /// `/mobai log`: snapshots wanted, no window.
    logged: bool,
    /// The file the log goes to.
    log_file: Option<String>,
    /// A guest's window shows it, on the guest's screen: snapshots and
    /// history wanted, no window here.
    remote: bool,
}

struct Observed {
    watch: Vec<Watch>,
    snapshots: Vec<BrainSnapshot>,
    data: Vec<DataSnapshot>,
    history: Vec<(u32, VecDeque<HistoryTick>)>,
    series: Vec<(u32, Series)>,
    blocks: Vec<InspectedBlock>,
}

/// The fast path for every mob's tick: whether anything is watched at all.
static WATCHING: AtomicBool = AtomicBool::new(false);
/// Whether any watch has a window (a view or a path): the tools' pointer follows it.
static SHOWING: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Observed> = Mutex::new(Observed {
    watch: Vec::new(),
    snapshots: Vec::new(),
    data: Vec::new(),
    history: Vec::new(),
    series: Vec::new(),
    blocks: Vec::new(),
});

fn state() -> std::sync::MutexGuard<'static, Observed> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

impl Observed {
    fn entry(&mut self, id: u32) -> &mut Watch {
        if let Some(i) = self.watch.iter().position(|w| w.id == id) {
            &mut self.watch[i]
        } else {
            self.watch.push(Watch {
                id,
                views: Vec::new(),
                paths: Vec::new(),
                logged: false,
                log_file: None,
                remote: false,
            });
            self.watch.last_mut().expect("just pushed")
        }
    }
    /// Drops what no watch asks for any more.
    fn prune(&mut self) {
        self.watch
            .retain(|w| !w.views.is_empty() || !w.paths.is_empty() || w.logged || w.remote);
        let ids: Vec<u32> = self.watch.iter().map(|w| w.id).collect();
        self.snapshots.retain(|s| ids.contains(&s.id));
        self.data.retain(|d| ids.contains(&d.id));
        self.history.retain(|(id, _)| {
            self.watch
                .iter()
                .any(|w| w.id == *id && (w.views.contains(&View::Goals) || w.remote))
        });
        self.series.retain(|(id, s)| {
            self.watch
                .iter()
                .any(|w| w.id == *id && w.paths.contains(&s.path))
        });
        WATCHING.store(
            !self.watch.is_empty() || !self.blocks.is_empty(),
            Ordering::Relaxed,
        );
        SHOWING.store(
            !self.blocks.is_empty()
                || self
                    .watch
                    .iter()
                    .any(|w| !w.views.is_empty() || !w.paths.is_empty()),
            Ordering::Relaxed,
        );
    }
}

/// Open `view` of these entities (by id), keeping what else is watched.
pub fn add(ids: &[u32], view: View) {
    let mut s = state();
    for &id in ids {
        let w = s.entry(id);
        if !w.views.contains(&view) {
            w.views.push(view);
        }
    }
    s.prune();
}

/// Watch a numeric NBT `path` of these entities.
pub fn add_path(ids: &[u32], path: &str) {
    let mut s = state();
    for &id in ids {
        let w = s.entry(id);
        if !w.paths.iter().any(|p| p == path) {
            w.paths.push(path.to_string());
        }
    }
    s.prune();
}

/// Close `view` of one entity, or every view of it with None.
pub fn remove(id: u32, view: Option<View>) {
    let mut s = state();
    if let Some(w) = s.watch.iter_mut().find(|w| w.id == id) {
        match view {
            Some(v) => w.views.retain(|x| *x != v),
            None => {
                w.views.clear();
                w.paths.clear();
            }
        }
    }
    s.prune();
}

/// Stop watching `path` of one entity.
pub fn remove_path(id: u32, path: &str) {
    let mut s = state();
    if let Some(w) = s.watch.iter_mut().find(|w| w.id == id) {
        w.paths.retain(|p| p != path);
    }
    s.prune();
}

/// Close `view` everywhere (None: every window of every entity).
pub fn clear(view: Option<View>) {
    let mut s = state();
    for w in &mut s.watch {
        match view {
            Some(v) => w.views.retain(|x| *x != v),
            None => {
                w.views.clear();
                w.paths.clear();
            }
        }
    }
    s.prune();
}

/// Snapshot these entities each tick for a log, with no window.
pub fn add_log(ids: &[u32]) {
    let mut s = state();
    for &id in ids {
        s.entry(id).logged = true;
    }
    s.prune();
}

/// A guest's window shows mob `id` (`on`), or stopped: the server keeps its
/// AI snapshots and history for it, with no window of its own.
pub fn set_remote(id: u32, on: bool) {
    let mut s = state();
    if on {
        s.entry(id).remote = true;
    } else if let Some(w) = s.watch.iter_mut().find(|w| w.id == id) {
        w.remote = false;
    }
    s.prune();
}

/// The goals history of one mob, for a guest's Goals page.
pub fn history_of(id: u32) -> Vec<HistoryTick> {
    state()
        .history
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, h)| h.iter().cloned().collect())
        .unwrap_or_default()
}

/// The file one entity's log goes to, for its Goals window.
pub fn set_log_file(id: u32, path: &str) {
    let mut s = state();
    s.entry(id).log_file = Some(path.to_string());
    s.prune();
}

/// One entity's log stopped.
pub fn stop_log(id: u32) {
    let mut s = state();
    if let Some(w) = s.watch.iter_mut().find(|w| w.id == id) {
        w.logged = false;
        w.log_file = None;
    }
    s.prune();
}

/// The logs stopped: what only they kept watched is dropped.
pub fn clear_logs() {
    let mut s = state();
    for w in &mut s.watch {
        w.logged = false;
        w.log_file = None;
    }
    s.prune();
}

/// The world closed: nothing of it is watched any more. Every watch, window,
/// path, log mark and stored snapshot goes, so an entity of the next world
/// that happens to have the same id is not shown under the old one's window.
pub fn reset() {
    REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *REMOTE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    OUTBOX.lock().unwrap_or_else(|e| e.into_inner()).clear();
    let mut s = state();
    s.watch.clear();
    s.snapshots.clear();
    s.data.clear();
    s.history.clear();
    s.series.clear();
    s.blocks.clear();
    s.prune();
}

/// Whether any window is wanted: a view or a path of some entity.
pub fn showing() -> bool {
    SHOWING.load(Ordering::Relaxed)
}

/// Stop watching every path.
pub fn clear_paths() {
    let mut s = state();
    for w in &mut s.watch {
        w.paths.clear();
    }
    s.prune();
}

/// The ids watched, in the order first named.
pub fn watched() -> Vec<u32> {
    state().watch.iter().map(|w| w.id).collect()
}

/// The paths watched of one entity.
pub fn paths_of(id: u32) -> Vec<String> {
    state()
        .watch
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.paths.clone())
        .unwrap_or_default()
}

/// Whether anything is watched (the game's cheap check before it reads data).
pub fn watching() -> bool {
    WATCHING.load(Ordering::Relaxed)
}

/// The game stores a watched entity's data, once a server tick.
pub fn set_data(id: u32, name: String, data: Tag) {
    let mut s = state();
    if !s.watch.iter().any(|w| w.id == id) {
        return;
    }
    let snap = DataSnapshot {
        id,
        name,
        data,
        taken: Instant::now(),
    };
    match s.data.iter_mut().find(|old| old.id == id) {
        Some(old) => *old = snap,
        None => s.data.push(snap),
    }
}

/// The game stores a watched path's value at `tick`, or why there is none.
pub fn set_sample(id: u32, path: &str, tick: u64, value: Result<f64, String>) {
    let mut s = state();
    if !s
        .watch
        .iter()
        .any(|w| w.id == id && w.paths.iter().any(|p| p == path))
    {
        return;
    }
    let series = match s
        .series
        .iter_mut()
        .position(|(i, x)| *i == id && x.path == path)
    {
        Some(i) => &mut s.series[i].1,
        None => {
            s.series.push((
                id,
                Series {
                    path: path.to_string(),
                    samples: VecDeque::new(),
                    error: None,
                },
            ));
            &mut s.series.last_mut().expect("just pushed").1
        }
    };
    match value {
        Ok(v) => {
            series.error = None;
            series.samples.push_back((tick, v));
            while series.samples.len() > SERIES_SAMPLES {
                series.samples.pop_front();
            }
        }
        Err(e) => series.error = Some(e),
    }
}

fn rows(selector: &super::goals::GoalSelector) -> Vec<GoalRow> {
    selector
        .entries()
        .map(|(priority, name, running, flags)| GoalRow {
            priority,
            name,
            running,
            flags,
        })
        .collect()
}

/// Called by the server's AI step for every mob, after its goals ticked:
/// snapshots the mob when it is watched. Nothing is touched.
pub fn observe(m: &Mob, w: &MobWorld) {
    if !WATCHING.load(Ordering::Relaxed) {
        return;
    }
    let mut s = state();
    let Some(watch) = s.watch.iter().find(|x| x.id == m.id) else {
        return;
    };
    let goals_view = watch.views.contains(&View::Goals) || watch.remote;
    let ai = &m.ai;
    let position = [m.position.x, m.position.y, m.position.z];
    let snap = BrainSnapshot {
        id: m.id,
        kind: m.kind.id.clone(),
        species: format!("{:?}", m.species()),
        custom_name: m.custom_name.clone(),
        game_time: w.game_time,
        position,
        velocity: [m.velocity.x, m.velocity.y, m.velocity.z],
        health: m.health,
        max_health: m.attrs.max_health,
        age: m.age,
        alive: m.alive(),
        no_ai: m.no_ai,
        target: ai.target.map(|who| format!("{who:?}")),
        last_hurt_by: ai.last_hurt_by.map(|who| format!("{who:?}")),
        last_hurt_by_time: ai.last_hurt_by_time,
        aggressive: ai.aggressive,
        panicking: ai.panicking,
        no_action_time: ai.no_action_time,
        home: ai.home,
        follow_range: m.attrs.follow_range,
        seen: ai
            .seen_cache
            .iter()
            .map(|(who, seen)| (format!("{who:?}"), *seen))
            .collect(),
        nearest_player: w.nearest_player.map(|p| {
            let d = ((p.x - position[0]).powi(2)
                + (p.y - position[1]).powi(2)
                + (p.z - position[2]).powi(2))
            .sqrt();
            ([p.x, p.y, p.z], d)
        }),
        nav_kind: format!("{:?}", ai.nav.kind),
        nav_target: ai.nav.target_pos,
        nav_speed: ai.nav.speed_modifier,
        nav_stuck: ai.nav.stuck,
        path: ai.nav.path.as_ref().map(|p| PathSnapshot {
            nodes: p.nodes.iter().map(|n| [n.x, n.y, n.z]).collect(),
            next: p.next,
            target: p.target,
            reached: p.reached,
        }),
        move_operation: format!("{:?}", ai.mov.operation),
        move_wanted: ai.mov.wanted,
        input: [ai.xxa, ai.yya, ai.zza],
        speed: ai.speed,
        goals: rows(&ai.goals),
        targets: rows(&ai.targets),
        taken: Instant::now(),
    };
    if goals_view {
        let running: Vec<&'static str> = snap
            .targets
            .iter()
            .chain(snap.goals.iter())
            .filter(|g| g.running)
            .map(|g| g.name)
            .collect();
        let history = match s.history.iter_mut().position(|(id, _)| *id == m.id) {
            Some(i) => &mut s.history[i].1,
            None => {
                s.history.push((m.id, VecDeque::new()));
                &mut s.history.last_mut().expect("just pushed").1
            }
        };
        history.push_back((m.age, running));
        while history.len() > HISTORY_TICKS {
            history.pop_front();
        }
    }
    match s.snapshots.iter_mut().find(|old| old.id == m.id) {
        Some(old) => *old = snap,
        None => s.snapshots.push(snap),
    }
}

/// The last snapshot of one mob, for the AI log.
pub fn brain_of(id: u32) -> Option<BrainSnapshot> {
    state().snapshots.iter().find(|b| b.id == id).cloned()
}

/// Open the block window of the block at `key`, clicked on `face`.
pub fn add_block(key: BlockKey, face: [i32; 3]) {
    let mut s = state();
    match s.blocks.iter_mut().find(|b| b.key == key) {
        Some(b) => b.face = face,
        None => s.blocks.push(InspectedBlock {
            key,
            face,
            snapshot: None,
        }),
    }
    s.prune();
}

/// Close the block window of the block at `key`.
pub fn remove_block(key: &BlockKey) {
    let mut s = state();
    s.blocks.retain(|b| b.key != *key);
    s.prune();
}

/// The game stores a watched block's facts, once a server tick.
pub fn set_block(key: &BlockKey, snapshot: BlockSnapshot) {
    let mut s = state();
    if let Some(b) = s.blocks.iter_mut().find(|b| b.key == *key) {
        b.snapshot = Some(snapshot);
    }
}

/// The watched blocks, in the order opened.
pub fn blocks() -> Vec<InspectedBlock> {
    state().blocks.clone()
}

/// Every watched entity, in the order first named, with what has been stored for it.
pub fn snapshots() -> Vec<Inspected> {
    let s = state();
    s.watch
        .iter()
        .map(|w| Inspected {
            id: w.id,
            views: w.views.clone(),
            data: s.data.iter().find(|d| d.id == w.id).cloned(),
            brain: s.snapshots.iter().find(|b| b.id == w.id).cloned(),
            history: s
                .history
                .iter()
                .find(|(id, _)| *id == w.id)
                .map(|(_, h)| h.iter().cloned().collect())
                .unwrap_or_default(),
            series: s
                .series
                .iter()
                .filter(|(id, _)| *id == w.id)
                .map(|(_, x)| x.clone())
                .collect(),
            log_file: w.log_file.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store is one static: the tests take turns.
    static TURN: Mutex<()> = Mutex::new(());

    #[test]
    fn views_and_paths_come_and_go() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear(None);
        add(&[3, 3, 5], View::Data);
        add(&[5], View::Goals);
        add_path(&[3], "Health");
        assert_eq!(watched(), vec![3, 5]);
        assert!(watching());
        remove(3, Some(View::Data));
        // The path keeps 3 watched.
        assert_eq!(watched(), vec![3, 5]);
        remove_path(3, "Health");
        assert_eq!(watched(), vec![5]);
        // A log keeps an entity watched without a window.
        add_log(&[9]);
        assert_eq!(watched(), vec![5, 9]);
        assert!(showing());
        clear(None);
        assert_eq!(watched(), vec![9]);
        assert!(watching() && !showing());
        clear_logs();
        assert!(watched().is_empty());
        add(&[5], View::Data);
        add(&[5], View::Goals);
        clear(Some(View::Goals));
        assert_eq!(watched(), vec![5]);
        clear(Some(View::Data));
        assert!(watched().is_empty());
        assert!(!watching());
        assert!(snapshots().is_empty());
    }

    #[test]
    fn data_and_samples_are_kept_for_watched_ids_only() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear(None);
        add(&[7], View::Data);
        add_path(&[7], "Health");
        set_data(7, "Zombie".into(), Tag::Compound(Default::default()));
        set_data(8, "Cow".into(), Tag::Compound(Default::default()));
        set_sample(7, "Health", 1, Ok(20.0));
        set_sample(7, "Health", 2, Err("not a number".into()));
        set_sample(7, "Pos", 1, Ok(1.0));
        let all = snapshots();
        assert_eq!(all.len(), 1);
        assert_eq!(
            all[0].data.as_ref().map(|d| d.name.as_str()),
            Some("Zombie")
        );
        assert!(all[0].brain.is_none());
        assert_eq!(all[0].series.len(), 1);
        assert_eq!(all[0].series[0].samples.len(), 1);
        assert_eq!(all[0].series[0].error.as_deref(), Some("not a number"));
        clear(None);
    }

    #[test]
    fn a_world_close_resets_the_whole_store() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear(None);
        add(&[7], View::Data);
        add_path(&[7], "Health");
        add_log(&[8]);
        set_data(7, "Zombie".into(), Tag::Compound(Default::default()));
        set_sample(7, "Health", 1, Ok(20.0));
        assert!(watching() && showing());
        reset();
        assert!(watched().is_empty());
        assert!(!watching() && !showing());
        assert!(snapshots().is_empty());
        assert!(brain_of(8).is_none());
        // A log mark does not survive either: the next world's entity 8 is not snapshotted.
        assert!(paths_of(7).is_empty());
    }

    #[test]
    fn a_block_window_keeps_its_block_watched_until_it_closes() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let key = BlockKey {
            dimension: "minecraft:overworld".into(),
            pos: [1, 64, -2],
        };
        add_block(key.clone(), [0, 1, 0]);
        add_block(key.clone(), [1, 0, 0]);
        assert!(watching() && showing());
        let all = blocks();
        assert_eq!(all.len(), 1, "one window per block");
        assert_eq!(all[0].face, [1, 0, 0]);
        // A block nobody watches gets no snapshot.
        let other = BlockKey {
            dimension: "minecraft:the_nether".into(),
            pos: [1, 64, -2],
        };
        set_block(
            &other,
            BlockSnapshot {
                name: "minecraft:stone".into(),
                properties: Vec::new(),
                entity: None,
                light: [0, 15],
                face_light: [0, 15],
                biome: None,
                power: 0,
                strong: [0; 6],
                random_ticks: false,
                scheduled: Vec::new(),
                tags: Vec::new(),
                declared_by: None,
                collision: Vec::new(),
                outline: Vec::new(),
                taken: Instant::now(),
            },
        );
        assert!(blocks()[0].snapshot.is_none());
        remove_block(&key);
        assert!(!watching() && !showing());
        add_block(key, [0, 1, 0]);
        reset();
        assert!(blocks().is_empty());
    }

    #[test]
    fn a_page_the_server_sends_shows_here_until_it_closes() {
        use crate::mobs::observe_wire::{EntityPage, Wire};
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let page = |id: u32, view: View| {
            Wire::Entity(EntityPage {
                id,
                view,
                name: "Pig".into(),
                data: Some("{Health:10.0f}".into()),
                brain: None,
                history: vec![(4, vec!["Float".into(), "Stroll".into()])],
                interval: 500,
            })
        };
        received(page(9, View::Goals));
        let all = snapshots();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].views, [View::Goals]);
        assert_eq!(all[0].data.as_ref().map(|d| d.name.as_str()), Some("Pig"));
        assert_eq!(all[0].history, [(4, vec!["Float", "Stroll"])]);
        assert_eq!(remote(), Some((Remote::Entity(9, View::Goals), 500)));
        assert!(is_remote_entity(9) && showing());
        // The next page replaces the window; a new interval goes to the server.
        received(page(9, View::Senses));
        assert_eq!(snapshots()[0].views, [View::Senses]);
        set_remote_interval(2000);
        assert_eq!(remote().map(|(_, ms)| ms), Some(2000));
        // Closing it here tells the server to stop.
        close_remote();
        assert!(!showing() && remote().is_none());
        let out = take_outbox();
        assert!(
            matches!(out[..], [Wire::Interval(2000), Wire::Stop]),
            "{out:?}"
        );
        // The server's own end closes it too, and sends nothing back.
        received(page(3, View::Data));
        received(Wire::Closed);
        assert!(!showing() && take_outbox().is_empty());
        reset();
    }

    #[test]
    fn one_log_stops_without_the_others() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        add(&[4], View::Goals);
        add_log(&[4, 6]);
        set_log_file(4, "dev-tools/logs/a.jsonl");
        set_log_file(6, "dev-tools/logs/b.jsonl");
        stop_log(4);
        // 4 keeps its window, 6 its log.
        assert_eq!(watched(), vec![4, 6]);
        let all = snapshots();
        assert_eq!(all[0].log_file, None);
        assert_eq!(all[1].log_file.as_deref(), Some("dev-tools/logs/b.jsonl"));
        stop_log(6);
        assert_eq!(watched(), vec![4]);
        // Requests come once each, in order, and a world close drops them.
        request(Request::StartLog(4));
        request(Request::StartLog(4));
        request(Request::StopLog(4));
        assert_eq!(
            take_requests(),
            vec![Request::StartLog(4), Request::StopLog(4)]
        );
        assert!(take_requests().is_empty());
        request(Request::StartLog(4));
        reset();
        assert!(take_requests().is_empty());
    }
}
