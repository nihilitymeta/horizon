//! The Lua console's window (docs/dev-tools.md). What it shows and writes is
//! the console's shared state ([`horizon_client::dev_input::console`]): the game's
//! own code runs what the window queues and answers its completion requests.
use horizon_client::dev_input::console::{
    Kind, OPEN, SCRIPTS_DIR, STOP_TICKS, TICKING, clear, completions_for, history, lines, push,
    remember, request_completions, submit, take_focus,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// The `.lua` files of the scripts folder, sorted; read at most once a second.
fn scripts() -> Vec<PathBuf> {
    static CACHE: Mutex<Option<(Instant, Vec<PathBuf>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((when, list)) = cache.as_ref()
        && when.elapsed().as_secs_f32() < 1.0
    {
        return list.clone();
    }
    let mut list: Vec<PathBuf> = std::fs::read_dir(SCRIPTS_DIR)
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "lua"))
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    *cache = Some((Instant::now(), list.clone()));
    list
}

/// Queue a script file to run.
pub fn run_file(path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(code) => submit(format!("run {}", path.display()), code),
        Err(e) => push(Kind::Err, format!("{}: {e}", path.display())),
    }
}

/// The scripted mods (docs/lua-bindings.md): each with its source, scripts,
/// handlers and goals, its last error in red, and a reload button that
/// reads its scripts again at the next server tick.
fn mods_fold(ui: &mut egui::Ui) {
    let mods = horizon_client::dev_input::mods::status();
    egui::CollapsingHeader::new(format!("Lua mods ({})", mods.len()))
        .id_salt("mods")
        .default_open(mods.iter().any(|m| m.error.is_some()))
        .show(ui, |ui| {
            if mods.is_empty() {
                ui.weak("none: a .hmod with capability scripts that is on, or --dev-mod <folder>");
                return;
            }
            for m in &mods {
                ui.horizontal(|ui| {
                    if m.scripts > 0 && ui.small_button("reload").clicked() {
                        horizon_client::dev_input::mods::request_reload(Some(&m.id));
                    }
                    ui.strong(&m.id);
                    ui.weak(&m.source);
                });
                if m.scripts > 0 {
                    let handlers: Vec<String> = m
                        .handlers
                        .iter()
                        .map(|(name, n)| if *n == 1 { name.clone() } else { format!("{name} ×{n}") })
                        .collect();
                    ui.weak(format!(
                        "{} script{}, reloaded {} time{}; handlers: {}; goals: {}",
                        m.scripts,
                        if m.scripts == 1 { "" } else { "s" },
                        m.reloads,
                        if m.reloads == 1 { "" } else { "s" },
                        if handlers.is_empty() { "none".to_string() } else { handlers.join(", ") },
                        if m.goals.is_empty() { "none".to_string() } else { m.goals.join(", ") },
                    ));
                }
                if let Some(e) = &m.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                }
            }
            if mods.len() > 1 && ui.small_button("reload all").clicked() {
                horizon_client::dev_input::mods::request_reload(None);
            }
            ui.weak("a reload runs at the next server tick, as /reload does; the mods' log and print lines show below");
        });
    ui.separator();
}

