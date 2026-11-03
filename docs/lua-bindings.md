# Lua as a modding language

Status: the runtime (`crates/horizon-client/src/lua_mods.rs`), scripts inside the `.hmod`, mob goals in Lua
(`lua_mods/goals.rs`), block behaviour (random and scheduled ticks, right clicks), reload on `/reload` and the
limits (time budget, memory cap) are built. The branch builds on `mob-ai-hooks`. Branch
`lua-bindings` of whos-carmen/horizon, `main` + this one concern.

## Why

A content mod (`.hmod`: data and assets) that needs behaviour today has one way to get it: a native mixin bundle,
Rust that the launcher compiles into the game ([native-mods.md](native-mods.md)). That is a source patch with the
player's rights, pinned to a Horizon commit, and two bundles can collide on the same function. A content team
should not have to learn Rust, ship patches or pin commits to give a block a tick or a mob a behaviour.

The goal: **scripts travel inside the content mod**, beside `data/` and `assets/`, and the game runs them. No
compile step, nothing patches the game, so two scripted mods never conflict in the build. Rust stays the way to add
a hook point or something fast; Lua is the way to use them.

## How it fits

The game already has one event surface shared by everything that is not a mixin: the hook points of
`native-hooks.json`, dispatched to a list of `Hooks` trait objects (`horizon_client::mod_api`). The Lua runtime is
one more `Hooks` implementation per scripted mod, in the same list and order as trait mods, so every hook point
reaches Lua by one adapter method, and a hook added for Rust is available to scripts for the price of that method.

- Luau through `mlua` (feature `luau`, built from source: no system library, no dynamic loading, the same on Metal
  and Vulkan). One state per mod; `Lua` is `Send` so it lives in the `Hooks` object.
- Scripts never hold the host. A handler's `horizon.broadcast(...)`, `horizon.summon(...)`, `horizon.kill_mob(...)`
  are queued and run by the runtime when the handler returns, with the host the hook was given, in order. No
  unsafe code, no reentrancy into the game from Lua.
- Errors in a script are logged with the mod's id and never stop the game.

## Libraries: a mod's scripts over another mod's

A mod lists the mods it needs in its manifest, `depends = ["horizon_modkit"]`: the game loads them first and does
not run a mod whose dependency is not installed or not on (the log says which). `require("modkit")` then finds
`scripts/modkit.lua` (or `scripts/modkit/init.lua`) in the mod's own scripts first, then in each dependency's, in
the manifest's order; a dotted name is a folder (`require("modkit.entity")` is `scripts/modkit/entity.lua`). The
library's code runs in the requiring mod's own Lua state, as a Lua module does; the library's own `init.lua` runs in
the library's state only. `/reload` reads the libraries again too. The intended library is
[whos-carmen/horizon-modkit](https://github.com/whos-carmen/horizon-modkit): entities, blocks and goals as classes
with lifecycle callbacks, the API a mod developer writes against, over this file's hook API.

## Registering content from Lua: `scripts/registry.lua`

A scripted mod may declare its entity types, blocks, items, mob effects, creative tabs and menu types, and write
any other data or asset JSON (loot tables, recipes, tags, lang, models), in `scripts/registry.lua` instead of
files. The script runs once, when the game reads the mod's content (at start: `/reload` does not run it again),
in a Luau state of its own with nothing but the `registry` table, a 5 s budget and 64 MiB. What it registers
becomes content files in memory, checked and read exactly like the files in `data/` and `assets/`, so the rest
of the game never knows the difference; a path both in the mod and registered is an error, as is anything the
file would have been refused for. Textures stay files. The mod needs capability `registries` as well as `scripts`.

| | |
|---|---|
| `registry.id`, `registry.namespaces` | the mod's id and the namespaces it owns |
| `registry.entity_type(name, def)`, `.block`, `.item`, `.mob_effect`, `.creative_tab`, `.menu` | `data/<id>/horizon/<kind>/<name>.json`, `def` the file's JSON as a table ([mods.md](mods.md)); `name` may hold `/` |
| `registry.data(name, value)` | `data/<ns>/<name>.json`, `ns:` from the name or the mod's id: `"loot_table/entities/crawler"`, `"minecraft:tags/block/mineable/hoe"` |
| `registry.asset(name, value)` | `assets/<ns>/<name>.json`: `"lang/en_us"`, `"models/block/hive"`, `"blockstates/hive"` |
| `registry.file(path, value)` | any content path; `value` a table (written as JSON) or a string (written as it is) |

A table whose keys are 1..n is a JSON array, any other an object (`{}` is an empty object); a whole number is an
integer. The launcher's Mods page, built without Lua, lists the mod's files only.

Two things a mob needs that are data, not script: a **spawn egg** is an item whose id ends in `_spawn_egg` and names
the kind (`registry.item("crawler_spawn_egg", { template = "minecraft:zombie_spawn_egg" })` with its own texture and
model under `assets/`), and **natural spawning** is a NeoForge biome modifier
(`registry.data("neoforge/biome_modifier/crawlers", { type = "neoforge:add_spawns", biomes = "#minecraft:is_overworld",
spawners = { { type = registry.id .. ":crawler", weight = 20, min_count = 1, max_count = 3 } } })`), which puts the
mob in the natural spawns of those biomes in its kind's category (a mod mob's: its template's; say `category` to
choose). Both are read when the game builds its content, so they need a restart, not a `/reload`. A mob's own look is
data too: `model` (`registry.asset("models/entity/<name>", { texture_size, parts })`, cubes by part as a vanilla
layer is written), `animation` (a set by name) and `sounds` on its entity type ([mob-ai-hooks.md](mob-ai-hooks.md)).

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

