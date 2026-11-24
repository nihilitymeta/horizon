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
//! - The gated table is EMPTY in this version: nothing is gated and no new host call becomes callable.
//!   `table()` is the hand-written hook the generator's table will replace.
//! - Declarations, `event.feedback`, the `horizon.attachment_*` calls and settings are NOT gated, and stay
//!   that way.

use mlua::Error;

use crate::host::warn_once;
use crate::manifest::Power;

/// The gated host operations: `("Family::method", power)`. Empty in this version.
fn table() -> &'static [(&'static str, Power)] {
    &[]
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

/// The refusal of a gated call of mod `owner` that needs `power` and does not hold it: the one warning
/// for this mod and power, and the Lua error the call raises. The error names the method and the power,
/// never the arguments.
pub(crate) fn refuse(op: &str, owner: &str, power: Power) -> Error {
    let method = op.rsplit("::").next().unwrap_or(op);
    let name = power.name();
    warn_once(
        owner,
        &format!(
            "{owner}: a host call needs the \"{name}\" power, which the owner has not granted; refused"
        ),
    );
    Error::runtime(format!(
        "horizon: {method} needs the \"{name}\" power; the owner has not granted it to {owner}"
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

    #[test]
    fn the_production_table_gates_nothing() {
        for op in [
            "EntityRef::set_glowing",
            "LevelRef::explode",
            "PlayerRef::disconnect",
        ] {
            assert_eq!(required(op), None, "{op}");
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
        let mut keys = vec![REFUSED_KEY.to_string()];
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
