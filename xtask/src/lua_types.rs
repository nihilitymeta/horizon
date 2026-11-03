//! `cargo run -p xtask -- lua-types`: `docs/api/horizon.d.lua`, Luau type
//! definitions for a scripted mod (`docs/lua-bindings.md`): the `horizon`
//! and `registry` globals, every function the runtime installs, every
//! hook's handler signature, and the tables they hand a script. For an
//! editor through luau-lsp (`--definitions @horizon=horizon.d.lua`); the
//! modkit ships a copy as `types/horizon.d.lua`. `--check` fails when the
//! file is not what the tables here say, and tests keep the tables equal to
//! the runtime: every hook `horizon.on` takes has a handler type, every
//! function the runtime installs has a declaration.
use std::{fs, path::Path};

/// The tables a hook or helper hands a script, as Luau types.
const TYPES: &[(&str, &str)] = &[
    ("Vec3", "{ x: number, y: number, z: number }"),
    (
        "Item",
        "{ id: string, count: number, damage: number, plain: boolean }",
    ),
    ("Held", "{ item: string, count: number, plain: boolean }"),
    (
        "Player",
        "{ uuid: string, name: string, dimension: string, position: Vec3, yaw: number, pitch: number, game_mode: number, health: number, held: Held? }",
    ),
    ("Named", "{ uuid: string, name: string }"),
    ("IdKind", "{ id: number, kind: string }"),
    ("Target", "{ id: number, kind: string, position: Vec3 }"),
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
        "{ navigate_to: (x: number, y: number, z: number, speed: number) -> (), navigate_to_target: (speed: number) -> (), stop_navigation: () -> (), move_toward: (x: number, y: number, z: number, speed: number) -> (), look_at: (x: number, y: number, z: number) -> (), look_at_target: () -> (), set_target: (who: Who) -> (), jump: () -> (), attack: (who: Who) -> (), swing: () -> (), hurt: (who: Who, amount: number) -> (), convert: (kind: string) -> (), summon: (kind: string, x: number, y: number, z: number) -> (), despawn: () -> (), heal: (amount: number) -> (), effect: (name: string, ticks: number, amplifier: number) -> () }",
    ),
    ("GoalState", "{ [string]: any }"),
    (
        "GoalDef",
        "{ flags: { string }?, every_tick: boolean?, interruptable: boolean?, can_use: (mob: MobView, state: GoalState, out: GoalOut) -> boolean, can_continue: ((mob: MobView, state: GoalState, out: GoalOut) -> boolean)?, start: ((mob: MobView, state: GoalState, out: GoalOut) -> ())?, tick: ((mob: MobView, state: GoalState, out: GoalOut) -> ())?, stop: ((mob: MobView, state: GoalState, out: GoalOut) -> ())? }",
    ),
    ("Data", "{ [string]: any }"),
    ("Props", "{ [string]: string }"),
    (
        "DeathCause",
        "{ damage_type: string, causing: number, player: Player? }",
    ),
    (
        "Hit",
        "{ x: number, y: number, z: number, entity: number?, entity_kind: string?, block: Block?, face: Vec3? }",
    ),
    (
        "LootSource",
        "{ table: string, dimension: string, origin: Vec3, block: LootBlock?, mob: IdKind?, player: Player?, tool: Item?, explosion: number? }",
    ),
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
];

