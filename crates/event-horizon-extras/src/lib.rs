//! Event Horizon's four HUD extras as one built-in native mod (issue #29):
//! zoom, the minimap, the FPS counter and keystrokes — the look and options
//! the engine's fallback code had (`docs/extras.md`), on the native hook
//! points. Each feature is its own row in Options > Mods (the `mods` hook)
//! with its own switch (the `mod.<id>` line of `horizon-mods.txt`) and its
//! own settings page (the `settings` hook); the zoom's key is `key.zoom` and
//! the minimap's `key.minimap`, the Key Binds rows the engine keeps. The
//! engine still computes the minimap's tiles and draws the map a mod asks
//! for (`native_minimap`), and applies a mod's zoom to the field of view
//! (`native_zoom`); this mod draws the FPS counter and keystrokes panels in
//! the `hud` hook, and reads the frame and key state through the host.
pub mod settings;

use horizon_client::mod_api::{
    Corner, Corners, Hooks, Host, HudBatch, NativeMinimap, NativeMod, NativeZoom,
};
use settings::{FPS, KEYSTROKES, MINIMAP, ZOOM};
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

/// The built-in mod, as `event_horizon::builtin_mods` installs it.
pub fn mods() -> Vec<Box<dyn Hooks>> {
    vec![Box::new(Extras::default())]
}

/// The four features as Options > Mods lists them, in the Mods page's order.
const FEATURES: [(&str, &str, &str); 4] = [
    (
        ZOOM,
        "Dynamic FOV Zoom 1.0",
        "Smooth logarithmic FOV zoom with camera dampening. Its key is in Controls > Key Binds.",
    ),
    (
        MINIMAP,
        "Voxel Minimap & Radar 1.0",
        "In-game top-down live terrain mapping with an entity radar. Its key is in Controls > Key Binds.",
    ),
    (
        FPS,
        "Performance Telemetry HUD 1.0",
        "Frame time graph, FPS counter and GPU latency on the HUD.",
    ),
    (
        KEYSTROKES,
        "Keystrokes Input Display 1.0",
        "Shows movement, sprint, jump and mouse clicks as they happen.",
    ),
];

/// The four extras, one mod.
#[derive(Default)]
pub struct Extras {
    state: Mutex<State>,
}

/// What the mod keeps between hooks: the features' settings, each loaded
/// once, and the keys and clicks the keystrokes panel shows.
#[derive(Default)]
struct State {
    settings: settings::Settings,
    loaded: [bool; 4],
    held: Held,
}

/// Where a feature's state is kept, by feature.
fn feature_index(id: &str) -> usize {
    match id {
        ZOOM => 0,
        MINIMAP => 1,
        FPS => 2,
        _ => 3,
    }
}

/// The keys and mouse buttons the keystrokes panel shows.
#[derive(Default)]
struct Held {
    /// W, A, S, D, Space: whether each is held.
    keys: [bool; 5],
    left: bool,
    right: bool,
    /// The clicks of the last second, for the clicks-per-second lines.
    clicks: [VecDeque<Instant>; 2],
}

impl Held {
    fn click(&mut self, left: bool) {
        let i = usize::from(!left);
        self.clicks[i].push_back(Instant::now());
        while self.clicks[i]
            .front()
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(1))
        {
            self.clicks[i].pop_front();
        }
    }
    fn per_second(&self, left: bool) -> u32 {
        self.clicks[usize::from(!left)].len() as u32
    }
    /// What the panel shows while nothing is held.
    fn clear(&mut self) {
        *self = Held::default();
    }
}

