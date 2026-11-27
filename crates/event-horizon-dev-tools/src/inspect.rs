//! The developer tools' server half, owner-local part (#31, docs/dev-tools.md): once a server tick,
//! after every level ticked ([`tick`], the mod's `server_tick` hook), the store's requests are
//! served (a window's `/mobai log` button starts or stops one mob's log), and each entity the
//! commands watch is read into the shared [`event_horizon_observe`] store: its data, its watched
//! paths' numbers, and its AI log lines. The commands fill the store with `remember`, `add` and
//! `add_path`; the windows read it. Nothing here runs without the mod installed.
//!
//! Each watched block (`/blockinfo`) is read here too, once a tick: its facts go to the block window
//! ([`blocks_tick`]). Left out, as a chunk of its own: the inspect stick (`Request::Stick`, 5c-d3:
//! dropped for now), the guests' remote pages and their clients' commands (5c-d2).
use event_horizon_observe::{self as observe, BlockKey, BlockSnapshot, Request};
use horizon_client::{data_cmd, lang, nbt_path::NbtPath};
use horizon_core::geometry::Direction;
use horizon_core::nbt::{Compound, Tag};
use mod_api::{BlockShapeKind, EntityRef, LightLayer, ServerHost};
use std::sync::Mutex;

/// Ticks since the tools first read anything: the x axis of `/watch`.
static TICKS: Mutex<u64> = Mutex::new(0);

/// The server's reads a tick makes, as a seam: the live host implements it ([`Live`]); the tests
/// implement it with fakes.
pub trait Server {
    /// The dimension id of the player whose entity id is `id`, where the player is now. None when
    /// no player has that id.
    fn player_dimension(&self, id: u32) -> Option<String>;
    /// The data of entity `id` in `dimension`, as `/data get entity` shows it. None when there is no
    /// such entity there, or when the data cannot be read (see [`Live::entity_data`]).
    fn entity_data(&mut self, dimension: &str, id: u32) -> Option<Compound>;
    /// The name of entity `id` in `dimension`, when it has one.
    fn entity_name(&self, dimension: &str, id: u32) -> Option<String>;
    /// Whether entity `id` in `dimension` is a living mob that is alive.
    fn is_mob(&self, dimension: &str, id: u32) -> bool;
    /// The facts of the block at `key`, read in its level; `face` is the clicked face's unit
    /// vector, for the light there. None when the level is not open or the block cannot be read,
    /// and the window keeps its last snapshot.
    fn block(&mut self, key: &BlockKey, face: [i32; 3]) -> Option<BlockSnapshot>;
}

/// The live host, as the tick reads it.
struct Live<'a> {
    host: &'a mut dyn ServerHost,
}

impl Live<'_> {
    /// The entity `id` when it is in `dimension`: the server's entity of that id, if its level is
    /// the one `dimension` names. The tick runs with no level in place, so the level is the
    /// entity's own, not the hook's.
    fn entity_in(&self, dimension: &str, id: u32) -> Option<EntityRef> {
        let entity = self.host.entity(id)?;
        let level = entity.level(&*self.host)?;
        (self.host.dev_level_dimension(level)? == dimension).then_some(entity)
    }
}

