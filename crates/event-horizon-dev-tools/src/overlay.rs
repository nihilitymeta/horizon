//! The developer tools' overlay: one egui window drawn over the world, in the `world_drawn`
//! callbacks. Its input is the frame's input from [`crate::input`] (the pointer and the keys the
//! tools take), or the screen's size and the time when there is none.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use mod_render::render::{WorldDrawn, on_world_drawn};

/// What the overlay keeps between frames. Empty until the first frame it draws.
#[derive(Default)]
struct Overlay {
    painter: Option<Painter>,
    started: Option<Instant>,
    /// The world (its count of closes, `observe::resets`) the server was last told this client
    /// shows the windows for (`Wire::Hello`).
    greeted: Option<u64>,
}

impl Overlay {
    const fn new() -> Self {
        Self {
            painter: None,
            started: None,
            greeted: None,
        }
    }
}

/// Whether the server must hear `Wire::Hello` again: it has not heard one for this world yet
/// (`greeted` is `None` or another world's count).
fn should_greet(greeted: Option<u64>, now: u64) -> bool {
    greeted != Some(now)
}

/// The egui wgpu renderer, made on the first frame for that frame's colour format.
struct Painter {
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
}

static OVERLAY: Mutex<Overlay> = Mutex::new(Overlay::new());

fn overlay() -> MutexGuard<'static, Overlay> {
    OVERLAY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers [`draw`] with the world's draw. Called once, when the tools are on.
pub fn install() {
    on_world_drawn(draw);
}

/// Draws the window over the world's picture, after the world and before the HUD. It does not
/// touch the depth: the window has no depth test.
fn draw(world: &mut WorldDrawn<'_>) {
    let [w, h] = world.size;
    if w == 0 || h == 0 {
        return;
    }
    let mut state = overlay();
    let state = &mut *state;
    let closes = event_horizon_observe::resets();
    if should_greet(state.greeted, closes) {
        event_horizon_observe::send_to_server(event_horizon_observe::wire::Wire::Hello);
        state.greeted = Some(closes);
    }
    let ctx = crate::input::ctx().clone();
    let started = *state.started.get_or_insert_with(Instant::now);
    let painter = match &mut state.painter {
        Some(p) if p.format == world.color_format => p,
        slot => slot.insert(Painter {
            renderer: egui_wgpu::Renderer::new(
                world.device,
                world.color_format,
                egui_wgpu::RendererOptions::default(),
            ),
            format: world.color_format,
        }),
    };
    let input = crate::input::take_input().unwrap_or_else(|| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(w as f32, h as f32),
        )),
        time: Some(started.elapsed().as_secs_f64()),
        ..Default::default()
    });
    let output = ctx.run_ui(input, |ui| crate::windows::ui(ui.ctx()));
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [w, h],
        pixels_per_point: output.pixels_per_point,
    };
    for (id, deltas) in &output.textures_delta.set {
        for delta in deltas {
            painter
                .renderer
                .update_texture(world.device, world.queue, *id, delta);
        }
    }
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    painter
        .renderer
        .update_buffers(world.device, world.queue, world.encoder, &jobs, &screen);
    {
        let pass = world
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("developer tools (egui)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: world.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        let mut pass = pass.forget_lifetime();
        painter.renderer.render(&mut pass, &jobs, &screen);
    }
    for id in &output.textures_delta.free {
        painter.renderer.free_texture(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_starts_empty() {
        let overlay = Overlay::new();
        assert!(overlay.painter.is_none());
        assert!(overlay.started.is_none());
        assert!(overlay.greeted.is_none());
    }

    #[test]
    fn the_server_hears_hello_once_per_world() {
        assert!(should_greet(None, 0), "the first draw greets");
        assert!(
            !should_greet(Some(0), 0),
            "the same world is not greeted again"
        );
        assert!(should_greet(Some(0), 1), "a new world is greeted");
        assert!(!should_greet(Some(1), 1));
        assert!(
            should_greet(Some(1), 2),
            "every world close starts a new one"
        );
    }
}
