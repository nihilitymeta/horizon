//! World markers a native client mod creates once and the engine keeps drawing
//! (docs/native-mods.md, "World markers"): `native_marker_set`,
//! `native_marker_move`, `native_marker_remove` and `native_marker_clear_owner`
//! keep them by `(mod id, marker id)` in [`Markers`], and each frame
//! [`Game::world_primitive_geometry`] appends them to the frame's primitives
//! before it asks the mods' `world_primitives` hook. Unlike `native_visuals`'
//! rims and lights, a marker is not re-pushed every frame: a mod creates it
//! once, and its lifetime, if any, runs on the world's game time. A marker
//! belongs to the level it was set in and is drawn only there; opening or
//! closing a world drops every marker.
use super::Game;
use horizon_client::mod_api::{NativeMarker, NativeMarkerShape};
use horizon_client::world_primitives::WorldPrimitives;
use std::collections::HashMap;

/// What one mod may keep at once, whatever other mods keep: its first
/// `MAX_MARKERS_PER_OWNER` markers; replacing one of them does not count twice.
const MAX_MARKERS_PER_OWNER: usize = 256;
/// Every owner's markers together: what the registry holds at once, so a mod
/// cannot store marker after marker by naming a new owner each time.
const MAX_MARKERS_TOTAL: usize = 4096;
/// A marker id's and an owner id's longest, in characters.
const MAX_ID_CHARS: usize = 64;
const MAX_OWNER_CHARS: usize = 64;
/// A marker's text's longest, in characters.
const MAX_TEXT_CHARS: usize = 64;
/// Every coordinate and size a marker gives must be finite and within this far
/// from the origin.
const MAX_COORD: f64 = 30_000_000.0;
/// A box's longest side, in blocks.
const MAX_BOX_SIDE: f64 = 256.0;
/// A line's longest offset, in blocks.
const MAX_LINE_OFFSET: f64 = 512.0;
/// A beam's tallest height, in blocks.
const MAX_BEAM_HEIGHT: f64 = 512.0;
/// Text's smallest and largest scale: a name tag's size is 1.
const MIN_TEXT_SCALE: f32 = 0.25;
const MAX_TEXT_SCALE: f32 = 8.0;
/// A marker icon's smallest and largest size, in blocks across: 1 is a block.
const MIN_ICON_SIZE: f32 = 0.25;
const MAX_ICON_SIZE: f32 = 16.0;
/// An icon's sprite name's longest, in characters.
const MAX_SPRITE_CHARS: usize = 64;
/// A lifetime's longest, in ticks (an hour of game time).
const MAX_TTL_TICKS: u32 = 72_000;
/// A marker farther than this from the camera is not drawn this frame; it is
/// kept, and drawn again when the camera comes closer.
const MAX_DRAW_DISTANCE: f64 = 256.0;
/// A marker line's and box outline's width in pixels, as a block outline's.
const MARKER_LINE_WIDTH: f32 = 2.0;

/// One kept marker and when it stops being drawn.
struct Entry {
    marker: NativeMarker,
    /// The level key (`minecraft:overworld`) the marker was set in: it is drawn
    /// in that level and no other, so a marker of the overworld does not show
    /// at the same coordinates in the Nether.
    level: String,
    /// The first tick it is no longer drawn (`tick + ttl_ticks`); None never.
    expires_at: Option<u64>,
}

/// Every mod's kept markers, by `(mod id, marker id)`.
#[derive(Default)]
pub(super) struct Markers {
    entries: HashMap<(String, String), Entry>,
}

impl Markers {
    /// Creates or replaces the marker `id` of `owner` in the level `level`,
    /// which lasts `ttl_ticks` from `now` when it has a lifetime. False
    /// (`marker` untouched) when the marker is invalid, when `owner` or `id`
    /// is longer than 64 characters, or when a marker the registry does not
    /// hold yet would be over `MAX_MARKERS_PER_OWNER` for the owner or
    /// `MAX_MARKERS_TOTAL` for every owner; replacing one it holds never
    /// counts.
    pub(super) fn set(
        &mut self,
        owner: &str,
        id: &str,
        marker: &NativeMarker,
        now: u64,
        level: &str,
    ) -> bool {
        if owner.is_empty() || owner.chars().count() > MAX_OWNER_CHARS || !valid(id, marker) {
            return false;
        }
        let key = (owner.to_owned(), id.to_owned());
        if !self.entries.contains_key(&key) {
            let mine = self.entries.keys().filter(|(o, _)| o == owner).count();
            if mine >= MAX_MARKERS_PER_OWNER || self.entries.len() >= MAX_MARKERS_TOTAL {
                return false;
            }
        }
        let expires_at = marker
            .ttl_ticks
            .map(|ttl| now.saturating_add(u64::from(ttl)));
        self.entries.insert(
            key,
            Entry {
                marker: marker.clone(),
                level: level.to_owned(),
                expires_at,
            },
        );
        true
    }

    /// Moves the marker `id` of `owner` to `pos`, keeping everything else
    /// (including the lifetime it has left and the level it was set in). False
    /// when the owner keeps no such marker, or `pos` is invalid: outside the
    /// coordinate bounds, or so far out that the marker's own far point would
    /// leave them.
    pub(super) fn move_to(&mut self, owner: &str, id: &str, pos: [f64; 3]) -> bool {
        if !pos.iter().all(|v| within(*v)) {
            return false;
        }
        let Some(entry) = self.entries.get_mut(&(owner.to_owned(), id.to_owned())) else {
            return false;
        };
        if !extent_within(pos, &entry.marker.shape) {
            return false;
        }
        entry.marker.pos = pos;
        true
    }

    /// Drops the marker `id` of `owner`; false when the owner keeps no such
    /// marker.
    pub(super) fn remove(&mut self, owner: &str, id: &str) -> bool {
        self.entries
            .remove(&(owner.to_owned(), id.to_owned()))
            .is_some()
    }

    /// Drops every marker `owner` keeps.
    pub(super) fn clear_owner(&mut self, owner: &str) {
        self.entries.retain(|(other, _), _| other != owner);
    }

    /// Drops every marker, whatever mod keeps it and in whatever level.
    pub(super) fn clear_all(&mut self) {
        self.entries.clear();
    }

    /// Drops the markers whose lifetime ended at or before `now`.
    pub(super) fn prune(&mut self, now: u64) {
        self.entries
            .retain(|_, entry| entry.expires_at.is_none_or(|at| now < at));
    }

