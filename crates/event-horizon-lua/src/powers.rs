//! The powers a Lua mod may be granted, and the check that gates the host calls which need one.
//!
//! Rules (the approved design, `/work/eh-reports/78-powers-design.md`):
//! - Fail closed. A mod holds a power only when its manifest asks for it (`powers`) AND the
//!   enabled-state file grants it (`[grants]` in `lua-mods/enabled.toml`, see `enabled.rs`).
//! - Effective = requested ∩ granted, computed once at scan (`discover::scan_on`) and applied at load
//!   (`LuaMod::set_powers`). A grant changes on the next load, never inside a running turn. A disabled,
//!   skipped or refused mod holds nothing. `network` and `files` are never effective: they have no
//!   meaning in this version.
//! - A gated host call runs only when the calling mod's own effective set holds its power (each turn's
//!   `Void` carries that set, see `hostcall.rs`). Otherwise the call is refused as a plain Lua error,
//!   which `pcall` can catch: the turn is not voided, the refusal costs one budget unit, the host is
//!   not called, and the warning is written once per mod and power, never with the arguments.
//! - The gated table is generated (`generated/powers.rs`, from `GATED_WRITES` in the generator): the
//!   `world` and `spawn` writes (the item spawns among them, capped in [`guard`]), then `explosions`
//!   (`explode`, `explosion_destroy_blocks`, with the power cap in [`guard`]), `game_rules`
//!   (`set_game_rule`), `inventory` (the player's and the living's item writes), `commands`
//!   (`EntityRef::run_command`, checked by `command_guard.rs`) and `players` (`award_recipes`,
//!   `reset_recipes`, `send_map`, checked in [`guard`]). Every other write stays denied.
//! - Declarations, `event.feedback`, the `horizon.attachment_*` calls and settings are NOT gated, and stay
//!   that way.

use mlua::{Error, MultiValue, Value};

use crate::command_guard::CommandPolicy;
use crate::host::warn_problem;
use crate::manifest::Power;

/// The gated host operations: `("Family::method", power)`, generated from the same members as the
/// handle methods (`generated/powers.rs`).
pub(crate) fn table() -> &'static [(&'static str, Power)] {
    crate::generated::powers::GATED
}

/// The translation key of a power's label; its description is the same key with `.description`
/// appended, and `label_key` + `.label` is its label. (The Mods screen reads both.)
impl Power {
    pub fn label_key(self) -> &'static str {
        match self {
            Power::Commands => "horizon.mods.power.commands",
            Power::Nbt => "horizon.mods.power.nbt",
            Power::Spawn => "horizon.mods.power.spawn",
            Power::Registration => "horizon.mods.power.registration",
            Power::Explosions => "horizon.mods.power.explosions",
            Power::World => "horizon.mods.power.world",
            Power::GameRules => "horizon.mods.power.game_rules",
            Power::Transactions => "horizon.mods.power.transactions",
            Power::Inventory => "horizon.mods.power.inventory",
            Power::Players => "horizon.mods.power.players",
            Power::Network => "horizon.mods.power.network",
            Power::Files => "horizon.mods.power.files",
        }
    }
}

/// The translation key of the refusal text the Mods screen shows for a refused power.
pub const REFUSED_KEY: &str = "horizon.mods.power.refused";

/// Whether a power is reserved: it may be requested and granted, and it is never effective.
pub fn is_reserved(power: Power) -> bool {
    matches!(power, Power::Network | Power::Files)
}

/// Whether this build has at least one gated write behind `power`, so granting it does something.
/// A power without one (every power but `world` and `spawn`, for now) can be granted but not used.
pub fn has_ops(power: Power) -> bool {
    table().iter().any(|(_, p)| *p == power)
}

/// The effective powers: those requested by the manifest and granted by the file, without the reserved
/// ones, in `Power::rank` order without repeats. Empty when either list is empty.
pub fn effective(requested: &[Power], granted: &[Power]) -> Vec<Power> {
    let mut out: Vec<Power> = requested
        .iter()
        .copied()
        .filter(|p| granted.contains(p) && !is_reserved(*p))
        .collect();
    out.sort_by_key(|p| p.rank());
    out.dedup();
    out
}

/// The power a host operation needs, or `None` when it is not gated. `op` is the name the generated
/// handles pass (`Family::method`).
pub fn required(op: &str) -> Option<Power> {
    #[cfg(test)]
    {
        let overridden = TEST_TABLE.with(|table| {
            table.borrow().as_ref().map(|entries| {
                entries
                    .iter()
                    .find(|(name, _)| *name == op)
                    .map(|(_, p)| *p)
            })
        });
        if let Some(found) = overridden {
            return found;
        }
    }
    table()
        .iter()
        .find(|(name, _)| *name == op)
        .map(|(_, power)| *power)
}