impl Extras {
    /// A feature's settings, loaded (and migrated from the old
    /// `horizon-extras.txt`) once, then kept in memory.
    fn settings(&self, host: &dyn Host, id: &str) -> settings::Settings {
        let mut state = self.state.lock().unwrap();
        let i = feature_index(id);
        if !state.loaded[i] {
            let s = settings::load_feature(host, id);
            match id {
                ZOOM => state.settings.zoom = s.zoom,
                MINIMAP => state.settings.minimap = s.minimap,
                FPS => state.settings.fps = s.fps,
                _ => state.settings.keystrokes = s.keystrokes,
            }
            state.loaded[i] = true;
        }
        state.settings.clone()
    }
    /// The zoom the `zoom` hook asks for: none while the feature is off or
    /// its key (`key.zoom`) is not held.
    fn zoom_answer(&self, host: &dyn Host) -> Option<NativeZoom> {
        if !host.on(ZOOM) {
            return None;
        }
        let z = self.settings(host, ZOOM).zoom;
        host.key_down("key.zoom").then(|| NativeZoom {
            factor: f32::from(z.level),
            smooth: z.smooth,
            turn: if z.sensitivity {
                1.0 / f32::from(z.level)
            } else {
                1.0
            },
        })
    }
    /// The minimap the `minimap` hook asks for: none while the feature is off.
    fn minimap_answer(&self, host: &dyn Host) -> Option<NativeMinimap> {
        if !host.on(MINIMAP) {
            return None;
        }
        let m = self.settings(host, MINIMAP).minimap;
        Some(NativeMinimap {
            corner: m.corner,
            size: f32::from(m.size),
            blocks_per_pixel: m.zoom,
            rotate: m.rotate,
            round: m.round,
        })
    }
    /// The lines of the keystrokes panel, as the extras' fallback drew them.
    fn keystrokes_lines(&self, host: &dyn Host, cps: bool) -> [String; 4] {
        let playing = host.playing();
        let state = self.state.lock().unwrap();
        let held = &state.held;
        let letter = |held: bool, name: &'static str| {
            if playing && held { name } else { "_" }
        };
        let [w, a, s, d, space] = held.keys;
        let line1 = format!("  {}  ", letter(w, "W"));
        let line2 = format!("{} {} {}", letter(a, "A"), letter(s, "S"), letter(d, "D"));
        let space = if playing && space {
            "[ === ]"
        } else {
            "[ --- ]"
        };
        let (l_down, r_down) = (held.left, held.right);
        let (l_cps, r_cps) = (held.per_second(true), held.per_second(false));
        let line4 = if cps {
            format!(
                "{}:{} {}:{}",
                if l_down { "LMB*" } else { "LMB" },
                l_cps,
                if r_down { "RMB*" } else { "RMB" },
                r_cps
            )
        } else {
            format!(
                "{} {}",
                if l_down { "LMB*" } else { "LMB" },
                if r_down { "RMB*" } else { "RMB" }
            )
        };
        [line1, line2, space.into(), line4]
    }
}

impl Hooks for Extras {
    /// Options > Mods: the four features, each with its own switch and page.
    fn mods(&self, _host: &mut dyn Host, event: &mut horizon_client::mod_api::ModsEvent<'_>) {
        for (id, title, description) in FEATURES {
            event.mods.push(NativeMod {
                id: id.into(),
                title: title.into(),
                description: description.into(),
                // Their keys are the Key Binds rows (`key.zoom`,
                // `key.minimap`) the engine keeps.
                keys: Vec::new(),
            });
        }
    }
    /// A feature's settings page: its buttons, as `caption: value` lines.
    fn settings(
        &self,
        host: &mut dyn Host,
        event: &mut horizon_client::mod_api::SettingsEvent<'_>,
    ) {
        let s = self.settings(host, event.mod_id);
        event.buttons.extend(settings::buttons(&s, event.mod_id));
    }
    /// A click on button `button` of a feature's page: step the setting.
    fn settings_click(
        &self,
        host: &mut dyn Host,
        event: &mut horizon_client::mod_api::SettingsClickEvent<'_>,
    ) {
        let mut s = self.settings(host, event.mod_id);
        if let Some(&opt) = settings::Opt::of(event.mod_id).get(event.button) {
            opt.cycle(&mut s);
            host.set_setting(event.mod_id, opt.stored_key(), &opt.stored_value(&s));
            let mut state = self.state.lock().unwrap();
            match event.mod_id {
                ZOOM => state.settings.zoom = s.zoom,
                MINIMAP => state.settings.minimap = s.minimap,
                FPS => state.settings.fps = s.fps,
                _ => state.settings.keystrokes = s.keystrokes,
            }
        }
    }
    /// The zoom while playing: the factor, easing and mouse scale the
    /// feature's settings give.
    fn zoom(
        &self,
        host: &dyn Host,
        _event: &mut horizon_client::mod_api::ZoomEvent,
    ) -> Option<NativeZoom> {
        self.zoom_answer(host)
    }
    /// The minimap the engine draws for the feature.
    fn minimap(
        &self,
        host: &dyn Host,
        _event: &mut horizon_client::mod_api::MinimapEvent,
    ) -> Option<NativeMinimap> {
        self.minimap_answer(host)
    }
    /// The FPS counter's and keystrokes' panels, after the minimap, in the
    /// HUD layer (the engine hides the layer with F1).
    fn hud(&self, host: &mut dyn Host, event: &mut horizon_client::mod_api::HudEvent<'_>) {
        if host.on(FPS) {
            let f = self.settings(host, FPS).fps;
            if host.corner_shown(f.corner) {
                let (fps, min, low) = host.frame_stats();
                let mut lines = vec![(format!("FPS: {fps}"), 0xFFFFFF)];
                if f.lows {
                    lines.push((format!("Min: {min}"), 0xAAAAAA));
                    lines.push((format!("1% Low: {low}"), 0xAAAAAA));
                }
                draw_lines(host, event.batch, event.corners, f.corner, &lines);
            }
        }
        if host.on(KEYSTROKES) {
            let k = self.settings(host, KEYSTROKES).keystrokes;
            if host.corner_shown(k.corner) {
                let lines: Vec<(String, u32)> = self
                    .keystrokes_lines(host, k.cps)
                    .into_iter()
                    .zip([0xFFFFFF, 0xFFFFFF, 0xCCCCCC, 0xAAAAAA])
                    .collect();
                draw_lines(host, event.batch, event.corners, k.corner, &lines);
            }
        }
    }
    /// A key or mouse button went down or up: remember what the keystrokes
    /// panel shows.
    fn key(&self, _host: &mut dyn Host, event: &mut horizon_client::mod_api::KeyEvent<'_>) {
        let mut state = self.state.lock().unwrap();
        let i = match event.key {
            "key.keyboard.w" => Some(0),
            "key.keyboard.a" => Some(1),
            "key.keyboard.s" => Some(2),
            "key.keyboard.d" => Some(3),
            "key.keyboard.space" => Some(4),
            "key.mouse.left" => {
                state.held.left = event.pressed;
                if event.pressed {
                    state.held.click(true);
                }
                return;
            }
            "key.mouse.right" => {
                state.held.right = event.pressed;
                if event.pressed {
                    state.held.click(false);
                }
                return;
            }
            _ => None,
        };
        if let Some(i) = i {
            state.held.keys[i] = event.pressed;
        }
    }
    /// Once per frame: keys let go behind screens no longer show.
    fn frame(&self, host: &mut dyn Host, _event: &mut horizon_client::mod_api::FrameEvent) {
        if !host.playing() {
            self.state.lock().unwrap().held.clear();
        }
    }
}

