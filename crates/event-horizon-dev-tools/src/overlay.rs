//! The developer tools' overlay: one egui window drawn over the world, in the `world_drawn`
//! callbacks. Its input is the screen's size and the time only: no pointer and no keys yet.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use mod_render::render::{WorldDrawn, on_world_drawn};

/// The lang key of the window's title (`en_us`).
pub const TITLE_KEY: &str = "horizon.dev_tools.title";

/// What the overlay keeps between frames. Empty until the first frame it draws.
#[derive(Default)]
struct Overlay {
    ctx: Option<egui::Context>,
    painter: Option<Painter>,
    started: Option<Instant>,
}

impl Overlay {
    const fn new() -> Self {
        Self {
            ctx: None,
            painter: None,
            started: None,
        }
    }
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
    let ctx = state.ctx.get_or_insert_with(egui::Context::default).clone();
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
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(w as f32, h as f32),
        )),
        time: Some(started.elapsed().as_secs_f64()),
        ..Default::default()
    };
    let output = ctx.run_ui(input, |ui| window(ui.ctx()));
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

/// The one window: titled by [`TITLE_KEY`], with one label naming the mod that drew it.
fn window(ctx: &egui::Context) {
    egui::Window::new(horizon_client_core::lang::get(TITLE_KEY)).show(ctx, |ui| {
        ui.label(format!(
            "{} {}",
            crate::package::ID,
            crate::package::VERSION
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_starts_empty() {
        let overlay = Overlay::new();
        assert!(overlay.ctx.is_none());
        assert!(overlay.painter.is_none());
        assert!(overlay.started.is_none());
    }

    #[test]
    fn the_title_key_is_in_en_us() {
        let en_us: serde_json::Map<String, serde_json::Value> = serde_json::from_str(include_str!(
            "../../horizon-client/assets/horizon/lang/en_us.json"
        ))
        .unwrap();
        assert_eq!(
            en_us.get(TITLE_KEY).and_then(|v| v.as_str()),
            Some("Developer Tools")
        );
    }
}
