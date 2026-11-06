//! The four features' settings, as the built-in extras mod keeps them
//! (`crates/event-horizon-extras`, issue #29): zoom, the minimap, the FPS
//! counter and keystrokes. Each feature's values live in its own mod data
//! folder (`Host::setting`/`set_setting`, one file per key under
//! `.horizon-mod-data/<id>/`), and on first use they are migrated from the
//! old `horizon-extras.txt` the engine kept, read once with the same rules
//! its parser had. The features' switches are the `mod.<id>` lines of
//! `horizon-mods.txt` (`Host::on`), which the engine seeds from that file.
use horizon_client::{
    extras::{Corner, MINIMAP_SIZES, MINIMAP_ZOOMS},
    lang,
    mod_api::Host,
};

/// The zoom feature's id, its settings folder and its Options > Mods line.
pub const ZOOM: &str = "zoom";
/// The minimap feature's id.
pub const MINIMAP: &str = "minimap";
/// The FPS counter feature's id.
pub const FPS: &str = "fps_counter";
/// The keystrokes feature's id.
pub const KEYSTROKES: &str = "keystrokes";

/// Every feature's settings, as the mod holds them in memory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub zoom: Zoom,
    pub minimap: Minimap,
    pub fps: Fps,
    pub keystrokes: Keystrokes,
}

/// The zoom's settings (`zoomLevel`, `zoomSmooth`, `zoomSensitivity`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    /// How many times the key magnifies, 2 to 10.
    pub level: u8,
    /// Ease through vanilla's FOV easing, as the spyglass does.
    pub smooth: bool,
    /// Divide mouse turning by the zoom.
    pub sensitivity: bool,
}
/// The minimap's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Minimap {
    pub corner: Corner,
    /// Side in GUI pixels (`MINIMAP_SIZES`).
    pub size: u8,
    /// Round instead of square.
    pub round: bool,
    /// Blocks per pixel (`MINIMAP_ZOOMS`).
    pub zoom: f32,
    /// Turn with the player (forward up) instead of north up.
    pub rotate: bool,
}
/// The FPS counter's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fps {
    pub corner: Corner,
    /// Also show the minimum and the 1% low.
    pub lows: bool,
}
/// The keystrokes' settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keystrokes {
    pub corner: Corner,
    /// Clicks per second under the mouse buttons.
    pub cps: bool,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            level: 4,
            smooth: true,
            sensitivity: true,
        }
    }
}
impl Default for Minimap {
    fn default() -> Self {
        Self {
            corner: Corner::TopRight,
            size: 96,
            round: false,
            zoom: 1.0,
            rotate: false,
        }
    }
}
impl Default for Fps {
    fn default() -> Self {
        Self {
            corner: Corner::TopLeft,
            lows: false,
        }
    }
}
impl Default for Keystrokes {
    fn default() -> Self {
        Self {
            corner: Corner::TopLeft,
            cps: true,
        }
    }
}

/// One button on a feature's settings page (Options > Mods): the value it
/// shows, and what a click changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opt {
    ZoomSmooth,
    ZoomSensitivity,
    ZoomLevel,
    MinimapCorner,
    MinimapSize,
    MinimapShape,
    MinimapZoom,
    MinimapRotate,
    FpsCorner,
    FpsLows,
    KeystrokesCorner,
    KeystrokesCps,
}