/// A box of text lines in a corner, as the extras' HUD panels looked: the
/// widest line plus 4 pixels wide, one 10-pixel line each plus 2, a
/// translucent black box with the lines in their own colours, 2 pixels in.
fn draw_lines(
    host: &dyn Host,
    batch: &mut HudBatch,
    corners: &mut Corners,
    corner: Corner,
    lines: &[(String, u32)],
) {
    if lines.is_empty() {
        return;
    }
    let w = lines
        .iter()
        .map(|(text, _)| host.text_width(text))
        .fold(0.0, f32::max)
        + 4.0;
    let h = 10.0 * lines.len() as f32 + 2.0;
    let (x, y) = corners.place(corner, w, h);
    host.fill(batch, [x, y, w, h], 0x7800_0000);
    for (i, (text, color)) in lines.iter().enumerate() {
        host.text(
            batch,
            text,
            [x + 2.0, y + 2.0 + 10.0 * i as f32],
            *color,
            true,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::mod_api::HudBatch;

    /// A host double: the feature switches, the held keys and the frame
    /// numbers the tests name. What it does not implement panics, as the
    /// trait's defaults do.
    struct TestHost {
        on: bool,
        key_down: bool,
        playing: bool,
        stats: (u32, u32, u32),
        drawn: Mutex<Vec<(f32, f32, f32, f32, u32)>>,
        written: Mutex<Vec<String>>,
        text_width: f32,
    }
    impl Host for TestHost {
        fn on(&self, _id: &str) -> bool {
            self.on
        }
        fn setting(&self, _id: &str, _key: &str) -> Option<String> {
            None
        }
        fn set_setting(&self, _id: &str, _key: &str, _value: &str) {}
        fn key_down(&self, _mapping: &str) -> bool {
            self.key_down
        }
        fn playing(&self) -> bool {
            self.playing
        }
        fn frame_stats(&self) -> (u32, u32, u32) {
            self.stats
        }
        fn corner_shown(&self, _corner: Corner) -> bool {
            true
        }
        fn text_width(&self, text: &str) -> f32 {
            self.text_width * text.chars().count() as f32
        }
        fn fill(&self, _batch: &mut HudBatch, rect: [f32; 4], argb: u32) {
            self.drawn
                .lock()
                .unwrap()
                .push((rect[0], rect[1], rect[2], rect[3], argb));
        }
        fn text(&self, _batch: &mut HudBatch, text: &str, at: [f32; 2], _rgb: u32, _shadow: bool) {
            self.written
                .lock()
                .unwrap()
                .push(format!("{text}@{:?},{:?}", at[0], at[1]));
        }
    }

    fn host() -> TestHost {
        TestHost {
            on: true,
            key_down: false,
            playing: true,
            stats: (60, 30, 55),
            drawn: Mutex::new(Vec::new()),
            written: Mutex::new(Vec::new()),
            text_width: 10.0,
        }
    }

    /// The events are `#[non_exhaustive]`, so the hooks' logic is tested
    /// through the mod's own methods and the panels' drawing.
    #[test]
    fn the_zoom_answers_while_its_key_is_held() {
        let mod_ = Extras::default();
        let mut host = host();
        assert_eq!(mod_.zoom_answer(&host), None, "the key is not held");
        host.key_down = true;
        assert_eq!(
            mod_.zoom_answer(&host),
            Some(NativeZoom {
                factor: 4.0,
                smooth: true,
                turn: 0.25,
            })
        );
        host.on = false;
        assert_eq!(mod_.zoom_answer(&host), None, "the feature is off");
    }

    #[test]
    fn the_minimap_answers_with_its_own_settings() {
        let mod_ = Extras::default();
        let host = host();
        assert_eq!(
            mod_.minimap_answer(&host),
            Some(NativeMinimap {
                corner: Corner::TopRight,
                size: 96.0,
                blocks_per_pixel: 1.0,
                rotate: false,
                round: false,
            })
        );
        let host = TestHost { on: false, ..host };
        assert_eq!(mod_.minimap_answer(&host), None, "the feature is off");
    }

    #[test]
    fn the_panels_draw_the_counter_and_the_keys_as_before() {
        let host = host();
        let mut batch = HudBatch::new([400, 300]);
        let mut corners = Corners::new(400.0, 300.0, 25.0);
        draw_lines(&host, &mut batch, &mut corners, Corner::TopLeft, &[]);
        assert!(host.drawn.lock().unwrap().is_empty(), "no lines, no box");
        // A panel: the box is the widest line plus 4, two pixels in.
        let lines = [(String::from("FPS: 60"), 0xFFFFFF)];
        draw_lines(&host, &mut batch, &mut corners, Corner::TopLeft, &lines);
        assert_eq!(
            host.drawn.lock().unwrap().as_slice(),
            &[(2.0, 2.0, 10.0 * 7.0 + 4.0, 12.0, 0x7800_0000)]
        );
        assert_eq!(host.written.lock().unwrap().as_slice(), ["FPS: 60@4.0,4.0"]);
        // A second panel in the same corner stacks below the first.
        let lines = [(String::from("Keystrokes"), 0xFFFFFF)];
        draw_lines(&host, &mut batch, &mut corners, Corner::TopLeft, &lines);
        assert_eq!(
            host.drawn.lock().unwrap().as_slice()[1],
            (2.0, 16.0, 10.0 * 10.0 + 4.0, 12.0, 0x7800_0000)
        );
    }

    #[test]
    fn the_keystrokes_panel_names_each_key_and_the_clicks() {
        let mod_ = Extras::default();
        let host = host();
        {
            let mut state = mod_.state.lock().unwrap();
            state.held.keys = [true, false, false, true, true];
            state.held.left = true;
        }
        assert_eq!(
            mod_.keystrokes_lines(&host, true),
            ["  W  ", "_ _ D", "[ === ]", "LMB*:0 RMB:0",]
        );
        // Behind a screen the keys show as unheld; the mouse button stays
        // until the `frame` hook clears it, as the engine's own code did.
        let host = TestHost {
            playing: false,
            ..host
        };
        assert_eq!(
            mod_.keystrokes_lines(&host, false),
            ["  _  ", "_ _ _", "[ --- ]", "LMB* RMB"]
        );
    }

    #[test]
    fn clicks_count_the_last_second_and_clear_behind_screens() {
        let mut held = Held::default();
        held.click(true);
        held.click(true);
        assert_eq!(held.per_second(true), 2);
        held.click(false);
        assert_eq!(held.per_second(false), 1);
        held.clear();
        assert_eq!(held.per_second(true), 0);
    }
}
