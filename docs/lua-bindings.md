# Lua as a modding language

Status: a first runtime is built (`crates/horizon-client/src/lua_mods.rs`); the rest is the plan. Branch
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
| Entry | `scripts/init.lua` in a `--dev-mod` folder; the mod's id from its `horizon-mod.toml` |
| `horizon.on(name, fn)` | Register a handler: `server_tick(phase, world_running)`, `mob_spawned(dimension, kind, x, y, z, natural)`, `world_lifecycle(phase)` |
| `horizon.log(msg)` | The game log, tagged `[lua:<id>]` |
| `horizon.broadcast(text)` | A chat line to every player |
| `horizon.summon(dimension, kind, x, y, z)` | `/summon` |
| `horizon.kill_mob(dimension, id)` | `/kill` a mob |
| `require("name")` | `scripts/name.lua` of the same mod, once |

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

Run: `horizon-client --dev-mod /path/to/mod`. A folder without `scripts/init.lua` is content only, as before.

## The plan

| Step | What | Why |
|---|---|---|
| 1 | Installed mods: `horizon-mods` accepts a `scripts/` top-level entry and a capability `scripts` in the manifest (today any other entry refuses the file, [api/mod-files.md](api/mod-files.md)); the game reads the scripts out of the `.hmod`; Options > Mods says "runs scripts" | Ship one `.hmod` with data, assets and behaviour |
| 2 | Hook coverage by generation: `xtask mod-api` emits the Lua adapter for every `listen` hook from the catalog, as it emits the trait | Every hook point reaches Lua without hand-written glue |
| 3 | Host reads: `horizon.block(dimension, x, y, z)`, `horizon.mobs(dimension)`, `horizon.mob(dimension, id)`, returned synchronously (reads borrow the host for the call) | Scripts need to look before they act |
| 4 | Mob goals in Lua: a `NativeGoal` adapter ([mob-ai-hooks.md](mob-ai-hooks.md), step 3) whose `can_use`, `start`, `tick`, `stop` call Lua functions, attached in `mob_ai_register` | The parasite mod's own mob behaviour in a script |
| 5 | Block behaviour: `random_tick`, `block_use`, `block_entity_tick` to Lua with the block's position and state, `horizon.set_block_property` back | A staged block (the parasite hive) grows from a script |
| 6 | Reload: `/reload` and the resource-reload key re-run `init.lua` with the handlers cleared (the `hot-reload` branch's path) | Edit a script, press a key |
| 7 | Limits: an instruction budget per handler (`Lua::set_interrupt`), memory cap per state, no `io`/`os` | A script cannot hang or read the player's files; a content mod stays shareable |

Steps 1 and 6 give the content team the workflow; steps 4 and 5 give the parasite mod its behaviour. Everything
stays off unless a mod carries scripts, and the launcher needs no change: a scripted `.hmod` installs like any
content mod.
