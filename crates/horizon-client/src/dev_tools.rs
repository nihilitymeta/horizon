//! Developer tools drawn with egui (docs/dev-tools.md). On with
//! `HORIZON_DEV_TOOLS=1`: a window over the world for each entity
//! `/inspect <targets>` names, showing the data `/data get entity` prints as
//! a tree (and a mob's goals), for people working on mods and engine
//! features, off in a player's game. Nothing is drawn until something is
//! inspected. A game with Lua mods has the windows without the commands, for
//! the inspect stick (`horizon.dev.inspect`): they show only while it opened
//! one.
//!
//! This is the windows' half: it draws with the shared state of
//! [`crate::dev_input`] and never the other way round, so the two can move
//! apart (#19).
pub mod console;

use crate::dev_input;
use crate::gfx::native_render::{RenderHooks, WorldDrawnEvent};
use crate::mobs::observe::{
    self, BrainSnapshot, GoalRow, HistoryTick, Inspected, InspectedBlock, Request, Series, View,
};
use crate::mod_api::{Hooks, Host, NativeBlockRim, VisualsEvent};
use horizon_client_core::snbt::java_hash_order;
use horizon_world::nbt::Tag;
use horizon_world::snbt_text::to_snbt;
use std::sync::Mutex;

/// The environment variable that turns the tools on (`1`); `--dev-tools` on
/// the command line does the same (a launcher's game arguments).
pub const ENV: &str = "HORIZON_DEV_TOOLS";
/// The launch flag that turns the tools on.
pub const FLAG: &str = "--dev-tools";

/// The built-in mods of a plain `horizon-client`: the dev tools when asked
/// for, and their windows alone when `scripted` (Lua mods are on, which may
/// use the inspect stick). A game without either has no mod here, so the
/// hooks' fast paths for a game without mods stay.
pub fn mods(scripted: bool) -> Vec<Box<dyn Hooks>> {
    let mut mods: Vec<Box<dyn Hooks>> = Vec::new();
    let asked = std::env::var(ENV).is_ok_and(|v| v == "1") || std::env::args().any(|a| a == FLAG);
    if asked || scripted {
        dev_input::install();
        // Their key, Free Mouse for Dev Windows, shows in Key Binds.
        horizon_client::keybinds::show_dev_tools_key(true);
    }
    if !asked && scripted {
        mods.push(Box::new(DevTools::default()));
    }
    if asked {
        log::info!(
            "Horizon: developer tools on ({ENV}=1 or {FLAG}); Free Mouse for Dev Windows (Key Binds) frees the pointer"
        );
        dev_input::enable();
        // The commands come with the tools and only then: they run Lua and
        // the owner's commands.
        horizon_client::command::vanilla::set_dev_tools_commands(true);
        mods.push(Box::new(DevTools::default()));
    }
    mods
}

/// The egui wgpu renderer, made on the first frame for the frame's colour format.
struct Painter {
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
}

#[derive(Default)]
pub struct DevTools {
    painter: Mutex<Option<Painter>>,
}

fn flags_label(flags: u8) -> String {
    let mut names = Vec::new();
    for (bit, name) in [(1, "move"), (2, "look"), (4, "jump"), (8, "target")] {
        if flags & bit != 0 {
            names.push(name);
        }
    }
    if names.is_empty() {
        "-".to_string()
    } else {
        names.join(" ")
    }
}

fn pos_label(p: [f64; 3]) -> String {
    format!("{:.2} {:.2} {:.2}", p[0], p[1], p[2])
}

fn block_label(p: [i32; 3]) -> String {
    format!("{} {} {}", p[0], p[1], p[2])
}

/// Whether a key passes the window's filter: the filter is empty, or the
/// key (lower-cased) contains it.
fn key_matches(key: &str, filter: &str) -> bool {
    filter.is_empty() || key.to_lowercase().contains(filter)
}

