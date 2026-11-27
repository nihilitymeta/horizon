# Lua mods

Event Horizon runs Lua mods as sandboxed Luau scripts. A Lua mod is a package of
scripts, data and assets that the game reads and runs at start-up. It cannot touch
files, start processes or read the environment. It reaches the game only through the
`horizon` table and the handles the game gives it. This page covers the package
format, how mods are found and ordered, the limits on what a script may use, the
write powers the owner grants, and the Mods screen.

Everything here is read from the code in `crates/event-horizon-lua`. Where a number
matters, the source constant is named so you can check it.

## Where mods live

Installed Lua mods are the packages in `<game folder>/lua-mods/`. A package is either
a folder or a `.hlua` file (a zip archive, with the extension in lower case).
Anything else in that folder is ignored. A symbolic link that names a package, or
points at a folder, is refused by name (`discover.rs`, `DIR`, `ARCHIVE_EXT`).

While developing, a folder can be loaded from anywhere with a dev path:

- `--lua-mod PATH` or `--lua-mod=PATH` on the command line, repeatable;
- the `EH_LUA_MODS` environment variable, a list of paths separated by the system's
  path separator.

Up to 8 dev paths are read; the rest are refused (`discover.rs`, `MAX_DEV_PATHS`).
A dev mod replaces an installed mod with the same id. Dev mods can be reloaded in the
game with `/luareload` (see the [developer tools](dev-tools.md)). Installed `.hlua`
archives cannot be reloaded.

A scan reads at most 1024 mods, dev paths and installed packages together
(`discover.rs`, `MAX_MODS`).

## The package

A package has one root manifest, `lua-mod.toml`. The root holds:

- `lua-mod.toml`, the manifest;
- `main.luau`, the default script (used when the manifest lists no `entries`);
- `icon.png`, `LICENSE*` and `README*` files;
- any script the manifest lists in `entries`;
- the folders `assets/`, `data/` and `lang/`, with any files under them.

Anything else refuses the whole package by name (`hlua.rs`, layout notes at the top).

Limits a package must respect (`hlua.rs`):

| Limit | Value | Constant |
|---|---|---|
| `.hlua` file on disk | 64 MiB | `MAX_ARCHIVE` |
| entries (files and folders) | 4096 | `MAX_ENTRIES` |
| path depth | 16 segments | `MAX_DEPTH` |
| one entry, uncompressed | 8 MiB | `MAX_ENTRY` |
| all entries, uncompressed | 64 MiB | `MAX_TOTAL` |
| one script, uncompressed | 1 MiB | `MAX_SCRIPT` |
| all scripts together | 4 MiB | `MAX_TOTAL_SOURCE` |
| icon, uncompressed | 256 KiB | `MAX_ICON` |

The reader never extracts part of a package. A package that breaks a limit is
refused whole, and the other mods still load.

## The manifest

`lua-mod.toml` must name the mod's `id`, `version` and `side`. Every key is known;
an unknown key, a reserved id, an unknown power or a wrong type refuses the manifest
with a named reason (`manifest.rs`).

```toml
id = "alpha"
version = "1.0.0"
side = "both"              # "client", "server" or "both"
name = "Alpha Mod"
authors = ["Ann"]
license = "MIT"
description = "What the mod does."
entries = ["main.luau"]
powers = ["commands"]      # write powers the mod asks for (see below)
dependencies = { beta = "^1" }
```

| Key | Rule | Constant |
|---|---|---|
| `id` | 1 to 64 bytes of lowercase ASCII letters, digits and underscores | `MAX_ID` |
| `name`, `version`, `license` | up to 64 bytes each; `name` defaults to the id | `MAX_NAME`, `MAX_VERSION`, `MAX_LICENSE` |
| `authors` | up to 16 names of 64 bytes | `MAX_AUTHORS`, `MAX_AUTHOR` |
| `description` | up to 1024 bytes; line breaks and tabs kept | `MAX_DESCRIPTION` |
| `dependencies` | up to 32 entries; a requirement up to 64 bytes with at most 4 comparators | `MAX_DEPENDENCIES`, `MAX_REQUIREMENT`, `MAX_COMPARATORS` |
| `entries` | up to 16 scripts | `MAX_ENTRIES` |
| `powers` | up to 16 names, none repeated | `MAX_POWERS` |
| `settings` | up to 64 settings; nesting up to 4 levels; strings up to 256 bytes | `MAX_SETTINGS`, `MAX_SETTING_DEPTH`, `MAX_SETTING_TEXT` |