impl Opt {
    /// The feature (its mod id) the setting belongs to.
    pub fn feature(self) -> &'static str {
        match self {
            Opt::ZoomSmooth | Opt::ZoomSensitivity | Opt::ZoomLevel => ZOOM,
            Opt::MinimapCorner
            | Opt::MinimapSize
            | Opt::MinimapShape
            | Opt::MinimapZoom
            | Opt::MinimapRotate => MINIMAP,
            Opt::FpsCorner | Opt::FpsLows => FPS,
            Opt::KeystrokesCorner | Opt::KeystrokesCps => KEYSTROKES,
        }
    }
    /// The feature's buttons, in the order its page shows them.
    pub fn of(feature: &str) -> &'static [Opt] {
        match feature {
            ZOOM => &[Opt::ZoomSmooth, Opt::ZoomSensitivity, Opt::ZoomLevel],
            MINIMAP => &[
                Opt::MinimapCorner,
                Opt::MinimapSize,
                Opt::MinimapShape,
                Opt::MinimapZoom,
                Opt::MinimapRotate,
            ],
            FPS => &[Opt::FpsCorner, Opt::FpsLows],
            KEYSTROKES => &[Opt::KeystrokesCorner, Opt::KeystrokesCps],
            _ => &[],
        }
    }
    /// The language key of the caption (`horizon.*`, assets/horizon/lang).
    pub fn caption_key(self) -> &'static str {
        match self {
            Opt::ZoomSmooth => "horizon.extras.zoom_smooth",
            Opt::ZoomSensitivity => "horizon.extras.zoom_sensitivity",
            Opt::ZoomLevel => "horizon.extras.zoom_level",
            Opt::MinimapCorner => "horizon.extras.minimap_corner",
            Opt::MinimapSize => "horizon.extras.minimap_size",
            Opt::MinimapShape => "horizon.extras.minimap_shape",
            Opt::MinimapZoom => "horizon.extras.minimap_zoom",
            Opt::MinimapRotate => "horizon.extras.minimap_rotate",
            Opt::FpsCorner => "horizon.extras.fps_corner",
            Opt::FpsLows => "horizon.extras.fps_lows",
            Opt::KeystrokesCorner => "horizon.extras.keystrokes_corner",
            Opt::KeystrokesCps => "horizon.extras.keystrokes_cps",
        }
    }
    /// The English caption: the fallback of the caption in the current
    /// language, and the text of the key in `en_us.json`.
    pub fn caption(self) -> &'static str {
        match self {
            Opt::ZoomSmooth => "Smooth zoom",
            Opt::ZoomSensitivity => "Dampen mouse",
            Opt::ZoomLevel => "Zoom level",
            Opt::MinimapCorner => "Minimap corner",
            Opt::MinimapSize => "Minimap size",
            Opt::MinimapShape => "Minimap shape",
            Opt::MinimapZoom => "Minimap zoom",
            Opt::MinimapRotate => "Rotate with player",
            Opt::FpsCorner => "FPS corner",
            Opt::FpsLows => "FPS minimum & 1% low",
            Opt::KeystrokesCorner => "Keystrokes corner",
            Opt::KeystrokesCps => "Mouse CPS",
        }
    }
    /// The caption in the current language (English when the key is missing).
    pub fn caption_text(self) -> String {
        lang::get_or(self.caption_key(), self.caption())
    }
    /// The value the button shows, as the extras page showed it.
    pub fn value(self, s: &Settings) -> String {
        let on_off =
            |on: bool| lang::get_or(if on { "On" } else { "Off" }, if on { "On" } else { "Off" });
        match self {
            Opt::ZoomSmooth => on_off(s.zoom.smooth),
            Opt::ZoomSensitivity => on_off(s.zoom.sensitivity),
            Opt::ZoomLevel => format!("{}x", s.zoom.level),
            Opt::MinimapCorner => s.minimap.corner.label().into(),
            Opt::MinimapSize => MINIMAP_SIZES
                .iter()
                .find(|t| t.0 == s.minimap.size)
                .map_or("Custom", |t| t.1)
                .into(),
            Opt::MinimapShape => {
                if s.minimap.round {
                    "Round".into()
                } else {
                    "Square".into()
                }
            }
            Opt::MinimapZoom => format!("{} b/px", s.minimap.zoom),
            Opt::MinimapRotate => on_off(s.minimap.rotate),
            Opt::FpsCorner => s.fps.corner.label().into(),
            Opt::FpsLows => on_off(s.fps.lows),
            Opt::KeystrokesCorner => s.keystrokes.corner.label().into(),
            Opt::KeystrokesCps => on_off(s.keystrokes.cps),
        }
    }
    /// A click: step the setting to its next value.
    pub fn cycle(self, s: &mut Settings) {
        match self {
            Opt::ZoomSmooth => s.zoom.smooth = !s.zoom.smooth,
            Opt::ZoomSensitivity => s.zoom.sensitivity = !s.zoom.sensitivity,
            Opt::ZoomLevel => {
                s.zoom.level = if s.zoom.level >= 10 {
                    2
                } else {
                    s.zoom.level + 1
                }
            }
            Opt::MinimapCorner => s.minimap.corner = next_corner(s.minimap.corner),
            Opt::MinimapSize => {
                let idx = MINIMAP_SIZES
                    .iter()
                    .position(|t| t.0 == s.minimap.size)
                    .unwrap_or(1);
                s.minimap.size = MINIMAP_SIZES[(idx + 1) % MINIMAP_SIZES.len()].0;
            }
            Opt::MinimapShape => s.minimap.round = !s.minimap.round,
            Opt::MinimapZoom => {
                let idx = MINIMAP_ZOOMS
                    .iter()
                    .position(|&z| (z - s.minimap.zoom).abs() < 0.01)
                    .unwrap_or(1);
                s.minimap.zoom = MINIMAP_ZOOMS[(idx + 1) % MINIMAP_ZOOMS.len()];
            }
            Opt::MinimapRotate => s.minimap.rotate = !s.minimap.rotate,
            Opt::FpsCorner => s.fps.corner = next_corner(s.fps.corner),
            Opt::FpsLows => s.fps.lows = !s.fps.lows,
            Opt::KeystrokesCorner => s.keystrokes.corner = next_corner(s.keystrokes.corner),
            Opt::KeystrokesCps => s.keystrokes.cps = !s.keystrokes.cps,
        }
    }
    /// The mod data key the setting is kept in.
    pub fn stored_key(self) -> &'static str {
        match self {
            Opt::ZoomSmooth => "smooth",
            Opt::ZoomSensitivity => "sensitivity",
            Opt::ZoomLevel => "level",
            Opt::MinimapCorner | Opt::FpsCorner | Opt::KeystrokesCorner => "corner",
            Opt::MinimapSize => "size",
            Opt::MinimapShape => "shape",
            Opt::MinimapZoom => "zoom",
            Opt::MinimapRotate => "rotate",
            Opt::FpsLows => "lows",
            Opt::KeystrokesCps => "cps",
        }
    }
    /// The setting as its mod data value, for `Host::set_setting`.
    pub fn stored_value(self, s: &Settings) -> String {
        match self {
            Opt::ZoomSmooth => bool_text(s.zoom.smooth).into(),
            Opt::ZoomSensitivity => bool_text(s.zoom.sensitivity).into(),
            Opt::ZoomLevel => s.zoom.level.to_string(),
            Opt::MinimapCorner => s.minimap.corner.name().into(),
            Opt::MinimapSize => s.minimap.size.to_string(),
            Opt::MinimapShape => if s.minimap.round { "round" } else { "square" }.into(),
            Opt::MinimapZoom => s.minimap.zoom.to_string(),
            Opt::MinimapRotate => bool_text(s.minimap.rotate).into(),
            Opt::FpsCorner => s.fps.corner.name().into(),
            Opt::FpsLows => bool_text(s.fps.lows).into(),
            Opt::KeystrokesCorner => s.keystrokes.corner.name().into(),
            Opt::KeystrokesCps => bool_text(s.keystrokes.cps).into(),
        }
    }
}