/// A compound's key as an NBT path writes it: as it is, or quoted when it
/// holds a character that ends an unquoted key (`NbtPath::parse`).
fn path_key(key: &str) -> String {
    let plain = !key.is_empty()
        && !key
            .bytes()
            .any(|b| matches!(b, b' ' | b'"' | b'\'' | b'[' | b']' | b'.' | b'{' | b'}'));
    if plain {
        key.to_string()
    } else {
        format!("\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// The NBT path of a compound's child `key` under `parent` (empty: the root).
fn child_path(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        path_key(key)
    } else {
        format!("{parent}.{}", path_key(key))
    }
}

/// Whether `/watch` charts the tag: a number.
fn watchable(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Byte(_) | Tag::Short(_) | Tag::Int(_) | Tag::Long(_) | Tag::Float(_) | Tag::Double(_)
    )
}

/// One node of the data tree: a compound or list folds, a leaf shows its
/// SNBT value as `/data get` prints it. `path` is the node's NBT path, the
/// salt of its fold state and the tooltip (it is what `/data get entity
/// <target> <path>` takes). With a filter, leaves whose key does not match
/// are left out, a container whose key matches shows all of itself, and
/// the rest open to show what matched inside. A number has a watch button,
/// which charts that path of entity `id` as `/watch` does (none without `id`).
fn tag_node(
    ui: &mut egui::Ui,
    id: Option<u32>,
    key: &str,
    path: &str,
    tag: &Tag,
    depth: usize,
    filter: &str,
) {
    let filter = if key_matches(key, filter) { "" } else { filter };
    match tag {
        Tag::Compound(c) => {
            if !filter.is_empty() && !compound_matches(c, filter) {
                return;
            }
            egui::CollapsingHeader::new(format!("{key}  {{{}}}", c.len()))
                .id_salt(path)
                .default_open(depth == 0 || !filter.is_empty())
                .show(ui, |ui| {
                    for (k, v) in java_hash_order(c) {
                        let k = k.to_string_lossy();
                        let child = child_path(path, &k);
                        tag_node(ui, id, &k, &child, v, depth + 1, filter);
                    }
                })
                .header_response
                .on_hover_text(if path.is_empty() {
                    "the whole entity"
                } else {
                    path
                });
        }
        Tag::List(items) => {
            if !filter.is_empty() && !list_matches(items, filter) {
                return;
            }
            egui::CollapsingHeader::new(format!("{key}  [{}]", items.len()))
                .id_salt(path)
                .default_open(!filter.is_empty() || (depth <= 1 && items.len() <= 8))
                .show(ui, |ui| {
                    for (i, v) in items.iter().enumerate() {
                        let child = format!("{path}[{i}]");
                        tag_node(ui, id, &format!("[{i}]"), &child, v, depth + 1, filter);
                    }
                })
                .header_response
                .on_hover_text(path);
        }
        leaf => {
            if !filter.is_empty() {
                return;
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{key}:")).color(egui::Color32::LIGHT_BLUE))
                    .on_hover_text(path);
                ui.add(egui::Label::new(egui::RichText::new(to_snbt(leaf)).monospace()).wrap());
                if let Some(id) = id
                    && watchable(leaf)
                    && ui
                        .small_button("watch")
                        .on_hover_text(format!("chart {path} over time, as /watch does"))
                        .clicked()
                {
                    observe::add_path(&[id], path);
                }
            });
        }
    }
}

fn compound_matches(c: &horizon_world::nbt::Compound, filter: &str) -> bool {
    c.iter().any(|(k, v)| {
        key_matches(&k.to_string_lossy(), filter)
            || match v {
                Tag::Compound(inner) => compound_matches(inner, filter),
                Tag::List(items) => list_matches(items, filter),
                _ => false,
            }
    })
}

fn list_matches(items: &[Tag], filter: &str) -> bool {
    items.iter().any(|v| match v {
        Tag::Compound(inner) => compound_matches(inner, filter),
        Tag::List(inner) => list_matches(inner, filter),
        _ => false,
    })
}

/// The window's title for an entity: its name and id.
fn entity_title(item: &Inspected, what: &str) -> String {
    match (&item.data, &item.brain) {
        (Some(d), _) => format!("{what}: {} #{}", d.name, item.id),
        (None, Some(b)) => format!("{what}: {} #{}", b.kind, item.id),
        (None, None) => format!("{what}: entity #{}", item.id),
    }
}

/// A yellow line when the snapshot stopped updating: half a second late,
/// past the interval a window the server shows comes at.
fn stale_note(ui: &mut egui::Ui, taken: std::time::Instant, what: &str) {
    let stale = taken.elapsed();
    let every = observe::remote().map_or(0, |(_, ms)| u128::from(ms));
    if stale.as_millis() > every + 500 {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!(
                "no {what} for {:.1} s: gone, unloaded or paused",
                stale.as_secs_f32()
            ),
        );
    }
}

