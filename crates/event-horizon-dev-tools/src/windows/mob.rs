//! The mob windows of the developer tools (docs/dev-tools.md): the goals, senses and nav windows of
//! one entity (`/goals`, `/senses`, `/nav`), and the chart window of one watched path (`/watch`).
//! They draw what the store holds: the AI tap fills the brain snapshot and the goal history, the
//! server tick fills the samples. The helpers they share with the data view are in the parent.

use std::collections::VecDeque;

use event_horizon_observe::{
    self as observe, BrainSnapshot, GoalRow, HistoryTick, Inspected, Request, Series, View,
};
use horizon_client_core::lang;

use super::{block_label, entity_title, flags_label, mob_header, pos_label, stale_note};

/// A table of goal rows: priority, name, running or idle, flags.
fn goal_table(ui: &mut egui::Ui, id: &str, rows: &[GoalRow]) {
    if rows.is_empty() {
        ui.weak(lang::get("horizon.dev_tools.none"));
        return;
    }
    egui::Grid::new(id)
        .num_columns(4)
        .striped(true)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            ui.weak(lang::get("horizon.dev_tools.prio"));
            ui.weak(lang::get("horizon.dev_tools.goal"));
            ui.weak(lang::get("horizon.dev_tools.state"));
            ui.weak(lang::get("horizon.dev_tools.flags"));
            ui.end_row();
            for row in rows {
                ui.label(row.priority.to_string());
                if row.running {
                    ui.strong(row.name);
                    ui.colored_label(
                        egui::Color32::LIGHT_GREEN,
                        lang::get("horizon.dev_tools.running"),
                    );
                } else {
                    ui.label(row.name);
                    ui.weak(lang::get("horizon.dev_tools.idle"));
                }
                ui.label(flags_label(row.flags));
                ui.end_row();
            }
        });
}

/// The runs of history ticks in which goal `name` ran, as `(first, end)` index pairs (`end` is
/// exclusive): the bars of the timeline.
fn goal_runs(history: &[HistoryTick], name: &'static str) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = None;
    for i in 0..=history.len() {
        let on = i < history.len() && history[i].1.contains(&name);
        match (start, on) {
            (None, true) => start = Some(i),
            (Some(first), false) => {
                runs.push((first, i));
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// The goals' history as a strip per goal: a bar where it ran, over the last ticks kept
/// (`HISTORY_TICKS`). Target goals first, as the selectors tick.
fn goal_timeline(ui: &mut egui::Ui, brain: &BrainSnapshot, history: &[HistoryTick]) {
    let names: Vec<&'static str> = brain
        .targets
        .iter()
        .chain(brain.goals.iter())
        .map(|g| g.name)
        .collect();
    if names.is_empty() || history.is_empty() {
        ui.weak(lang::get("horizon.dev_tools.no_history"));
        return;
    }
    let (first, last) = (history[0].0, history[history.len() - 1].0);
    ui.weak(lang::tr(
        "horizon.dev_tools.history_summary",
        &[&history.len(), &first, &last],
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
        for (start, end) in goal_runs(history, name) {
            let x0 = strip_x + start as f32 * step;
            let x1 = strip_x + end as f32 * step;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x0, y + 2.0),
                    egui::pos2(x1.max(x0 + 1.0), y + row_h - 2.0),
                ),
                1.0,
                color,
            );
        }
    }
    if let Some(pos) = response.hover_pos()
        && pos.x >= strip_x
    {
        let i = (((pos.x - strip_x) / step) as usize).min(history.len() - 1);
        let (age, running) = &history[i];
        let what = if running.is_empty() {
            lang::get("horizon.dev_tools.nothing_running")
        } else {
            running.join(", ")
        };
        response.on_hover_text(lang::tr("horizon.dev_tools.age_running", &[&age, &what]));
    }
}

/// The goals window's log button, `/mobai log` for this mob: the game opens the file at its next
/// tick, and the line names it while it is written.
fn log_line(ui: &mut egui::Ui, item: &Inspected) {
    ui.horizontal(|ui| match &item.log_file {
        Some(file) => {
            if ui.button(lang::get("horizon.dev_tools.stop_log")).clicked() {
                observe::request(Request::StopLog(item.id));
            }
            ui.colored_label(
                egui::Color32::LIGHT_GREEN,
                lang::tr("horizon.dev_tools.log_to", &[&file]),
            );
        }
        None => {
            if ui
                .button(lang::get("horizon.dev_tools.log"))
                .on_hover_text(lang::get("horizon.dev_tools.log_hover"))
                .clicked()
            {
                observe::request(Request::StartLog(item.id));
            }
        }
    });
}

/// A label and its value, as a row of a two-column grid.
fn field(ui: &mut egui::Ui, key: &str, value: impl Into<egui::WidgetText>) {
    ui.weak(lang::get(key));
    ui.label(value);
    ui.end_row();
}

/// The goals of one entity: its goals and target goals as tables, and their history as a timeline.
pub(super) fn goals_window(ctx: &egui::Context, item: &Inspected) {
    egui::Window::new(entity_title(
        item,
        &lang::get("horizon.dev_tools.win.goals"),
    ))
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
        ui.strong(lang::get("horizon.dev_tools.goals"));
        goal_table(ui, "goals", &brain.goals);
        ui.add_space(4.0);
        ui.strong(lang::get("horizon.dev_tools.target_goals"));
        goal_table(ui, "targets", &brain.targets);
        ui.separator();
        ui.strong(lang::get("horizon.dev_tools.history"));
        goal_timeline(ui, brain, &item.history);
    });
}

