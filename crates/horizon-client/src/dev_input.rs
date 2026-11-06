//! The developer tools' input half: the state the game's own code reaches
//! for (the pointer, the key, the windows' input) and the queues the game
//! and the tools share (the console's lines, the scripted mods' status).
//! The windows half (`dev_tools`) draws with this and never the other way
//! round, so the two can move apart (#19).
//!
//! Three threads meet here. The window thread feeds egui its events
//! ([`window_event`], from `Game::handle_window_event_inner`) and hands over
//! each frame's input ([`frame`], from `Game::extras_frame`); the render
//! thread runs egui and draws it in the renderer's hook point after the
//! world and before the HUD; the server thread fills the brain snapshots
//! (`mobs::observe`). The pointer is the game's until the tools' key frees it
//! ([`holds_pointer`], which `Game::capture` obeys); while free, the mouse is
//! the tools' and the keyboard is theirs only when a text field has focus.
pub mod console;
pub mod mods;

use crate::mobs::observe;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use winit::event::{ElementState, WindowEvent};
use winit::keyboard::PhysicalKey;
use winit::window::Window;

/// `HORIZON_DEV_TOOLS_SCALE`: egui's zoom over the window's own scale. Unset,
/// it follows the window's height in pixels: 1.0 below 1200, 1.5 up to 2000,
/// 2.0 above (a 1440p or 4K window at scale 1 would otherwise be tiny).
pub const SCALE_ENV: &str = "HORIZON_DEV_TOOLS_SCALE";

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Whether the windows are installed: with the tools, or for the inspect stick.
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// Whether this client told its server, since it joined, that it shows the
/// windows (`observe_wire::Wire::Hello`).
pub static HELLO: AtomicBool = AtomicBool::new(false);
static HOLDS_POINTER: AtomicBool = AtomicBool::new(false);
/// The egui context, shared by the window thread (input) and the render thread (drawing).
static CTX: OnceLock<egui::Context> = OnceLock::new();
/// The frame's input, built on the window thread and taken by the render thread.
static INPUT: Mutex<Option<egui::RawInput>> = Mutex::new(None);
/// Whether a window of the tools was open last frame, for [`frame`].
static HAD_WINDOWS: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// egui-winit's event translation; the window thread's alone.
    static WINIT: RefCell<Option<egui_winit::State>> = const { RefCell::new(None) };
}

/// Whether the tools are on (`HORIZON_DEV_TOOLS=1`).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turn the tools on.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether the windows are installed: with the tools, or for the inspect stick.
pub fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// The windows are installed: with the tools, or for the inspect stick.
pub fn install() {
    INSTALLED.store(true, Ordering::Relaxed);
}

/// Whether the windows take input and draw now: the tools are on, or they
/// are installed and the inspect stick opened a window.
fn active() -> bool {
    enabled() || (INSTALLED.load(Ordering::Relaxed) && (observe::showing() || holds_pointer()))
}

/// Whether the tools hold the pointer: freed by their key, so nothing in the
/// game grabs it until the key is pressed again.
pub fn holds_pointer() -> bool {
    HOLDS_POINTER.load(Ordering::Relaxed)
}

/// The zoom the variable asks for, if any.
fn scale_from_env() -> Option<f32> {
    std::env::var(SCALE_ENV)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|s| s.is_finite() && *s >= 0.25 && *s <= 8.0)
}

/// The zoom for a window `height` pixels tall, in points of the window's own scale.
fn scale_for_height(height: f32) -> f32 {
    if height >= 2000.0 {
        2.0
    } else if height >= 1200.0 {
        1.5
    } else {
        1.0
    }
}

/// The egui context, shared by the window thread (input) and the render
/// thread (drawing).
pub fn ctx() -> &'static egui::Context {
    CTX.get_or_init(|| {
        let ctx = egui::Context::default();
        ctx.set_zoom_factor(scale_from_env().unwrap_or(1.0));
        ctx
    })
}

/// Sets the zoom from the window's size once, unless the variable chose it.
static ZOOM_SET: AtomicBool = AtomicBool::new(false);
fn zoom_for(window: &Window) {
    if ZOOM_SET.swap(true, Ordering::Relaxed) {
        return;
    }
    if scale_from_env().is_none() {
        let height = window.inner_size().height as f32 / window.scale_factor() as f32;
        ctx().set_zoom_factor(scale_for_height(height));
    }
}

fn with_winit<R>(window: &Window, f: impl FnOnce(&mut egui_winit::State) -> R) -> R {
    WINIT.with(|cell| {
        let mut slot = cell.borrow_mut();
        let state = slot.get_or_insert_with(|| {
            egui_winit::State::new(
                ctx().clone(),
                egui::ViewportId::ROOT,
                window,
                Some(window.scale_factor() as f32),
                window.theme(),
                None,
            )
        });
        f(state)
    })
}

/// What the tools did with a window event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Not theirs: the game handles it.
    Pass,
    /// Theirs: the game skips it.
    Consumed,
    /// Their key: the pointer is now free (`true`) or the game's again.
    Toggled { free: bool },
}