/// The interval a window the server shows (a guest's) comes at, as a
/// choice: the server keeps it, down to its floor.
fn interval_choice(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug) {
    let Some((_, mut ms)) = observe::remote() else {
        return;
    };
    let label = |ms: u32| format!("every {} s", f64::from(ms) / 1000.0);
    let before = ms;
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(ms))
        .show_ui(ui, |ui| {
            for choice in crate::mobs::observe_wire::INTERVALS {
                ui.selectable_value(&mut ms, choice, label(choice));
            }
        })
        .response
        .on_hover_text("how often the server sends this window");
    if ms != before {
        observe::set_remote_interval(ms);
    }
}

/// An entity window's stop button: a window the server shows ends there too.
fn stop_entity(ui: &mut egui::Ui, id: u32, view: View) {
    let remote = observe::is_remote_entity(id);
    if ui.button("stop").clicked() {
        if remote {
            observe::close_remote();
        } else {
            observe::remove(id, Some(view));
        }
    }
    if remote {
        interval_choice(ui, ("interval", id));
    }
}

/// The top line of the mob windows: stop button and the usual warnings.
fn mob_header<'a>(ui: &mut egui::Ui, item: &'a Inspected, view: View) -> Option<&'a BrainSnapshot> {
    ui.horizontal(|ui| stop_entity(ui, item.id, view));
    match &item.brain {
        None => {
            ui.weak("no AI step yet: not a mob, or not ticked since it was named");
            None
        }
        Some(b) => {
            stale_note(ui, b.taken, "AI tick");
            if b.no_ai {
                ui.colored_label(egui::Color32::LIGHT_RED, "NoAI: goals do not run");
            }
            if !b.alive {
                ui.colored_label(egui::Color32::LIGHT_RED, "dead");
            }
            Some(b)
        }
    }
}

fn inspect_window(ctx: &egui::Context, item: &Inspected) {
    let window_id = egui::Id::unique(("dev tools inspect", item.id));
    egui::Window::new(entity_title(item, "data"))
        .id(window_id)
        .default_width(420.0)
        .default_height(520.0)
        .vscroll(true)
        .show(ctx, |ui| {
            let filter_id = window_id.with("filter");
            let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
            ui.horizontal(|ui| {
                ui.label("filter");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut filter)
                        .hint_text("key, e.g. Health or Pos")
                        .desired_width(160.0),
                );
                if edit.changed() {
                    ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
                }
                stop_entity(ui, item.id, View::Data);
            });
            let filter = filter.to_lowercase();
            // A window the server shows has no charts: /watch reads this game's world.
            let watch = !observe::is_remote_entity(item.id);
            match &item.data {
                None => {
                    ui.weak("no data yet: not ticked since it was named, or no longer there");
                }
                Some(d) => {
                    stale_note(ui, d.taken, "update");
                    ui.separator();
                    tag_node(
                        ui,
                        watch.then_some(item.id),
                        "entity data",
                        "",
                        &d.data,
                        0,
                        &filter,
                    );
                    ui.separator();
                    egui::CollapsingHeader::new("SNBT, as /data get prints it")
                        .id_salt("snbt")
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(to_snbt(&d.data)).monospace())
                                    .wrap(),
                            );
                        });
                }
            }
        });
}

fn goal_table(ui: &mut egui::Ui, id: &str, rows: &[GoalRow]) {
    if rows.is_empty() {
        ui.weak("none");
        return;
    }
    egui::Grid::new(id)
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            ui.weak("prio");
            ui.weak("goal");
            ui.weak("state");
            ui.weak("flags");
            ui.end_row();
            for row in rows {
                ui.label(row.priority.to_string());
                if row.running {
                    ui.strong(row.name);
                    ui.colored_label(egui::Color32::LIGHT_GREEN, "running");
                } else {
                    ui.label(row.name);
                    ui.weak("idle");
                }
                ui.label(flags_label(row.flags));
                ui.end_row();
            }
        });
}