The manifest is read only up to 64 KiB (`MAX_MANIFEST`). The ids `minecraft`, `c` and
`horizon` are reserved and refused (`RESERVED_IDS`).

`side` decides where the mod runs. On a dedicated server a client-only mod is not
loaded; it is listed with a notice, not a refusal (`discover.rs`,
`SKIPPED_CLIENT_ONLY`).

## Dependencies and load order

A dependency loads before the mods that need it. A mod whose dependency is missing,
disabled or refused is refused too, with the reason (`load_order.rs`). Dependency
cycles are found and refused. The order is the same on every run: a dev mod comes
before an installed one, then the ids sort alphabetically.

If two installed packages have the same id, the one whose file name sorts first is
kept (`discover.rs`).

## Turning mods on and off

A mod with no entry in `lua-mods/enabled.toml` is enabled. The file's `[mods]` table
holds `id = false` for a mod that is off. The game writes that file when you use the
Mods screen. A file that is missing, not valid, or over 64 KiB is read as "every mod
enabled", and the game starts with one notice in the log (`enabled.rs`).

A change takes effect at the next start. The Mods screen shows a restart note
(`provider.rs`, `set_enabled`).

## Write powers

Scripts can read the world freely, but a write that changes the world needs a
power. A power is effective only when two things both hold (`powers.rs`):

1. the mod's manifest asks for it (`powers = [...]`), and
2. the owner grants it in `lua-mods/enabled.toml`:

```toml
[grants]
alpha = ["world"]
```

Anything not granted is refused. A mod that holds no power keeps every read and
every declaration. A grant changes on the next load, never inside a running turn.
A disabled, skipped or refused mod holds no power at all.

