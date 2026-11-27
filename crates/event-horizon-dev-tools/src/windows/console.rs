//! The Lua console window (5e-c): the output lines, a one-line input with the history on Up and
//! Down, and Run, Clear and Close. The state is [`crate::console`]: what is typed is queued there,
//! and the server tick runs it (`console::tick`), so the window never runs a chunk itself.

use horizon_client_core::lang;

use crate::console::{Action, Kind, State, state};

/// The colour of a chunk's value, `= <value>`: a cyan that reads apart from the output.
const VALUE: egui::Color32 = egui::Color32::from_rgb(120, 220, 230);

/// A line's colour: the echo gray, the output in the window's text colour, a value cyan, an error
/// red.
fn colour(kind: Kind, ui: &egui::Ui) -> egui::Color32 {
    match kind {
        Kind::Echo => egui::Color32::GRAY,
        Kind::Out => ui.visuals().text_color(),
        Kind::Value => VALUE,
        Kind::Err => egui::Color32::LIGHT_RED,
    }
}

/// Draws the console window while it is open: its output, its input, and its three buttons.
pub fn window(ctx: &egui::Context) {
    let mut guard = state();
    let st: &mut State = &mut guard;
    if !st.open {
        return;
    }
    let mut close = false;
    egui::Window::new(lang::get("horizon.dev_tools.console.title"))
        .id(egui::Id::unique("dev tools console"))
        .default_width(520.0)
        .default_height(360.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.weak(lang::get("horizon.dev_tools.console.hint"));
            let reserve = 2.0 * ui.spacing().interact_size.y + 16.0;
            let height = (ui.available_height() - reserve).max(60.0);
            egui::ScrollArea::vertical()
                .max_height(height)
                .min_scrolled_height(height)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for (kind, line) in &st.lines {
                        let text = egui::RichText::new(line.as_str())
                            .monospace()
                            .color(colour(*kind, ui));
                        ui.label(text);
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                // The keys are read before the field runs: a singleline field consumes Up, Down and
                // Enter itself, so a check after it would see nothing.
                let id = egui::Id::unique("dev tools console input");
                let focused = ui.memory(|m| m.has_focus(id));
                let (up, down, enter_key) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                        i.key_pressed(egui::Key::Enter),
                    )
                });
                let width = (ui.available_width() - 190.0).max(120.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut st.input)
                        .id(id)
                        .font(egui::TextStyle::Monospace)
                        .hint_text(lang::get("horizon.dev_tools.console.input_hint"))
                        .desired_width(width),
                );
                if st.focus {
                    edit.request_focus();
                    st.focus = false;
                }
                if focused {
                    if up {
                        st.history_up();
                    }
                    if down {
                        st.history_down();
                    }
                }
                // A singleline field gives up its focus on Enter: that is the submit.
                if edit.lost_focus() && enter_key {
                    st.submit();
                    edit.request_focus();
                }
                if ui
                    .button(lang::get("horizon.dev_tools.console.run"))
                    .clicked()
                {
                    st.submit();
                    edit.request_focus();
                }
                if ui
                    .button(lang::get("horizon.dev_tools.console.clear"))
                    .clicked()
                {
                    st.act(Action::Clear);
                }
                if ui
                    .button(lang::get("horizon.dev_tools.console.close"))
                    .clicked()
                {
                    close = true;
                }
            });
        });
    if close {
        st.act(Action::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of the window on a screen of 800 by 600 points. The deltas are dropped unapplied,
    /// as the other windows' smoke frame does (no texture is uploaded here).
    fn frame() -> Vec<egui::epaint::ClippedShape> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let mut output = egui::Context::default().run_ui(raw, |screen| window(screen.ctx()));
        output.textures_delta.clear();
        output.shapes
    }

    #[test]
    fn a_closed_console_draws_nothing() {
        let _console = crate::console::test_guard();
        *state() = State::default();
        assert!(frame().is_empty());
    }

    #[test]
    fn an_open_console_with_a_few_lines_draws_its_window() {
        let _console = crate::console::test_guard();
        *state() = State::default();
        {
            let mut st = state();
            st.act(Action::Open);
            st.input = "print(1+1)".to_owned();
            st.submit();
            st.push(Kind::Out, "2");
            st.push(Kind::Value, "= 2");
            st.push(Kind::Err, "boom");
        }
        let shapes = frame();
        *state() = State::default();
        assert!(!shapes.is_empty());
    }
}