    /// Adds every marker of the level `level` within `MAX_DRAW_DISTANCE` of
    /// `camera` to `out`, in world coordinates (the caller makes them
    /// camera-relative as it does the mods' primitives), with through-walls
    /// where the marker asks for it. A marker of another level is skipped, not
    /// dropped: it shows again when the window is back in its level.
    pub(super) fn emit(&self, camera: [f64; 3], level: &str, out: &mut WorldPrimitives) {
        for entry in self.entries.values() {
            if entry.level != level {
                continue;
            }
            let marker = &entry.marker;
            let offset = [0, 1, 2].map(|axis| marker.pos[axis] - camera[axis]);
            if offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]
                > MAX_DRAW_DISTANCE * MAX_DRAW_DISTANCE
            {
                continue;
            }
            let pos = marker.pos;
            let corner = |size: [f64; 3]| [pos[0] + size[0], pos[1] + size[1], pos[2] + size[2]];
            let primitive = match &marker.shape {
                NativeMarkerShape::Box { size, filled: true } => {
                    out.filled_box(pos, corner(*size), marker.argb)
                }
                NativeMarkerShape::Box {
                    size,
                    filled: false,
                } => out.box_outline(pos, corner(*size), marker.argb, MARKER_LINE_WIDTH),
                NativeMarkerShape::Line { to } => {
                    out.line(pos, corner(*to), marker.argb, MARKER_LINE_WIDTH)
                }
                NativeMarkerShape::Beam { height } => {
                    out.beam(pos, height.round() as u32, marker.argb)
                }
                NativeMarkerShape::Text { text, scale } => {
                    out.text(pos, text.as_str(), marker.argb, *scale)
                }
                NativeMarkerShape::Icon { sprite, size } => {
                    out.sprite(pos, sprite.as_str(), *size, marker.argb)
                }
            };
            if marker.through_walls {
                primitive.through_walls();
            }
        }
    }

    #[cfg(test)]
    fn get(&self, owner: &str, id: &str) -> Option<&NativeMarker> {
        self.entries
            .get(&(owner.to_owned(), id.to_owned()))
            .map(|entry| &entry.marker)
    }
}

/// A coordinate or size a marker may name: finite and within `MAX_COORD`.
fn within(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_COORD
}

/// Whether the far point a shape reaches from `pos` is within `MAX_COORD` too:
/// a box's maximum corner, a line's end, a beam's top and either half of an
/// icon must stay in the coordinate bounds, not only the anchor every shape
/// shares.
fn extent_within(pos: [f64; 3], shape: &NativeMarkerShape) -> bool {
    match shape {
        NativeMarkerShape::Box { size, .. } => (0..3).all(|axis| within(pos[axis] + size[axis])),
        NativeMarkerShape::Line { to } => (0..3).all(|axis| within(pos[axis] + to[axis])),
        NativeMarkerShape::Beam { height } => within(pos[1] + height),
        NativeMarkerShape::Text { .. } => true,
        NativeMarkerShape::Icon { size, .. } => {
            let half = f64::from(*size) / 2.0;
            (0..3).all(|axis| within(pos[axis] + half) && within(pos[axis] - half))
        }
    }
}

/// Whether `marker` under `id` is a marker the engine can draw.
fn valid(id: &str, marker: &NativeMarker) -> bool {
    let id_ok = !id.is_empty() && id.chars().count() <= MAX_ID_CHARS;
    let pos_ok = marker.pos.iter().all(|v| within(*v)) && extent_within(marker.pos, &marker.shape);
    let shape_ok = match &marker.shape {
        NativeMarkerShape::Box { size, .. } => {
            size.iter()
                .all(|v| v.is_finite() && (0.0..=MAX_BOX_SIDE).contains(v))
                && size.iter().any(|v| *v != 0.0)
        }
        NativeMarkerShape::Line { to } => {
            to.iter().all(|v| within(*v))
                && to.iter().map(|v| v * v).sum::<f64>() <= MAX_LINE_OFFSET * MAX_LINE_OFFSET
        }
        NativeMarkerShape::Beam { height } => {
            height.is_finite() && (0.0..=MAX_BEAM_HEIGHT).contains(height)
        }
        NativeMarkerShape::Text { text, scale } => {
            !text.is_empty()
                && text.chars().count() <= MAX_TEXT_CHARS
                && scale.is_finite()
                && (MIN_TEXT_SCALE..=MAX_TEXT_SCALE).contains(scale)
        }
        NativeMarkerShape::Icon { sprite, size } => {
            valid_sprite(sprite)
                && size.is_finite()
                && (MIN_ICON_SIZE..=MAX_ICON_SIZE).contains(size)
        }
    };
    let ttl_ok = marker
        .ttl_ticks
        .is_none_or(|ttl| (1..=MAX_TTL_TICKS).contains(&ttl));
    id_ok && pos_ok && shape_ok && ttl_ok
}

/// Whether `sprite` can name an atlas sprite: 1 to 64 characters of `a-z`,
/// `0-9`, `_`, `/`, `.` and `-` (`item/diamond`, `block/stone`,
/// `entity/beacon/beacon_beam`). Whether the atlas holds it is the draw path's
/// question, not this one's.
fn valid_sprite(sprite: &str) -> bool {
    !sprite.is_empty()
        && sprite.chars().count() <= MAX_SPRITE_CHARS
        && sprite.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '/' | '.' | '-')
        })
}

