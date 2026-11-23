//! The developer tools' input: the egui context the overlay draws with, the window's events and
//! each frame's input. The old `dev_input` window adapter, on the engine's window taps
//! (`horizon_client_input::window_tap`). The tools are on whenever this mod is loaded, so the taps
//! always answer; the pointer is the game's until the tools' key (`key.devTools`, F8) frees it.
//! While it is free the mouse and the keyboard are the tools' (egui's), not the game's; the same
//! key gives them back.
//!
//! Two threads meet here: the window thread feeds egui its events ([`window_event`]) and hands
//! over each frame's input ([`frame`]); the render thread takes that input in the overlay's draw
//! ([`take_input`]).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use horizon_client_input::keybinds::{key_name, mouse_name};
use horizon_client_input::window_tap::{PointerChange, Tap, TapContext, Taps};
use winit::event::{ElementState, WindowEvent};
use winit::keyboard::PhysicalKey;
use winit::window::Window;

/// `HORIZON_DEV_TOOLS_SCALE`: egui's zoom over the window's own scale. Unset, it follows the
/// window's height in pixels (see [`scale_for_height`]).
pub const SCALE_ENV: &str = "HORIZON_DEV_TOOLS_SCALE";

/// Whether the tools hold the pointer: freed by their key, so nothing in the game grabs it.
static HOLDS_POINTER: AtomicBool = AtomicBool::new(false);
/// Whether the tools' windows were showing last frame (the console or an entity window).
static HAD_WINDOWS: AtomicBool = AtomicBool::new(false);
/// The egui context, shared by the window thread (input) and the render thread (drawing).
static CTX: OnceLock<egui::Context> = OnceLock::new();
/// The frame's input, built on the window thread and taken by the render thread.
static INPUT: Mutex<Option<egui::RawInput>> = Mutex::new(None);
/// Whether the zoom was set from the window's size (once, unless the variable chose it).
static ZOOM_SET: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// egui-winit's event translation; the window thread's alone.
    static WINIT: RefCell<Option<egui_winit::State>> = const { RefCell::new(None) };
}

/// Installs the tools' window taps. Called once, with the mod (`lib.rs` `mods`).
pub fn install() {
    horizon_client_input::window_tap::install(Taps {
        window_event,
        holds_pointer,
        frame,
    });
}

/// The zoom a value of [`SCALE_ENV`] asks for, if it is a usable one.
fn parse_scale(value: &str) -> Option<f32> {
    value
        .parse::<f32>()
        .ok()
        .filter(|s| s.is_finite() && (0.25..=8.0).contains(s))
}

/// The zoom the variable asks for, if any.
fn scale_from_env() -> Option<f32> {
    std::env::var(SCALE_ENV).ok().and_then(|v| parse_scale(&v))
}

/// The zoom for a window `height` points tall: 1.0 below 1200, 1.5 up to 2000, 2.0 above (a 1440p
/// or 4K window at scale 1 would otherwise be tiny).
fn scale_for_height(height: f32) -> f32 {
    if height >= 2000.0 {
        2.0
    } else if height >= 1200.0 {
        1.5
    } else {
        1.0
    }
}

/// The egui context, shared by the window thread and the render thread.
pub fn ctx() -> &'static egui::Context {
    CTX.get_or_init(|| {
        let ctx = egui::Context::default();
        ctx.set_zoom_factor(scale_from_env().unwrap_or(1.0));
        ctx
    })
}

/// Sets the zoom from the window's height on the first frame, unless the variable chose it.
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

/// Whether `event` presses `key`, the binding of `key.devTools`: a key, or a mouse button.
fn presses(event: &WindowEvent, key: &str) -> bool {
    match event {
        WindowEvent::KeyboardInput {
            event: k,
            is_synthetic: false,
            ..
        } => {
            k.state == ElementState::Pressed
                && !k.repeat
                && matches!(k.physical_key, PhysicalKey::Code(code) if key_name(code) == Some(key))
        }
        WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button,
            ..
        } => mouse_name(*button) == key,
        _ => false,
    }
}

/// Whether the tools' key changes the pointer now: it frees a pointer the game holds only while
/// the game is `playing`, and it takes back a pointer the tools hold from anywhere.
fn toggles(held: bool, playing: bool) -> bool {
    held || playing
}

