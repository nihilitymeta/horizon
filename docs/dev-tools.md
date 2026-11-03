# Developer tools (egui)

Status: the overlay, pointer input with a release key, the read-only entity windows (`/inspect`, `/goals`,
`/senses`, `/nav`, `/watch`, `/mobai log`) and the Lua console (`/lua`) are built
(`crates/horizon-client/src/dev_tools.rs`, `dev_tools/console.rs`, `mobs/observe.rs`, `game/commands/inspect.rs`,
`game/commands/lua_console.rs`); the rest is the plan. Branch `egui-tools` of whos-carmen/horizon, stacked on
`lua-bindings` (which sits on `mob-ai-hooks`): the console shows and reloads the scripted mods
([lua-bindings.md](lua-bindings.md)). For people working on mods and engine features, never on in a player's game.

**Only with the tools, and only the owner's.** The commands exist only in a game started with the tools (`--dev-tools` or
`HORIZON_DEV_TOOLS=1` call `vanilla::set_dev_tools_commands`; `tree_extra::register_dev_tools`), as `/pregen` exists
only with `--pregen`: they run Lua and the owner's commands, so no LAN world or dedicated server has them unless its
owner asked. The chat tree lists them under the same switch. The flag works in every build, release builds included:
there is no `cfg(debug_assertions)` or Cargo feature around the tools (a design call, not an oversight; the switch is
the flag, which only the person starting the game can give). Every command of theirs, `/inspect`, `/goals`, `/senses`,
`/nav`, `/watch`, `/mobai log` and `/lua` alike, is the owner's own (`Game::dev_tools_owner_only`, the acting
player against `Serving::owner`): a LAN guest of any permission level is refused with `horizon.dev_tools.owner_only`,
because the windows open on the host's screen, `/mobai log` writes on the host's disk and the console runs as the
host. The owner's own commands act through the hub as the owner and pass; a dedicated server has no owner, so its
console passes and every player is refused. `mc.run` takes a file name in the scripts folder and no path, and a chunk
that runs over two seconds is stopped by Luau's interrupt, so a typo's endless loop does not freeze the server thread.

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
meanwhile (chat, the pause menu) leaves it free; the key takes it back from anywhere, and so does closing the
tools' last window (`dev_tools::frame` tells `extras_frame` to grab). `HORIZON_DEV_TOOLS_SCALE` is
egui's zoom over the window's own scale; unset, it follows the window's height (1.0 below 1200 px, 1.5 up to 2000,
2.0 above).

**The windows.** Each command but `/lua` takes an entity selector (operators, like `/data`), opens one window per entity,
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
| `/nav <targets>` | Navigation kind, destination and speed, stuck, move control and movement input, the path's nodes, and **the path drawn in the world** as outlines of the blocks under the nodes (a node is where the feet go, usually air, and a rim takes the block's shape) through the `visuals` hook (`NativeBlockRim`): green the next node, yellow the six ahead, red the target (the game draws eight rims a frame, within 32 blocks). |
| `/watch <targets> <path>` | A numeric NBT path (`Health`, `Pos[1]`, `Brain.memories…`) sampled once a server tick, as a line chart of the last 1200 samples with min, max and last; hover for a sample. A path that is not a number says so. |
| `/mobai log <targets>` | Each server tick, the mob's AI snapshot (everything `/goals`, `/senses` and `/nav` show) as one JSON line in `dev-tools/logs/mobai-<name>-<id>-<time>.jsonl` under the game folder, for diffing offline; `/mobai log stop` closes the files. |

**The Lua console.** `/lua` opens it, takes the pointer for the tools and focuses the line (`/lua <code>` runs a
line in it): a Luau state on the server thread
(`game/commands/lua_console.rs`, mlua, built from source) that runs what the window queues
(`dev_tools/console.rs`) once a server tick, after the levels ticked. The window lists the `.lua` files of
`dev-tools/scripts/` under the game folder with a run button each, shows the output, and takes a line. The line
completes like the chat box: `Tab` takes the completion, `Up` and `Down` pick one; with nothing to pick, `Up` and
`Down` recall the lines run before, as a shell does (kept in `dev-tools/console-history.txt`). Inside an `mc.cmd("…")` string the
completions are the chat box's own, from the tree the server sent (`Dispatcher::client_suggestions` on the window's
game, asked once a frame through `dev_tools::console`); an entity argument (`mc.get("@e[ty`) completes as `/tp`'s
target does; a path argument offers common NBT keys and `mc.inspect`'s second argument its views; elsewhere they are
the `mc` names and Lua's keywords. Inside any call the popup shows its signature and an example. The `mc`
table is bound to the game for the chunk's duration (`Lua::scope`), so a script cannot keep the game past its run;
`mc` is bound afresh each run and each tick. An entity argument is a selector string (`"@e[type=pig,limit=1]"`) or
an id a selector found before (`mc.select` returns them); players have no id here, so reach them by selector in a
command.