/// The senses of one entity: its target, what hurt it, its range and its nearest player, and the
/// line-of-sight checks of the tick.
pub(super) fn senses_window(ctx: &egui::Context, item: &Inspected) {
    egui::Window::new(entity_title(
        item,
        &lang::get("horizon.dev_tools.win.senses"),
    ))
    .id(egui::Id::unique(("dev tools senses", item.id)))
    .default_width(380.0)
    .vscroll(true)
    .show(ctx, |ui| {
        let Some(b) = mob_header(ui, item, View::Senses) else {
            return;
        };
        let none = lang::get("horizon.dev_tools.none");
        egui::Grid::new("senses")
            .num_columns(2)
            .spacing([12.0, 2.0])
            .show(ui, |ui| {
                field(
                    ui,
                    "horizon.dev_tools.row.target",
                    b.target.as_deref().unwrap_or(&none),
                );
                let hurt = match &b.last_hurt_by {
                    Some(who) => {
                        lang::tr("horizon.dev_tools.hurt_at", &[&who, &b.last_hurt_by_time])
                    }
                    None => lang::get("horizon.dev_tools.nobody"),
                };
                field(ui, "horizon.dev_tools.row.last_hurt_by", hurt);
                field(
                    ui,
                    "horizon.dev_tools.row.follow_range",
                    lang::tr(
                        "horizon.dev_tools.blocks",
                        &[&format!("{:.1}", b.follow_range)],
                    ),
                );
                let nearest = match b.nearest_player {
                    Some((p, d)) => {
                        let in_range = if d <= b.follow_range {
                            lang::get("horizon.dev_tools.in_range")
                        } else {
                            String::new()
                        };
                        lang::tr(
                            "horizon.dev_tools.nearest_away",
                            &[&format!("{d:.1}"), &pos_label(p), &in_range],
                        )
                    }
                    None => lang::get("horizon.dev_tools.no_player_in_level"),
                };
                field(ui, "horizon.dev_tools.row.nearest_player", nearest);
                let mut mood = String::new();
                if b.aggressive {
                    mood += &lang::get("horizon.dev_tools.aggressive");
                }
                if b.panicking {
                    mood += &lang::get("horizon.dev_tools.panicking");
                }
                field(
                    ui,
                    "horizon.dev_tools.row.mood",
                    lang::tr("horizon.dev_tools.idle_for", &[&mood, &b.no_action_time]),
                );
                let home = match b.home {
                    Some((c, r)) => {
                        lang::tr("horizon.dev_tools.home_within", &[&block_label(c), &r])
                    }
                    None => none.clone(),
                };
                field(ui, "horizon.dev_tools.row.home", home);
                field(
                    ui,
                    "horizon.dev_tools.row.health",
                    format!("{:.1} / {:.1}", b.health, b.max_health),
                );
            });
        ui.separator();
        ui.strong(lang::get("horizon.dev_tools.sight"));
        if b.seen.is_empty() {
            ui.weak(lang::get("horizon.dev_tools.no_sight_checks"));
        } else {
            for (who, seen) in &b.seen {
                ui.horizontal(|ui| {
                    if *seen {
                        ui.colored_label(
                            egui::Color32::LIGHT_GREEN,
                            lang::get("horizon.dev_tools.seen"),
                        );
                    } else {
                        ui.weak(lang::get("horizon.dev_tools.hidden"));
                    }
                    ui.label(who);
                });
            }
        }
    });
}