| Power | What it gates in this build |
|---|---|
| `world` | 20 writes: removing entities and block-entity bars, `remove_block`, weather, clock time, world border, lightning, saving, random ticks, teleports, and removing mob goals |
| `spawn` | 7 writes: `award_experience`, `set_respawn_data`, `PlayerRef::respawn`, `spawn_mob` (a mob joins the level, as a natural spawn does), and the item spawns `spawn_item`, `spawn_item_with_motion` and `pop_resource` (an item stack `{id, count, nbt}`). One hook turn may cause at most `spawns_per_turn` spawns over the four spawn writes (default 64, owner ceiling 1024, see Owner limits below): the next one is refused |
| `inventory` (changes players' and mobs' items) | 6 writes: `PlayerRef::give_stack(stack)`, `PlayerRef::set_inventory_slot(slot, stack)`, `PlayerRef::set_ender_chest_slot(slot, stack)`, `LivingRef::set_hand_item(hand, stack)`, `PlayerRef::drop_stack(stack, thrown_by_player)`, which throws the stack from the player's eyes as an item and returns nil, or returns the stack back when the drop was refused (`{id, count, nbt}`), and `LivingRef::set_equipment(slot, stack)`, where `slot` is one of `main_hand`, `off_hand`, `head`, `chest`, `legs`, `feet`, `body`, `saddle` and a nil stack clears the slot. A `LivingRef` comes from an entity: `entity:living()` answers a living entity (a mob, a player's entity, an armor stand, the dragon) with its handle and nil for any other entity, so `spawn_mob(...):living():set_equipment("head", nil)` works from a command or event handler. A player's handle is `player:entity():living()`. A `MobRef` only arrives in mob hook turns (goals, brain), where writes are refused; `mob:living()` is the same conversion there. An item stack is `{id, count, nbt}`. Still denied: `LivingRef::hurt_and_break` (an equipment slot, and it breaks an item) and `LivingRef::drop` (it takes a stack and returns an entity, not yet decided) |
| `explosions` | 2 writes: `level:explode(source, center, power, fire, interaction, settings)` and `level:explosion_destroy_blocks(positions, source, center, power, fire, interaction)`. The power is a Lua number from 0 to `explosion_power` (default 16, owner ceiling 64): a missing, non-number (a string, even `"999"`), negative, NaN, infinite or larger power is refused before the host sees the call. One hook turn may cause at most `explosions_per_turn` explosions over both writes (default 4, owner ceiling 64): the fifth is refused. The read `explosion_resistance` needs no power. Still denied: `explosion_resistances` (its `ResistanceCells` argument does not convert) |
| `game_rules` | 1 write: `level:set_game_rule(name, value)`. It changes the world's saved rules, and a changed rule stays changed after the mod is removed. The value is a boolean, a whole number, or `{tag = "bool", true}` / `{tag = "int", 7}` |
| `players` (changes players' recipes and maps) | 3 writes: `PlayerRef::award_recipes(ids)` and `PlayerRef::reset_recipes(ids)`, where `ids` is a list of recipe ids (strings), which unlock the recipes in the player's book (as `/recipe give`) or take them out (as `/recipe take`), each returning the number of recipe displays changed, or nil for a player the host cannot reach. The guard refuses a list of more than 64 ids, an empty id, an id of more than 128 characters, and an id with a control character (`horizon: award_recipes refused: <reason>`). `PlayerRef::send_map(map_id, colors)` sends a player the picture of a map: `colors` is a Lua string or an array table of integers 0 to 255, exactly 16384 bytes (128 x 128 colours, row by row), and `map_id` is the map's id (use ids far from the world's own). It returns false for a player not here, or a picture of the wrong length. The guard refuses a picture of more than 16384 bytes, or a table entry outside 0 to 255, and at most `maps_per_turn` pictures per hook turn (default 8, see Owner limits below). `PlayerRef::disconnect(reason)` removes the player from the server, with a reason they see: it returns true when the host removed them. A reason must be one plain string (a component table is refused). The host receives `[<mod id>] <reason>`, cut to 200 characters, and a turn makes at most `kicks_per_turn` disconnects (default 1, see Owner limits below). A player at permission level 2 or above (an operator) is refused (`horizon: disconnect refused: that player cannot be removed by a mod`), and so is a player whose level cannot be read. Read the disconnect note below before granting this power. Still denied: the other player writes, such as `award_stat` and `set_game_mode` |
| `nbt` | 2 writes: `EntityRef:merge_nbt(table)` (a raw merge into a saved entity; returns true when its data changed) and `BlockEntityRef:merge_data(table)` (a raw merge into a saved block entity). A merge adds and changes keys; it cannot remove one, so `{}` changes nothing. Refused by the guard: any merge into a player (`horizon: merge_nbt refused: players cannot be changed with raw NBT`), and a table whose top-level keys include `UUID`, `id`, `Pos`, `Passengers`, `Inventory`, `EnderItems` or `horizon:attachments` (a block entity merge also refuses `x`, `y`, `z`) (`horizon: merge_data refused: key Pos cannot be merged`; case-sensitive, top level only). Still denied: `replace_data`, `set_lock`, `EntityRef:nbt` (a raw read), `set_persistent_data` on a player, and every raw NBT write to a player |
| `commands` | 1 write: `entity:run_command(line, level)`, which runs a game command as the entity (the player, or a mob). `level` is `"all"`, `"moderators"` or `"gamemasters"` (the level cap is 2: `"admins"` and `"owners"` are refused). The line is checked against the command allowlist below before the host sees it. `entity:run_command_with_output(line, level)` takes the same arguments and the same checks, and returns a table `{success = boolean, feedback = {string, ...}}` with the command's messages: at most 64 lines of 1024 characters each, and a final `"..."` entry when lines were cut or dropped. Still denied: the console-level command runner |
| `registration`, `transactions` | named in the manifest and the Mods screen; no write is gated by them yet, so granting them does nothing |
| `network`, `files` | reserved: they can be requested and granted, but they are never effective |

The gated list is generated (`generated/powers.rs`). Every other write stays denied, among them the writes that are loot or world operations: `LevelRef::destroy_block`, `LevelRef::block_drops` and `LevelRef::roll_loot`.
Gated under `players`: `PlayerRef::award_recipes`, `PlayerRef::reset_recipes`,
`PlayerRef::send_map` and `PlayerRef::disconnect`. Gated under `commands`:
`EntityRef::run_command` and `EntityRef::run_command_with_output`.
Declarations, `event.feedback`, the `horizon.attachment_*` calls and settings are not
gated.

### Per-mod private data

A mod's private data on a player, an entity or a block entity is `horizon.attachment_get`,
`horizon.attachment_set` and `horizon.attachment_remove` (players included), under the mod's own
namespace. No raw NBT is needed for it. See the attachment functions in the Lua API.

Raw NBT merges (`merge_nbt`, `merge_data`) need the `nbt` power, never target a player, and refuse the keys
`UUID`, `id`, `Pos`, `Passengers`, `Inventory`, `EnderItems` and `horizon:attachments` (top level, case-sensitive); a block entity merge also refuses `x`, `y` and `z`.

Known limits of the power check:

- A `world` grant is not limited to the owner's own world. The check is per mod, not per
  world.