## Every hook point, and which reach Lua

[docs/api/hook-points.md](api/hook-points.md) lists every hook point and host helper of the engine with its API
level and, for each hook a scripted mod hears, the Lua handler it reaches. It is generated
(`cargo run -p xtask -- hooks-doc`) from `native-hooks.json` and the hook functions' own docs, and a test keeps it
current, so it is the place to look for what the hooks below are built on.

## What is built

| | |
|---|---|
| Entry | `scripts/init.lua` of a mod with capability `scripts` (and `experimental = ["scripts"]`), installed in `horizon-mods/` and on, or a `--dev-mod` folder; the mod's id from its `horizon-mod.toml` |
| `horizon.on(name, fn[, kind])` | Register a handler: `server_tick(phase, world_running)`, `mob_spawned(dimension, kind, x, y, z, natural)`, `world_lifecycle(phase)`, the block hooks and the mob hooks below. With `kind` (`"minecraft:zombie"`, `"zombie"`, or a mod's own id), a mob hook's handler runs only for mobs of that kind, after the handlers for every kind; a kind no handler watches costs nothing |
| `horizon.log(msg)` | The game log, tagged `[lua:<id>]` |
| `horizon.broadcast(text)` | A chat line to every player |
| `horizon.summon(dimension, kind, x, y, z)` | `/summon`; in a level hook (`random_tick`, `tick_block`) the mob is made right after the level's pass, in that level |
| `horizon.kill_mob(dimension, id)` | `/kill` a mob |
| `horizon.remove_mob(dimension, id)`, `horizon.set_mob_health(dimension, id, health)`, `horizon.push_mob(dimension, id, x, y, z)`, `horizon.teleport_mob(dimension, id, x, y, z)` | Host actions on a mob by id, from any hook with a host: take it out with no death or loot, set its health (cut to its maximum), add to its velocity, move it as `/tp` does |
| `require("name")` | `scripts/name.lua` of the same mod, once |
| `horizon.goal(name, def)` | Define a mob goal: `def.flags` (`{"move", "look", "jump", "target"}`), `def.every_tick`, `def.interruptable`, and `can_use(mob, state, out)` (required), `can_continue`, `start`, `tick`, `stop`. `mob` is the mob's view, `state` a table of the goal's own that lives with the mob, `out` the actions |
| `mob_ai_register(mob)` | A hook: as a mob's goals are set up. Handlers call `horizon.add_goal(priority, name)`, `horizon.add_target_goal(priority, name)`, `horizon.clear_goals()`, `horizon.clear_target_goals()` |
| `mob` (the view) | `id`, `kind`, `position`, `velocity` (`{x, y, z}`), `yaw`, `head_pitch`, `health`, `max_health`, `age`, `on_ground`, `in_water`, `in_lava`, `hurt_time`, `last_hurt_by`, `target` (`"player"`, a mob id or nil), `target_position`, `target_in_melee_range`, `navigation_done`, `nearest_player`, `home` (`{x, y, z, radius}`), `nearby` (the mobs within follow range, nearest first: `{id, kind, position, distance, health, alive, hostile, baby, target}`) |
| `out` (the actions) | `navigate_to(x, y, z, speed)`, `navigate_to_target(speed)`, `stop_navigation()`, `move_toward(x, y, z, speed)`, `look_at(x, y, z)`, `look_at_target()`, `set_target(who)`, `jump()`, `attack(who)`, `swing()`, `hurt(who, amount)`, `convert(kind)` (this mob becomes one of `kind`, equipment and name kept), `summon(kind, x, y, z)`, `despawn()`, `heal(amount)`, `effect(name, ticks, amplifier)`; applied in order after the call |
| `horizon.distance(a, b)` | Between two `{x, y, z}` tables |
| `horizon.block(x, y, z)`, `horizon.block_property(x, y, z, key)` | Reads, answered at once from the hook's host: the block at that position of the level in place (`minecraft:air` where nothing is loaded) and one of its state properties (nil without). The reads work inside a handler of a hook with a host, for the length of the call; `mob_hurt`, `mob_ai_register` and goal functions have none (a goal reads its `mob` view), and a level hook (`random_tick`, `tick_block`) reads blocks only |
| `horizon.mob(id)`, `horizon.mobs()` | A living mob's view (as a goal sees it, without `nearby` and `target_position`) or nil; the views of every living mob of the level |
| `horizon.players()`, `horizon.player(name_or_uuid)` | The players of the world (a single-player world's owner among them, under the uuid the hooks name them by), in join order, as `player` tables (`uuid`, `name`, `dimension`, `position`, `yaw`, `pitch`, `game_mode`, `health`); one by name or UUID, or nil |
| `horizon.time()` | `{dimension, day_time, day, time_of_day, rain, thunder}`: the clock in ticks (`day` and `time_of_day` 0–23999 from it), the rain and thunder levels 0–1 |
| `horizon.command(usage, level, fn)` | A command of the mod's in the server's tree, for players of operator `level` (0: everyone) and above, completed in the chat box as vanilla's: `usage` is its words then its arguments, `<name:kind>` required and `[name:kind]` optional, kinds `word`, `text` (the rest of the line, last), `integer`, `number`, `boolean`, `player`, `item`, `block_pos` (`"parasite count [kind:word]"`). `fn(player, args)` runs it: `args` holds the arguments by name (numbers as numbers, the rest as words) and `args.raw`; `horizon.feedback(text)` answers the player; the host actions work. Declared at load, as goals are |
| `horizon.tell(player, text)`, `horizon.action_bar(player, text)`, `horizon.title(player, text)`, `horizon.subtitle(player, text)`, `horizon.title_times(player, fade_in, stay, fade_out)` | Host actions on one player (by name or UUID): a system chat line, the action bar, a title and its subtitle (shown with the next title), the fade times in ticks |
| `horizon.sound(name, x, y, z[, volume, pitch])`, `horizon.particles(name, x, y, z[, count, spread, speed])` | Host actions in the level in place: a sound event for the players in range (16 blocks, more with a volume past 1), as `/playsound`; `count` particles of a simple type (`minecraft:flame`, `minecraft:happy_villager`...) spread by `spread` blocks for the players within 32, as `/particle` |
| `horizon.world_data()`, `horizon.player_data(player)` | The mod's own saved table on the world, and on a player (by name or UUID; nil for one the server does not hold: a player on the hub, or the acting one), empty at first, written back after the call like a mob's `data`; kept through saves as the engine's attachments under the mod's id (`horizon:attachments`, names `world` and `player`) |
| `horizon.after(ticks, fn)`, `horizon.every(ticks, fn)`, `horizon.ticks()` | Timers: `fn` runs once after `ticks` server ticks, or every `ticks`, at the end of a tick while the world runs, as a `server_tick` handler of the mod's own (reads and actions work in it); `ticks()` the ticks seen since the scripts loaded. A timer that fails is logged and, if repeating, kept. Forgotten on `/reload` with the handlers |
| `horizon.raycast(player, distance)` | The first block within `distance` (64 by default, 1024 at most) along that player's look: `{id, x, y, z, block = {x, y, z}}` (the point hit and the block), or nil |
| Reload | `/reload` (any data load after the first) reads the mod's scripts again and runs `scripts/init.lua` afresh: handlers, goals and `require`d modules are forgotten, the Lua state and its globals stay. A script that fails to load leaves the old handlers running, with the error in the log |
| `random_tick(dimension, block, x, y, z, props, data)` | A block that ticks randomly does (a mod block as its template does; the parasite hive through `random_ticks`); `props` is the state's properties by name. Handlers may queue `horizon.set_block_property(key, value[, flags])` (flags 2, clients see it, by default), `horizon.schedule_tick(delay)` and `horizon.handled()`, which skips the block's own random tick `data` is the block entity's saved table when the block has one (`horizon.block_entity`), written back after the call |
| `tick_block(dimension, block, x, y, z, props, data)` | A scheduled tick of the block is due: the same actions and data |
| `right_click_block(x, y, z, block, face, player, held, data)` | A player right-clicks a block, before the block and the item act; `face` and `player.position` are `{x, y, z}`, `player` has `name`, `dimension`, `game_mode`, `held` has `item`, `count`, `plain`. Handlers may queue `horizon.set_block_property(key, value)`, `horizon.cancel()` (nothing acts, no swing) and the host actions; `data` as above |
| `horizon.block_entity(block, { ticks = true, data = {...} })` | Gives the mod's block a block entity, made with `data` (a table: numbers, strings, booleans, nested tables and sequences) when the block is placed and saved with the chunk; `ticks` makes it tick. Declared at load (`init.lua`), as goals are |
| `block_entity_tick(dimension, block, x, y, z, data)` | Every server tick of a block entity declared with `ticks`: `data` is its saved table, written back after the call (a number keeps its NBT type, a key set to nil is gone); actions: the host's (`summon`, `broadcast`...) and `set_block_property(key, value)` on the block |
| `mob_hurt(mob, damage_type, causing, direct, amount)` | A mob is about to take damage (`living_incoming_damage`): `mob` is `{id, kind}` only (the game is mid-hit: no host, so no view and no data); `damage_type` such as `minecraft:mob_attack`, `minecraft:on_fire`; `causing` and `direct` the entities behind it (-1: none). Handlers may queue `horizon.set_amount(n)` and `horizon.cancel()` (the hit is refused). The game is mid-hit: the host actions are dropped |
| `mob_killed(dimension, mob, killer, data)` | A player's melee blow killed a mob (other deaths are not told yet): `mob` is `{id, kind}` (it is dead), `killer` the player's table, `data` the mod's data on the mob |
| `mob_interact(dimension, mob, player, held, data)` | A player right-clicks a mob, before the mob and the item act: `mob` is its view (as in a goal, without `nearby` and `target_position`; a mob the host has no view of, dead or gone, is not told), `held` is the stack of the hand asked (main, then off); `horizon.cancel()` takes the click; the host actions; `data` written back |
| `mob_tick(dimension, mob, data)` | A mob ticked (`entity_tick_post`): `mob` is its view without the fields that need the world (`nearby` is empty, `target_position` nil), `data` the mod's data, written back; the host actions. Register it with a `kind`: a handler for every kind hears every mob of the level each tick |
| `item_use(item, player)`, `item_use_on(item, player, block, face)` | A player right-clicks with an item (`item` `{id, count, damage, plain}`), in the air or on a block the block let through (`block` `{x, y, z, id}`, `face` `{x, y, z}`, `{0, 1, 0}` the top): before the item's own use. `horizon.cancel()` takes the use (the hand swings, nothing else runs). Register by item id: `horizon.on("item_use", fn, "my_mod:wand")` |
| `item_hit(item, player, target)` | A player attacks `target` (`{id, kind, position}`) holding `item` (an empty hand is `minecraft:air`), before the blow; `horizon.cancel()` and it does nothing. Register by the held item's id |
| `item_finished(item, player)` | A player finished using an item over time: ate or drank it, or held a mod item for its `use_duration`. `horizon.set_result(id, count)` sets what stays in the hand (`set_result()` nothing) |
| `item_tick(item, player, slot, selected)` | A server tick of a stack a player carries (`slot` as the inventory saves it: 0–35, armour 100–103, off hand -106; `selected` for the held one). `horizon.set_count(n)` changes that stack (0 empties the slot). Every stack of every player, each tick: register by item id |
| `horizon.take_items(id, count)`, `horizon.give_items(id, count)` | Host actions on the acting player's inventory: take up to `count` of a plain item (the main hand first) or give where they fit |
| `mob_died(dimension, mob, cause, data)` | A mob died, by any cause (a blow, an arrow, fire, a fall, `/kill`): `mob` is `{id, kind}` (gone: no view), `cause` `{damage_type, causing, player}` (`causing` the mob behind it or 0, `player` the player credited with the kill, or nil), `data` the mod's data while the game still holds it. Register by kind. `mob_killed` stays for a player's melee blow |
| `player_joined(player)`, `player_left(player)`, `player_respawned(player)`, `player_died(player, cause)` | A player joined the game, left it by any path, respawned after a death, died (`cause` the death message's key, `death.attack.lava`, or nil) |
| `block_broken(x, y, z, block, player)` | A player breaks a block, once the game allowed it; `horizon.cancel()` keeps it (nothing breaks or drops). Register by block id |
| `block_placed(x, y, z, block, player, item)` | A player placed a block from `item`; it only hears (take the click in `item_use_on` to stop one). Register by block id |
| `item_picked_up(player, item, count)` | A player is about to pick up `count` of `item` (an id) from the ground; `horizon.cancel()` leaves them there. Register by item id |
| `player` (in the hooks) | `uuid`, `name`, `dimension`, `position` (`{x, y, z}`), `game_mode` (0 survival, 1 creative, 2 adventure, 3 spectator), `health`; a player the world has no record of at that moment (one dying) has `uuid` and the `name` a player hook told before |
| `data` (on a mob) | The mod's own saved table on the mob, empty at first; kept with the mob as the engine's attachment under the mod's id (`horizon:attachments` in its NBT), so it survives a save and a restart, and other mods' data stays apart. Written back after the call as a block entity's is (types kept, nil removes); nil when the host has no such mob |
| The `.hmod` | `cargo horizon-mod build` packs `scripts/` beside `assets/` and `data/`; `horizon_mods::Content` lists them (`scripts`), checks the paths and that `scripts/init.lua` is there; the Mods page says "May run its own scripts in your worlds" |

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

A Lua goal is a `NativeGoal` over a `LuaBehaviour` ([mob-ai-hooks.md](mob-ai-hooks.md)): the game copies the mob into
the view before each call and applies the actions after it, so a script never holds the mob. A goal function's error
is logged once at error level, then at debug level, and reads as "no".

Limits: a hook's handlers (250 ms of wall-clock time per handler registered, as one call) or a goal function that runs past 250 ms is stopped by Luau's interrupt with an
error that reads as any other error of its (the log, the dev tools' console, "no" for a goal), and `init.lua` with the
modules it requires has 5 s at load and reload; a state may take 64 MiB. Luau has no `io`, `loadfile` or `dofile`,
its `os` is `clock`, `date`, `difftime` and `time`, and `require` is the game's own over the mod's `scripts/`, so a
script reads nothing but its mod (`lua_mods::Budget`).

Run: install the `.hmod` and turn it on, or `horizon-client --dev-mod /path/to/mod`. A mod without capability `scripts`
is content only, as before. Scripted mods are gathered when the game starts, like trait mods: turning one on or off on
the Mods page takes effect at the next start.

## The plan

| Step | What | Why |
|---|---|---|
| 4 | Hook coverage by generation: `xtask mod-api` emits the Lua adapter for every `listen` hook from the catalog, as it emits the trait | Every hook point reaches Lua without hand-written glue |
| 5 | Host reads: `horizon.block(dimension, x, y, z)`, `horizon.mobs(dimension)`, `horizon.mob(dimension, id)`, returned synchronously (reads borrow the host for the call) | Scripts need to look before they act |

The block hooks needed one addition to the mod API: `LevelHost::state_block`
and `state_properties`, so a hook that gets a state can name its block without a world lookup (trait mods get them too). Everything
stays off unless a mod carries scripts, and the launcher needs no change: a scripted `.hmod` installs like any
content mod.