impl Server for Live<'_> {
    fn player_dimension(&self, id: u32) -> Option<String> {
        let player = self
            .host
            .players()
            .server_players()
            .into_iter()
            .find(|p| p.entity().id() == id)?;
        let level = player.level(&*self.host)?;
        self.host.dev_level_dimension(level)
    }

    /// The entity's saved data (`/data get entity`), through the developer tools' own read
    /// (`dev_entity_nbt`), once the entity is found in `dimension`.
    fn entity_data(&mut self, dimension: &str, id: u32) -> Option<Compound> {
        let entity = self.entity_in(dimension, id)?;
        self.host.dev_entity_nbt(entity)
    }

    fn entity_name(&self, dimension: &str, id: u32) -> Option<String> {
        self.entity_in(dimension, id)?.name(&*self.host)
    }

    fn is_mob(&self, dimension: &str, id: u32) -> bool {
        let Some(entity) = self.entity_in(dimension, id) else {
            return false;
        };
        let Some(mob) = self.host.mob(entity) else {
            return false;
        };
        mob.living().alive(&*self.host) == Some(true)
    }

    /// The block's facts, through the host's reads. The level is the dimension's; the state is
    /// the block's name and properties, split from `block_state_string`. The block entity's data,
    /// the light (block and sky, here and at the face), the biome, the power, the six strong
    /// signals in `Direction::ALL` order, and the collision and outline boxes in world
    /// coordinates. A read that fails gives the empty value (no entity, no biome, 0), except the
    /// level and the state: without them there is no snapshot. Random ticks, pending ticks, tags
    /// and the declaring mod have no read on this base; they stay empty (the window says so).
    fn block(&mut self, key: &BlockKey, face: [i32; 3]) -> Option<BlockSnapshot> {
        let host: &dyn ServerHost = &*self.host;
        let level = host.level(&key.dimension)?;
        let pos = key.pos;
        mod_api::__private::backend_read(host, |backend| {
            let state = backend.level_block_state(level, pos)?;
            let (name, properties) = split_state(&host.block_state_string(state)?);
            let light = |layer: LightLayer, at: [i32; 3]| {
                backend
                    .level_brightness(level, layer, at)
                    .unwrap_or(0)
                    .clamp(0, 15) as u8
            };
            let face_at = [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]];
            let entity = host
                .block_entity(level, pos)
                .and_then(|e| backend.block_entity_data(e))
                .map(Tag::Compound);
            let shape = |kind| {
                backend
                    .level_block_shape(level, state, pos, kind)
                    .unwrap_or_default()
            };
            Some(BlockSnapshot {
                name,
                properties,
                entity,
                light: [light(LightLayer::Block, pos), light(LightLayer::Sky, pos)],
                face_light: [
                    light(LightLayer::Block, face_at),
                    light(LightLayer::Sky, face_at),
                ],
                biome: backend.level_biome_at(level, pos),
                power: backend.level_best_neighbor_signal(level, pos).unwrap_or(0),
                strong: Direction::ALL
                    .map(|d| backend.level_direct_signal(level, pos, d).unwrap_or(0)),
                random_ticks: false,
                scheduled: Vec::new(),
                tags: Vec::new(),
                declared_by: None,
                collision: shape(BlockShapeKind::Collision),
                outline: shape(BlockShapeKind::Outline),
                taken: std::time::Instant::now(),
            })
        })
        .flatten()
    }
}

/// A state string as `block_state_string` gives it, `minecraft:repeater[delay=1,facing=north]`,
/// as its name and its properties in the order written.
fn split_state(text: &str) -> (String, Vec<(String, String)>) {
    let Some((name, rest)) = text.split_once('[') else {
        return (text.to_owned(), Vec::new());
    };
    let properties = rest
        .trim_end_matches(']')
        .split(',')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    (name.to_owned(), properties)
}

/// Once a server tick, after every level ticked, while the world runs (the mod's `server_tick`
/// hook at `ServerTickEnd`): the store's requests first, then each watched entity's data and the
/// watched paths' values, and the AI logs' lines. An entity that is gone keeps its last data,
/// which the windows mark stale.
pub fn tick(host: &mut dyn ServerHost) {
    run(&mut Live { host });
}

/// The tick's steps, over the seam. The guests' clients' commands and pages (5c-d2) are not ported:
/// they run here once their chunk lands.
fn run(server: &mut dyn Server) {
    requests(server);
    let logging = observe::is_logging();
    if !observe::watching() && !logging {
        return;
    }
    blocks_tick(server);
    let tick = {
        let mut t = TICKS.lock().unwrap_or_else(|e| e.into_inner());
        *t += 1;
        *t
    };
    for (id, remembered) in observe::watch_entries() {
        let Some(dimension) = dimension_of(server, id, remembered.as_deref()) else {
            continue;
        };
        let Some(data) = server.entity_data(&dimension, id) else {
            continue;
        };
        let name = server
            .entity_name(&dimension, id)
            .unwrap_or_else(|| lang::get("horizon.dev_tools.entity"));
        let root = Tag::Compound(data);
        for path in observe::paths_of(id) {
            observe::set_sample(id, &path, tick, sample(&path, &root));
        }
        observe::set_data(id, name, root);
        if logging
            && let Some(brain) = observe::brain_of(id)
            && let Ok(line) = serde_json::to_string(&brain)
        {
            observe::write_log_line(id, &line);
        }
    }
}

