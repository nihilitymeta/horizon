//! The block windows (`/blockinfo`, docs/dev-tools.md): one per watched block, drawn from the
//! snapshot the server's tick read ([`crate::inspect`]). The block's name and state, its block
//! entity's data as a tree, its light, biome and power, the six strong signals, and its collision
//! and outline boxes. The four facts this base does not read (random ticks, pending ticks, tags and
//! the declaring mod) are said to be unavailable, never shown as "no" or "none".

use event_horizon_observe::{self as observe, BlockSnapshot, InspectedBlock};
use horizon_client_core::lang;

use super::{block_label, pos_label, stale_note, tag_node};

/// The face names of `Direction::ALL`, in its order: the strong signals are listed in it.
const FACES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// Draws the window of one watched block. Its stop button closes it.
pub(super) fn window(ctx: &egui::Context, block: &InspectedBlock) {
    let key = &block.key;
    let window_id = egui::Id::unique(("dev tools block", &key.dimension, key.pos));
    egui::Window::new(lang::tr(
        "horizon.dev_tools.block.title",
        &[&block_label(key.pos)],
    ))
    .id(window_id)
    .default_width(420.0)
    .default_height(520.0)
    .vscroll(true)
    .show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button(lang::get("horizon.dev_tools.stop")).clicked() {
                observe::remove_block(key);
            }
        });
        let Some(b) = &block.snapshot else {
            ui.weak(lang::get("horizon.dev_tools.block.waiting"));
            return;
        };
        stale_note(ui, b.taken, &lang::get("horizon.dev_tools.stale.update"));
        facts(ui, block, b);
        ui.separator();
        boxes(ui, b);
        ui.separator();
        match &b.entity {
            None => {
                ui.weak(lang::get("horizon.dev_tools.block.no_entity"));
            }
            Some(data) => tag_node(
                ui,
                None,
                &lang::get("horizon.dev_tools.block.entity_data"),
                "",
                data,
                0,
                "",
            ),
        }
    });
}

/// The name, the state and the readings of the block, as a two-column grid, then the line that
/// says what this base does not read.
fn facts(ui: &mut egui::Ui, block: &InspectedBlock, b: &BlockSnapshot) {
    ui.strong(&b.name);
    egui::Grid::new(("dev tools block facts", &block.key.dimension, block.key.pos))
        .num_columns(2)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            for (k, v) in &b.properties {
                ui.weak(k);
                ui.monospace(v);
                ui.end_row();
            }
            row(
                ui,
                &lang::get("horizon.dev_tools.block.dimension"),
                &block.key.dimension,
            );
            row(
                ui,
                &lang::get("horizon.dev_tools.block.biome"),
                b.biome
                    .as_deref()
                    .unwrap_or(&lang::get("horizon.dev_tools.block.unknown")),
            );
            let face = block_label(offset(block.key.pos, block.face));
            row(
                ui,
                &lang::get("horizon.dev_tools.block.light"),
                &lang::tr(
                    "horizon.dev_tools.block.light_value",
                    &[
                        &b.light[0],
                        &b.light[1],
                        &face,
                        &b.face_light[0],
                        &b.face_light[1],
                    ],
                ),
            );
            ui.weak(lang::get("horizon.dev_tools.block.power"));
            ui.label(b.power.to_string())
                .on_hover_text(lang::get("horizon.dev_tools.block.power_hover"));
            ui.end_row();
            ui.weak(lang::get("horizon.dev_tools.block.strong"));
            ui.label(strong_text(&b.strong))
                .on_hover_text(lang::get("horizon.dev_tools.block.strong_hover"));
            ui.end_row();
        });
    ui.weak(lang::get("horizon.dev_tools.block.not_read"));
}

/// A grid row of a weak label and its value.
fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.weak(label);
    ui.label(value);
    ui.end_row();
}

/// The six strong signals, each with its face's name, in `Direction::ALL` order.
fn strong_text(strong: &[i32; 6]) -> String {
    FACES
        .iter()
        .zip(strong)
        .map(|(face, power)| format!("{} {power}", lang::get(&face_key(face))))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The language key of a face's name.
fn face_key(face: &str) -> String {
    format!("horizon.dev_tools.block.face.{face}")
}

/// `at` moved by `by`, the face's block.
fn offset(at: [i32; 3], by: [i32; 3]) -> [i32; 3] {
    [at[0] + by[0], at[1] + by[1], at[2] + by[2]]
}

/// The collision and the outline boxes, each a collapsible list of its boxes, min to max.
fn boxes(ui: &mut egui::Ui, b: &BlockSnapshot) {
    ui.label(lang::tr(
        "horizon.dev_tools.block.shapes",
        &[&b.collision.len(), &b.outline.len()],
    ));
    for (id, name, shape) in [
        (
            "collision",
            "horizon.dev_tools.block.collision",
            &b.collision,
        ),
        ("outline", "horizon.dev_tools.block.outline", &b.outline),
    ] {
        egui::CollapsingHeader::new(format!("{}  [{}]", lang::get(name), shape.len()))
            .id_salt(id)
            .show(ui, |ui| {
                for bounds in shape {
                    ui.monospace(lang::tr(
                        "horizon.dev_tools.block.box",
                        &[
                            &pos_label([bounds[0], bounds[1], bounds[2]]),
                            &pos_label([bounds[3], bounds[4], bounds[5]]),
                        ],
                    ));
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::frame;
    use super::*;
    use event_horizon_observe::BlockKey;
    use horizon_core::geometry::Direction;
    use horizon_core::nbt::{Compound, JavaString, Tag};

    fn key() -> BlockKey {
        BlockKey {
            dimension: "minecraft:overworld".to_owned(),
            pos: [4, -64, 9],
        }
    }

    /// A repeater's snapshot, with a block entity and one collision box.
    fn snapshot() -> BlockSnapshot {
        let mut chest = Compound::default();
        chest.insert(
            JavaString::from("id"),
            Tag::String("minecraft:chest".into()),
        );
        BlockSnapshot {
            name: "minecraft:repeater".to_owned(),
            properties: vec![("delay".to_owned(), "1".to_owned())],
            entity: Some(Tag::Compound(chest)),
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
            outline: vec![[0.0, 0.0, 0.0, 1.0, 0.125, 1.0]],
            taken: std::time::Instant::now(),
        }
    }

    #[test]
    fn the_six_signals_are_labelled_in_the_order_of_direction_all() {
        let names = Direction::ALL.map(|d| format!("{d:?}").to_lowercase());
        assert_eq!(names, FACES.map(str::to_owned));
        // The face names are translated: read them in the bundled English.
        let (text, _) = lang::scoped(lang::Language::load(&[], "en_us"), || {
            strong_text(&[0, 0, 0, 0, 15, 0])
        });
        assert_eq!(
            text, "down 0, up 0, north 0, south 0, west 15, east 0",
            "the power of each face is under its name, in Direction::ALL's order"
        );
    }

    #[test]
    fn a_block_window_with_a_snapshot_paints() {
        let _store = crate::inspect::store_guard();
        observe::reset();
        observe::add_block(key(), [0, 1, 0]);
        observe::set_block(&key(), snapshot());
        let shapes = frame();
        observe::reset();
        assert!(!shapes.is_empty());
    }

    #[test]
    fn a_block_window_with_no_snapshot_yet_says_it_waits() {
        let _store = crate::inspect::store_guard();
        observe::reset();
        observe::add_block(key(), [0, 1, 0]);
        let shapes = frame();
        observe::reset();
        assert!(!shapes.is_empty(), "the window opens with its waiting line");
    }
}
