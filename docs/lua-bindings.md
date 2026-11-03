# Lua Scripting for Mods

This page describes how content mods carry Lua scripts that the game runs on its hook points. It covers the
runtime, the `registry` and `horizon` tables, every hook a script can listen to, and the limits a script runs
under. It lives on the `lua-bindings` branch of whos-carmen/horizon, which builds on `mob-ai-hooks`.

Most mods are easier to write with [Horizon Modkit](https://github.com/whos-carmen/horizon-modkit), a library over
this API with classes and callbacks. This page is the layer under it.

| Part | Status |
|---|---|
| The runtime (`crates/horizon-client/src/lua_mods.rs`) and scripts inside the `.hmod` | built |
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
scripted mod, in the same list and order as trait mods. Every hook point reaches Lua through one adapter method, so a
hook added for Rust becomes available to scripts for the price of that method.

- The language is Luau, through `mlua` with the `luau` feature. It is built from source: no system library and no
  dynamic loading, the same on Metal and Vulkan. Each mod has its own Lua state. `Lua` is `Send`, so it lives in the
  `Hooks` object.
- Scripts never hold the host. Calls such as `horizon.broadcast(...)`, `horizon.summon(...)` and
  `horizon.kill_mob(...)` are queued. The runtime runs them in order when the handler returns, with the host that the
  hook was given. There is no unsafe code, and Lua never re-enters the game.
- An error in a script is logged with the mod's id, and never stops the game.

## Getting started

`cargo horizon-mod new my-mod --template lua` writes a scripted mod that depends on the modkit:

- a manifest
- `scripts/registry.lua`, with one item
- `scripts/init.lua`, with the item's `on_use`
- a `.luaurc` for an editor
- a README

`cargo horizon-mod run -- --dev-mod /path/to/horizon-modkit` runs it.

To play a mod, install its `.hmod` and turn it on, or start the game with `horizon-client --dev-mod /path/to/mod`.
The game gathers scripted mods when it starts, as it does trait mods, so turning one on or off on the Mods page takes
effect at the next start. A mod without capability `scripts` is content only, as before.

A mod's entry point is `scripts/init.lua`. The mod needs capability `scripts` and `experimental = ["scripts"]` in
its manifest, and its id comes from its `horizon-mod.toml`. `cargo horizon-mod build` packs `scripts/` next to
`assets/` and `data/`; `horizon_mods::Content` lists the scripts, checks their paths, and checks that
`scripts/init.lua` is there. The Mods page says "May run its own scripts in your worlds".

This script counts natural zombie spawns:

```lua
-- scripts/init.lua
local spawns = 0
horizon.on("mob_spawned", function(dimension, kind, x, y, z, natural)
  if kind == "minecraft:zombie" and natural then
    spawns = spawns + 1
    horizon.log(("zombie %d at %.0f %.0f %.0f"):format(spawns, x, y, z))
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
64 MiB. It has no `require`, because there is no other script to reach, no `loadstring` and no files.

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

A table whose keys are 1 to n becomes a JSON array, and any other table an object; `{}` is an empty object. A whole
number becomes an integer.

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

## The `horizon` table

Calls that act on the world are *actions*: they are queued, and run after the handler returns. Calls that read the
world are *reads*: they answer at once, from the hook's host, for the length of the call. Both need a hook with a
host. `mob_hurt`, `mob_ai_register` and goal functions have no host; a goal reads its `mob` view instead. A level hook
(`random_tick`, `tick_block`) reads blocks only.

An action called where no hook's handlers run is an error at the call. This covers a goal function, which acts
through its `out` table, and the top level of `init.lua`, which registers handlers. Nothing would run the action,
and it would otherwise wait in the queue for the life of the state.

Some functions take a *player*. A player is a `player` table that a hook handed out, a name or a UUID.

### The mod

| Function | Description |
|---|---|
| `horizon.on(name, fn[, kind])` | Registers a handler for a hook. The game refuses a *name* that it does not know when the script loads, with the line; the names are `LUA_HOOK_NAMES` in `lua_mods.rs`, the hooks under [Hooks](#hooks). With *kind* (`"minecraft:zombie"`, `"zombie"`, or a mod's own id), the handler runs only for that kind, after the handlers for every kind. A kind that no handler watches costs nothing |
| `horizon.log(msg)` | Writes a line to the game log, tagged `[lua:<id>]` |
| `horizon.mod_id`, `horizon.mod_version` | the mod's id and version, from its manifest |
| `horizon.libraries` | the versions of the mods it depends on, by id, such as `horizon.libraries.horizon_modkit` |
| `require(name)` | Runs `scripts/<name>.lua` of the same mod, or of a library, once, and returns its value |
| `horizon.ticks()` | Gets the number of server ticks seen since the scripts loaded |
| `horizon.distance(a, b)` | Gets the distance between two `{x, y, z}` tables |

### Mob goals

| Function | Description |
|---|---|
| `horizon.goal(name, def)` | Defines a mob goal. *def* has `flags` (any of `"move"`, `"look"`, `"jump"` and `"target"`), `every_tick`, `interruptable`, and the functions `can_use(mob, state, out)` (required), `can_continue`, `start`, `tick` and `stop`. *mob* is the [mob view](#the-mob-view), *state* a table of the goal's own that lives with the mob, and *out* the [goal actions](#goal-actions). Call it at load |
| `horizon.add_goal(priority, name)`, `horizon.add_target_goal(priority, name)` | Adds a defined goal to the mob's goal selector or target selector. In `mob_ai_register` |
| `horizon.clear_goals()`, `horizon.clear_target_goals()` | Removes every goal from a selector. In `mob_ai_register` |

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
horizon.on("mob_ai_register", function(mob)
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
| `horizon.player(name_or_uuid)` | Gets one player by name or UUID, or nil |
| `horizon.time()` | Gets `{dimension, day_time, day, time_of_day, rain, thunder}`: the clock in ticks, `day` and `time_of_day` (0 to 23999) from it, and the rain and thunder levels from 0 to 1 |
| `horizon.raycast(player, distance)` | Gets the first block within *distance* (64 by default, 1024 at most) along the player's look: `{id, x, y, z, block = {x, y, z}}`, the point hit and the block, or nil |
| `horizon.light(x, y, z)` | Gets `{block, sky}` light at the position, from 0 to 15; both are 0 where nothing is loaded |
| `horizon.surface_height(x, z[, heightmap])` | Gets the column's height by `world_surface` (the default), `motion_blocking`, `motion_blocking_no_leaves` or `ocean_floor`; nil where nothing is loaded |
| `horizon.data(path)` | Reads a data file of the game's resources, content mods' included (`data/my_mod/spawns.json`): JSON as a table, any other file as a string, nil when there is none |
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
| `horizon.sound(name, x, y, z[, volume, pitch])` | Plays a sound event for the players in range, 16 blocks or more with a volume above 1, as `/playsound` does |
| `horizon.particles(name, x, y, z[, count, spread, speed])` | Shows *count* particles of a simple type (`minecraft:flame`, `minecraft:happy_villager`) spread by *spread* blocks, for the players within 32 blocks, as `/particle` does |

`horizon.run_command` and `horizon.set_nickname` work only where no player's state is swapped in: a timer,
`server_tick`, a mob or block tick, a death. Inside a player's own hook, such as a click, a command or a pickup,
nothing runs, and the log says so at debug level. Queue the call there with
`horizon.after(1, function() horizon.run_command(...) end)`. `run_command` is the way to reach what the API has no
function for yet, such as the time, the weather, `/tp` of a player and game rules.

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
it as vanilla's commands. Call it at load, as goals are defined.

- *usage*: the command's words, then its arguments, such as `"parasite count [kind:word]"`. `<name:kind>` is
  required: when it is missing, or an `integer` or `number` is not one, the player gets a usage line and the handler
  is not called. `[name:kind]` is optional. The kinds are `word`, `text` (the rest of the line, last), `integer`,
  `number`, `boolean`, `player`, `item` and `block_pos`
- *level*: the operator level a player needs; 0 is everyone, and nil is 0
- *fn*: called as `fn(player, args)`. *args* holds the arguments by name, numbers as numbers and the rest as words,
  and `args.raw`, the whole line. `horizon.feedback(text)` answers the player, and the host actions work

### Timers and saved data

| Function | Description |
|---|---|
| `horizon.after(ticks, fn)` | Runs *fn* once after *ticks* server ticks |
| `horizon.every(ticks, fn)` | Runs *fn* every *ticks* server ticks |
| `horizon.world_data()` | Gets the mod's saved table on the world |
| `horizon.player_data(player)` | Gets the mod's saved table on a player, by name or UUID; nil for a player the server does not hold, such as a player on the hub or the acting one |
| `horizon.config()` | Gets the mod's settings, `config/<id>.json` in the game folder, as a table; empty when there is no file yet. Every mod's settings share that folder, as on NeoForge |
| `horizon.save_config(t)` | Writes a table to the mod's settings file, as JSON that a person can edit |

Timers run at the end of a tick while the world runs, as a `server_tick` handler of the mod's own, so reads and
actions work in them. A timer that fails is logged, a repeating one is kept, and the timers after it run as usual.
`/reload` drops the timers with the handlers.

The saved tables start empty. They are written back after the call, like a mob's `data`, and kept through saves as
the engine's attachments under the mod's id (`horizon:attachments`, names `world` and `player`).

`horizon.config` and `horizon.save_config` work at load, in `init.lua`, with no host. A mod reads its settings, fills
in the defaults and saves them, so the file is there to edit after the first run.

### Blocks

`horizon.block_entity(block, { ticks = true, data = {...} })` gives the mod's block a block entity. It is made with
*data*, a table of numbers, strings, booleans, nested tables and sequences, when the block is placed, and saved with
the chunk. `ticks` makes it tick. Call it at load, in `init.lua`.

Some actions work only in the block hooks:

- `horizon.set_block_property(key, value[, flags])`: sets a state property of the block; *flags* are 2 by default,
  which tells the clients. In `random_tick`, `tick_block`, `right_click_block` and `block_entity_tick`
- `horizon.schedule_tick(delay)`: asks for a scheduled tick. In `random_tick` and `tick_block`
- `horizon.handled()`: skips the block's own random tick. In `random_tick` and `tick_block`

## Hooks

A handler receives the hook's arguments. Some hooks let it change the event, with calls such as
`horizon.cancel()` or `horizon.set_amount(n)`; the table says which. A hook "by kind" takes a `kind` in
`horizon.on`, a block id, an item id or a mob kind.

### The game and the world

| Hook | Called when |
|---|---|
| `server_tick(phase, world_running)` | a server tick starts or ends |
| `world_lifecycle(phase)` | the world opens or closes |
| `explosion(dimension, center, blocks)` | an explosion is about to break *blocks*, a sequence of `{x, y, z}`. Remove entries to spare those blocks: they keep their block and get no fire |
| `entity_joined(dimension, id, kind, x, y, z, loaded)` | any entity joins the level, such as `minecraft:item`, `minecraft:arrow` or a mob; *loaded* is true for one that comes back with its chunk. By kind. `cancel()` keeps it out. `mob_spawned` stays for mobs |
| `projectile_launched(projectile, kind, player)` | a projectile is added to the level; *player* is the shooter's table, or nil for a mob's or a dispenser's. By projectile kind. It only listens |
| `projectile_impact(projectile, kind, owner, hit)` | a projectile, such as an arrow, a snowball, an egg, a potion or a fireball, hits something, before the hit acts. *owner* is the shooter's entity id, or 0. *hit* has `x`, `y`, `z`, and either `entity` and `entity_kind`, or `block` (`{x, y, z, id}`) and `face`. By projectile kind. `cancel()` and it flies on. The engine keeps the copy each impact needs only while a script listens |

### Mobs

| Hook | Called when |
|---|---|
| `mob_spawned(dimension, kind, x, y, z, natural)` | a mob spawns |
| `mob_ai_register(mob)` | a mob's goals are set up. Handlers call `horizon.add_goal` and the other goal functions |
| `mob_tick(dimension, mob, data)` | a mob has ticked (`entity_tick_post`). *mob* is its view without the fields that need the world (`nearby` is empty, `target_position` is nil). The host actions work. Register it by kind: a handler for every kind hears every mob of the level each tick |
| `mob_interact(dimension, mob, player, held, data)` | a player right-clicks a mob, before the mob and the item act. *mob* is its view, without `nearby` and `target_position`; a mob the host has no view of, because it is dead or gone, is not reported. *held* is the stack of the hand being tried, main then off. `cancel()` takes the click. The host actions work |
| `mob_hurt(mob, damage_type, causing, direct, amount)` | a mob is about to take damage (`living_incoming_damage`). *mob* is `{id, kind}` only; the game is in the middle of the hit, so there is no host, no view and no data. *damage_type* is such as `minecraft:mob_attack` or `minecraft:on_fire`; *causing* and *direct* are the entities behind it, or -1. `set_amount(n)` and `cancel()`. The host actions are dropped |
| `mob_killed(dimension, mob, killer, data)` | a player's melee blow kills a mob. *mob* is `{id, kind}`, and *killer* the player's table. `mob_died` covers every other cause |
| `mob_died(dimension, mob, cause, data)` | a mob dies, by any cause: a blow, an arrow, fire, a fall or `/kill`. *mob* is `{id, kind}`. *cause* is `{damage_type, causing, player}`: *causing* is the mob behind it or 0, and *player* the player credited with the kill, or nil. *data* is the mod's data while the game still holds it. By kind |
| `mob_fall(mob, distance, multiplier)` | a living entity is about to handle a fall. No host; *mob* is `{id, kind}`. `set_distance(n)`, `set_multiplier(n)`, and `cancel()` for no fall damage. By kind |
| `mob_heal(mob, amount)` | a living entity is about to heal. No host; *mob* is `{id}`, because the engine names no kind here, so every handler hears every entity, players too. `set_amount(n)` and `cancel()` |
| `mob_knockback(mob, damage_type, causing, direct, strength, x, z)` | a living entity is about to be knocked back. No host. *strength* is the raw value, and *x* and *z* the direction. `set_strength(n)` and `cancel()`. By kind |
| `mob_equipped(mob, slot, from, to)` | an equipment slot of a mob (`mainhand`, `offhand`, `feet`, `legs`, `chest`, `head`, `body` or `saddle`) takes a different stack. *from* and *to* are item tables, or nil. By kind. It only listens |
| `loot_drops(source, drops)` | a block broken by a player, an explosion or a mob, or a mob that died, is about to drop its loot. By block id or mob kind. *source* has `table` (the loot table's id), `dimension`, `origin`, and `block` (`{x, y, z, id, state}`) or `mob` (`{id, kind}`), `player` (the one credited, or nil), `tool` (an item table, or nil) and `explosion` (a radius, or nil). *drops* is a sequence of `{id, count, damage, plain}`: change it in place, and it is written back after the call. An entry whose id matches the stack at that place keeps the stack's components |

### Blocks

| Hook | Called when |
|---|---|
| `random_tick(dimension, block, x, y, z, props, data)` | a block that ticks randomly does: a mod's block as its template does, or one with `random_ticks`. Register it by block id: a handler for every block hears every random tick of the level, hundreds a tick. *props* holds the state's properties by name, and *data* the block entity's saved table, when the block has one |
| `tick_block(dimension, block, x, y, z, props, data)` | a scheduled tick of the block is due. Register it by block id |
| `right_click_block(x, y, z, block, face, player, held, data)` | a player right-clicks a block, before the block and the item act. *face* and `player.position` are `{x, y, z}`; *held* has `item`, `count` and `plain`. `cancel()` skips the block and the item's use on it for that hand, with no swing. See the note below |
| `block_entity_tick(dimension, block, x, y, z, data)` | a server tick of a block entity declared with `ticks`. *data* is its saved table. The host actions and `set_block_property` work |
| `block_broken(x, y, z, block, player)` | a player breaks a block, once the game has allowed it. `cancel()` keeps the block, so nothing breaks or drops. By block id |
| `block_placed(x, y, z, block, player, item)` | a player has placed a block from *item*, the stack as it was. It only listens; to stop a placement, take the click in `item_use_on`. By block id |

`right_click_block` runs once for each hand, the main hand and then the off hand, as the game tries both. NeoForge's
`RightClickBlock` fires for each hand too. *held* is the main hand's stack both times. Make a handler do something
that is the same when done twice, such as lighting a lamp rather than toggling it, or read the block's state first.

### Items

| Hook | Called when |
|---|---|
| `item_use(item, player)` | a player right-clicks with an item in the air, or on a block that lets the click through, before the item's own use. *item* is `{id, count, damage, plain}`. `cancel()` takes the use: the hand swings, and nothing else runs. By item id |
| `item_use_on(item, player, block, face)` | the same, on a block. *block* is `{x, y, z, id}`, and *face* `{x, y, z}`, with `{0, 1, 0}` the top. By item id |
| `item_hit(item, player, target)` | a player attacks *target* (`{id, kind, position}`) holding *item*, before the blow. An empty hand is `minecraft:air`. `cancel()` and the blow does nothing. By item id |
| `item_hurt_enemy(item, player, target)` | a player's blow has landed on *target* (`{id, kind}`) holding *item*, after the damage. By item id. It only listens |
| `item_use_start(item, player, hand, duration)` | a player starts to use an item over time: eating, drinking, drawing a bow, or a mod item with a `use_duration`. *hand* is `"main"` or `"off"`, and *duration* the ticks it takes. `set_duration(ticks)` changes it, and `cancel()` stops the use. By item id |
| `item_finished(item, player)` | a player has finished using an item over time: eaten or drunk it, or held a mod item for its `use_duration`. `set_result(id, count)` sets what stays in the hand; `set_result()` leaves nothing |
| `item_tick(item, player, slot, selected)` | a server tick of a stack that a player carries. *slot* is as the inventory saves it: 0 to 35, armour 100 to 103, the off hand -106. *selected* is true for the held stack. `set_count(n)` changes the stack; 0 empties the slot. Every stack of every player ticks, so register it by item id |
| `item_crafted(player, recipe, item)` | a player takes a crafting grid's result. *recipe* is its id, and *item* the stack. By the result's item id. It only listens |
| `item_dropped(player, item, count)` | a player throws an item, with the drop key or from a menu; items scattered at death do not count. By item id. It only listens |
| `item_picked_up(player, item, count)` | a player is about to pick up *count* of *item* (an id) from the ground. `cancel()` leaves them there. By item id |

### Players

| Hook | Called when |
|---|---|
| `player_joined(player)`, `player_left(player)` | a player joins the game, or leaves it by any path |
| `player_respawned(player)` | a player respawns after a death |
| `player_died(player, cause)` | a player dies. *cause* is the death message's key, such as `death.attack.lava`, or nil |
| `player_tick(player)` | a living player's server tick has ended. Every player, every tick, with the host actions and the reads. Keep it light. It takes no `kind` |
| `player_hurt(player, damage_type, causing, direct, amount)` | a player is about to take damage, as `mob_hurt` reports it for a mob. *player* is `{id, kind}` only, with no host and no name. `set_amount(n)` and `cancel()` |
| `player_fall(player, distance, multiplier)` | as `mob_fall`, for players |
| `player_knockback(...)` | as `mob_knockback`, for players |
| `player_command(player, line)` | a player types any command, before it runs. *line* has no slash. `cancel()` and nothing runs. `reply(text)` answers them; no player's state is in place here, so `tell` and the item functions reach nobody |
| `player_chat(player, line)` | a player's chat line is about to go to everyone, after the chat filter. *player* is `{uuid, name}`; there is no host, because the hub is in the middle of a packet. `reply(text)` sends a line back to the sender. `cancel()` and nobody sees it, the sender included. The line itself can't change, because a signed message would no longer verify; the hook point is upstream's `native_player_chat`. Commands go to `player_command` |
| `player_changed_dimension(player, from, to)` | a player's move to another level has ended: a portal, `/execute in ... run tp`, or a respawn elsewhere. The engine reports it on a dedicated server only |
| `player_bed_enter(player, x, y, z)` | a player is about to enter the bed at that position. `cancel()` keeps them out |
| `player_exp_change(player, amount)` | a player's experience is about to grow by *amount* from an orb, after Mending's share. `set_amount(n)` |
| `player_container_open(player, menu, pos)` | a player opens a menu. *menu* is its type, such as `minecraft:generic_9x3`, `minecraft:furnace` or `minecraft:crafting`, and *pos* the position of its block, or nil |

## Shared tables

### `player`

The `player` table in the hooks has these fields:

- `uuid` and `name`
- `dimension`, and `position` as `{x, y, z}`
- `game_mode`: 0 survival, 1 creative, 2 adventure, 3 spectator
- `health`
- `held`, for the player whose action the hook reports, such as a click, a command or a break: the main hand's
  `{item, count, plain}`, or nil when it is empty

A player that the world has no record of at that moment, such as one who is dying, has `uuid`, and the `name` that
an earlier player hook reported.

### Saved tables

A mob's `data` is the mod's own saved table on the mob, empty at first. It is kept with the mob as the engine's
attachment under the mod's id (`horizon:attachments` in its NBT), so it survives a save and a restart, and other
mods' data stays apart. It is nil when the host has no such mob.

A saved table, whether a mob's `data`, a block entity's, `world_data`, `player_data` or `save_config`'s, follows
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
handler signature, and the tables they hand a script, such as `Player`, `MobView`, `Item`, `Block`, `Hit` and
`LootSource`. It is generated by `cargo run -p xtask -- lua-types`; the tests run it with `--check`.

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

With `--resources <26.3 folder>` (the prepared vanilla folder), it also layers the mod's content on that folder, as the
game does at start, and exits with code 1 when the game would refuse it. A refused file leaves every content mod out
of the world, such as an `add_spawns` spawner with `min_count` where NeoForge writes `minCount`. A content mod without
scripts gets only this check.

`cargo horizon-mod check <folder> --game <horizon-client> --dev-mod <library> [--resources <26.3 folder>]` runs it
after the registry's checks.

## Reloading

`/reload`, and any data load after the first, reads the mod's scripts again and runs `scripts/init.lua` from the
start. Handlers, timers, goals, commands, block entity declarations and `require`d modules are dropped. The Lua state
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

Some builds layer the content again on `/reload`: the server starts from the content root it plays on, which stands
for the base it was built from. Such a build takes the mod's data and assets as they are now, and its declarations
as they were at start:

- A changed declaration is kept as it was at start.
- A new mod with declarations is left out.
- A mod that is gone keeps its declarations, with no files.

Each case writes a `Horizon: mod <id>: ...` line to the log, which says that a restart applies it. A mod that
declares what it declared at start is layered without a message.

## Limits and errors

A handler, or a goal function, that runs longer than 250 ms is stopped by Luau's interrupt. A hook's handlers share
one budget: 250 ms of wall-clock time for each handler registered, as one call. The interrupt raises an error that
behaves like any other error of the handler: it goes to the log and the dev tools' console, and a goal reads it as
"no". A `pcall` around the slow code catches that error, but the next loop iteration or call raises it again, so
nothing of the handler runs past its deadline, and the actions it queued are dropped. This holds for the timers' own
`pcall` too.

`init.lua`, with the modules it requires, has five seconds at load and at reload. A state may use up to 64 MiB.

A script reads nothing but its own mod (`lua_mods::Budget`):

- Luau has no `io`, `loadfile` or `dofile`.
- Its `os` has only `clock`, `date`, `difftime` and `time`.
- `require` is the game's own, over the mod's `scripts/` and its libraries.

A hook's handlers run in registration order, as one call. When one of them fails, through an error or the budget,
the handlers after it do not run for that event. Nothing that the handlers before it asked for reaches the game:
neither their actions, `cancel()` included, nor their changes to a saved table (`data`, `world_data`,
`player_data`). The event goes on as if the mod had not heard it, and the log has the error. So a mod that must not
lose work splits the work across hooks, or guards it with `pcall` itself.

## Not built yet

These are what a gameplay mod asks for next, with the API each is planned to get, so that a mod can be written
toward it. None of it is built or tested. The names are the design, and the final spelling may differ. The engine's
own helpers ([hook-points.md](api/hook-points.md)) are the limit of what a script can reach today. A row leaves this
table when its step lands, with the tests and the game check that show it.

| Feature | Planned shape |
|---|---|
| A mob's attributes, equipment and name at runtime | Host helpers over a mob by id (`set_attribute(dimension, id, name, value)`, `equip(dimension, id, slot, stack)`, `set_name(dimension, id, text)`), then `horizon.set_attribute(id, "max_health", 40)`, `horizon.equip(id, "mainhand", "minecraft:iron_sword")` and `horizon.set_name(id, "Old One")` from any hook with a host, and `mob:set_attribute`, `mob:equip` and `mob:set_name` in the modkit. Until then, a mod declares attributes and equipment in its entity type (`registry.entity_type`), and names mobs through `run_command` |
| Custom projectiles and other entities that are not mobs | A thrown mod item already flies as its template does (a snowball's `template` in `registry.item`). The planned shape is an entity type of the mod's own over a projectile template (`registry.entity_type("spore", { template = "minecraft:snowball", ... })`), with `projectile_impact` by that kind, and `horizon.shoot(dimension, kind, x, y, z, vx, vy, vz)` to launch one. Vehicles, displays and markers stay the engine's own |
| Screens | Content mods have menu types (`data/<id>/horizon/menu/`), but a screen that draws them is native only (`native_screen`). Scripts get no screens: titles, subtitles, the action bar, chat lines and commands are their user interface. A later `horizon.open_menu(player, menu, pos)` would show a content mod's menu |
| Client-side scripts | Every script runs on the server side of the game: the window's own server in single player, the host's in LAN. `side` in the manifest is the content's. There is no HUD, key binding, rendering or sound of the client's own; a server script reaches the client through the engine's packets, such as titles, sounds and particles. A client script would be a second state, with its own hooks (`hud`, `key`, `render_*`), kept apart |
| Configuration | `horizon.config()` and `horizon.save_config(t)` are all of it: one JSON file that a person edits. There is no options screen and no setting per world; a mod that wants values per world keeps them in `horizon.world_data()`, and sets them with a command of its own |

## Working on a script

The developer tools are made for this ([dev-tools.md](dev-tools.md), on the `egui-tools` branch, which is stacked on
this one). Start the game with `--dev-tools --dev-mod /path/to/mod`, then:

- Type `/lua` to open the console. Its first section lists the scripted mods, with their scripts, handlers, goals
  and last error. Its **reload** button reads a mod's scripts again and runs `init.lua` from the start at the next
  server tick, as `/reload` does, and shows the result. The mod's `horizon.log` and `print` lines, and its errors,
  appear there too, tagged `[mod_id]`, so you see a failing handler at once instead of finding it in the log.
- Type `/goals @e[type=zombie,sort=nearest,limit=1]` to see a mob's goal selectors. A Lua goal is named
  `mod_id:name`, marked running or idle, with a history of when it ran. `/senses` and `/nav` show what the goal's
  view sees (the target, the nearest player, navigation) and the path that the game walks for `out.navigate_to`.
- Type `/inspect <targets>` to see an entity's data as `/data get entity` prints it, and
  `/watch <targets> <path>` to chart one number of it over time.
- In the console, `mc.mods()` and `mc.reload_mods([id])` do what the console's mod section does, for a script of
  the tools' own.

## Plan

| Step | What | Why |
|---|---|---|
| 4 | Hooks covered by generation: `xtask mod-api` emits the Lua adapter for every `listen` hook from the catalogue, as it emits the trait | Every hook point reaches Lua without hand-written glue |

The block hooks needed one addition to the mod API: `LevelHost::state_block` and `state_properties`, so a hook that
gets a state can name its block without a world lookup. Trait mods get them too. Everything stays off unless a mod
carries scripts, and the launcher needs no change: a scripted `.hmod` installs like any content mod.
