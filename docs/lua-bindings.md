# Lua as a modding language

Status: the runtime (`crates/horizon-client/src/lua_mods.rs`), scripts inside the `.hmod` and mob goals in Lua
(`lua_mods/goals.rs`) are built; the rest is the plan. The branch builds on `mob-ai-hooks`. Branch
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

## What is built

| | |
|---|---|
| Entry | `scripts/init.lua` of a mod with capability `scripts` (and `experimental = ["scripts"]`), installed in `horizon-mods/` and on, or a `--dev-mod` folder; the mod's id from its `horizon-mod.toml` |
| `horizon.on(name, fn)` | Register a handler: `server_tick(phase, world_running)`, `mob_spawned(dimension, kind, x, y, z, natural)`, `world_lifecycle(phase)` |
| `horizon.log(msg)` | The game log, tagged `[lua:<id>]` |
| `horizon.broadcast(text)` | A chat line to every player |
| `horizon.summon(dimension, kind, x, y, z)` | `/summon` |
| `horizon.kill_mob(dimension, id)` | `/kill` a mob |
| `require("name")` | `scripts/name.lua` of the same mod, once |
| `horizon.goal(name, def)` | Define a mob goal: `def.flags` (`{"move", "look", "jump", "target"}`), `def.every_tick`, `def.interruptable`, and `can_use(mob, state, out)` (required), `can_continue`, `start`, `tick`, `stop`. `mob` is the mob's view, `state` a table of the goal's own that lives with the mob, `out` the actions |
| `mob_ai_register(mob)` | A hook: as a mob's goals are set up. Handlers call `horizon.add_goal(priority, name)`, `horizon.add_target_goal(priority, name)`, `horizon.clear_goals()`, `horizon.clear_target_goals()` |
| `mob` (the view) | `id`, `kind`, `position`, `velocity` (`{x, y, z}`), `yaw`, `head_pitch`, `health`, `max_health`, `age`, `on_ground`, `in_water`, `in_lava`, `hurt_time`, `last_hurt_by`, `target` (`"player"`, a mob id or nil), `target_position`, `target_in_melee_range`, `navigation_done`, `nearest_player`, `home` (`{x, y, z, radius}`) |
| `out` (the actions) | `navigate_to(x, y, z, speed)`, `navigate_to_target(speed)`, `stop_navigation()`, `move_toward(x, y, z, speed)`, `look_at(x, y, z)`, `look_at_target()`, `set_target(who)`, `jump()`, `attack(who)`, `swing()`; applied in order after the call |
| `horizon.distance(a, b)` | Between two `{x, y, z}` tables |
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

Run: install the `.hmod` and turn it on, or `horizon-client --dev-mod /path/to/mod`. A mod without capability `scripts`
is content only, as before. Scripted mods are gathered when the game starts, like trait mods: turning one on or off on
the Mods page takes effect at the next start.

## The plan

| Step | What | Why |
|---|---|---|
| 2 | Hook coverage by generation: `xtask mod-api` emits the Lua adapter for every `listen` hook from the catalog, as it emits the trait | Every hook point reaches Lua without hand-written glue |
| 3 | Host reads: `horizon.block(dimension, x, y, z)`, `horizon.mobs(dimension)`, `horizon.mob(dimension, id)`, returned synchronously (reads borrow the host for the call) | Scripts need to look before they act |
| 5 | Block behaviour: `random_tick`, `block_use`, `block_entity_tick` to Lua with the block's position and state, `horizon.set_block_property` back | A staged block (the parasite hive) grows from a script |
| 6 | Reload: `/reload` and the resource-reload key re-run `init.lua` with the handlers cleared (the `hot-reload` branch's path) | Edit a script, press a key |
| 7 | Limits: an instruction budget per handler (`Lua::set_interrupt`), memory cap per state, no `io`/`os` | A script cannot hang or read the player's files; a content mod stays shareable |

Step 6 completes the content team's workflow; step 5 gives the parasite hive its behaviour. Everything
stays off unless a mod carries scripts, and the launcher needs no change: a scripted `.hmod` installs like any
content mod.
