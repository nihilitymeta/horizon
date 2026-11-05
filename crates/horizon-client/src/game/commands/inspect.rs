//! The developer tools' commands (docs/dev-tools.md): `/inspect`, `/goals`,
//! `/senses`, `/nav` and `/watch` name entities for the egui windows, `/mobai
//! log` writes a mob's AI snapshot to a file each tick. Each server tick the
//! data `/data get entity` prints goes to `mobs::observe` for every watched
//! entity, read with the same `data_of`; nothing is written back.
use super::{Arg, Context, Game, R, Target, data::DataRef, err};
use horizon_client::{
    data_cmd,
    mobs::observe::{self, Request, View},
    nbt_path::NbtPath,
    text::Text,
};
use horizon_world::nbt::Tag;
use std::io::Write;
use std::sync::Mutex;

/// The entities watched, as the selectors found them; the lib's store holds
/// their ids. One list a process: single player's server thread writes the
/// data its window shows.
static WATCH: Mutex<Vec<Target>> = Mutex::new(Vec::new());
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

/// Remember targets a command or the Lua console named, so the tick reads their data.
pub(super) fn remember(targets: &[Target]) {
    let mut watch = WATCH.lock().unwrap_or_else(|e| e.into_inner());
    for &t in targets {
        if !watch.contains(&t) {
            watch.push(t);
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
        .retain(|t| id_of(*t).is_some_and(|id| watched.contains(&id) || logged.contains(&id)));
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
        *TICKS.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        observe::reset();
        super::lua_console::world_closed();
        crate::dev_tools::world_closed();
    }

    /// The targets of the command with their ids, players left out.
    fn watchable(&mut self, ctx: &Context) -> R<(Vec<Target>, Vec<u32>, usize)> {
        let targets = self.entities(ctx, "targets", false)?;
        let watched: Vec<Target> = targets
            .iter()
            .copied()
            .filter(|t| id_of(*t).is_some())
            .collect();
        if watched.is_empty() {
            return Err(err("argument.entity.notfound.entity", &[]));
        }
        let ids = watched.iter().filter_map(|t| id_of(*t)).collect();
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
        remember(&targets);
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
        remember(&targets);
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
        let logging = !LOGS.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if !observe::watching() && !logging {
            return;
        }
        let tick = {
            let mut t = TICKS.lock().unwrap_or_else(|e| e.into_inner());
            *t += 1;
            *t
        };
        let targets = WATCH.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for t in targets {
            let Some(id) = id_of(t) else {
                continue;
            };
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
                        .copied()
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
            }
        }
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