/// The goals' history as a strip per goal: a bar where it ran, over the
/// last ticks kept (`HISTORY_TICKS`). Target goals first, as the selectors
/// tick.
fn goal_timeline(ui: &mut egui::Ui, brain: &BrainSnapshot, history: &[HistoryTick]) {
    let names: Vec<&'static str> = brain
        .targets
        .iter()
        .chain(brain.goals.iter())
        .map(|g| g.name)
        .collect();
    if names.is_empty() || history.is_empty() {
        ui.weak("no history yet");
        return;
    }
    let (first, last) = (history[0].0, history[history.len() - 1].0);
    ui.weak(format!(
        "{} ticks, age {first} to {last}; a bar where the goal ran",
        history.len()
    ));
    let row_h = 14.0;
    let label_w = 170.0;
    let width = ui.available_width().max(200.0);
    let (response, painter) = ui.allocate_painter(
        egui::vec2(width, row_h * names.len() as f32),
        egui::Sense::hover(),
    );
    let rect = response.rect;
    let strip_x = rect.left() + label_w;
    let strip_w = (rect.width() - label_w).max(10.0);
    let step = strip_w / history.len() as f32;
    for (row, name) in names.iter().enumerate() {
        let y = rect.top() + row as f32 * row_h;
        if row % 2 == 1 {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), y),
                    egui::vec2(rect.width(), row_h),
                ),
                0.0,
                egui::Color32::from_white_alpha(6),
            );
        }
        painter.text(
            egui::pos2(rect.left() + 2.0, y + row_h / 2.0),
            egui::Align2::LEFT_CENTER,
            *name,
            egui::FontId::proportional(11.0),
            ui.visuals().text_color(),
        );
        let running_now = history.last().is_some_and(|(_, r)| r.contains(name));
        let color = if running_now {
            egui::Color32::LIGHT_GREEN
        } else {
            egui::Color32::from_rgb(90, 160, 120)
        };
        let mut run: Option<usize> = None;
        for i in 0..=history.len() {
            let on = i < history.len() && history[i].1.contains(name);
            match (run, on) {
                (None, true) => run = Some(i),
                (Some(start), false) => {
                    let x0 = strip_x + start as f32 * step;
                    let x1 = strip_x + i as f32 * step;
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x0, y + 2.0),
                            egui::pos2(x1.max(x0 + 1.0), y + row_h - 2.0),
                        ),
                        1.0,
                        color,
                    );
                    run = None;
                }
                _ => {}
            }
        }
    }
    if let Some(pos) = response.hover_pos()
        && pos.x >= strip_x
    {
        let i = (((pos.x - strip_x) / step) as usize).min(history.len() - 1);
        let (age, running) = &history[i];
        response.on_hover_text(format!(
            "age {age}: {}",
            if running.is_empty() {
                "nothing running".to_string()
            } else {
                running.join(", ")
            }
        ));
    }
}

/// The Goals window's log button, `/mobai log` for this mob: the game opens
/// the file at its next tick, and the line names it while it is written.
fn log_line(ui: &mut egui::Ui, item: &Inspected) {
    ui.horizontal(|ui| match &item.log_file {
        Some(file) => {
            if ui.button("stop log").clicked() {
                observe::request(Request::StopLog(item.id));
            }
            ui.colored_label(egui::Color32::LIGHT_GREEN, format!("logging to {file}"));
        }
        None => {
            if ui
                .button("log")
                .on_hover_text(
                    "write its AI snapshot each tick to dev-tools/logs/, as /mobai log does",
                )
                .clicked()
            {
                observe::request(Request::StartLog(item.id));
            }
        }
    });
}

fn goals_window(ctx: &egui::Context, item: &Inspected) {
    egui::Window::new(entity_title(item, "goals"))
        .id(egui::Id::unique(("dev tools goals", item.id)))
        .default_width(520.0)
        .vscroll(true)
        .show(ctx, |ui| {
            let Some(brain) = mob_header(ui, item, View::Goals) else {
                return;
            };
            if !observe::is_remote_entity(item.id) {
                log_line(ui, item);
            }
            ui.strong("goals");
            goal_table(ui, "goals", &brain.goals);
            ui.add_space(4.0);
            ui.strong("target goals");
            goal_table(ui, "targets", &brain.targets);
            ui.separator();
            ui.strong("history");
            goal_timeline(ui, brain, &item.history);
        });
}

