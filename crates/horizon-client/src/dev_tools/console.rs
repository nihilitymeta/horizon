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
            // Input: Enter runs the line; Shift+Enter is not needed, a script file is.
            let input_id = window_id.with("input");
            let mut line: String = ui.data_mut(|d| d.get_temp(input_id).unwrap_or_default());
            let mut run = false;
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut line)
                        .hint_text("Lua, e.g. print(mc.snbt(\"@e[type=pig,limit=1]\", \"Health\"))")
                        .desired_width(ui.available_width() - 150.0)
                        .font(egui::TextStyle::Monospace),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("run").clicked() || enter {
                    run = true;
                    edit.request_focus();
                }
                if ui.button("clear").clicked() {
                    clear();
                }
            });
            ui.horizontal(|ui| {
                if TICKING.load(Ordering::Relaxed) {
                    if ui.button("stop callbacks").clicked() {
                        STOP_TICKS.store(true, Ordering::Relaxed);
                    }
                    ui.weak("mc.on_tick handlers are running");
                } else {
                    ui.weak("mc.cmd, mc.select, mc.get, mc.snbt, mc.set, mc.merge, mc.pos, mc.inspect, mc.watch, mc.on_tick, mc.run");
                }
            });
            if run && !line.trim().is_empty() {
                submit(format!("> {line}"), line.clone());
                line.clear();
            }
            ui.data_mut(|d| d.insert_temp(input_id, line));
        });
    if !open {
        OPEN.store(false, Ordering::Relaxed);
    }
}