/// Each watched block's facts, read through the seam once a tick. A block whose read gives none
/// keeps its last snapshot.
fn blocks_tick(server: &mut dyn Server) {
    for block in observe::blocks() {
        if let Some(snapshot) = server.block(&block.key, block.face) {
            observe::set_block(&block.key, snapshot);
        }
    }
}

/// The dimension a remembered entity is read in: the one the command remembered it with; for a
/// player (remembered with none), the dimension the player is in now. None when the player left.
fn dimension_of(server: &dyn Server, id: u32, remembered: Option<&str>) -> Option<String> {
    match remembered {
        Some(dimension) => Some(dimension.to_owned()),
        None => server.player_dimension(id),
    }
}

/// The value of the numeric NBT path `path` in `root`, or why there is none.
fn sample(path: &str, root: &Tag) -> Result<f64, String> {
    let parsed = NbtPath::parse(path).map_err(|e| format!("{e:?}"))?;
    let tag = data_cmd::single_tag(&parsed, root).map_err(|_| "no such path".to_string())?;
    number(&tag).ok_or_else(|| "not a number".to_string())
}

/// What the windows and the stick asked the game for since the last tick. The Goals window's log
/// button starts or stops one mob's log, as `/mobai log` does. The inspect stick is not ported yet
/// (5c-d3): its request is dropped, with a debug line.
fn requests(server: &mut dyn Server) {
    for request in observe::take_requests() {
        match request {
            Request::StartLog(id) => start_log(server, id),
            Request::StopLog(id) => {
                observe::stop_log(id);
                observe::forget_unwatched();
            }
            Request::Stick { player, target } => {
                log::debug!(
                    "dev tools: an inspect stick use is not ported yet, dropped: {player:#x} {target:?}"
                );
            }
        }
    }
}

/// Starts mob `id`'s log, when a command named it as a mob in a dimension. Anything else is ignored.
fn start_log(server: &mut dyn Server, id: u32) {
    let named = observe::watch_entries().into_iter().find(|(i, dimension)| {
        *i == id && dimension.as_deref().is_some_and(|d| server.is_mob(d, id))
    });
    let Some((_, Some(dimension))) = named else {
        return;
    };
    let name = server.entity_name(&dimension, id).unwrap_or_default();
    if let Err(e) = observe::open_log(&name, id) {
        log::warn!("dev tools: {e}");
    }
}

/// The world closed: the tick count starts again. The store is reset by the engine when the world
/// closes (`dev_tools_world_closed`), so only the mod's own state is kept here.
pub fn world_closed() {
    *TICKS.lock().unwrap_or_else(|e| e.into_inner()) = 0;
}

/// A numeric tag's value, as `/watch` plots it: the integers and the floats; None for the rest.
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