/// Whether `event` presses `key`, the binding Free Mouse for Dev Windows
/// (`key.devTools`): a key, or a mouse button.
fn presses(event: &WindowEvent, key: &str) -> bool {
    match event {
        WindowEvent::KeyboardInput {
            event: k,
            is_synthetic: false,
            ..
        } => {
            k.state == ElementState::Pressed
                && !k.repeat
                && matches!(k.physical_key, PhysicalKey::Code(code)
                    if crate::keybinds::key_name(code) == Some(key))
        }
        WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button,
            ..
        } => crate::keybinds::mouse_name(*button) == key,
        _ => false,
    }
}

/// The binding Free Mouse for Dev Windows was pressed: the pointer is freed
/// for the windows (only while `playing`), or given back. A window event's
/// press comes here, and so does horizon-ctl's `key`, which skips the window.
pub fn toggle(window: &Window, playing: bool) -> Input {
    let held = holds_pointer();
    if !held && !playing {
        return Input::Pass;
    }
    HOLDS_POINTER.store(!held, Ordering::Relaxed);
    if held {
        // The pointer leaves egui with the grab: no hover stays lit.
        with_winit(window, |s| {
            s.egui_input_mut().events.push(egui::Event::PointerGone);
        });
    }
    Input::Toggled { free: !held }
}

/// Whether the windows take a press of their key now (see [`toggle`]).
pub fn takes_key() -> bool {
    active()
}

/// A window event, before the game handles it. `playing` is whether the
/// pointer would be the game's now (in the world, no screen): the key frees
/// it only then, and takes it back from anywhere. `key` is the binding Free
/// Mouse for Dev Windows (`key.devTools`, F8 unless rebound).
pub fn window_event(
    window: Option<&Window>,
    event: &WindowEvent,
    playing: bool,
    key: &str,
) -> Input {
    if !active() {
        return Input::Pass;
    }
    let Some(window) = window else {
        return Input::Pass;
    };
    if presses(event, key) {
        return toggle(window, playing);
    }
    // Modifiers are tracked even while the game has the pointer, so a text
    // field typed into right after the key sees the right ones.
    if let WindowEvent::ModifiersChanged(_) = event {
        let _ = with_winit(window, |s| s.on_window_event(window, event));
        return Input::Pass;
    }
    if !holds_pointer() {
        return Input::Pass;
    }
    let response = with_winit(window, |s| s.on_window_event(window, event));
    match event {
        // The mouse is the tools' while it is free: a click beside their
        // windows does nothing, and the key gives it back.
        WindowEvent::CursorMoved { .. }
        | WindowEvent::CursorEntered { .. }
        | WindowEvent::CursorLeft { .. }
        | WindowEvent::MouseInput { .. }
        | WindowEvent::MouseWheel { .. } => Input::Consumed,
        // Keys are theirs only when a text field has focus, so walking with
        // the pointer free still works.
        WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) if response.consumed => {
            Input::Consumed
        }
        _ => Input::Pass,
    }
}

/// The world closed: the tools let the pointer go (the game frees it with
/// the world and grabs it again when the next one opens) and forget that
/// they had windows, which the store no longer lists (`observe::reset`).
pub fn world_closed() {
    HOLDS_POINTER.store(false, Ordering::Relaxed);
    HELLO.store(false, Ordering::Relaxed);
    HAD_WINDOWS.store(false, Ordering::Relaxed);
}

/// What [`frame`] asks of the game's pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerChange {
    None,
    /// The tools took the pointer (`/lua` opened the console): release it.
    Release,
    /// The tools let the pointer go (their last window closed): grab it back.
    Grab,
}

/// Once a frame on the window thread: egui's input for the frame (events,
/// time, the screen rectangle, the window's scale), kept for the render
/// thread. A frame the render thread skipped keeps its events. The answer
/// says whether the game should release or grab the pointer now.
pub fn frame(window: Option<&Window>) -> PointerChange {
    if !active() && !HAD_WINDOWS.load(Ordering::Relaxed) {
        return PointerChange::None;
    }
    let Some(window) = window else {
        return PointerChange::None;
    };
    zoom_for(window);
    let windows = observe::showing() || console::OPEN.load(Ordering::Relaxed);
    let had = HAD_WINDOWS.swap(windows, Ordering::Relaxed);
    let mut change = PointerChange::None;
    if console::WANT_POINTER.swap(false, Ordering::Relaxed) && !holds_pointer() {
        HOLDS_POINTER.store(true, Ordering::Relaxed);
        change = PointerChange::Release;
    } else if holds_pointer() && had && !windows {
        HOLDS_POINTER.store(false, Ordering::Relaxed);
        with_winit(window, |s| {
            s.egui_input_mut().events.push(egui::Event::PointerGone);
        });
        change = PointerChange::Grab;
    }
    let mut input = with_winit(window, |s| s.take_egui_input(window));
    let mut slot = INPUT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(previous) = slot.take() {
        let mut events = previous.events;
        events.append(&mut input.events);
        input.events = events;
    }
    *slot = Some(input);
    change
}

/// The frame's input, taken by the render thread (the windows' half).
pub fn take_input() -> Option<egui::RawInput> {
    INPUT.lock().unwrap_or_else(|e| e.into_inner()).take()
}