fn senses_window(ctx: &egui::Context, item: &Inspected) {
    egui::Window::new(entity_title(item, "senses"))
        .id(egui::Id::unique(("dev tools senses", item.id)))
        .default_width(380.0)
        .vscroll(true)
        .show(ctx, |ui| {
            let Some(b) = mob_header(ui, item, View::Senses) else {
                return;
            };
            egui::Grid::new("senses")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    ui.weak("target");
                    ui.label(b.target.as_deref().unwrap_or("none"));
                    ui.end_row();
                    ui.weak("last hurt by");
                    ui.label(match &b.last_hurt_by {
                        Some(who) => format!("{who} at tick {}", b.last_hurt_by_time),
                        None => "nobody".to_string(),
                    });
                    ui.end_row();
                    ui.weak("follow range");
                    ui.label(format!("{:.1} blocks", b.follow_range));
                    ui.end_row();
                    ui.weak("nearest player");
                    ui.label(match b.nearest_player {
                        Some((p, d)) => format!(
                            "{:.1} blocks away at {}{}",
                            d,
                            pos_label(p),
                            if d <= b.follow_range {
                                " (in range)"
                            } else {
                                ""
                            }
                        ),
                        None => "none in the level".to_string(),
                    });
                    ui.end_row();
                    ui.weak("mood");
                    ui.label(format!(
                        "{}{}idle for {} ticks",
                        if b.aggressive { "aggressive, " } else { "" },
                        if b.panicking { "panicking, " } else { "" },
                        b.no_action_time
                    ));
                    ui.end_row();
                    ui.weak("home");
                    ui.label(match b.home {
                        Some((c, r)) => format!("{} within {r}", block_label(c)),
                        None => "none".to_string(),
                    });
                    ui.end_row();
                    ui.weak("health");
                    ui.label(format!("{:.1} / {:.1}", b.health, b.max_health));
                    ui.end_row();
                });
            ui.separator();
            ui.strong("sight this tick");
            if b.seen.is_empty() {
                ui.weak("no line-of-sight checks made");
            } else {
                for (who, seen) in &b.seen {
                    ui.horizontal(|ui| {
                        if *seen {
                            ui.colored_label(egui::Color32::LIGHT_GREEN, "seen");
                        } else {
                            ui.weak("hidden");
                        }
                        ui.label(who);
                    });
                }
            }
        });
}

fn nav_window(ctx: &egui::Context, item: &Inspected) {
    egui::Window::new(entity_title(item, "nav"))
        .id(egui::Id::unique(("dev tools nav", item.id)))
        .default_width(380.0)
        .vscroll(true)
        .show(ctx, |ui| {
            let Some(b) = mob_header(ui, item, View::Nav) else {
                return;
            };
            egui::Grid::new("nav")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    ui.weak("kind");
                    ui.label(&b.nav_kind);
                    ui.end_row();
                    ui.weak("position");
                    ui.label(pos_label(b.position));
                    ui.end_row();
                    ui.weak("going to");
                    ui.label(match b.nav_target {
                        Some(t) => format!(
                            "{} at speed {:.2}{}",
                            block_label(t),
                            b.nav_speed,
                            if b.nav_stuck { ", stuck" } else { "" }
                        ),
                        None => "nowhere".to_string(),
                    });
                    ui.end_row();
                    ui.weak("move control");
                    ui.label(format!("{} towards {}", b.move_operation, pos_label(b.move_wanted)));
                    ui.end_row();
                    ui.weak("input");
                    ui.label(format!(
                        "strafe {:.2} up {:.2} forward {:.2}, speed {:.3}",
                        b.input[0], b.input[1], b.input[2], b.speed
                    ));
                    ui.end_row();
                });
            ui.separator();
            match &b.path {
                None => {
                    ui.weak("no path");
                }
                Some(p) => {
                    ui.strong(format!(
                        "path: node {} of {} to {}{}",
                        p.next,
                        p.nodes.len(),
                        block_label(p.target),
                        if p.reached { ", reached" } else { "" }
                    ));
                    ui.weak("drawn in the world under the nodes: green the next, yellow the six ahead, red the target");
                    for (i, node) in p.nodes.iter().enumerate() {
                        let text = format!("[{i}] {}", block_label(*node));
                        if i == p.next {
                            ui.colored_label(egui::Color32::LIGHT_GREEN, text);
                        } else if i < p.next {
                            ui.weak(text);
                        } else {
                            ui.label(text);
                        }
                    }
                }
            }
        });
}

