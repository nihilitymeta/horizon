//! `cargo run -p xtask -- lua-types`: `docs/api/horizon.d.lua`, Luau type
//! definitions for a scripted mod (`docs/lua-bindings.md`): the `horizon`
//! and `registry` globals, every function the runtime installs, every
//! hook's handler signature, and the tables they hand a script. For an
//! editor through luau-lsp (`--definitions @horizon=horizon.d.lua`); the
//! modkit ships a copy as `types/horizon.d.lua`. `--check` fails when the
//! file is not what the tables here say, and tests keep the tables equal to
//! the runtime: every hook `horizon.on` takes has a handler type, every
//! function the runtime installs has a declaration.
use super::lua_helper_model::{GenKind, HelperClass, classify_helpers, luau_function};
use super::lua_model::{Class, Kind, classify, event_name, extra_ts, replace_field_ts};
use std::{fs, path::Path};

/// The `(name, type)` lines of the helpers the helper generator emits
/// (every helper it classifies as Generated except the denylist
/// `EXCLUDED`), from the same model that emits the bridge: not a second
/// hand list. A helper that stops classifying as Generated is "dropped
/// from the output" and fails the run.
fn generated_helper_functions() -> Result<Vec<(String, String)>, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let rows = classify_helpers(&root)?;
    let mut out = Vec::new();
    for row in &rows {
        if !matches!(
            &row.class,
            HelperClass::Generated(GenKind::Read | GenKind::Action)
        ) {
            continue;
        }
        if super::lua_helpers::exclusion_of(&row.name).is_some() {
            continue;
        }
        out.push(luau_function(row));
    }
    Ok(out)
}

/// The tables a hook or helper hands a script, as Luau types.
const TYPES: &[(&str, &str)] = &[
    ("Vec3", "{ x: number, y: number, z: number }"),
    // `extra` is the components compound when the stack has any (nil when
    // plain); `plain` stays for the older readers.
    (
        "Item",
        "{ id: string, count: number, damage: number, extra: Data?, plain: boolean }",
    ),
    ("Held", "{ item: string, count: number, plain: boolean }"),
    // A player of the world, as the generated `player` seeds carry it:
    // the shared fields, with no `held`. A seed never sets a hand, so a
    // type built on this one never picks one up by accident.
    (
        "Player",
        "{ uuid: string, name: string, dimension: string, position: Vec3, yaw: number, pitch: number, game_mode: number, health: number }",
    ),
    // What the `horizon.player()`, `horizon.players()` and
    // `horizon.acting_player()` reads hand out: the bare player plus
    // `held`, the selected hotbar (main-hand) stack as an `Item` table,
    // nil for an empty hand or a player the host has no stack for (as on
    // a client window).
    ("ReadPlayer", "Player & { held: Item? }"),
    // The command handler's player: the bare player plus the acting
    // player's main hand in the `{item, count, plain}` hand shape
    // (`Held`), nil when empty. Never the read helpers' full `Item`
    // table: the two `held` shapes live on different types now.
    ("CommandPlayer", "Player & { held: Held? }"),
    // A player named by uuid in a hook: the world's record while they are
    // on, else just the uuid and the name a hook told before, if any (a
    // player whose state is out of reach otherwise).
    ("PlayerRef", "Player | { uuid: string, name: string? }"),
    ("IdKind", "{ id: number, kind: string }"),
    ("Block", "{ x: number, y: number, z: number, id: string }"),
    (
        "LootBlock",
        "{ x: number, y: number, z: number, id: string, state: string }",
    ),
    (
        "Nearby",
        "{ id: number, kind: string, position: Vec3, distance: number, health: number, alive: boolean, hostile: boolean, baby: boolean, target: any }",
    ),
    (
        "Home",
        "{ x: number, y: number, z: number, radius: number }",
    ),
    (
        "MobView",
        "{ id: number, kind: string, position: Vec3, velocity: Vec3, yaw: number, head_pitch: number, health: number, max_health: number, age: number, on_ground: boolean, in_water: boolean, in_lava: boolean, hurt_time: number, last_hurt_by: number?, target: any, target_position: Vec3?, target_in_melee_range: boolean, navigation_done: boolean, nearest_player: Vec3?, home: Home?, nearby: { Nearby } }",
    ),
    ("Who", "string | number | nil"),
    (
        "GoalOut",
        "{ navigate_to: (x: number, y: number, z: number, speed: number?) -> (), navigate_to_target: (speed: number?) -> (), stop_navigation: () -> (), move_toward: (x: number, y: number, z: number, speed: number?) -> (), look_at: (x: number, y: number, z: number) -> (), look_at_target: () -> (), set_target: (who: Who) -> (), jump: () -> (), attack: (who: Who) -> (), swing: () -> (), hurt: (who: Who, amount: number) -> (), convert: (kind: string) -> (), summon: (kind: string, x: number, y: number, z: number) -> (), despawn: () -> (), heal: (amount: number) -> (), effect: (name: string, ticks: number?, amplifier: number?) -> () }",
    ),
    ("GoalState", "{ [string]: any }"),
    (
        "GoalDef",
        "{ flags: { string }?, every_tick: boolean?, interruptable: boolean?, can_use: (mob: MobView, state: GoalState, out: GoalOut) -> boolean, can_continue: ((mob: MobView, state: GoalState, out: GoalOut) -> boolean)?, start: ((mob: MobView, state: GoalState, out: GoalOut) -> ())?, tick: ((mob: MobView, state: GoalState, out: GoalOut) -> ())?, stop: ((mob: MobView, state: GoalState, out: GoalOut) -> ())? }",
    ),
    ("Data", "{ [string]: any }"),
    ("Props", "{ [string]: string }"),
    // A block-entity tick's entity: the ticking entity's id, with the saved
    // compound as a table (always present, empty without keys).
    ("BlockEntity", "{ id: string, data: Data }"),
    // The vanilla JSON text-component table a text field's `_component`
    // carries (`{text=, color=, bold=, extra={}}`, `{translate=, with={}}`).
    (
        "Component",
        "{ text: string?, translate: string?, color: string?, bold: boolean?, italic: boolean?, underlined: boolean?, strikethrough: boolean?, obfuscated: boolean?, extra: { Component }? }",
    ),
    // Base shapes the generated event tables below stand on (`Vec3` for
    // positions, `Item` for stacks, `PlayerRef` for players the table names
    // by uuid, `Vec2` for pairs, `Chunk` for chunk columns).
    ("Vec2", "{ x: number, y: number }"),
    ("Chunk", "{ x: number, z: number }"),
    (
        "Time",
        "{ dimension: string, day_time: number, day: number, time_of_day: number, rain: number, thunder: number }",
    ),
    (
        "Raycast",
        "{ id: string, x: number, y: number, z: number, block: Vec3 }",
    ),
    ("Light", "{ block: number, sky: number }"),
    ("CommandArgs", "{ raw: string, [string]: any }"),
    ("Inventory", "{ [number]: Item }"),
    ("Carried", "{ [string]: number }"),
    // What `horizon.run_command_as`'s callback receives: whether the
    // command ran, vanilla's integer result, and one `output` line per
    // chat line with its matching component table in `output_components`
    // (the rich-text convention: a string plus a `_component` table).
    (
        "CommandResult",
        "{ ran: boolean, result: number, output: { string }, output_components: { Component } }",
    ),
    // The draw hooks' handles (`hud_hooks`): tables of plain functions
    // over the call's batch and corners, dot-called (`batch.fill(...)`).
    // Every method draws at once; the tables expire with the call. The
    // `HudEvent` and `GuiLayerPostEvent` tables are generated below, from
    // the same model as the runtime and its smoke test.
    (
        "HudBatch",
        "{ fill: (x: number, y: number, w: number, h: number, argb: number) -> (), text: (text: string, x: number, y: number, rgb: number, shadow: boolean) -> (), sprite: (name: string, sx: number, sy: number, sw: number, sh: number, dx: number, dy: number, dw: number, dh: number) -> boolean, item: (id: string, x: number, y: number, size: number) -> boolean, item_stack: (stack: Item, x: number, y: number, size: number) -> boolean, item_model: (item: string, model: string, x: number, y: number, size: number) -> boolean, quad: (x1: number, y1: number, x2: number, y2: number, x3: number, y3: number, x4: number, y4: number, argb: number) -> (), text_width: (text: string) -> number }",
    ),
    (
        "HudCorners",
        "{ place: (corner: string, w: number, h: number) -> (number, number), shown: (corner: string) -> boolean }",
    ),
];