/// The console window, when open.
pub fn window(ctx: &egui::Context) {
    if !OPEN.load(Ordering::Relaxed) {
        return;
    }
    let mut open = true;
    let window_id = egui::Id::unique("dev tools lua console");
    egui::Window::new("Lua console")
        .id(window_id)
        .open(&mut open)
        .default_width(560.0)
        .default_height(480.0)
        .show(ctx, |ui| {
            mods_fold(ui);
            // Scripts: the folder's files, each with a run button.
            egui::CollapsingHeader::new(format!("scripts in {SCRIPTS_DIR}/"))
                .id_salt("scripts")
                .default_open(true)
                .show(ui, |ui| {
                    let files = scripts();
                    if files.is_empty() {
                        ui.weak("no .lua files there yet; mc.scripts_dir() is the full path");
                    }
                    for file in files {
                        ui.horizontal(|ui| {
                            if ui.small_button("run").clicked() {
                                run_file(&file);
                            }
                            ui.monospace(
                                file.file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            );
                        });
                    }
                });
            ui.separator();
            // Output.
            let input_h = 60.0;
            let out_h = (ui.available_height() - input_h).max(80.0);
            egui::ScrollArea::vertical()
                .id_salt("output")
                .max_height(out_h)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    for (kind, line) in lines() {
                        let text = egui::RichText::new(line).monospace();
                        match kind {
                            Kind::Echo => ui.label(text.color(egui::Color32::LIGHT_BLUE)),
                            Kind::Out => ui.label(text),
                            Kind::Err => ui.label(text.color(egui::Color32::LIGHT_RED)),
                        };
                    }
                });
            ui.separator();
            // Input: Enter runs the line; Tab takes the completion, Up and
            // Down pick one. The keys are taken before the field sees them,
            // against last frame's completions.
            let input_id = window_id.with("input");
            let selected_id = window_id.with("selected");
            let mut line: String = ui.data_mut(|d| d.get_temp(input_id).unwrap_or_default());
            let mut selected: usize = ui.data_mut(|d| d.get_temp(selected_id).unwrap_or_default());
            let edit_id = window_id.with("edit");
            let focused = ui.memory(|m| m.has_focus(edit_id));
            let cursor_of = |ctx: &egui::Context, text: &str| -> usize {
                egui::text_edit::TextEditState::load(ctx, edit_id)
                    .and_then(|st| st.cursor.char_range())
                    .map(|r| r.primary.index)
                    .map(|chars| text.char_indices().nth(chars.into()).map_or(text.len(), |(b, _)| b))
                    .unwrap_or(text.len())
            };
            let just_focused = take_focus();
            if just_focused {
                ui.memory_mut(|m| m.request_focus(edit_id));
            }
            let cursor = cursor_of(ui.ctx(), &line);
            let made = completions_for(&line, cursor);
            let hint = made.as_ref().and_then(|c| c.hint.clone());
            let shown = made.filter(|c| !c.items.is_empty());
            let mut set_cursor_to: Option<usize> = if just_focused { Some(line.len()) } else { None };
            // With no completions to pick from, Up and Down walk the lines
            // run before, as a shell does; Down past the newest gives back
            // what was being typed.
            let hist_pos_id = window_id.with("history pos");
            let draft_id = window_id.with("draft");
            if focused && shown.is_none() {
                let (down, up) = ui.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    )
                });
                if up || down {
                    let history = history();
                    let mut pos: Option<usize> = ui.data_mut(|d| d.get_temp(hist_pos_id)).flatten();
                    if up && !history.is_empty() {
                        let next = match pos {
                            None => {
                                ui.data_mut(|d| d.insert_temp(draft_id, line.clone()));
                                history.len() - 1
                            }
                            Some(p) => p.saturating_sub(1),
                        };
                        pos = Some(next);
                        line = history[next].clone();
                        set_cursor_to = Some(line.len());
                    } else if down && let Some(p) = pos {
                        if p + 1 < history.len() {
                            pos = Some(p + 1);
                            line = history[p + 1].clone();
                        } else {
                            pos = None;
                            line = ui.data_mut(|d| d.get_temp(draft_id)).unwrap_or_default();
                        }
                        set_cursor_to = Some(line.len());
                    }
                    ui.data_mut(|d| d.insert_temp(hist_pos_id, pos));
                }
            }
            if focused && let Some(c) = &shown {
                let n = c.items.len();
                selected = selected.min(n - 1);
                let (tab, down, up) = ui.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    )
                });
                if down {
                    selected = (selected + 1) % n;
                }
                if up {
                    selected = (selected + n - 1) % n;
                }
                if tab {
                    let item = &c.items[selected].0;
                    let (a, b) = (c.range.0.min(line.len()), c.range.1.min(line.len()));
                    line = format!("{}{}{}", &line[..a], item, &line[b..]);
                    set_cursor_to = Some(a + item.len());
                    selected = 0;
                }
            }
            // A Tab the field would type as text is dropped with the key.
            ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "\t")));
            let mut run = false;
            ui.horizontal(|ui| {
                // egui's thin line cursor is replaced by a blinking block below.
                ui.style_mut().visuals.text_cursor.stroke = egui::Stroke::NONE;
                ui.style_mut().visuals.text_cursor.blink = false;
                let out = egui::TextEdit::singleline(&mut line)
                    .id(edit_id)
                    .desired_width(ui.available_width() - 150.0)
                    .font(egui::TextStyle::Monospace)
                    .lock_focus(true)
                    .show(ui);
                if let Some(byte) = set_cursor_to {
                    let chars = line[..byte.min(line.len())].chars().count();
                    let mut st = out.state;
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(chars),
                    )));
                    st.store(ui.ctx(), edit_id);
                }
                // The block cursor: at egui's cursor, or at the end of the
                // line while the field has focus but no cursor yet (just
                // focused by /lua, before a click or a key).
                if out.response.has_focus() {
                    let ccursor = out.cursor_range.map_or_else(
                        || egui::text::CCursor::new(line.chars().count()),
                        |r| r.primary,
                    );
                    let at = out.galley.pos_from_cursor(ccursor);
                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                    let (w, h) = ui
                        .ctx()
                        .fonts_mut(|f| (f.glyph_width(&font, ' '), f.row_height(&font)));
                    let on = ui.input(|i| i.time) % 1.0 < 0.6;
                    if on {
                        let min = out.galley_pos + at.min.to_vec2();
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(min, egui::vec2(w.max(4.0), h)),
                            1.0,
                            egui::Color32::from_white_alpha(150),
                        );
                    }
                    ui.ctx().request_repaint();
                }
                let enter =
                    out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("run").clicked() || enter {
                    run = true;
                    out.response.request_focus();
                }
                if ui.button("clear").clicked() {
                    clear();
                }
            });
            // Ask for this frame's completions; they show next frame.
            let cursor = set_cursor_to.unwrap_or_else(|| cursor_of(ui.ctx(), &line));
            request_completions(&line, cursor);
            if focused && (shown.is_some() || hint.is_some()) {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    if let Some(h) = &hint {
                        ui.label(egui::RichText::new(h).monospace().weak());
                    }
                    let Some(c) = &shown else { return };
                    if hint.is_some() {
                        ui.separator();
                    }
                    for (i, (text, tooltip)) in c.items.iter().take(10).enumerate() {
                        let label = egui::RichText::new(text).monospace();
                        let label = if i == selected { label.strong() } else { label };
                        let r = ui.selectable_label(i == selected, label);
                        if let Some(tip) = tooltip {
                            r.clone().on_hover_text(tip);
                        }
                        if r.clicked() {
                            selected = i;
                            ui.input_mut(|i| i.events.push(egui::Event::Key {
                                key: egui::Key::Tab,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            }));
                        }
                    }
                    if c.items.len() > 10 {
                        ui.weak(format!("{} more", c.items.len() - 10));
                    }
                });
            }
            ui.horizontal(|ui| {
                if TICKING.load(Ordering::Relaxed) {
                    if ui.button("stop callbacks").clicked() {
                        STOP_TICKS.store(true, Ordering::Relaxed);
                    }
                    ui.weak("mc.on_tick handlers are running");
                } else {
                    ui.weak("Tab completes mc.* and, inside mc.cmd(\"…\"), vanilla commands; Up/Down pick or recall; Enter runs");
                }
            });
            if run && !line.trim().is_empty() {
                remember(&line);
                submit(format!("> {line}"), line.clone());
                line.clear();
                ui.data_mut(|d| d.insert_temp(hist_pos_id, None::<usize>));
            }
            ui.data_mut(|d| {
                d.insert_temp(input_id, line);
                d.insert_temp(selected_id, selected);
            });
        });
    if !open {
        OPEN.store(false, Ordering::Relaxed);
    }
}