/// A line chart of the samples, with the range on the left.
fn series_chart(ui: &mut egui::Ui, series: &Series) {
    let n = series.samples.len();
    if n < 2 {
        ui.weak("waiting for samples");
        return;
    }
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(_, v) in &series.samples {
        min = min.min(v);
        max = max.max(v);
    }
    let last = series.samples[n - 1].1;
    ui.label(format!("last {last}, min {min}, max {max}, {n} samples"));
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width().max(200.0), 140.0),
        egui::Sense::hover(),
    );
    let rect = response.rect.shrink(4.0);
    painter.rect_filled(response.rect, 2.0, egui::Color32::from_white_alpha(6));
    let span = if (max - min).abs() < f64::EPSILON {
        1.0
    } else {
        max - min
    };
    let (first_tick, last_tick) = (series.samples[0].0, series.samples[n - 1].0);
    let ticks = (last_tick - first_tick).max(1) as f64;
    let points: Vec<egui::Pos2> = series
        .samples
        .iter()
        .map(|&(t, v)| {
            let x = rect.left() + ((t - first_tick) as f64 / ticks) as f32 * rect.width();
            let y = rect.bottom() - ((v - min) / span) as f32 * rect.height();
            egui::pos2(x, y)
        })
        .collect();
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(1.5, egui::Color32::LIGHT_GREEN),
    ));
    let font = egui::FontId::proportional(10.0);
    let color = ui.visuals().weak_text_color();
    painter.text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        format!("{max}"),
        font.clone(),
        color,
    );
    painter.text(
        rect.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        format!("{min}"),
        font.clone(),
        color,
    );
    painter.text(
        rect.right_bottom(),
        egui::Align2::RIGHT_BOTTOM,
        format!("{} ticks", last_tick - first_tick),
        font,
        color,
    );
    if let Some(pos) = response.hover_pos() {
        let i = (((pos.x - rect.left()) / rect.width()) * (n - 1) as f32)
            .round()
            .clamp(0.0, (n - 1) as f32) as usize;
        let (t, v) = series.samples[i];
        response.on_hover_text(format!("tick {t}: {v}"));
    }
}

fn watch_window(ctx: &egui::Context, item: &Inspected, series: &Series) {
    let title = format!("{} {}", entity_title(item, "watch"), series.path);
    egui::Window::new(title)
        .id(egui::Id::unique((
            "dev tools watch",
            item.id,
            series.path.clone(),
        )))
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.monospace(&series.path);
                if ui.button("stop").clicked() {
                    observe::remove_path(item.id, &series.path);
                }
            });
            if let Some(d) = &item.data {
                stale_note(ui, d.taken, "update");
            }
            if let Some(e) = &series.error {
                ui.colored_label(egui::Color32::YELLOW, e);
            }
            series_chart(ui, series);
        });
}

