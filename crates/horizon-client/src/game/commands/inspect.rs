//! The developer tools' commands (docs/dev-tools.md): `/inspect`, `/goals`,
//! `/senses`, `/nav` and `/watch` name entities for the egui windows, `/mobai
//! log` writes a mob's AI snapshot to a file each tick. Each server tick the
//! data `/data get entity` prints goes to `mobs::observe` for every watched
//! entity, read with the same `data_of`; nothing is written back.
use super::{Arg, Context, Game, R, Target, data::DataRef, err};
use horizon_client::{
    data_cmd,
    mobs::observe::{self, View},
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
    WATCH.lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|t| id_of(*t).is_some_and(|id| watched.contains(&id) || logged.contains(&id)));
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Game<'_> {
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

    pub(super) fn tools_note(&self) -> &'static str {
        if crate::dev_tools::enabled() {
            ""
        } else {
            " (the windows need --dev-tools or HORIZON_DEV_TOOLS=1 on the window's game)"
        }
    }

    /// `/inspect|goals|senses|nav <targets>`: open that window for them;
    /// `… clear` closes it everywhere.
    pub(super) fn cmd_view(&mut self, ctx: &Context, view: View, clear: bool) -> R<i32> {
        let name = match view {
            View::Data => "inspect",
            View::Goals => "goals",
            View::Senses => "senses",
            View::Nav => "nav",
        };
        if clear {
            observe::clear(Some(view));
            forget_unwatched();
            self.feedback(Text::literal(format!("Closed every /{name} window")));
            return Ok(0);
        }
        let (targets, ids, skipped) = self.watchable(ctx)?;
        remember(&targets);
        observe::add(&ids, view);
        self.dev_inspect_tick();
        let mut line = format!("/{name}: {}", plural(ids.len(), "entity", "entities"));
        if skipped > 0 {
            line.push_str(&format!(" ({skipped} player(s) left out)"));
        }
        line.push_str(self.tools_note());
        self.feedback(Text::literal(line));
        Ok(ids.len() as i32)
    }

    /// `/inspect` alone: what is watched, and how to use the tools.
    pub(super) fn cmd_inspect_list(&mut self) -> R<i32> {
        let watched = observe::snapshots();
        if watched.is_empty() {
            self.feedback(Text::literal(
                "Watching nothing. /inspect <targets> opens a dev tools window per entity \
                 with its /data (also /goals, /senses, /nav, /watch <targets> <path>, \
                 /mobai log <targets>), e.g. /inspect @e[type=zombie,sort=nearest,limit=1]; \
                 /inspect clear closes them.",
            ));
            return Ok(0);
        }
        let names: Vec<String> = watched
            .iter()
            .map(|w| {
                let name = w
                    .data
                    .as_ref()
                    .map_or_else(|| "entity".to_string(), |d| d.name.clone());
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
        self.feedback(Text::literal(format!("Watching {}", names.join("; "))));
        Ok(watched.len() as i32)
    }

    /// `/watch <targets> <path>`: a numeric NBT path over time; `/watch clear`.
    pub(super) fn cmd_watch(&mut self, ctx: &Context, clear: bool) -> R<i32> {
        if clear {
            observe::clear_paths();
            forget_unwatched();
            self.feedback(Text::literal("Closed every /watch window"));
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
        let mut line = format!("/watch {path}: {}", plural(ids.len(), "entity", "entities"));
        if skipped > 0 {
            line.push_str(&format!(" ({skipped} player(s) left out)"));
        }
        line.push_str(self.tools_note());
        self.feedback(Text::literal(line));
        Ok(ids.len() as i32)
    }

    /// `/mobai log <targets>`: each tick, the mob's AI snapshot as a JSON
    /// line in `dev-tools/logs/`; `/mobai log stop` closes the files.
    pub(super) fn cmd_mobai_log(&mut self, ctx: &Context, stop: bool) -> R<i32> {
        if stop {
            let n = LOGS.lock().unwrap_or_else(|e| e.into_inner()).drain(..).count();
            observe::clear_logs();
            forget_unwatched();
            self.feedback(Text::literal(format!("Closed {}", plural(n, "AI log", "AI logs"))));
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
        let dir = std::path::Path::new(DIR).join("logs");
        std::fs::create_dir_all(&dir)
            .map_err(|e| super::CommandError::new(Text::literal(format!("{}: {e}", dir.display()))))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let mut opened = Vec::new();
        {
            let mut logs = LOGS.lock().unwrap_or_else(|e| e.into_inner());
            for &(t, id) in &mobs {
                if logs.iter().any(|(i, _)| *i == id) {
                    continue;
                }
                let name = self
                    .target_name(t)
                    .plain()
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect::<String>();
                let path = dir.join(format!("mobai-{name}-{id}-{stamp}.jsonl"));
                match std::fs::File::create(&path) {
                    Ok(file) => {
                        logs.push((id, file));
                        opened.push(path.display().to_string());
                    }
                    Err(e) => {
                        return Err(super::CommandError::new(Text::literal(format!(
                            "{}: {e}",
                            path.display()
                        ))));
                    }
                }
            }
        }
        remember(&mobs.iter().map(|(t, _)| *t).collect::<Vec<_>>());
        // The snapshots come from the AI step, which only watches what the
        // store lists: a log is a watch without a window.
        let ids: Vec<u32> = mobs.iter().map(|(_, id)| *id).collect();
        observe::add_log(&ids);
        self.feedback(Text::literal(if opened.is_empty() {
            "Already logging those".to_string()
        } else {
            format!("Logging to {}", opened.join(", "))
        }));
        Ok(opened.len() as i32)
    }

    /// Once a server tick, after every level ticked: each watched entity's
    /// data (read where `/data get entity` reads it), the watched paths'
    /// values, and the AI logs' lines. An entity that is gone keeps its
    /// last data, which the windows mark stale.
    pub(in crate::game) fn dev_inspect_tick(&mut self) {
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