/// The next corner, as a click on a corner picker steps it.
fn next_corner(c: Corner) -> Corner {
    let all = Corner::ALL;
    let idx = all.iter().position(|&x| x == c).unwrap_or(0);
    all[(idx + 1) % all.len()]
}

fn bool_text(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

/// A feature's settings, from its mod data folder, migrating the old
/// `horizon-extras.txt` on first use (no `migrated` marker yet).
pub fn load_feature(host: &dyn Host, id: &str) -> Settings {
    load_feature_with(host, id, &OldExtras::load())
}

/// [`load_feature`] with the old settings named: the file is read once per
/// feature, so the tests hand over what it said.
pub fn load_feature_with(host: &dyn Host, id: &str, old: &OldExtras) -> Settings {
    let mut s = Settings::default();
    if host.setting(id, "migrated").is_none() {
        match id {
            ZOOM => {
                s.zoom = Zoom {
                    level: old.zoom_level,
                    smooth: old.zoom_smooth,
                    sensitivity: old.zoom_sensitivity,
                }
            }
            MINIMAP => {
                s.minimap = Minimap {
                    corner: old.minimap_corner,
                    size: old.minimap_size,
                    round: old.minimap_round,
                    zoom: old.minimap_zoom,
                    rotate: old.minimap_rotate,
                }
            }
            FPS => {
                s.fps = Fps {
                    corner: old.fps_corner,
                    lows: old.fps_lows,
                }
            }
            _ => {
                s.keystrokes = Keystrokes {
                    corner: old.keystrokes_corner,
                    cps: old.keystrokes_cps,
                }
            }
        }
        store_feature(host, id, &s);
        host.set_setting(id, "migrated", "1");
        return s;
    }
    let corner = |v: Option<String>, default| {
        v.as_deref()
            .map_or(default, |name| Corner::parse(name, default))
    };
    match id {
        ZOOM => {
            s.zoom = Zoom {
                level: host
                    .setting(id, "level")
                    .and_then(|v| v.parse::<u8>().ok())
                    .map(|v| v.clamp(2, 10))
                    .unwrap_or(s.zoom.level),
                smooth: host
                    .setting(id, "smooth")
                    .map_or(s.zoom.smooth, |v| v == "true"),
                sensitivity: host
                    .setting(id, "sensitivity")
                    .map_or(s.zoom.sensitivity, |v| v == "true"),
            }
        }
        MINIMAP => {
            let size = host
                .setting(id, "size")
                .and_then(|v| MINIMAP_SIZES.iter().find(|t| t.0.to_string() == v).copied())
                .map_or(s.minimap.size, |(size, _)| size);
            s.minimap = Minimap {
                corner: corner(host.setting(id, "corner"), s.minimap.corner),
                size,
                round: host.setting(id, "shape").is_some_and(|v| v == "round"),
                zoom: host
                    .setting(id, "zoom")
                    .and_then(|v| MINIMAP_ZOOMS.iter().find(|z| z.to_string() == v).copied())
                    .unwrap_or(s.minimap.zoom),
                rotate: host
                    .setting(id, "rotate")
                    .map_or(s.minimap.rotate, |v| v == "true"),
            }
        }
        FPS => {
            s.fps = Fps {
                corner: corner(host.setting(id, "corner"), s.fps.corner),
                lows: host.setting(id, "lows").map_or(s.fps.lows, |v| v == "true"),
            }
        }
        _ => {
            s.keystrokes = Keystrokes {
                corner: corner(host.setting(id, "corner"), s.keystrokes.corner),
                cps: host
                    .setting(id, "cps")
                    .map_or(s.keystrokes.cps, |v| v == "true"),
            }
        }
    }
    s
}

/// Writes a feature's settings to its mod data folder.
fn store_feature(host: &dyn Host, id: &str, s: &Settings) {
    for opt in Opt::of(id) {
        host.set_setting(id, opt.stored_key(), &opt.stored_value(s));
    }
}

/// One feature's settings page (the buttons the `settings` hook pushes):
/// each a `caption: value` line, in `Opt::of` order.
pub fn buttons(s: &Settings, feature: &str) -> Vec<String> {
    Opt::of(feature)
        .iter()
        .map(|&opt| format!("{}: {}", opt.caption_text(), opt.value(s)))
        .collect()
}

/// The old `horizon-extras.txt` as the engine kept it, for the one-time
/// move of the four features' values into the mod's own settings: the same
/// lines, the same rules its parser had. The file stays as it is.
#[derive(Clone, Debug, PartialEq)]
pub struct OldExtras {
    pub zoom_level: u8,
    pub zoom_smooth: bool,
    pub zoom_sensitivity: bool,
    pub minimap_corner: Corner,
    pub minimap_size: u8,
    pub minimap_round: bool,
    pub minimap_zoom: f32,
    pub minimap_rotate: bool,
    pub fps_corner: Corner,
    pub fps_lows: bool,
    pub keystrokes_corner: Corner,
    pub keystrokes_cps: bool,
}

impl Default for OldExtras {
    /// The defaults the engine's `Extras::default` had for these settings.
    fn default() -> Self {
        Self::defaults()
    }
}

impl OldExtras {
    /// The defaults the engine's `Extras::default` had for these settings.
    fn defaults() -> Self {
        Self {
            zoom_level: 4,
            zoom_smooth: true,
            zoom_sensitivity: true,
            minimap_corner: Corner::TopRight,
            minimap_size: 96,
            minimap_round: false,
            minimap_zoom: 1.0,
            minimap_rotate: false,
            fps_corner: Corner::TopLeft,
            fps_lows: false,
            keystrokes_corner: Corner::TopLeft,
            keystrokes_cps: true,
        }
    }
    /// The file beside `options.txt`, read once on the first use.
    fn load() -> Self {
        let path = horizon_client::logging::player_dir().join(horizon_client::extras::FILE);
        std::fs::read_to_string(path)
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }
    /// Parses the file as the engine's `Extras::parse` did: `key:value`
    /// lines, unknown or broken values keep the defaults.
    pub fn parse(text: &str) -> Self {
        let mut e = Self::defaults();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let flag = |d: bool| match value {
                "true" => true,
                "false" => false,
                _ => d,
            };
            match key.trim() {
                "zoomLevel" => {
                    if let Ok(v) = value.parse::<u8>() {
                        e.zoom_level = v.clamp(2, 10);
                    }
                }
                "zoomSmooth" => e.zoom_smooth = flag(e.zoom_smooth),
                "zoomSensitivity" => e.zoom_sensitivity = flag(e.zoom_sensitivity),
                "minimapCorner" => e.minimap_corner = Corner::parse(value, e.minimap_corner),
                "minimapSize" => {
                    if let Some((v, _)) = MINIMAP_SIZES.iter().find(|t| t.0.to_string() == value) {
                        e.minimap_size = *v;
                    }
                }
                "minimapShape" => e.minimap_round = value == "round",
                "minimapZoom" => {
                    if let Some(&z) = MINIMAP_ZOOMS.iter().find(|z| z.to_string() == value) {
                        e.minimap_zoom = z;
                    }
                }
                "minimapRotate" => e.minimap_rotate = flag(e.minimap_rotate),
                "fpsCorner" => e.fps_corner = Corner::parse(value, e.fps_corner),
                "fpsLows" => e.fps_lows = flag(e.fps_lows),
                "keystrokesCorner" => {
                    e.keystrokes_corner = Corner::parse(value, e.keystrokes_corner);
                }
                "keystrokesCps" => e.keystrokes_cps = flag(e.keystrokes_cps),
                _ => {}
            }
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four features' settings as a `Settings`, for the tests.
    fn settings_of(s: &OldExtras) -> Settings {
        Settings {
            zoom: Zoom {
                level: s.zoom_level,
                smooth: s.zoom_smooth,
                sensitivity: s.zoom_sensitivity,
            },
            minimap: Minimap {
                corner: s.minimap_corner,
                size: s.minimap_size,
                round: s.minimap_round,
                zoom: s.minimap_zoom,
                rotate: s.minimap_rotate,
            },
            fps: Fps {
                corner: s.fps_corner,
                lows: s.fps_lows,
            },
            keystrokes: Keystrokes {
                corner: s.keystrokes_corner,
                cps: s.keystrokes_cps,
            },
        }
    }

    #[test]
    fn the_old_file_parses_as_the_engine_did() {
        // The defaults match the engine's `Extras::default` for these keys.
        assert_eq!(OldExtras::parse(""), OldExtras::defaults());
        let old = OldExtras::parse(
            "zoomLevel:6\nzoomSmooth:false\nzoomSensitivity:maybe\nminimapCorner:bottom_left\nminimapSize:128\nminimapShape:round\nminimapZoom:0.5\nminimapRotate:true\nfpsCorner:bottom_right\nfpsLows:true\nkeystrokesCorner:top_right\nkeystrokesCps:false\n",
        );
        assert_eq!(
            settings_of(&old),
            Settings {
                zoom: Zoom {
                    level: 6,
                    smooth: false,
                    sensitivity: true,
                },
                minimap: Minimap {
                    corner: Corner::BottomLeft,
                    size: 128,
                    round: true,
                    zoom: 0.5,
                    rotate: true,
                },
                fps: Fps {
                    corner: Corner::BottomRight,
                    lows: true,
                },
                keystrokes: Keystrokes {
                    corner: Corner::TopRight,
                    cps: false,
                },
            }
        );
        // Out-of-range values clamp as the engine's parser did: a later
        // line wins, so 1 stays inside 2 to 10 as 2.
        let old = OldExtras::parse(
            "zoomLevel:99\nzoomLevel:1\nminimapSize:7\nminimapZoom:3\nfpsCorner:middle\nnoise\n",
        );
        assert_eq!(old.zoom_level, 2);
        assert_eq!(old.minimap_size, 96);
        assert_eq!(old.minimap_zoom, 1.0);
        assert_eq!(old.fps_corner, Corner::TopLeft);
    }

    #[test]
    fn every_setting_steps_through_its_values_and_shows_its_old_caption() {
        let mut s = Settings::default();
        assert_eq!(
            Opt::ZoomLevel.value(&s),
            format!("{}x", s.zoom.level),
            "the captions come from the lang keys"
        );
        assert_eq!(Opt::ZoomSmooth.caption(), "Smooth zoom");
        assert_eq!(Opt::MinimapZoom.value(&s), "1 b/px");
        assert_eq!(Opt::FpsLows.caption_text(), "FPS minimum & 1% low");
        for (opt, expect) in [
            (Opt::ZoomLevel, "Zoom level: 5x"),
            (Opt::ZoomSmooth, "Smooth zoom: Off"),
            (Opt::ZoomSensitivity, "Dampen mouse: Off"),
            (Opt::MinimapCorner, "Minimap corner: Bottom left"),
            (Opt::MinimapSize, "Minimap size: Large"),
            (Opt::MinimapShape, "Minimap shape: Round"),
            (Opt::MinimapZoom, "Minimap zoom: 2 b/px"),
            (Opt::MinimapRotate, "Rotate with player: On"),
            (Opt::FpsCorner, "FPS corner: Top right"),
            (Opt::FpsLows, "FPS minimum & 1% low: On"),
            (Opt::KeystrokesCorner, "Keystrokes corner: Top right"),
            (Opt::KeystrokesCps, "Mouse CPS: Off"),
        ] {
            opt.cycle(&mut s);
            assert_eq!(
                format!("{}: {}", opt.caption_text(), opt.value(&s)),
                expect,
                "{opt:?}"
            );
        }
        // A corner wraps around, and the zoom level stays inside 2 to 10.
        for _ in 0..3 {
            Opt::MinimapCorner.cycle(&mut s);
        }
        assert_eq!(s.minimap.corner, Corner::TopRight);
        for _ in 0..5 {
            Opt::ZoomLevel.cycle(&mut s);
        }
        assert_eq!(s.zoom.level, 10);
        Opt::ZoomLevel.cycle(&mut s);
        assert_eq!(s.zoom.level, 2);
    }

    /// A host double over a folder of mod data, as the settings tests use.
    struct Files {
        dir: std::path::PathBuf,
    }
    impl Host for Files {
        fn setting(&self, id: &str, key: &str) -> Option<String> {
            std::fs::read_to_string(self.dir.join(id).join(key))
                .ok()
                .map(|v| v.trim().to_owned())
        }
        fn set_setting(&self, id: &str, key: &str, value: &str) {
            let dir = self.dir.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(key), value).unwrap();
        }
        fn on(&self, id: &str) -> bool {
            !id.contains("off")
        }
    }

    #[test]
    fn settings_carry_over_from_the_old_file_once_and_then_stay() {
        let dir = std::env::temp_dir().join("event-horizon-extras-test");
        let _ = std::fs::remove_dir_all(&dir);
        let host = Files { dir: dir.clone() };
        // The old file decides the first load's values, and they are kept,
        // so the next load reads them from the mod's own settings.
        let old = OldExtras {
            zoom_level: 7,
            zoom_smooth: false,
            ..OldExtras::defaults()
        };
        let s = load_feature_with(&host, ZOOM, &old);
        assert_eq!(
            s.zoom,
            Zoom {
                level: 7,
                smooth: false,
                sensitivity: true
            }
        );
        let s = load_feature_with(&host, ZOOM, &old);
        assert_eq!(s.zoom.level, 7);
        assert!(!s.zoom.smooth);
        // A click steps a setting and keeps it.
        let mut s = load_feature_with(&host, ZOOM, &old);
        Opt::ZoomLevel.cycle(&mut s);
        host.set_setting(ZOOM, "level", &s.zoom.level.to_string());
        assert_eq!(
            host.setting(ZOOM, "level").as_deref(),
            Some("8"),
            "kept as its own setting"
        );
        assert_eq!(load_feature_with(&host, ZOOM, &old).zoom.level, 8);
        // The buttons and the clicks agree on the order.
        let fps = load_feature_with(&host, FPS, &OldExtras::defaults());
        assert_eq!(
            buttons(&fps, FPS)[0],
            format!("FPS corner: {}", Corner::TopLeft.label())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