/// The face names of `Direction::ALL`, as vanilla names them.
const FACES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// The block window (the inspect stick): what the game knows of one block,
/// read once a server tick. Its collision and outline boxes are drawn in the
/// world while it is open (`debug_gizmos`).
fn block_window(ctx: &egui::Context, block: &InspectedBlock) {
    let key = &block.key;
    egui::Window::new(format!("block: {}", block_label(key.pos)))
        .id(egui::Id::unique((
            "dev tools block",
            &key.dimension,
            key.pos,
        )))
        .default_width(420.0)
        .default_height(520.0)
        .vscroll(true)
        .show(ctx, |ui| {
            let remote = observe::is_remote_block(key);
            ui.horizontal(|ui| {
                if ui.button("stop").clicked() {
                    if remote {
                        observe::close_remote();
                    } else {
                        observe::remove_block(key);
                    }
                }
                if remote {
                    interval_choice(ui, ("interval", &key.dimension, key.pos));
                }
            });
            let Some(b) = &block.snapshot else {
                ui.weak("no read yet: the level is not loaded, or not ticked since");
                return;
            };
            stale_note(ui, b.taken, "update");
            ui.strong(&b.name);
            egui::Grid::new("block")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    for (k, v) in &b.properties {
                        ui.weak(k);
                        ui.monospace(v);
                        ui.end_row();
                    }
                    ui.weak("dimension");
                    ui.label(&key.dimension);
                    ui.end_row();
                    ui.weak("biome");
                    ui.label(b.biome.as_deref().unwrap_or("unknown"));
                    ui.end_row();
                    ui.weak("light");
                    ui.label(format!(
                        "block {} sky {}; on the face {}: block {} sky {}",
                        b.light[0],
                        b.light[1],
                        block_label(block.face),
                        b.face_light[0],
                        b.face_light[1]
                    ));
                    ui.end_row();
                    ui.weak("power received");
                    ui.label(b.power.to_string())
                        .on_hover_text("getBestNeighborSignal");
                    ui.end_row();
                    ui.weak("strong power");
                    ui.label(
                        FACES
                            .iter()
                            .zip(b.strong)
                            .map(|(f, p)| format!("{f} {p}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                    .on_hover_text(
                        "getDirectSignal(pos, direction): the strong signal it gives that way",
                    );
                    ui.end_row();
                    ui.weak("random ticks");
                    ui.label(if b.random_ticks { "yes" } else { "no" });
                    ui.end_row();
                    ui.weak("scheduled ticks");
                    ui.label(if b.scheduled.is_empty() {
                        "none".to_string()
                    } else {
                        b.scheduled
                            .iter()
                            .map(|(kind, delay, priority)| {
                                format!("{kind} in {delay} ticks, priority {priority}")
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    });
                    ui.end_row();
                    ui.weak("declared by");
                    ui.label(match &b.declared_by {
                        Some((m, template)) => format!("{m}, template {template}"),
                        None => "the game".to_string(),
                    });
                    ui.end_row();
                    ui.weak("shapes");
                    ui.label(format!(
                        "{} collision boxes (blue), {} outline boxes (yellow), drawn in the world",
                        b.collision.len(),
                        b.outline.len()
                    ));
                    ui.end_row();
                });
            ui.separator();
            egui::CollapsingHeader::new(format!("tags  [{}]", b.tags.len()))
                .id_salt("tags")
                .show(ui, |ui| {
                    for t in &b.tags {
                        ui.monospace(t);
                    }
                });
            match &b.entity {
                None => {
                    ui.weak("no block entity");
                }
                Some(data) => {
                    ui.separator();
                    block_entity_tree(ui, data);
                }
            }
        });
}

/// A block entity's data as the Inspect window's tree, without watch buttons
/// (`/watch` charts entities).
fn block_entity_tree(ui: &mut egui::Ui, data: &Tag) {
    fn node(ui: &mut egui::Ui, key: &str, path: &str, tag: &Tag) {
        match tag {
            Tag::Compound(c) => {
                egui::CollapsingHeader::new(format!("{key}  {{{}}}", c.len()))
                    .id_salt(path)
                    .default_open(path.is_empty())
                    .show(ui, |ui| {
                        for (k, v) in java_hash_order(c) {
                            let k = k.to_string_lossy();
                            node(ui, &k, &child_path(path, &k), v);
                        }
                    });
            }
            Tag::List(items) => {
                egui::CollapsingHeader::new(format!("{key}  [{}]", items.len()))
                    .id_salt(path)
                    .show(ui, |ui| {
                        for (i, v) in items.iter().enumerate() {
                            node(ui, &format!("[{i}]"), &format!("{path}[{i}]"), v);
                        }
                    });
            }
            leaf => {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{key}:")).color(egui::Color32::LIGHT_BLUE),
                    )
                    .on_hover_text(path);
                    ui.add(egui::Label::new(egui::RichText::new(to_snbt(leaf)).monospace()).wrap());
                });
            }
        }
    }
    node(ui, "block entity data", "", data);
}

impl DevTools {
    fn ui(&self, ctx: &egui::Context) {
        console::window(ctx);
        for block in &observe::blocks() {
            block_window(ctx, block);
        }
        for item in &observe::snapshots() {
            for view in &item.views {
                match view {
                    View::Data => inspect_window(ctx, item),
                    View::Goals => goals_window(ctx, item),
                    View::Senses => senses_window(ctx, item),
                    View::Nav => nav_window(ctx, item),
                }
            }
            for series in &item.series {
                watch_window(ctx, item, series);
            }
        }
    }
}

impl Hooks for DevTools {
    fn render(&self) -> Option<&dyn RenderHooks> {
        Some(self)
    }
    /// `/nav`: the path drawn in the world as block outlines. A node is
    /// where the mob's feet go, usually air, and a rim takes the shape of
    /// the block at its position, so the block under each node is outlined.
    /// The game draws eight rims a frame, within 32 blocks: the next node
    /// and the target go first, then the nodes ahead.
    fn visuals(&self, _host: &dyn Host, event: &mut VisualsEvent<'_>) {
        if !observe::watching() {
            return;
        }
        let under = |p: [i32; 3]| [p[0], p[1] - 1, p[2]];
        for item in observe::snapshots() {
            if !item.views.contains(&View::Nav) {
                continue;
            }
            let Some(b) = &item.brain else { continue };
            let Some(p) = &b.path else { continue };
            if let Some(next) = p.nodes.get(p.next) {
                event.rims.push(NativeBlockRim {
                    pos: under(*next),
                    argb: 0xFF40FF40,
                    width: 3.0,
                });
            }
            event.rims.push(NativeBlockRim {
                pos: under(p.target),
                argb: 0xFFFF4040,
                width: 2.0,
            });
            for node in p.nodes.iter().skip(p.next + 1).take(6) {
                event.rims.push(NativeBlockRim {
                    pos: under(*node),
                    argb: 0xFFFFE040,
                    width: 1.5,
                });
            }
        }
    }
}

