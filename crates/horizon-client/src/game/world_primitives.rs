//! Native mods' world primitives (`native_world_primitives`): asked once a frame, tessellated
//! here camera-relative into quads from the shared atlas and gizmo lines, and drawn by the
//! renderer's own pipelines (`gfx::world_primitives`) after the entities, before the hand.

use super::Game;
use horizon_client::block_entity_render::{BeamShape, beam};
use horizon_client::gfx::world_primitives::Batch;
use horizon_client::gizmos::{self, Gizmos, Line};
use horizon_client::hud::Rect;
use horizon_client::mesher::Vertex;
use horizon_client::text::Text;
use horizon_client::world_primitives::{
    MAX_BEAM_HEIGHT, PrimitiveShape, WorldPrimitives, text_unit, vertex_color,
};
use std::collections::HashMap;

/// A frame's primitives as the renderer draws them, depth tested and through walls.
#[derive(Default)]
pub(super) struct PrimitiveGeometry {
    quads: [Vec<Vertex>; 2],
    lines: [Vec<[f32; 8]>; 2],
}

impl PrimitiveGeometry {
    pub(super) fn batch(&self) -> Batch<'_> {
        Batch {
            quads: &self.quads[0],
            quads_through: &self.quads[1],
            lines: &self.lines[0],
            lines_through: &self.lines[1],
        }
    }
}

impl Game<'_> {
    /// Asks the mods for this frame's primitives and tessellates them, `alpha` ticks past the
    /// last tick, seen from `camera` through `view` (the camera's rotation).
    pub(super) fn world_primitive_geometry(
        &mut self,
        camera: glam::DVec3,
        alpha: f64,
        view: glam::Mat4,
    ) -> PrimitiveGeometry {
        let mut primitives = WorldPrimitives::default();
        // The window's retained markers first, then the mods' frame-local ones.
        // A marker of another level is skipped: it belongs to the level it was
        // set in.
        let now = self.world.game_time.max(0) as u64;
        self.markers.prune(now);
        let level = self.marker_level_key();
        self.markers.emit(camera.to_array(), level, &mut primitives);
        self.native_world_primitives(alpha as f32, camera.to_array(), &mut primitives);
        let mut out = PrimitiveGeometry::default();
        if primitives.is_empty() {
            return out;
        }
        let atlas = &self.gui.atlas;
        let white = horizon_client::gfx::lightning::white_uv(&atlas.font.white());
        let beam_sprite = atlas.sprites.get("entity/beacon/beacon_beam").copied();
        // BeaconRenderer's animation time: game time mod 40 plus the partial tick.
        let time = self.world.game_time.rem_euclid(40) as f32 + alpha as f32;
        let relative = |p: [f64; 3]| (glam::DVec3::from_array(p) - camera).as_vec3();
        let mut lines: [Vec<Line>; 2] = Default::default();
        let mut texts = Vec::new();
        for primitive in primitives.iter() {
            let through = usize::from(primitive.through_walls);
            let quads = &mut out.quads[through];
            match &primitive.shape {
                PrimitiveShape::Line {
                    from,
                    to,
                    color,
                    width,
                } if finite(from) && finite(to) && width.is_finite() => {
                    lines[through].push(Line {
                        start: *from,
                        end: *to,
                        color: *color,
                        width: *width,
                    });
                }
                PrimitiveShape::BoxOutline {
                    min,
                    max,
                    color,
                    width,
                } if finite(min) && finite(max) && width.is_finite() => {
                    let mut edges = Gizmos::default();
                    edges.cuboid(*min, *max, *color, *width);
                    lines[through].extend(edges.lines);
                }
                PrimitiveShape::FilledBox { min, max, color } if finite(min) && finite(max) => {
                    let color = vertex_color(*color, 255);
                    for quad in gizmos::cuboid_fill_quads(*min, *max) {
                        quads.extend(quad.map(|corner| plain(relative(corner), white, color)));
                    }
                }
                PrimitiveShape::Beam {
                    base,
                    height,
                    color,
                } if finite(base) => {
                    let Some(sprite) = beam_sprite else {
                        continue;
                    };
                    // `beam` draws around the block's centre, half a block in from its corner.
                    let corner = relative(*base) - glam::Vec3::new(0.5, 0.0, 0.5);
                    let start = quads.len();
                    beam(
                        glam::Mat4::from_translation(corner),
                        BeamShape {
                            scale: 1.0,
                            time,
                            start: 0,
                            height: (*height).min(MAX_BEAM_HEIGHT) as i32,
                            color: color & 0xFF_FFFF,
                            solid_radius: 0.2,
                            glow_radius: 0.25,
                        },
                        sprite,
                        quads,
                    );
                    // The colour's alpha scales the beacon's own (the core's 255, the glow's 32).
                    for v in &mut quads[start..] {
                        let a = (v.color >> 24) * (color >> 24) / 255;
                        v.color = a << 24 | v.color & 0xFF_FFFF;
                    }
                }
                PrimitiveShape::Sprite {
                    at,
                    sprite,
                    size,
                    color,
                } if finite(at) && size.is_finite() && *size > 0.0 => {
                    sprite_quad(
                        &atlas.sprites,
                        sprite,
                        relative(*at),
                        view,
                        *size,
                        *color,
                        quads,
                    );
                }
                PrimitiveShape::Text {
                    at,
                    text,
                    color,
                    scale,
                    scale_with_distance,
                    background,
                } if finite(at) => {
                    let o = glam::DVec3::from_array(*at) - camera;
                    let unit = text_unit(*scale, *scale_with_distance, o.length());
                    if unit > 0.0 {
                        texts.push((through, o, text.clone(), *color, unit, *background));
                    }
                }
                _ => {}
            }
        }
        for (through, o, text, color, unit, background) in texts {
            // A line of text is 9 font pixels tall, hung below its anchor: lift it by half.
            let line = [(
                Text::colored(text, color & 0xFF_FFFF),
                f64::from(unit * 4.5),
            )];
            let patches = self.name_tag_quads(
                o,
                &line,
                color >> 24,
                background,
                0,
                0,
                unit,
                &mut out.quads[through],
            );
            self.map_patches.extend(patches);
        }
        let at = camera.to_array();
        out.lines = lines.map(|lines| gizmos::camera_relative(&lines, at, view));
        out
    }
}