- A refused call raises a plain Lua error, which `pcall` can catch. The refusal
  costs one unit of the turn's budget. The warning goes to the log, once per mod and
  power, without the arguments. Nothing is shown on screen.
- `[grants]` allows at most 64 mods, and each up to 16 powers. A longer list for one
  mod refuses that mod's list as a whole.

### Owner limits (`enabled.toml` `[limits]`)

The owner may change the caps the guard applies to the gated writes:

```toml
[limits]
explosion_power = 16        # the most power one explosion may have (default 16, owner ceiling 64)
explosions_per_turn = 4     # the most explosions one hook turn may cause (default 4, ceiling 64)
spawns_per_turn = 64        # the most item and mob spawns one hook turn may cause (default 64, ceiling 1024)
maps_per_turn = 8           # the most map pictures (`send_map`) one hook turn may send (default 8, ceiling 64)
kicks_per_turn = 1          # the most players one hook turn may disconnect (default 1, ceiling 8)
```

A value that is missing, negative, not a whole number (for the counts), not a
number (for the power), or above its ceiling is the default instead, and the file gets
one notice in the log for all of them. A missing `[limits]` table is the defaults. The
ceilings are constants in the code: an owner cannot set a value above them, and a mod
can never read or change these numbers (`powers.rs`, `Limits`; `enabled.rs`).

### Disconnect (the `players` power)

`PlayerRef::disconnect` is the one write that removes a player, so it needs the owner's grant
like every gated write. The guard (`disconnect.rs`) refuses a player at permission level 2 or
above, a reason that is not one plain string, and a second disconnect in one turn (with
`kicks_per_turn` at its default of 1). It cannot tell the host from another player, and it
does not have to: in a singleplayer world without cheats the host has permission level 0, so
a mod granted `players` can remove the host, the way Bukkit's `kick` removes anyone not
protected by a permission level. The host API itself refuses the single-player owner
(its doc says `/kick` refuses the owner), which is the only host protection the call has;
that is INFERRED from the host's documentation, not from a test here. Grant `players`
only to a mod you trust with that.

### Command allowlist (`enabled.toml` `[command_allow]`)

A mod granted `commands` runs `entity:run_command(line, level)` through this allowlist (`command_guard.rs`):

```toml
[grants]
alpha = ["commands"]

[command_allow]
alpha = ["say", "give"]       # replaces the default set for alpha
beta = ["*"]                  # any command not on the never list, still at level 2 or lower
```

- **Default set** (a mod with no `[command_allow]` entry): `say`, `tellraw`, `title`, `playsound`, `particle`, `effect`, `tag`, `bossbar`, `scoreboard`. Anything else is refused.
- **Never** (refused whatever the owner lists, `*` included): `execute`, `function`, `reload`, `datapack`, `op`, `deop`, `ban`, `ban-ip`, `pardon`, `kick`, `stop`, `save-all`, `whitelist`, `perf`, `debug`.
- **Level cap**: `all`, `moderators` and `gamemasters` pass; `admins` and `owners` are refused. Level 3 and 4 commands therefore cannot run from a mod.
- **Names**: the command name is the first word of the line, after trimming spaces, one leading `/` and a `minecraft:` prefix. Matching is exact and case-sensitive. List bare names (`give`, not `minecraft:give`).
- **Lines**: at most 1024 characters, with no newline, carriage return or NUL.
- **Invalid entries** (not a list of strings, a name with a space or longer than 64 characters, more than 64 names, an invalid mod id) are ignored as a whole, so the mod gets the default set; one notice counts them. An empty list allows no command.

The refusals are plain Lua errors that `pcall` can catch, and the host is not called:

- `horizon: run_command refused: permission level is capped at 2`
- `horizon: run_command refused: "<name>" cannot be run by a mod`
- `horizon: run_command refused: "<name>" is not an allowed command for <mod>; the owner may list it under [command_allow]`
- `horizon: run_command refused: no command` (an empty line, or only a slash)
- `horizon: run_command refused: the command line is not valid`
- `run_command_with_output` refuses with the same messages, under its own name.

### Showing errors on screen

By default a mod's runtime errors and refused power calls go to the log only. The
owner can ask for one chat line instead, in `lua-mods/enabled.toml`:

```toml
[display]
errors = "owner"      # "off" (default) | "owner"
```

With `"owner"`, the first problem of each mod in a session sends the owner one line,
`[Lua] <mod>: 1 problem, e.g. "<first message>". See the log.`. Later problems of
that mod send nothing more. At most one line per mod, and five lines in all per
session. The owner is the host player in singleplayer and on a LAN host; on a
dedicated server there is no such player, so nothing is sent. No screen, no toast.
A missing, unknown or invalid value reads as `"off"`, with one notice in the log.

