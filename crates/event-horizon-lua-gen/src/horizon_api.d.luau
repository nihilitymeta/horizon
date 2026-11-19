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

-- A config file (declare.rs OWNED_HOOKS): the event field of the config hooks. Only the mod that owns
-- the file's namespace runs their handlers. `text` is the file's contents.
export type ConfigFile = { mod_id: string, file: string, text: string }

-- A block state: its canonical id and its properties (item.rs: at most 64 properties, each name and
-- value at most 64 bytes). `props` may be left out when a state has none.
export type BlockStateSpec = { id: string, props: { [string]: string }? }

-- An item stack: its canonical id, its count (0 is the empty stack, at most 99) and its component
-- patch as a compound (item.rs: an id at most 256 bytes). `nbt` may be left out.
export type ItemStackSpec = { id: string, count: number, nbt: Compound? }

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

-- A config file: `kind` is "client", "common" or "server"; `file` names it (at most 128 bytes,
-- optional); `default` is its default text (at most 65536 bytes). At most 8 configs per mod.
export type ConfigRecord = {
	kind: "client" | "common" | "server",
	file: string?,
	default: string,
}

-- The `horizon` global. Its functions split by time: the declaring functions (block, item,
-- block_entity_type, world_fluid, block_part, item_part, block_entity_part, fluid_part,
-- client_command, config, command) and `on` work only while the mod loads; `log` and `none` work
-- only inside a handler. `horizon.log` takes a string. Control characters are written as `\u{..}`
-- escapes, and a line is cut at 1000 characters after escaping (host.rs MAX_LOG_LINE; an escape is
-- never split). A mod may write 20 lines at once, then 20 a second (host.rs LOG_BURST, LOG_PER_SECOND);
-- a line over that rate is dropped without an error. The mod keeps its last 100 lines
-- (MAX_LOG_LINES), dropping the oldest. Log lines have no per-turn cap: the per-turn cap of 16 lines
-- (4096 bytes each) is for a `Vec<Text>` output (dispatch.rs MAX_LINES). Outside its own handler, `horizon.<name>` raises
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
	client_command: (record: ClientCommandRecord) -> (),
	config: (record: ConfigRecord) -> (),
	command: (record: CommandRecord) -> (),
	log: (message: string) -> (),
	none: NoneMarker,
}

declare horizon: HorizonApi