/// The Luau keywords (the language reference's list): a table field named
/// one cannot be written bare, but a quoted string-literal property can
/// (`{ start: number, ["end"]: number }`).
const LUAU_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// The keyword field that starts after the separator at `sep` (`{`, `,` or a
/// newline), with any spacing or indentation before its name: the name's
/// index and the keyword. The name must be followed by a colon, so
/// `endpoint:` is not `end` (and `elseif:` is not `else`).
fn keyword_field_at(defs: &str, sep: usize) -> Option<(usize, &'static str)> {
    let trimmed = defs[sep + 1..].trim_start_matches([' ', '\t', '\r', '\n']);
    let start = defs.len() - trimmed.len();
    let keyword = LUAU_KEYWORDS.iter().copied().find(|kw| {
        trimmed
            .strip_prefix(kw)
            .is_some_and(|after| after.trim_start_matches([' ', '\t']).starts_with(':'))
    })?;
    Some((start, keyword))
}

/// Rewrites every keyword-named table field of a rendered definitions file
/// to its quoted form. It sees the field lists of both the hand-written
/// tables (`TYPES`, `FUNCTIONS`) and the generated ones, whose nested table
/// types (`{ start: number, end: number }`) are built in the model, out of
/// reach here. A field starts after `{`, `,` or a line break, with any
/// spacing or indentation before its name; parameter names share the
/// `, ` separator, but a keyword can never be one, so the rewrite is safe
/// there too. String literals and `--` comments are skipped: a `{ end: ` or
/// `, end: ` inside one is text, and quoting it would break the string.
fn quote_keyword_fields(defs: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Code,
        LineComment,
        LongComment(usize),
        SingleString,
        DoubleString,
        LongString(usize),
    }
    /// A long bracket opener at `i` (`[[`, `[=[`, ...): its `=` count.
    fn open_bracket(bytes: &[u8], i: usize) -> Option<usize> {
        if bytes.get(i) != Some(&b'[') {
            return None;
        }
        let mut level = 0;
        while bytes.get(i + 1 + level) == Some(&b'=') {
            level += 1;
        }
        if bytes.get(i + 1 + level) == Some(&b'[') {
            Some(level)
        } else {
            None
        }
    }
    /// A long bracket closer at `i` (`]]`, `]=]`, ...) for `level`.
    fn close_bracket(bytes: &[u8], i: usize, level: usize) -> bool {
        if bytes.get(i) != Some(&b']') {
            return false;
        }
        for k in 0..level {
            if bytes.get(i + 1 + k) != Some(&b'=') {
                return false;
            }
        }
        bytes.get(i + 1 + level) == Some(&b']')
    }
    let bytes = defs.as_bytes();
    let mut out = String::with_capacity(defs.len());
    let mut i = 0;
    let mut state = State::Code;
    while i < bytes.len() {
        match state {
            State::Code => {
                if bytes[i] == b'-' && bytes.get(i + 1) == Some(&b'-') {
                    if let Some(level) = open_bracket(bytes, i + 2) {
                        let end = i + 4 + level;
                        out.push_str(&defs[i..end]);
                        i = end;
                        state = State::LongComment(level);
                    } else {
                        out.push_str("--");
                        i += 2;
                        state = State::LineComment;
                    }
                } else if bytes[i] == b'\'' {
                    out.push('\'');
                    i += 1;
                    state = State::SingleString;
                } else if bytes[i] == b'"' {
                    out.push('"');
                    i += 1;
                    state = State::DoubleString;
                } else if let Some(level) = open_bracket(bytes, i) {
                    let end = i + 2 + level;
                    out.push_str(&defs[i..end]);
                    i = end;
                    state = State::LongString(level);
                } else if matches!(bytes[i], b'{' | b',' | b'\n') {
                    out.push(bytes[i] as char);
                    i += 1;
                    if let Some((start, keyword)) = keyword_field_at(defs, i - 1) {
                        // The indentation or spacing before the name is kept.
                        out.push_str(&defs[i..start]);
                        out.push_str(&format!("[\"{keyword}\"]"));
                        i = start + keyword.len();
                    }
                } else {
                    let c = defs[i..].chars().next().expect("a char at i");
                    out.push(c);
                    i += c.len_utf8();
                }
            }
            State::LineComment => {
                let c = defs[i..].chars().next().expect("a char at i");
                out.push(c);
                i += c.len_utf8();
                if c == '\n' {
                    state = State::Code;
                }
            }
            State::SingleString | State::DoubleString => {
                let quote = if state == State::SingleString {
                    b'\''
                } else {
                    b'"'
                };
                let c = defs[i..].chars().next().expect("a char at i");
                out.push(c);
                i += c.len_utf8();
                if c == '\\' {
                    // An escape: the next character is literal text.
                    if let Some(next) = defs[i..].chars().next() {
                        out.push(next);
                        i += next.len_utf8();
                    }
                } else if (c as u32) == quote as u32 {
                    state = State::Code;
                }
            }
            State::LongString(level) | State::LongComment(level) => {
                if close_bracket(bytes, i, level) {
                    let end = i + 2 + level;
                    out.push_str(&defs[i..end]);
                    i = end;
                    state = State::Code;
                } else {
                    let c = defs[i..].chars().next().expect("a char at i");
                    out.push(c);
                    i += c.len_utf8();
                }
            }
        }
    }
    out
}

