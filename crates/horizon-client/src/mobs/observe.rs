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
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// A window of the tools, by the command that opens it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PathSnapshot {
    pub nodes: Vec<[i32; 3]>,
    pub next: usize,
    pub target: [i32; 3],
    pub reached: bool,
}

/// What a watched mob looked like after its last AI step.
#[derive(Clone, Debug, Serialize)]
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
    /// When it was taken; the panel shows a stale one as such.
    #[serde(skip)]
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
}

struct Watch {
    id: u32,
    views: Vec<View>,
    paths: Vec<String>,
}

struct Observed {
    watch: Vec<Watch>,
    snapshots: Vec<BrainSnapshot>,
    data: Vec<DataSnapshot>,
    history: Vec<(u32, VecDeque<HistoryTick>)>,
    series: Vec<(u32, Series)>,
}

/// The fast path for every mob's tick: whether anything is watched at all.
static WATCHING: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Observed> = Mutex::new(Observed {
    watch: Vec::new(),
    snapshots: Vec::new(),
    data: Vec::new(),
    history: Vec::new(),
    series: Vec::new(),
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
            });
            self.watch.last_mut().expect("just pushed")
        }
    }
    /// Drops what no watch asks for any more.
    fn prune(&mut self) {
        self.watch.retain(|w| !w.views.is_empty() || !w.paths.is_empty());
        let ids: Vec<u32> = self.watch.iter().map(|w| w.id).collect();
        self.snapshots.retain(|s| ids.contains(&s.id));
        self.data.retain(|d| ids.contains(&d.id));
        self.history.retain(|(id, _)| {
            self.watch
                .iter()
                .any(|w| w.id == *id && w.views.contains(&View::Goals))
        });
        self.series.retain(|(id, s)| {
            self.watch
                .iter()
                .any(|w| w.id == *id && w.paths.contains(&s.path))
        });
        WATCHING.store(!self.watch.is_empty(), Ordering::Relaxed);
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
    let series = match s.series.iter_mut().position(|(i, x)| *i == id && x.path == path) {
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
    let goals_view = watch.views.contains(&View::Goals);
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
        assert_eq!(all[0].data.as_ref().map(|d| d.name.as_str()), Some("Zombie"));
        assert!(all[0].brain.is_none());
        assert_eq!(all[0].series.len(), 1);
        assert_eq!(all[0].series[0].samples.len(), 1);
        assert_eq!(all[0].series[0].error.as_deref(), Some("not a number"));
        clear(None);
    }
}
