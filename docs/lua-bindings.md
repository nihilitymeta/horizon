# Lua Scripting for Mods

This page describes how content mods carry Lua scripts that the game runs on its hook points. It covers the
runtime, the `registry` and `horizon` tables, every hook a script can listen to, and the limits a script runs
under.

Most mods are easier to write with [Horizon Modkit](https://github.com/whos-carmen/horizon-modkit), a library over
this API with classes and callbacks. This page is the layer under it.

| Part | Status |
|---|---|
| The runtime (`crates/event-horizon-lua/src/lua_mods.rs`) and scripts inside the `.hmod` | built |
| Mob goals in Lua (`lua_mods/goals.rs`) | built |
| Block behaviour: random and scheduled ticks, right clicks, block entities | built |
| Reloading on `/reload` | built |
| Limits: a time budget and a memory cap | built |
| What a gameplay mod asks for next | [not built yet](#not-built-yet) |

## Why Lua

Before this branch, a content mod (a `.hmod` with data and assets) that needed behaviour had one way to get it: a
native mixin bundle, Rust that the launcher compiles into the game ([native-mods.md](native-mods.md)). That is a
source patch with the player's rights, pinned to a Horizon commit, and two bundles can collide on the same function.
A team that makes content should not have to learn Rust, ship patches or pin commits to give a block a tick or a mob
a behaviour.

With this branch, scripts travel inside the content mod, next to `data/` and `assets/`, and the game runs them.
There is no compile step and nothing patches the game, so two scripted mods never conflict in the build. Rust stays
the way to add a hook point or to write something fast; Lua is the way to use the hook points.

## How it fits

Everything that is not a mixin shares one event surface: the hook points of `native-hooks.json`, dispatched to a list
of `Hooks` trait objects (`horizon_client::mod_api`). The Lua runtime adds one more `Hooks` implementation for each
scripted mod, in the same list and order as trait mods. A hook the generator covers reaches Lua under its native
name; a hook it cannot cover is listed with its reason in [hook-exposure.md](api/hook-exposure.md).

- The language is Luau, through `mlua` with the `luau` feature. It is built from source: no system library and no
  dynamic loading, the same on Metal and Vulkan. Each mod has one Lua state per side, each started from its entry
  point (`scripts/server/init.lua` on the server, `scripts/client/init.lua` on the client; `scripts/shared/` holds
  the modules either side may `require`). `Lua` is `Send`, so it lives in the
  `Hooks` object.
- Scripts never hold the host. Calls such as `horizon.broadcast(...)`, `horizon.summon(...)` and
  `horizon.kill_mob(...)` are queued. The runtime runs them in order when the handler returns, with the host that the
  hook was given. There is no unsafe code, and Lua never re-enters the game.
- An error in a script is logged with the mod's id, and never stops the game.

## Getting started

`cargo horizon-mod new my-mod --template lua` writes a scripted mod that depends on the modkit:

- a manifest
- `scripts/registry.lua`, with one item
- `scripts/server/init.lua`, with the item's `on_use`
- `scripts/client/init.lua`, with a client example (`client_tick_post`)
- `scripts/shared/util.lua`, a shared module required by both entries
- a `.luaurc` for an editor
- a README

`cargo horizon-mod run -- --dev-mod /path/to/horizon-modkit` runs it.

To play a mod, install its `.hmod` and turn it on, or start the game with `horizon-client --dev-mod /path/to/mod`.
The game gathers scripted mods when it starts, as it does trait mods, so turning one on or off on the Mods page takes
effect at the next start. A mod without capability `scripts` is content only, as before.

A mod runs one Lua state per side, each started from its entry point: `scripts/server/init.lua` on the
server, `scripts/client/init.lua` in the player's game (a mod needs one or both, or none as a shared-only library with only `scripts/shared/`). `scripts/shared/` holds the
modules either side may `require`. `horizon.side` says which side a state is, and a dedicated server runs no client
state. A mod with only the old `scripts/init.lua` is not loaded. The mod needs
capability `scripts` and `experimental = ["scripts"]` in its manifest, and its id comes from its
`horizon-mod.toml`. The folders decide what the mod runs on; its `side` line does not. `cargo horizon-mod build`
packs `scripts/` next to `assets/` and `data/`; `horizon_mods::Content` lists the scripts, checks their paths,
and checks the entry points. The Mods page says "May run its own scripts in your worlds".

A mod may pin the hook surface it was built for with `lua_hook_surface = 1` in its manifest. The engine's
surface version is `LUA_HOOK_SURFACE_VERSION` in `crates/event-horizon-lua/src/generated_hooks/mod.rs`, written
by `cargo run -p xtask -- lua-hooks` from the committed version and fingerprint in `xtask/src/lua_hooks.rs`
(the check fails with "hook surface changed" when the surface changes shape without a bump, so the stamp never
goes stale silently). A mod without the line loads as before; a mod whose number differs from the engine's is
not loaded, with `mod <id> was built for Lua hook surface <N>, this engine has <M>`. `cargo horizon-mod check`
reports the same error, since it loads the mod the way the game does.

This script counts natural zombie spawns:

```lua
-- scripts/server/init.lua
local spawns = 0
horizon.on("mob_spawned", function(event)
  if event.kind == "minecraft:zombie" and event.natural then
    spawns = spawns + 1
    horizon.log(("zombie %d at %.0f %.0f %.0f"):format(spawns, event.pos.x, event.pos.y, event.pos.z))
  end
end)
```

## Libraries

A mod lists the mods it needs in its manifest, such as `depends = ["horizon_modkit"]`. The game loads those first,
and does not run a mod whose dependency is missing or turned off. The log says which one.

An entry can ask for an oldest version, such as `"horizon_modkit >= 0.7"`. The game does not load a mod whose
dependency is older, and the log says what it found.

`require("modkit")` looks for `scripts/modkit.lua`, or `scripts/modkit/init.lua`, first in the mod's own scripts and
then in each dependency's, in the manifest's order. A dotted name is a folder: `require("modkit.entity")` is
`scripts/modkit/entity.lua`. The form `require("@alias/path")` works too, with the alias from a `.luaurc`.

A library's code runs in the state of the mod that requires it, as a Lua module does. The library's own `init.lua`
runs in the library's state only. `/reload` reads the libraries again too.

[whos-carmen/horizon-modkit](https://github.com/whos-carmen/horizon-modkit) is the library meant for this: entities,
blocks and goals as classes with callbacks.

## Declaring content: `scripts/registry.lua`

A scripted mod can declare its entity types, blocks, items, mob effects, creative tabs and menu types in
`scripts/registry.lua`, instead of files. It can also write any other data or asset JSON there, such as loot
tables, recipes, tags, lang files and models. Textures stay files. The mod needs capability `registries` as well as
`scripts`.

The script runs when the game reads the mod's content: at start, and again whenever the game builds its content
root while running. That happens on a resource pack reload, and on a `/reload` in builds that layer the mods' data
again. What the script declares is fixed at start. A later run can write different data and assets, but a changed,
new or missing declaration is set aside, with a log line that says a restart applies it ([Reloading](#reloading)).

The script runs in a Luau state of its own, with nothing but the `registry` table, a five-second budget and
64 MiB. It has no `loadstring` and no files. Its `require` reaches only the mod's registry modules, in
`scripts/registry/`.

### Splitting the registry into modules

A mod that adds a lot can keep its declarations in several files. Put them in `scripts/registry/`, and require
each one from `scripts/registry.lua`, in the order they should run:

```lua
-- scripts/registry.lua
require("@registry/items")   -- scripts/registry/items.lua
require("@registry/blocks")  -- scripts/registry/blocks.lua, or scripts/registry/blocks/init.lua
```

- `@registry/items`, `registry/items` and `registry.items` name the same module.
- Each module runs once. A second `require` returns the value that the module returned, or `true` if it
  returned nothing.
- Modules run in the same state as `scripts/registry.lua`, with the same `registry` table, budget and memory.
- A missing module, a name outside `scripts/registry/`, and a module that requires itself on the way are
  errors that name the module.
- The mod's other scripts can't require a registry module. Those modules run once, when the mod is read.

For luau-lsp to find the modules, add the alias to `.luaurc`: `"registry": "scripts/registry"`.
`cargo horizon-mod new --template lua` writes it.

The launcher runs the script the same way when it inspects a mod that it installs, so a declaration error shows
there first. The registry server and its CI are built without the `lua` feature: they list the mod's files only, and
don't run the script.

What the script registers becomes content files in memory. They are checked and read exactly like the files in
`data/` and `assets/`, so the rest of the game never knows the difference. A path that is both in the mod and
registered is an error, as is anything that the file would have been refused for.

| Member | Description |
|---|---|
| `registry.id`, `registry.namespaces` | the mod's id, and the namespaces it owns |
| `registry.entity_type(name, def)`, `.block`, `.item`, `.mob_effect`, `.creative_tab`, `.menu` | Declares `data/<id>/horizon/<kind>/<name>.json`. *def* is the file's JSON as a table ([mods.md](mods.md)); *name* may hold `/` |
| `registry.data(name, value)` | Writes `data/<ns>/<name>.json`. The namespace comes from the name, or is the mod's id: `"loot_table/entities/crawler"`, `"minecraft:tags/block/mineable/hoe"` |
| `registry.asset(name, value)` | Writes `assets/<ns>/<name>.json`: `"lang/en_us"`, `"models/block/hive"`, `"blockstates/hive"` |
| `registry.file(path, value)` | Writes any content path. *value* is a table, written as JSON, or a string, written as it is |
| `registry.lang(locale, entries)` | Adds names to `assets/<id>/lang/<locale>.json`. Every call for a locale, from any module, goes into the same file. A key given twice is an error that names both calls. *locale* is a lang file name, such as `"en_us"` |

A table whose keys are 1 to n becomes a JSON array, and any other table an object; `{}` is an empty object. A whole
number becomes an integer.

Use `registry.lang` for names when the registry is split into modules, so that each module names what it adds:

```lua
-- scripts/registry/items.lua
registry.item("wand", { template = "minecraft:stick" })
registry.lang("en_us", { ["item.my_mod.wand"] = "Wand" })
```

A locale that `registry.lang` writes can't also come from `registry.asset("lang/<locale>")`, `registry.file`, or a
`lang/<locale>.json` file of the mod. Use one of them for each locale.

Two things a mob needs are data rather than script:

- A spawn egg is an item whose id ends in `_spawn_egg` and names the kind:
  `registry.item("crawler_spawn_egg", { template = "minecraft:zombie_spawn_egg" })`, with its own texture and model
  under `assets/`.
- Natural spawning is a NeoForge biome modifier:
  `registry.data("neoforge/biome_modifier/crawlers", { type = "neoforge:add_spawns", biomes = "#minecraft:is_overworld", spawners = { { type = registry.id .. ":crawler", weight = 20, minCount = 1, maxCount = 3 } } })`.
  It adds the mob to the natural spawns of those biomes, in its kind's category; for a mod's mob, that is its
  template's. `category` chooses another.

Both are read when the game builds its content, so they need a restart, not a `/reload`. A mob's look is data too:
`model` (`registry.asset("models/entity/<name>", { texture_size, parts })`), `animation` (a set by name) and
`sounds` on its entity type ([mob-ai-hooks.md](mob-ai-hooks.md)).

```lua
-- scripts/registry.lua
registry.entity_type("crawler", {
  template = "minecraft:zombie",
  attributes = { max_health = 30, movement_speed = 0.3 },
  texture = registry.id .. ":entity/crawler",
  goals = "none",
})
registry.data("loot_table/entities/crawler", {
  type = "minecraft:entity",
  pools = { { rolls = 1, entries = { { type = "minecraft:item", name = "minecraft:slime_ball" } } } },
})
```

## Hook points

[docs/api/hook-points.md](api/hook-points.md) lists every hook point and host helper of the engine, with its API
level. For each hook that a scripted mod hears, it names the Lua handler, and for each helper that the runtime calls,
the `horizon.*` function behind it. A helper with an empty Lua column is one that scripts can't reach yet.

The page is generated by `cargo run -p xtask -- hooks-doc` from `native-hooks.json` and the hook functions' own
docs, and a test keeps it current. It is the place to look for what the hooks below are built on.

### Generated helpers

Some helpers are not hand-written: `lua-helpers` generates their Lua bridge from the `native_*` signatures
(`crates/event-horizon-lua/src/generated_helpers.rs`, and its tests in `generated_helpers_tests.rs`), the same way
`lua-hooks` generates the hooks — `&self` helpers become reads and `&mut self` helpers of unit/bool return become
queued actions, with the saturating casts and finite checks the hand-written ones spell out. Every generated helper
is emitted except the denylist [`EXCLUDED`](../xtask/src/lua_helpers.rs) in `xtask/src/lua_helpers.rs` (dev/diagnostic,
client-side, and unclear helpers, each with its reason). [helper-exposure.md](api/helper-exposure.md) lists every
helper's class with its reason and the blocking types; `hook-points.md` marks the enabled ones `generated`.
Entities are named by their network id, the id the hook events carry: the `entity_*` helpers take it as a number,
resolved against the world (a read with no such entity answers nil, an action does nothing). Positions are
`{x, y, z}` tables: a `[f64; 3]` component must be finite, and a `[i32; 3]` component that is fractional or out of
range is an error (it does not saturate, unlike a plain integer argument). Levels are named by their dimension id
string (`"minecraft:overworld"`, `"minecraft:the_nether"`): the `level_*` helpers take it as a string, resolved
against the loaded dimensions (a read with an unknown dimension answers nil, an action does nothing, the same
rule as an unresolved entity or player). A `Direction` is one of its six lowercase names (`"down"`, `"up"`,
`"north"`, `"south"`, `"west"`, `"east"`); any other value is an error. A string list reads as a sequence of
strings (`{"a", "b"}`, in the host's order; an empty list is `{}`, and an optional one is nil for `None`). A plain pair reads as a positional
sequence `{a, b}`, or as a named table where a sequence would lose a nil second element: the optional climate and
profile reads return the pair or nil — e.g. `level_height` returns `{min = ..., max = ...}` and
`player_textures` returns `{value = ..., signature = ...}` (the signature is nil when unsigned) — and a
compass target or profile property takes the pair or nil: `set_compass_target` as the sequence
`{dimension, {x, y, z}}`, `set_profile_property` as the named `{value = ..., signature = ...}`. An item is the
`Item` table (`{id, count, damage, extra, plain}`): `count` must be an integer in 1..=99, `damage` a
non-negative integer, `id` a string, and any other key an error. An NBT table is a `Data` table. An equipment
slot is one of its eight lowercase names (`"mainhand"`, `"offhand"`, `"head"`, `"chest"`, `"legs"`, `"feet"`,
`"body"`, `"saddle"`), and an optional block state a block-state name string or nil. `docs/api/helper-exposure.md` lists every helper's class; the denylist of generated
helpers that scripts never see (`EXCLUDED` in `xtask/src/lua_helpers.rs`) is where the client-side and
diagnostic exclusions live. The two placements of issue 52's round 3: `mod_key` reads the client's key
bindings, so it is a client helper; `text_allowed` asks the server's chat filter, so it stays server-side.
`setting` stays out (it joins `mod_id` and `key` onto the data dir with no validation, so a script could read
any file with `../`; `horizon.config()` is the Lua way to keep settings) and `run_mod_command` stays out (it
runs a console command; `run_command_as` already covers that). Regenerate with
`cargo run -p xtask -- lua-helpers` (its `--check` fails when any of the five outputs — the bridge, its tests, the
exposure page, `hook-points.md` and `horizon.d.lua` — is stale).
The helpers in `BOUND_MOD_ID` (the same file) take the calling mod's id from the runtime, not from the script: their
first parameter `mod_id` is left out of the Lua signature. Today those are the scoreboard writes (objectives, scores, teams and display slots).

## The `horizon` table

Calls that act on the world are *actions*: they are queued, and run after the handler returns. Calls that read the
world are *reads*: they answer at once, from the hook's host, for the length of the call. Both need a hook with a
host. `living_incoming_damage`, `living_fall`, `living_heal`, `living_knockback`, `mob_ai_register` and goal
functions have no host, among others; a goal reads its `mob` view instead. A level hook (`random_tick`, `tick_block`) reads blocks
only.

An action called where no hook's handlers run is an error at the call. This covers a goal function, which acts
through its `out` table, and the top level of `init.lua`, which registers handlers. Nothing would run the action,
and it would otherwise wait in the queue for the life of the state.

Some functions take a *player*. A player is a `player` table that a hook handed out, a name or a UUID.

A client state (`scripts/client/`) keeps only a small `horizon` table: `horizon.mod_id`, `horizon.mod_version`, `horizon.side`, `horizon.libraries`, `horizon.log`, `horizon.on`, `horizon.after`, `horizon.every`, `horizon.ticks`, `horizon.config`, `horizon.save_config`, `horizon.dev`, `horizon.distance`, `horizon.handled`, `horizon.cancel`, `horizon.require`, the reads `horizon.player` and `horizon.real_time` (in every client hook that has a host, not only the draw hooks) and the queued action `horizon.net_send_to_server`. A client state also keeps the generated client helpers ([Client helpers](#client-helpers)): reads like `horizon.has_shift_down()` and actions like `horizon.flame_particle(pos, soul)`, generated from the client natives (`GENERATED_CLIENT_READS`, `GENERATED_CLIENT_ACTIONS`, issue 50). They run like `horizon.player`/`real_time` (reads answer from the hook's host) and `horizon.net_send_to_server`/`horizon.marker_set` (actions queue and run after the handler), and a server state stubs each with `horizon.<name> is only available in scripts/client/ (this is the server state of mod <id>)` — the mirror of the server-only error. Every other `horizon.*` entry present on a server state (including `horizon.set_block`, `horizon.summon`, `horizon.players`, `horizon.world_data`, `horizon.player_data` and `horizon.entity_data`) is replaced by a stub that raises `horizon.<name> is only available in scripts/server/ (this is the client state of mod <id>)`. The exception is a draw-hook handler (`hud`, `gui_layer_post`), which may still call the read functions (`horizon.time`, `block`, `block_property`, `light`, `surface_height`, `raycast`, `players`, `acting_player`, `mob`, `mobs`, `inventory`, `count_items`, `carried_items`, `data`) and the two queued chat actions `horizon.tell` and `horizon.broadcast`; every other server-only call — queued actions such as `horizon.set_block` or `horizon.run_command`, and declarations such as `horizon.command`, `horizon.goal` and `horizon.block_entity` — raises the same server-only error even inside a draw hook. (`horizon.world_data`, `horizon.player_data`, `horizon.entity_data` and `horizon.summon` are refused in a draw hook as before, with the draw-hook error.)

### Client helpers

The client helpers are the generated helpers a client state (and only a client state) has: the reads the bridge emits into `GENERATED_CLIENT_READS` and the actions in `GENERATED_CLIENT_ACTIONS`. Reads answer from the hook's host, as `horizon.player` does; actions queue and run after the handler, as `horizon.marker_set` does. In a draw hook (`hud`, `gui_layer_post`), a client read answers through the shared host as `horizon.player` does, while a client action raises `horizon.<name>: not available in draw hooks (hud, gui_layer_post)` at the call, as a marker call does. A server state's calls raise the client-only error above. The full list, with each helper's side and Lua signature, is in `docs/api/helper-exposure.md` (the "Generated, enabled" table).

### The mod

| Function | Description |
|---|---|
| `horizon.on(name, fn[, kind])` | Registers a handler for a hook. The game refuses a *name* that it does not know when the script loads, with the line; the names are the generated hooks (`GENERATED_HOOK_NAMES`). The [Hooks](#hooks) tables below describe the main ones. With *kind* (`"minecraft:zombie"`, `"zombie"`, or a mod's own id), the handler runs only for that kind, after the handlers for every kind. A kind that no handler watches costs nothing |
| `horizon.log(msg)` | Writes a line to the game log, tagged `[lua:<id>]` |
| `horizon.mod_id`, `horizon.mod_version` | the mod's id and version, from its manifest |
| `horizon.libraries` | the versions of the mods it depends on, by id, such as `horizon.libraries.horizon_modkit` |
| `require(name)` | Looks in the state's side folder (`scripts/server/<name>.lua` in a server state, `scripts/client/<name>.lua` in a client state) then `scripts/shared/<name>.lua`, then each library's `scripts/`, once, and returns its value |
| `horizon.ticks()` | Gets the number of ticks of the state's own side (server ticks in a server state, client ticks in a client state) seen since the scripts loaded |
| `horizon.distance(a, b)` | Gets the distance between two `{x, y, z}` tables |

### Mob goals

| Function | Description |
|---|---|
| `horizon.goal(name, def)` | Defines a mob goal. *def* has `flags` (any of `"move"`, `"look"`, `"jump"` and `"target"`), `every_tick`, `interruptable`, and the functions `can_use(mob, state, out)` (required), `can_continue`, `start`, `tick` and `stop`. *mob* is the [mob view](#the-mob-view), *state* a table of the goal's own that lives with the mob, and *out* the [goal actions](#goal-actions). A def is plain data: its fields are read without consulting a metatable, so a field only `__index` would supply counts as missing. Call it at load |
| `horizon.add_goal(priority, name)`, `horizon.add_target_goal(priority, name)` | Adds a defined goal to the mob's goal selector or target selector. In `mob_ai_register` or `register_goals` |
| `horizon.clear_goals()`, `horizon.clear_target_goals()` | Removes every goal from a selector. In `mob_ai_register` or `register_goals` |
| `horizon.set_body_rotation(mode, rate)` | Sets how a stationary mob's body follows its head. In `mob_ai_register` or `register_goals`. *mode* is `"vanilla"` (the default ramp), `"smooth"` (the body turns towards the head by at most *rate* degrees a tick, default `5`) or `"frozen"` (the body never turns). *rate* stays within `0.1` to `90` |

```lua
horizon.goal("stalk", {
  flags = { "move", "look" },
  can_use = function(mob) return mob.nearest_player and horizon.distance(mob.position, mob.nearest_player) < 24 end,
  tick = function(mob, state, out)
    local p = mob.nearest_player
    out.look_at(p.x, p.y + 1.6, p.z)
    if mob.navigation_done then out.navigate_to(p.x, p.y, p.z, 1.1) end
  end,
})
horizon.on("mob_ai_register", function(event)
  local mob = event.mob
  if mob.kind == "minecraft:zombie" then horizon.add_goal(3, "stalk") end
end)
```

A Lua goal is a `NativeGoal` over a `LuaBehaviour` ([mob-ai-hooks.md](mob-ai-hooks.md)). The game copies the mob
into the view before each call, and applies the actions after it, so a script never holds the mob. A goal function's
error is logged once at error level, then at debug level, and reads as "no".

#### The mob view

The `mob` that a goal function receives:

| Field | Description |
|---|---|
| `id`, `kind` | the entity id and the type's id |
| `position`, `velocity` | `{x, y, z}` |
| `yaw`, `head_pitch`, `health`, `max_health`, `age` | its state |
| `on_ground`, `in_water`, `in_lava`, `hurt_time`, `last_hurt_by` | its state |
| `target` | `"player"`, a mob id, or nil |
| `target_position`, `target_in_melee_range` | where the target is, and whether the mob can hit it |
| `navigation_done` | true when no path is being walked |
| `nearest_player` | the nearest player's position, or nil |
| `home` | `{x, y, z, radius}`, or nil |
| `nearby` | the mobs within the follow range, nearest first: `{id, kind, position, distance, health, alive, hostile, baby, target}` |

#### Goal actions

The `out` table of a goal function. Its actions run in order after the call:

- `navigate_to(x, y, z, speed)`, `navigate_to_target(speed)`, `stop_navigation()`
- `move_toward(x, y, z, speed)`, `look_at(x, y, z)`, `look_at_target()`, `jump()`
- `set_target(who)`, `attack(who)`, `swing()`, `hurt(who, amount)`
- `convert(kind)`: replaces this mob with one of *kind*, keeping its equipment and name
- `summon(kind, x, y, z)`, `despawn()`, `heal(amount)`, `effect(name, ticks, amplifier)`

### Reading the world

| Function | Description |
|---|---|
| `horizon.block(x, y, z)` | Gets the id of the block at a position in the level in place; `minecraft:air` where nothing is loaded |
| `horizon.block_property(x, y, z, key)` | Gets one state property of that block, or nil |
| `horizon.mob(id)` | Gets a living mob's view, as a goal sees it but without `nearby` and `target_position`, or nil |
| `horizon.mobs()` | Gets the views of every living mob of the level |
| `horizon.players()` | Gets the players of the world in join order, as `player` tables. A single-player world's owner is among them, under the UUID that the hooks use |
| `horizon.player([name_or_uuid])` | Gets one player by name or UUID, or nil. On a client state, with no argument, the window's own player instead (see below); a server state's argument stays required |
| `horizon.acting_player()` | Gets the player whose action or tick the hook is hearing, as a `player` table, or nil. In a hook fired for one player — `player_tick`, the click, use, break, place, attack, hurt, interact, inventory-tick and use-item hooks, the craft, pickup and drop hooks — it is that player. It is nil in hooks with no acting player: `player_chat`, `player_command`, `perform_console_command` (the dedicated-server console hook), `server_tick`, `world_lifecycle`, `entity_tick_post`, `player_died` and the player's `living_death`, hostless hooks, level hooks, and draw hooks on a client. A single-player owner's console command runs as that owner, through `perform_command` as their own command would |
| `horizon.time()` | Gets `{dimension, day_time, day, time_of_day, rain, thunder}`: the clock in ticks, `day` and `time_of_day` (0 to 23999) from it, and the rain and thunder levels from 0 to 1. The **game clock**: `/tick freeze` holds it, like vanilla |
| `horizon.real_time()` | Gets `{seconds, delta}`: the wall clock, monotonic seconds since the game started and the last frame's wall time. It never freezes; see below for which clock a HUD wants |
| `horizon.raycast(player, distance)` | Gets the first block within *distance* (64 by default, 1024 at most) along the player's look: `{id, x, y, z, block = {x, y, z}}`, the point hit and the block, or nil |
| `horizon.light(x, y, z)` | Gets `{block, sky}` light at the position, from 0 to 15; both are 0 where nothing is loaded |
| `horizon.surface_height(x, z[, heightmap])` | Gets the column's height by `world_surface` (the default), `motion_blocking`, `motion_blocking_no_leaves` or `ocean_floor`; nil where nothing is loaded |
| `horizon.data(path)` | Reads a data file of the game's resources, content mods' included (`data/my_mod/spawns.json`): JSON as a table, any other file as a string, nil when there is none |
| `horizon.dev.inspect(who, what)` | The inspect stick: opens the developer tools' windows for *who* on *what*, an entity id or a block `{x, y, z, face}` (*face* a unit vector out of the block; *dimension* defaults to the player's). Answers true when the click is the stick's, a player in creative with permission 2 or more, and false otherwise. The server opens the window or turns its page at the end of the tick, on the host's screen only ([dev-tools.md](dev-tools.md#the-inspect-stick)) |
| `horizon.inventory(player)` | Gets the player's 36 slots by number, 1 to 36 with the hotbar first, as `{id, count, damage, plain}`; an empty slot is absent |
| `horizon.count_items(player, id)` | Gets how many of a plain item the player carries |
| `horizon.carried_items(player)` | Gets every plain item the player carries, with how many, by id |

The three inventory reads return nil for a player who is not there, and, inside another player's hook, for anyone
but that player.

### Acting on the world

| Function | Description |
|---|---|
| `horizon.broadcast(text)` | Sends a chat line to every player |
| `horizon.summon(dimension, kind, x, y, z)` | Makes an entity, as `/summon` does. In a hook with a host, the entity is made at once and its id returned, or nil when it could not be. In a level hook it is made right after the level's pass, in that level, and nothing is returned |
| `horizon.kill_mob(dimension, id)` | Kills a mob, as `/kill` does |
| `horizon.remove_mob(dimension, id)` | Removes a mob, with no death and no loot |
| `horizon.set_mob_health(dimension, id, health)` | Sets a mob's health, up to its maximum |
| `horizon.push_mob(dimension, id, x, y, z)` | Adds to a mob's velocity |
| `horizon.teleport_mob(dimension, id, x, y, z)` | Moves a mob, as `/tp` does |
| `horizon.set_block(x, y, z, id[, props][, flags])` | Sets a block in the level in place, as `/setblock` does. *id* gets `minecraft:` when it has no namespace; *props* holds the state properties that differ from the default (`{ axis = "x" }`); *flags* are those of `Level.setBlock`, 3 by default, which tells the neighbours and the clients. A block entity block gets its block entity. An id or property that the game does not have is an error of the handler |
| `horizon.fill(x1, y1, z1, x2, y2, z2, id[, props])` | Fills a box with one state, as `/fill ... strict` does, straight into the chunks. No neighbour is told, and the chunks go to the players whole. At most 256 by 256 columns, at any height; a chunk that is not loaded is left out |
| `horizon.drop_item(x, y, z, id[, count])` | Drops an item entity of *count* (1) of a plain item at the point |
| `horizon.falling_block(x, y, z, id[, vx, vy, vz])` | Throws a falling block from the point (its bottom centre) at that velocity, in blocks per tick. It lands and places itself as sand does |
| `horizon.run_command(line)` | Runs a command as the server console does: permission level 4, from the overworld's spawn, with no output. A leading slash is dropped. See below for where it works |
| `horizon.run_command_as(player, line[, callback])` | Runs a command as the player typing it would (their source, operator level and position), and calls `callback(result)` with what it did. A leading slash is dropped. See below |
| `horizon.sound(name, x, y, z[, volume, pitch])` | Plays a sound event for the players in range, 16 blocks or more with a volume above 1, as `/playsound` does |
| `horizon.particles(name, x, y, z[, count, spread, speed])` | Shows *count* particles of a simple type (`minecraft:flame`, `minecraft:happy_villager`) spread by *spread* blocks, for the players within 32 blocks, as `/particle` does |

`horizon.run_command` and `horizon.set_nickname` work only where no player's state is swapped in: a timer,
`server_tick`, a mob or block tick, a death. Inside a player's own hook, such as a click, a command or a pickup,
nothing runs, and the log says so at debug level. Queue the call there with
`horizon.after(1, function() horizon.run_command(...) end)`. `run_command` is the way to reach what the API has no
function for yet, such as the time, the weather, `/tp` of a player and game rules.

### Running a command as a player

`horizon.run_command_as(player, line[, callback])` (server state) runs `line` as the player typing it would:
*player* is the uuid text or `player` table the other player actions take, and a leading `/` is dropped.
The command runs in queue order when its turn comes, like every other action; `callback` is an optional
function. With no callback the result is dropped and the action is `horizon.run_command` for one player.

With a callback, its result is collected and the callback runs after the whole queue of that firing has
been applied, in the order the commands ran, as `callback(result)` where `result` is
`{ ran = boolean, result = integer, output = { string, ... }, output_components = { component, ... } }`:
`ran` says whether it ran without failing, `result` is vanilla's command result, `output` holds one plain
string per chat line the command printed and `output_components` the matching component table per line
(the rich-text convention: a string plus a component table). A player who is not on the server runs
nothing: the callback still gets `ran = false` with empty output. A callback that is not a function is a
script error naming the argument.

Actions a callback queues run in a follow-up drain of the same firing, which may collect more callbacks.
The chain is capped at 8 follow-up drains: past it the remaining callbacks are dropped with one logged
error naming the mod. The same cap of 8 also bounds a Lua-handled command that queues `run_command_as`
of itself: the nested commands run synchronously inside the draining call, so the 9th nested ask does not
run, its callback is delivered the empty result, and the same one-line error is logged. A callback that
raises is reported (`run_command_as callback`) and stops neither the other callbacks nor the rest of the
firing. Each callback runs inside the same per-handler time budget as the firing's handlers (a quarter
second): a callback that runs past it is stopped and logged, and the next callback still runs.

```lua
horizon.run_command_as(event.player, "/say hello", function(result)
  if result.ran and result.output[1] then
    horizon.tell(event.player, "The command said: " .. result.output[1])
  end
end)
```

### Mod messages

`horizon.net_send_to_player(player, channel, data)` (server state) sends *data* on *channel* (`mod_id:name`, at most 30 000 bytes, at most 32 messages to one player per tick) to the player's Horizon client; `horizon.send_custom_payload_to_player(player, channel, data)` sends it on a plugin channel (`namespace:path`, outside Horizon's own `horizon:` ones, at most 1 MiB) to any client, a vanilla one included. `horizon.net_send_to_server(channel, data)` (client state) sends *data* on *channel* to the server (the same caps as `net_send_to_player`, 32 a tick), and fails there until the player has joined, and when the server carries no mod messages. *player* is the uuid text a hook event gives (such as `event.player` of `net_from_client`); *channel* is a string; *data* is a Lua string carrying arbitrary bytes, NUL and high bytes included, so what one side sends is what the other side's `event.data` reads in `net_from_client` or `net_from_server`, byte for byte. The natives refuse a bad channel, a past-cap payload or a wrong side: the refusal is reported and the message is not sent, and it does not cancel the rest of the handler's queued actions. All three queue like the other actions and apply after the handler, on either side's hooks and timers alike; a client draw hook may queue the client send, as it may queue `horizon.tell` and `horizon.broadcast`.

### World markers

A marker is something a client mod keeps drawn in the world: a box, a line, a beam, text or an icon at a world
position, drawn every frame until the mod moves or removes it or its lifetime runs out. Unlike
`event.batch` draws, a marker is set once and the engine keeps drawing it. Markers are the client
state only (`scripts/client/`): a server state's marker calls raise `horizon.<name> is only
available in scripts/client/`. They are queued actions like `net_send_to_server`: call them in a
client hook that runs actions (a client tick, a key handler, a timer), never at load and never in
a draw hook (`hud`, `gui_layer_post`), whose only queued actions are `horizon.tell` and
`horizon.broadcast`; a marker call there raises `horizon.<name>: not available in draw hooks
(hud, gui_layer_post)`.

| Function | Description |
|---|---|
| `horizon.marker_set(id, spec)` | Creates or replaces the marker `id` of this mod, from the table below |
| `horizon.marker_move(id, pos)` | Moves the anchor of this mod's marker `id` to the `{x, y, z}` `pos`; shape, colour and the lifetime it has left stay |
| `horizon.marker_remove(id)` | Stops drawing this mod's marker `id` |
| `horizon.marker_clear()` | Stops drawing every marker of this mod |

`id` is a string of 1 to 64 characters (a longer or empty id is a script error). The owner is
always the mod's own id (`horizon.mod_id`), never the script's: a script only ever sees its own
markers. A mod id longer than 64 characters is refused by the native (see below), so its markers
are never drawn.

The `spec` table of `marker_set` holds exactly the fields of its `kind`, plus the shared ones:

| Key | Kinds | Meaning (default) |
|---|---|---|
| `kind` | all | `"box"`, `"line"`, `"beam"`, `"text"` or `"icon"` (required) |
| `pos` | all | The anchor, `{x, y, z}` in world coordinates (required) |
| `size` | `box`, `icon` | For a `box`, `{x, y, z}`, the box's size along each axis (required). For an `icon`, a number, the icon's width in blocks (required) |
| `filled` | `box` | `true` draws the six faces, `false` the twelve edges (`false`) |
| `to` | `line` | `{x, y, z}`, an offset from `pos` to the line's end (required) |
| `height` | `beam` | A number, blocks up (required; drawn rounded to a whole block) |
| `text` | `text` | A string, centred on `pos` and facing the camera (required) |
| `scale` | `text` | A number, a name tag's size at 1 (`1`) |
| `sprite` | `icon` | A string naming an atlas sprite, such as `"item/diamond"` or `"block/stone"` (required); an empty one is a script error, and the native checks its characters and length |
| `color` | all | An ARGB integer `0xAARRGGBB` (`0xFFFFFFFF`), or `{r, g, b, a?}` with bytes 0..255 (`a` defaults to 255) |
| `through_walls` | all | Seen through walls (`false`) |
| `ttl` | all | An integer lifetime in game ticks; absent means until removed |

An unknown key is a script error naming the key (a typo like `thru_walls` is not ignored), and a
key that does not belong to the kind (like `height` on a box) is an error naming it. A missing
required key, a wrong type, a non-finite number, a non-integer or non-positive `ttl`, a colour
component outside 0..255, an empty icon `sprite`, or an icon `size` that is negative or does not
fit in 32 bits are script errors naming the helper and the field (`marker_set: size: ...`). A spec is read as plain data, not through its metatable: a prototype table's `__index` is
never consulted, so every field must be set on the spec itself. The `{x, y, z}` tables (`pos`,
a box's `size`, `to`) carry exactly `x`, `y` and `z`, and `scale` must fit in 32 bits (a finite number
past f32's range is an error, not infinity). The native's own ranges are not re-checked in the
script: the character set and length of an icon's `sprite` name and the icon `size` range (0.25 to
16) are the native's to refuse, answering `false`.

A host refusal is not a script error: over a cap, out of range, off a window, or a marker id of
another owner. The refusal is reported (`marker_set: the engine refused ...`) and it does not
cancel the handler's remaining queued actions, matching the net-send actions. A refused marker is
simply not drawn.

The native limits, in one place: 256 markers per mod and 4096 in every mod together (replacing a
marker does not count twice); marker and mod ids of 1 to 64 characters; text of 1 to 64
characters; icon sprite names of 1 to 64 characters of `a-z`, `0-9`, `_`, `/`, `.` and `-`; every coordinate finite within 30,000,000 of the origin, the far point of a
box/line/beam/icon included; box sides 0 to 256; a line offset at most 512 blocks long; beam heights
0 to 512; text scales 0.25 to 8; icon sizes 0.25 to 16; lifetimes of 1 to 72,000 game ticks. A lifetime runs on game
time, so `/tick freeze` holds it too. A marker belongs to the level it was set in and is drawn
only there, including after a move; markers farther than 256 blocks are not drawn until the
camera comes closer. A mod turned off loses its markers; opening or closing a world drops every
marker; a successful reload of the mod's client state clears them (at the next client hook with a
host, since a reload itself has none), and a failed reload keeps them with the old scripts.

```lua
-- scripts/client/init.lua: a box around the block the player looks at.
horizon.on("client_tick_post", function(event)
  local me = horizon.player() -- the window's own player
  local hit = me and me.crosshair
  if hit and hit.kind == "block" then
    horizon.marker_set("target", { kind = "box", pos = hit.pos,
      size = { x = 1, y = 1, z = 1 }, color = 0xFFFF4444, through_walls = true })
    horizon.marker_move("target", hit.pos)
    horizon.marker_set("note", { kind = "text", pos = hit.pos,
      text = "look here", ttl = 200 })
  end
end)
```

### Players

| Function | Description |
|---|---|
| `horizon.tell(player, text)` | Sends a system chat line to the player |
| `horizon.action_bar(player, text)` | Shows a line over the player's hotbar |
| `horizon.title(player, text)`, `horizon.subtitle(player, text)` | Shows a title; the subtitle shows with the next title |
| `horizon.title_times(player, fade_in, stay, fade_out)` | Sets the title's fade times, in ticks |
| `horizon.take_items(id, count)`, `horizon.give_items(id, count)` | Takes up to *count* of a plain item from the acting player, the main hand first, or gives it where it fits |
| `horizon.give_items(player, id, count)`, `horizon.take_items(player, id, count)` | The same on a named player, from any hook with a host, a timer too. What does not fit is not given, and nothing drops. Inside another player's hook, only that player is reached |
| `horizon.give_experience(player, points)` | Gives experience as an orb does; Mending takes its share first |
| `horizon.give_levels(player, levels)` | Gives levels; a negative number takes them away |
| `horizon.effect(target, name, ticks[, amplifier])` | Gives a status effect, as `/effect give` does, to a mob by id (*target* a number) or to a player. *ticks* -1 lasts for ever. A stronger effect replaces the current one, and an equal one extends it |
| `horizon.set_nickname(player[, name])` | Sets the name shown for the player in chat and the tab list; no name shows their own again |

### Commands

`horizon.command(usage, level, fn)` adds a command of the mod's to the server's command tree. The chat box completes
it as vanilla's commands. Call it at load, as goals are defined. Only commands declared this way are performed:
a `table.insert` into `event.commands` in a `register_commands` handler only edits the tree the server lists, and
a line for such an inserted command is answered as an unknown command.

- *usage*: the command's words, then its arguments, such as `"parasite count [kind:word]"`. `<name:kind>` is
  required: when it is missing, or an `integer` or `number` is not one, the player gets a usage line and the handler
  is not called. `[name:kind]` is optional. The kinds are `word`, `text` (the rest of the line, last), `integer`,
  `number`, `boolean`, `player`, `entities` (the multi-entity selector, `@e` or `@a`), `item` and `block_pos`
- *level*: the operator level a player needs; 0 is everyone, and nil is 0
- *fn*: called as `fn(player, args)`. *args* holds the arguments by name, numbers as numbers and the rest as words,
  and `args.raw`, the whole line. `horizon.feedback(text)` answers the player, and the host actions work.
  A command that matches no usage is left alone: the next handler still sees it, and nothing is cancelled.
  `horizon.on("perform_command", fn, "words")` hears the commands whose words are *words*, the same handlers
  `horizon.command` registers

`horizon.on("command", fn)` is the client hook: it fires for every slash line the player types, before the line
reaches the server. `horizon.cancel()` there drops the line, so the server never sees it and no `perform_command`
handler runs for it. Hearing and stopping every typed line is a client power, separate from performing mod
commands, which is what `horizon.command` declares and `perform_command` hears.

### Scoreboard

| Function | Description |
|---|---|
| `horizon.scoreboard_score(holder, objective)` | The holder's score in any objective (vanilla's, a command block's, another mod's); nil when the objective or the score does not exist. A read never creates a score |
| `horizon.scoreboard_objectives()` | The names of every objective on the board (vanilla's, a command block's, every mod's), in the order `/scoreboard objectives list` shows them, as a sequence of strings (`{}` when there are none) |
| `horizon.scoreboard_set_score(holder, objective, value)` | Sets the score in one of your own objectives |
| `horizon.scoreboard_add_score(holder, objective, delta)` | Adds to the score in one of your own objectives (saturating at the i32 bounds; a score not yet set starts at 0) |
| `horizon.scoreboard_add_objective(name, criteria)` | Creates the objective `<your mod id>.<name>` with the criteria (`dummy`, `health`, ...) |
| `horizon.scoreboard_remove_objective(name)` | Removes your objective `<your mod id>.<name>` and every score in it |
| `horizon.scoreboard_teams()` | The names of every team on the board (vanilla's and every mod's), as a sequence of strings (`{}` when there are none) |
| `horizon.scoreboard_team_of(holder)` | The name of the team the holder is on, any team's; nil when on none. A read never creates an entry |
| `horizon.scoreboard_team_members(team)` | The holders on the team, any team's, as a sequence of strings (`{}` for an unknown team) |
| `horizon.scoreboard_add_team(name)` | Creates the team `<your mod id>.<name>`; the name must be one `/team add` accepts (letters, digits, `_`, `-`, `.`, `+`) |
| `horizon.scoreboard_remove_team(name)` | Removes your team `<your mod id>.<name>`; its members lose their team |
| `horizon.scoreboard_team_join(team, holder)` | Puts the holder on your team `<your mod id>.<team>`, off their previous team |
| `horizon.scoreboard_team_leave(holder)` | Takes the holder off your team; only from a team you own |
| `horizon.scoreboard_display_objective(slot)` | The name of the objective shown in the display slot (`list`, `sidebar`, `below_name`, `sidebar.team.<color>`), any objective's; nil when the slot is empty or is not a display slot. A read never changes the board |
| `horizon.scoreboard_set_display(slot, objective)` | Shows your objective `<your mod id>.<name>` in the slot, replacing what the slot showed |
| `horizon.scoreboard_clear_display(slot)` | Empties the slot, only when it shows one of your objectives |

Teams follow the same rule as objectives: a mod writes only teams named `<your mod id>.<name>`, and reads any team.
Joining a team puts the holder on it, so it moves a holder off their previous team. A join is refused when the
team does not exist, or when the holder is on a team the mod does not own (a vanilla team or another mod's): a
mod cannot pull a player off someone else's team. Re-joining the team the holder is on succeeds. A leave is
refused unless the holder is on one of your teams. Team colours, prefixes and options are not in these helpers.

Display slots: a read shows what any valid slot holds. A set shows your objective in a valid slot and may replace
what the slot shows, another owner's objective included; the displaced objective keeps its scores. A set is refused
unless the slot is valid and the objective is one of yours, existing and not read-only. A clear is refused unless the
slot is valid and shows one of your objectives, so a mod never empties a slot another owner set. This asymmetry is the
lead's choice (issue 61, not in the owner's wording): the owner may reject it, and then a set would be held to the
same namespace rule as a clear.

Reads take any objective. A write takes only an objective named `<your mod id>.<name>` that exists (a score write)
or is free (an add), and not a read-only criteria. The runtime supplies your mod id, the one `horizon.mod_id`
holds: the script does not pass it. A call that still passes it first (`scoreboard_set_score("me", holder, ...)`)
is an error naming the helper and the count (`scoreboard_set_score: takes 3 argument(s), got 4`), and nothing is
written. A write the host refuses (another mod's or a vanilla objective, a missing one, a read-only criteria) changes
nothing and is silent in Lua: the bridge drops the host's `false`, so a script that needs to know reads the score
back with `scoreboard_score` (or `scoreboard_team_of` for a team). Wrong types and missing arguments are errors naming the helper and the argument
(`scoreboard_set_score: value: ...`). The board is the server's: the scoreboard helpers exist only in a server state (`scripts/server/`), and a client state that calls one gets the usual server-only error (`horizon.scoreboard_score is only available in scripts/server/ (this is the client state of mod <id>)`).
A read sees the world's saved board from the moment the world opens, `level_load` included.

### Timers and saved data

| Function | Description |
|---|---|
| `horizon.after(ticks, fn)` | Runs *fn* once after *ticks* of the state's own side (server ticks in a server state, client ticks in a client state) (a finite number, zero or more; anything else fails the load) |
| `horizon.every(ticks, fn)` | Runs *fn* every *ticks* of the state's own side (server ticks in a server state, client ticks in a client state) (a finite number, one or more; anything else fails the load) |

Timers count ticks of the state's own side: server ticks in a server state and client ticks in a client state.
| `horizon.world_data()` | Gets the mod's saved table on the world |
| `horizon.player_data(player)` | Gets the mod's saved table on a player, by name or UUID; nil for a player the server does not hold, such as a player on the hub or the acting one |
| `horizon.entity_data(id)` | Gets the mod's saved table on an entity, by id. Nil on a server when the entity is not in the level, or on an attachment error. On a client copy the id is not checked: an unknown id reads as an empty table, recorded for writeback, but the attachment is not synced, so a write stays on the client copy — a table is no proof the entity exists there. Written back after the call, exactly like `horizon.world_data()` |
| `horizon.config()` | Gets the mod's settings, `config/<id>.json` in the game folder for the server state and `config/<id>.client.json` for the client state, as a table; empty when there is no file yet. Every mod's settings share that folder, as on NeoForge |
| `horizon.save_config(t)` | Writes a table to the mod's settings file, as JSON that a person can edit |

Timers run at the end of a tick of the state's own side while the world runs, as a `server_tick` handler in a
server state and as a `client_tick_post` handler in a client state, so reads and actions work in them. A timer
that fails is logged, a repeating one is kept, and the timers after it run as usual.
`/reload` drops the timers with the handlers.

The saved tables start empty. They are written back after the call and kept through saves as
the engine's attachments under the mod's id (`horizon:attachments`, names `world`, `player` and
`data`). A table handed out twice in one call is the same table, so no edit is lost; when a later
handler fails, nothing is written back, like the queued actions. Use `horizon.entity_data(id)` for
per-mob state, keyed by what the mob hooks name as `entity`.

`horizon.config` and `horizon.save_config` work at load, in `init.lua`, with no host. A mod reads its settings, fills
in the defaults and saves them, so the file is there to edit after the first run.

### Blocks

`horizon.block_entity(block, { ticks = true, data = {...} })` gives the mod's block a block entity. It is made with
*data*, a table of numbers, strings, booleans, nested tables and sequences, when the block is placed, and saved with
the chunk. `ticks` makes it tick. Call it at load, in `init.lua`. The provider (`block_entity`) and
`block_entity_ticks` are served from these declarations, not by script handlers: there is nothing to register
with `horizon.on` for them: `horizon.on("block_entity", ...)` and `horizon.on("block_entity_ticks", ...)` fail the load with "no such hook" (`horizon.on("block_entity"): no such hook`).

Some actions work only in the block hooks:

- `horizon.set_block_property(key, value[, flags])`: sets a state property of the block; *flags* are 2 by default,
  which tells the clients. In `random_tick`, `tick_block` and `block_entity_tick`
- `horizon.schedule_tick(delay)`: asks for a scheduled tick. In `random_tick` and `tick_block`
- `horizon.handled()`: skips the block's own random tick. In `random_tick` and `tick_block`

## Hooks

Lua hears the engine's native hooks under the native names: `horizon.on("mob_spawned", fn)` hears the native
`mob_spawned` hook, and so on for every generated hook. Every row below names the native hook it hears.
[hook-points.md](api/hook-points.md) lists every hook point of the engine with its API level; the generated
`HorizonOn` overloads in [horizon.d.lua](api/horizon.d.lua) are the authoritative handler signatures, and
[hook-exposure.md](api/hook-exposure.md) lists the hooks Lua does not hear and why. A mod written against the old
hand-written hooks migrates with [lua-hook-migration.md](lua-hook-migration.md).

Each hook runs on one side, and a state registers only the hooks of its own side and the ones of both. A
`scripts/server/` state that calls `horizon.on` for a client hook (`hud`, say) fails to load with `hook 'hud' runs on
the client: register it from scripts/client/`, and the client state refuses a server hook the same way. The check
runs when the entry point runs, so `cargo horizon-mod check` shows it with the side. A module in `scripts/shared/`
runs once in each state that requires it, so one that registers a hook of one side must guard it with `horizon.side`
(`if horizon.side == "client" then ... end`), or only the matching state may require it.

A both-sides hook reaches only the state of the side that fired it, because each state reports its side to the engine's filter (`Hooks::mod_side`).

A handler takes one event table, the hook's fields by name:

```lua
horizon.on("mob_spawned", function(event)
  horizon.log(("spawned %s at %.0f %.0f %.0f"):format(event.kind, event.pos.x, event.pos.y, event.pos.z))
end)
```

A handler that changes the event changes what the engine does next: assign to a mutable field
(`event.amount = 10`), edit a sequence in place (`table.remove(event.blocks, i)`), or call
`horizon.cancel()` where the row says so. A field the hook only reports reads back what the engine holds, and a
write to it does nothing. The exact field list of each hook is its event type in `horizon.d.lua`; the shapes are
the same in every hook:

- an optional native value reads as the value, or nil when there is none
- a list reads as a sequence (`{ Item }`); a hole reads as nil; a sequence written back takes integer keys, so a float key errors naming the field instead of dropping the value
- a position or motion vector reads as `{ x, y, z }`
- a uuid reads as its lowercase text (`"6ba7b811-9dad-11d1-80b4-00c04fd430c8"`), never a number
- an id reads as the qualified string (`"minecraft:zombie"`)
- an enum reads as its lowercase name (`"main"`, `"down"`)
- rich text reads as its plain string, with a `<field>_component` table of the vanilla JSON beside it
- saved NBT reads as a `Data` table, written back after the call
- an item stack reads as `{ id, count, damage, extra, plain }`
- a block state reads as its id string, with its properties by name in the `<field>_props` table (`state_props`)
- raw bytes read as a Lua string
- a `Duration` value (such as `frame.elapsed`) reads as seconds; a use-item `duration` is ticks

A hook "by kind" takes a *kind* in `horizon.on(name, fn, kind)`: a block id, a mob or entity kind, a projectile
kind, or command words for `perform_command`. Only handlers of that kind run for a matching event; a handler with
no kind hears every event of the hook. The `HorizonOn` overload is authoritative: a hook whose overload names
`kind` filters by it, and a hook whose overload does not refuses it as a load error. Each row below says what its
kind is. The draw hooks take no kind.

Calls that act on the world are queued and run after the handler with the hook's host ([The `horizon`
table](#the-horizon-table) says which hooks have a host). A hook with no host runs no queued actions: set the
event's fields, or cancel, and nothing else. Reads answer at once from the hook's host, for the length of the
call.

A handler that runs longer than 250 ms is stopped ([Limits and errors](#limits-and-errors)). The draw hooks get
8 ms instead: a draw hook that keeps missing its frame budget is disabled after 3 misses, until `/reload`.

### The game and the world

| Hook | Called when |
|---|---|
| `server_tick(event)` | a server tick starts (`event.phase == "start"`) or ends (`"end"`), each with `event.world_running` and the context (`event.authority`, `event.epoch`, `event.tick`) |
| `world_lifecycle(event)` | the world opens or closes; `event.phase` names the phase, `event.error_text` carries a failure's text (empty otherwise), with the context |
| `data_pack_files(event)` | the data packs load, the first load included: `event.files` is the packs' files in load order, a sequence of `{path, data}` byte strings. Append entries and they join the load. From the second load on (such as `/reload`), the mod's scripts run again first, so an edited script is live for the load |
| `explosion_detonate(event)` | an explosion is about to break `event.blocks`, a sequence of `{x, y, z}`. Remove entries to spare those blocks: they keep their block and get no fire. The event holds `dimension`, `center` (`{x, y, z}`) and `source`: `{entity, kind}` for the entity that exploded (its id and kind, such as `minecraft:tnt`), or nil for a blast the engine cannot attribute |
| `entity_join_level(event)` | any entity joins the level, such as `minecraft:item`, `minecraft:arrow` or a mob. The event holds `dimension`, `entity` (its id), `kind`, `pos` (`{x, y, z}`) and `loaded_from_disk`, true for one that comes back with its chunk. `mob_spawned` fires for mobs too. `horizon.cancel()` keeps it out. Register it by entity kind |
| `projectile_launch(event)` | a projectile is added to the level. The event holds `entity` (its id), `kind` and `player` (the shooter's table, or nil for a mob's or a dispenser's). Register it by projectile kind. It only listens |
| `projectile_impact(event)` | a projectile, such as an arrow, a snowball, an egg, a potion or a fireball, hits something, before the hit acts. The event holds `dimension`, `projectile` (its id), `kind`, `owner` (the shooter's entity id, or 0), `entity` and `entity_kind` for an entity hit, or `block` (`{x, y, z}`) and `face` for a block hit (the block's id comes from the `horizon.block` read), and `at` (the point, `{x, y, z}`). Branch on `event.entity`: on an entity hit `block` and `face` are still `{0, 0, 0}`, and on a block hit `entity` is nil and `entity_kind` is `""`. Register it by projectile kind. `horizon.cancel()` and it flies on. The engine keeps the copy each impact needs only while a script listens |

### Mobs

| Hook | Called when |
|---|---|
| `mob_spawned(event)` | a mob spawns. The event holds `dimension`, `kind`, `pos` (`{x, y, z}`) and `natural`. A mob that loads with its chunk is not told. Register it by mob kind |
| `mob_ai_register(event)` | a mob's goals are set up, once, on the side that runs its AI. Register it by mob kind. The event holds `mob`, the mob's view as its goals see it. Handlers call `horizon.add_goal`, `horizon.add_target_goal`, the clears and `horizon.set_body_rotation`, which run on the mob after the call |
| `register_goals(event)` | a mob's species goals are registered, before `mob_ai_register`. Register it by mob kind. The event holds `mob`, and the same five calls run on the mob after the call. It fires during species goal registration, for a client's copies of a server's mobs too, and before a `"goals": "none"` mob's selectors are wiped: goals added here on such a mob are gone by `mob_ai_register`. Handlers on both hooks append, so a script that calls `horizon.add_goal` in both attaches the goal twice |
| `entity_tick_post(event)` | a mob has ticked. The event holds `dimension`, `entity` (its id) and `kind`. The host actions and reads work, but there is no mob view: read the mob with `horizon.mob(id)` when needed, and its saved table with `horizon.entity_data(event.entity)`. Register it by mob kind: a handler for every mob kind hears every mob of the level each tick. The engine keeps the ticking list only while a handler is registered |
| `interact_entity(event)` | a player right-clicks an entity, mobs included: one hook for every entity, even one the host has no view of, dead or gone. The event holds `entity` (its id), `kind` and `stack` (the hand's stack `{id, count, damage, plain}`, or nil). It carries no player and no dimension: the acting player is `horizon.acting_player()`, and the level comes from the host reads. `horizon.cancel()` takes the click, and the hand still swings. The host actions and reads work. Register it by entity kind |
| `living_incoming_damage(event)` | a living entity is about to take damage, a mob or a player (kind `minecraft:player`). The event holds `entity`, `kind`, `damage_type` (such as `minecraft:mob_attack` or `minecraft:on_fire`), `causing` and `direct` (the entities behind it, or -1) and `amount`. No host: set `event.amount`, or `horizon.cancel()` to refuse the hit; other queued actions are dropped. Register it by entity kind |
| `mob_killed(event)` | a player's melee blow kills a mob. The event holds `player` (the killer's uuid text), `entity` and `kind`. `mob_died` covers every other cause. The host actions work. Register it by mob kind |
| `mob_died(event)` | a mob dies, by any cause: a blow, an arrow, fire, a fall or `/kill`. The event holds `dimension`, `entity`, `kind`, `damage_type`, `causing` (the mob behind it, or 0) and `player` (the credited player's uuid text, or nil). It only hears. Register it by mob kind |
| `living_fall(event)` | a living entity is about to handle a fall, a mob or a player (kind `minecraft:player`). The event holds `entity`, `kind`, `distance` and `multiplier`. No host: set the fields, or `horizon.cancel()` for no fall damage. Register it by entity kind |
| `living_heal(event)` | a living entity is about to heal. The event holds `entity` and `amount`. No host: every handler hears every entity, players too. Set `event.amount`, or `horizon.cancel()` |
| `living_knockback(event)` | a living entity is about to be knocked back, a mob or a player (kind `minecraft:player`). The event holds `entity`, `kind`, `damage_type`, `causing`, `direct`, `strength`, `x` and `z`. No host: set `event.strength` (and `x`/`z` to steer), or `horizon.cancel()`. Register it by entity kind |
| `living_equipment_change(event)` | an equipment slot of a mob (`mainhand`, `offhand`, `feet`, `legs`, `chest`, `head`, `body` or `saddle`) takes a different stack. The event holds `entity`, `kind`, `slot`, `from` and `to` (item tables `{id, count, damage, plain}`, or nil). It only hears. Register it by mob kind. The engine keeps the equipment watch only while a handler is registered |
| `loot_drops(event)` | a block broken by a player, an explosion or a mob, or a mob that died, is about to drop its loot. By block id or mob kind. The loot context is flat on the event: `table` (the loot table's id), `dimension`, `origin`, and `block` (`{x, y, z, id, state}`) or `entity` (`{id, kind}`), `player` (the one credited, or nil), `tool` (an item table, or nil), `explosion` (a radius, or nil) and `used`. *drops* (`event.drops`) is a sequence of `{id, count, damage, plain}`: change it in place, and it is written back after the call. An entry whose id matches the stack at that place keeps the stack's components |

### Blocks

| Hook | Called when |
|---|---|
| `random_tick(event)` | a block that ticks randomly does: a mod's block as its template does, or one with `random_ticks`. Register it by block id: a handler for every block hears every random tick of the level, hundreds a tick. The event holds `dimension`, `state` (the id, as the other level hooks name it), `pos` (`{x, y, z}`), `state_props` (the state's properties by name) and `data` (the block entity's saved table, or nil without one, written back after the call); there is no state-string field, only the id and its properties. `set_block_property`, `schedule_tick` and `summon` run on the level; `handled()` skips the block's own tick |
| `tick_block(event)` | a scheduled tick of the block is due. Register it by block id. The event and the actions are as in `random_tick` |
| `right_click_block(event)` | a player right-clicks a block, before the block and the item act. The event holds `pos` and `face` (`{x, y, z}`). The block's id comes from the `horizon.block` read; the acting player is `horizon.acting_player()`, and their `held` is the selected hotbar stack (main hand, nil when empty), unchanged between the two passes. The event names no block-entity data. `horizon.cancel()` skips the block and the item's use on it for that hand, with no swing. See the note below |
| `block_entity_tick(event)` | a server tick of a block entity declared with `ticks`. Register it by block id. The event holds `dimension` (the host's level), `pos` (`{x, y, z}`) and `entity` (`{id, data}`): `id` is the entity's block id, and `data` is its saved table, written back after the call; replacing `event.entity` or `event.entity.data` writes nothing (only edits to the seeded data table are written back). The host actions and `set_block_property` work |
| `destroy_block(event)` | a player breaks a block, once the game has allowed it. The event holds `pos` (`{x, y, z}`), `state` (the id) and `state_props`. `horizon.cancel()` keeps the block, so nothing breaks or drops. The acting player is `horizon.acting_player()`. Every break wakes the handler: keep the block ids it cares about in Lua |
| `place_block(event)` | a player has placed a block. The event holds `stack` (the stack placed), `pos`, `state` (the id) and `state_props`. It only listens; to stop a placement, take the click in `use_item_on` |

`right_click_block` runs once for each hand, the main hand and then the off hand, as the game tries both (NeoForge's
`RightClickBlock` fires for each hand too). The event names no hand and no stack: `horizon.acting_player().held`
is the selected hotbar stack on both passes — it does not follow the hand of the pass, and the off-hand stack is
not on this event (`use_item` / `use_item_on` carry `stack` for the hand in use). Make a handler do something
that is the same when done twice, such as lighting a lamp rather than toggling it, or read the block's state first.

### Items

| Hook | Called when |
|---|---|
| `use_item(event)` | a player right-clicks with an item in the air, or on a block that lets the click through, before the item's own use. The event holds `stack` (`{id, count, damage, plain}`). `horizon.cancel()` takes the use: the hand swings, and nothing else runs. The acting player is `horizon.acting_player()`. Every use wakes the handler: keep the item ids it cares about in Lua |
| `use_item_on(event)` | the same, on a block. The event holds `stack`, `pos` (`{x, y, z}`) and `face` (`{x, y, z}`, with `{0, 1, 0}` the top). The block's id comes from the `horizon.block` read. `horizon.cancel()` takes the use |
| `attack_entity(event)` | a player attacks an entity, before the blow. The event holds `entity` (its id), `kind` and `pos` (`{x, y, z}`). Register it by entity kind. `horizon.cancel()` and the blow does nothing. The event names no stack: the acting player's hand is `horizon.acting_player().held` |
| `hurt_enemy(event)` | a player's blow has landed on an entity, after the damage. The event holds `stack` (the weapon, `{id, count, damage, plain}`), `entity` and `kind`. Register it by entity kind. It only listens |
| `living_entity_use_item_start(event)` | a player starts to use an item over time: eating, drinking, drawing a bow, or a mod item with a `use_duration`. The event holds `stack`, `entity` (nil for the player's own use), `hand` (`"main"` or `"off"`) and `duration`, the ticks it takes. Set `event.duration` to change it, or `horizon.cancel()` to stop the use. Mob uses filter by kind; the player's own use reaches bare handlers |
| `living_entity_use_item_finish(event)` | a player has finished using an item over time: eaten or drunk it, or held a mod item for its `use_duration`. The event holds `stack`, `entity` (nil for the player's own use), `hand`, `duration` and `result`. Set `event.result` to an item table for what stays in the hand, or nil for nothing. Mob uses filter by kind; the player's own use reaches bare handlers |
| `inventory_tick(event)` | a server tick of a stack that a player carries. The event holds `slot` (as the inventory saves it: 0 to 35, armour 100 to 103, the off hand -106) and `selected`, true for the held stack. The ticking stack is not on the event, and there is no count to change (`horizon.set_count` fails: it has nothing to write to). The ticking player is `horizon.acting_player()`. The ticking stack itself is not available — take/give items act on the player — and `held` is only the selected hotbar stack, not the stack in the ticking slot. Every stack of every player ticks, so keep it light |
| `item_crafted(event)` | a player takes a crafting grid's result. The event holds `player` (their uuid text: `horizon.player(event.player)` reads their table), `recipe` (its id) and `stack` (the result). Every craft wakes the handler: keep the recipes it cares about in Lua. It only listens |
| `item_dropped(event)` | a player throws an item, with the drop key or from a menu; items scattered at death do not count. The event holds `player` (their uuid text), `item` (the id) and `count`. It only listens |
| `item_picked_up(event)` | a player is about to pick up `event.count` of `event.item` (an id) from the ground. The event holds `player` (their uuid text). `horizon.cancel()` leaves them there |

### Players

| Hook | Called when |
|---|---|
| `player_logged_in(event)`, `player_logged_out(event)` | a player joins the game, or leaves it by any path. The event holds `player`: the full table while they are on, else `uuid` and the told `name` |
| `player_respawned(event)` | a player respawns after a death. The event holds `player`, as above |
| `player_died(event)` | a player dies. The event holds `player` (uuid text) and `cause`, the death message's key such as `death.attack.lava`, or nil. It only hears |
| `player_tick(event)` | a living player's server tick has ended. The event is empty. Every player, every tick, with the host actions and the reads: `horizon.players()` is the whole list (the placed guests plus the single-player owner), and `horizon.acting_player()` is the ticking player — that is how a handler learns whose tick fired. Keep it light. It takes no `kind` |
| `living_incoming_damage(event)` with kind `minecraft:player` | a player is about to take damage. The event holds `entity`, `kind`, `damage_type`, `causing`, `direct` and `amount`. No host and no name: set `event.amount`, or `horizon.cancel()` |
| `living_fall(event)` with kind `minecraft:player` | a player is about to handle a fall. The event holds `entity`, `kind`, `distance` and `multiplier`. No host: set the fields, or `horizon.cancel()` for no fall damage |
| `living_knockback(event)` with kind `minecraft:player` | a player is about to be knocked back. The event holds `entity`, `kind`, `damage_type`, `causing`, `direct`, `strength`, `x` and `z`. No host: set `event.strength` (and `x`/`z` to steer), or `horizon.cancel()` |
| `player_command(event)` | a player types any command, before it runs. The event holds `player`, `command` (no slash) and `replies`. Set `event.replies` (a sequence of strings or `{text, component?}` tables) to answer them; `horizon.cancel()` and nothing runs. No player's state is in place here, so `tell` and the item functions reach nobody |
| `player_chat(event)` | a player's chat line is about to go to everyone, after the chat filter. The event holds `player` (uuid text), `name`, `message` and `replies`. There is no host while the chat packet is being handled: set `event.replies` to send a line back to the sender. `horizon.cancel()` and nobody sees it, the sender included. The line itself can't change, because a signed message would no longer verify; the hook point is upstream's `native_player_chat`. Commands go to `player_command` |
| `player_changed_dimension(event)` | a player's move to another level has ended: a portal, `/execute in ... run tp`, or a respawn elsewhere. The event holds `player`, `from` and `to` (dimension ids). Heard for a player of the server thread (single-player, LAN and dedicated) once a move into another level has ended; not heard for a move inside one level, a move that ends where it started, or a player who leaves before it ends |
| `player_bed_enter(event)` | a player is about to enter the bed at that position. The event holds `player` and `pos` (`{x, y, z}` of the bed head, always present). `horizon.cancel()` keeps them out |
| `player_exp_change(event)` | a player's experience is about to grow by *amount* from an orb, after Mending's share. The event holds `player` and `amount`; set `event.amount` to change the points added |
| `player_container_open(event)` | a player opens a menu. The event holds `player`, `dimension`, `menu` (its type, such as `minecraft:generic_9x3`, `minecraft:furnace` or `minecraft:crafting`), and `pos` (the position of its block, or nil) |

### Draw hooks

| Hook | Called when |
|---|---|
| `hud(event)` | every frame, to draw over the game. The event holds `batch` (the draw calls: `fill`, `text`, `sprite`, `item`, `item_stack`, `item_model`, `quad`, `text_width`, each drawn at once) and `corners` (`place`, `shown`). The calls are dot-called (`batch.fill(...)`), draw at once, and expire with the call: keeping `event.batch` and calling it later is an error |
| `gui_layer_post(event)` | every frame, after a screen layer draws. The event holds `layer` (its id) and `batch`, as in `hud` |

`gui_layer_pre(event)` fires every frame before a screen layer draws, with the same `layer` and scoped `batch`: draw below the layer with the same calls, and call `horizon.cancel()` to hide the layer (first cancel wins). It runs on the same 8 ms frame budget and takes no kind.

`world_primitives(event)` fires every frame while the world is drawn (not on the title screen), after the entities and before the first-person hand, on the client only (a server state refuses it at load). The event holds `partial` (the tick fraction past the last tick, 0 to 1), `camera` (`{x, y, z}`) and `primitives`, the frame's list, with six calls: `line(from, to, color, width)`, `box_outline(min, max, color, width)`, `filled_box(min, max, color)`, `beam(base, height, color)`, `text(at, text, color, scale)` and `sprite(at, sprite, size, color)`. Positions are `{x, y, z}` tables in world coordinates; a colour is an ARGB integer or `{r, g, b, a?}`; sizes are finite and not negative; `beam` height is an integer from 0 to 2048; `text` and `sprite` names are not empty. The last argument of each call is an optional options table: `through_walls = true` draws over the world without a depth test, and `text` also takes `scale_with_distance` and `background`. A call draws at once and the table expires with the call: keeping `event.primitives` and calling it later is an error. A handler may draw at most 4096 primitives per frame (the engine's bound on world markers); the next call is an error naming the cap, and the primitives before it stay drawn. The hook's reads are the client's, as in `hud`. It cannot queue actions: `horizon.tell` and `horizon.broadcast` are reported as errors, since the hook has no mutable host to run them with.

The draw hooks run on the frame's 8 ms budget and take no kind. Their reads are the client's: `horizon.time`,
`block`, `block_property` and `light` read the client's world; `horizon.real_time` reads the wall clock and
`horizon.player()` with no argument reads the window's own player (see the next section); `players`, `mob`,
`mobs`, `raycast` and `surface_height` need a server and read empty on a client window, with `held` nil and
`horizon.acting_player()` nil there too; `world_data`, `player_data`, `entity_data` and `summon`
raise an error there. The acting player's inventory reads raise an error in a draw hook that has an acting
player; on a client window, which has none, those reads are empty too, like the named `player` reads. The only
actions a draw handler may queue are `horizon.tell` and `horizon.broadcast`; any other
server-only call raises the server-only error (`horizon.<name> is only available in scripts/server/`)
even there. A draw hook never waits for its state: when another hook holds the state (for example
`log_line` on `Log writer`), the frame is skipped silently, with no error line.

### Reading the game from a HUD

A draw hook is the wrong place to feel blind: the window's own player and the wall clock read live there, in
every client hook that has a host (a draw hook, a client tick, a key handler), not only in draws. On the
client state, `horizon.player` and `horizon.real_time` never raise the server-only error; in a hook without
a host they raise like the other reads (`reads work inside a hook with a host`).

`horizon.player()` with no argument answers the window's own player as a `local player` table:

- `local` (true; it tells this table from a server player's)
- `health`, `max_health`, `absorption`, `food` (0 to 20), `saturation`, `air`, `max_air`, `armor`
- `xp` as `{ level, progress, total }` (`progress` 0 to 1 towards the next level)
- `position` as `{ x, y, z }` (the feet), `yaw`, `pitch`
- `selected_slot` (0 to 8, the hotbar)
- `crosshair`, nil when nothing is in reach, else `{ kind = "block" or "entity", distance, pos = { x, y, z },
  entity }` (`entity` the target's network id, nil for a block or an entity without one)

The table has no `uuid`, `name`, `dimension` or `game_mode`: the native does not provide them. The dimension
is on `horizon.time().dimension`. On the title screen, with no player, `horizon.player()` answers nil rather
than raising. With a name, `horizon.player(name)` is the other read: on a client window the world has no
players to name, so it reads nil (the same empty-on-client contract as `players()`, `mob` and `mobs`).

Two clocks, two jobs. `horizon.time()` is the **game clock**: it reports the world's day time (see `Time`),
and `/tick freeze` holds it, like vanilla — use it for a HUD clock that shows the world's time. `horizon.real_time()`
is the **wall clock**: monotonic `seconds` since the game started and `delta`, the last frame's wall time in
seconds; `/tick freeze` never touches it — use it for a HUD animation that keeps pulsing. On a dedicated
server there is no frame loop, so `delta` is whatever the native holds at the time (0 before a frame).

```lua
horizon.on("hud", function(event)
  local p = horizon.player()
  if not p then return end
  local pulse = 0.5 + 0.5 * math.sin(horizon.real_time().seconds * 4)
  event.batch.fill(10, 10, 100, 6, 0xFF000000)
  event.batch.fill(10, 10, 100 * p.health / p.max_health, 6, 0xFFFF4444)
  event.batch.fill(10, 18, 100 * p.food / 20, 6, 0xFFCC8833)
  event.batch.fill(10, 26, 10 + 6 * pulse, 10 + 6 * pulse, 0xFF88CCFF)
  event.batch.text("lvl " .. p.xp.level, 10, 30, 0xFFFFFFFF, true)
end)
```

### Screen and fog hooks

The hooks about the world fog are client-side hooks: a client state hears them, a server state never
(registering one from `scripts/server/` fails like any wrong-side hook). The screen hooks are
client-side too, and all ten v1 hooks below are in Lua, alongside the fog hooks and the v2 twins.

The fog hooks take `event.partial`, `event.camera` (`{ x, y, z }`), `event.fog_type` (the lowercase name:
`"atmospheric"`, `"water"`, `"lava"`, `"powder_snow"`). `compute_fog_color` sets `event.rgb` (`{ x, y, z }`
for red, green and blue, written back like `camera_pose`'s position); a non-finite component (a NaN) is a script
error naming `rgb.x`, `rgb.y` or `rgb.z` and the write is dropped, and the native replaces the computed color only
when all three are in 0..1, so a finite value outside that range keeps the original. `render_fog` sets `event.fog` (`color = { r, g, b, a }`
and the six distances by their native names); a non-finite value is a script error naming the field, and the
native keeps the whole original fog unless color and distances validate.

The screen hooks take `event.screen`, the view of the open screen: `screen.id`
is its stable id (`"inventory"`, `"creative"`, `"paused"`, `"options"`, the title's own ids),
`screen.menu` the server-opened menu's type id or nil, `screen.title` the server title as its plain string
with `screen.title_component` the vanilla JSON table (the rich-text pair, both nil without a server title),
`screen.rect` the container panel as `{ x, y, width, height }` in GUI pixels (the whole window for a screen
with no panel), `screen.slots` how many slots the menu has, `screen.mouse` the pointer in GUI pixels as
`{ x, y }`, and `screen.partial_tick` the frame's partial tick. The lifecycle hooks (`screen_opening`,
`screen_init_post`, `screen_closing`) only hear; with no screen open they never fire. The cancellable screen
hooks (`..._pre`) add their input and take `horizon.cancel()`: `screen_key_pressed_pre` (`event.key` —
vanilla's key name, `"key.keyboard.e"`, `event["repeat"]` — keyboard auto-repeat (`repeat` is a Luau keyword, so index it with brackets), `event.modifiers` — GLFW
bits), `screen_key_released_pre` (`event.key`, `event.modifiers`), `screen_char_typed_pre` (`event.c` — a
one-character string, `event.modifiers`), `screen_mouse_button_pressed_pre` /
`screen_mouse_button_released_pre` (`event.button` — the GLFW button index, `event.modifiers`),
`screen_mouse_dragged_pre` (`event.button`, `event.delta` — `{ x, y }` GUI-space movement,
`event.modifiers`), `screen_mouse_scrolled_pre` (`event.scroll` — `{ x, y }` wheel movement as the game
scales it, `event.modifiers`). Note the v1/v2 split: `screen_mouse_dragged_pre` and
`screen_mouse_scrolled_pre` have v2 twins which also lend the game mutably and fire at the same point just
before v1, for the same drags and wheel turns — hear the v2 hook, not both: a mod hearing both acts twice.

### Hooks Lua does not hear

Lua hears 460 of the 516 hook points as event tables (`GENERATED_HOOK_NAMES`). The tables above spell out the hooks this page describes;
every other generated name is still an event table whose shape is its `HorizonOn` overload in
[horizon.d.lua](api/horizon.d.lua). [hook-exposure.md](api/hook-exposure.md) lists only the hooks that are not
generated, for one reason each: borrows of render state (26 hooks, among them the draw calls outside `hud`
and `screen_render_post`), of the generating chunk (2), of borrowed loot state (2), and of client input (1);
hooks that need a
converter the generator does not have (14 adapters), a live handle of their own design (6), or raw packets (3); and the
level-tick host the runtime does not implement (2). Five of the 460 generated hooks take no `horizon.on`
registration: the two declaration-served hooks, `block_entity` and `block_entity_ticks`, which are served from the
`horizon.block_entity` declarations, and the three listened queries, `entity_tick_listened`,
`living_equipment_listened` and `hears_projectile_impacts`, which the game answers from the sibling hooks'
handlers instead.

## Shared tables

### `player`

The `player` table the `horizon.players()`, `horizon.player()`,
`horizon.acting_player()` and `horizon.command` reads hand out
(the generated hook seeds' `player` tables stay bare)
has these fields:

- `uuid` and `name`
- `dimension`, and `position` as `{x, y, z}`
- `yaw` and `pitch`
- `game_mode`: 0 survival, 1 creative, 2 adventure, 3 spectator
- `health`
- `held`, the player's selected hotbar stack (main hand), or nil when it is empty: on a command's `player` table as
  `{item, count, plain}`, and on the `horizon.player(name)`, `horizon.players()` and
  `horizon.acting_player()` tables as the full `Item` table (`{id, count, damage, extra, plain}`).
  The generated hook seeds' `player` tables stay bare, so an unheard player hook costs no stack read

A player that the world has no record of at that moment, such as one who is dying, has `uuid`, and the `name` that
an earlier player hook reported. `player_died` names no player table at all: its `player` is the uuid text.

`horizon.player()` with no argument on a client state hands out a different table instead, the window's own
player: see [Reading the game from a HUD](#reading-the-game-from-a-hud).

### Saved tables

A saved table, whether a block entity's, `world_data`, `player_data`, `entity_data` or `save_config`'s, follows
these rules:

- It is written back after the call. A number keeps its NBT type, and a key set to nil is removed.
- It may nest 128 tables deep, and may not hold itself. Either is an error that names the key path, such as
  `items[1].me`, and the data is not written.
- A table with items 1 to n is saved as a list, which has no names. A key next to those items, as in
  `data.items = {"a"}; data.items.count = 1`, is an error that names it; it is not dropped silently.
- A Lua number is a double. A long, or a JSON integer in `config` or `data`, above 2^53 reads rounded. A long that
  the script leaves as it read it is written back exactly; a changed one is rounded, with a warning in the log.
- A whole number is written to JSON as an integer.

## Tools

### Types for an editor

`docs/api/horizon.d.lua` holds Luau definitions of `horizon` and `registry`: every function above, every hook's
handler signature, and the tables they hand a script, such as `Player`, `ReadPlayer`, `CommandPlayer`, `LocalPlayer`, `MobView`, `Item`, `Block` and `Data`. It is generated by `cargo run -p xtask -- lua-types`; the tests run it with `--check`.

With [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp), this command reports a misspelled function or hook, and
an editor completes them:

```sh
luau-lsp analyze --definitions @horizon=docs/api/horizon.d.lua --platform=standard scripts
```

The modkit ships a copy as `types/horizon.d.lua`, and a `.luaurc` whose `modkit` alias makes `require("@modkit")`
resolve.

### Checking a mod without a world

`horizon-client --check-mod <mod folder or .hmod> [--dev-mod <library>]...` loads a scripted mod's scripts without
a world, as the game would at start: its registry script through the content loader, then `init.lua` with the
libraries it depends on from the `--dev-mod` list. It prints the scripts and libraries it loaded. It exits with code 1,
and the file and line, when:

- a script fails to load
- `horizon.on` is given a hook it does not know
- a command or goal fails when it is declared
- the manifest's `lua_hook_surface` differs from the engine's `LUA_HOOK_SURFACE_VERSION`

With `--resources <26.3 folder>` (the prepared vanilla folder), it also layers the mod's content on that folder, as the
game does at start, and exits with code 1 when the game would refuse it. A refused file leaves every content mod out
of the world, such as an `add_spawns` spawner with `min_count` where NeoForge writes `minCount`. A content mod without
scripts gets only this check.

`cargo horizon-mod check <folder> --game <horizon-client> --dev-mod <library> [--resources <26.3 folder>]` runs it
after the registry's checks.

For a mod folder, `check` also runs the script steps, each reported on its own and each failing the command on its
own (issue 53):

1. the headless load above (`--no-scripts` skips this step only);
2. StyLua formatting, as `cargo horizon-mod fmt [folder]` writes it: Luau syntax, two-space indent;
3. selene linting, against the Luau std plus the `horizon` and `registry` globals (a hand-written std in
   `crates/cargo-horizon-mod/tools/` for now);
4. `luau-lsp analyze` with `docs/api/horizon.d.lua` as `@horizon` definitions — luau-lsp is C++, so a pinned
   release binary (1.70.1) is downloaded into the user cache dir, never the repo: `$XDG_CACHE_HOME/horizon-mod/tools` or
`~/.cache/horizon-mod/tools` on Linux and macOS, `%LOCALAPPDATA%\horizon-mod\tools` on Windows (`HORIZON_MOD_TOOLS_CACHE`
or `--tools-cache <dir>` override it)
   and its SHA-256 verified against `crates/cargo-horizon-mod/tools/luau-lsp.toml` before it runs. `--offline`
   (or `HORIZON_MOD_OFFLINE`) skips the step with a notice when the binary is not cached; `--luau-lsp <binary>`
   runs a binary of your choosing instead of the pinned one. Each side the mod has (`scripts/server/init.lua`,
   `scripts/client/init.lua`) is analysed as the runtime resolves its requires: that side's `scripts/<side>/` and
   `scripts/shared/` are flattened into one view, so `require("util")` and `require("example.command")` find
   their files, and a failure names the real file (`scripts/shared/util.lua`) with the side it failed on
   (`[server]`, `[client]`). The modkit's own scripts are a dependency, not the
   mod's code, so their diagnostics are not reported. The mod's `.luaurc` aliases `@modkit` to a sibling
   `horizon-modkit` checkout: when that sibling is missing, `--modkit <dir>` (or `$MODKIT`) points the analysis
   at the real checkout through a staged `.luaurc` in the cache — the mod's own `.luaurc` is never edited — and
   with no modkit to be found at all, only the `@modkit` require's diagnostics are dropped, with a notice (the step
   still passes, and its JSON `detail` carries the notice). A `--modkit` that is not a directory is an error, and so
   is a `$MODKIT` that is set but not a directory (`$MODKIT: <path> is not a directory`); `--modkit` wins over it.

Steps 2 and 3 run in-process (StyLua and selene are linked in). Nothing is looked up on the `PATH`: step 4 runs the
luau-lsp in the cache (downloaded once) or the binary that `--luau-lsp <path>` names.

## Reloading

`/reload`, and any data load after the first, reads the mod's scripts again and runs each side's entry
(`scripts/server/init.lua`, `scripts/client/init.lua`) from the start. Handlers, timers, goals, commands, block entity declarations and `require`d modules are dropped. The Lua state
and its globals stay.

The new script registers into fresh tables, which replace the old ones only when the script ran to the end. A script
that fails to load, before or after it registered anything, leaves the old handlers, timers, commands and goals
running as they were, with the error in the log.

The server asks the mods for their commands again after the reload, so a command that the new script declares is in
the players' command trees at once, and one that it dropped is gone.

A goal that already runs on a mob keeps the definition it was made with, until that mob's AI is set up again, as
for a new spawn or a chunk coming back. `horizon.goal` changes only what later `add_goal` calls make.

The mod's content (`scripts/registry.lua`, `data/` and `assets/`) is a separate matter. The block, item, mob effect,
entity type and menu type registries are read once, when the game starts, and stay until a restart.

`horizon-ctl reload-mods` reloads every state by default (both sides in single player) and `--side client` or `--side server` reloads one side only.

Some builds layer the content again on `/reload`: the server starts from the content root it plays on, which stands
for the base it was built from. Such a build takes the mod's data and assets as they are now, and its declarations
as they were at start:

- A changed declaration is kept as it was at start.
- A new mod with declarations is left out.
- A mod that is gone keeps its declarations, with no files.

Each case writes a `Horizon: mod <id>: ...` line to the log, which says that a restart applies it. A mod that
declares what it declared at start is layered without a message.

## Limits and errors

A handler, or a goal function, that runs longer than 250 ms is stopped by Luau's interrupt (per state). A hook's
handlers share one budget per state: 250 ms of wall-clock time for each handler registered in that state, as one
call (frame hooks get 8 ms per handler in that state, as above). The interrupt raises an error that
behaves like any other error of the handler: it goes to the log and the dev tools' console, and a goal reads it as
"no". A `pcall` around the slow code catches that error, but the next loop iteration or call raises it again, so
nothing of the handler runs past its deadline, and the actions it queued are dropped. This holds for the timers' own
call too. A state is used by one thread at a time; hooks from worker threads wait for their turn.

`init.lua`, with the modules it requires, has five seconds at load and at reload (per state). A state may use up to 64 MiB (per state, so a mod with both side folders can use 128 MiB in single player). Names the scripts hand over where the engine needs them for the whole session (command words) are interned once each: at most 4096 distinct names of at most 256 bytes each, and anything past either limit is refused with a Lua error instead of leaking without bound.

A script reads nothing but its own mod (`lua_mods::Budget`, in `event-horizon-lua`):

- Luau has no `io`, `loadfile` or `dofile`.
- Its `os` has only `clock`, `date`, `difftime` and `time`.
- `require` is the game's own, over the mod's `scripts/` and its libraries.

A hook's handlers run in registration order, as one call. When one of them fails, through an error or the budget,
the handlers after it do not run for that event. Nothing that the handlers before it asked for reaches the game:
neither their actions, `cancel()` included, nor their changes to a saved table (`data`, `world_data`,
`player_data`, `entity_data`). The event goes on as if the mod had not heard it, and the log has the error. So a mod that must not
lose work splits the work across hooks, or guards it with `pcall` itself.

A handler's error reads the same in the log, the dev tools' console and the mod's status row: the hook first, then
the message's first line (which names the Lua file and line), then the traceback indented under it, capped to the
first 20 frames with a `... N more` line. A thrown value that is not a string is reported with its type. A write-back that
hands the game a NaN or an infinity is refused the same way, naming the field. Setting an optional number (or string)
field to `nil` clears it.

## Chest menus

A script can show a chest GUI and react to clicks. All six helpers are server state only: like `horizon.broadcast` they queue and run after the handler, and like `horizon.time` the two reads answer at once; they are not in `CLIENT_API` or `DRAW_API`.

Actions (queued, run after the handler like `give_items`):

* `horizon.open_menu(player, rows, title, items)` opens a chest menu of `rows` rows (1 to 6; the native clamps `rows` to 1..=6, no Lua error; values past 6 rows or below 1 clamp to 6 and 1) titled `title` (a Lua string, as `horizon.broadcast`). It closes the player's previous menu (native behaviour).
* `horizon.open_menu_with_input(player, rows, title, items, input)` is the same, with player-filled slots `input = { start = 0, ["end"] = 9 }` (`start` inclusive, `end` exclusive; `end` is a Lua keyword and is quoted in the table). An empty range or `end > rows*9` is a silent no-op (native returns `None`), not an error.
* `horizon.set_menu_items(player, items)` replaces what the player's open menu shows.
* `horizon.close_menu(player)` closes the player's open menu.

`player` is the uuid text a hook event gives, or a player table, exactly as the net-send helpers accept it (`string | ReadPlayer`). `items` is a sequence of item tables or holes, one entry per slot from slot 0 (slot numbers are the native ones, the same numbers `menu_clicked` reports in `event.slot`; they start at 0). An item is `{ id, count, damage, extra }` (`nil` or `false` is an empty slot, `extra` holds components). The native pads or truncates to `rows*9`; the Lua code does not duplicate that.

Reads (answer now, like `horizon.time`):

* `horizon.menu_contents(player)` is a sequence of items, or `nil` when the player has no native menu open. Holes are pinned like other generated sequences; items are the `{ id, count, damage, extra, plain }` tables.
* `horizon.menu_cursor(player)` is the item on the player's cursor, or `nil`.

The native `open_menu` returns the new window id (`Option<i32>`). Actions are queued and cannot return a value to the script, so the id is dropped: a script learns the window only from the events (`menu_clicked` and `menu_closed` carry `event.window`).

A player who is not online: `open_menu`/`open_menu_with_input` return `None` (silent no-op), `set_menu_items`/`close_menu` are silent no-ops, and `menu_contents` returns `None` (Lua `nil`). None of that is a Lua error.

```lua
horizon.command("menu", 0, function(player, args)
  horizon.open_menu(player.uuid, 3, "Pick", {
    { id = "minecraft:stick", count = 1 },
    nil, -- slot 1 empty
    { id = "minecraft:apple", count = 2 },
  })
end)
horizon.on("menu_clicked", function(event)
  if event.slot == 0 then horizon.close_menu(event.uuid) end
end)
```

## Permissions and moderation

A script can ask what a player may do, and can kick or teleport a player. All four helpers are server state only: like `horizon.broadcast` the two actions queue and run after the handler, and like `horizon.time` the two reads answer at once; they are not in `CLIENT_API` or `DRAW_API`.

Reads (answer now):

* `horizon.has_permission(player, node, default_level)` returns whether `player` has the permission node `node` (a string). `default_level` is an integer; it saturates into `u8` (below 0 arrives as 0, above 255 as 255) instead of wrapping. When no mod's `permission_check` handler answers, the answer is the player's operator level against `default_level` (`ops.json`; 0 lets everyone).
* `horizon.player_permission(player)` returns the player's permission level, an integer 0 to 4, or `nil` when the player is not on the server.

Actions (queued, run after the handler like `give_items`):

* `horizon.kick(player, reason)` disconnects `player` with `reason` (a Lua string, as `horizon.broadcast`) shown whole, as `/kick` does, at the tick's end.
* `horizon.teleport_player(player, dimension, pos, yaw, pitch)` teleports `player` as `/tp` does: `dimension` is one of `minecraft:overworld`, `minecraft:the_nether`, `minecraft:the_end`; `pos` is `{ x = ..., y = ..., z = ... }`; `yaw` and `pitch` are degrees, with `pitch` clamped to −90..90. The position must be finite and inside `/tp`'s bounds (x and z within ±30M, y within ±20M).

`player` is the uuid text a hook event gives, or a player table, exactly as the net-send helpers accept it (`string | ReadPlayer`). Both actions return nothing to the script: the native's `bool` (for a player not on the server, a bad dimension, an out-of-bounds position, or a dead player) is dropped, and a refusal is a silent no-op, never a script error. A non-finite number (NaN or infinity) is a script error naming the argument (`teleport_player.pos.x`, `teleport_player.yaw`, `teleport_player.pitch`).

A player who cannot be resolved (an unknown name, a bad uuid): `player_permission` is `nil`; `has_permission` counts the player as level 0 — `true` when `default_level == 0`, `false` otherwise — without asking the host; a parseable uuid does reach the host. `kick` and `teleport_player` are silent no-ops.

`has_permission` asks every mod's `permission_check` handler first: a permissions mod answers by setting `event.result`, and the first handler that sets it decides. Inside a `permission_check` handler itself, `horizon.has_permission` raises like every read does there (`reads work inside a hook with a host`), since that hook has no host to read from.

```lua
-- One command: an operator sends a name to the nether; anyone else is kicked.
horizon.command("send", 2, function(player, args)
  if not horizon.has_permission(player.uuid, "ruby:send", 2) then
    horizon.kick(player.uuid, "You may not use this command")
    return
  end
  horizon.teleport_player(player.uuid, "minecraft:the_nether", { x = 0, y = 70, z = 0 }, 0, 0)
end)
```

## Not built yet

These are what a gameplay mod asks for next, with the API each is planned to get, so that a mod can be written
toward it. None of it is built or tested. The names are the design, and the final spelling may differ. The engine's
own helpers ([hook-points.md](api/hook-points.md)) are the limit of what a script can reach today. A row leaves this
table when its step lands, with the tests and the game check that show it.

| Feature | Planned shape |
|---|---|
| A mob's attributes, equipment and name at runtime | Host helpers over a mob by id (`set_attribute(dimension, id, name, value)`, `equip(dimension, id, slot, stack)`, `set_name(dimension, id, text)`), then `horizon.set_attribute(id, "max_health", 40)`, `horizon.equip(id, "mainhand", "minecraft:iron_sword")` and `horizon.set_name(id, "Old One")` from any hook with a host, and `mob:set_attribute`, `mob:equip` and `mob:set_name` in the modkit. Until then, a mod declares attributes and equipment in its entity type (`registry.entity_type`), and names mobs through `run_command` |
| Custom projectiles and other entities that are not mobs | A thrown mod item already flies as its template does (a snowball's `template` in `registry.item`). The planned shape is an entity type of the mod's own over a projectile template (`registry.entity_type("spore", { template = "minecraft:snowball", ... })`), with `projectile_impact` by that kind, and `horizon.shoot(dimension, kind, x, y, z, vx, vy, vz)` to launch one. Vehicles, displays and markers stay the engine's own |
| Screens | Content mods have menu types (`data/<id>/horizon/menu/`), but a screen that draws them is native only (`native_screen`). Scripts get no screens: titles, subtitles, the action bar, chat lines and commands are their user interface. A later content-menu helper would show a content mod's menu |
| Client-side scripts | A client state exists when `scripts/client/` does and runs the client hooks (those tagged `client` or `both` in [docs/api/hook-points.md](api/hook-points.md)). A server state runs the server hooks. A mod that registers the other side's hook is refused at load with an error that names the folder to use. Saved data (`horizon.world_data()`, `horizon.player_data`) is server state; the client reads server data only through messages (client-to-server messages are planned, saved data on the client is planned) |
| Configuration | `horizon.config()` and `horizon.save_config(t)` are all of it: one JSON file that a person edits. There is no options screen and no setting per world; a mod that wants values per world keeps them in `horizon.world_data()`, and sets them with a command of its own |

## Working on a script

The developer tools are made for this ([dev-tools.md](dev-tools.md)). Start the game with `--dev-tools --dev-mod /path/to/mod`, then:

- Type `/lua` to open the console. Its first section lists the scripted mods, with their scripts, handlers, goals
  and last error. Its **reload** button reads a mod's scripts again and runs `init.lua` from the start at the next tick of that side (the server tick for a server row, the client tick for a client row), as `/reload` does, and shows the result. The mod's `horizon.log` and `print` lines, and its errors,
  appear there too, tagged `[mod_id]`, so you see a failing handler at once instead of finding it in the log.
- Type `/goals @e[type=zombie,sort=nearest,limit=1]` to see a mob's goal selectors. A Lua goal is named
  `mod_id:name`, marked running or idle, with a history of when it ran. `/senses` and `/nav` show what the goal's
  view sees (the target, the nearest player, navigation) and the path that the game walks for `out.navigate_to`.
- Type `/inspect <targets>` to see an entity's data as `/data get entity` prints it, and
  `/watch <targets> <path>` to chart one number of it over time.
- In the console, `mc.mods()` and `mc.reload_mods([id[, side]])` do what the console's mod section does, for a script of
  the tools' own.
- In the console, `mc.profile()` reports each mod's hook cost since the last reset, keyed by mod id and side then hook
  name (a server state by `"<id>"` and a client state by `"<id>/client"`): each hook's table carries `calls` (hook firings: one per firing, however many handlers the mod has for
  the hook), `total_ms` (all firings), `max_ms` (the slowest hook firing) and `overruns` (firings the time budget
  stopped). The times are fractional milliseconds. `mc.profile_rows()` gives the same numbers as an array of
  `{mod=, side=, hook=, calls=, total_ms=, max_ms=, overruns=}` sorted by mod id, side, then hook name, for a listing whose order
  matters. `mc.profile_reset()` zeroes every counter.
- In the console, `mc.mod_state(id)` reads a loaded scripted mod's saved data on the server thread, read only: a table
  with `world_data` (the `horizon.world_data()` table) and `players`, each online player's `horizon.player_data()` table
  keyed by uuid text. An id that names no loaded mod is a Lua error, `unknown mod <id>`. A Lua number is a double, so a
  long past 2^53 loses digits here. `mc.mod_state_json(id)` returns the same data as JSON text: integers are exact (a
  long past 2^53 too); a float is its value widened to a double (a float 0.1 prints as 0.10000000149011612), and NaN
  and the infinities are `null`; a table nested past 64 levels is an error. `mc.emit_mod_state(id)` prints that JSON
  as one marked line, which `horizon-ctl mod-state ID` queues on this console and reads (the reply is that line, plus
  `ok` and `id`; a `print` the script or a `/lua` snippet rebound does not matter). Entity data
  (`horizon.entity_data`) is not read by either: no per-entity enumeration.
- In the console, `mc.set(entity, path, value)` and `mc.merge(entity, value)` convert a Lua value to SNBT before
  touching the entity: a table that holds itself, or nested past 128 tables, is an error rather than an abort. A
  tag nested past 128 levels is likewise an error where it is read back as a Lua table (`mc.get`, `mc.mod_state`).

## Plan

| Step | What | Why |
|---|---|---|
| 4 | Hooks covered by generation: the `lua-hooks` generator emits the Lua event table for every hook the model covers, as `xtask mod-api` emits the trait | Every covered hook point reaches Lua without hand-written glue; the rest are listed with their reason in [hook-exposure.md](api/hook-exposure.md) |

The block hooks needed one addition to the mod API: `LevelHost::state_block` and `state_properties`, so a hook that
gets a state can name its block without a world lookup. Trait mods get them too. Everything stays off unless a mod
carries scripts, and the launcher needs no change: a scripted `.hmod` installs like any content mod.
