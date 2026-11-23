//! The windows' half of the developer tools (docs/dev-tools.md): the egui windows that draw what the
//! store holds. Nothing is drawn until something is inspected: [`ui`] draws one window per view of
//! each watched entity, and the `/watch` charts of each. This file holds the data view (`/inspect`):
//! the NBT tree with its filter and its per-number watch buttons, and the stop button, plus the
//! helpers the mob windows share. The goals, senses, nav and watch windows are in [`mob`]; the Lua
//! console window is in [`console`]. The block windows (5c-w3) come later; their place is marked
//! in [`ui`].

mod console;
mod mob;

use event_horizon_observe::wire::INTERVALS;
use event_horizon_observe::{self as observe, BrainSnapshot, Inspected, View};
use horizon_client_core::lang;
use horizon_client_core::snbt::java_hash_order;
use horizon_core::nbt::{Compound, Tag};
use horizon_world::snbt_text::to_snbt;

use mob::{goals_window, nav_window, senses_window, watch_window};

/// Draws the windows of everything watched: one per view of each entity, and one chart per watched
/// path.
pub fn ui(ctx: &egui::Context) {
    // 5c-w3: the watched blocks' windows (`observe::blocks()`, `block_window`) come with the block readout.
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
    console::window(ctx);
}

/// The flags of a goal row, as the goals window names them: `-` for none.
fn flags_label(flags: u8) -> String {
    let mut names = Vec::new();
    for (bit, key) in [
        (1, "horizon.dev_tools.flag.move"),
        (2, "horizon.dev_tools.flag.look"),
        (4, "horizon.dev_tools.flag.jump"),
        (8, "horizon.dev_tools.flag.target"),
    ] {
        if flags & bit != 0 {
            names.push(lang::get(key));
        }
    }
    if names.is_empty() {
        "-".to_string()
    } else {
        names.join(" ")
    }
}

/// A position, two decimals a number.
fn pos_label(p: [f64; 3]) -> String {
    format!("{:.2} {:.2} {:.2}", p[0], p[1], p[2])
}

/// A block position.
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
                    lang::get("horizon.dev_tools.whole_entity")
                } else {
                    path.to_owned()
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
                        .small_button(lang::get("horizon.dev_tools.chart"))
                        .on_hover_text(lang::tr("horizon.dev_tools.chart_hover", &[&path]))
                        .clicked()
                {
                    observe::add_path(&[id], path);
                }
            });
        }
    }
}

fn compound_matches(c: &Compound, filter: &str) -> bool {
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
        (None, None) => format!(
            "{what}: {} #{}",
            lang::get("horizon.dev_tools.entity"),
            item.id
        ),
    }
}

/// A yellow line when the snapshot stopped updating: half a second late,
/// past the interval a window the server shows comes at.
fn stale_note(ui: &mut egui::Ui, taken: std::time::Instant, what: &str) {
    let stale = taken.elapsed();
    let every = observe::remote().map_or(0, |(_, ms)| u128::from(ms));
    if stale.as_millis() > every + 500 {
        let secs = format!("{:.1}", stale.as_secs_f32());
        ui.colored_label(
            egui::Color32::YELLOW,
            lang::tr("horizon.dev_tools.stale", &[&what, &secs]),
        );
    }
}

/// The interval a window the server shows (a guest's) comes at, as a
/// choice: the server keeps it, down to its floor.
fn interval_choice(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug) {
    let Some((_, mut ms)) = observe::remote() else {
        return;
    };
    let label = |ms: u32| lang::tr("horizon.dev_tools.every", &[&(f64::from(ms) / 1000.0)]);
    let before = ms;
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(ms))
        .show_ui(ui, |ui| {
            for choice in INTERVALS {
                ui.selectable_value(&mut ms, choice, label(choice));
            }
        })
        .response
        .on_hover_text(lang::get("horizon.dev_tools.interval_hover"));
    if ms != before {
        observe::set_remote_interval(ms);
    }
}

/// An entity window's stop button: a window the server shows ends there too.
fn stop_entity(ui: &mut egui::Ui, id: u32, view: View) {
    let remote = observe::is_remote_entity(id);
    if ui.button(lang::get("horizon.dev_tools.stop")).clicked() {
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
            ui.weak(lang::get("horizon.dev_tools.no_ai_step"));
            None
        }
        Some(b) => {
            stale_note(ui, b.taken, &lang::get("horizon.dev_tools.stale.ai_tick"));
            if b.no_ai {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    lang::get("horizon.dev_tools.no_ai"),
                );
            }
            if !b.alive {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    lang::get("horizon.dev_tools.dead"),
                );
            }
            Some(b)
        }
    }
}