| | |
|---|---|
| `mc.cmd(line)` | Runs a vanilla command as the world's owner (level 4, in the owner's level, no slash needed): `ok, lines` with the chat replies |
| `mc.select(sel)` | The entity ids the selector finds |
| `mc.get(entity[, path])` | The data `/data get entity` reads, as a Lua table (compounds are tables, lists sequences, numbers lose their NBT type); a path narrows it |
| `mc.snbt(entity[, path])` | The same as SNBT text, types kept |
| `mc.set(entity, path, value)` | `/data modify entity … set value`: `value` is SNBT text (`"20.0f"`, `"{NoAI:1b}"`) or a Lua value (integers are ints, other numbers doubles, booleans bytes, tables compounds or lists); the count changed |
| `mc.merge(entity, value)` | `/data merge entity`; the count changed |
| `mc.pos(entity)`, `mc.name(entity)` | Position as three numbers; the display name |
| `mc.inspect(entity[, view])` | Opens the window: `"data"` (default), `"goals"`, `"senses"`, `"nav"` |
| `mc.watch(entity, path)` | Opens a `/watch` chart |
| `mc.on_tick(fn)`, `mc.off_tick()` | Run `fn` every server tick until stopped (the window's button, `mc.off_tick()`, or an error in a handler) |
| `mc.run(file)`, `mc.scripts_dir()` | Run a script of the scripts folder; the folder's path |
| `print(...)`, `mc.log(msg)` | To the window; `mc.log` also to the game log |
| `mc.mods()` | The scripted mods ([lua-bindings.md](lua-bindings.md)): `id`, `source`, `scripts`, `reloads`, `handlers` (hook name to count), `goals`, `error` |
| `mc.reload_mods([id])` | The scripted mods (or one, by id) read their scripts again and run `init.lua` afresh at the next server tick, as `/reload` makes them do |

```lua
-- dev-tools/scripts/follow.lua: the nearest pig walks after the player.
local pig = mc.select("@e[type=pig,sort=nearest,limit=1]")[1]
mc.inspect(pig, "nav")
mc.on_tick(function()
  local x, y, z = mc.pos("@p")
  mc.cmd(string.format("execute as @e[type=pig,limit=1,sort=nearest] run tp @s %.1f %.1f %.1f", x, y, z))
end)
```

**The scripted mods in the console.** The window's first fold lists every Lua mod (`lua_mods::status`): its source,
how many scripts it has, how often it was reloaded, its handlers by hook and its goals, its last error in red (a
failed load or reload, or a handler's error, until the next reload) and a reload button (`lua_mods::request_reload`,
served at the start of the next server tick). A mod's `horizon.log` and `print` lines, and its errors, show in the
console's output beside the console's own, tagged `[mod_id]`. In `/goals`, a Lua goal is named `mod_id:name`.

The chat box completes and colours these like vanilla's commands: the tree a player gets is vanilla's commands
report plus `lan/commands.rs`'s `horizon_commands`, where Horizon's own commands must be listed. A guest sees them in
the tree (they are level 2 there, as `/data` is) but the server refuses the command itself, as above. Their chat
feedback is translated (`horizon.dev_tools.*` in `assets/horizon/lang/en_us.json`); the egui panels' own labels are
developer text and are not.

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
| 3 | ~~`/goals`, `/senses`, `/nav`, `/watch`, `/mobai log`~~ built (above); left: the F3 numbers as live plots, a mod list with each mod's hooks and the time spent in them,  brain species (villagers, piglins and wardens run `*_brain.rs`, not goals), block entities and storage as `/data` reads them | What a mod developer needs while the game runs |
| 4 | ~~`--dev-tools` launch option~~ built; a registry entry (`registry/dev_tools.json`, kind `engine`) so the launcher's Mods page switches it | No environment variables for people |
| 5 | Build feature `dev-tools` so a player's binary carries no egui (~~the commands registered only with the tools~~ built). Decided for now: no gate, the flag works in every build (above) | A smaller binary, not a safety switch: the flag already is one |
| 6 | Brains over the network: the server sends snapshots on a mod channel (`net_send_to_player`) so a remote server's mobs show too | Multiplayer |

Step 1 is an engine hook point: small, upstreamable, and useful to any mod that draws. Step 3 is where the tools
earn their keep.