/// Extra `horizon.on(name, fn[, kind])` overloads beyond the generated
/// hooks' own: empty now that every hook is generated (the generated
/// overloads come from the model below). The declaration-served
/// `block_entity` and `block_entity_ticks` take no `horizon.on`
/// registration, so they have no overload.
const HOOKS: &[(&str, &str)] = &[];

/// One generated hook's types, derived from the same model as the runtime
/// table and its smoke test, so the three cannot drift: the event-table
/// type, and the `horizon.on` overload with the kind parameter exactly
/// where the runtime filters by kind.
struct GeneratedHook {
    hook: String,
    event_ty: String,
    event_def: String,
    handler: String,
    has_kind: bool,
}

fn generated_hooks() -> Result<Vec<GeneratedHook>, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    for hook in classify(&root).map_err(|e| e.to_string())? {
        let Class::Generated(model) = hook.class else {
            continue;
        };
        // Declaration-served hooks take no `horizon.on` registration, so
        // they have neither an event type nor an overload. A listened game
        // query is likewise no script hook, so it has neither either.
        if super::lua_model::declaration_served(&model.name)
            || super::lua_model::listened_flag(&model.name).is_some()
        {
            continue;
        }
        let event_ty = event_name(&model.name);
        let mut fields: Vec<String> = Vec::new();
        for f in &model.fields {
            // Flat seeds, text-component siblings and all: the same keys
            // the reference's field lists derive from.
            for (name, ts) in super::lua_model::event_keys(f) {
                fields.push(format!("{name}: {ts}"));
            }
        }
        for extra in &model.extras {
            fields.push(format!("{}: {}", extra.name, extra_ts(&extra.conv)));
        }
        if let Some(o) = &model.out {
            // A replace out seeds nil until the handler answers.
            fields.push(format!("{}: {}", o.name, replace_field_ts(o)));
        }
        // `perform_command` answers `(player, args)`, not an event table:
        // the handler keeps the `horizon.command` shape, kind-filtered by
        // the dispatched command words.
        let (handler, has_kind) = if model.name == "perform_command" {
            (
                "(player: CommandPlayer, args: CommandArgs) -> ()".to_owned(),
                true,
            )
        } else {
            (
                format!("(event: {event_ty}) -> ()"),
                !matches!(model.kind, Kind::None),
            )
        };
        out.push(GeneratedHook {
            hook: model.name.clone(),
            event_def: format!("{{ {} }}", fields.join(", ")),
            handler,
            event_ty,
            has_kind,
        });
    }
    out.sort_by(|a, b| a.hook.cmp(&b.hook));
    Ok(out)
}

