# Developer Tools

The developer tools are windows drawn with egui over the game: views of an entity's data, a mob's goals, senses and
path, charts of a value over time, and a Lua console that also shows and reloads scripted mods. They are for people
who work on mods and engine features, and are never on in a player's game.

The tools live on the `egui-tools` branch of whos-carmen/horizon. It is stacked on `lua-bindings`, which sits on
`mob-ai-hooks`, because the console shows and reloads the scripted mods ([lua-bindings.md](lua-bindings.md)).

| Part | Status |
|---|---|
| The overlay, and pointer input with a release key | built |
| The entity windows: `/inspect`, `/goals`, `/senses`, `/nav`, `/watch`, `/mobai log` | built |
| The Lua console, `/lua` | built |
| The inspect stick and the block window | built |
| The rest | [planned](#plan) |

The code is in `crates/horizon-client/src/dev_tools.rs`, `dev_tools/console.rs`, `mobs/observe.rs`,
`game/commands/inspect.rs` and `game/commands/lua_console.rs`.

## Turning the tools on

Start the game with `--dev-tools`, or set `HORIZON_DEV_TOOLS=1`:

```sh
horizon-client --dev-tools …
```

A launcher's game arguments can carry the flag. The tools draw nothing until one of the commands below names
something; then a window opens over the world for each entity it named.

A game with Lua mods has the windows without the flag, for the [inspect stick](#the-inspect-stick), and none of the
commands. The windows draw only while the stick opened one, and a game without Lua mods has no part of the tools.

### Who can use them

The commands exist only in a game started with the tools. `--dev-tools` and `HORIZON_DEV_TOOLS=1` call
`vanilla::set_dev_tools_commands` and `tree_extra::register_dev_tools`, the same way `/pregen` exists only with
`--pregen`. The commands run Lua and the owner's commands, so no LAN world or dedicated server has them unless its
owner asked for them. The chat's command tree lists them under the same switch.

The flag works in every build, release builds included. No `cfg(debug_assertions)` or Cargo feature guards the
tools. This is a design decision: the switch is the flag, and only the person who starts the game can give it.

Every command of the tools belongs to the world's owner alone (`Game::dev_tools_owner_only` compares the acting
player with `Serving::owner`). A LAN guest of any permission level is refused with `horizon.dev_tools.owner_only`,
because:

- the windows open on the host's screen
- `/mobai log` writes to the host's disk
- the console runs as the host

The owner's own commands act through the hub as the owner, and pass. A dedicated server has no owner, so its console
passes and every player is refused.

`mc.run` takes the name of a file in the scripts folder, and no path. A chunk that runs longer than two seconds is
stopped by Luau's interrupt, so an endless loop does not freeze the server thread.

### One world at a time

Everything the tools remember belongs to the process: the watched entities, the open AI log files, the chart's tick
count, the console's tick handlers and the ids its selectors found, and whether the tools hold the pointer. An entity
id means nothing outside its world. So closing a world clears it all (`Game::dev_tools_world_closed`, from
`finish_close_world`):

- the windows close, and the log files are closed
- the console window closes, and its handlers stop at the next server tick; its output and history stay
- the pointer goes back to the game

A watch set in one world never shows an entity with the same id in the next world.

## The pointer

Press F8 to free the mouse from mouselook, and again to give it back. Set `HORIZON_DEV_TOOLS_KEY` to a vanilla key
name, such as `key.keyboard.f9`, to use another key.

While the mouse is free, it belongs to the tools:

- You can drag, resize, scroll and click the panels.
- The window's own title bar and edges work.
- A click next to a panel does nothing, and the camera does not turn.

The keyboard stays the game's, so you can walk with the pointer free, except while a text field of the tools has
focus. `Game::capture` refuses to grab the pointer while the tools hold it, so a screen that opens and closes in the
meantime, such as the chat or the pause menu, leaves it free. The key takes the pointer back from anywhere, and so
does closing the tools' last window (`dev_tools::frame` tells `extras_frame` to grab it).

`HORIZON_DEV_TOOLS_SCALE` sets egui's zoom over the window's own scale. When it is unset, the zoom follows the
window's height: 1.0 below 1200 pixels, 1.5 up to 2000, and 2.0 above.

## The entity windows

Each command except `/lua` takes an entity selector, for operators, as `/data` does. It opens one window for each
entity. `… clear` closes that kind of window everywhere, and a window's **stop** button closes it alone. `/inspect`
with no arguments lists what is watched.

Players are watched by their connection and shown by their network entity id, so `/inspect @a` shows each
player's data as `/data get entity` prints it. All the windows are read-only: nothing touches the entity.

The snapshots are statics of the process, which the server thread fills. This works in single player. A dedicated
server fills snapshots that nobody shows, and a client of a remote server sees none. An entity that stops updating,
because it is gone, unloaded or the game is paused, keeps its last data, marked as stale.

| Command | Window |
|---|---|
| `/inspect <targets>` | Shows the data that `/data get entity` prints, read with the same `data_of` once each server tick (`game/commands/inspect.rs`, from the end of `tick_server`). The data is a tree: compounds and lists fold, and leaves show their SNBT value, such as `20.0f`, `1b` or `"minecraft:zombie"`. Keys come in the order `/data` prints them. Hovering a node shows its NBT path, which `/data get entity <target> <path>` takes. A filter box narrows the tree to matching keys, and a fold has the whole SNBT line. Each number has a **watch** button, which opens its `/watch` chart with that path: no path to type |
| `/goals <targets>` | Shows a mob's two goal selectors as tables, with each goal's priority, name, state (running or idle) and flags. A history strip for each goal marks where it ran over the last 1200 AI ticks; hover for that tick's running set. Taken after the mob's AI step (`mobs/observe.rs`, from `mobs/ai.rs`). The **log** button starts `/mobai log` for this mob, and the window names the file while it is written; **stop log** closes that file only |
| `/senses <targets>` | Shows what the mob perceives: its target, who hurt it last, its follow range, the nearest player and whether they are in range, its mood (aggressive, panicking, idle ticks), its home, its health, and this tick's line-of-sight checks (who, and whether they were seen) |
| `/nav <targets>` | Shows the navigation kind, destination and speed, whether the mob is stuck, its move control and movement input, and the path's nodes. It also draws the path in the world, as outlines of the blocks under the nodes, through the `visuals` hook (`NativeBlockRim`). A node is where the feet go, usually air, and an outline takes the block's shape. Green marks the next node, yellow the six after it, and red the target. The game draws eight outlines a frame, within 32 blocks |
| `/watch <targets> <path>` | Charts a numeric NBT path, such as `Health`, `Pos[1]` or `Brain.memories…`, sampled once each server tick, over the last 1200 samples, with the minimum, maximum and last value. Hover for a sample. A path that is not a number says so. There is no limit on how many charts are open; a chart's **stop** ends it alone |
| `/mobai log <targets>` | Writes the mob's AI snapshot, everything that `/goals`, `/senses` and `/nav` show, as one JSON line each server tick to `dev-tools/logs/mobai-<name>-<id>-<time>.jsonl` under the game folder, for comparing offline. `/mobai log stop` closes the files |

A button in a window can't do what only the game does, such as opening a file, from the render thread. It asks
(`observe::request`), and the game does it at the end of its next server tick (`dev_requests` in
`game/commands/inspect.rs`).

## The inspect stick

The modkit's item `horizon_modkit:inspect_stick` opens these windows on what it is used on, as vanilla's debug stick
changes block states. It copies the debug stick, so it is listed only with the operator items; `/give @s
horizon_modkit:inspect_stick` gets one. The modkit's scripts call `horizon.dev.inspect` from `entity_interact` and
`right_click_block`, and cancel the click, both hands' turns of it, so nothing else happens: no trade, no chest, no
button.

| Used on | Opens |
|---|---|
| An entity, the first time | Its Inspect window |
| The same entity again | Its next page: a mob has Inspect, Goals, Senses and Nav; any other entity, a player included, Inspect alone. Sneaking turns back. Both wrap around |
| Another entity | The same page, when it has it, else Inspect |
| A block | The block window |

Each use replaces the window the stick opened before, and a line over the hotbar says which page is open.

Who may use it:

- A player in creative with permission level 2 or more (op, or cheats on in single player): vanilla's
  `Player.canUseGameMasterBlocks`. `horizon.dev.inspect` answers false for anyone else, and the stick is any stick.
- The world's owner: the windows open on the host's screen. Another player's use is refused with
  `horizon.dev_tools.stick.host_only`.

The windows open the same way with and without `--dev-tools`. The stick's request goes through `observe::request`,
and the server thread handles it at the end of its tick (`stick_used` in `game/commands/inspect.rs`).

### The block window

The block window shows what the game knows of one block, read once each server tick in the block's level:

- the block's id and state properties
- its dimension and biome
- block light and sky light, at the block and on the face the stick clicked
- the power it receives (`getBestNeighborSignal`), and the strong signal it gives each way (`getDirectSignal`)
- whether it gets random ticks, and its scheduled block and fluid ticks with their delay and priority
- the block tags it is in
- for a content mod's block, the mod that declares it and its template
- its block entity's data as a tree, as `/data get block` prints it

While the window is open, the block's collision boxes are drawn in the world in blue, and its outline in yellow
(`debug_gizmos`).

## The Lua console

`/lua` opens the console, takes the pointer for the tools, and focuses the input line. `/lua <code>` runs one line
in it.

The console is a Luau state on the server thread (`game/commands/lua_console.rs`, mlua built from source). Once
each server tick, after the levels have ticked, it runs what the window queued (`dev_tools/console.rs`).

The window:

- lists the `.lua` files of `dev-tools/scripts/` under the game folder, each with a **run** button
- shows the output
- takes a line of code

### Completion and history

The input line completes as the chat box does: Tab takes the completion, and Up and Down pick one. With nothing to
pick, Up and Down recall the lines run before, as a shell does. The history is kept in
`dev-tools/console-history.txt`.

What the line completes depends on where the cursor is:

- inside an `mc.cmd("…")` string: the chat box's own completions, from the command tree that the server sent
  (`Dispatcher::client_suggestions` on the window's game, asked once each frame through `dev_tools::console`)
- an entity argument, such as `mc.get("@e[ty`: the completions of `/tp`'s target
- a path argument: common NBT keys; `mc.inspect`'s second argument: its views
- anywhere else: the `mc` names and Lua's keywords

Inside any call, a popup shows the function's signature and an example.

### The `mc` table

The `mc` table is bound to the game for the length of the chunk (`Lua::scope`), so a script cannot keep the game
after its run. It is bound again for each run and each tick.

An *entity* argument is a selector string, such as `"@e[type=pig,limit=1]"`, or an id that a selector found before
(`mc.select` returns them). Players have no id here, so reach them by selector, in a command.

| Function | Description |
|---|---|
| `mc.cmd(line)` | Runs a vanilla command as the world's owner: level 4, in the owner's level, with no slash needed. Returns `ok, lines`, with the chat replies |
| `mc.select(sel)` | Gets the entity ids that the selector finds |
| `mc.get(entity[, path])` | Gets the data that `/data get entity` reads, as a Lua table: compounds are tables, lists are sequences, and numbers lose their NBT type. *path* narrows it |
| `mc.snbt(entity[, path])` | Gets the same data as SNBT text, with the types kept |
| `mc.set(entity, path, value)` | Sets a value, as `/data modify entity … set value` does. *value* is SNBT text, such as `"20.0f"` or `"{NoAI:1b}"`, or a Lua value: integers become ints, other numbers doubles, booleans bytes, and tables compounds or lists. Returns the number of values changed |
| `mc.merge(entity, value)` | Merges data, as `/data merge entity` does. Returns the number of values changed |
| `mc.pos(entity)` | Gets the position, as three numbers |
| `mc.name(entity)` | Gets the display name |
| `mc.inspect(entity[, view])` | Opens a window: `"data"` (the default), `"goals"`, `"senses"` or `"nav"` |
| `mc.watch(entity, path)` | Opens a `/watch` chart |
| `mc.on_tick(fn)` | Runs *fn* every server tick, until the window's button, `mc.off_tick()` or an error in a handler stops it |
| `mc.off_tick()` | Stops the tick handlers |
| `mc.run(file)` | Runs a script from the scripts folder |
| `mc.scripts_dir()` | Gets the scripts folder's path |
| `print(...)` | Writes to the window |
| `mc.log(msg)` | Writes to the window and to the game log |
| `mc.mods()` | Gets the scripted mods ([lua-bindings.md](lua-bindings.md)), each with `id`, `source`, `scripts`, `reloads`, `handlers` (the count for each hook name), `goals` and `error` |
| `mc.reload_mods([id])` | Makes the scripted mods, or one of them by id, read their scripts again and run `init.lua` from the start at the next server tick, as `/reload` does |

This script, saved as `dev-tools/scripts/follow.lua`, makes the nearest pig walk after the player:

```lua
-- dev-tools/scripts/follow.lua: the nearest pig walks after the player.
local pig = mc.select("@e[type=pig,sort=nearest,limit=1]")[1]
mc.inspect(pig, "nav")
mc.on_tick(function()
  local x, y, z = mc.pos("@p")
  mc.cmd(string.format("execute as @e[type=pig,limit=1,sort=nearest] run tp @s %.1f %.1f %.1f", x, y, z))
end)
```

### Scripted mods in the console

The window's first section lists every Lua mod (`lua_mods::status`), with:

- its source, its number of scripts, and how often it was reloaded
- its handlers by hook, and its goals
- its last error, in red: a failed load or reload, or a handler's error, until the next reload
- a **reload** button (`lua_mods::request_reload`, served at the start of the next server tick)

A mod's `horizon.log` and `print` lines, and its errors, appear in the console's output next to the console's own,
tagged `[mod_id]`. In `/goals`, a Lua goal is named `mod_id:name`.

## Commands in the chat

The chat box completes and colours the tools' commands as it does vanilla's. The command tree that a player gets is
vanilla's commands report plus `horizon_commands` in `lan/commands.rs`, where Horizon's own commands must be listed. A
guest sees the commands in the tree, at level 2 as `/data` is, but the server refuses them, as described under
[Who can use them](#who-can-use-them).

The commands' chat feedback is translated (`horizon.dev_tools.*` in `assets/horizon/lang/en_us.json`). The labels in
the egui panels are developer text, and are not translated.

## Why egui

The game draws with wgpu 30.0.1 and winit 0.30.13, and has no immediate-mode UI library. egui 0.37 tracks exactly
those versions (`egui-wgpu` needs `wgpu ^30.0`, and `egui-winit` needs `winit ^0.30.13`), has its own wgpu and winit
crates, and is written in Rust. The Rust backends of Dear ImGui for wgpu are several major versions behind wgpu, and
need the C++ cimgui build. Both libraries work the same way, with a function that builds the UI each frame, so the
tools lose nothing.

## How it is wired

The tools are a built-in trait mod. `dev_tools::mods()` gives `entry::main` a `Hooks` object when the tools are on,
or when Lua mods are, for the inspect stick. A game with neither has no `Hooks` object, so the engine's paths for a
game without mods stay as they are.
Its `RenderHooks::world_drawn` draws egui on the render thread, with the frame's device, queue, encoder and colour
target, through the renderer's own hook point for mods ([native-mods.md](native-mods.md#render-hook-points)).

Input goes through `egui-winit` on the window thread. `Game::handle_window_event_inner` offers every window event to
`dev_tools::window_event` first, and `Game::extras_frame` hands the frame's `RawInput` to the render thread.
`egui_wgpu::Renderer` is made on the first frame, for the frame's colour format.

That route has costs, which come from the hook's design:

- The panels draw before the first-person hand and the HUD, so the HUD overlaps them.
- egui's cursor icons and clipboard are not wired: there is no resize cursor at a panel's edge, and no copying.

## Plan

| Step | What | Why |
|---|---|---|
| 1 | A `hud_drawn` render hook point after the HUD and screens, as a fragment like `world_drawn`, and draw there | Tools drawn over everything |
| 2 | egui's platform output: cursor icons and the clipboard. Pointer input and a toggle key are built | Tools that behave like the rest of the desktop |
| 3 | The F3 numbers as live charts; the time spent in each mod's hooks; species with a brain (villagers, piglins and wardens run `*_brain.rs`, not goals); block entities and storage, as `/data` reads them. `/goals`, `/senses`, `/nav`, `/watch`, `/mobai log` and the mod list are built | What a mod developer needs while the game runs |
| 4 | A registry entry (`registry/dev_tools.json`, kind `engine`), so the launcher's Mods page switches the tools. The `--dev-tools` option is built | No environment variables for people |
| 5 | A build feature `dev-tools`, so a player's binary carries no egui. For now the decision is no gate: the flag works in every build. Registering the commands only with the tools is built | A smaller binary, not a safety switch: the flag already is one |
| 6 | Brains over the network: the server sends snapshots on a mod channel (`net_send_to_player`), so a remote server's mobs show too | Multiplayer |

Step 1 is an engine hook point: small, possible to send upstream, and useful to any mod that draws. Step 3 is where
the tools are most useful.