## Limits on what a script can use

Each mod runs in its own Luau state (`state.rs`):

- The state has no file, process or environment access. Its libraries are
  coroutine, table, os, string, utf8, bit, math, buffer and vector. The script
  loader (`require`) is removed.
- The state is limited to 64 MiB of memory (`state.rs`, `MEMORY_LIMIT`). A mod over
  that fails with a Lua memory error; other mods are not affected.

Time is budgeted in several places:

| Where | Budget | Source |
|---|---|---|
| Loading a mod's scripts (declaration) | 2 seconds, shared by all of a mod's scripts | `declare.rs`, `DECLARE_BUDGET` |
| One hook handler | 50 ms | `host.rs`, `HANDLER_BUDGET` |
| One goal turn (a mob goal's handler) | 1 ms | `goal_budget.rs`, `GOAL_TURN_BUDGET` |
| All goal turns of all mobs in a window | 4 ms per 50 ms window | `goal_budget.rs`, `GOAL_WINDOW_BUDGET`, `GOAL_WINDOW` |
| A goal handler stopped by its turn budget, in a row | 3 strikes, then the goal is quarantined | `goal_budget.rs`, `GOAL_STRIKES` |
| One developer console run | 500 ms | `event-horizon-lua/src/console.rs`, `CHUNK_BUDGET` |

A handler that runs over its budget stops with an error. `pcall` cannot swallow the
stop: the failure is repeated at every interrupt point (`budget.rs`).

A mod's host calls share a budget per turn of 10,000 units (`hostcall.rs`,
`TURN_BUDGET`). Each operation costs 1, 10 or 100 units, from the cost table in
`generated/handles.rs` (`cost`): a read is 1, a write 10, a heavy operation 100.
Once spent, every further call in that turn fails, and the
turn is void. Hook calls can nest at most 8 deep (`hostcall.rs`, `MAX_DEPTH`).

A mod whose host call panicked for a hook stops receiving that hook for the rest of
the process. Its other hooks and every other mod are not affected. At most 1024
(mod, hook) pairs are quarantined (`quarantine.rs`, `MAX_QUARANTINED`).

## Declaring a mod

A mod's `main.luau` runs once, at start-up, in declaring mode. It learns the hooks
it listens to, and the blocks, items, commands and other records it adds. In this
mode the chunk can call `horizon.on` and the declaration calls; it cannot reach the
world or the host (`declare.rs`). The declarations are then fixed until the game
restarts.

Limits on declarations, per mod (`declare.rs`):

| Call | Most per mod |
|---|---|
| `horizon.on` | 256 |
| `horizon.block`, `horizon.item` | 256 each |
| `horizon.block_part`, `horizon.item_part` | 128 together |
| `horizon.block_entity_type` | 64 |
| `horizon.world_fluid` | 8 |
| `horizon.entity_part` | 16 (one per entity type) |
| `horizon.menu_part` | 16 (one per menu type) |
| `horizon.command` | 64 (`horizon.d.luau`, `CommandRecord`: "At most 64 per mod") |

## Writing handlers

A handler is registered with `horizon.on(name, handler, options)`. The name is one
of the hooks in the `OnHook` type. The options are optional:

- `priority`: a whole number from -2147483648 to 2147483647; default 0.
- `receive_cancelled`: whether the handler runs on an event another mod already
  cancelled; default false (`horizon.d.luau`, `HandlerOptions`).

Two examples, written to match the declarations and the tests.

A server tick handler that does something only while a world is running:

```luau
horizon.on("server_tick", function(event)
	if event.phase.tag == "server_tick_start" and event.phase.world_running then
		-- runs at the start of each tick while a world is running
	end
end)
```

A server command, answered by `run_mod_command`. The mod declares the command's
usage once, and answers the event:

```luau
horizon.command{usage = "demo run <n:integer>"}

horizon.on("run_mod_command", function(event)
	event.feedback("the demo mod ran")
	event.ran = true
end)
```

`event.feedback` gives at most 32 lines per command, each at most 4096 bytes
(`commands.rs`, `MAX_FEEDBACK`, `MAX_FEEDBACK_BYTES`). Setting `event.ran = true`
answers the command as run; the command test that checks it expects the command to
be cancelled (`commands.rs`, tests around line 650).

Settings are read with `horizon.setting(key)` and changes are seen once with
`horizon.on_setting_changed(handler)`. A script cannot change a setting.