/// The quad of the atlas sprite `name` facing the camera: its four corners centred on
/// `center` (camera-relative), spanned by the camera's right and up axes in `view` (the
/// camera's rotation) scaled by `size / 2`, its uv the sprite's atlas region, tinted `color`.
/// A name the atlas lacks adds nothing.
fn sprite_quad(
    sprites: &HashMap<String, Rect>,
    name: &str,
    center: glam::Vec3,
    view: glam::Mat4,
    size: f32,
    color: u32,
    out: &mut Vec<Vertex>,
) {
    let Some(rect) = sprites.get(name) else {
        return;
    };
    // The view's inverse maps camera axes to world axes: its x and y columns are the
    // camera's right and up in world space.
    let inverse = view.inverse();
    let half = size / 2.0;
    let right = inverse.x_axis.truncate() * half;
    let up = inverse.y_axis.truncate() * half;
    let color = vertex_color(color, 255);
    let corner = |across: f32, along: f32, uv: [f32; 2]| Vertex {
        pos: (center + right * across + up * along).to_array(),
        uv,
        layer: 0,
        color,
        light: 0,
        normal: [0; 4],
        glint_uv: [0.0, 0.0],
    };
    out.extend([
        corner(-1.0, 1.0, [rect.u0, rect.v0]),
        corner(-1.0, -1.0, [rect.u0, rect.v1]),
        corner(1.0, -1.0, [rect.u1, rect.v1]),
        corner(1.0, 1.0, [rect.u1, rect.v0]),
    ]);
}

fn finite(p: &[f64; 3]) -> bool {
    p.iter().all(|v| v.is_finite())
}

/// A vertex of a plain colour: the atlas's white texel.
fn plain(pos: glam::Vec3, white: [f32; 2], color: u32) -> Vertex {
    Vertex {
        pos: pos.to_array(),
        uv: white,
        layer: 0,
        color,
        light: 0,
        normal: [0; 4],
        glint_uv: [0.0, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect {
            u0: 0.25,
            v0: 0.5,
            u1: 0.75,
            v1: 1.0,
            w: 16,
            h: 16,
        }
    }

    fn atlas() -> HashMap<String, Rect> {
        HashMap::from([("item/diamond".to_owned(), rect())])
    }

    #[test]
    fn a_sprite_quad_faces_a_camera_looking_down_plus_x() {
        // Looking down +x with +y up, the camera's right is +z. A sprite 2 blocks across
        // centred at (5, 0, 0) spans y and z around its centre, in its atlas region.
        let view = glam::Mat4::look_to_rh(glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y);
        let mut out = Vec::new();
        sprite_quad(
            &atlas(),
            "item/diamond",
            glam::Vec3::new(5.0, 0.0, 0.0),
            view,
            2.0,
            0xFF11_2233,
            &mut out,
        );
        let pos: Vec<[f32; 3]> = out.iter().map(|v| v.pos).collect();
        assert_eq!(
            pos,
            vec![
                [5.0, 1.0, -1.0],
                [5.0, -1.0, -1.0],
                [5.0, -1.0, 1.0],
                [5.0, 1.0, 1.0],
            ]
        );
        // Every corner lies in the plane through the sprite perpendicular to the view
        // direction, so the sprite faces the camera.
        assert!(out.iter().all(|v| v.pos[0] == 5.0));
        let uv: Vec<[f32; 2]> = out.iter().map(|v| v.uv).collect();
        assert_eq!(uv, vec![[0.25, 0.5], [0.25, 1.0], [0.75, 1.0], [0.75, 0.5]]);
    }

    #[test]
    fn a_sprite_quad_is_tinted_by_its_colour_with_full_coverage() {
        let mut out = Vec::new();
        sprite_quad(
            &atlas(),
            "item/diamond",
            glam::Vec3::ZERO,
            glam::Mat4::IDENTITY,
            1.0,
            0x80FF_8000,
            &mut out,
        );
        // ARGB 0x80FF8000 as RGBA8 bytes: red 0xFF, green 0x80, blue 0x00, alpha 0x80.
        assert!(out.iter().all(|v| v.color == 0x800080FF && v.layer == 0));
    }

    #[test]
    fn a_sprite_the_atlas_lacks_adds_no_quad_and_does_not_panic() {
        let mut out = Vec::new();
        sprite_quad(
            &atlas(),
            "item/unknown",
            glam::Vec3::ZERO,
            glam::Mat4::IDENTITY,
            1.0,
            0xFFFF_FFFF,
            &mut out,
        );
        assert!(out.is_empty());
    }
}