/// The most item and mob spawns one hook turn may cause by default, over `spawn_mob`, `spawn_item`,
/// `spawn_item_with_motion` and `pop_resource`: the 65th is refused before the host sees it. A placeholder:
/// the balance number is a maintainer decision, and the owner may lower or raise it (`[limits]`).
pub const DEFAULT_SPAWNS_PER_TURN: u32 = 64;

/// The most item and mob spawns the owner may set in a turn (`spawns_per_turn`); a hard ceiling in code.
pub const SPAWNS_PER_TURN_CEILING: u32 = 1024;

/// The spawn writes the cap counts (`Family::method`); each is gated under `spawn`.
const SPAWN_OPS: &[&str] = &[
    "LevelRef::spawn_mob",
    "LevelRef::spawn_item",
    "LevelRef::spawn_item_with_motion",
    "LevelRef::pop_resource",
];

/// The most map pictures one hook turn may send by default, over `send_map`: the ninth is refused before the
/// host sees it. A placeholder like [`DEFAULT_SPAWNS_PER_TURN`]; the owner may change it (`[limits] maps_per_turn`).
pub const DEFAULT_MAPS_PER_TURN: u32 = 8;

/// The most map pictures per turn an owner may set (`maps_per_turn`); a hard ceiling in code.
pub const MAPS_PER_TURN_CEILING: u32 = 64;

/// The bytes of one map picture (`send_map`): 128 x 128 colours, a longer or shorter one is refused.
pub const MAP_BYTES: usize = 16 * 1024;

/// The most recipe ids one `award_recipes` or `reset_recipes` call may name.
pub const RECIPE_IDS_PER_CALL: usize = 64;

/// The most characters one recipe id may have.
pub const RECIPE_ID_CHARS: usize = 128;

/// What one hook turn has done so far, counted by [`guard`]: the explosions, the spawns and the map pictures.
/// One value per turn, held by the turn's scope; a refused call is not counted.
#[derive(Debug, Default)]
pub(crate) struct TurnCounts {
    pub explosions: u32,
    pub spawns: u32,
    pub maps: u32,
}

/// The default cap on an explosion's power (the `power` argument of the explosion writes, a Lua number): a
/// call above it is refused before the host sees it. A placeholder like [`DEFAULT_SPAWNS_PER_TURN`]; the
/// owner may change it (`[limits] explosion_power`). A refused call costs one unit, as a power refusal does.
pub const DEFAULT_EXPLOSION_POWER: f64 = 16.0;

/// The most an owner may set the explosion power cap to (`explosion_power`); a hard ceiling in code.
pub const EXPLOSION_POWER_CEILING: f64 = 64.0;

/// The default most explosions one hook turn may cause, over both explosion writes: the fifth is refused
/// before the host sees it. A placeholder; the owner may change it (`[limits] explosions_per_turn`).
pub const DEFAULT_EXPLOSIONS_PER_TURN: u32 = 4;

/// The most explosions per turn an owner may set (`explosions_per_turn`); a hard ceiling in code.
pub const EXPLOSIONS_PER_TURN_CEILING: u32 = 64;

/// The owner's limits on the guard, read from `[limits]` in `enabled.toml` (see `enabled.rs`). A mod never
/// reads or changes them, and an owner's value above a ceiling, or an invalid one, is the default instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// The most power one explosion may have.
    pub explosion_power: f64,
    /// The most explosions one hook turn may cause.
    pub explosions_per_turn: u32,
    /// The most item and mob spawns one hook turn may cause.
    pub spawns_per_turn: u32,
    /// The most map pictures one hook turn may send.
    pub maps_per_turn: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            explosion_power: DEFAULT_EXPLOSION_POWER,
            explosions_per_turn: DEFAULT_EXPLOSIONS_PER_TURN,
            spawns_per_turn: DEFAULT_SPAWNS_PER_TURN,
            maps_per_turn: DEFAULT_MAPS_PER_TURN,
        }
    }
}

/// The explosion writes the cap checks, `(op, index of the power among the Lua arguments)`, counted
/// after the handle. Checked against the surface: `explode(entity, position, power, ...)` and
/// `explosion_destroy_blocks(positions, entity, position, power, ...)` (surface.json).
const EXPLOSION_POWER_ARGS: &[(&str, usize)] = &[
    ("LevelRef::explode", 2),
    ("LevelRef::explosion_destroy_blocks", 3),
];

