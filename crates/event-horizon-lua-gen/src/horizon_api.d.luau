-- Hand-written part of horizon.d.luau (issue 74, Phase 2 gate G2). The generated part follows it:
-- the event types, the `horizon.on` overloads, the Part handler tables and the handle families.
-- Every limit below is the host's own constant, named where it is read.
-- Lua shapes follow crates/event-horizon-lua-convert (vector.rs, text.rs, item.rs, nbt.rs, ids.rs).
-- Where a shape is only partly described here, the field is typed `any` and says so.

-- The Lua shapes of the convert crate's values.

-- A block position: `{x=, y=, z=}`, integers.
export type BlockPos = { x: number, y: number, z: number }

-- A rotation: `{pitch=, yaw=}`, floats.
export type Rotation = { pitch: number, yaw: number }

-- A text: a plain string, or a component table `{text=, color=, extra={...}}` (text.rs). Its other
-- keys are the component's style fields, typed loosely here.
export type Text = string | { text: string?, color: string?, extra: { Text }?, [string]: any }

-- Compound (NBT): a table keyed by tag name. Its tags are read and written as plain Lua values.
export type Compound = { [string]: any }

-- The options of `attachment_set`: both default to false.
export type AttachmentOpts = { sync: boolean?, copy_on_death: boolean? }

-- A config file (declare.rs OWNED_HOOKS): the event field of the config hooks. Only the mod that owns
-- the file's namespace runs their handlers. `text` is the file's contents.
export type ConfigFile = { mod_id: string, file: string, text: string }

-- A block state: its canonical id and its properties (item.rs: at most 64 properties, each name and
-- value at most 64 bytes). `props` may be left out when a state has none.
export type BlockStateSpec = { id: string, props: { [string]: string }? }

-- An item stack: its canonical id, its count (0 is the empty stack, at most 99) and its component
-- patch as a compound (item.rs: an id at most 256 bytes). `nbt` may be left out.
export type ItemStackSpec = { id: string, count: number, nbt: Compound? }

-- The `loot_drops` context (loot_lua.rs loot_drops_lua; read-only). Hand-written, because the
-- converter builds the table by hand. `block` is `{pos, state}`, `entity` is `{id, type}`,
-- `player` a UUID string, `tool` and `weapon_item` item stacks (`{id, count, nbt}`).
export type LootDropsContext = {
	table: string,
	dimension: string,
	origin: { x: number, y: number, z: number },
	block: { pos: BlockPos, state: BlockStateSpec }?,
	entity: { id: number, type: string }?,
	player: string?,
	damage_source: HurtSource?,
	tool: ItemStackSpec?,
	weapon_item: ItemStackSpec?,
	explosion: number?,
	used: boolean,
}

-- A handle of an entity, a level, a block entity, ... is a userdata. Each family's methods are
-- generated below as a `*Ref` type; a method is called with a colon: `player:name()`.

-- The right-click of a block (hostcall.rs block_click_lua). `dimension` is a LevelDimension (a
-- string, or a table for a custom dimension), `hand` a Hand, and `hit` a BlockHit. `player` is the
-- player's id as a string (a u128); the Lua handle is `event.player` of the events that give one.
export type BlockClick = {
	side: LogicalSide,
	dimension: LevelDimension,
	pos: BlockPos,
	hit: BlockHit,
	hand: Hand,
	stack: ItemStackSpec?,
	player: string,
	player_entity_id: number,
}

-- `event.source` of `run_mod_command`: `kind` names what sent the command, `permission` is its
-- level. The other keys depend on `kind` (commands.rs source_lua): `player` and `entity` are
-- handles; `position` is a BlockPos. For a command block, `level` is the dimension's id as text
-- (for example `minecraft:overworld`), not a handle (commands.rs `dimension_text`).
export type CommandSource = { kind: string, permission: number, [string]: any }

-- `event.command` of `run_mod_command`: the word path (`literals`), each argument by name (`args`;
-- the last of a name wins), the sender's `dimension` and its `position`.
export type CommandInvocation = {
	literals: { string },
	args: { [string]: any },
	dimension: string,
	position: BlockPos,
}

-- `horizon.none`: the answer that clears an optional output (dispatch.rs). At runtime it is a
-- userdata, which Luau cannot name, so this table type stands for it; an optional output takes it
-- as `T | NoneMarker?`. No other value clears one.
export type NoneMarker = { horizon_none: true }

