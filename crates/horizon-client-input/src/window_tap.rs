//! Taps on the window, for a built-in mod that holds the pointer (the developer
//! tools' overlay). A mod installs one set of plain functions once; the game asks
//! it three things: whether it takes a raw window event, whether it holds the
//! pointer, and, once a frame, whether the pointer should be released or grabbed
//! back. With nothing installed the answers are "not mine", "no", and "no change",
//! so a build without the mod behaves as before.
use std::sync::OnceLock;
use winit::event::WindowEvent;
use winit::window::Window;

/// What a tap did with a window event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    /// Not the tap's: the game handles it.
    Pass,
    /// The tap's: the game skips it.
    Consumed,
    /// The tap's key: the pointer is now free (`true`) or the game's again.
    Toggled { free: bool },
}

/// What [`frame`] asks of the game's pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerChange {
    None,
    /// The tap took the pointer: release it.
    Release,
    /// The tap let the pointer go: grab it back.
    Grab,
}

/// What the game tells a tap about the window it is handling.
pub struct TapContext<'a> {
    pub window: Option<&'a Window>,
    /// The game is in play: no title, and the world's screen is open.
    pub playing: bool,
    /// The key that frees and grabs the pointer (the binding of `key.devTools`).
    pub devtools_key: &'a str,
}

/// The functions a built-in mod installs.
#[derive(Clone, Copy)]
pub struct Taps {
    pub window_event: fn(&WindowEvent, &TapContext<'_>) -> Tap,
    pub holds_pointer: fn() -> bool,
    pub frame: fn(Option<&Window>) -> PointerChange,
}

/// One install slot. The game uses the process-wide [`SLOT`]; tests use their own.
struct Slot(OnceLock<Taps>);

impl Slot {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    /// The first install wins; a later one is ignored.
    fn install(&self, taps: Taps) {
        let _ = self.0.set(taps);
    }

    fn window_event(&self, event: &WindowEvent, cx: &TapContext<'_>) -> Tap {
        self.0
            .get()
            .map_or(Tap::Pass, |t| (t.window_event)(event, cx))
    }

    fn holds_pointer(&self) -> bool {
        self.0.get().is_some_and(|t| (t.holds_pointer)())
    }

    fn frame(&self, window: Option<&Window>) -> PointerChange {
        self.0
            .get()
            .map_or(PointerChange::None, |t| (t.frame)(window))
    }
}

static SLOT: Slot = Slot::new();

/// Installs the mod's taps. The first install wins; later ones are ignored.
pub fn install(taps: Taps) {
    SLOT.install(taps);
}

/// Offers a window event to the installed tap before the game handles it.
pub fn window_event(event: &WindowEvent, cx: &TapContext<'_>) -> Tap {
    SLOT.window_event(event, cx)
}

/// Whether the installed tap holds the pointer: the game must not grab it.
pub fn holds_pointer() -> bool {
    SLOT.holds_pointer()
}

/// Once a frame: the pointer change the installed tap asks for.
pub fn frame(window: Option<&Window>) -> PointerChange {
    SLOT.frame(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses its own `Slot`, never the process-wide `SLOT`, so the
    // tests do not share install state.
    fn consume(_: &WindowEvent, _: &TapContext<'_>) -> Tap {
        Tap::Consumed
    }
    fn holds() -> bool {
        true
    }
    fn release(_: Option<&Window>) -> PointerChange {
        PointerChange::Release
    }
    fn pass(_: &WindowEvent, _: &TapContext<'_>) -> Tap {
        Tap::Pass
    }
    fn never() -> bool {
        false
    }
    fn grab(_: Option<&Window>) -> PointerChange {
        PointerChange::Grab
    }

    #[test]
    fn nothing_installed_answers_not_mine_no_and_no_change() {
        let slot = Slot::new();
        assert!(!slot.holds_pointer());
        assert_eq!(slot.frame(None), PointerChange::None);
    }

    #[test]
    fn the_first_install_wins_and_its_functions_answer() {
        let slot = Slot::new();
        slot.install(Taps {
            window_event: consume,
            holds_pointer: holds,
            frame: release,
        });
        slot.install(Taps {
            window_event: pass,
            holds_pointer: never,
            frame: grab,
        });
        assert!(slot.holds_pointer());
        assert_eq!(slot.frame(None), PointerChange::Release);
    }
}