/// The nav of one entity: its navigation and move control, and the path it walks, node by node.
pub(super) fn nav_window(ctx: &egui::Context, item: &Inspected) {
    egui::Window::new(entity_title(item, &lang::get("horizon.dev_tools.win.nav")))
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
                    field(ui, "horizon.dev_tools.row.kind", b.nav_kind.as_str());
                    field(ui, "horizon.dev_tools.row.position", pos_label(b.position));
                    let going = match b.nav_target {
                        Some(t) => {
                            let mut s = lang::tr(
                                "horizon.dev_tools.at_speed",
                                &[&block_label(t), &format!("{:.2}", b.nav_speed)],
                            );
                            if b.nav_stuck {
                                s += &lang::get("horizon.dev_tools.stuck");
                            }
                            s
                        }
                        None => lang::get("horizon.dev_tools.nowhere"),
                    };
                    field(ui, "horizon.dev_tools.row.going_to", going);
                    field(
                        ui,
                        "horizon.dev_tools.row.move_control",
                        lang::tr(
                            "horizon.dev_tools.towards",
                            &[&b.move_operation, &pos_label(b.move_wanted)],
                        ),
                    );
                    field(
                        ui,
                        "horizon.dev_tools.row.input",
                        lang::tr(
                            "horizon.dev_tools.input_values",
                            &[
                                &format!("{:.2}", b.input[0]),
                                &format!("{:.2}", b.input[1]),
                                &format!("{:.2}", b.input[2]),
                                &format!("{:.3}", b.speed),
                            ],
                        ),
                    );
                });
            ui.separator();
            match &b.path {
                None => {
                    ui.weak(lang::get("horizon.dev_tools.no_path"));
                }
                Some(p) => {
                    let mut head = lang::tr(
                        "horizon.dev_tools.path_node",
                        &[&p.next, &p.nodes.len(), &block_label(p.target)],
                    );
                    if p.reached {
                        head += &lang::get("horizon.dev_tools.reached");
                    }
                    ui.strong(head);
                    ui.weak(lang::get("horizon.dev_tools.path_drawn"));
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

/// The points of a chart's line in `rect`: time runs along x from the first sample to the last,
/// the value up from the lowest to the highest. A flat series sits on the bottom edge.
fn chart_points(samples: &VecDeque<(u64, f64)>, rect: egui::Rect) -> Vec<egui::Pos2> {
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(_, v) in samples {
        min = min.min(v);
        max = max.max(v);
    }
    let span = if (max - min).abs() < f64::EPSILON {
        1.0
    } else {
        max - min
    };
    let (Some(&(first, _)), Some(&(last, _))) = (samples.front(), samples.back()) else {
        return Vec::new();
    };
    let ticks = (last - first).max(1) as f64;
    samples
        .iter()
        .map(|&(t, v)| {
            let x = rect.left() + ((t - first) as f64 / ticks) as f32 * rect.width();
            let y = rect.bottom() - ((v - min) / span) as f32 * rect.height();
            egui::pos2(x, y)
        })
        .collect()
}

/// A line chart of the samples, with the range on the left.
fn series_chart(ui: &mut egui::Ui, series: &Series) {
    let samples = &series.samples;
    let n = samples.len();
    if n < 2 {
        ui.weak(lang::get("horizon.dev_tools.waiting_samples"));
        return;
    }
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(_, v) in samples {
        min = min.min(v);
        max = max.max(v);
    }
    let last = samples[n - 1].1;
    ui.label(lang::tr(
        "horizon.dev_tools.samples_summary",
        &[&last, &min, &max, &n],
    ));
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width().max(200.0), 140.0),
        egui::Sense::hover(),
    );
    let rect = response.rect.shrink(4.0);
    painter.rect_filled(response.rect, 2.0, egui::Color32::from_white_alpha(6));
    painter.add(egui::Shape::line(
        chart_points(samples, rect),
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
    let (first_tick, last_tick) = (samples[0].0, samples[n - 1].0);
    painter.text(
        rect.right_bottom(),
        egui::Align2::RIGHT_BOTTOM,
        lang::tr("horizon.dev_tools.ticks", &[&(last_tick - first_tick)]),
        font,
        color,
    );
    if let Some(pos) = response.hover_pos() {
        let i = (((pos.x - rect.left()) / rect.width()) * (n - 1) as f32)
            .round()
            .clamp(0.0, (n - 1) as f32) as usize;
        let (t, v) = samples[i];
        response.on_hover_text(lang::tr("horizon.dev_tools.sample_at", &[&t, &v]));
    }
}

/// The chart window of one watched path of an entity, as `/watch` opens it.
pub(super) fn watch_window(ctx: &egui::Context, item: &Inspected, series: &Series) {
    let title = format!(
        "{} {}",
        entity_title(item, &lang::get("horizon.dev_tools.win.watch")),
        series.path
    );
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
                if ui.button(lang::get("horizon.dev_tools.stop")).clicked() {
                    observe::remove_path(item.id, &series.path);
                }
            });
            if let Some(d) = &item.data {
                stale_note(ui, d.taken, &lang::get("horizon.dev_tools.stale.update"));
            }
            if let Some(e) = &series.error {
                ui.colored_label(egui::Color32::YELLOW, e);
            }
            series_chart(ui, series);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::tests::frame;
    use event_horizon_observe::PathSnapshot;

    /// A mob's snapshot with the given goals and target goals; everything else is a quiet default.
    fn brain(id: u32, goals: Vec<GoalRow>, targets: Vec<GoalRow>) -> BrainSnapshot {
        BrainSnapshot {
            id,
            kind: "minecraft:cow".to_owned(),
            species: "cow".to_owned(),
            custom_name: None,
            game_time: 100,
            position: [1.0, 64.0, 2.0],
            velocity: [0.0; 3],
            health: 10.0,
            max_health: 10.0,
            age: 5,
            alive: true,
            no_ai: false,
            target: None,
            last_hurt_by: None,
            last_hurt_by_time: 0,
            aggressive: false,
            panicking: true,
            no_action_time: 3,
            home: Some(([0, 64, 0], 8)),
            follow_range: 16.0,
            seen: vec![("Steve".to_owned(), true)],
            nearest_player: Some(([4.0, 64.0, 2.0], 3.0)),
            nav_kind: "GroundPathNavigation".to_owned(),
            nav_target: Some([5, 64, 2]),
            nav_speed: 1.0,
            nav_stuck: false,
            path: Some(PathSnapshot {
                nodes: vec![[1, 64, 2], [2, 64, 2], [5, 64, 2]],
                next: 1,
                target: [5, 64, 2],
                reached: false,
            }),
            move_operation: "MOVE_TO".to_owned(),
            move_wanted: [5.5, 64.0, 2.5],
            input: [0.0, 0.0, 1.0],
            speed: 0.1,
            goals,
            targets,
            taken: std::time::Instant::now(),
        }
    }

    fn row(priority: i32, name: &'static str, running: bool, flags: u8) -> GoalRow {
        GoalRow {
            priority,
            name,
            running,
            flags,
        }
    }

    #[test]
    fn a_goal_runs_as_the_bars_of_its_history() {
        let history: Vec<HistoryTick> = vec![
            (1, vec!["Float"]),
            (2, vec!["Float", "Panic"]),
            (3, vec!["Panic"]),
            (4, vec!["Float"]),
        ];
        assert_eq!(goal_runs(&history, "Float"), vec![(0, 2), (3, 4)]);
        assert_eq!(goal_runs(&history, "Panic"), vec![(1, 3)]);
        assert!(goal_runs(&history, "Attack").is_empty());
    }

    #[test]
    fn a_chart_plots_time_along_x_and_the_value_up() {
        let samples: VecDeque<(u64, f64)> = [(10, 0.0), (15, 5.0), (20, 10.0)].into();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 50.0));
        assert_eq!(
            chart_points(&samples, rect),
            vec![
                egui::pos2(0.0, 50.0),
                egui::pos2(50.0, 25.0),
                egui::pos2(100.0, 0.0),
            ]
        );
    }

    #[test]
    fn a_flat_chart_sits_on_its_bottom_edge() {
        let samples: VecDeque<(u64, f64)> = [(0, 3.0), (1, 3.0)].into();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 50.0));
        let points = chart_points(&samples, rect);
        assert!(points.iter().all(|p| p.y == 50.0), "{points:?}");
    }

    #[test]
    fn a_watched_goal_window_paints_its_goals_and_their_history() {
        let _store = crate::inspect::store_guard();
        observe::reset();
        observe::add(&[9], View::Goals);
        let goals = vec![row(1, "Float", true, 1 | 4)];
        let targets = vec![row(0, "NearestAttackableTarget", false, 8)];
        for age in 1..=3 {
            observe::observe_entity(9, age, || brain(9, goals.clone(), targets.clone()));
        }
        assert_eq!(observe::history_of(9).len(), 3);
        let shapes = frame();
        observe::reset();
        assert!(!shapes.is_empty());
    }

    #[test]
    fn a_watched_senses_window_paints() {
        let _store = crate::inspect::store_guard();
        observe::reset();
        observe::add(&[9], View::Senses);
        observe::observe_entity(9, 1, || brain(9, Vec::new(), Vec::new()));
        let shapes = frame();
        observe::reset();
        assert!(!shapes.is_empty());
    }

    #[test]
    fn a_watched_nav_window_paints_its_path() {
        let _store = crate::inspect::store_guard();
        observe::reset();
        observe::add(&[9], View::Nav);
        observe::observe_entity(9, 1, || brain(9, Vec::new(), Vec::new()));
        let shapes = frame();
        observe::reset();
        assert!(!shapes.is_empty());
    }

    #[test]
    fn a_watched_path_paints_its_chart() {
        let _store = crate::inspect::store_guard();
        observe::reset();
        observe::add_path(&[9], "Health");
        observe::set_sample(9, "Health", 1, Ok(20.0));
        observe::set_sample(9, "Health", 2, Ok(18.5));
        let shapes = frame();
        observe::reset();
        assert!(!shapes.is_empty());
    }
}