-- Feedback: `event.feedback(text)` in a `run_mod_command` handler sends one literal line to the
-- sender. At most 32 calls per command, each at most 4096 bytes (commands.rs MAX_FEEDBACK,
-- MAX_FEEDBACK_BYTES); a call past the limit raises.
export type Feedback = (text: string) -> ()

-- Options of `horizon.on(name, handler, options)`. Every `horizon.on` of one hook must use the same
-- options (declare.rs parse_options).
export type HandlerOptions = {
	-- A whole number from -2147483648 to 2147483647; the default is 0.
	priority: number?,
	-- Whether a handler runs on an event another mod already cancelled; the default is false.
	receive_cancelled: boolean?,
}

-- The records a mod declares while it loads (declare.rs). Declarations are refused once the chunk has
-- run, and `horizon.on` too.

-- A goal of an entity part's `goals` table. `name` is `<mod>:<path>` in the mod's own namespace (at
-- most 64 bytes, declared once per mod). `priority` is a whole number from 0 to 20; as vanilla's
-- addGoal, a lower number runs first. `flags` lists what the goal holds while it runs (at most one
-- goal holds each flag): `move`, `look`, `jump`, `target`; an unknown flag raises. `can_use` (required)
-- answers true to start the goal, and anything else (nil included) does not. `can_continue` answers
-- whether a running goal keeps running: nil keeps it running, false stops it, and a failed call stops
-- it; without a `can_continue` the goal keeps its `can_use` answer. `start`, `tick` and `stop` answer
-- nothing; `stop` always runs. Each handler is called with one argument, `e.mob` (a MobRef). A goal
-- can read through its mob's handle, but its writes raise (no server host in a goal's turn). Keep a
-- table per mob in an upvalue keyed by `e.mob:entity()` if you need one: the host does not save it.
-- Each goal's can_use, can_continue, start and tick runs under its own deadline of 1 ms (a loop is
-- stopped there); stop runs under the 50 ms handler budget. The goals of all mobs share a budget of 4 ms
-- per 50 ms window, so one window costs at most 4 ms plus one turn's 1 ms. Once the window's budget is
-- spent: can_use answers false, can_continue keeps its last answer, and tick is skipped (stop still
-- runs). A turn stopped by its deadline is a budget failure: it answers nothing (can_use false,
-- can_continue stops the goal), and the actions it queued are DROPPED. An ordinary Lua error is not a
-- budget failure: the actions made before the error stay applied, as handle writes do. Three budget
-- failures in a row of one goal handler quarantine that goal handler (the other goals of the mod keep
-- theirs) and log one warning; a turn that is not a budget failure resets the count. A stop stopped by
-- its budget is dropped with its actions and is not counted. A panic in a goal handler quarantines its
-- handler name (`can_use`, `tick`, ...) for the whole mod, so every goal of the mod stops getting that
-- call, not only the one that panicked. `e.ctx` (GoalCtx) makes the mob's navigation and target
-- changes, at most 64 per call, each costing 10 units of the turn's budget; they are applied in order
-- when the handler returns, so a read in the same handler does not see them yet. A read in a handler
-- sees the mob as the tick left it.
export type EntityGoalSpec = {
	name: string,
	priority: number,
	flags: { string }?,
	can_use: (event: GoalEvent) -> (boolean?),
	can_continue: ((event: GoalEvent) -> (boolean?))?,
	start: ((event: GoalEvent) -> ())?,
	tick: ((event: GoalEvent) -> ())?,
	stop: ((event: GoalEvent) -> ())?,
}

-- The goal actions of one goal call (goal_actions.rs). Each returns nothing. `move_to` takes a
-- finite position and a finite speed from 0.0 to 3.0; `look_at` a finite position; `set_target` a
-- MobRef, an EntityRef or nil (no target). A value outside these is an error naming it, and the
-- action is not made. A `GoalCtx` kept past its call raises "not available outside this goal call".
export type GoalCtx = {
	move_to: (self: GoalCtx, x: number, y: number, z: number, speed: number) -> (),
	stop_navigation: (self: GoalCtx) -> (),
	look_at: (self: GoalCtx, x: number, y: number, z: number) -> (),
	set_target: (self: GoalCtx, target: MobRef | EntityRef | nil) -> (),
	jump: (self: GoalCtx) -> (),
}