/// The tools' key was pressed: the pointer is freed for the tools (only while `playing`), or
/// given back to the game.
fn toggle(window: &Window, playing: bool) -> Tap {
    let held = holds_pointer();
    if !toggles(held, playing) {
        return Tap::Pass;
    }
    HOLDS_POINTER.store(!held, Ordering::Relaxed);
    if held {
        // The pointer leaves egui with the grab: no hover stays lit.
        with_winit(window, |s| {
            s.egui_input_mut().events.push(egui::Event::PointerGone);
        });
    }
    Tap::Toggled { free: !held }
}

/// What kind of window event a tap sees while the pointer is free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FreeKind {
    /// The cursor and the mouse buttons and wheel: always the tools' while free.
    Pointer,
    /// The keyboard and text input: the tools' only when egui took it (a text field has focus).
    Keyboard,
    /// Anything else (touch included): the game's.
    Other,
}

fn free_kind(event: &WindowEvent) -> FreeKind {
    match event {
        WindowEvent::CursorMoved { .. }
        | WindowEvent::CursorEntered { .. }
        | WindowEvent::CursorLeft { .. }
        | WindowEvent::MouseInput { .. }
        | WindowEvent::MouseWheel { .. } => FreeKind::Pointer,
        WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => FreeKind::Keyboard,
        _ => FreeKind::Other,
    }
}

/// Whether a window event is the tools' while the pointer is free, as the old adapter had it:
/// the mouse always, the keyboard and text input only when egui consumed them. So the game still
/// walks with the pointer free, and a key held across the toggle cannot stick.
fn takes_while_free(kind: FreeKind, egui_consumed: bool) -> bool {
    match kind {
        FreeKind::Pointer => true,
        FreeKind::Keyboard => egui_consumed,
        FreeKind::Other => false,
    }
}

/// The window taps' event hook: the tools' key toggles the pointer; while the tools hold it,
/// egui sees each event and the game does not see the mouse or the keyboard.
fn window_event(event: &WindowEvent, cx: &TapContext<'_>) -> Tap {
    let Some(window) = cx.window else {
        return Tap::Pass;
    };
    if presses(event, cx.devtools_key) {
        return toggle(window, cx.playing);
    }
    // Modifiers are tracked even while the game has the pointer, so a text field typed into right
    // after the key sees the right ones.
    if let WindowEvent::ModifiersChanged(_) = event {
        with_winit(window, |s| {
            let _ = s.on_window_event(window, event);
        });
        return Tap::Pass;
    }
    if !holds_pointer() {
        return Tap::Pass;
    }
    let response = with_winit(window, |s| s.on_window_event(window, event));
    if takes_while_free(free_kind(event), response.consumed) {
        Tap::Consumed
    } else {
        Tap::Pass
    }
}

/// Whether the tools hold the pointer now: the game must not grab it.
fn holds_pointer() -> bool {
    HOLDS_POINTER.load(Ordering::Relaxed)
}

/// The pointer's change for one request: the tools take the pointer when the request asks for it
/// (`console`) and they do not hold it yet. A request the tools already hold is nothing.
fn pointer_change(want: bool) -> PointerChange {
    if want && !holds_pointer() {
        HOLDS_POINTER.store(true, Ordering::Relaxed);
        PointerChange::Release
    } else {
        PointerChange::None
    }
}

/// The pointer's change the frame asks for: the console's request, taken once.
fn take_pointer_request() -> PointerChange {
    pointer_change(crate::console::take_want_pointer())
}

/// Whether the tools give the pointer back this frame: they hold it, their windows were showing
/// last frame and none shows now (the last one closed, as the old adapter did).
fn hand_back(holds: bool, had: bool, windows: bool) -> bool {
    holds && had && !windows
}

/// The pointer's change for one frame: the request wins (a console open takes the pointer), else
/// the hand-back when the last window closed. Records whether windows show now, for the next
/// frame.
fn pointer_step(request: PointerChange, windows: bool) -> PointerChange {
    let had = HAD_WINDOWS.swap(windows, Ordering::Relaxed);
    if request != PointerChange::None {
        return request;
    }
    if hand_back(holds_pointer(), had, windows) {
        HOLDS_POINTER.store(false, Ordering::Relaxed);
        return PointerChange::Grab;
    }
    PointerChange::None
}