/// Every function and field of `horizon`, with its type; the runtime
/// installs the same names (the `api.set` calls, the reads, the timers and
/// the actions of `lua_mods.rs`). `on` is written out of `HOOKS`.
const FUNCTIONS: &[(&str, &str)] = &[
    ("mod_id", "string"),
    ("mod_version", "string"),
    ("side", "\"server\" | \"client\""),
    ("libraries", "{ [string]: string }"),
    ("log", "(msg: string) -> ()"),
    (
        "command",
        "(usage: string, level: number?, fn: (player: CommandPlayer, args: CommandArgs) -> ()) -> ()",
    ),
    ("goal", "(name: string, def: GoalDef) -> ()"),
    (
        "block_entity",
        "(block: string, def: { ticks: boolean?, data: Data? }?) -> ()",
    ),
    ("config", "() -> Data"),
    ("save_config", "(t: Data) -> ()"),
    ("require", "(name: string) -> any"),
    ("distance", "(a: Vec3, b: Vec3) -> number"),
    // Reads.
    ("block", "(x: number, y: number, z: number) -> string"),
    (
        "block_property",
        "(x: number, y: number, z: number, key: string) -> string?",
    ),
    ("mob", "(id: number) -> MobView?"),
    ("mobs", "() -> { MobView }"),
    ("players", "() -> { ReadPlayer }"),
    ("player", "(who: string) -> ReadPlayer?"),
    ("time", "() -> Time"),
    ("raycast", "(player: string, distance: number?) -> Raycast?"),
    ("world_data", "() -> Data"),
    ("player_data", "(who: string) -> Data?"),
    ("entity_data", "(id: number) -> Data?"),
    ("acting_player", "() -> ReadPlayer?"),
    ("light", "(x: number, y: number, z: number) -> Light"),
    (
        "surface_height",
        "(x: number, z: number, heightmap: string?) -> number?",
    ),
    ("inventory", "(who: string | ReadPlayer) -> Inventory?"),
    (
        "count_items",
        "(who: string | ReadPlayer, id: string) -> number?",
    ),
    ("carried_items", "(who: string | ReadPlayer) -> Carried?"),
    ("data", "(path: string) -> any"),
    ("menu_contents", "(who: string | ReadPlayer) -> { Item? }?"),
    ("menu_cursor", "(who: string | ReadPlayer) -> Item?"),
    (
        "has_permission",
        "(who: string | ReadPlayer, node: string, default_level: number) -> boolean",
    ),
    ("player_permission", "(who: string | ReadPlayer) -> number?"),
    (
        "dev",
        "{ inspect: (who: ReadPlayer | string, what: number | { x: number, y: number, z: number, face: Vec3?, dimension: string? }) -> boolean }",
    ),
    (
        "summon",
        "(dimension: string, kind: string, x: number, y: number, z: number) -> number?",
    ),
    // Timers.
    ("after", "(ticks: number, fn: () -> ()) -> ()"),
    ("every", "(ticks: number, fn: () -> ()) -> ()"),
    ("ticks", "() -> number"),
    // Actions, applied after the handler.
    ("broadcast", "(text: string) -> ()"),
    ("kill_mob", "(dimension: string, id: number) -> ()"),
    ("remove_mob", "(dimension: string, id: number) -> ()"),
    (
        "set_mob_health",
        "(dimension: string, id: number, health: number) -> ()",
    ),
    (
        "push_mob",
        "(dimension: string, id: number, x: number, y: number, z: number) -> ()",
    ),
    (
        "teleport_mob",
        "(dimension: string, id: number, x: number, y: number, z: number) -> ()",
    ),
    (
        "take_items",
        "((id: string, count: number) -> ()) & ((who: string | ReadPlayer, id: string, count: number) -> ())",
    ),
    (
        "give_items",
        "((id: string, count: number) -> ()) & ((who: string | ReadPlayer, id: string, count: number) -> ())",
    ),
    ("feedback", "(text: string) -> ()"),
    ("tell", "(who: string | ReadPlayer, text: string) -> ()"),
    (
        "action_bar",
        "(who: string | ReadPlayer, text: string) -> ()",
    ),
    ("title", "(who: string | ReadPlayer, text: string) -> ()"),
    ("subtitle", "(who: string | ReadPlayer, text: string) -> ()"),
    (
        "title_times",
        "(who: string | ReadPlayer, fade_in: number, stay: number, fade_out: number) -> ()",
    ),
    (
        "sound",
        "(name: string, x: number, y: number, z: number, volume: number?, pitch: number?) -> ()",
    ),
    (
        "particles",
        "(name: string, x: number, y: number, z: number, count: number?, spread: number?, speed: number?) -> ()",
    ),
    ("add_goal", "(priority: number, name: string) -> ()"),
    ("add_target_goal", "(priority: number, name: string) -> ()"),
    ("clear_goals", "() -> ()"),
    ("clear_target_goals", "() -> ()"),
    (
        "set_body_rotation",
        "(mode: string, degrees_per_tick: number?) -> ()",
    ),
    (
        "set_block_property",
        "(key: string, value: string, flags: number?) -> ()",
    ),
    ("schedule_tick", "(delay: number) -> ()"),
    ("handled", "() -> ()"),
    ("cancel", "() -> ()"),
    (
        "set_block",
        "(x: number, y: number, z: number, id: string, props: Props?, flags: number?) -> ()",
    ),
    (
        "fill",
        "(x1: number, y1: number, z1: number, x2: number, y2: number, z2: number, id: string, props: Props?) -> ()",
    ),
    ("run_command", "(line: string) -> ()"),
    (
        "give_experience",
        "(who: string | ReadPlayer, points: number) -> ()",
    ),
    (
        "give_levels",
        "(who: string | ReadPlayer, levels: number) -> ()",
    ),
    (
        "falling_block",
        "(x: number, y: number, z: number, id: string, vx: number?, vy: number?, vz: number?) -> ()",
    ),
    (
        "drop_item",
        "(x: number, y: number, z: number, id: string, count: number?) -> ()",
    ),
    (
        "set_nickname",
        "(who: string | ReadPlayer, name: string?) -> ()",
    ),
    (
        "effect",
        "(target: number | string | ReadPlayer, name: string, ticks: number, amplifier: number?) -> ()",
    ),
    (
        "net_send_to_player",
        "(who: string | ReadPlayer, channel: string, data: string) -> ()",
    ),
    (
        "send_custom_payload_to_player",
        "(who: string | ReadPlayer, channel: string, data: string) -> ()",
    ),
    (
        "net_send_to_server",
        "(channel: string, data: string) -> ()",
    ),
    (
        "open_menu",
        "(who: string | ReadPlayer, rows: number, title: string, items: { (Item | false)? }) -> ()",
    ),
    (
        "open_menu_with_input",
        "(who: string | ReadPlayer, rows: number, title: string, items: { (Item | false)? }, input: { start: number, [\"end\"]: number }) -> ()",
    ),
    (
        "set_menu_items",
        "(who: string | ReadPlayer, items: { (Item | false)? }) -> ()",
    ),
    ("close_menu", "(who: string | ReadPlayer) -> ()"),
    ("kick", "(who: string | ReadPlayer, reason: string) -> ()"),
    (
        "teleport_player",
        "(who: string | ReadPlayer, dimension: string, pos: Vec3, yaw: number, pitch: number) -> ()",
    ),
    (
        "run_command_as",
        "(who: string | ReadPlayer, line: string, callback: ((result: CommandResult) -> ())?) -> ()",
    ),
];