/// The data view of one entity: its saved data as a tree, filtered by the window's filter box,
/// with the SNBT text under it. `/inspect <targets>` opens it.
fn inspect_window(ctx: &egui::Context, item: &Inspected) {
    let window_id = egui::Id::unique(("dev tools inspect", item.id));
    let title = entity_title(item, &lang::get("horizon.dev_tools.data"));
    egui::Window::new(title)
        .id(window_id)
        .default_width(420.0)
        .default_height(520.0)
        .vscroll(true)
        .show(ctx, |ui| {
            let filter_id = window_id.with("filter");
            let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
            ui.horizontal(|ui| {
                ui.label(lang::get("horizon.dev_tools.filter"));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut filter)
                        .hint_text(lang::get("horizon.dev_tools.filter_hint"))
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
                    ui.weak(lang::get("horizon.dev_tools.no_data"));
                }
                Some(d) => {
                    stale_note(ui, d.taken, &lang::get("horizon.dev_tools.stale.update"));
                    ui.separator();
                    tag_node(
                        ui,
                        watch.then_some(item.id),
                        &lang::get("horizon.dev_tools.entity_data"),
                        "",
                        &d.data,
                        0,
                        &filter,
                    );
                    ui.separator();
                    egui::CollapsingHeader::new(lang::get("horizon.dev_tools.snbt"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client_core::nbt_path::NbtPath;
    use horizon_core::nbt::JavaString;

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

    /// A small entity: a root with a number, a list, a nested compound and a list of compounds.
    fn sample() -> Compound {
        let mut inner = Compound::default();
        inner.insert(JavaString::from("Health"), Tag::Float(20f32.to_bits()));
        let mut root = Compound::default();
        root.insert(
            JavaString::from("Pos"),
            Tag::List(vec![Tag::Double(1f64.to_bits())]),
        );
        root.insert(JavaString::from("Attributes"), Tag::Compound(inner));
        root.insert(
            JavaString::from("Items"),
            Tag::List(vec![Tag::Compound(Compound::default())]),
        );
        root
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

    #[test]
    fn the_filter_matches_a_key_whatever_its_case() {
        assert!(key_matches("Health", ""));
        assert!(key_matches("Health", "heal"));
        assert!(!key_matches("Pos", "health"));
    }

    #[test]
    fn the_filter_finds_a_key_inside_a_compound_or_a_list() {
        let root = sample();
        // "health" is only under Attributes, one compound down.
        assert!(compound_matches(&root, "health"));
        assert!(!compound_matches(&root, "armor"));
        // A key inside a compound that sits in a list.
        let mut item = Compound::default();
        item.insert(JavaString::from("Slot"), Tag::Byte(0));
        let items = vec![Tag::Compound(item)];
        assert!(list_matches(&items, "slot"));
        assert!(!list_matches(&items, "health"));
    }

    #[test]
    fn a_position_and_its_flags_read_as_the_goal_windows_show_them() {
        assert_eq!(pos_label([1.0, -2.5, 3.75]), "1.00 -2.50 3.75");
        assert_eq!(block_label([4, -64, 9]), "4 -64 9");
        // The flag names are translated: read them in the bundled English, not the raw keys.
        let (names, _) = lang::scoped(lang::Language::load(&[], "en_us"), || {
            (
                flags_label(0),
                flags_label(1 | 4),
                flags_label(1 | 2 | 4 | 8),
            )
        });
        assert_eq!(
            names,
            (
                "-".to_owned(),
                "move jump".to_owned(),
                "move look jump target".to_owned()
            )
        );
    }

    #[test]
    fn the_title_names_the_entity_by_its_data_name_and_id() {
        let item = Inspected {
            id: 5,
            views: vec![View::Data],
            data: Some(event_horizon_observe::DataSnapshot {
                id: 5,
                name: "Cow".to_owned(),
                data: Tag::Compound(Compound::default()),
                taken: std::time::Instant::now(),
            }),
            brain: None,
            history: Vec::new(),
            series: Vec::new(),
            log_file: None,
        };
        assert_eq!(entity_title(&item, "data"), "data: Cow #5");
    }

    /// One frame of `ui`, on a screen of 800 by 600 points, with the store as the test left it.
    pub(super) fn frame() -> Vec<egui::epaint::ClippedShape> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let mut output = egui::Context::default().run_ui(raw, |screen| ui(screen.ctx()));
        // No texture is uploaded here, so the deltas are dropped unapplied (epaint panics otherwise).
        output.textures_delta.clear();
        output.shapes
    }

    #[test]
    fn nothing_is_drawn_until_something_is_inspected() {
        let _store = crate::inspect::store_guard();
        let _console = crate::console::test_guard();
        observe::reset();
        assert!(frame().is_empty());
    }

    #[test]
    fn an_inspected_entity_paints_its_data_window() {
        let _store = crate::inspect::store_guard();
        observe::reset();
        observe::add(&[7], View::Data);
        observe::set_data(7, "Cow".to_owned(), Tag::Compound(sample()));
        let shapes = frame();
        observe::reset();
        assert!(!shapes.is_empty());
    }
}