export type GoalEvent = { mob: MobRef, ctx: GoalCtx }

-- A brain behavior of an entity part's `behaviors` table (behavior.rs). `name` is `<mod>:<path>` in the mod's
-- own namespace (at most 64 bytes, declared once per mod). `activity` is the brain activity it starts in:
-- core, idle, work, play, rest, meet, panic, raid, pre_raid, hide, fight, play_dead, sniff, dig, emerge,
-- roar or investigate; any other name raises. `priority` is a whole number from 0 to 20 (a lower number runs
-- first). `min_duration` and `max_duration` give the window the run is drawn from (default 40 and 80 brain
-- ticks: an assumption, not a vanilla value); the window must be one vanilla draws (min_duration at least 0,
-- and max_duration + 1 - min_duration at least 1). At most 8 behaviors per part. At least one handler is
-- needed. Each handler is called with one argument, `e` (BehaviorEvent).
-- `check_extra_start_conditions` answers true to start the behavior; only a boolean true starts it, and nil
-- or any other value does not (without the handler the behavior starts). `can_still_use` answers whether it
-- keeps running: nil keeps it running, false stops it, and a failed call stops it; without a handler it
-- stops after its first tick. `start`, `tick` and `stop` answer nothing; `stop` always runs.
-- Budget: every behavior of every brain shares the goals' budget of 4 ms per 50 ms of wall clock. Each
-- handler's turn runs under its own deadline of 1 ms (a loop is stopped there); `stop` runs under the 50 ms
-- handler budget. Once the window's budget is spent: check_extra_start_conditions answers false (it is not
-- run), can_still_use keeps its last answer, and tick is skipped (start and stop still run). A turn stopped
-- by its deadline is a budget failure: it answers nothing (check false, can_still_use stops the behavior), and
-- the e.ctx actions it queued are DROPPED. An ordinary error keeps the actions made before it. Three budget
-- failures in a row of one handler quarantine that handler (the mod's other behaviors keep theirs), with one
-- warning. A panic in a handler quarantines its handler name for the whole mod.
export type BehaviorSpec = {
	name: string,
	activity: string,
	priority: number,
	min_duration: number?,
	max_duration: number?,
	check_extra_start_conditions: ((event: BehaviorEvent) -> (boolean?))?,
	can_still_use: ((event: BehaviorEvent) -> (boolean?))?,
	start: ((event: BehaviorEvent) -> ())?,
	tick: ((event: BehaviorEvent) -> ())?,
	stop: ((event: BehaviorEvent) -> ())?,
}

-- The behavior actions of one behavior call (behavior_actions.rs). Each returns nothing and is applied after
-- the handler returns. `set_walk_target` takes whole block coordinates, a speed from 0.0 to 3.0 and `close`,
-- a whole number from 0 to 64; `set_brain_target` takes a MobRef, an EntityRef or nil. At most 64 actions per
-- call, each costing 10 units of the turn's budget. A value outside these is an error naming it, and the action
-- is not made. A `BehaviorCtx` kept past its call raises "not available outside this behavior call".
export type BehaviorCtx = {
	set_walk_target: (self: BehaviorCtx, x: number, y: number, z: number, speed: number, close: number) -> (),
	set_brain_target: (self: BehaviorCtx, target: MobRef | EntityRef | nil) -> (),
}

-- The argument of a behavior handler: `mob` (a MobRef; its reads go through the handle) and `ctx`.
export type BehaviorEvent = { mob: MobRef, ctx: BehaviorCtx }

-- A block made from a vanilla block of the given template: `id` is `<mod>:<path>`, `template` is
-- `minecraft:<path>`. At most 256 blocks per mod.
export type BlockRecord = { id: string, template: string }

-- An item, the same fields as a block. At most 256 items per mod.
export type ItemRecord = { id: string, template: string }

-- A block entity type: `id` is `<mod>:<path>`; `blocks` lists the block ids it serves (at most 32).
-- At most 64 block entity types per mod.
export type BlockEntityTypeRecord = { id: string, blocks: { string } }

-- A fluid of the world: `id` is `<mod>:<path>`. The other fields are optional integers or
-- booleans as the fluid's behaviour needs them. At most 8 world fluids per mod.
export type WorldFluidRecord = {
	id: string,
	tick_delay: number?,
	drop_off: number?,
	slope_find_distance: number?,
	source_conversion: boolean?,
	bucket: boolean?,
}

-- A client-side command: `usage` is its usage line, at most 256 characters. At most 64 per mod.
export type ClientCommandRecord = { usage: string }

-- A server command: `usage` as above, and `level`, an integer 0 to 4 (default 0). At most 64 per
-- mod, and each usage is declared once.
export type CommandRecord = { usage: string, level: number? }

-- A data map type: `id` is `<mod>:<path>` in the mod's own namespace (at most 64 characters);
-- `registry` is the registry it is for, any the game names (`minecraft:item`). At most 16 per mod.
export type DataMapTypeRecord = { id: string, registry: string }

-- A data pack file of a data map: `key` is `data_maps/<mod>:<registry folder>/<path>` in the mod's
-- own namespace; `json` is its text, at most 262144 bytes, nested at most 32 deep, with at most 4096
-- entries (object members and array elements). At most 64 files per mod.
export type DataPackFileRecord = { key: string, json: string }

-- A config file: `kind` is "client", "common" or "server"; `file` names it (at most 128 bytes,
-- optional); `default` is its default text (at most 65536 bytes). At most 8 configs per mod.
export type ConfigRecord = {
	kind: "client" | "common" | "server",
	file: string?,
	default: string,
}

-- The `horizon` global. Its functions split by time: the declaring functions (block, item,
-- block_entity_type, world_fluid, block_part, item_part, block_entity_part, fluid_part,
-- entity_part, menu_part, client_command, config, command, data_map_type, data_pack_file) and `on` work only while the mod
-- loads; `log`, `none`, `data_map` and the attachment functions work only inside a handler.
-- `data_map(registry, entry, map)` reads the value an entry has in a data map, as NBT in Lua (nil
-- where it has none); a read costs 1 unit of the turn's budget. `tag_contains(kind, tag_id, entry_id)`
-- says whether an entry is in a tag, and `tag_entries(kind, tag_id)` lists a tag's members in the game's
-- order (at most 1024; more raises). `kind` is "item", "block" or "fluid"; ids are `namespace:path`, at most
-- 256 bytes; a malformed id raises, and an entry the game does not know is in no tag. Each read costs 1 unit.
-- Tags are read in every turn that has a host, and in a loot condition or a loot function too. The attachment functions keep a
-- value on an entity, a player or a block entity under a name `<mod>:<path>` in the mod's own
-- namespace: `get` reads it (nil where none), `set` stores a table (true), `remove` drops it (true if
-- it was there). A holder must be resident in the server's level; a block entity handle must be the
-- current one (a removed or unloaded block entity raises). Set and remove are server only: a client
-- turn raises. A client turn's `get` reads the copy of a value the server synced (nil where none), and
-- raises for a block entity, whose attachments the server does not send. Set takes an optional `opts`
-- table: `{ sync = true }` sends the value to the clients that see the holder, `{ copy_on_death = true
-- }` copies it to a respawned player (a player holder only, an entity raises); both default to false,
-- and an unknown key or a non-boolean value raises naming it. A block entity takes neither option
-- (each raises). Limits, each refused with its reason and nothing changed: the name at most 256 bytes;
-- the value at most 65536 bytes as NBT (4096 for a synced or copied value), with the converter's
-- depth, entry and string limits; 16 names per holder per mod; 262144 bytes of values per holder per
-- mod; 4 synced and 4 copied-on-death names per holder per mod; 16 synced writes per turn; 4096
-- block-entity names created by new-name sets per mod in one run of the game (a runaway guard: an
-- overwrite does not count, and the count starts again at each start). A set without `sync` over a
-- synced name, or a remove of one, sends the clients a removal of their copy. The mod's own index of
-- names is never synced. Get costs 1 unit, set and remove 10. `entity_part(id, handlers)`
-- binds one entity type's handlers, at most 16 parts per mod, and its `goals` (EntityGoalSpec: at most
-- 8 goals per part, each a mob goal of the mod that the mob gets at its construction, at most one
-- entity part per type). Declaring a goal runs no Lua; each of its calls is one Lua turn under a shared
-- budget of 4 ms of goal time per 50 ms of wall clock for all mobs (goal_budget.rs). `is_shearable` answers a boolean or
-- nothing. `on_sheared` (server only) answers its drops: at most 16 stacks, 256 items in all, each
-- within its item's maximum; a list over a bound is refused whole, with a warning. `spawn_cluster_size`
-- answers a whole number from 0 to 64 (an answer above 64 is refused with a warning and the next part,
-- or vanilla, answers). `horizon.log` takes a string. Control characters are written as `\u{..}`
-- escapes, and a line is cut at 1000 characters after escaping (host.rs MAX_LOG_LINE; an escape is
-- never split). A mod may write 20 lines at once, then 20 a second (host.rs LOG_BURST, LOG_PER_SECOND);
-- a line over that rate is dropped without an error. The mod keeps its last 100 lines
-- (MAX_LOG_LINES), dropping the oldest. Log lines have no per-turn cap: the per-turn cap of 16 lines
-- (4096 bytes each) is for a `Vec<Text>` output (dispatch.rs MAX_LINES). `menu_part(id, handlers)` binds
-- `menu_still_valid` to a mod menu type that another mod (native or content) declared: at most 16 menu
-- parts per mod, one per menu id, the id namespaced. A Lua mod cannot declare a menu type itself (menu
-- types are content files, and a Lua package has none), so the menu it names must come from another mod.
-- Outside its own handler, `horizon.<name>` raises
-- "not available yet" (host.rs runtime_table).
export type HorizonApi = {
	on: OnHook,
	block: (record: BlockRecord) -> (),
	item: (record: ItemRecord) -> (),
	block_entity_type: (record: BlockEntityTypeRecord) -> (),
	world_fluid: (record: WorldFluidRecord) -> (),
	-- A Part's id is `<mod>:<path>` (or another mod's block or item); its handler table names the
	-- Part handlers the Part trait has (generated below). At most 128 Parts of each kind.
	block_part: (id: string, handlers: BlockPartHandlers) -> (),
	item_part: (id: string, handlers: ItemPartHandlers) -> (),
	block_entity_part: (id: string, handlers: BlockEntityPartHandlers) -> (),
	fluid_part: (id: string, handlers: FluidPartHandlers) -> (),
	entity_part: (id: string, handlers: EntityPartHandlers & { goals: { EntityGoalSpec }?, behaviors: { BehaviorSpec }? }) -> (),
	-- A menu part's id is the mod menu type's id (`<mod>:<path>`, any namespace); it is not checked
	-- against the game's menus, so a part on a menu that never opens is never asked.
	menu_part: (id: string, handlers: MenuPartHandlers) -> (),
	client_command: (record: ClientCommandRecord) -> (),
	config: (record: ConfigRecord) -> (),
	command: (record: CommandRecord) -> (),
	data_map_type: (record: DataMapTypeRecord) -> (),
	data_pack_file: (record: DataPackFileRecord) -> (),
	data_map: (registry: string, entry: string, map: string) -> any,
	tag_contains: (kind: string, tag_id: string, entry_id: string) -> boolean,
	tag_entries: (kind: string, tag_id: string) -> { string },
	attachment_get: (holder: EntityRef | PlayerRef | BlockEntityRef, name: string) -> any,
	attachment_set: (holder: EntityRef | PlayerRef | BlockEntityRef, name: string, value: any, opts: AttachmentOpts?) -> boolean,
	attachment_remove: (holder: EntityRef | PlayerRef | BlockEntityRef, name: string) -> boolean,
	log: (message: string) -> (),
	-- `horizon.setting(key)`: the current value of a setting the mod declares in its manifest (`[[settings]]`),
	-- read from the player's `config/<mod>/settings.toml` or its default. A toggle is a boolean, a slider a
	-- number, and a choice, colour, keybind or text setting a string. Usable in a handler and while the mod
	-- loads (a declaration may depend on it). An unknown key is an error that names it.
	setting: (key: string) -> boolean | number | string,
	-- `horizon.on_setting_changed(handler)`: the mod's one change handler, registered while the mod loads.
	-- The handler gets the key, the new value and the old one, once per change, in order. A failing handler
	-- is logged and does not undo the change. There is no `set` in Lua: a handler cannot change a setting.
	on_setting_changed: (handler: (key: string, value: boolean | number | string, old: boolean | number | string) -> ()) -> (),
	none: NoneMarker,
}

declare horizon: HorizonApi