## Loot hooks

Four hooks take part in a loot roll. Each has one answer field, which the handler sets to
change the result. A value of the wrong type is refused and the game keeps its own value.

| Hook | Answer | Runs when |
|---|---|---|
| `loot_table_load` | `table` | a loot table is loaded from data |
| `loot_drops` | `drops` | a roll's drops are about to fall: a block breaks, a mob dies, an explosion goes off, or a player's use drops items (shears, a brush) |
| `loot_condition` | `holds` | a loot table checks a condition whose type is a mod's own kind |
| `loot_function` | `stack` | a loot table runs a function whose type is a mod's own kind on an item stack |

### Kinds of a mod's own

A loot table names a function or a condition of a mod's own kind with a `type` whose
namespace is not `minecraft:`. A function sits in a `modifier` list, and a condition in a
`condition` key:

```json
{"type": "minecraft:item", "name": "minecraft:stick", "modifier": [{"type": "mymod:double"}]}
```

- The `data` a hook gets is the whole object, `type` included.
- Every listener of `loot_function` and of `loot_condition` is called for every kind. A
  listener returns early when `event.kind` is not its own.
- An unclaimed function leaves the stack as it was. An unclaimed condition stays `false`,
  because `holds` starts `false`.

### Read-only tables and copies

`event.id`, `event.kind`, `event.data`, `event.ctx`, `event.context` and the tables the
game hands over (`event.table`, `event.stack`) are read-only. A change to one of them, even
a nested field, raises an error, and the handler stops without an answer. To change a stack
or a loot table, build a modified copy and assign it to `event.stack` or `event.table`.

