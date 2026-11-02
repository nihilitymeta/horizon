//! The Lua console's window and its queues (docs/dev-tools.md). `/lua`
//! opens it; a line typed, or a script of `dev-tools/scripts/` picked from
//! its list, is queued here and run on the server thread by
//! `game/commands/lua_console.rs`, whose output comes back as lines. The
//! window never touches the game: it only reads and writes these queues.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Where scripts live, under the game folder (the working directory).
pub const SCRIPTS_DIR: &str = "dev-tools/scripts";
/// Output lines kept.
const KEEP: usize = 1000;

/// Whether the window is open (`/lua` toggles it).
pub static OPEN: AtomicBool = AtomicBool::new(false);
/// A click on "stop callbacks": the tick handlers are dropped.
pub static STOP_TICKS: AtomicBool = AtomicBool::new(false);

/// A chunk to run: what the window shows for it, and the code.
#[derive(Clone, Debug)]
pub struct Request {
    pub label: String,
    pub code: String,
}

/// What a line of the output is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What was run, echoed.
    Echo,
    /// `print` and results.
    Out,
    /// An error.
    Err,
}

static INPUT: Mutex<Vec<Request>> = Mutex::new(Vec::new());
static OUTPUT: Mutex<VecDeque<(Kind, String)>> = Mutex::new(VecDeque::new());
/// Whether the server thread has a VM running tick callbacks (shown in the window).
pub static TICKING: AtomicBool = AtomicBool::new(false);

/// Completions for the input line: the byte range they replace and the
/// candidates (text, tooltip), for the line and cursor they were made for.
#[derive(Clone, Debug, Default)]
pub struct Completions {
    pub text: String,
    pub cursor: usize,
    pub range: (usize, usize),
    pub items: Vec<(String, Option<String>)>,
}

static COMPLETION_REQUEST: Mutex<Option<(String, usize)>> = Mutex::new(None);
static COMPLETIONS: Mutex<Option<Completions>> = Mutex::new(None);

/// The window asks for completions of `text` at byte `cursor`; the window
/// thread's game answers with [`set_completions`].
pub fn request_completions(text: &str, cursor: usize) {
    let known = COMPLETIONS.lock().unwrap_or_else(|e| e.into_inner());
    if known
        .as_ref()
        .is_some_and(|c| c.text == text && c.cursor == cursor)
    {
        return;
    }
    *COMPLETION_REQUEST.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((text.to_string(), cursor));
}

/// The pending request, if any (the window thread's game).
pub fn take_completion_request() -> Option<(String, usize)> {
    COMPLETION_REQUEST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

pub fn set_completions(c: Completions) {
    *COMPLETIONS.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
}

/// The completions made for exactly this line and cursor, if any.
pub fn completions_for(text: &str, cursor: usize) -> Option<Completions> {
    COMPLETIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|c| c.text == text && c.cursor == cursor)
        .cloned()
}

/// The `mc` table and the Lua names worth offering, with what they do.
const LUA_NAMES: &[(&str, &str)] = &[
    ("mc.cmd(", "run a vanilla command as the owner: ok, lines"),
    ("mc.select(", "entity ids a selector finds"),
    ("mc.get(", "entity data as a table; a path narrows it"),
    ("mc.snbt(", "entity data as SNBT text; a path narrows it"),
    ("mc.set(", "/data modify … set value: entity, path, value"),
    ("mc.merge(", "/data merge entity: entity, value"),
    ("mc.pos(", "x, y, z of an entity"),
    ("mc.name(", "an entity's display name"),
    ("mc.inspect(", "open a window: entity, \"data\"|\"goals\"|\"senses\"|\"nav\""),
    ("mc.watch(", "open a /watch chart: entity, path"),
    ("mc.on_tick(", "run a function every server tick"),
    ("mc.off_tick()", "stop the tick functions"),
    ("mc.run(", "run a script of the scripts folder"),
    ("mc.scripts_dir()", "the scripts folder"),
    ("mc.log(", "to the window and the game log"),
    ("print(", "to the window"),
    ("string.format(", ""),
    ("tostring(", ""),
    ("tonumber(", ""),
    ("ipairs(", ""),
    ("pairs(", ""),
    ("local ", ""),
    ("function", ""),
    ("return", ""),
    ("end", ""),
    ("then", ""),
    ("for ", ""),
    ("if ", ""),
    ("while ", ""),
    ("not ", ""),
    ("and ", ""),
    ("or ", ""),
    ("nil", ""),
    ("true", ""),
    ("false", ""),
];

