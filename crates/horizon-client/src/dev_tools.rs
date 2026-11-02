//! Developer tools drawn with egui (docs/dev-tools.md). On with
//! `HORIZON_DEV_TOOLS=1`: a panel over the world with the frame's numbers,
//! for people working on mods and engine features, off in a player's game.
//!
//! It is wired as a built-in trait mod: a [`Hooks`] whose [`RenderHooks`]
//! draw in `world_drawn`, the renderer's hook point after the world and
//! before the HUD, with the frame's device, queue, encoder and colour target.
//! That keeps the renderer untouched; the costs are listed in the document
//! (drawn under the HUD, no pointer input yet, a fixed scale).
use crate::gfx::native_render::{RenderHooks, WorldDrawnEvent};
use crate::mod_api::{FrameEvent, Hooks, Host, Mods};
use std::sync::Mutex;

/// The environment variable that turns the tools on (`1`).
pub const ENV: &str = "HORIZON_DEV_TOOLS";
/// `HORIZON_DEV_TOOLS_SCALE`: egui points per pixel, 1.0 by default.
pub const SCALE_ENV: &str = "HORIZON_DEV_TOOLS_SCALE";

/// The built-in mods of a plain `horizon-client`: the dev tools when asked for.
pub fn mods() -> Vec<Box<dyn Hooks>> {
    let mut mods: Vec<Box<dyn Hooks>> = Vec::new();
    if std::env::var(ENV).is_ok_and(|v| v == "1") {
        log::info!("Horizon: developer tools on ({ENV}=1)");
        mods.push(Box::new(DevTools::default()));
    }
    mods
}

/// What the tools show, gathered on the game thread by [`Hooks::frame`] and
/// drawn on the render thread.
#[derive(Clone, Copy, Debug, Default)]
struct Stats {
    frames: u64,
    frame_ms: f32,
}

/// The egui context and its wgpu renderer, made on the first frame for the
/// frame's colour format.
struct Painter {
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
}

#[derive(Default)]
pub struct DevTools {
    ctx: egui::Context,
    painter: Mutex<Option<Painter>>,
    stats: Mutex<Stats>,
}

impl DevTools {
    fn scale() -> f32 {
        std::env::var(SCALE_ENV)
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|s| s.is_finite() && *s >= 0.25 && *s <= 8.0)
            .unwrap_or(1.0)
    }

    fn ui(&self, ctx: &egui::Context, stats: Stats) {
        egui::Window::new("Horizon dev tools")
            .default_pos([16.0, 16.0])
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("frames: {}", stats.frames));
                ui.label(format!("frame: {:.2} ms", stats.frame_ms));
                ui.separator();
                ui.small("HORIZON_DEV_TOOLS=1 · drawn after the world, before the HUD");
            });
    }
}

impl Hooks for DevTools {
    fn render(&self) -> Option<&dyn RenderHooks> {
        Some(self)
    }
    fn frame(&self, _host: &mut dyn Host, event: &mut FrameEvent) {
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.frames += 1;
        stats.frame_ms = event.elapsed.as_secs_f32() * 1000.0;
    }
}

impl RenderHooks for DevTools {
    fn world_drawn(&self, event: &mut WorldDrawnEvent<'_, '_>) {
        let world = &mut *event.world;
        let scale = Self::scale();
        let [w, h] = world.size;
        if w == 0 || h == 0 {
            return;
        }
        let stats = *self.stats.lock().unwrap_or_else(|e| e.into_inner());
        let mut painter = self.painter.lock().unwrap_or_else(|e| e.into_inner());
        let painter = match painter.as_mut() {
            Some(p) if p.format == world.color_format => p,
            _ => painter.insert(Painter {
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
                egui::vec2(w as f32 / scale, h as f32 / scale),
            )),
            ..Default::default()
        };
        let output = self.ctx.run_ui(input, |ui| self.ui(ui.ctx(), stats));
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: scale,
        };
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                painter
                    .renderer
                    .update_texture(world.device, world.queue, *id, delta);
            }
        }
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        painter
            .renderer
            .update_buffers(world.device, world.queue, world.encoder, &jobs, &screen);
        {
            let pass = world
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("dev tools (egui)"),
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
}
