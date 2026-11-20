//! Brain behaviors a mod declares (#74, B2): the `behaviors` table of an entity part adds one
//! [`LuaBehavior`] per declared behavior to each brain of that type, when the brain is built (the
//! generated `register_behaviors` calls `add_declared`). Declaring a behavior runs no Lua, and building a
//! brain runs none either: each `LuaBehavior` is only a record of its name until a turn runs.
//!
//! B2a (this chunk): the handler bodies are safe stubs. `check_extra_start_conditions` answers false, so
//! the behavior never starts; `can_still_use` answers false; `start`, `tick` and `stop` do nothing. The
//! real turns (the answers, the `e.ctx` actions and the shared budget gating) are B2b.

use std::sync::Arc;

use mod_api::{Behavior, BehaviorCx, BehaviorRegistrar};

use crate::host::{LuaMod, warn_once};
use crate::mob_turn::intern;

/// Adds the behaviors `mods[index]` declared for the entity type `entity` to the brain being built. A
/// refusal from the registrar (the type has no brain, the activity is not one of the brain's, or the
/// duration is not one vanilla draws) is logged once per mod, since the registrar asks every non-brain
/// type at construction too.
pub(crate) fn add_declared(
    mods: &Arc<[LuaMod]>,
    index: usize,
    entity: &str,
    behaviors: &mut BehaviorRegistrar<'_>,
) {
    let module = &mods[index];
    for record in module.behavior_records() {
        if record.entity != entity {
            continue;
        }
        let behavior = LuaBehavior {
            name: intern(&record.name),
        };
        let added = behaviors.add(
            record.activity,
            record.priority,
            record.min_duration,
            record.max_duration,
            &[],
            behavior,
        );
        if let Err(refusal) = added {
            warn_once(
                module.id(),
                &format!("{}: behavior {}: {refusal}", module.id(), record.name),
            );
        }
    }
}

/// One behavior of a mod on one brain: its name in the mod's namespace. B2a keeps nothing else (see the
/// module's note on the stubs).
pub struct LuaBehavior {
    name: &'static str,
}

impl Behavior for LuaBehavior {
    fn check_extra_start_conditions(&mut self, _cx: &mut BehaviorCx<'_>) -> bool {
        // B2a stub: the behavior never starts until B2b runs the handler.
        false
    }

    fn can_still_use(&mut self, _cx: &mut BehaviorCx<'_>) -> bool {
        // B2a stub: nothing runs, so nothing keeps running.
        false
    }

    fn name(&self) -> &'static str {
        self.name
    }
}