/// Completions of a Lua name at the cursor: the identifier before it
/// (letters, digits, `_`, `.`) against [`LUA_NAMES`].
pub fn lua_completions(text: &str, cursor: usize) -> Completions {
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '.')
        .last()
        .map_or(cursor, |(i, _)| i);
    let prefix = &before[start..];
    let items = if prefix.is_empty() {
        Vec::new()
    } else {
        LUA_NAMES
            .iter()
            .filter(|(name, _)| name.starts_with(prefix) && *name != prefix)
            .map(|(name, what)| {
                (
                    name.to_string(),
                    (!what.is_empty()).then(|| what.to_string()),
                )
            })
            .collect()
    };
    Completions {
        text: text.to_string(),
        cursor,
        range: (start, cursor),
        items,
    }
}

/// When the cursor is inside the string of an `mc.cmd("…")` call: where
/// the command text starts (after the quote and a slash, if any) and the
/// command typed so far.
pub fn command_context(text: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = cursor.min(text.len());
    let mut quote: Option<(char, usize)> = None;
    let mut escaped = false;
    for (i, c) in text[..cursor].char_indices() {
        match quote {
            Some((q, _)) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => quote = Some((c, i + 1)),
            None => {}
        }
    }
    let (_, start) = quote?;
    let call = text[..start - 1].trim_end();
    if !call.ends_with("mc.cmd(") {
        return None;
    }
    let start = if text[start..cursor].starts_with('/') {
        start + 1
    } else {
        start
    };
    Some((start, text[start..cursor].to_string()))
}

/// Queue `code` to run on the server thread.
pub fn submit(label: impl Into<String>, code: impl Into<String>) {
    INPUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Request {
            label: label.into(),
            code: code.into(),
        });
}

/// The requests queued since the last take (the server thread's).
pub fn take_requests() -> Vec<Request> {
    std::mem::take(&mut *INPUT.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A line for the window.
pub fn push(kind: Kind, text: impl Into<String>) {
    let mut out = OUTPUT.lock().unwrap_or_else(|e| e.into_inner());
    for line in text.into().lines() {
        out.push_back((kind, line.to_string()));
    }
    while out.len() > KEEP {
        out.pop_front();
    }
}

/// Everything shown.
pub fn lines() -> Vec<(Kind, String)> {
    OUTPUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

/// Forget the output.
pub fn clear() {
    OUTPUT.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

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
            let cursor = cursor_of(ui.ctx(), &line);
            let shown = completions_for(&line, cursor).filter(|c| !c.items.is_empty());
            let mut set_cursor_to: Option<usize> = None;
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
                let out = egui::TextEdit::singleline(&mut line)
                    .id(edit_id)
                    .hint_text("Lua, e.g. print(mc.snbt(\"@e[type=pig,limit=1]\", \"Health\"))")
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
            if focused && let Some(c) = &shown {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
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
                    ui.weak("Tab completes mc.* and, inside mc.cmd(\"…\"), vanilla commands; Enter runs");
                }
            });
            if run && !line.trim().is_empty() {
                submit(format!("> {line}"), line.clone());
                line.clear();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_inside_an_mc_cmd_string_is_a_command_context() {
        assert_eq!(
            command_context("mc.cmd(\"tp @e\")", 11),
            Some((8, "tp ".to_string()))
        );
        // A leading slash is skipped; the quote kind does not matter.
        assert_eq!(command_context("mc.cmd('/kill', 1)", 12), Some((9, "kil".to_string())));
        // A closed string, another call, or no string: nothing.
        assert_eq!(command_context("mc.cmd(\"tp\") ", 13), None);
        assert_eq!(command_context("print(\"tp\")", 9), None);
        assert_eq!(command_context("mc.cmd(", 7), None);
        // An escaped quote stays inside the string.
        assert_eq!(command_context("mc.cmd(\"say \\\"hi\\\" x", 20), Some((8, "say \\\"hi\\\" x".to_string())));
    }

    #[test]
    fn lua_names_complete_by_prefix() {
        let c = lua_completions("x = mc.se", 9);
        assert_eq!(c.range, (4, 9));
        let names: Vec<&str> = c.items.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["mc.select(", "mc.set("]);
        // Nothing to complete without a name, and an exact name is not offered again.
        assert!(lua_completions("x = ", 4).items.is_empty());
        assert!(lua_completions("end", 3).items.is_empty());
    }
}
