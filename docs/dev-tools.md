# Developer tools (egui)

Status: a first overlay is built (`crates/horizon-client/src/dev_tools.rs`); the rest is the plan. Branch
`egui-tools` of whos-carmen/horizon, `main` + this one concern. For people working on mods and engine features,
never on in a player's game.

## Why egui, not Dear ImGui

The game draws with wgpu 30.0.1 and winit 0.30.13 and has no immediate-mode UI library. `egui` 0.37 tracks exactly
those versions (`egui-wgpu` needs `wgpu ^30.0`, `egui-winit` `winit ^0.30.13`), has first-party wgpu and winit
crates, and is pure Rust. The Rust Dear ImGui wgpu backends lag wgpu by several major versions and need the C++
cimgui build. Same programming model (a function a frame builds the UI), so nothing is lost for tools.

## What is built

`HORIZON_DEV_TOOLS=1 horizon-client …` shows a window over the world with the frame count and frame time.
`HORIZON_DEV_TOOLS_SCALE` sets egui points per pixel (1.0).

It is wired as a built-in trait mod: `dev_tools::mods()` gives `entry::main` a `Hooks` object when the variable is
set, its `frame` hook gathers numbers on the game thread, and its `RenderHooks::world_drawn` draws egui on the
render thread with the frame's device, queue, encoder and colour target, the renderer's own hook point for mods
([native-mods.md](native-mods.md#render-hook-points)). `egui_wgpu::Renderer` is made on the first frame for the
frame's colour format. The renderer and the window code are untouched; `egui-winit` is not a dependency yet.

Costs of that route, by design of the hook: the panel draws **before the first-person hand and the HUD**, so the
HUD overlaps it; there is **no pointer input** (the hook points give key events, mouse buttons and scrolling, not the
cursor position), so the panel is read-only for now; the scale is fixed by the variable.

## The plan

| Step | What | Why |
|---|---|---|
| 1 | A `hud_drawn` render hook point after the HUD and screens (a fragment, like `world_drawn`), and draw there | Tools over everything |
| 2 | Pointer input: a `cursor_moved` hook point (or the window's `CursorMoved` handed to `Hooks`), `egui-winit` for the translation, and a toggle key that releases the game's mouse grab while the tools are open | Clickable tools |
| 3 | Panels: the F3 numbers as live plots (frame time, memory, chunk and entity counts from `debug_entries`), a mod list with each mod's hooks and the time spent in them, a Lua console for [lua-bindings](lua-bindings.md) (`print` to the panel, run a line), an entity inspector (`native_mobs`, goals running, from [mob-ai-hooks](mob-ai-hooks.md)) | What a mod developer needs while the game runs |
| 4 | `--dev-tools` launch option and a registry entry (`registry/dev_tools.json`, kind `engine`) so the launcher's Mods page switches it | No environment variables for people |
| 5 | Build feature `dev-tools`, off in release profiles | A player's binary carries no egui |

Steps 1 and 2 are engine hook points: small, upstreamable, and useful to any mod that draws. Step 3 is where the
tools earn their keep.