/// `horizon.*` names the Lua crate installs that are not native
/// helpers: hook registration (`on`), commands, the timers, and the
/// removed-action stubs (which raise with the field that replaces
/// them). A generated helper must never claim one of these names: it
/// would overwrite the hand-written function.
pub(crate) const RESERVED_EXTRA: &[&str] = &[
    "on",
    "profile",
    "set_count",
    "set_result",
    "set_duration",
    "set_distance",
    "set_multiplier",
    "set_strength",
    "set_amount",
    "reply",
];

/// Whether `name` is already a `horizon.*` function the Lua crate
/// installs (a hand-written [`FUNCTIONS`] entry or a reserved extra),
/// so the helper generator must never emit it as `horizon.<name>`.
pub(crate) fn reserved_horizon_name(name: &str) -> bool {
    FUNCTIONS.iter().any(|(n, _)| *n == name) || RESERVED_EXTRA.contains(&name)
}

/// The `registry` global of `scripts/registry.lua`.
const REGISTRY: &[(&str, &str)] = &[
    ("id", "string"),
    ("namespaces", "{ string }"),
    ("entity_type", "(name: string, def: Data) -> ()"),
    ("block", "(name: string, def: Data) -> ()"),
    ("item", "(name: string, def: Data) -> ()"),
    ("mob_effect", "(name: string, def: Data) -> ()"),
    ("creative_tab", "(name: string, def: Data) -> ()"),
    ("menu", "(name: string, def: Data) -> ()"),
    ("data", "(path: string, value: Data) -> ()"),
    ("asset", "(path: string, value: Data) -> ()"),
    ("file", "(path: string, value: Data | string) -> ()"),
    (
        "lang",
        "(locale: string, entries: { [string]: string }) -> ()",
    ),
];