This helper copies a read-only value. It returns an empty table as it is: a copy of an
empty JSON object would come out as an empty array (see [JSON rules](#json-rules)).

```luau
local function copy(value)
	if type(value) ~= "table" or next(value) == nil then
		return value
	end
	local result = {}
	for key, item in pairs(value) do
		result[key] = copy(item)
	end
	return result
end
```

### Example: a function of a mod's own kind

The sample below adds the function `loot_function_sample:double` to the modifier list of
every entry in `minecraft:chests/simple_dungeon`, when the table loads. The
`loot_function` handler then doubles the count of the stack it is given, capped at 64.

```luau
-- Loot-function sample: a loot function of this mod's own kind, reached from a vanilla table.
-- loot_table_load adds the function "loot_function_sample:double" to the modifier list of every
-- entry in minecraft:chests/simple_dungeon. loot_function then answers that kind: the stack's
-- count is doubled, capped at 64. Other kinds are left alone.
-- Hooks used: loot_table_load (listen, answer field: table), loot_function (listen, answer field: stack).

local TABLE_ID = "minecraft:chests/simple_dungeon"
local KIND = "loot_function_sample:double"
local MAX_COUNT = 64

-- A copy of a read-only JSON value. An empty table is shared, not copied: a copy would lose the
-- marker that keeps an empty JSON object an object.
local function copy(value)
	if type(value) ~= "table" or next(value) == nil then
		return value
	end
	local result = {}
	for key, item in pairs(value) do
		result[key] = copy(item)
	end
	return result
end

horizon.on("loot_table_load", function(event)
	if event.id ~= TABLE_ID then
		return
	end
	local table_copy = copy(event.table)
	for _, pool in ipairs(table_copy.pools) do
		for _, entry in ipairs(pool.entries) do
			local modifiers = {}
			for _, existing in ipairs(entry.modifier or {}) do
				modifiers[#modifiers + 1] = copy(existing)
			end
			modifiers[#modifiers + 1] = { type = KIND }
			entry.modifier = modifiers
		end
	end
	event.table = table_copy
end)

horizon.on("loot_function", function(event)
	if event.kind ~= KIND then
		return
	end
	local stack = copy(event.stack)
	stack.count = math.min(stack.count * 2, MAX_COUNT)
	event.stack = stack
end)
```

### loot_table_load

- `event.id` is the table's id, such as `minecraft:chests/simple_dungeon` (read-only).
- `event.table` is the table's JSON as a Lua table (see [JSON rules](#json-rules)).
- `table` answers with a whole new table. A non-table answer (`event.table = 5`) is refused,
  and the loaded table is kept.
- A table with a `null` inside an array is not delivered: the handler does not run.

### loot_drops

`event.context` describes what dropped the items:

| Field | Value |
|---|---|
| `table` | the loot table's id |
| `dimension` | the dimension's id |
| `origin` | `{x=, y=, z=}`, numbers |
| `block` | nil, or `{pos = {x=, y=, z=}, state = {id = ...}}` for a block break |
| `entity` | nil, or `{id = number, type = string}` for a mob |
| `player` | nil, or the player's UUID as a string |
| `damage_source` | nil, or the damage source table |
| `tool` | nil, or a stack table `{id, count, nbt}` |
| `weapon_item` | nil, or the death weapon as a stack table `{id, count, nbt}`: the weapon of a player's or mob's killing blow (nil for a block break) |
| `explosion` | nil, or a number |
| `used` | a boolean: true for a player's use |

`event.drops` is the list the game rolled. Each entry is a stack table `{id, count, nbt}`,
where `nbt` is optional. The `drops` answer is a whole new list, of at most 1024 entries.
An entry with `count` 0 refuses the whole answer, and the previous list is kept.

```luau
-- Loot-drops sample: a zombie that a player kills drops one extra bone.
-- loot_drops fires when a loot table rolls for a block, a mob or an explosion. event.context
-- says what dropped the items; event.drops is the list the game rolled, each entry a stack
-- table {id, count, nbt}. The handler answers by assigning a new list.
-- Hook used: loot_drops (listen, answer field: drops).

local TABLE_ID = "minecraft:entities/zombie"
local EXTRA_ITEM = "minecraft:bone"

horizon.on("loot_drops", function(event)
	local context = event.context
	if context.table ~= TABLE_ID or context.player == nil then
		return
	end

	-- The drops are read-only: keep every stack in a new list, then append the extra one.
	local drops = {}
	for _, stack in ipairs(event.drops) do
		table.insert(drops, stack)
	end
	table.insert(drops, { id = EXTRA_ITEM, count = 1 })
	event.drops = drops
end)
```

### loot_condition

- `kind`, the condition's type; `data`, the condition's object; `ctx`, the roll's context.
- `ctx` has `queried_table_id`, `queried_stack`, `entity`, `damage`, `player`, `tool`,
  `block`, `luck`, `biome`, `seed` and `place`, as the type file (`horizon.d.luau`) declares
  them. Missing values are nil. `queried_stack` is an item stack in the `ItemStackSpec` shape
  `{id, count, nbt}` (`nbt` optional), the same shape as the stacks of `loot_drops`. `seed` is a decimal string, because a roll's seed does not fit a Lua number.
- `holds` takes a boolean. Any other value is refused and `holds` stays `false`.

### loot_function

- `kind`, `data` and `ctx` as for a condition.
- `stack` is the stack the roll has so far, a loot stack table with the fields `item`
  (the id), `count`, `damage`, `enchantments`, `potion`, `map`, `exploration`,
  `ominous_amplifier`, `stew_effects`, `names` and `components`.
- The `stack` answer is a whole new loot stack table. Only `item` and `count` are required.
  The rest default as the game does: `damage` 0, the lists empty, `potion` and
  `ominous_amplifier` absent, `map` false. A component set to nil or false is removed.
- The engine checks the stack again when it writes it back. A stack it rejects leaves the
  roll unchanged.

### Refused answers

A refused answer is logged as a warning, and the value the game had stays. An answer of
`nil` is no answer: nothing changes and nothing is logged.

| Answer | Refused because |
|---|---|
| any value of the wrong type | for example `holds = "yes"`, `drops = 5` or `table = 5` |
| a stack key the game does not have | any key not in the list above is refused by name |
| an unknown item id | the id is not a known item |
| a missing or fractional `count` | `count` is required and must be a whole number |
| `exploration` set | exploration maps are read-only |
| a list over 256 entries | any list in a stack (`enchantments`, `stew_effects`, `names`, `components`) takes at most 256 items |
| a name over 64 characters | an item id, enchantment, potion, name or component name is at most 64 characters |
| a list with a hole | list keys must be 1 to n |
| a stack the engine rejects | the roll is unchanged |

### JSON rules

The data of loot tables and of a mod's own kinds crosses into Lua as JSON:

| Rule | Behaviour |
|---|---|
| an object | a table with string keys |
| an array | a table with keys 1 to n |
| a table that mixes keys, has a hole, or has a key that is not a string | refused |
| an empty JSON object | an empty table that is still an object when it comes back |
| a table a mod builds as `{}` | an empty array (`[]`) when it comes back |
| `null` inside an object | the key is left out |
| `null` inside an array | the hook is not delivered |
| a whole number up to 2^53 | an integer; any other finite number is a float; NaN and infinities are refused |
| nesting | at most 64 levels |
| size | at most 100,000 values in one JSON value |

A value past the nesting or size limit is not delivered.

### Tags

`horizon.tag_contains(kind, tag_id, entry_id)` says whether an entry is in a tag, and
`horizon.tag_entries(kind, tag_id)` lists a tag's members in the game's order (at most 1024; a tag with
more raises). `kind` is `"item"`, `"block"` or `"fluid"`; an `entity_type` raises, since the tag service
does not read entity-type tags. Ids are `namespace:path`, at most 256 bytes; a malformed id raises by
name, and an entry the game does not know is in no tag. Each read costs 1 unit of the turn's budget.

The reads work in every hook turn that has a host, and in `loot_condition` and `loot_function`, whose
events carry the tag service. A hook with neither (for example `loot_table_load`) raises
`not available in this hook`.

### Not available yet

- A loot stack's `exploration` is read-only (see [Refused answers](#refused-answers)).

## The Lua API

The full API is declared in `crates/event-horizon-lua/lua-types/horizon.d.luau`. Use
that file rather than a list copied here:

- `HorizonApi` gives the `horizon` table's functions;
- `OnHook` lists every hook name and its event type;
- the `*Event`, `*Ref` and `*Record` types give the fields of each event and handle.

An item stack argument is a table `{id = "namespace:path", count = 3, nbt = {...}}`
(`nbt` is optional and holds the component patch). The host makes the stack when the
call runs, and a call with a bad one raises a Lua error before the host sees it. The
argument is refused when it is nil and the parameter is required, when `id` is not a
valid identifier or names no item, when the item refuses the `nbt` patch, when `count`
is not a whole number, when `count` is 0 for a required stack, or when `count` is above
the item's maximum stack size. An optional stack (`nil` or `count = 0`) is passed as
none. Only reads take item stacks in this build: `LevelRef:block_break_speed`,
`MenuRef:slot_holds` and `PlayerRef:may_use_item_at`. Every write that takes a stack
stays denied.

To check a mod, run the checker on its folder or archive:

```
cargo run -p event-horizon-lua-check -- check path/to/mod
```

It loads the mod headless through the Lua host, lints its scripts with selene and
checks their format with StyLua. Exit code 0 means fine, 1 is a finding (a refused load,
a syntax or lint error, or a warning with `--strict`), and 2 is a usage error.
`--list` shows which Lua mods a game folder and dev paths give, and their status.
`emit-types <dir>` writes `horizon.d.luau` and a `.luaurc` for `luau-lsp`.

## The Mods screen

The Mods screen lists every installed mod. Lua mods come after the others, each with
a Lua badge. The row shows the mod's icon, kind and side; a mod that is off or
refused says so (`crates/horizon-client-render/src/mods_screen.rs`).

For a selected Lua mod, the panel offers:

- **Turn off / Turn on**, unless the mod was refused. It writes `enabled.toml` and
  shows the restart note (`mods_screen.rs`, `toggle_lua`).
- **Config**, for a mod that has settings or requests a power.

The Config page of a mod that requests a power has a Powers section. For each power
it shows whether it is granted and gives one button:

- **Revoke** removes a grant at once.
- **Grant** asks for a second click. The first click arms the button, which then
  reads **Confirm grant**; the second click grants.
- A power that is reserved, or has no write behind it in this build, shows
  **Not available** and cannot be granted.

Each change to a grant shows the restart note. A refused change shows its reason on
the page (`crates/horizon-client-render/src/options_ui.rs`).

## Checking a mod end to end

`tools/lua_host_e2e.py` runs the Lua host in a real game: it copies two fixture mods
(`tools/lua_host_e2e/lua-mods/`) into a scratch game folder, launches the game through
`horizon-ctl`, spawns loot rolls and kills a zombie, quits, and reads the game's log. It passes
when the log shows both mods loaded, the table-load, condition and function hooks reached Lua,
and the zombie's drops hold the gated emerald and the doubled diamonds. It needs a display (it
re-runs itself under `xvfb-run` when there is none) and the prepared resources.

Four small sample mods to copy and start from are in [`lua-samples/`](lua-samples/README.md).

    python3 tools/lua_host_e2e.py [--bin-dir DIR] [--resources DIR] [--out DIR] [--keep]

`--bin-dir` holds `horizon-client`, `horizon-ctl` and `horizon-native-mods/` (default
`target/fast`); `--out` gets the game log copy and the scratch folder is removed unless `--keep`.
The result is one JSON object of checks on stdout; the exit code is 0 only if all pass.
