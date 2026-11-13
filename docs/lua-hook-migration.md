# Migrating a Lua mod to the generated hooks

Every handler now takes one event table, and Lua hears the engine's native hooks under the native names. This page
maps each old hand-written hook name to its successor. Rule of thumb: **a name that kept its name changed shape
only** — the handler still hears the same event, as one event table instead of positional arguments. A name that
is gone moved to the native hook in the second column.

The generated `HorizonOn` overloads in [horizon.d.lua](api/horizon.d.lua) are authoritative for every shape below;
where this page and that file disagree, that file wins. Field types follow the shapes in
[lua-bindings.md](lua-bindings.md#hooks): an optional value reads as the value or nil, a position as `{x, y, z}`,
a uuid as lowercase text, rich text as a string plus a `<field>_component` table, a stack as
`{id, count, damage, extra, plain}`, a block state as its id string plus `<field>_props`.

## How to migrate a mod

Four passes: the mod loads, what it declares is accepted, it runs in a world, and its log is quiet. Each pass has one
command that says where you are.

1. **Update the engine and the modkit.** The generated hooks are on the engine's `engine-work` branch, and the modkit
   that speaks them is on `main`. The checkouts sit side by side, as [dev-setup.md](dev-setup.md) lays them out.

   ```sh
   cd ~/src/horizon && git switch engine-work && git pull --ff-only
   cargo build --profile fast -p event-horizon -p horizon-client-lib
   cargo build -p cargo-horizon-mod
   cd ~/src/horizon-modkit && git pull --ff-only
   ```

2. **Move the entry points.** A mod used to have one Lua state and `scripts/init.lua`. It has one state per side now:
   `scripts/server/init.lua` on the server, `scripts/client/init.lua` in a player's game, and `scripts/shared/` for
   the modules either side may `require`. A mod with only `scripts/init.lua` is not loaded
   ([lua-bindings.md](lua-bindings.md#getting-started)). A hook the client fires — `hud`, `gui_layer_post`, `command` —
   belongs in `scripts/client/`. `scripts/registry.lua` and `scripts/registry/` keep their paths.

3. **Load it without a world.** `check` runs the game's own headless loader on both sides, then StyLua, selene and
   luau-lsp, and names the file and line of every failure
   ([lua-bindings.md](lua-bindings.md#checking-a-mod-without-a-world)):

   ```sh
   cd ~/src/horizon
   target/debug/cargo-horizon-mod horizon-mod check ~/src/horizon-parasite-mod \
     --game target/fast/horizon-client \
     --dev-mod ~/src/horizon-modkit \
     --resources ~/src/horizon-resources/26.3
   ```

   `cargo horizon-mod check` is the same command when the binary is on your `PATH`. If luau-lsp cannot find the
   modkit's types, add `--modkit ~/src/horizon-modkit` (the mod's own `.luaurc` is never edited for it). Fix what it
   names, in the order it names it. A load failure is one of:

   - `horizon.on("mob_tick"): no such hook; the hooks are ...` — an old hook name. Take the new one from the table
     below, or from the `HorizonOn` overloads in [horizon.d.lua](api/horizon.d.lua).
   - `hook 'command' runs on the client: register it from scripts/client/` — the hook moved to the other side.
   - `horizon.on("destroy_block"): this hook takes no kind` — the hook no longer filters by kind. Drop the third
     argument and check `event.state` (or `event.stack.id`) in Lua.
   - `horizon.on("block_entity"): no such hook` — `block_entity` and `block_entity_ticks` are declaration-served:
     declare them with `horizon.block_entity(block, { ticks = true, data = {...} })` at load, and hear
     `block_entity_tick`.
   - a goal or a command that fails when it is declared.

   The type step reports separately: an `event.<field>` that luau-lsp says is not on the type is not on that hook's
   event, and the table below names the field that replaces it.

4. **Run it in a world and read the log.** Errors that need a world do not show up in `check`: a handler still written
   for the old arguments gets nil for its second parameter and fails at its first use of it. Start a scratch game and
   drive the mod through the same socket ([horizon-ctl-commands.md](horizon-ctl-commands.md)):

   ```sh
   cd ~/src/horizon
   GAME_DIR=$(mktemp -d)
   CTL="target/fast/horizon-ctl --socket $PWD/migrate.sock"
   $CTL launch --game target/fast/horizon-client --wait --offscreen \
     --game-dir "$GAME_DIR" --resources ~/src/horizon-resources/26.3 \
     --world Migrate --dev-mod ~/src/horizon-modkit --dev-mod ~/src/horizon-parasite-mod
   $CTL wait-ready 180
   ```

   Play the mod's own scenarios with `command`, `summon`, `give`, `chat`, `attack`, `interact` and `break`, then read
   the errors:

   ```sh
   $CTL logs --lines 200
   ```

   Each error is a line tagged `[lua:<id>]`, with the hook, the message naming the script and its line, and the
   traceback under it ([lua-bindings.md](lua-bindings.md#limits-and-errors)). Fix the script, then run it again
   without a restart:

   ```sh
   $CTL reload-mods            # every scripted mod, both sides; `reload-mods <mod id>` for one
   ```

   A change to `scripts/registry.lua` or to a declaration needs a restart: `$CTL quit`, then launch again.

Repeat passes 3 and 4 until both are clean. With `--dev-tools`, the in-game `/lua` console lists each scripted mod's
handlers, goals and last error, and its reload button does what `reload-mods` does
([lua-bindings.md](lua-bindings.md#working-on-a-script)).

When both are clean, pin the hook surface the mod was built for: add `lua_hook_surface = 1` to its
`horizon-mod.toml`. A later engine that changes the hook surface bumps its number, and the mod then fails at load with
`mod <id> was built for Lua hook surface <N>, this engine has <M>` instead of on a missing hook name. The line is
optional; a mod without it loads as before ([lua-bindings.md](lua-bindings.md#getting-started)).

## Per-hook rows

Old name | New hook | New event shape | Old positional arguments become | Fields lost | Fields gained | How to filter
|---|---|---|---|---|---|---|
| `server_tick` | `server_tick` | `{authority: string, epoch: number, tick: number, phase: string, world_running: boolean}` | `(phase, world_running)` become `event.phase` (`"start"`/`"end"`) and `event.world_running` | none | the context (`authority`, `epoch`, `tick`) | bare; branch on `event.phase` in Lua |
| `world_lifecycle` | `world_lifecycle` | `{authority: string, epoch: number, tick: number, phase: string, error_text: string}` | `(phase)` becomes `event.phase` | none | `error_text`, and the context | bare |
| `mob_ai_register` | `mob_ai_register` | `{mob: MobView}` | `(mob)` becomes `event.mob`, the same mob view | none | none (`register_goals` hears the same setup earlier; handlers on both append) | kind = mob kind |
| `random_tick` | `random_tick` | `{dimension: string, pos: Vec3, state: string, state_props: Props, data: Data?}` | `(dimension, block, x, y, z, props, data?)` become `event.dimension`, `event.state` (the id `block` named) with `event.state_props`, `event.pos`, and `event.data` | `x`/`y`/`z` spread (same data as `pos`) | none | kind = block id |
| `tick_block` | `tick_block` | `{dimension: string, pos: Vec3, state: string, state_props: Props, data: Data?}` | as `random_tick` | as `random_tick` | none | kind = block id |
| `block_entity_tick` | `block_entity_tick` | `{pos: Vec3, entity: {id: string, data: Data}, dimension: string}` | `(dimension, block, x, y, z, data)` become `event.dimension`, `event.entity.id` (the `block` id), `event.pos`, and `event.entity.data` (edited in place, written back) | `x`/`y`/`z` spread (same data as `pos`) | none | kind = block id |
| `loot_drops` | `loot_drops` | `{table: string, dimension: string, origin: Vec3, block: { x: number, y: number, z: number, id: string, state: string }?, entity: { id: number, kind: string }?, player: PlayerRef?, tool: Item?, explosion: number?, used: boolean, drops: {Item}}` | `(source, drops)` flattens: `source`'s fields move to the top level, `drops` stays `event.drops` (edited in place, written back) | the nested `source` table; the `mob` alias (the field is `entity`) | `used` | kind = block id or mob kind |
| `command` | `command`, `perform_command` | `command`: `{command: string}`; `perform_command`: the declaring call's `fn(player, args)` with `args.raw` | the old server-side `horizon.on("command")` (heard a mod command being performed) splits: typed slash lines go to `command`, mod commands go to `horizon.command` + `perform_command` | cancelling a mod command from a bare `on("command")` handler (register the command words with `horizon.command` instead) | `command` hears every typed line client-side, before the server sees it | `command` takes no kind; `perform_command` filters by command words |
| `hud` | `hud` | `{batch: HudBatch, corners: HudCorners}` | `(event)` stays one event table with the same `batch` and `corners` | none | none | takes no kind |
| `gui_layer_post` | `gui_layer_post` | `{layer: string, batch: HudBatch}` | `(event)` stays one event table with the same `layer` and `batch` | none | none | takes no kind |
| `mob_spawned` | `mob_spawned` | `{dimension: string, kind: string, pos: Vec3, natural: boolean}` | `(dimension, kind, x, y, z, natural)` become the same fields, with `x`/`y`/`z` as `event.pos` | `x`/`y`/`z` spread (same data as `pos`) | none | kind = mob kind, as before |
| `mob_died` | `mob_died` | `{dimension: string, entity: number, kind: string, damage_type: string, causing: number, player: string?}` | `(dimension, mob, cause, data?)` become `event.dimension`, `event.entity` + `event.kind`, the flat `damage_type`/`causing`/`player` fields | `mob` table (now `entity` + `kind`); the `cause` table (now flat fields); the killer `player` table (now uuid text or nil) | `horizon.entity_data(event.entity)` for per-mob saved state | kind = mob kind, as before |
| `mob_killed` | `mob_killed` | `{player: string, entity: number, kind: string}` | `(dimension, mob, killer, data?)` become `event.entity` + `event.kind` and `event.player` (the killer's uuid text); the dimension comes from the host reads | the positional `dimension`; the `mob` table; the `killer` player table | `horizon.entity_data(event.entity)` for per-mob saved state | kind = mob kind, as before |
| `mob_hurt` | `living_incoming_damage` | `{entity: number, kind: string, damage_type: string, causing: number, direct: number, amount: number}` | `(mob, damage_type, causing, direct, amount)` become `event.entity` + `event.kind` and the same scalar fields | the `mob` table (now `entity` + `kind`); `horizon.set_amount` (assign `event.amount`) | none | kind = mob kind; bare hears every kind |
| `player_hurt` | `living_incoming_damage` | same as `mob_hurt` | `(player, ...)` become `event.entity` + `event.kind`, with kind `minecraft:player` | the `player` table (no name; the hook has no host) | none | kind `minecraft:player`, or filter `event.kind` in Lua |
| `mob_fall` | `living_fall` | `{entity: number, kind: string, distance: number, multiplier: number}` | `(mob, distance, multiplier)` become `event.entity` + `event.kind` and the same scalar fields | the `mob` table; `horizon.set_distance`/`set_multiplier` (assign the fields) | none | kind = mob kind, as before |
| `player_fall` | `living_fall` | same as `mob_fall` | `(player, ...)` become `event.entity` + `event.kind`, with kind `minecraft:player` | the `player` table | none | kind `minecraft:player` |
| `mob_heal` | `living_heal` | `{entity: number, amount: number}` | `(mob, amount)` become `event.entity` and `event.amount` | the `mob` table (now the entity id); `horizon.set_amount` (assign `event.amount`) | none | none: every handler hears every entity, as before |
| `mob_knockback` | `living_knockback` | `{entity: number, kind: string, damage_type: string, causing: number, direct: number, strength: number, x: number, z: number}` | `(mob, damage_type, causing, direct, strength, x, z)` become `event.entity` + `event.kind` and the same scalar fields | the `mob` table; `horizon.set_strength` (assign `event.strength`) | mutable `x`/`z` (steer the knockback) | kind = mob kind, as before |
| `player_knockback` | `living_knockback` | same as `mob_knockback` | `(player, ...)` become `event.entity` + `event.kind`, with kind `minecraft:player` | the `player` table | mutable `x`/`z` | kind `minecraft:player` |
| `mob_equipped` | `living_equipment_change` | `{entity: number, kind: string, slot: string, from: Item?, to: Item?}` | `(mob, slot, from, to)` become `event.entity` + `event.kind`, `event.slot`, and full item tables | the `mob` view (read it with `horizon.mob`); `{item, count, plain}` stacks (now full `Item` tables with `id`, `damage`, `extra`) | `damage` and `extra` on the stacks | kind = mob kind, as before |
| `entity_joined` | `entity_join_level` | `{dimension: string, entity: number, kind: string, pos: Vec3, loaded_from_disk: boolean}` | `(dimension, id, kind, x, y, z, loaded)` become the same fields, with `x`/`y`/`z` as `event.pos` | `x`/`y`/`z` spread (same data as `pos`); `loaded` is now `loaded_from_disk` | none | kind, as before |
| `mob_tick` | `entity_tick_post` | `{dimension: string, entity: number, kind: string}` | `(dimension, mob, data?)` become `event.dimension`, `event.entity` + `event.kind` | the `mob` view (read it with `horizon.mob(id)` when needed) | `horizon.entity_data(event.entity)` for per-mob saved state | kind = mob kind, as before |
| `mob_interact` | `interact_entity` | `{entity: number, kind: string, stack: Item?}` | `(dimension, mob, player, held?, data?)` become `event.entity` + `event.kind` and `event.stack` (the hand's stack); the dimension and the acting player come from the host reads | the positional `dimension`; the `player` table (now `horizon.acting_player()`); the per-hand `held` (now the single `stack`); the mob view | `horizon.entity_data(event.entity)` for per-mob saved state | kind = mob kind, as before |
| `entity_interact` | `interact_entity` | same as `mob_interact` | `(dimension, entity, player, held?)` become the same fields as `mob_interact` | the positional `dimension`; the `player` table (now `horizon.acting_player()`); `held` (now `stack`) | `horizon.entity_data(event.entity)` for per-entity saved state | kind = entity kind; one hook now, so registration order replaces the old before-ordering |
| `right_click_block` | `right_click_block` | `{pos: Vec3, face: Vec3}` | `(x, y, z, block, face, player, held?, data?)` become `event.pos` and `event.face` | the `block` id (read it with `horizon.block`); the `player` table (now `horizon.acting_player()`) and the per-hand `held` stack (the off-hand stack is not on the event: `held` is the selected hotbar stack on both passes); block-entity `data` and its writeback; `set_block_property`. The double fire stays: it still fires once per hand | none | bare; keep the block ids it cares about in Lua |
| `item_use` | `use_item` | `{stack: Item}` | `(item, player)` become `event.stack`; the acting player is `horizon.acting_player()` | the by-item-id registration; the `player` table | none | bare; check `event.stack.id` in Lua |
| `item_use_on` | `use_item_on` | `{stack: Item, pos: Vec3, face: Vec3}` | `(item, player, block, face)` become `event.stack`, `event.pos`, `event.face`; the block's id comes from the `horizon.block` read | the by-item-id registration; the `player` table (now `horizon.acting_player()`); the `block` id position | none | bare; check `event.stack.id` in Lua |
| `item_hit` | `attack_entity` | `{entity: number, kind: string, pos: Vec3}` | `(item, player, target)` become `event.entity` + `event.kind` and `event.pos` | the held `stack` (now `horizon.acting_player().held`) and the by-held-item registration (air included); the `player` table (now `horizon.acting_player()`) | the entity-kind filter | kind = foe kind; read the hand at `horizon.acting_player().held` |
| `item_finished` | `living_entity_use_item_finish` | `{stack: Item, entity: number?, hand: "main" \| "off", duration: number, result: Item?}` | `(item, player)` become `event.stack`, with `event.entity` nil for the player's own use | `horizon.set_result(id, count)` (assign `event.result`, a table or nil); the `player` table (now `horizon.acting_player()`); the by-item-id registration; the player-only gating | `entity`, `hand`, `duration`, and the `result` writeback | bare; the player's own use reaches bare handlers (`event.entity == nil`) |
| `item_use_start` | `living_entity_use_item_start` | `{stack: Item, entity: number?, hand: "main" \| "off", duration: number}` | `(item, player, hand, duration)` become `event.stack`, `event.hand`, `event.duration`, with `event.entity` nil for the player's own use | `horizon.set_duration` (assign `event.duration`); the `player` table (now `horizon.acting_player()`); the by-item-id registration; the player-only gating | none | bare; keep the player's own subset with `event.entity == nil` |
| `item_tick` | `inventory_tick` | `{slot: number, selected: boolean}` | `(item, player, slot, selected)` become `event.slot` and `event.selected` | the `item` argument entirely; `horizon.set_count` (no replacement: the hook has no stack); the by-item-id registration; the `player` table (the ticking player is `horizon.acting_player()`) | none | bare only: every stack of every player ticks, so keep it light |
| `item_hurt_enemy` | `hurt_enemy` | `{stack: Item, entity: number, kind: string}` | `(item, player, target)` become `event.stack` and `event.entity` + `event.kind` | the by-held-item registration; the `player` table (now `horizon.acting_player()`) | the entity-kind filter and the full stack table | kind = foe kind, plus an in-Lua `stack.id` check |
| `item_crafted` | `item_crafted` | `{player: string, recipe: string, stack: Item}` | `(player, recipe, item)` become `event.player` (uuid text), `event.recipe`, `event.stack` | the `player` table (now `horizon.acting_player()`, or `horizon.player(event.player)`); the by-result-item registration | none | bare; check the recipe in Lua |
| `item_picked_up` | `item_picked_up` | `{player: string, item: string, count: number}` | `(player, item, count)` become `event.player` (uuid text) and the same `item`/`count` | the `player` table (now `horizon.acting_player()`); the by-item-id registration | none | bare; check the item in Lua (`horizon.cancel()` leaves the stack there) |
| `item_dropped` | `item_dropped` | `{player: string, item: string, count: number}` | as `item_picked_up` | as `item_picked_up` | none | bare; check the item in Lua (it only hears) |
| `block_broken` | `destroy_block` | `{pos: Vec3, state: string, state_props: Props}` | `(x, y, z, block, player)` become `event.pos` and `event.state` (the id `block` named) with `event.state_props`; the acting player is `horizon.acting_player()` | the by-block-id registration; the `player` table | `state_props` | bare; check `event.state` in Lua (`horizon.cancel()` keeps the block) |
| `block_placed` | `place_block` | `{stack: Item, pos: Vec3, state: string, state_props: Props}` | `(x, y, z, block, player, item)` become `event.pos`, `event.state` with `event.state_props`, and `event.stack` | the by-block-id registration; the `player` table (the placing player is `horizon.acting_player()`; `item` survives as `stack`) | `state_props` | bare; to stop a placement, take the click in `use_item_on` |
| `projectile_impact` | `projectile_impact` | `{dimension: string, projectile: number, kind: string, owner: number, entity: number?, entity_kind: string, block: Vec3, face: Vec3, at: Vec3}` | `(projectile, kind, owner, hit)` splits: the `hit` table's entity or block half becomes the flat `entity`/`entity_kind` or `block`/`face` fields, plus `event.at` | the `hit` table shape (branch on `event.entity`: on an entity hit `block` and `face` are still `{0, 0, 0}`, on a block hit `entity` is nil and `entity_kind` is `""`); the block id (read it with `horizon.block`) | `dimension` | kind = projectile kind, as before |
| `projectile_launched` | `projectile_launch` | `{player: PlayerRef?, entity: number, kind: string}` | `(projectile, kind, player)` become `event.entity`, `event.kind`, and `event.player` (the shooter's table, or nil) | the full `player` table (now a `PlayerRef` or nil) | none | kind = projectile kind, as before |
| `explosion` | `explosion_detonate` | `{dimension: string, center: Vec3, source: {entity: number, kind: string}?, blocks: {Vec3}}` | `(dimension, center, blocks)` stay the same, with `blocks` edited in place and written back | none | none | bare |
| `player_joined` | `player_logged_in` | `{player: PlayerRef}` | `(player)` becomes `event.player`, the full table while they are on | none | none | bare |
| `player_left` | `player_logged_out` | `{player: PlayerRef}` | as `player_joined` | none | none | bare |
| `player_respawned` | `player_respawned` | `{player: PlayerRef}` | `(player)` becomes `event.player` | none | none | bare |
| `player_died` | `player_died` | `{player: string, cause: string?}` | `(player, cause)` become `event.player` (uuid text) and `event.cause` (the death message's key, or nil) | the `player` table (no health, no position; the name is unrecoverable) | none | bare |
| `player_tick` | `player_tick` | `{}` (empty) | `(player)` goes away entirely: the event carries nothing | the whole `player` argument, including `held` (now `horizon.acting_player()`, with the hand in its `held`) | `horizon.acting_player()` for the ticking player | takes no kind; `horizon.players()` is the whole list, and the host actions and reads work |
| `player_chat` | `player_chat` | `{player: string, name: string, message: string, replies: {{text: string, component: Component?}}}` | `(player, line)` become `event.player` (uuid text), `event.name`, `event.message` | `horizon.reply()` (assign `event.replies`); the other queued host actions (the hook has no host) | `replies` reads back in Lua | bare |
| `player_command` | `player_command` | `{player: PlayerRef, command: string, replies: {{text: string, component: Component?}}}` | `(player, line)` become `event.player`, `event.command` (no slash) | `horizon.reply()` (assign `event.replies`); no player's state is in place here, so `tell` and the item functions reach nobody | `replies` reads back in Lua | bare |
| `player_changed_dimension` | `player_changed_dimension` | `{player: PlayerRef, from: string, to: string}` | `(player, from, to)` become the same fields | none | none | bare |
| `player_bed_enter` | `player_bed_enter` | `{player: PlayerRef, pos: Vec3}` | `(player, x, y, z)` become `event.player` and `event.pos` (always present) | `x`/`y`/`z` spread (same data as `pos`) | none | bare |
| `player_exp_change` | `player_exp_change` | `{player: PlayerRef, amount: number}` | `(player, amount)` become `event.player` and `event.amount` | `horizon.set_amount()` (assign `event.amount`); the old negative clamp (the writeback does not clamp) | none | bare |
| `player_container_open` | `player_container_open` | `{player: PlayerRef, dimension: string, menu: string, pos: Vec3?}` | `(player, menu, pos?)` become the same fields | none | `dimension` | bare |

## Cross-cutting changes

### Every handler takes one event table

There are no positional `horizon.on` handler arguments anymore, with one exception: `perform_command` keeps its declaring
call's `fn(player, args)` handler. Every other `horizon.on` handler is `function(event)`, and the old argument positions map
to the event fields in the table above. A parameter the handler names past `event` reads nil (past `args` for
`perform_command`). (Goal functions keep their own shape, `function(mob, state, out)`.)

The third column of each row is the new event shape, and the fourth maps the old arguments onto it. Three rows,
worked:

**`mob_spawned`: a position replaces three numbers.** The old handler took `(dimension, kind, x, y, z, natural)`:

```lua
-- old
horizon.on("mob_spawned", function(dimension, kind, x, y, z, natural)
  if kind == "minecraft:zombie" and natural then
    horizon.log(("zombie at %.0f %.0f %.0f"):format(x, y, z))
  end
end, "minecraft:zombie")

-- new: one event table, the same values by name
horizon.on("mob_spawned", function(event)
  if event.kind == "minecraft:zombie" and event.natural then
    horizon.log(("zombie at %.0f %.0f %.0f"):format(event.pos.x, event.pos.y, event.pos.z))
  end
end, "minecraft:zombie")
```

**`block_broken` → `destroy_block`: a bare handler and a read replace the by-block-id registration and the player.**
The old handler took `(x, y, z, block, player)` and was registered by block id:

```lua
-- old
horizon.on("block_broken", function(x, y, z, block, player)
  if block == "minecraft:diamond_ore" then
    horizon.broadcast(player.name .. " found diamonds at " .. x .. " " .. y .. " " .. z)
  end
end, "minecraft:diamond_ore")

-- new: bare; the block's id is event.state, the player is a read
horizon.on("destroy_block", function(event)
  if event.state == "minecraft:diamond_ore" then
    local player = horizon.acting_player()
    horizon.broadcast(player.name .. " found diamonds at " .. event.pos.x .. " " .. event.pos.y .. " " .. event.pos.z)
  end
end)
```

**`item_hit` → `attack_entity`: the hand is a read, not an argument.** The old handler took `(item, player, target)` and
was registered by the held item's id:

```lua
-- old
horizon.on("item_hit", function(item, player, target)
  if target.kind == "minecraft:zombie" and item.id == "minecraft:iron_sword" then
    horizon.cancel()
  end
end, "minecraft:iron_sword")
```

```lua
-- new: register by the foe's kind and check the hand in Lua
horizon.on("attack_entity", function(event)
  local held = horizon.acting_player().held
  if event.kind == "minecraft:zombie" and held and held.id == "minecraft:iron_sword" then
    horizon.cancel()
  end
end, "minecraft:zombie")
```

### Bare uuid fields are uuid text

Where the native hook carries a bare uuid, the event field reads as its lowercase text form, never a number.
The rows above whose fields changed this way: `mob_killed.player`, `mob_died.player`, `item_crafted.player`,
`item_picked_up.player`, `item_dropped.player`, `player_died.player`, `player_chat.player`. The same rule changed
these fields elsewhere in the hook surface: `creeper_ignite.player`, `experience_orb_pickup.player`,
`baby_entity_spawn.player`, `client_chat_received.sender`, `item_dropped_v2.player`, `item_picked_up_v2.player`,
`player_hurt_by_player.attacker`, `player_hurt_by_player.victim`, `player_move.player`, `player_teleport.player`,
`village_siege.player`, `menu_input_closed.uuid`, `menu_links.player`, `player_pre_login.player`,
`recipe_sources.player`, `custom_payload_from_client.player`, `fishing_hook_landed.player`,
`mending_repair.player`, `menu_clicked.uuid`, `menu_closed.uuid`, `net_from_client.player`,
`player_start_tracking.uuid`, `player_stop_tracking.uuid`, `resource_pack_status_v2.pack`,
`menu_extra_data.player`, `permission_check.player`.

### Removed helpers raise an error naming the replacement field

These calls used to queue a change; now each fails at the call with the event field that replaces it, so the
handler fails through the hook's error path instead of silently doing nothing:

- `horizon.set_count` ("`inventory_tick` has no stack"): no replacement field — see below
- `horizon.set_result` → assign `event.result` (a table or nil) in `living_entity_use_item_finish`
- `horizon.set_duration` → assign `event.duration` in `living_entity_use_item_start`
- `horizon.set_distance` → assign `event.distance` in `living_fall`
- `horizon.set_multiplier` → assign `event.multiplier` in `living_fall`
- `horizon.set_strength` → assign `event.strength` in `living_knockback`
- `horizon.set_amount` ("horizon.set_amount does nothing: set event.amount = n in item_damaged, living_damage_pre, living_heal, living_incoming_damage, living_incoming_damage_v2, living_incoming_damage_v3, player_exp_change, player_hurt_by_player or player_xp_change"): assign `event.amount` in one of those hooks instead
- `horizon.reply` → assign `event.replies` in `player_chat` or `player_command`

### Capabilities that were lost on purpose

The generated events carry less than the hand-written hooks did, on purpose: a value an event does not need is not
copied on every firing. Every loss below except `set_count` has a Lua replacement; the rows above name the same
replacements per hook.

| Lost | Replacement |
|---|---|
| per-mob saved `data` in the mob events (`mob_died`, `mob_killed`, `mob_tick`, `mob_interact`) | `horizon.entity_data(event.entity)`: read and write it keyed by the entity id, with the same writeback rules as `horizon.world_data()` |
| the event `player` table in `player_chat`, `item_crafted`, `item_picked_up`, `item_dropped`, `mob_killed` and `mob_died` | the uuid text. `horizon.acting_player()` in a hook fired for that player (a craft, pickup or drop); `horizon.player(event.player)` where the hook has a host. `player_died` has a host, so that read works while they are still on, and the name is unrecoverable once they are gone. `player_chat` has no host and no acting player |
| the held item in `player_tick`, `attack_entity` and `inventory_tick` | `horizon.acting_player().held`, a full `Item` table, or `horizon.player(name).held`. In `inventory_tick` that is the ticking player's selected stack, not the stack in the ticking slot. Only a `perform_command` handler's `player` table keeps the old `{item, count, plain}` hand shape |
| the ticking player in `player_tick` | `horizon.acting_player()` returns the player whose tick fired; `horizon.players()` is the whole list when no player acts |
| `set_count` from `inventory_tick` | none: the hook names no stack |
| the by-item-id and by-block-id registration (`item_use`, `item_use_on`, `item_hit`, `item_hurt_enemy`, `item_crafted`, `item_picked_up`, `item_dropped`, `block_broken`, `block_placed`) | register bare and check `event.stack.id`, `event.item` or `event.state` in Lua, or register by kind where the hook takes one. Every firing wakes the handler, so keep it short |
| the block entity's `data` in `right_click_block`, and `set_block_property` there | declare the block entity with `horizon.block_entity(block, { ticks = true, data = {...} })`, read its table in `block_entity_tick`, and replace a click's block with `horizon.set_block`. `set_block_property` runs in `random_tick`, `tick_block` and `block_entity_tick` only |

### Registrations that now mean something else

- `horizon.on("command")` is the client hook: it fires for every slash line the player types, before the line
  reaches the server. The old server-side mod-command hook is `perform_command`, declared with
  `horizon.command(usage, level, fn)`; the declaring call's `fn(player, args)` shape is unchanged.
- `register_goals` and `mob_ai_register` both fire for server mobs (handlers on both append, so registering the
  same goal in both attaches it twice). `register_goals` also fires for client-side copies; goals added there on
  a `"goals": "none"` mob are gone by `mob_ai_register`.
- `hears`/`listened` flags (`hears_projectile_impacts`, `entity_tick_listened`, `living_equipment_listened`) are
  not script hooks: the game answers them from the sibling hooks' registries, and registering for them fails the
  load. They exist so an unheard hook costs nothing.
- `block_entity` and `block_entity_ticks` are declaration-served, not registerable: declare them with
  `horizon.block_entity(block, { ticks = true, data = {...} })` instead of `horizon.on`. Registering for them fails the
  load with "no such hook".

## Things the old API had that moved or changed

What the parasite mod is known to use, item by item: where it is today, how it was called, how it is called now, and a
before/after. Five of the six changed; `self:every` did not.

### `self:every`

**Where it is today**: the modkit's goal-state counter, `State:every(name, ticks)` in
`horizon-modkit/scripts/server/modkit/goal.lua:21` (the sibling checkout that [dev-setup.md](dev-setup.md) puts beside
this one), documented in the modkit's `docs/api.md` ("`self:every(name, ticks)`") and used in its
`docs/guides/mobs.md`. It is not an engine helper: the engine hands a goal a plain `state` table, passed to
`can_use(mob, state, out)` (`crates/event-horizon-lua/src/lua_mods/goals.rs:4-5`; `GoalState` at
[horizon.d.lua:20](api/horizon.d.lua)).

**Old usage**: `self:every("path", 20)` inside a goal's `tick`, where `self` is that goal's state for that mob.
**New usage**: the same call, from the same place. The goal functions keep their shape, `function(mob, state, out)`
in the engine and `function(self, mob)` in the modkit; only the hooks around them changed.

```lua
-- old and new: the goal body does not change
local Approach = modkit.Goal({
  flags = { "move", "look" },
  can_use = function(self, mob) return mob:player_within(8) end,
  tick = function(self, mob)
    local p = mob.nearest_player
    if p and (mob.navigation_done or self:every("path", 20)) then
      mob:navigate_to(p.x, p.y, p.z, 1.0)
    end
  end,
})
```

Only a goal's state table has `:every` and `:cooldown` in the modkit. A counter that sat on an entity, item or player
class was the mod's own: keep it, or count with `horizon.ticks()`, or use a `horizon.every` timer.

### `horizon.feedback`

**Where it is today**: declared as `feedback: (text: string) -> ()` ([horizon.d.lua:996](api/horizon.d.lua)), installed
as a queued action at `crates/event-horizon-lua/src/lua_mods.rs:2218`, and answered only by the command host
(`apply_command_action`, `lua_mods.rs:3223`), which the `perform_command` hook calls
(`crates/event-horizon-lua/src/generated_runtime.rs:1923`). Described at
[lua-bindings.md:373](lua-bindings.md#commands).

**Old usage**: inside a server-side `horizon.on("command", function(player, args) ... end)` handler, or the declaring
`horizon.command(usage, level, fn)` call, which heard a mod command being performed.
**New usage**: the same call, inside `horizon.command(usage, level, fn(player, args))` or a
`horizon.on("perform_command", fn, words)` handler. The function itself is unchanged. From any other hook it is queued
and dropped with `unknown action feedback` in the log (`lua_mods.rs:3213`), and in a client state the name raises the
server-only error ([lua-bindings.md:226](lua-bindings.md#the-horizon-table)).

```lua
-- old: scripts/init.lua; the declaring call carried the answer
horizon.command("parasite count [kind:word]", 0, function(player, args)
  horizon.feedback(("%d parasites"):format(count_parasites(args.kind)))
end)

-- new: scripts/server/init.lua; the hook is perform_command, and its handler is the
-- one hook that keeps positional arguments, fn(player, args)
horizon.command("parasite count [kind:word]", 0, function(player, args)
  horizon.feedback(("%d parasites"):format(count_parasites(args.kind)))
end)
horizon.on("perform_command", function(player, args)
  horizon.log("a mod command ran: " .. args.raw)
end)
```

### `horizon.ticks`

**Where it is today**: `crates/event-horizon-lua/src/lua_mods.rs:385`, in the timer code the runtime installs
(`lua_mods.rs:356`); declared as `ticks: () -> number` ([horizon.d.lua:987](api/horizon.d.lua)) and described at
[lua-bindings.md:237](lua-bindings.md#the-mod).

**Old usage**: the number of server ticks seen since the scripts loaded, because a mod had one state and it was the
server's. `horizon.after` and `horizon.every` counted the same ticks.
**New usage**: the same calls, but each state counts its own side: server ticks in `scripts/server/init.lua`, client
ticks in `scripts/client/init.lua` ([lua-bindings.md:387-390](lua-bindings.md#timers-and-saved-data)). A server script
needs no change. A module in `scripts/shared/` runs once in each state, so a timer there now runs twice in single
player, once per side, each copy counting its own side's ticks.

```lua
-- old: scripts/init.lua, one state, server ticks
horizon.every(20, function() horizon.log(("tick %d"):format(horizon.ticks())) end)

-- new: scripts/server/init.lua, the same call and the same ticks
horizon.every(20, function() horizon.log(("tick %d"):format(horizon.ticks())) end)

-- new: the same code in scripts/shared/ runs in both states, so choose one
if horizon.side == "server" then
  horizon.every(20, function() horizon.log(("tick %d"):format(horizon.ticks())) end)
end
```

### Goals: the five actions, `register_goals`, `mob_ai_register`

**Where it is today**: the five actions are declared at [horizon.d.lua:1004-1008](api/horizon.d.lua) (`add_goal`,
`add_target_goal`, `clear_goals`, `clear_target_goals`, `set_body_rotation`), installed as queued actions at
`crates/event-horizon-lua/src/lua_mods.rs:2226-2230`, and applied to the registering mob after the handler returns
(`crates/event-horizon-lua/src/generated_runtime.rs:1565-1578`). The hooks are `mob_ai_register` and
`register_goals` (`crates/event-horizon-lua/src/generated_hooks/mod.rs:2251` and `:2269`), described at
[lua-bindings.md:502-503](lua-bindings.md#mobs).

**Old usage**: `horizon.on("mob_ai_register", function(mob) ... end, kind)`, the mob's view as the only argument, and
the five calls on `horizon`. `horizon.goal(name, def)` defined the goal, with `can_use(mob, state, out)`.
**New usage**: `horizon.on("mob_ai_register", function(event) ... end, kind)`, the view at `event.mob`. The five calls
keep their names, may also be made from `register_goals`, and run on the mob after the call. The goal's own functions
keep their shape. Three traps the rows above name: `register_goals` also fires for a client's copies, and before a
`"goals": "none"` mob's selectors are wiped, so a goal added there on such a mob is gone by `mob_ai_register`; a goal
added in both hooks is attached twice; and a handler that fails drops the goals queued before it.

`"stalk"` and `"hunt_player"` are goal names defined with `horizon.goal` at load, as in
[lua-bindings.md:249-263](lua-bindings.md#mob-goals).

```lua
-- old: scripts/init.lua
horizon.on("mob_ai_register", function(mob)
  if mob.kind == "minecraft:zombie" then
    horizon.clear_goals()
    horizon.clear_target_goals()
    horizon.add_goal(2, "stalk")
    horizon.add_target_goal(1, "hunt_player")
    horizon.set_body_rotation("smooth", 5)
  end
end, "minecraft:zombie")

-- new: scripts/server/init.lua; the same five calls, on event.mob
horizon.on("mob_ai_register", function(event)
  local mob = event.mob
  if mob.kind == "minecraft:zombie" then
    horizon.clear_goals()
    horizon.clear_target_goals()
    horizon.add_goal(2, "stalk")
    horizon.add_target_goal(1, "hunt_player")
    horizon.set_body_rotation("smooth", 5)
  end
end, "minecraft:zombie")
```

### Commands: `horizon.command` and `perform_command`

**Where it is today**: `horizon.command(usage, level, fn)` is installed at
`crates/event-horizon-lua/src/lua_mods.rs:2136-2137` and declared at [horizon.d.lua:958](api/horizon.d.lua).
`perform_command` is the generated hook (`crates/event-horizon-lua/src/generated_hooks/mod.rs:1864`, its runtime at
`crates/event-horizon-lua/src/generated_runtime.rs:1880`), described at
[lua-bindings.md:360-381](lua-bindings.md#commands).

**Old usage**: `horizon.command(usage, level, fn)` declared the command and attached `fn(player, args)`; a bare
`horizon.on("command", fn)` was the server hook that heard a declared command being performed.
**New usage**: `horizon.command(usage, level, fn(player, args))` is unchanged. The hook that hears a mod command is
`perform_command`, and it is the one hook whose handler keeps positional arguments: `fn(player, args)`, with the
command's words as the kind. A bare `horizon.on("command", fn)` is now the client hook: it fires for every slash line
the player types, before the server sees it, and `horizon.cancel()` there drops the line. Registering it from
`scripts/server/` fails the load, and `horizon.command` in a client state raises the server-only error.

```lua
-- old: scripts/init.lua
horizon.command("parasite count [kind:word]", 0, function(player, args)
  horizon.feedback(("%d"):format(count_parasites(args.kind)))
end)
horizon.on("command", function(player, args)        -- also heard the declared commands
  horizon.log("a mod command ran: " .. args.raw)
end)

-- new: scripts/server/init.lua; the second handler is perform_command (no kind hears
-- every declared command, as the bare old handler did). scripts/client/ gets the
-- client `command` hook instead, with `function(event)` and `event.command`
horizon.command("parasite count [kind:word]", 0, function(player, args)
  horizon.feedback(("%d"):format(count_parasites(args.kind)))
end)
horizon.on("perform_command", function(player, args)
  horizon.log("a mod command ran: " .. args.raw)
end)
```

### Per-mob saved data: `horizon.entity_data(id)`

**Where it is today**: declared as `entity_data: (id: number) -> Data?` ([horizon.d.lua:975](api/horizon.d.lua)), the
function at `crates/event-horizon-lua/src/lua_mods.rs:331`, the read at `lua_mods.rs:639-681`, its writeback
`store_fetched` (`lua_mods.rs:973`) after the handlers return (`lua_mods.rs:2873`);
described at [lua-bindings.md:393](lua-bindings.md#timers-and-saved-data).

**Old usage**: the last argument of the mob hooks (`mob_died`, `mob_killed`, `mob_tick`, `mob_interact`), edited in
place and written back after the handler. None of the generated events carries it.
**New usage**: read the table by the entity id the event names, in any hook with a host:
`horizon.entity_data(event.entity)`, written back after the call exactly like `horizon.world_data()`
([lua-bindings.md:405-406](lua-bindings.md#timers-and-saved-data)). It is nil when the entity is not in the level on a
server. On a client it is server-only: from any hook that is not a draw hook it raises "horizon.entity_data is only
available in scripts/server/ (this is the client state of mod ...)", and a draw hook is refused before any table is
built.

```lua
-- old: the saved table was the last argument, written back for you
horizon.on("mob_tick", function(dimension, mob, data)
  data.ticks = (data.ticks or 0) + 1
end, "minecraft:cow")

-- new: the event names the entity; the table is a read with the same writeback
horizon.on("entity_tick_post", function(event)
  local data = horizon.entity_data(event.entity)
  if data then data.ticks = (data.ticks or 0) + 1 end
end, "minecraft:cow")
```

A mod that already worked around the loss kept this state in `horizon.world_data()` keyed by the mob's id. That still
loads, but move the keys to `horizon.entity_data`: an entity's table is saved with the entity, and stays out of the
world's table.

