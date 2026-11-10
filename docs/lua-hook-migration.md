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
| `explosion` | `explosion_detonate` | `{dimension: string, center: Vec3, blocks: {Vec3}}` | `(dimension, center, blocks)` stay the same, with `blocks` edited in place and written back | none | none | bare |
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

There are no positional handler arguments anymore, with one exception: `perform_command` keeps its declaring
call's `fn(player, args)` handler. Every other handler is `function(event)`, and the old argument positions map
to the event fields in the table above. A parameter the handler names past `event` reads nil (past `args` for
`perform_command`).

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

### Removed capabilities and their Lua replacements

Every loss below except `set_count` now has a Lua replacement; the rows above name the same
replacements per hook.

- Per-mob saved `data` in mob events (`mob_died`, `mob_killed`, `mob_tick`, `mob_interact`): the generated events
  carry no `data` field and no writeback. Read and write it with `horizon.entity_data(event.entity)` instead,
  keyed by the entity id, with the same writeback rules as `horizon.world_data()`.
- The event `player` table in `player_chat`, `item_crafted`, `item_picked_up`, `item_dropped`,
  and `mob_killed`: the event carries the uuid text instead. In a hook fired for that player,
  `horizon.acting_player()` is their table; otherwise resolve it with `horizon.player(event.player)` where
  the hook has a host. In `player_died` the event carries the uuid text and the hook has a host, so
  `horizon.player(event.player)` reads the table while they are still on; the name is unrecoverable once
  they are gone.
- The held item: `player_tick`'s `player` argument (with `held`) is gone with the argument; `attack_entity`
  and `inventory_tick` name no stack. Read the selected hotbar stack at `horizon.player(name).held` or
  `horizon.acting_player().held` instead, as a full `Item` table — for `inventory_tick` that is the ticking
  player's selected stack, not the stack in the ticking slot. Only a `perform_command` handler's
  `player` table still carries `held` in the old `{item, count, plain}` hand shape.
- The ticking player in `player_tick`: `horizon.acting_player()` returns the player whose tick fired.
  Read the whole list with `horizon.players()` instead when no player acts.
- `set_count` from `inventory_tick`: no replacement, since the hook has no stack to write to.

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
  `horizon.block_entity(block, {ticks, data})` instead of `horizon.on`. Registering for them fails the load with
  "no such hook".