/// The guard of a gated call, run after its power check passed and before the host is called. It
/// refuses with a plain Lua error when a rule of the guard says no, against the owner's `limits`. The rule
/// for an explosion: its power must be a Lua number (an integer or a float), finite, and at most
/// `limits.explosion_power`. A missing argument, a string (even one that looks like a number), NaN,
/// infinity or a larger power is refused before the host sees the call. The explosions of a turn are
/// capped at `limits.explosions_per_turn`, the spawns at `limits.spawns_per_turn`. `EntityRef::run_command`
/// is checked against the mod's `commands` (`command_guard::check`: the level cap, the allow list, the
/// never list). Any other call passes.
pub(crate) fn guard(
    op: &str,
    args: &MultiValue,
    counts: &mut TurnCounts,
    limits: &Limits,
    commands: &CommandPolicy,
) -> Result<(), Error> {
    if op == "EntityRef::run_command" {
        return crate::command_guard::check(args, commands);
    }
    let method = op.rsplit("::").next().unwrap_or(op);
    if RECIPE_OPS.contains(&op) {
        return check_recipe_ids(method, args);
    }
    if op == SEND_MAP_OP {
        check_map_bytes(method, args)?;
        if counts.maps >= limits.maps_per_turn {
            return Err(refusal(
                method,
                &format!("at most {} maps per turn", limits.maps_per_turn),
            ));
        }
        counts.maps += 1;
        return Ok(());
    }
    if SPAWN_OPS.contains(&op) {
        if counts.spawns >= limits.spawns_per_turn {
            return Err(Error::runtime(format!(
                "horizon: {method} refused: at most {} spawns per turn",
                limits.spawns_per_turn
            )));
        }
        counts.spawns += 1;
        return Ok(());
    }
    let Some((_, index)) = EXPLOSION_POWER_ARGS.iter().find(|(name, _)| *name == op) else {
        return Ok(());
    };
    let power = match args.get(*index) {
        Some(Value::Integer(value)) => Some(*value as f64),
        Some(Value::Number(value)) => Some(*value),
        _ => None,
    };
    match power {
        Some(power) if power.is_finite() && (0.0..=limits.explosion_power).contains(&power) => {}
        _ => {
            return Err(Error::runtime(format!(
                "horizon: {method} refused: power must be a number from 0 to {}",
                limits.explosion_power
            )));
        }
    }
    counts.explosions += 1;
    if counts.explosions > limits.explosions_per_turn {
        return Err(Error::runtime(format!(
            "horizon: {method} refused: at most {} explosions per turn",
            limits.explosions_per_turn
        )));
    }
    Ok(())
}

/// The recipe writes whose one argument is the list of recipe ids (`&[&str]`).
const RECIPE_OPS: &[&str] = &["PlayerRef::award_recipes", "PlayerRef::reset_recipes"];

/// The map write: its first argument is the map id, its second the picture's bytes.
const SEND_MAP_OP: &str = "PlayerRef::send_map";

/// A refusal of a gated call, naming the method and the problem, never the arguments.
fn refusal(method: &str, reason: &str) -> Error {
    Error::runtime(format!("horizon: {method} refused: {reason}"))
}

/// The recipe ids of `award_recipes` and `reset_recipes`: a list of at most [`RECIPE_IDS_PER_CALL`] strings,
/// each non-empty, at most [`RECIPE_ID_CHARS`] characters and free of control characters. A missing or
/// non-table argument passes here; the conversion refuses it.
fn check_recipe_ids(method: &str, args: &MultiValue) -> Result<(), Error> {
    let Some(Value::Table(table)) = args.front() else {
        return Ok(());
    };
    let mut count = 0usize;
    for pair in table.pairs::<Value, Value>() {
        let (_, item) = pair.map_err(|_| refusal(method, "the recipe list cannot be read"))?;
        count += 1;
        if count > RECIPE_IDS_PER_CALL {
            return Err(refusal(
                method,
                &format!("at most {RECIPE_IDS_PER_CALL} recipe ids per call"),
            ));
        }
        let Value::String(text) = item else {
            return Err(refusal(method, "each recipe id must be a string"));
        };
        let Ok(text) = text.to_str() else {
            return Err(refusal(method, "a recipe id must be UTF-8 text"));
        };
        if text.is_empty() {
            return Err(refusal(method, "a recipe id is empty"));
        }
        if text.chars().count() > RECIPE_ID_CHARS {
            return Err(refusal(
                method,
                &format!("a recipe id is longer than {RECIPE_ID_CHARS} characters"),
            ));
        }
        if text.chars().any(char::is_control) {
            return Err(refusal(method, "a recipe id has a control character"));
        }
    }
    Ok(())
}