/// Once a frame on the window thread: egui's input for the frame (events, time, screen rectangle,
/// scale), kept for the render thread. A frame the render thread skipped keeps its events. The
/// pointer changes when the console asks for it (`console` opens the window: the tools take the
/// pointer so the input can be typed in), or when the tools' last window closes while they hold
/// it (the pointer goes back to the game).
fn frame(window: Option<&Window>) -> PointerChange {
    let Some(window) = window else {
        return PointerChange::None;
    };
    zoom_for(window);
    let windows = event_horizon_observe::showing() || crate::console::is_open();
    let change = pointer_step(take_pointer_request(), windows);
    if change == PointerChange::Grab {
        with_winit(window, |s| {
            s.egui_input_mut().events.push(egui::Event::PointerGone);
        });
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

/// The frame's input, taken by the render thread in the overlay's draw. `None` when no frame came
/// in since the last take.
pub fn take_input() -> Option<egui::RawInput> {
    INPUT.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::{DeviceId, MouseButton, MouseScrollDelta, TouchPhase};

    #[test]
    fn the_zoom_follows_the_window_height() {
        assert_eq!(scale_for_height(800.0), 1.0);
        assert_eq!(scale_for_height(1199.0), 1.0);
        assert_eq!(scale_for_height(1200.0), 1.5);
        assert_eq!(scale_for_height(1999.0), 1.5);
        assert_eq!(scale_for_height(2000.0), 2.0);
        assert_eq!(scale_for_height(2160.0), 2.0);
    }

    #[test]
    fn the_env_override_takes_a_zoom_in_range_only() {
        assert_eq!(parse_scale("1.25"), Some(1.25));
        assert_eq!(parse_scale("0.25"), Some(0.25));
        assert_eq!(parse_scale("8"), Some(8.0));
        assert_eq!(parse_scale("0.2"), None);
        assert_eq!(parse_scale("9"), None);
        assert_eq!(parse_scale("nan"), None);
        assert_eq!(parse_scale("big"), None);
    }

    #[test]
    fn the_key_toggles_only_while_the_game_plays_or_the_tools_hold_the_pointer() {
        assert!(!toggles(false, false), "no pointer to free outside play");
        assert!(toggles(false, true), "play frees the pointer");
        assert!(toggles(true, false), "the tools give it back from anywhere");
        assert!(toggles(true, true));
    }

    #[test]
    fn a_mouse_press_of_the_key_is_a_press_and_a_release_is_not() {
        let press = WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Pressed,
            button: MouseButton::Middle,
        };
        let release = WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Released,
            button: MouseButton::Middle,
        };
        let name = mouse_name(MouseButton::Middle);
        assert!(presses(&press, &name));
        assert!(!presses(&release, &name));
        assert!(!presses(&press, "key.mouse.left"));
    }

    #[test]
    fn while_free_the_mouse_is_the_tools_and_the_keyboard_only_when_egui_took_it() {
        // The old adapter's rule: the mouse always, the keys and text only when egui consumed them.
        assert!(takes_while_free(FreeKind::Pointer, false));
        assert!(takes_while_free(FreeKind::Pointer, true));
        assert!(
            !takes_while_free(FreeKind::Keyboard, false),
            "walking with the pointer free"
        );
        assert!(
            takes_while_free(FreeKind::Keyboard, true),
            "typing in a field with focus"
        );
        assert!(
            !takes_while_free(FreeKind::Other, true),
            "touch passes, as in the old code"
        );
        assert!(!takes_while_free(FreeKind::Other, false));
    }

    #[test]
    fn the_events_sort_into_pointer_keyboard_and_other() {
        let wheel = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::LineDelta(0.0, 1.0),
            phase: TouchPhase::Moved,
        };
        let left = WindowEvent::CursorLeft {
            device_id: DeviceId::dummy(),
        };
        assert_eq!(free_kind(&wheel), FreeKind::Pointer);
        assert_eq!(free_kind(&left), FreeKind::Pointer);
        assert_eq!(free_kind(&WindowEvent::Focused(true)), FreeKind::Other);
        assert_eq!(
            free_kind(&WindowEvent::Ime(winit::event::Ime::Enabled)),
            FreeKind::Keyboard
        );
    }

    #[test]
    fn a_console_request_takes_the_pointer_once_and_not_again_while_held() {
        let _console = crate::console::test_guard();
        HOLDS_POINTER.store(false, Ordering::Relaxed);
        crate::console::act(crate::console::Action::Open);
        assert_eq!(take_pointer_request(), PointerChange::Release);
        assert!(holds_pointer(), "the tools hold the pointer now");
        assert_eq!(
            take_pointer_request(),
            PointerChange::None,
            "the request is taken"
        );
        crate::console::act(crate::console::Action::Open);
        assert_eq!(
            take_pointer_request(),
            PointerChange::None,
            "a pointer the tools hold is not released again"
        );
        HOLDS_POINTER.store(false, Ordering::Relaxed);
        *crate::console::state() = crate::console::State::default();
    }

    #[test]
    fn with_no_request_the_pointer_does_not_change() {
        assert_eq!(pointer_change(false), PointerChange::None);
    }

    #[test]
    fn the_tools_hand_the_pointer_back_only_when_their_last_window_closed() {
        for holds in [false, true] {
            for had in [false, true] {
                for windows in [false, true] {
                    assert_eq!(
                        hand_back(holds, had, windows),
                        holds && had && !windows,
                        "holds {holds}, had {had}, windows {windows}"
                    );
                }
            }
        }
    }

    /// Resets the statics the frame step reads, so the tests do not see each other.
    fn fresh_step_state(holds: bool) {
        HOLDS_POINTER.store(holds, Ordering::Relaxed);
        HAD_WINDOWS.store(false, Ordering::Relaxed);
        *crate::console::state() = crate::console::State::default();
    }

    #[test]
    fn closing_the_console_gives_the_pointer_back_once() {
        let _console = crate::console::test_guard();
        fresh_step_state(false);
        // `console` opens the window: the tools take the pointer once.
        crate::console::act(crate::console::Action::Open);
        let windows = crate::console::is_open();
        assert_eq!(
            pointer_step(take_pointer_request(), windows),
            PointerChange::Release
        );
        assert!(holds_pointer());
        // The console stays open: nothing changes.
        assert_eq!(pointer_step(PointerChange::None, true), PointerChange::None);
        assert!(holds_pointer());
        // `console close`: the last window is gone, so the pointer goes back to the game.
        crate::console::act(crate::console::Action::Close);
        let windows = crate::console::is_open();
        assert_eq!(
            pointer_step(take_pointer_request(), windows),
            PointerChange::Grab
        );
        assert!(!holds_pointer(), "the hold is dropped");
        assert_eq!(
            pointer_step(PointerChange::None, false),
            PointerChange::None
        );
        fresh_step_state(false);
    }

    #[test]
    fn an_entity_window_closing_while_the_tools_hold_the_pointer_hands_it_back() {
        let _console = crate::console::test_guard();
        // F8 freed the pointer with a window open: the hold is the tools'.
        fresh_step_state(true);
        HAD_WINDOWS.store(true, Ordering::Relaxed);
        // A window still open: nothing changes.
        assert_eq!(pointer_step(PointerChange::None, true), PointerChange::None);
        assert!(holds_pointer());
        // The last window closes: the pointer is the game's again, once.
        assert_eq!(
            pointer_step(PointerChange::None, false),
            PointerChange::Grab
        );
        assert!(!holds_pointer());
        assert_eq!(
            pointer_step(PointerChange::None, false),
            PointerChange::None
        );
        fresh_step_state(false);
    }

    #[test]
    fn no_window_ever_shown_changes_nothing_without_a_hold() {
        let _console = crate::console::test_guard();
        // The pointer was never held (the game's), and an entity window came and went.
        fresh_step_state(false);
        assert_eq!(pointer_step(PointerChange::None, true), PointerChange::None);
        assert_eq!(
            pointer_step(PointerChange::None, false),
            PointerChange::None
        );
        assert!(!holds_pointer());
        fresh_step_state(false);
    }

    #[test]
    fn focus_is_not_the_tools_while_free() {
        assert!(!takes_while_free(
            free_kind(&WindowEvent::Focused(true)),
            true
        ));
        assert!(!takes_while_free(
            free_kind(&WindowEvent::Focused(false)),
            true
        ));
    }
}