// Native mods use these; nothing in Horizon calls them.
impl Game<'_> {
    /// The key of the level the window is in now (`minecraft:overworld`), which
    /// a marker set here belongs to: the overworld's before a server's first
    /// spawn info, as a fresh `Remote` holds.
    pub(super) fn marker_level_key(&self) -> &str {
        self.remote
            .as_ref()
            .map_or("minecraft:overworld", |r| r.level.as_str())
    }

    /// Drops every kept marker: the window's world is opening or closing, and a
    /// marker belongs to the level it was set in. Every path that replaces the
    /// window's world calls this, as the server's own world open and close do.
    pub(super) fn drop_markers_for_new_world(&mut self) {
        self.markers.clear_all();
    }

    /// Creates the world marker `id` of the mod `owner`, or replaces what that
    /// id showed: a box, a line, a beam, text or an atlas icon at a world
    /// position, in ARGB, which the engine draws every frame until the mod
    /// moves or removes it or its lifetime runs out. `id` is the mod's own key,
    /// 1 to 64 characters (`owner` 1 to 64 too); a mod keeps at most 256
    /// markers, every mod together 4096, and replacing one does not count twice.
    /// A lifetime runs on the world's game time, which `/tick freeze` holds. A
    /// marker belongs to the level it was set in and is drawn only there,
    /// including after a move, and opening or closing a world drops them all.
    /// Markers farther than 256 blocks from the camera are not drawn until it
    /// comes closer. False, with the marker the id had left as it was, for an
    /// invalid marker or over the cap; false on a server, which draws none.
    /// API @API:marker_set@.
    pub(super) fn native_marker_set(
        &mut self,
        owner: &str,
        id: &str,
        marker: &NativeMarker,
    ) -> bool {
        if !self.native_window() {
            return false;
        }
        let level = self.marker_level_key().to_owned();
        let now = self.world.game_time.max(0) as u64;
        self.markers.set(owner, id, marker, now, &level)
    }

    /// Moves the world marker `id` of the mod `owner` to `pos`: its anchor
    /// only, so its shape, colour, through-walls setting, the lifetime it has
    /// left and the level it was set in stay. False when that id has no
    /// marker, or `pos` is invalid: outside the game's coordinate bounds
    /// itself, or enough that the marker's far corner, line end, beam top or
    /// icon edge would leave them; false on a server. API @API:marker_move@.
    pub(super) fn native_marker_move(&mut self, owner: &str, id: &str, pos: [f64; 3]) -> bool {
        if !self.native_window() {
            return false;
        }
        self.markers.move_to(owner, id, pos)
    }

    /// Stops drawing the world marker `id` of the mod `owner`. False when that
    /// id has no marker; false on a server. API @API:marker_remove@.
    pub(super) fn native_marker_remove(&mut self, owner: &str, id: &str) -> bool {
        if !self.native_window() {
            return false;
        }
        self.markers.remove(owner, id)
    }

    /// Stops drawing every world marker of the mod `owner`: what a mod turned
    /// off or reloaded leaves behind. Nothing when it keeps none, or on a
    /// server. API @API:marker_clear_owner@.
    pub(super) fn native_marker_clear_owner(&mut self, owner: &str) {
        if !self.native_window() {
            return;
        }
        self.markers.clear_owner(owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::world_primitives::PrimitiveShape;

    const OWNER: &str = "example";
    const OTHER: &str = "other";
    const LEVEL: &str = "minecraft:overworld";
    const NETHER: &str = "minecraft:the_nether";

    fn boxed() -> NativeMarker {
        NativeMarker {
            pos: [10.0, 20.0, 30.0],
            shape: NativeMarkerShape::Box {
                size: [4.0, 5.0, 6.0],
                filled: false,
            },
            argb: 0xFF11_2233,
            through_walls: false,
            ttl_ticks: None,
        }
    }

    fn emitted(
        markers: &Markers,
        camera: [f64; 3],
        level: &str,
    ) -> Vec<horizon_client::world_primitives::WorldPrimitive> {
        let mut out = WorldPrimitives::default();
        markers.emit(camera, level, &mut out);
        out.iter().cloned().collect()
    }

    #[test]
    fn set_replaces_and_move_keeps_everything_but_the_anchor() {
        let mut markers = Markers::default();
        let first = boxed();
        assert!(markers.set(OWNER, "a", &first, 10, LEVEL));
        assert_eq!(markers.get(OWNER, "a"), Some(&first));

        let second = NativeMarker {
            pos: [7.0, 8.0, 9.0],
            shape: NativeMarkerShape::Text {
                text: "hello".to_owned(),
                scale: 2.0,
            },
            argb: 0x8012_3456,
            through_walls: true,
            ttl_ticks: Some(100),
        };
        assert!(markers.set(OWNER, "a", &second, 20, LEVEL));
        assert_eq!(markers.get(OWNER, "a"), Some(&second));

        assert!(markers.move_to(OWNER, "a", [-5.0, 64.0, 12.5]));
        let moved = markers.get(OWNER, "a").expect("still there").clone();
        assert_eq!(moved.pos, [-5.0, 64.0, 12.5]);
        assert_eq!(moved.shape, second.shape);
        assert_eq!(moved.argb, second.argb);
        assert_eq!(moved.through_walls, second.through_walls);
        assert_eq!(moved.ttl_ticks, second.ttl_ticks);

        assert!(!markers.move_to("nobody", "a", [0.0; 3]));
        assert!(!markers.move_to(OWNER, "unknown", [0.0; 3]));
        assert!(markers.remove(OWNER, "a"));
        assert!(!markers.remove(OWNER, "a"));
        assert_eq!(markers.get(OWNER, "a"), None);
    }

    #[test]
    fn owners_are_separate_namespaces() {
        let mut markers = Markers::default();
        let mine = boxed();
        let theirs = NativeMarker {
            pos: [-1.0, -2.0, -3.0],
            shape: NativeMarkerShape::Beam { height: 40.0 },
            argb: 0xFF00_FF00,
            through_walls: false,
            ttl_ticks: None,
        };
        assert!(markers.set(OWNER, "same", &mine, 0, LEVEL));
        assert!(markers.set(OTHER, "same", &theirs, 0, LEVEL));
        assert_eq!(markers.get(OWNER, "same"), Some(&mine));
        assert_eq!(markers.get(OTHER, "same"), Some(&theirs));

        markers.clear_owner(OWNER);
        assert_eq!(markers.get(OWNER, "same"), None);
        assert_eq!(markers.get(OTHER, "same"), Some(&theirs));
    }

    #[test]
    fn a_mod_keeps_at_most_256_markers() {
        let mut markers = Markers::default();
        let marker = boxed();
        for i in 0..MAX_MARKERS_PER_OWNER {
            assert!(
                markers.set(OWNER, &format!("m{i}"), &marker, 0, LEVEL),
                "m{i}"
            );
        }
        assert!(!markers.set(OWNER, "one-too-many", &marker, 0, LEVEL));
        // Replacing an id it already keeps is not a new marker.
        assert!(markers.set(OWNER, "m0", &marker, 0, LEVEL));
        // Another mod has its own budget.
        assert!(markers.set(OTHER, "m0", &marker, 0, LEVEL));
    }

    #[test]
    fn owners_are_capped_and_so_is_the_registry() {
        let mut markers = Markers::default();
        let marker = boxed();
        // An owner is 1 to 64 characters.
        assert!(markers.set(&"o".repeat(MAX_OWNER_CHARS), "a", &marker, 0, LEVEL));
        assert!(!markers.set(&"o".repeat(MAX_OWNER_CHARS + 1), "a", &marker, 0, LEVEL));

        // 200 owners of 21 markers are 4200, but every owner together holds
        // 4096: the first 4096 are kept and the ones past that are refused.
        let mut markers = Markers::default();
        let mut stored = 0;
        let mut refused = None;
        for owner in 0..200 {
            let owner = format!("owner{owner}");
            for i in 0..21 {
                if markers.set(&owner, &format!("m{i}"), &marker, 0, LEVEL) {
                    stored += 1;
                } else {
                    refused.get_or_insert((owner.clone(), i));
                }
            }
        }
        assert_eq!(stored, MAX_MARKERS_TOTAL);
        assert_eq!(markers.entries.len(), MAX_MARKERS_TOTAL);
        assert_eq!(
            refused,
            Some((format!("owner{}", MAX_MARKERS_TOTAL / 21), 1)),
            "4095 markers fit for the first 195 owners, then one more"
        );
        // Replacing a marker it already keeps still works at the cap.
        assert!(markers.set("owner0", "m0", &marker, 0, LEVEL));
        assert_eq!(markers.entries.len(), MAX_MARKERS_TOTAL);

        // Clearing an owner frees its room.
        markers.clear_owner("owner1");
        assert!(markers.set("owner195", "m1", &marker, 0, LEVEL));
    }

    #[test]
    fn invalid_markers_are_refused_and_leave_the_old_one() {
        let mut markers = Markers::default();
        let good = boxed();
        assert!(markers.set(OWNER, "keep", &good, 0, LEVEL));

        let mut check = |marker: NativeMarker| {
            assert!(
                !markers.set(OWNER, "keep", &marker, 0, LEVEL),
                "accepted {marker:?}"
            );
            assert_eq!(markers.get(OWNER, "keep"), Some(&good), "{marker:?}");
        };
        for pos in [
            [f64::NAN, 0.0, 0.0],
            [0.0, f64::INFINITY, 0.0],
            [30_000_001.0, 0.0, 0.0],
            [0.0, -30_000_001.0, 0.0],
        ] {
            check(NativeMarker {
                pos,
                ..good.clone()
            });
        }
        for size in [
            [257.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            [f64::NAN, 1.0, 1.0],
        ] {
            check(NativeMarker {
                shape: NativeMarkerShape::Box { size, filled: true },
                ..good.clone()
            });
        }
        for to in [[513.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0], [1e300, 0.0, 0.0]] {
            check(NativeMarker {
                shape: NativeMarkerShape::Line { to },
                ..good.clone()
            });
        }
        for height in [-0.1, 512.1, f64::NAN] {
            check(NativeMarker {
                shape: NativeMarkerShape::Beam { height },
                ..good.clone()
            });
        }
        for (text, scale) in [
            ("".to_owned(), 1.0),
            ("x".repeat(65), 1.0),
            ("ok".to_owned(), 0.24),
            ("ok".to_owned(), 8.1),
            ("ok".to_owned(), f32::NAN),
        ] {
            check(NativeMarker {
                shape: NativeMarkerShape::Text { text, scale },
                ..good.clone()
            });
        }
        for ttl in [0, MAX_TTL_TICKS + 1] {
            check(NativeMarker {
                ttl_ticks: Some(ttl),
                ..good.clone()
            });
        }
        drop(check);
        assert!(!markers.set(OWNER, "", &good, 0, LEVEL));
        assert!(!markers.set(OWNER, &"x".repeat(65), &good, 0, LEVEL));
        assert!(!markers.set("", "keep", &good, 0, LEVEL));
        assert_eq!(markers.get(OWNER, "keep"), Some(&good));
    }

    #[test]
    fn the_edges_of_every_range_are_accepted() {
        let mut markers = Markers::default();
        let id = "i".repeat(MAX_ID_CHARS);
        // The largest accepted box and line end and the tallest beam all put
        // their far point exactly on the coordinate bound.
        let edges = NativeMarker {
            pos: [MAX_COORD - MAX_BOX_SIDE, 0.0, 0.0],
            shape: NativeMarkerShape::Box {
                size: [MAX_BOX_SIDE, 0.0, 0.0],
                filled: false,
            },
            argb: 0x0000_0000,
            through_walls: true,
            ttl_ticks: Some(MAX_TTL_TICKS),
        };
        assert!(markers.set(OWNER, &id, &edges, 0, LEVEL));
        let text = NativeMarker {
            shape: NativeMarkerShape::Text {
                text: "t".repeat(MAX_TEXT_CHARS),
                scale: MAX_TEXT_SCALE,
            },
            ttl_ticks: Some(1),
            ..boxed()
        };
        assert!(markers.set(OWNER, "text", &text, 0, LEVEL));
        let small = NativeMarker {
            shape: NativeMarkerShape::Text {
                text: "t".to_owned(),
                scale: MIN_TEXT_SCALE,
            },
            ttl_ticks: None,
            ..boxed()
        };
        assert!(markers.set(OWNER, "small", &small, 0, LEVEL));
        let line = NativeMarker {
            pos: [0.0, 0.0, MAX_COORD - MAX_LINE_OFFSET],
            shape: NativeMarkerShape::Line {
                to: [0.0, 0.0, MAX_LINE_OFFSET],
            },
            ..boxed()
        };
        assert!(markers.set(OWNER, "line", &line, 0, LEVEL));
        let beam = NativeMarker {
            pos: [0.0, MAX_COORD - MAX_BEAM_HEIGHT, 0.0],
            shape: NativeMarkerShape::Beam {
                height: MAX_BEAM_HEIGHT,
            },
            ..boxed()
        };
        assert!(markers.set(OWNER, "beam", &beam, 0, LEVEL));
        // The largest accepted owner and marker id.
        assert!(markers.set(&"o".repeat(MAX_OWNER_CHARS), &id, &boxed(), 0, LEVEL));
    }

    #[test]
    fn a_shape_cannot_reach_past_the_coordinate_bound() {
        let mut markers = Markers::default();
        // One block further than the largest accepted box: its corner is past
        // the bound while its anchor is still inside it.
        let past_box = NativeMarker {
            pos: [MAX_COORD - MAX_BOX_SIDE + 1.0, 0.0, 0.0],
            shape: NativeMarkerShape::Box {
                size: [MAX_BOX_SIDE, 0.0, 0.0],
                filled: true,
            },
            ..boxed()
        };
        assert!(!markers.set(OWNER, "box", &past_box, 0, LEVEL));
        let past_line = NativeMarker {
            pos: [0.0, 0.0, MAX_COORD - MAX_LINE_OFFSET + 1.0],
            shape: NativeMarkerShape::Line {
                to: [0.0, 0.0, MAX_LINE_OFFSET],
            },
            ..boxed()
        };
        assert!(!markers.set(OWNER, "line", &past_line, 0, LEVEL));
        let past_beam = NativeMarker {
            pos: [0.0, MAX_COORD - MAX_BEAM_HEIGHT + 1.0, 0.0],
            shape: NativeMarkerShape::Beam {
                height: MAX_BEAM_HEIGHT,
            },
            ..boxed()
        };
        assert!(!markers.set(OWNER, "beam", &past_beam, 0, LEVEL));

        // `move_to` checks the far point too, and a refused move leaves the
        // marker where it was.
        let boxed_shape = NativeMarkerShape::Box {
            size: [MAX_BOX_SIDE, 0.0, 0.0],
            filled: false,
        };
        let marker = NativeMarker {
            pos: [0.0, 0.0, 0.0],
            shape: boxed_shape.clone(),
            ..boxed()
        };
        assert!(markers.set(OWNER, "moved", &marker, 0, LEVEL));
        assert!(markers.move_to(OWNER, "moved", [MAX_COORD - MAX_BOX_SIDE, 0.0, 0.0]));
        assert_eq!(
            markers.get(OWNER, "moved").map(|m| m.pos),
            Some([MAX_COORD - MAX_BOX_SIDE, 0.0, 0.0])
        );
        assert!(!markers.move_to(OWNER, "moved", [MAX_COORD - MAX_BOX_SIDE + 1.0, 0.0, 0.0]));
        assert_eq!(
            markers.get(OWNER, "moved").map(|m| m.pos),
            Some([MAX_COORD - MAX_BOX_SIDE, 0.0, 0.0])
        );

        let line = NativeMarker {
            pos: [0.0, 0.0, 0.0],
            shape: NativeMarkerShape::Line {
                to: [0.0, 0.0, MAX_LINE_OFFSET],
            },
            ..boxed()
        };
        assert!(markers.set(OWNER, "line-moved", &line, 0, LEVEL));
        assert!(markers.move_to(OWNER, "line-moved", [0.0, 0.0, MAX_COORD - MAX_LINE_OFFSET]));
        assert!(!markers.move_to(
            OWNER,
            "line-moved",
            [0.0, 0.0, MAX_COORD - MAX_LINE_OFFSET + 1.0]
        ));
        assert_eq!(
            markers.get(OWNER, "line-moved").map(|m| m.pos),
            Some([0.0, 0.0, MAX_COORD - MAX_LINE_OFFSET])
        );

        let beam = NativeMarker {
            pos: [0.0, 0.0, 0.0],
            shape: NativeMarkerShape::Beam {
                height: MAX_BEAM_HEIGHT,
            },
            ..boxed()
        };
        assert!(markers.set(OWNER, "beam-moved", &beam, 0, LEVEL));
        assert!(markers.move_to(OWNER, "beam-moved", [0.0, MAX_COORD - MAX_BEAM_HEIGHT, 0.0]));
        assert!(!markers.move_to(
            OWNER,
            "beam-moved",
            [0.0, MAX_COORD - MAX_BEAM_HEIGHT + 1.0, 0.0]
        ));
        assert_eq!(
            markers.get(OWNER, "beam-moved").map(|m| m.pos),
            Some([0.0, MAX_COORD - MAX_BEAM_HEIGHT, 0.0])
        );
    }

    #[test]
    fn a_lifetime_ends_at_set_tick_plus_ticks() {
        // Set at 100 with a lifetime of 10: drawn through tick 109, gone on 110.
        let mut timed = Markers::default();
        let marker = NativeMarker {
            ttl_ticks: Some(10),
            ..boxed()
        };
        assert!(timed.set(OWNER, "timed", &marker, 100, LEVEL));
        timed.prune(109);
        assert_eq!(timed.get(OWNER, "timed"), Some(&marker));
        assert_eq!(emitted(&timed, marker.pos, LEVEL).len(), 1);
        timed.prune(110);
        assert_eq!(timed.get(OWNER, "timed"), None);
        assert!(emitted(&timed, marker.pos, LEVEL).is_empty());

        // Without a lifetime it is never pruned.
        let mut forever = Markers::default();
        let marker = NativeMarker {
            pos: [-3.0, 4.0, 5.0],
            ttl_ticks: None,
            ..boxed()
        };
        assert!(forever.set(OWNER, "forever", &marker, 100, LEVEL));
        forever.prune(u64::MAX);
        assert_eq!(forever.get(OWNER, "forever"), Some(&marker));
        assert_eq!(emitted(&forever, marker.pos, LEVEL).len(), 1);
    }

    #[test]
    fn a_move_leaves_the_lifetime_it_has_left() {
        let mut markers = Markers::default();
        let marker = NativeMarker {
            ttl_ticks: Some(10),
            ..boxed()
        };
        assert!(markers.set(OWNER, "timed", &marker, 100, LEVEL));
        assert!(markers.move_to(OWNER, "timed", [1.0, 2.0, 3.0]));
        // Still set at tick 100: it lasts through 109, not ten ticks from 0.
        markers.prune(109);
        assert_eq!(
            markers.get(OWNER, "timed").map(|m| m.pos),
            Some([1.0, 2.0, 3.0])
        );
        markers.prune(110);
        assert_eq!(markers.get(OWNER, "timed"), None);
    }

    #[test]
    fn a_replace_starts_the_lifetime_from_the_new_now() {
        let mut markers = Markers::default();
        let marker = NativeMarker {
            ttl_ticks: Some(10),
            ..boxed()
        };
        assert!(markers.set(OWNER, "timed", &marker, 100, LEVEL));
        // Set again at 200: the lifetime restarts there, not from 100.
        assert!(markers.set(OWNER, "timed", &marker, 200, LEVEL));
        markers.prune(209);
        assert_eq!(markers.get(OWNER, "timed"), Some(&marker));
        markers.prune(210);
        assert_eq!(markers.get(OWNER, "timed"), None);
    }

    #[test]
    fn each_shape_emits_its_primitive() {
        let color = 0x9912_3456;
        let pos = [10.0, 20.0, 30.0];
        let cases = [
            (
                NativeMarkerShape::Box {
                    size: [4.0, 5.0, 6.0],
                    filled: false,
                },
                PrimitiveShape::BoxOutline {
                    min: pos,
                    max: [14.0, 25.0, 36.0],
                    color,
                    width: MARKER_LINE_WIDTH,
                },
            ),
            (
                NativeMarkerShape::Box {
                    size: [4.0, 5.0, 6.0],
                    filled: true,
                },
                PrimitiveShape::FilledBox {
                    min: pos,
                    max: [14.0, 25.0, 36.0],
                    color,
                },
            ),
            (
                NativeMarkerShape::Line {
                    to: [1.5, -2.5, 3.0],
                },
                PrimitiveShape::Line {
                    from: pos,
                    to: [11.5, 17.5, 33.0],
                    color,
                    width: MARKER_LINE_WIDTH,
                },
            ),
            (
                NativeMarkerShape::Beam { height: 7.4 },
                PrimitiveShape::Beam {
                    base: pos,
                    height: 7,
                    color,
                },
            ),
            (
                NativeMarkerShape::Beam { height: 7.5 },
                PrimitiveShape::Beam {
                    base: pos,
                    height: 8,
                    color,
                },
            ),
            (
                NativeMarkerShape::Text {
                    text: "hi".to_owned(),
                    scale: 1.5,
                },
                PrimitiveShape::Text {
                    at: pos,
                    text: "hi".to_owned(),
                    color,
                    scale: 1.5,
                    scale_with_distance: false,
                    background: false,
                },
            ),
        ];
        for (shape, expected) in cases {
            let mut markers = Markers::default();
            let marker = NativeMarker {
                pos,
                shape: shape.clone(),
                argb: color,
                through_walls: false,
                ttl_ticks: None,
            };
            assert!(markers.set(OWNER, "a", &marker, 0, LEVEL));
            let list = emitted(&markers, pos, LEVEL);
            assert_eq!(list.len(), 1, "{shape:?}");
            assert_eq!(list[0].shape, expected, "{shape:?}");
            assert!(!list[0].through_walls, "{shape:?}");
        }
    }

    /// An icon marker at the anchor `boxed()` uses.
    fn icon(sprite: &str, size: f32) -> NativeMarker {
        NativeMarker {
            pos: [10.0, 20.0, 30.0],
            shape: NativeMarkerShape::Icon {
                sprite: sprite.to_owned(),
                size,
            },
            argb: 0xFF11_2233,
            through_walls: false,
            ttl_ticks: None,
        }
    }

    #[test]
    fn an_icon_names_a_sprite_the_atlas_could_hold_and_a_size() {
        let mut markers = Markers::default();
        // An item's sprite, a block's and another of the atlas's.
        for (id, sprite) in [
            ("item", "item/diamond"),
            ("block", "block/stone"),
            ("other", "entity/beacon/beacon_beam"),
        ] {
            assert!(
                markers.set(OWNER, id, &icon(sprite, 1.0), 0, LEVEL),
                "{sprite}"
            );
        }
        // The ends of the size range and the longest sprite.
        assert!(markers.set(
            OWNER,
            "small",
            &icon("item/diamond", MIN_ICON_SIZE),
            0,
            LEVEL
        ));
        assert!(markers.set(
            OWNER,
            "large",
            &icon("item/diamond", MAX_ICON_SIZE),
            0,
            LEVEL
        ));
        let longest = "a".repeat(MAX_SPRITE_CHARS);
        assert!(markers.set(OWNER, "long", &icon(&longest, 2.0), 0, LEVEL));

        // A refused icon leaves the marker the id had.
        assert!(markers.set(OWNER, "keep", &boxed(), 0, LEVEL));
        for (sprite, size) in [
            (String::new(), 1.0),
            ("a".repeat(MAX_SPRITE_CHARS + 1), 1.0),
            ("Item/Diamond".to_owned(), 1.0),
            ("item/diamond ".to_owned(), 1.0),
            ("item/*".to_owned(), 1.0),
            ("item/diamond".to_owned(), MIN_ICON_SIZE - 0.01),
            ("item/diamond".to_owned(), MAX_ICON_SIZE + 0.5),
            ("item/diamond".to_owned(), f32::NAN),
            ("item/diamond".to_owned(), f32::INFINITY),
        ] {
            assert!(
                !markers.set(OWNER, "keep", &icon(&sprite, size), 0, LEVEL),
                "{sprite:?} {size}"
            );
            assert_eq!(markers.get(OWNER, "keep"), Some(&boxed()), "{sprite:?}");
        }
    }

    #[test]
    fn an_icon_stays_inside_the_coordinate_bound_on_both_sides() {
        // It is centred on its anchor, so either half must stay in bounds.
        let half = f64::from(MAX_ICON_SIZE) / 2.0;
        let at = |x: f64| NativeMarker {
            pos: [x, 0.0, 0.0],
            ..icon("item/diamond", MAX_ICON_SIZE)
        };
        let mut markers = Markers::default();
        assert!(markers.set(OWNER, "far-edge", &at(MAX_COORD - half), 0, LEVEL));
        assert!(!markers.set(OWNER, "past", &at(MAX_COORD - half + 1.0), 0, LEVEL));
        assert!(markers.set(OWNER, "near-edge", &at(-(MAX_COORD - half)), 0, LEVEL));
        assert!(!markers.set(OWNER, "past-low", &at(-(MAX_COORD - half + 1.0)), 0, LEVEL));

        // A move checks the same bounds, and a refused one leaves the anchor.
        assert!(markers.move_to(OWNER, "far-edge", [0.0, MAX_COORD - half, 0.0]));
        assert!(!markers.move_to(OWNER, "far-edge", [0.0, MAX_COORD - half + 1.0, 0.0]));
        assert_eq!(
            markers.get(OWNER, "far-edge").map(|m| m.pos),
            Some([0.0, MAX_COORD - half, 0.0])
        );
    }

    #[test]
    fn an_icon_marker_lives_and_expires_as_the_other_shapes_do() {
        let mut markers = Markers::default();
        let timed = NativeMarker {
            ttl_ticks: Some(10),
            ..icon("item/diamond", 1.0)
        };
        assert!(markers.set(OWNER, "timed", &timed, 100, LEVEL));
        markers.prune(109);
        assert_eq!(markers.get(OWNER, "timed"), Some(&timed));
        markers.prune(110);
        assert_eq!(markers.get(OWNER, "timed"), None);

        // It belongs to the level it was set in, and is kept in another.
        assert!(markers.set(OWNER, "mine", &timed, 0, LEVEL));
        assert!(emitted(&markers, timed.pos, NETHER).is_empty());
        assert_eq!(markers.get(OWNER, "mine"), Some(&timed));

        // And it counts against the per-owner cap as any marker does.
        let mut full = Markers::default();
        for i in 0..MAX_MARKERS_PER_OWNER {
            assert!(full.set(OWNER, &format!("m{i}"), &timed, 0, LEVEL), "m{i}");
        }
        assert!(!full.set(OWNER, "one-too-many", &timed, 0, LEVEL));
        assert!(full.set(OWNER, "m0", &timed, 0, LEVEL));
    }

    #[test]
    fn an_icon_marker_emits_a_sprite_at_its_anchor_size_and_colour() {
        let mut markers = Markers::default();
        let marker = icon("item/diamond", 2.0);
        assert!(markers.set(OWNER, "icon", &marker, 0, LEVEL));
        let drawn = emitted(&markers, [0.0; 3], LEVEL);
        assert_eq!(drawn.len(), 1);
        assert_eq!(
            drawn[0].shape,
            PrimitiveShape::Sprite {
                at: [10.0, 20.0, 30.0],
                sprite: "item/diamond".to_owned(),
                size: 2.0,
                color: 0xFF11_2233,
            }
        );
        assert!(!drawn[0].through_walls);
        // Through walls is kept for an icon as for the other shapes.
        let through = NativeMarker {
            through_walls: true,
            ..marker
        };
        assert!(markers.set(OWNER, "icon", &through, 0, LEVEL));
        assert!(emitted(&markers, [0.0; 3], LEVEL)[0].through_walls);
    }

    #[test]
    fn ids_and_text_are_measured_in_characters() {
        let mut markers = Markers::default();
        let marker = boxed();
        // 64 characters of two and of four bytes each are both 64 characters.
        let two_byte = "é".repeat(MAX_ID_CHARS);
        let four_byte = "😀".repeat(MAX_ID_CHARS);
        assert!(markers.set(OWNER, &two_byte, &marker, 0, LEVEL));
        assert!(markers.set(OWNER, &four_byte, &marker, 0, LEVEL));
        assert!(!markers.set(OWNER, &"é".repeat(MAX_ID_CHARS + 1), &marker, 0, LEVEL));
        assert!(!markers.set(OWNER, &"😀".repeat(MAX_ID_CHARS + 1), &marker, 0, LEVEL));

        let text = |text: String| NativeMarker {
            shape: NativeMarkerShape::Text { text, scale: 1.0 },
            ..boxed()
        };
        assert!(markers.set(OWNER, "text-two", &text(two_byte.clone()), 0, LEVEL));
        assert!(markers.set(OWNER, "text-four", &text(four_byte.clone()), 0, LEVEL));
        assert!(!markers.set(
            OWNER,
            "text-65",
            &text("é".repeat(MAX_TEXT_CHARS + 1)),
            0,
            LEVEL
        ));
        assert!(!markers.set(
            OWNER,
            "text-65",
            &text("😀".repeat(MAX_TEXT_CHARS + 1)),
            0,
            LEVEL
        ));

        // An owner is measured the same way.
        assert!(markers.set(&two_byte, "owner", &marker, 0, LEVEL));
        assert!(!markers.set(
            &"😀".repeat(MAX_OWNER_CHARS + 1),
            "owner",
            &marker,
            0,
            LEVEL
        ));
    }

    #[test]
    fn through_walls_is_kept_per_marker() {
        let mut markers = Markers::default();
        let seen = NativeMarker {
            through_walls: true,
            ..boxed()
        };
        let hidden = NativeMarker {
            pos: [0.0, 0.0, 0.0],
            through_walls: false,
            ..boxed()
        };
        assert!(markers.set(OWNER, "seen", &seen, 0, LEVEL));
        assert!(markers.set(OWNER, "hidden", &hidden, 0, LEVEL));
        let list = emitted(&markers, [0.0, 0.0, 0.0], LEVEL);
        assert_eq!(list.len(), 2);
        let seen = list
            .iter()
            .find(|p| {
                p.shape
                    == PrimitiveShape::BoxOutline {
                        min: [10.0, 20.0, 30.0],
                        max: [14.0, 25.0, 36.0],
                        color: 0xFF11_2233,
                        width: MARKER_LINE_WIDTH,
                    }
            })
            .expect("the through-walls marker");
        assert!(seen.through_walls);
        let hidden = list
            .iter()
            .find(|p| {
                p.shape
                    == PrimitiveShape::BoxOutline {
                        min: [0.0, 0.0, 0.0],
                        max: [4.0, 5.0, 6.0],
                        color: 0xFF11_2233,
                        width: MARKER_LINE_WIDTH,
                    }
            })
            .expect("the depth-tested marker");
        assert!(!hidden.through_walls);
    }

    #[test]
    fn a_marker_past_256_blocks_is_skipped_but_kept() {
        let mut markers = Markers::default();
        let far = NativeMarker {
            pos: [257.0, 0.0, 0.0],
            ..boxed()
        };
        let edge = NativeMarker {
            pos: [256.0, 0.0, 0.0],
            ..boxed()
        };
        assert!(markers.set(OWNER, "far", &far, 0, LEVEL));
        assert!(markers.set(OWNER, "edge", &edge, 0, LEVEL));
        let list = emitted(&markers, [0.0, 0.0, 0.0], LEVEL);
        assert_eq!(list.len(), 1, "only the one at exactly 256 blocks");
        assert_eq!(
            list[0].shape,
            PrimitiveShape::BoxOutline {
                min: [256.0, 0.0, 0.0],
                max: [260.0, 5.0, 6.0],
                color: 0xFF11_2233,
                width: MARKER_LINE_WIDTH,
            }
        );
        assert_eq!(markers.get(OWNER, "far"), Some(&far), "kept, not dropped");
    }

    #[test]
    fn emit_measures_from_the_camera() {
        let mut markers = Markers::default();
        let at = |x: f64| NativeMarker {
            pos: [x, 0.0, 0.0],
            ..boxed()
        };
        assert!(markers.set(OWNER, "near", &at(755.0), 0, LEVEL));
        assert!(markers.set(OWNER, "far", &at(757.0), 0, LEVEL));
        assert!(markers.set(OTHER, "at-the-origin", &at(0.0), 0, LEVEL));
        // A camera at 500: only the marker 255 blocks away is in reach, though
        // the one at the origin is 755 blocks away.
        let list = emitted(&markers, [500.0, 0.0, 0.0], LEVEL);
        assert_eq!(list.len(), 1, "only the marker 255 blocks away");
        assert_eq!(
            list[0].shape,
            PrimitiveShape::BoxOutline {
                min: [755.0, 0.0, 0.0],
                max: [759.0, 5.0, 6.0],
                color: 0xFF11_2233,
                width: MARKER_LINE_WIDTH,
            }
        );
        // From the origin the same markers come out the other way round.
        let list = emitted(&markers, [0.0, 0.0, 0.0], LEVEL);
        assert_eq!(list.len(), 1, "only the marker at the origin");
        assert_eq!(
            list[0].shape,
            PrimitiveShape::BoxOutline {
                min: [0.0, 0.0, 0.0],
                max: [4.0, 5.0, 6.0],
                color: 0xFF11_2233,
                width: MARKER_LINE_WIDTH,
            }
        );
    }

    #[test]
    fn a_marker_is_drawn_only_in_the_level_it_was_set_in() {
        let mut markers = Markers::default();
        let mine = boxed();
        let theirs = NativeMarker {
            pos: [-1000.0, 64.0, 0.0],
            shape: NativeMarkerShape::Beam { height: 40.0 },
            argb: 0xFF00_FF00,
            through_walls: false,
            ttl_ticks: None,
        };
        assert!(markers.set(OWNER, "mine", &mine, 0, LEVEL));
        assert!(markers.set(OTHER, "theirs", &theirs, 0, NETHER));
        // Each level draws only the markers set in it, from the camera where
        // its own marker is; a marker of the other level is kept, not dropped.
        assert_eq!(emitted(&markers, mine.pos, LEVEL).len(), 1);
        assert_eq!(emitted(&markers, mine.pos, NETHER).len(), 0);
        assert_eq!(emitted(&markers, theirs.pos, NETHER).len(), 1);
        assert_eq!(emitted(&markers, theirs.pos, LEVEL).len(), 0);
        assert_eq!(markers.get(OWNER, "mine"), Some(&mine));
        // A move keeps the level the marker was set in, and does not take the
        // level it is moved in.
        assert!(markers.move_to(OWNER, "mine", [1.0, 2.0, 3.0]));
        assert_eq!(emitted(&markers, [1.0, 2.0, 3.0], NETHER).len(), 0);
        assert_eq!(emitted(&markers, [1.0, 2.0, 3.0], LEVEL).len(), 1);
    }

    #[test]
    fn clear_all_drops_every_mods_markers() {
        let mut markers = Markers::default();
        let marker = boxed();
        assert!(markers.set(OWNER, "a", &marker, 0, LEVEL));
        assert!(markers.set(OTHER, "b", &marker, 0, NETHER));
        markers.clear_all();
        assert_eq!(markers.get(OWNER, "a"), None);
        assert_eq!(markers.get(OTHER, "b"), None);
        assert!(emitted(&markers, marker.pos, LEVEL).is_empty());
        assert!(emitted(&markers, marker.pos, NETHER).is_empty());
    }

    /// A `Game` with prepared resources, on the window's side (`Client`).
    fn game() -> Game<'static> {
        use crate::game::execution_role::ExecutionRole;
        use crate::game::{Audio, Options, Progress, Settings, load_resources};
        let root = std::path::PathBuf::from(
            std::env::var_os("HORIZON_TEST_RESOURCES")
                .expect("set HORIZON_TEST_RESOURCES to prepared resources"),
        );
        let settings = Settings::default();
        let loaded = load_resources(
            root.clone(),
            None,
            &root,
            &settings,
            std::time::Instant::now(),
            false,
            true,
            &Progress::new(),
            &mut |_, _| unreachable!(),
        )
        .unwrap();
        let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
        options.resources = root.clone();
        options.execution_role = ExecutionRole::Client;
        let res: &'static _ = Box::leak(Box::new(loaded.resources));
        Game::new(
            res,
            loaded.pack,
            loaded.gui.unwrap(),
            options,
            settings,
            Audio::silent(&root),
            None,
        )
        .unwrap()
    }

    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn the_helpers_work_on_the_window_and_refuse_on_a_server() {
        use crate::game::execution_role::ExecutionRole;
        let mut game = game();
        let marker = boxed();
        assert!(game.native_marker_set(OWNER, "a", &marker));
        assert_eq!(game.markers.get(OWNER, "a"), Some(&marker));

        // A server-thread Game has no window: nothing is stored, nothing changes.
        game.options.execution_role = ExecutionRole::IntegratedServer;
        assert!(!game.native_marker_set(OWNER, "b", &marker));
        assert_eq!(game.markers.get(OWNER, "b"), None);
        assert!(!game.native_marker_move(OWNER, "a", [1.0, 2.0, 3.0]));
        assert_eq!(game.markers.get(OWNER, "a"), Some(&marker));
        assert!(!game.native_marker_remove(OWNER, "a"));
        assert_eq!(game.markers.get(OWNER, "a"), Some(&marker));
        game.native_marker_clear_owner(OWNER);
        assert_eq!(game.markers.get(OWNER, "a"), Some(&marker));

        // Back on the window: move, remove, clear.
        game.options.execution_role = ExecutionRole::Client;
        assert!(game.native_marker_move(OWNER, "a", [1.0, 2.0, 3.0]));
        assert_eq!(
            game.markers.get(OWNER, "a").map(|m| m.pos),
            Some([1.0, 2.0, 3.0])
        );
        game.native_marker_clear_owner(OWNER);
        assert_eq!(game.markers.get(OWNER, "a"), None);
        assert!(game.native_marker_set(OWNER, "a", &marker));
        assert!(game.native_marker_remove(OWNER, "a"));
        assert!(!game.native_marker_remove(OWNER, "a"));
    }

    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn leaving_the_world_drops_the_windows_markers() {
        let mut game = game();
        let marker = boxed();
        assert!(game.native_marker_set(OWNER, "a", &marker));
        assert!(game.native_marker_set(OTHER, "b", &marker));
        assert!(game.native_marker_set(OTHER, "c", &marker));

        // The seam every world-open and world-close path calls.
        game.drop_markers_for_new_world();
        assert!(game.markers.entries.is_empty(), "the seam empties it");

        // And the window leaving a world, as single player's close does.
        assert!(game.native_marker_set(OWNER, "a", &marker));
        assert!(game.native_marker_set(OTHER, "b", &marker));
        game.leave_server(None).unwrap();
        assert!(game.markers.entries.is_empty(), "leave_server empties it");
        let mut out = WorldPrimitives::default();
        game.markers
            .emit([0.0; 3], game.marker_level_key(), &mut out);
        assert!(out.is_empty(), "and nothing is drawn on the next world");
    }
}