fn render() -> Result<String, String> {
    let generated = generated_hooks()?;
    let mut out = String::new();
    out.push_str("-- Generated by `cargo run -p xtask -- lua-types` from xtask/src/lua_types.rs: edit that, then regenerate.\n");
    out.push_str("-- Luau type definitions for a Horizon scripted mod (docs/lua-bindings.md), for luau-lsp:\n");
    out.push_str("--   luau-lsp analyze --definitions @horizon=horizon.d.lua --platform=standard scripts\n\n");
    for (name, def) in TYPES {
        out.push_str(&format!("export type {name} = {def}\n"));
    }
    out.push_str("\n-- The event tables of the generated hooks: one table per hook,\n-- holding the struct's fields by name.\n");
    out.push_str("--\n-- A text field reads as its plain string plus a `<field>_component`\n-- table (vanilla JSON). A write is a string, a component table, or both:\n-- the changed side wins; both changed, the component wins; a cleared\n-- component falls back to the string. Setting the plain side to nil leaves\n-- a required text alone and clears an optional one (nil on both sides\n-- clears it too). In-place edits of the seeded component table count.\n--\n-- A sequence field reads its holes as nil. Edits in place keep positions:\n-- a cleared trailing slot shrinks the list (`false` pins an explicit hole\n-- instead), an append past the seeded length grows it (up to 65536 extra\n-- entries; replace the table to grow it further), while a table the script\n-- replaces wholesale reads its own length (use `false` for holes there).\n-- An item's `extra` keeps its NBT types when edited in place; a fresh\n-- table reads with default types.\n--\n-- In hud and gui_layer_post, `horizon.time`, `block`, `block_property`,\n-- `light` read the client's world; `player`, `players`, `mob`, `mobs`,\n-- `raycast`, `surface_height` need a server and read empty on a client\n-- window; `world_data`, `player_data`, `entity_data`, the acting player's\n-- inventory reads and `summon` raise an error. `held` on the player tables\n-- reads nil there, and `acting_player` answers nil with no acting player.\n");
    for g in &generated {
        out.push_str(&format!("export type {} = {}\n", g.event_ty, g.event_def));
    }
    out.push_str("\n-- `horizon.on(name, fn[, kind])`: one overload per hook, with its handler's signature.\n");
    out.push_str("export type HorizonOn =\n");
    for (hook, handler) in HOOKS {
        out.push_str(&format!(
            "    ((name: \"{hook}\", fn: {handler}, kind: string?) -> ()) &\n"
        ));
    }
    for (i, g) in generated.iter().enumerate() {
        // The runtime rejects a kind for hooks with no kind to filter by.
        let kind = if g.has_kind { ", kind: string?" } else { "" };
        let sep = if i + 1 == generated.len() { "" } else { " &" };
        out.push_str(&format!(
            "    ((name: \"{}\", fn: {}{kind}) -> ()){sep}\n",
            g.hook, g.handler,
        ));
    }
    out.push_str("\ndeclare horizon: {\n    on: HorizonOn,\n");
    for (name, ty) in FUNCTIONS {
        out.push_str(&format!("    {name}: {ty},\n"));
    }
    for (name, ty) in generated_helper_functions()? {
        out.push_str(&format!("    {name}: {ty},\n"));
    }
    out.push_str("}\n\n-- `scripts/registry.lua` and its modules (`scripts/registry/`) run with this instead of `horizon`.\ndeclare registry: {\n");
    for (name, ty) in REGISTRY {
        out.push_str(&format!("    {name}: {ty},\n"));
    }
    out.push_str("}\n");
    // Keyword-named fields get their quoted form last, over everything:
    // the generated events, the hand-written tables and the nested table
    // types the model builds.
    Ok(quote_keyword_fields(&out))
}

