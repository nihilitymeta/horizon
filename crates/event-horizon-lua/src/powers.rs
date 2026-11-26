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
//! - The gated table is generated (`generated/powers.rs`, from `GATED_WRITES` in the generator): the first
//!   wave is the `world` and `spawn` writes listed there. Every other write stays denied.
//! - Declarations, `event.feedback`, the `horizon.attachment_*` calls and settings are NOT gated, and stay
//!   that way.

use mlua::{Error, MultiValue, Value};

use crate::host::warn_once;
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

/// The placeholder cap on an explosion's power (the radius argument of the explosion writes): a call
/// above it is refused before the host sees it. It is a placeholder, not a balance decision; the design
/// sets the real number. A refused call costs one unit, as a power refusal does.
pub const MAX_EXPLOSION_POWER: f64 = 16.0;

/// The explosion writes the cap checks, `(op, index of the power among the Lua arguments)`. Each is
/// listed only so the cap applies once the op binds: `explode` and `explosion_destroy_blocks` do not
/// convert from Lua yet (docs/lua-modding.md, the `explosions` row), so no mod reaches the check today.
const EXPLOSION_POWER_ARGS: &[(&str, usize)] = &[
    ("LevelRef::explode", 2),
    ("LevelRef::explosion_destroy_blocks", 3),
];

/// The guard of a gated call, run after its power check passed and before the host is called. It
/// refuses with a plain Lua error when a rule of the guard says no. The rule: an explosion whose power
/// is above [`MAX_EXPLOSION_POWER`] (or NaN). Any other call passes; a missing or non-numeric power is
/// left to the host's own conversion.
pub(crate) fn guard(op: &str, args: &MultiValue) -> Result<(), Error> {
    let Some((_, index)) = EXPLOSION_POWER_ARGS.iter().find(|(name, _)| *name == op) else {
        return Ok(());
    };
    let Some(power) = args.get(*index).and_then(Value::as_f64) else {
        return Ok(());
    };
    if power.is_nan() || power > MAX_EXPLOSION_POWER {
        let method = op.rsplit("::").next().unwrap_or(op);
        return Err(Error::runtime(format!(
            "horizon: {method} refused: power {power} is above the cap of {MAX_EXPLOSION_POWER}"
        )));
    }
    Ok(())
}

/// The refusal of a gated call of mod `owner` that needs `power` and does not hold it: the one warning
/// for this mod and power, and the Lua error the call raises. The error names the method and the power,
/// never the arguments.
pub(crate) fn refuse(op: &str, owner: &str, power: Power) -> Error {
    let method = op.rsplit("::").next().unwrap_or(op);
    let name = power.name();
    warn_once(
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

    const ALL: [Power; 10] = [
        Power::Commands,
        Power::Nbt,
        Power::Spawn,
        Power::Registration,
        Power::Explosions,
        Power::World,
        Power::GameRules,
        Power::Transactions,
        Power::Network,
        Power::Files,
    ];

    /// The first wave, written out by hand (not read from the generator): each op and its power.
    const FIRST_WAVE: [(&str, Power); 24] = [
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
        ("PlayerRef::respawn", Power::Spawn),
    ];

    #[test]
    fn the_generated_table_gates_exactly_the_first_wave() {
        assert_eq!(table().len(), FIRST_WAVE.len());
        for (op, power) in FIRST_WAVE {
            assert_eq!(required(op), Some(power), "{op}");
        }
        for op in [
            "EntityRef::set_glowing",
            "LevelRef::explode",
            "LevelRef::explosion_destroy_blocks",
            "LevelRef::set_game_rule",
            "LevelRef::spawn_item",
            "LevelRef::spawn_item_with_motion",
            "LevelRef::pop_resource",
            "PlayerRef::disconnect",
            "EntityRef::remove",
            "EntityRef::merge_nbt",
            "PlayerRef::set_nickname",
            "EntityRef::run_command",
            "PlayerRef::give_stack",
            "LevelRef::add_water_ticket",
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

    /// The explosion cap refuses a power above [`MAX_EXPLOSION_POWER`] and passes one at it. The rule is
    /// checked on the guard itself: no explosion write binds yet, so no mod reaches it through Lua.
    #[test]
    fn an_explosion_above_the_cap_is_refused_and_one_at_it_passes() {
        let args = |power: Value| MultiValue::from_vec(vec![Value::Nil, Value::Nil, power]);
        let refusal = guard("LevelRef::explode", &args(Value::Number(17.0)))
            .expect_err("power 17 is above the cap");
        assert_eq!(
            refusal.to_string(),
            "runtime error: horizon: explode refused: power 17 is above the cap of 16"
        );
        assert!(guard("LevelRef::explode", &args(Value::Number(16.0))).is_ok());
        assert!(guard("LevelRef::explode", &args(Value::Integer(16))).is_ok());
        assert!(guard("LevelRef::explode", &args(Value::Number(f64::NAN))).is_err());
        // The power of explosion_destroy_blocks is its fourth argument.
        let blocks = MultiValue::from_vec(vec![
            Value::Nil,
            Value::Nil,
            Value::Nil,
            Value::Number(17.5),
        ]);
        let refusal = guard("LevelRef::explosion_destroy_blocks", &blocks)
            .expect_err("power 17.5 is above the cap");
        assert!(
            refusal.to_string().contains(
                "horizon: explosion_destroy_blocks refused: power 17.5 is above the cap of 16"
            ),
            "{refusal}"
        );
        // Any other op passes, whatever its numbers.
        assert!(guard("LevelRef::strike_lightning", &args(Value::Number(99.0))).is_ok());
    }

    /// `has_ops` is true exactly for the powers the generated table gates.
    #[test]
    fn a_power_has_ops_when_the_table_gates_a_write_for_it() {
        assert!(has_ops(Power::World));
        assert!(has_ops(Power::Spawn));
        for power in [
            Power::Commands,
            Power::Nbt,
            Power::Registration,
            Power::Explosions,
            Power::GameRules,
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