/// The picture of `send_map`: a Lua string or an array table of integers 0 to 255, at most [`MAP_BYTES`]
/// long. A string of any other length, or a table entry outside 0 to 255, is refused. A missing or other
/// argument passes here; the conversion refuses it.
fn check_map_bytes(method: &str, args: &MultiValue) -> Result<(), Error> {
    match args.get(1) {
        Some(Value::String(text)) if text.as_bytes().len() > MAP_BYTES => Err(refusal(
            method,
            &format!("a map is at most {MAP_BYTES} bytes"),
        )),
        Some(Value::Table(table)) => {
            let mut count = 0usize;
            for pair in table.pairs::<Value, Value>() {
                let (_, item) = pair.map_err(|_| refusal(method, "the map cannot be read"))?;
                count += 1;
                if count > MAP_BYTES {
                    return Err(refusal(
                        method,
                        &format!("a map is at most {MAP_BYTES} bytes"),
                    ));
                }
                let whole = match item {
                    Value::Integer(value) => (0..=255).contains(&value),
                    Value::Number(value) => value.fract() == 0.0 && (0.0..=255.0).contains(&value),
                    _ => false,
                };
                if !whole {
                    return Err(refusal(
                        method,
                        "a map byte must be a whole number from 0 to 255",
                    ));
                }
            }
            if count != table.raw_len() {
                return Err(refusal(method, "a map must be an array of bytes"));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The refusal of a gated call of mod `owner` that needs `power` and does not hold it: the one warning
/// for this mod and power, and the Lua error the call raises. The error names the method and the power,
/// never the arguments.
pub(crate) fn refuse(op: &str, owner: &str, power: Power) -> Error {
    let method = op.rsplit("::").next().unwrap_or(op);
    let name = power.name();
    warn_problem(
        owner,
        &format!(
            "{owner}: a host call needs the \"{name}\" power, which is not in effect (the mod did not request it, or the owner has not granted it); refused"
        ),
    );
    Error::runtime(format!(
        "horizon: {method} needs the \"{name}\" power; it is not in effect for {owner} (the mod must request it and the owner must grant it)"
    ))
}

#[cfg(test)]
thread_local! {
    /// A test's own gated table, on its thread; unset in production.
    static TEST_TABLE: std::cell::RefCell<Option<Vec<(&'static str, Power)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Replaces the gated table on this thread for the life of the guard.
#[cfg(test)]
pub(crate) fn gate_for_test(entries: Vec<(&'static str, Power)>) -> TestGate {
    TEST_TABLE.with(|table| *table.borrow_mut() = Some(entries));
    TestGate
}

/// Restores the production table (empty) when dropped.
#[cfg(test)]
pub(crate) struct TestGate;

#[cfg(test)]
impl Drop for TestGate {
    fn drop(&mut self) {
        TEST_TABLE.with(|table| *table.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Power; 12] = [
        Power::Commands,
        Power::Nbt,
        Power::Spawn,
        Power::Registration,
        Power::Explosions,
        Power::World,
        Power::GameRules,
        Power::Transactions,
        Power::Inventory,
        Power::Players,
        Power::Network,
        Power::Files,
    ];

    /// The gated writes, written out by hand (not read from the generator): each op and its power.
    const GATED: [(&str, Power); 42] = [
        ("BossBarRef::remove", Power::World),
        ("BossBarRef::remove_player", Power::World),
        ("EntityRef::discard", Power::World),
        ("EntityRef::remove_tag", Power::World),
        ("EntityRef::teleport_to_level", Power::World),
        ("LevelRef::block_random_tick", Power::World),
        ("LevelRef::invalidate_capabilities", Power::World),
        ("LevelRef::remove_block", Power::World),
        ("LevelRef::request_save", Power::World),
        ("LevelRef::set_clock_time", Power::World),
        ("LevelRef::set_weather", Power::World),
        ("LevelRef::set_weather_levels", Power::World),
        ("LevelRef::set_world_border_center", Power::World),
        ("LevelRef::set_world_border_damage", Power::World),
        ("LevelRef::set_world_border_size", Power::World),
        ("LevelRef::set_world_border_warning", Power::World),
        ("LevelRef::strike_lightning", Power::World),
        ("LivingRef::random_teleport", Power::World),
        ("MobRef::remove_goal", Power::World),
        ("MobRef::remove_target_goal", Power::World),
        ("LevelRef::award_experience", Power::Spawn),
        ("LevelRef::set_respawn_data", Power::Spawn),
        ("LevelRef::spawn_mob", Power::Spawn),
        ("LevelRef::spawn_item", Power::Spawn),
        ("LevelRef::spawn_item_with_motion", Power::Spawn),
        ("LevelRef::pop_resource", Power::Spawn),
        ("PlayerRef::respawn", Power::Spawn),
        ("LevelRef::explode", Power::Explosions),
        ("LevelRef::explosion_destroy_blocks", Power::Explosions),
        ("LevelRef::set_game_rule", Power::GameRules),
        ("PlayerRef::give_stack", Power::Inventory),
        ("PlayerRef::set_inventory_slot", Power::Inventory),
        ("PlayerRef::set_ender_chest_slot", Power::Inventory),
        ("LivingRef::set_hand_item", Power::Inventory),
        ("PlayerRef::drop_stack", Power::Inventory),
        ("LivingRef::set_equipment", Power::Inventory),
        ("EntityRef::run_command", Power::Commands),
        ("EntityRef::merge_nbt", Power::Nbt),
        ("BlockEntityRef::merge_data", Power::Nbt),
        ("PlayerRef::award_recipes", Power::Players),
        ("PlayerRef::reset_recipes", Power::Players),
        ("PlayerRef::send_map", Power::Players),
    ];

    #[test]
    fn the_generated_table_gates_exactly_the_gated_writes() {
        assert_eq!(table().len(), GATED.len());
        for (op, power) in GATED {
            assert_eq!(required(op), Some(power), "{op}");
        }
        for op in [
            "EntityRef::set_glowing",
            "PlayerRef::disconnect",
            "EntityRef::remove",
            "PlayerRef::set_nickname",
            "LevelRef::add_water_ticket",
            "LevelRef::destroy_block",
            "LevelRef::block_drops",
            "LevelRef::roll_loot",
            "LivingRef::hurt_and_break",
            "EntityRef::run_command_with_output",
        ] {
            assert_eq!(required(op), None, "{op} stays denied");
        }
    }

    /// Each row names a write the generated handles call under the same op string, and the generated
    /// dispatcher binds it (`cost` is its table): the gate and the call cannot name different ops.
    #[test]
    fn every_gated_row_is_a_bound_write_that_calls_its_op() {
        let text = include_str!("generated/handles.rs");
        for &(op, _) in table() {
            assert_eq!(
                crate::generated::handles::cost(op),
                Some(10),
                "{op} is bound as a write"
            );
            assert!(
                text.contains(&format!(
                    "crate::hostcall::host_call(lua, \"{op}\", this, args)"
                )),
                "{op} is called by its method"
            );
        }
    }

    #[test]
    fn effective_is_requested_and_granted_minus_the_reserved_powers() {
        assert_eq!(
            effective(&[Power::World, Power::Spawn], &[Power::Spawn]),
            [Power::Spawn]
        );
        assert_eq!(effective(&[Power::World], &[]), []);
        assert_eq!(effective(&[], &[Power::World]), []);
        assert_eq!(
            effective(&[Power::Files, Power::Network], &ALL),
            [],
            "reserved powers are never effective"
        );
        assert_eq!(
            effective(&[Power::Spawn, Power::World], &[Power::World, Power::Spawn]),
            [Power::Spawn, Power::World],
            "in rank order, not request order"
        );
    }

    /// The explosion guard passes a power that is a finite number from 0 to the cap, and refuses every
    /// other power with one plain message: too large, negative, NaN, infinite, a string, or missing.
    #[test]
    fn an_explosion_power_must_be_a_finite_number_from_zero_to_the_cap() {
        let lua = mlua::Lua::new();
        let args = |power: Value| MultiValue::from_vec(vec![Value::Nil, Value::Nil, power]);
        let message =
            "runtime error: horizon: explode refused: power must be a number from 0 to 16";
        let ok = |power: Value| {
            guard(
                "LevelRef::explode",
                &args(power),
                &mut TurnCounts::default(),
                &Limits::default(),
                &CommandPolicy::default(),
            )
        };
        assert!(ok(Value::Number(16.0)).is_ok());
        assert!(ok(Value::Integer(16)).is_ok());
        assert!(ok(Value::Integer(3)).is_ok());
        assert!(ok(Value::Number(0.0)).is_ok());
        for refused in [
            Value::Number(17.0),
            Value::Number(-0.5),
            Value::Integer(-1),
            Value::Number(f64::NAN),
            Value::Number(f64::INFINITY),
            Value::Number(f64::NEG_INFINITY),
            Value::Integer(17),
            Value::String(lua.create_string("999").expect("a Lua string")),
            Value::Nil,
            Value::Boolean(true),
        ] {
            let refusal = ok(refused.clone()).expect_err("a refused power");
            assert_eq!(refusal.to_string(), message, "{refused:?}");
        }
        // The power of explosion_destroy_blocks is its fourth argument.
        let blocks =
            |power: Value| MultiValue::from_vec(vec![Value::Nil, Value::Nil, Value::Nil, power]);
        assert!(
            guard(
                "LevelRef::explosion_destroy_blocks",
                &blocks(Value::Number(16.0)),
                &mut TurnCounts::default(),
                &Limits::default(),
                &CommandPolicy::default()
            )
            .is_ok()
        );
        let refusal = guard(
            "LevelRef::explosion_destroy_blocks",
            &blocks(Value::Number(17.5)),
            &mut TurnCounts::default(),
            &Limits::default(),
            &CommandPolicy::default(),
        )
        .expect_err("power 17.5 is above the cap");
        assert!(
            refusal.to_string().contains(
                "horizon: explosion_destroy_blocks refused: power must be a number from 0 to 16"
            ),
            "{refusal}"
        );
        // Any other op passes, whatever its numbers.
        assert!(
            guard(
                "LevelRef::strike_lightning",
                &args(Value::Number(99.0)),
                &mut TurnCounts::default(),
                &Limits::default(),
                &CommandPolicy::default()
            )
            .is_ok()
        );
    }

    /// One turn may cause at most `DEFAULT_EXPLOSIONS_PER_TURN` explosions over both writes: the next is
    /// refused, and a refused power does not use one up.
    #[test]
    fn a_turn_may_cause_only_so_many_explosions() {
        let args = |power: Value| MultiValue::from_vec(vec![Value::Nil, Value::Nil, power]);
        let mut count = TurnCounts::default();
        for _ in 0..DEFAULT_EXPLOSIONS_PER_TURN {
            assert!(
                guard(
                    "LevelRef::explode",
                    &args(Value::Number(4.0)),
                    &mut count,
                    &Limits::default(),
                    &CommandPolicy::default()
                )
                .is_ok()
            );
        }
        let refusal = guard(
            "LevelRef::explode",
            &args(Value::Number(4.0)),
            &mut count,
            &Limits::default(),
            &CommandPolicy::default(),
        )
        .expect_err("the fifth explosion of a turn");
        assert_eq!(
            refusal.to_string(),
            "runtime error: horizon: explode refused: at most 4 explosions per turn"
        );
        let mut count = TurnCounts::default();
        assert!(
            guard(
                "LevelRef::explode",
                &args(Value::Number(99.0)),
                &mut count,
                &Limits::default(),
                &CommandPolicy::default()
            )
            .is_err()
        );
        assert_eq!(count.explosions, 0, "a refused power is not counted");
    }

    /// One turn may spawn at most `DEFAULT_SPAWNS_PER_TURN` times over the four spawn writes: the next is refused
    /// for any of them, and a refused spawn is not counted. Explosions are counted apart.
    #[test]
    fn a_turn_may_spawn_only_so_many_times() {
        let args = MultiValue::new();
        let mut counts = TurnCounts::default();
        for i in 0..DEFAULT_SPAWNS_PER_TURN {
            let op = SPAWN_OPS[i as usize % SPAWN_OPS.len()];
            assert!(
                guard(
                    op,
                    &args,
                    &mut counts,
                    &Limits::default(),
                    &CommandPolicy::default()
                )
                .is_ok(),
                "spawn {i}"
            );
        }
        for op in SPAWN_OPS {
            let method = op.rsplit("::").next().unwrap_or(op);
            let refusal = guard(
                op,
                &args,
                &mut counts,
                &Limits::default(),
                &CommandPolicy::default(),
            )
            .expect_err(op);
            assert_eq!(
                refusal.to_string(),
                format!("runtime error: horizon: {method} refused: at most 64 spawns per turn"),
                "{op}"
            );
        }
        assert_eq!(counts.spawns, DEFAULT_SPAWNS_PER_TURN);
        assert_eq!(counts.explosions, 0);
    }

    /// The owner's limits replace the defaults in the guard: a lowered cap refuses sooner, a raised one lets
    /// more through, and the refusal names the cap in force.
    #[test]
    fn the_owner_limits_replace_the_defaults() {
        let args = |power: Value| MultiValue::from_vec(vec![Value::Nil, Value::Nil, power]);
        let low = Limits {
            explosion_power: 2.0,
            explosions_per_turn: 1,
            spawns_per_turn: 2,
            maps_per_turn: 1,
        };
        let high = Limits {
            explosion_power: 64.0,
            explosions_per_turn: 10,
            spawns_per_turn: 100,
            maps_per_turn: 10,
        };
        let refusal = guard(
            "LevelRef::explode",
            &args(Value::Number(3.0)),
            &mut TurnCounts::default(),
            &low,
            &CommandPolicy::default(),
        )
        .expect_err("3 is above the lowered cap of 2");
        assert_eq!(
            refusal.to_string(),
            "runtime error: horizon: explode refused: power must be a number from 0 to 2"
        );
        assert!(
            guard(
                "LevelRef::explode",
                &args(Value::Number(50.0)),
                &mut TurnCounts::default(),
                &high,
                &CommandPolicy::default()
            )
            .is_ok()
        );
        let mut count = TurnCounts::default();
        assert!(
            guard(
                "LevelRef::explode",
                &args(Value::Number(1.0)),
                &mut count,
                &low,
                &CommandPolicy::default()
            )
            .is_ok()
        );
        assert!(
            guard(
                "LevelRef::explode",
                &args(Value::Number(1.0)),
                &mut count,
                &low,
                &CommandPolicy::default()
            )
            .is_err()
        );
        let mut count = TurnCounts::default();
        for _ in 0..10 {
            assert!(
                guard(
                    "LevelRef::explode",
                    &args(Value::Number(1.0)),
                    &mut count,
                    &high,
                    &CommandPolicy::default()
                )
                .is_ok()
            );
        }
        let mut counts = TurnCounts::default();
        for _ in 0..2 {
            assert!(
                guard(
                    "LevelRef::spawn_mob",
                    &MultiValue::new(),
                    &mut counts,
                    &low,
                    &CommandPolicy::default()
                )
                .is_ok()
            );
        }
        let refusal = guard(
            "LevelRef::spawn_mob",
            &MultiValue::new(),
            &mut counts,
            &low,
            &CommandPolicy::default(),
        )
        .expect_err("the third spawn is above the lowered cap of 2");
        assert_eq!(
            refusal.to_string(),
            "runtime error: horizon: spawn_mob refused: at most 2 spawns per turn"
        );
        let mut counts = TurnCounts::default();
        for _ in 0..100 {
            assert!(
                guard(
                    "LevelRef::spawn_item",
                    &MultiValue::new(),
                    &mut counts,
                    &high,
                    &CommandPolicy::default()
                )
                .is_ok()
            );
        }
        assert!(
            guard(
                "LevelRef::spawn_item",
                &MultiValue::new(),
                &mut counts,
                &high,
                &CommandPolicy::default()
            )
            .is_err()
        );
    }

    /// The recipe ids of `award_recipes` and `reset_recipes` are a list of at most 64 strings, each
    /// non-empty, at most 128 characters and free of control characters; anything else is refused by name.
    #[test]
    fn the_recipe_ids_are_bounded_and_clean() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        let call = |items: Vec<Value>| -> mlua::Result<MultiValue> {
            let table = lua.create_sequence_from(items)?;
            Ok(MultiValue::from_vec(vec![Value::Table(table)]))
        };
        let text = |s: &str| -> mlua::Result<Value> { Ok(Value::String(lua.create_string(s)?)) };
        let check = |op: &str, args: &MultiValue| {
            guard(
                op,
                args,
                &mut TurnCounts::default(),
                &Limits::default(),
                &CommandPolicy::default(),
            )
        };
        let refusal_of = |op: &str, args: &MultiValue| check(op, args).expect_err(op).to_string();
        for op in ["PlayerRef::award_recipes", "PlayerRef::reset_recipes"] {
            let method = op.rsplit("::").next().unwrap_or(op);
            let full: Vec<Value> = (0..64)
                .map(|_| text("minecraft:stone"))
                .collect::<mlua::Result<_>>()?;
            assert!(check(op, &call(full)?).is_ok(), "{op}: 64 ids pass");
            let too_many: Vec<Value> = (0..65).map(|_| text("a")).collect::<mlua::Result<_>>()?;
            assert_eq!(
                refusal_of(op, &call(too_many)?),
                format!("runtime error: horizon: {method} refused: at most 64 recipe ids per call"),
                "{op}"
            );
            let longest = "a".repeat(128);
            assert!(
                check(op, &call(vec![text(&longest)?])?).is_ok(),
                "{op}: 128 chars pass"
            );
            let longer = "a".repeat(129);
            assert_eq!(
                refusal_of(op, &call(vec![text(&longer)?])?),
                format!(
                    "runtime error: horizon: {method} refused: a recipe id is longer than 128 characters"
                ),
                "{op}"
            );
            assert_eq!(
                refusal_of(op, &call(vec![text("")?])?),
                format!("runtime error: horizon: {method} refused: a recipe id is empty"),
                "{op}"
            );
            assert_eq!(
                refusal_of(op, &call(vec![text("a\nb")?])?),
                format!(
                    "runtime error: horizon: {method} refused: a recipe id has a control character"
                ),
                "{op}"
            );
            assert_eq!(
                refusal_of(op, &call(vec![Value::Integer(5)])?),
                format!(
                    "runtime error: horizon: {method} refused: each recipe id must be a string"
                ),
                "{op}"
            );
        }
        Ok(())
    }

    /// `send_map`'s picture is a Lua string or an array table of integers 0 to 255, at most 16 KiB long; a
    /// bad byte or a longer picture is refused by name, and a refused picture is not counted.
    #[test]
    fn a_map_picture_is_at_most_16_kib_of_bytes() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        let args = |picture: Value| MultiValue::from_vec(vec![Value::Integer(7), picture]);
        let table = |items: Vec<Value>| -> mlua::Result<Value> {
            Ok(Value::Table(lua.create_sequence_from(items)?))
        };
        let mut counts = TurnCounts::default();
        let mut send = |picture: Value| {
            guard(
                "PlayerRef::send_map",
                &args(picture),
                &mut counts,
                &Limits::default(),
                &CommandPolicy::default(),
            )
        };
        let full = "x".repeat(MAP_BYTES);
        assert!(send(Value::String(lua.create_string(&full)?)).is_ok());
        let over = "x".repeat(MAP_BYTES + 1);
        let refusal = send(Value::String(lua.create_string(&over)?)).expect_err("16385 bytes");
        assert_eq!(
            refusal.to_string(),
            "runtime error: horizon: send_map refused: a map is at most 16384 bytes"
        );
        let bytes: Vec<Value> = (0..MAP_BYTES)
            .map(|i| Value::Integer((i % 256) as i64))
            .collect();
        assert!(send(table(bytes)?).is_ok(), "16384 bytes in a table pass");
        let longer: Vec<Value> = (0..=MAP_BYTES).map(|_| Value::Integer(0)).collect();
        assert_eq!(
            send(table(longer)?).expect_err("16385 entries").to_string(),
            "runtime error: horizon: send_map refused: a map is at most 16384 bytes"
        );
        for bad in [Value::Integer(256), Value::Integer(-1), Value::Number(1.5)] {
            assert_eq!(
                send(table(vec![Value::Integer(1), bad])?)
                    .expect_err("a byte outside 0 to 255")
                    .to_string(),
                "runtime error: horizon: send_map refused: a map byte must be a whole number from 0 to 255"
            );
        }
        let text_entry = lua.create_string("x")?;
        assert!(send(table(vec![Value::String(text_entry)])?).is_err());
        assert_eq!(counts.maps, 2, "the refused pictures are not counted");
        Ok(())
    }

    /// One turn may send at most `DEFAULT_MAPS_PER_TURN` map pictures: the next is refused before the host
    /// sees it, and an owner's `maps_per_turn` replaces the count.
    #[test]
    fn a_turn_may_send_only_so_many_maps() -> mlua::Result<()> {
        let lua = mlua::Lua::new();
        let args = MultiValue::from_vec(vec![
            Value::Integer(7),
            Value::String(lua.create_string("picture")?),
        ]);
        let mut counts = TurnCounts::default();
        for i in 0..DEFAULT_MAPS_PER_TURN {
            assert!(
                guard(
                    "PlayerRef::send_map",
                    &args,
                    &mut counts,
                    &Limits::default(),
                    &CommandPolicy::default()
                )
                .is_ok(),
                "map {i}"
            );
        }
        let refusal = guard(
            "PlayerRef::send_map",
            &args,
            &mut counts,
            &Limits::default(),
            &CommandPolicy::default(),
        )
        .expect_err("the ninth map");
        assert_eq!(
            refusal.to_string(),
            "runtime error: horizon: send_map refused: at most 8 maps per turn"
        );
        let one = Limits {
            maps_per_turn: 1,
            ..Limits::default()
        };
        let mut lowered = TurnCounts::default();
        assert!(
            guard(
                "PlayerRef::send_map",
                &args,
                &mut lowered,
                &one,
                &CommandPolicy::default()
            )
            .is_ok()
        );
        assert!(
            guard(
                "PlayerRef::send_map",
                &args,
                &mut lowered,
                &one,
                &CommandPolicy::default()
            )
            .is_err()
        );
        Ok(())
    }

    /// The defaults are the placeholders the write-powers design named, and each owner ceiling is above them.
    #[test]
    fn the_defaults_are_the_placeholders_under_the_ceilings() {
        assert_eq!(Limits::default().explosion_power, 16.0);
        assert_eq!(Limits::default().explosions_per_turn, 4);
        assert_eq!(Limits::default().spawns_per_turn, 64);
        assert_eq!(Limits::default().maps_per_turn, 8);
        const { assert!(DEFAULT_EXPLOSION_POWER <= EXPLOSION_POWER_CEILING) };
        const { assert!(DEFAULT_EXPLOSIONS_PER_TURN <= EXPLOSIONS_PER_TURN_CEILING) };
        const { assert!(DEFAULT_SPAWNS_PER_TURN <= SPAWNS_PER_TURN_CEILING) };
        const { assert!(DEFAULT_MAPS_PER_TURN <= MAPS_PER_TURN_CEILING) };
    }

    /// `has_ops` is true exactly for the powers the generated table gates.
    #[test]
    fn a_power_has_ops_when_the_table_gates_a_write_for_it() {
        assert!(has_ops(Power::World));
        assert!(has_ops(Power::Spawn));
        assert!(has_ops(Power::Explosions));
        assert!(has_ops(Power::GameRules));
        assert!(has_ops(Power::Inventory));
        assert!(has_ops(Power::Nbt));
        for power in [
            Power::Registration,
            Power::Transactions,
            Power::Network,
            Power::Files,
        ] {
            assert!(!has_ops(power), "{} has no ops yet", power.name());
        }
    }

    #[test]
    fn every_power_has_its_name_and_label_key() {
        for power in ALL {
            assert_eq!(Power::from_name(power.name()), Some(power));
            assert_eq!(
                power.label_key(),
                format!("horizon.mods.power.{}", power.name())
            );
        }
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    /// Every power's label and description, and the refusal text, each carry an English entry.
    #[test]
    fn every_power_key_has_an_english_entry() {
        let text = include_str!("../../horizon-client/assets/horizon/lang/en_us.json");
        let lang: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text).unwrap();
        let mut keys = vec![
            REFUSED_KEY.to_string(),
            "mods.provider.power_unknown".to_string(),
            "mods.provider.power_not_requested".to_string(),
            "mods.provider.power_unavailable".to_string(),
            "mods.provider.power_not_editable".to_string(),
        ];
        for power in [
            Power::Commands,
            Power::Nbt,
            Power::Spawn,
            Power::Registration,
            Power::Explosions,
            Power::World,
            Power::GameRules,
            Power::Transactions,
            Power::Inventory,
            Power::Players,
            Power::Network,
            Power::Files,
        ] {
            keys.push(format!("{}.label", power.label_key()));
            keys.push(format!("{}.description", power.label_key()));
        }
        for key in keys {
            let value = lang.get(&key).and_then(|v| v.as_str());
            assert!(
                value.is_some_and(|v| !v.is_empty()),
                "en_us.json has no entry for {key}"
            );
        }
    }
}