/// Every hook a script registers for (`horizon.on(name, fn[, kind])`) and
/// its handler's type; the runtime's `LUA_HOOK_NAMES` names the same hooks.
const HOOKS: &[(&str, &str)] = &[
    (
        "server_tick",
        "(phase: string, world_running: boolean) -> ()",
    ),
    ("world_lifecycle", "(phase: string) -> ()"),
    (
        "mob_spawned",
        "(dimension: string, kind: string, x: number, y: number, z: number, natural: boolean) -> ()",
    ),
    ("mob_ai_register", "(mob: MobView) -> ()"),
    (
        "random_tick",
        "(dimension: string, block: string, x: number, y: number, z: number, props: Props, data: Data?) -> ()",
    ),
    (
        "tick_block",
        "(dimension: string, block: string, x: number, y: number, z: number, props: Props, data: Data?) -> ()",
    ),
    (
        "right_click_block",
        "(x: number, y: number, z: number, block: string, face: Vec3, player: Player, held: Held?, data: Data?) -> ()",
    ),
    (
        "block_entity_tick",
        "(dimension: string, block: string, x: number, y: number, z: number, data: Data) -> ()",
    ),
    (
        "mob_hurt",
        "(mob: IdKind, damage_type: string, causing: number, direct: number, amount: number) -> ()",
    ),
    (
        "player_hurt",
        "(player: IdKind, damage_type: string, causing: number, direct: number, amount: number) -> ()",
    ),
    (
        "mob_killed",
        "(dimension: string, mob: IdKind, killer: Player, data: Data?) -> ()",
    ),
    (
        "mob_interact",
        "(dimension: string, mob: MobView, player: Player, held: Held?, data: Data?) -> ()",
    ),
    (
        "mob_tick",
        "(dimension: string, mob: MobView, data: Data?) -> ()",
    ),
    (
        "mob_died",
        "(dimension: string, mob: IdKind, cause: DeathCause, data: Data?) -> ()",
    ),
    (
        "mob_fall",
        "(mob: IdKind, distance: number, multiplier: number) -> ()",
    ),
    (
        "player_fall",
        "(player: IdKind, distance: number, multiplier: number) -> ()",
    ),
    ("mob_heal", "(mob: { id: number }, amount: number) -> ()"),
    (
        "mob_knockback",
        "(mob: IdKind, damage_type: string, causing: number, direct: number, strength: number, x: number, z: number) -> ()",
    ),
    (
        "player_knockback",
        "(player: IdKind, damage_type: string, causing: number, direct: number, strength: number, x: number, z: number) -> ()",
    ),
    (
        "mob_equipped",
        "(mob: MobView, slot: string, from: Item?, to: Item?) -> ()",
    ),
    (
        "entity_joined",
        "(dimension: string, id: number, kind: string, x: number, y: number, z: number, loaded: boolean) -> ()",
    ),
    ("item_use", "(item: Item, player: Player) -> ()"),
    (
        "item_use_on",
        "(item: Item, player: Player, block: Block, face: Vec3) -> ()",
    ),
    (
        "item_hit",
        "(item: Item, player: Player, target: Target) -> ()",
    ),
    (
        "item_hurt_enemy",
        "(item: Item, player: Player, target: IdKind) -> ()",
    ),
    (
        "item_use_start",
        "(item: Item, player: Player, hand: string, duration: number) -> ()",
    ),
    ("item_finished", "(item: Item, player: Player) -> ()"),
    (
        "item_tick",
        "(item: Item, player: Player, slot: number, selected: boolean) -> ()",
    ),
    (
        "item_picked_up",
        "(player: Player, item: string, count: number) -> ()",
    ),
    (
        "item_crafted",
        "(player: Player, recipe: string, item: Item) -> ()",
    ),
    (
        "item_dropped",
        "(player: Player, item: string, count: number) -> ()",
    ),
    ("loot_drops", "(source: LootSource, drops: { Item }) -> ()"),
    (
        "projectile_impact",
        "(projectile: number, kind: string, owner: number, hit: Hit) -> ()",
    ),
    (
        "projectile_launched",
        "(projectile: number, kind: string, player: Player?) -> ()",
    ),
    (
        "explosion",
        "(dimension: string, center: Vec3, blocks: { Vec3 }) -> ()",
    ),
    ("player_joined", "(player: Player) -> ()"),
    ("player_left", "(player: Player) -> ()"),
    ("player_respawned", "(player: Player) -> ()"),
    ("player_died", "(player: Player, cause: string?) -> ()"),
    ("player_tick", "(player: Player) -> ()"),
    ("player_chat", "(player: Named, line: string) -> ()"),
    ("player_command", "(player: Player, line: string) -> ()"),
    (
        "player_changed_dimension",
        "(player: Player, from: string, to: string) -> ()",
    ),
    (
        "player_bed_enter",
        "(player: Player, x: number, y: number, z: number) -> ()",
    ),
    (
        "player_exp_change",
        "(player: Player, amount: number) -> ()",
    ),
    (
        "player_container_open",
        "(player: Player, menu: string, pos: Vec3?) -> ()",
    ),
    (
        "block_broken",
        "(x: number, y: number, z: number, block: string, player: Player) -> ()",
    ),
    (
        "block_placed",
        "(x: number, y: number, z: number, block: string, player: Player, item: string) -> ()",
    ),
];

