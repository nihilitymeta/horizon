# Developer tools (egui)

Status: the overlay, pointer input with a release key, and a read-only entity inspector (`/data get entity` as a
tree, plus a mob's goals) are built (`crates/horizon-client/src/dev_tools.rs`, `mobs/observe.rs`,
`game/commands/inspect.rs`, `/inspect`); the rest is the plan. Branch
`egui-tools` of whos-carmen/horizon, `main` + this one concern. For people working on mods and engine features,
never on in a player's game.

## Why egui, not Dear ImGui

The game draws with wgpu 30.0.1 and winit 0.30.13 and has no immediate-mode UI library. `egui` 0.37 tracks exactly
those versions (`egui-wgpu` needs `wgpu ^30.0`, `egui-winit` `winit ^0.30.13`), has first-party wgpu and winit
crates, and is pure Rust. The Rust Dear ImGui wgpu backends lag wgpu by several major versions and need the C++
cimgui build. Same programming model (a function a frame builds the UI), so nothing is lost for tools.

## What is built

`HORIZON_DEV_TOOLS=1 horizon-client …` (or `horizon-client --dev-tools …`, which a launcher's game arguments can
carry) draws nothing until a command below names something, then a window over the world for each entity it named.

**The pointer.** `F8` (`HORIZON_DEV_TOOLS_KEY`, a vanilla key name such as `key.keyboard.f9`) frees the mouse from
mouselook and gives it back. While it is free the mouse is the tools': the panels can be dragged, resized, scrolled
and clicked, the window's own title bar and edges work, a click beside a panel does nothing, and the camera does not
turn. The keyboard stays the game's (walking with the pointer free works) except while a text field of the tools
has focus. `Game::capture` refuses to grab the pointer while the tools hold it, so a screen opened and closed
meanwhile (chat, the pause menu) leaves it free; the key takes it back from anywhere. `HORIZON_DEV_TOOLS_SCALE` is
egui's zoom over the window's own scale; unset, it follows the window's height (1.0 below 1200 px, 1.5 up to 2000,
2.0 above).

**The windows.** Each command takes an entity selector (operators, like `/data`), opens one window per entity,
and `… clear` closes that kind everywhere; a window's `stop` closes it alone. `/inspect` alone lists what is
watched. Players are left out (their data is read as the acting player, who is nobody on a tick). All of it is
read-only: nothing touches the entity. The snapshots are process-wide statics the server thread fills, so this
works in single player; a dedicated server fills snapshots nobody shows, and a client of a remote server sees none.
An entity that stops updating (gone, unloaded, the game paused) keeps its last data, marked stale.

| Command | Window |
|---|---|
| `/inspect <targets>` | **The data `/data get entity` prints**, read with the same `data_of` once a server tick (`game/commands/inspect.rs`, from the end of `tick_server`), as a tree built from the compound: compounds and lists fold, leaves show their SNBT value (`20.0f`, `1b`, `"minecraft:zombie"`), keys come in the order `/data` prints them, hovering a node shows its NBT path (what `/data get entity <target> <path>` takes), a filter box narrows the tree to matching keys, and a fold has the whole SNBT line. |
| `/goals <targets>` | A mob's two goal selectors as tables (priority, goal, running or idle, flags) and their **history**: a strip per goal with a bar where it ran over the last 1200 AI ticks, hover for the tick's running set. Taken after the mob's AI step (`mobs/observe.rs`, from `mobs/ai.rs`). |
| `/senses <targets>` | What the mob perceives: its target, who hurt it last, follow range, the nearest player and whether it is in range, mood (aggressive, panicking, idle ticks), home, health, and this tick's line-of-sight checks (who, seen or hidden). |
| `/nav <targets>` | Navigation kind, destination and speed, stuck, move control and movement input, the path's nodes, and **the path drawn in the world** as block outlines through the `visuals` hook (`NativeBlockRim`): green the next node, grey walked, yellow ahead, red the target. |
| `/watch <targets> <path>` | A numeric NBT path (`Health`, `Pos[1]`, `Brain.memories…`) sampled once a server tick, as a line chart of the last 1200 samples with min, max and last; hover for a sample. A path that is not a number says so. |
| `/mobai log <targets>` | Each server tick, the mob's AI snapshot (everything `/goals`, `/senses` and `/nav` show) as one JSON line in `dev-tools/logs/mobai-<name>-<id>-<time>.jsonl` under the game folder, for diffing offline; `/mobai log stop` closes the files. |

The chat box completes and colours these like vanilla's commands: the tree a player gets is vanilla's commands
report plus `lan/commands.rs`'s `horizon_commands`, where Horizon's own commands must be listed.

**How it is wired.** A built-in trait mod: `dev_tools::mods()` gives `entry::main` a `Hooks` object when the
variable is set, and its `RenderHooks::world_drawn` draws egui
on the render thread with the frame's device, queue, encoder and colour target, the renderer's own hook point for
mods ([native-mods.md](native-mods.md#render-hook-points)). Input is `egui-winit` on the window thread:
`Game::handle_window_event_inner` offers every window event to `dev_tools::window_event` first, and
`Game::extras_frame` hands the frame's `RawInput` over for the render thread. `egui_wgpu::Renderer` is made on the
first frame for the frame's colour format.

Costs of that route, by design of the hook: the panels draw **before the first-person hand and the HUD**, so the
HUD overlaps them; egui's cursor icons and clipboard are not wired (no resize cursor at a panel's edge, no copy).

## The plan

| Step | What | Why |
|---|---|---|
| 1 | A `hud_drawn` render hook point after the HUD and screens (a fragment, like `world_drawn`), and draw there | Tools over everything |
| 2 | ~~Pointer input and a toggle key~~ built (above); left: egui's platform output (cursor icons, clipboard) | Clickable tools |
| 3 | ~~`/goals`, `/senses`, `/nav`, `/watch`, `/mobai log`~~ built (above); left: the F3 numbers as live plots, a mod list with each mod's hooks and the time spent in them, a Lua console for [lua-bindings](lua-bindings.md) (a stacked branch), brain species (villagers, piglins and wardens run `*_brain.rs`, not goals), block entities and storage as `/data` reads them | What a mod developer needs while the game runs |
| 4 | ~~`--dev-tools` launch option~~ built; a registry entry (`registry/dev_tools.json`, kind `engine`) so the launcher's Mods page switches it | No environment variables for people |
| 5 | Build feature `dev-tools`, off in release profiles; `/inspect` registered only then | A player's binary carries no egui and no `/inspect` |
| 6 | Brains over the network: the server sends snapshots on a mod channel (`net_send_to_player`) so a remote server's mobs show too | Multiplayer |

Step 1 is an engine hook point: small, upstreamable, and useful to any mod that draws. Step 3 is where the tools
earn their keep.