pub fn run(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let path = root.join("docs/api/horizon.d.lua");
    let out = render()?;
    if check {
        let current = fs::read_to_string(&path).unwrap_or_default();
        if current != out {
            return Err(format!(
                "{} is out of date: run cargo run -p xtask -- lua-types",
                path.display()
            ));
        }
    } else {
        fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    fn runtime_source() -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        std::fs::read_to_string(root.join("crates/event-horizon-lua/src/lua_mods.rs")).unwrap()
    }

    /// Every generated hook the runtime takes (`GENERATED_HOOK_NAMES` in the
    /// generated hooks) has a handler type, and no other hook has one: the
    /// generated overloads of `generated_hooks()` plus the hand-written
    /// `HOOKS` entries name exactly the generated set.
    #[test]
    fn every_hook_the_runtime_takes_has_a_handler_type() {
        fn names(src: &str, list: &str) -> BTreeSet<String> {
            let start = src.find(list).unwrap();
            let end = start + src[start..].find("];").unwrap();
            src[start..end]
                .split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect()
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let generated = std::fs::read_to_string(
            root.join("crates/event-horizon-lua/src/generated_hooks/mod.rs"),
        )
        .unwrap();
        let runtime = names(&generated, "pub const GENERATED_HOOK_NAMES");
        let mut typed: BTreeSet<String> = super::HOOKS.iter().map(|(h, _)| h.to_string()).collect();
        typed.extend(
            super::generated_hooks()
                .unwrap()
                .iter()
                .map(|g| g.hook.clone()),
        );
        assert_eq!(runtime, typed);
    }

    /// Every new hook name in the migration table (`docs/lua-hook-migration.md`,
    /// the per-hook rows' new-hook column) is a generated hook the runtime
    /// takes: a renamed row must name the hook that actually fires.
    #[test]
    fn migration_table_new_names_are_generated_hooks() {
        fn names(src: &str, list: &str) -> BTreeSet<String> {
            let start = src.find(list).unwrap();
            let end = start + src[start..].find("];").unwrap();
            src[start..end]
                .split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect()
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let generated = std::fs::read_to_string(
            root.join("crates/event-horizon-lua/src/generated_hooks/mod.rs"),
        )
        .unwrap();
        let runtime = names(&generated, "pub const GENERATED_HOOK_NAMES");
        let table = std::fs::read_to_string(root.join("docs/lua-hook-migration.md")).unwrap();
        let rows: Vec<&str> = table
            .lines()
            .skip_while(|l| !l.starts_with("## Per-hook rows"))
            .skip(1)
            .take_while(|l| !l.starts_with("## Cross-cutting"))
            .filter(|l| l.starts_with('|'))
            .collect();
        assert!(
            !rows.is_empty(),
            "no per-hook rows parsed; the migration-table check is vacuous"
        );
        // The `|---|---|...` separator names no hook: skip separator rows
        // (every cell empty or all dashes), never index 0 blindly.
        let (separators, rows): (Vec<&str>, Vec<&str>) = rows.into_iter().partition(|row| {
            row.split('|')
                .all(|cell| cell.trim().is_empty() || cell.trim().chars().all(|c| c == '-'))
        });
        assert_eq!(
            separators.len(),
            1,
            "expected the single |---| separator row in the migration table"
        );
        assert_eq!(
            rows.len(),
            52,
            "expected 52 per-hook rows in the migration table, found {}",
            rows.len()
        );
        let mut missing = Vec::new();
        for row in rows {
            let cells: Vec<&str> = row.split('|').collect();
            // `| old | new | shape | old args | lost | gained | filter |`.
            let new = cells[2];
            let mut hooks = Vec::new();
            for part in new.split('`').skip(1).step_by(2) {
                hooks.push(part.to_owned());
            }
            assert!(
                !hooks.is_empty(),
                "row names no new hook: {row}; the migration-table check is vacuous"
            );
            for hook in hooks {
                if !runtime.contains(&hook) {
                    missing.push(hook);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "migration table names hooks the runtime does not take: {}",
            missing.join(", ")
        );
    }

    /// Every function and field the runtime installs on `horizon` (the
    /// `api.set` calls, the reads and timers of the Lua code, the actions)
    /// is declared, and nothing is declared that the runtime does not install.
    #[test]
    fn every_function_the_runtime_installs_is_declared() {
        let src = runtime_source();
        let mut installed = BTreeSet::new();
        for part in src.split("api.set(").skip(1) {
            let part = part.trim_start();
            if let Some(name) = part.strip_prefix('"').and_then(|p| p.split('"').next()) {
                installed.insert(name.to_owned());
            }
        }
        for line in src.lines() {
            let line = line.trim();
            // `horizon.name = function` and a table of them, `horizon.name = {`.
            if let Some(rest) = line.strip_prefix("horizon.")
                && let Some(name) = rest.split(" = ").next()
                && (rest.contains(" = function") || rest.contains(" = {"))
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                installed.insert(name.to_owned());
            }
        }
        let actions_start = src.find("for action in [").unwrap();
        let actions_end = actions_start + src[actions_start..].find("]").unwrap();
        for name in src[actions_start..actions_end]
            .split('"')
            .skip(1)
            .step_by(2)
        {
            installed.insert(name.to_owned());
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let bridge =
            std::fs::read_to_string(root.join("crates/event-horizon-lua/src/generated_helpers.rs"))
                .unwrap();
        for list in ["GENERATED_ACTIONS", "GENERATED_READS"] {
            let start = bridge.find(list).unwrap();
            let end = start + bridge[start..].find("];").unwrap();
            for name in bridge[start..end].split('"').skip(1).step_by(2) {
                installed.insert(name.to_owned());
            }
        }
        let declared: BTreeSet<String> = super::FUNCTIONS
            .iter()
            .map(|(n, _)| n.to_string())
            .chain(
                super::generated_helper_functions()
                    .unwrap()
                    .into_iter()
                    .map(|(n, _)| n),
            )
            .chain(std::iter::once("on".to_owned()))
            .collect();
        assert_eq!(installed, declared);
    }

    /// Every name the registry script's `registry` table gets (the
    /// `registry.set` calls, the `data`/`asset` pair and the declaration
    /// kinds of `horizon-mods`' registry.rs) is declared, and no other.
    #[test]
    fn every_name_of_the_registry_table_is_declared() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let src =
            std::fs::read_to_string(root.join("crates/horizon-mods/src/registry.rs")).unwrap();
        let mut installed = BTreeSet::new();
        for part in src.split("registry.set(").skip(1) {
            let part = part.trim_start();
            if let Some(name) = part.strip_prefix('"').and_then(|p| p.split('"').next()) {
                installed.insert(name.to_owned());
            }
        }
        let pair_start = src.find("for (function, top) in [").unwrap();
        let pair_end = pair_start + src[pair_start..].find("] {").unwrap();
        for name in src[pair_start..pair_end].split('"').skip(1).step_by(4) {
            installed.insert(name.to_owned());
        }
        let kinds_start = src.find("const KINDS").unwrap();
        let kinds_end = kinds_start + src[kinds_start..].find("];").unwrap();
        for name in src[kinds_start..kinds_end].split('"').skip(1).step_by(2) {
            installed.insert(name.to_owned());
        }
        let declared: BTreeSet<String> =
            super::REGISTRY.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(installed, declared);
    }

    /// No player type carries both `held` shapes: the bare `Player` sets
    /// no hand (a `held` re-added there would intersect both shapes
    /// again on `CommandPlayer`, and leak an `Item` hand into
    /// `PlayerRef`), `ReadPlayer` the full `Item` table, `CommandPlayer`
    /// the `{item, count, plain}` hand.
    #[test]
    fn no_player_type_carries_both_held_shapes() {
        fn def(name: &str) -> &'static str {
            super::TYPES
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("no {name} type"))
                .1
        }
        assert!(
            !def("Player").contains("held"),
            "bare Player must not set held: {}",
            def("Player")
        );
        assert_eq!(def("ReadPlayer"), "Player & { held: Item? }");
        assert_eq!(def("CommandPlayer"), "Player & { held: Held? }");
        for (name, d) in super::TYPES {
            assert!(
                !(d.contains("held: Item?") && d.contains("held: Held?")),
                "{name} carries both held shapes: {d}",
            );
        }
    }

    #[test]
    fn the_definitions_are_what_the_tables_say() {
        super::run(true).unwrap();
    }

    /// A `{ end: ` or `, end: ` inside a string literal or a `--` comment is
    /// text, not a field: quoting it there would break the string. A field
    /// in code is still quoted.
    #[test]
    fn keyword_lookalikes_in_strings_and_comments_are_left_alone() {
        let input = "export type T = { start: number, end: number }\n\
             -- a comment: { end: number }, end: number\n\
             --[[ a long comment: { end: number }, end: number ]]\n\
             local s = \"{ end: number }, end: number\"\n\
             local q = '{ end: number }, end: number'\n\
             local l = [[{ end: number }, end: number]]\n\
             local e = \"an escaped quote \\\" { end: number }\"\n";
        let out = super::quote_keyword_fields(input);
        assert!(
            out.contains("export type T = { start: number, [\"end\"]: number }"),
            "{out}"
        );
        for kept in [
            "-- a comment: { end: number }, end: number\n",
            "--[[ a long comment: { end: number }, end: number ]]\n",
            "\"{ end: number }, end: number\"",
            "'{ end: number }, end: number'",
            "[[{ end: number }, end: number]]",
            "\"an escaped quote \\\" { end: number }\"",
        ] {
            assert!(out.contains(kept), "{kept} was rewritten: {out}");
        }
    }

    /// A keyword field is quoted when it starts a line of a multi-line block
    /// and when its braces or commas are unspaced; a keyword that only starts
    /// a longer name (`endpoint:`, `ending:`) is left alone.
    #[test]
    fn keyword_fields_are_quoted_across_lines_and_without_spaces() {
        let input = "declare x: {\n    start: number,\n    end: number,\n}\n\
             export type U = {end: number,start: number}\n\
             export type V = { endpoint: number, ending: number }\n";
        let out = super::quote_keyword_fields(input);
        assert!(out.contains("\n    [\"end\"]: number,\n"), "{out}");
        assert!(out.contains("{[\"end\"]: number,start: number}"), "{out}");
        assert!(
            out.contains("{ endpoint: number, ending: number }"),
            "{out}"
        );
    }

    /// A table field named with a Luau keyword is rendered in its quoted
    /// form, and nothing in the generated file writes one bare: a keyword
    /// field is a syntax error to luau-lsp. The keyword list here is its
    /// own (the language reference's), not the generator's.
    #[test]
    fn keyword_named_fields_are_quoted() {
        let keywords = [
            "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
            "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
        ];
        let defs = super::render().unwrap();
        // The three event tables that carry a range: their `end` is quoted.
        for line in defs.lines() {
            if line.contains("start: number") {
                assert!(line.contains("[\"end\"]: number"), "{line}");
            }
        }
        // No bare keyword field anywhere: not after `{`, not after `,`.
        for line in defs.lines() {
            for keyword in keywords {
                assert!(
                    !line.contains(&format!("{{ {keyword}: "))
                        && !line.contains(&format!(", {keyword}: ")),
                    "bare keyword field `{keyword}`: {line}",
                );
            }
        }
    }

    /// Every enabled read declares its answer, never `()`: a read's
    /// `bool` is `boolean` (an action's stays `()`), derived from the
    /// model's `luau_function` for each enabled read.
    #[test]
    fn enabled_reads_declare_a_value_return() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let rows = crate::lua_helper_model::classify_helpers(&root).unwrap();
        for row in &rows {
            if !matches!(
                row.class,
                crate::lua_helper_model::HelperClass::Generated(
                    crate::lua_helper_model::GenKind::Read
                )
            ) {
                continue;
            }
            if crate::lua_helpers::exclusion_of(&row.name).is_some() {
                continue;
            }
            let (_, sig) = crate::lua_helper_model::luau_function(row);
            assert!(!sig.ends_with("-> ()"), "{} declares {sig}", row.name);
        }
    }

    /// The plain-`bool` read `sneaking` declares `() -> boolean`.
    #[test]
    fn sneaking_is_typed_as_boolean() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let rows = crate::lua_helper_model::classify_helpers(&root).unwrap();
        let row = rows.iter().find(|r| r.name == "sneaking").unwrap();
        let (name, sig) = crate::lua_helper_model::luau_function(row);
        assert_eq!(name, "sneaking");
        assert_eq!(sig, "() -> boolean");
    }
}