/// Every function and field of `horizon`, with its type; the runtime
/// installs the same names (the `api.set` calls, the reads, the timers and
/// the actions of `lua_mods.rs`). `on` is written out of `HOOKS`.
const FUNCTIONS: &[(&str, &str)] = &[
    ("mod_id", "string"),
    ("mod_version", "string"),
    ("libraries", "{ [string]: string }"),
    ("log", "(msg: string) -> ()"),
    (
        "command",
        "(usage: string, level: number, fn: (player: Player, args: CommandArgs) -> ()) -> ()",
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
    ("players", "() -> { Player }"),
    ("player", "(who: string) -> Player?"),
    ("time", "() -> Time"),
    ("raycast", "(player: string, distance: number?) -> Raycast?"),
    ("world_data", "() -> Data"),
    ("player_data", "(who: string) -> Data?"),
    ("light", "(x: number, y: number, z: number) -> Light"),
    (
        "surface_height",
        "(x: number, z: number, heightmap: string?) -> number?",
    ),
    ("inventory", "(who: string | Player) -> Inventory?"),
    (
        "count_items",
        "(who: string | Player, id: string) -> number?",
    ),
    ("carried_items", "(who: string | Player) -> Carried?"),
    ("data", "(path: string) -> any"),
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
    ("set_amount", "(n: number) -> ()"),
    (
        "take_items",
        "((id: string, count: number) -> ()) & ((who: string | Player, id: string, count: number) -> ())",
    ),
    (
        "give_items",
        "((id: string, count: number) -> ()) & ((who: string | Player, id: string, count: number) -> ())",
    ),
    ("set_result", "(id: string?, count: number?) -> ()"),
    ("set_count", "(n: number) -> ()"),
    ("feedback", "(text: string) -> ()"),
    ("tell", "(who: string | Player, text: string) -> ()"),
    ("action_bar", "(who: string | Player, text: string) -> ()"),
    ("title", "(who: string | Player, text: string) -> ()"),
    ("subtitle", "(who: string | Player, text: string) -> ()"),
    (
        "title_times",
        "(who: string | Player, fade_in: number, stay: number, fade_out: number) -> ()",
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
        "(who: string | Player, points: number) -> ()",
    ),
    (
        "give_levels",
        "(who: string | Player, levels: number) -> ()",
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
        "(who: string | Player, name: string?) -> ()",
    ),
    (
        "effect",
        "(target: number | string | Player, name: string, ticks: number, amplifier: number?) -> ()",
    ),
    ("set_distance", "(n: number) -> ()"),
    ("set_multiplier", "(n: number) -> ()"),
    ("set_strength", "(n: number) -> ()"),
    ("set_duration", "(ticks: number) -> ()"),
    ("reply", "(text: string) -> ()"),
    ("set_line", "(text: string) -> ()"),
];

/// The `registry` global of `scripts/registry.lua`.
const REGISTRY: &[(&str, &str)] = &[
    ("id", "string"),
    ("entity_type", "(name: string, def: Data) -> ()"),
    ("block", "(name: string, def: Data) -> ()"),
    ("item", "(name: string, def: Data) -> ()"),
    ("data", "(path: string, value: Data) -> ()"),
    ("asset", "(path: string, value: Data) -> ()"),
];

fn render() -> String {
    let mut out = String::new();
    out.push_str("-- Generated by `cargo run -p xtask -- lua-types` from xtask/src/lua_types.rs: edit that, then regenerate.\n");
    out.push_str("-- Luau type definitions for a Horizon scripted mod (docs/lua-bindings.md), for luau-lsp:\n");
    out.push_str("--   luau-lsp analyze --definitions @horizon=horizon.d.lua --platform=standard scripts\n\n");
    for (name, def) in TYPES {
        out.push_str(&format!("export type {name} = {def}\n"));
    }
    out.push_str("\n-- `horizon.on(name, fn[, kind])`: one overload per hook, with its handler's signature.\n");
    out.push_str("export type HorizonOn =\n");
    for (i, (hook, handler)) in HOOKS.iter().enumerate() {
        let sep = if i + 1 == HOOKS.len() { "" } else { " &" };
        out.push_str(&format!(
            "    ((name: \"{hook}\", fn: {handler}, kind: string?) -> ()){sep}\n"
        ));
    }
    out.push_str("\ndeclare horizon: {\n    on: HorizonOn,\n");
    for (name, ty) in FUNCTIONS {
        out.push_str(&format!("    {name}: {ty},\n"));
    }
    out.push_str("}\n\n-- `scripts/registry.lua` runs with this instead of `horizon`.\ndeclare registry: {\n");
    for (name, ty) in REGISTRY {
        out.push_str(&format!("    {name}: {ty},\n"));
    }
    out.push_str("}\n");
    out
}

pub fn run(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let path = root.join("docs/api/horizon.d.lua");
    let out = render();
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
        std::fs::read_to_string(root.join("crates/horizon-client/src/lua_mods.rs")).unwrap()
    }

    /// Every hook `horizon.on` takes (the runtime's `LUA_HOOK_NAMES`, less
    /// `command`, which is `horizon.command`'s) has a handler type, and no
    /// other hook has one.
    #[test]
    fn every_hook_the_runtime_takes_has_a_handler_type() {
        let src = runtime_source();
        let start = src.find("pub const LUA_HOOK_NAMES").unwrap();
        let end = start + src[start..].find("];").unwrap();
        let mut runtime: BTreeSet<String> = src[start..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        runtime.remove("command");
        let typed: BTreeSet<String> = super::HOOKS.iter().map(|(h, _)| h.to_string()).collect();
        assert_eq!(runtime, typed);
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
            if let Some(rest) = line.strip_prefix("horizon.")
                && let Some(name) = rest.split(" = function").next()
                && rest.contains(" = function")
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                installed.insert(name.to_owned());
            }
        }
        let actions_start = src.find("for action in [").unwrap();
        let actions_end = actions_start + src[actions_start..].find("] {").unwrap();
        for name in src[actions_start..actions_end]
            .split('"')
            .skip(1)
            .step_by(2)
        {
            installed.insert(name.to_owned());
        }
        let declared: BTreeSet<String> = super::FUNCTIONS
            .iter()
            .map(|(n, _)| n.to_string())
            .chain(std::iter::once("on".to_owned()))
            .collect();
        assert_eq!(installed, declared);
    }

    #[test]
    fn the_definitions_are_what_the_tables_say() {
        super::run(true).unwrap();
    }
}