impl RenderHooks for DevTools {
    fn world_drawn(&self, event: &mut WorldDrawnEvent<'_, '_>) {
        // Without the tools, egui runs only while the stick's window is open.
        if !dev_input::enabled() && !observe::showing() {
            return;
        }
        let world = &mut *event.world;
        let [w, h] = world.size;
        if w == 0 || h == 0 {
            return;
        }
        let ctx = dev_input::ctx();
        let mut painter = self.painter.lock().unwrap_or_else(|e| e.into_inner());
        let painter = match painter.as_mut() {
            Some(p) if p.format == world.color_format => p,
            _ => painter.insert(Painter {
                renderer: egui_wgpu::Renderer::new(
                    world.device,
                    world.color_format,
                    egui_wgpu::RendererOptions::default(),
                ),
                format: world.color_format,
            }),
        };
        // The window thread's input for this frame, or a bare one for a
        // frame drawn before it came (the sizes agree; only events are lost).
        let input = dev_input::take_input().unwrap_or_else(|| {
            let ppp = ctx.pixels_per_point();
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(w as f32 / ppp, h as f32 / ppp),
                )),
                ..Default::default()
            }
        });
        let output = ctx.run_ui(input, |ui| self.ui(ui.ctx()));
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: output.pixels_per_point,
        };
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                painter
                    .renderer
                    .update_texture(world.device, world.queue, *id, delta);
            }
        }
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        painter
            .renderer
            .update_buffers(world.device, world.queue, world.encoder, &jobs, &screen);
        {
            let pass = world
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("dev tools (egui)"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: world.color_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            let mut pass = pass.forget_lifetime();
            painter.renderer.render(&mut pass, &jobs, &screen);
        }
        for id in &output.textures_delta.free {
            painter.renderer.free_texture(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client_core::nbt_path::NbtPath;
    use horizon_world::nbt::{Compound, JavaString};

    /// Every number of `tag`, with the path the tree gives it.
    fn numbers(path: &str, tag: &Tag, out: &mut Vec<(String, Tag)>) {
        match tag {
            Tag::Compound(c) => {
                for (k, v) in c.iter() {
                    numbers(&child_path(path, &k.to_string_lossy()), v, out);
                }
            }
            Tag::List(items) => {
                for (i, v) in items.iter().enumerate() {
                    numbers(&format!("{path}[{i}]"), v, out);
                }
            }
            leaf if watchable(leaf) => out.push((path.to_string(), leaf.clone())),
            _ => {}
        }
    }

    #[test]
    fn a_watch_button_charts_the_path_watch_would_read() {
        let mut inner = Compound::default();
        inner.insert(JavaString::from("a.b"), Tag::Int(3));
        inner.insert(JavaString::from("say \"hi\""), Tag::Short(4));
        inner.insert(JavaString::from("back\\slash"), Tag::Long(5));
        let mut root = Compound::default();
        root.insert(JavaString::from("Health"), Tag::Float(20f32.to_bits()));
        root.insert(
            JavaString::from("Motion"),
            Tag::List(vec![
                Tag::Double(0f64.to_bits()),
                Tag::Double((-0.5f64).to_bits()),
            ]),
        );
        root.insert(
            JavaString::from("horizon:attachments"),
            Tag::Compound(inner),
        );
        root.insert(JavaString::from("with space"), Tag::Byte(1));
        root.insert(JavaString::from("CustomName"), Tag::String("x".into()));
        let root = Tag::Compound(root);
        let mut found = Vec::new();
        numbers("", &root, &mut found);
        assert_eq!(found.len(), 7, "{found:?}");
        for (path, leaf) in found {
            let parsed = NbtPath::parse(&path).unwrap_or_else(|e| panic!("{path}: {e:?}"));
            assert_eq!(parsed.get(&root).unwrap(), vec![leaf], "{path}");
        }
        assert_eq!(child_path("", "Health"), "Health");
        assert_eq!(child_path("a", "a.b"), "a.\"a.b\"");
    }
}