/// The tests share the process-wide store: each takes this guard, and `commands`' tests take it too.
#[cfg(test)]
pub(crate) fn store_guard() -> std::sync::MutexGuard<'static, ()> {
    static STORE_TESTS: Mutex<()> = Mutex::new(());
    STORE_TESTS.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_horizon_observe::View;

    const OVERWORLD: &str = "minecraft:overworld";

    /// The server's reads as a test sets them: where each player is, each entity's data, name and
    /// whether it is a mob.
    #[derive(Default)]
    struct Fake {
        players: Vec<(u32, String)>,
        data: Vec<(u32, Compound)>,
        names: Vec<(u32, String)>,
        mobs: Vec<u32>,
        /// The block reads the tick gives, and the snapshot each block's read gives (None: no read).
        blocks: Vec<(BlockKey, BlockSnapshot)>,
        block_reads: Vec<BlockKey>,
    }

    impl Server for Fake {
        fn player_dimension(&self, id: u32) -> Option<String> {
            self.players
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, d)| d.clone())
        }
        fn entity_data(&mut self, _dimension: &str, id: u32) -> Option<Compound> {
            self.data
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, c)| c.clone())
        }
        fn entity_name(&self, _dimension: &str, id: u32) -> Option<String> {
            self.names
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, n)| n.clone())
        }
        fn is_mob(&self, _dimension: &str, id: u32) -> bool {
            self.mobs.contains(&id)
        }
        fn block(&mut self, key: &BlockKey, _face: [i32; 3]) -> Option<BlockSnapshot> {
            self.block_reads.push(key.clone());
            self.blocks
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, snapshot)| snapshot.clone())
        }
    }

    /// A repeater's facts, as a test's read gives them.
    fn repeater(delay: &str) -> BlockSnapshot {
        BlockSnapshot {
            name: "minecraft:repeater".to_owned(),
            properties: vec![("delay".to_owned(), delay.to_owned())],
            entity: None,
            light: [0, 15],
            face_light: [0, 15],
            biome: Some("minecraft:plains".to_owned()),
            power: 15,
            strong: [0, 0, 0, 0, 15, 0],
            random_ticks: false,
            scheduled: Vec::new(),
            tags: Vec::new(),
            declared_by: None,
            collision: vec![[0.0, 0.0, 0.0, 1.0, 0.125, 1.0]],
            outline: Vec::new(),
            taken: std::time::Instant::now(),
        }
    }

    fn at_origin() -> BlockKey {
        BlockKey {
            dimension: OVERWORLD.to_owned(),
            pos: [0, 64, 0],
        }
    }

    /// A zombie's data with a float `Health` of 20 and an int `Age` of 7.
    fn zombie() -> Compound {
        let mut c = Compound::new();
        c.insert("Health".into(), Tag::Float(20.0_f32.to_bits()));
        c.insert("Age".into(), Tag::Int(7));
        c
    }

    /// A clean store with `id` watched in the overworld, its data and name known to the fake.
    fn watched(id: u32) -> Fake {
        observe::reset();
        *TICKS.lock().unwrap() = 0;
        observe::remember(&[(id, Some(OVERWORLD.to_owned()))]);
        observe::add(&[id], View::Data);
        Fake {
            data: vec![(id, zombie())],
            names: vec![(id, "Zombie".to_owned())],
            ..Fake::default()
        }
    }

    /// The store's watch of `id`, as the windows see it.
    fn seen(id: u32) -> observe::Inspected {
        observe::snapshots()
            .into_iter()
            .find(|w| w.id == id)
            .expect("the entity is watched")
    }

    #[test]
    fn a_numeric_tag_is_its_value_and_the_rest_are_not_numbers() {
        assert_eq!(number(&Tag::Byte(-3)), Some(-3.0));
        assert_eq!(number(&Tag::Int(7)), Some(7.0));
        assert_eq!(number(&Tag::Long(1 << 40)), Some(1_099_511_627_776.0));
        assert_eq!(number(&Tag::Float(1.5_f32.to_bits())), Some(1.5));
        assert_eq!(number(&Tag::Double(2.25_f64.to_bits())), Some(2.25));
        assert_eq!(number(&Tag::String("12".into())), None);
    }

    #[test]
    fn a_watched_path_gives_its_number_and_a_bad_path_gives_its_error() {
        let root = Tag::Compound(zombie());
        assert_eq!(sample("Health", &root), Ok(20.0));
        assert_eq!(sample("Age", &root), Ok(7.0));
        assert_eq!(sample("Missing", &root), Err("no such path".to_owned()));
        assert!(sample("Health[", &root).is_err());
    }

    #[test]
    fn a_watched_entity_gets_its_data_and_name_after_one_tick() {
        let _guard = store_guard();
        let mut fake = watched(5);
        run(&mut fake);
        let snap = seen(5).data.expect("the data is stored after a tick");
        assert_eq!(snap.name, "Zombie");
        assert_eq!(snap.data, Tag::Compound(zombie()));
    }

    #[test]
    fn a_player_remembered_without_a_dimension_is_read_where_they_are() {
        let _guard = store_guard();
        let mut fake = watched(9);
        observe::reset();
        observe::remember(&[(9, None)]);
        observe::add(&[9], View::Data);
        fake.players = vec![(9, OVERWORLD.to_owned())];
        run(&mut fake);
        assert_eq!(
            seen(9).data.expect("read in the player's dimension").name,
            "Zombie"
        );
    }

    #[test]
    fn a_watched_path_gets_a_sample_at_the_tick_and_a_bad_path_an_error() {
        let _guard = store_guard();
        let mut fake = watched(5);
        observe::add_path(&[5], "Health");
        observe::add_path(&[5], "Missing");
        run(&mut fake);
        let series = seen(5).series;
        let health = series.iter().find(|s| s.path == "Health").expect("sampled");
        assert_eq!(health.samples.back(), Some(&(1, 20.0)));
        let missing = series
            .iter()
            .find(|s| s.path == "Missing")
            .expect("sampled");
        assert_eq!(missing.error.as_deref(), Some("no such path"));
        assert!(missing.samples.is_empty());
    }

    #[test]
    fn nothing_watched_returns_early_and_does_not_count_a_tick() {
        let _guard = store_guard();
        observe::reset();
        *TICKS.lock().unwrap() = 0;
        run(&mut Fake::default());
        assert_eq!(*TICKS.lock().unwrap(), 0);
        let mut watching = watched(5);
        run(&mut watching);
        assert_eq!(*TICKS.lock().unwrap(), 1);
    }

    #[test]
    fn a_log_for_a_mob_starts_and_one_for_another_entity_is_ignored() {
        let _guard = store_guard();
        let dir = tempfile::tempdir().expect("a scratch folder");
        let before = std::env::current_dir().expect("a working folder");
        std::env::set_current_dir(dir.path()).expect("into the scratch folder");
        let mut fake = watched(5);
        observe::request(Request::StartLog(6));
        run(&mut fake);
        assert!(!observe::is_logging(), "entity 6 was never named");
        observe::request(Request::StartLog(5));
        run(&mut fake);
        assert!(!observe::is_logging(), "entity 5 is no mob");
        fake.mobs = vec![5];
        observe::request(Request::StartLog(5));
        run(&mut fake);
        assert!(observe::is_logging(), "a named mob's log is open");
        observe::request(Request::StopLog(5));
        run(&mut fake);
        let log_file = seen(5).log_file;
        std::env::set_current_dir(before).expect("back to the working folder");
        assert_eq!(log_file, None, "the window's log mark is cleared");
        assert!(!observe::is_logging(), "the stopped log's file is closed");
    }

    #[test]
    fn a_watched_block_gets_its_snapshot_once_a_tick() {
        let _guard = store_guard();
        observe::reset();
        observe::add_block(at_origin(), [0, 1, 0]);
        let mut fake = Fake {
            blocks: vec![(at_origin(), repeater("1"))],
            ..Fake::default()
        };
        run(&mut fake);
        let shown = observe::blocks()[0]
            .snapshot
            .clone()
            .expect("the block is read after one tick");
        assert_eq!(shown.name, "minecraft:repeater");
        assert_eq!(shown.properties, vec![("delay".to_owned(), "1".to_owned())]);
        assert_eq!(fake.block_reads, vec![at_origin()]);
        observe::reset();
    }

    #[test]
    fn a_block_whose_read_gives_nothing_keeps_its_last_snapshot() {
        let _guard = store_guard();
        observe::reset();
        observe::add_block(at_origin(), [0, 1, 0]);
        let mut fake = Fake {
            blocks: vec![(at_origin(), repeater("1"))],
            ..Fake::default()
        };
        run(&mut fake);
        fake.blocks.clear();
        run(&mut fake);
        assert_eq!(
            observe::blocks()[0]
                .snapshot
                .as_ref()
                .map(|s| s.properties.clone()),
            Some(vec![("delay".to_owned(), "1".to_owned())]),
            "the read gave nothing, so the old snapshot stays"
        );
        observe::reset();
    }

    #[test]
    fn nothing_watched_reads_no_block() {
        let _guard = store_guard();
        observe::reset();
        let mut fake = Fake {
            blocks: vec![(at_origin(), repeater("1"))],
            ..Fake::default()
        };
        run(&mut fake);
        assert!(fake.block_reads.is_empty());
    }

    #[test]
    fn a_state_string_splits_into_its_name_and_its_properties_in_order() {
        assert_eq!(
            split_state("minecraft:repeater[delay=1,facing=north,powered=false]"),
            (
                "minecraft:repeater".to_owned(),
                vec![
                    ("delay".to_owned(), "1".to_owned()),
                    ("facing".to_owned(), "north".to_owned()),
                    ("powered".to_owned(), "false".to_owned()),
                ]
            )
        );
        assert_eq!(
            split_state("minecraft:grass_block"),
            ("minecraft:grass_block".to_owned(), Vec::new())
        );
    }

    #[test]
    fn the_world_closing_starts_the_tick_count_again() {
        let _guard = store_guard();
        *TICKS.lock().unwrap() = 9;
        world_closed();
        assert_eq!(*TICKS.lock().unwrap(), 0);
    }
}
