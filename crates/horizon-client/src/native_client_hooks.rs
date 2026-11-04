//! Callbacks into the executable for native trait mods' client-library hooks.
use crate::items::ItemStack;
use crate::mobs::Mob;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Client-library hooks, installed before any menu, entity or recipe is used.
pub struct NativeClientHooks {
    pub menu_slot_rules: fn(&str, usize, &ItemStack, &mut bool, &mut u32),
    pub living_heal: fn(u32, &mut f32, &mut bool),
    pub player_chat: fn(u128, &str, &str, &mut Vec<horizon_client_core::text::Text>, &mut bool),
    pub living_breathe: fn(u32, &str, &mut bool, &mut i32, &mut i32),
    pub living_drown: fn(u32, &str, &mut bool, &mut f32, i32, &mut bool),
    pub living_fall: fn(u32, &str, &mut f64, &mut f32, &mut bool),
    pub living_jump: fn(u32, &str),
    pub ingredient: fn(&str, &Map<String, Value>, &mut Vec<String>),
    pub ingredient_test: fn(&str, &Map<String, Value>, &ItemStack, &mut bool),
    pub mob_ai_register: fn(&mut Mob),
}

static HOOKS: OnceLock<NativeClientHooks> = OnceLock::new();

/// Install the process-wide callbacks once; a second installation keeps the first.
pub fn install(hooks: NativeClientHooks) -> Result<(), NativeClientHooks> {
    HOOKS.set(hooks)
}

pub(crate) fn menu_slot_rules(
    menu: &str,
    slot: usize,
    stack: &ItemStack,
    may_place: &mut bool,
    max_stack: &mut u32,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.menu_slot_rules)(menu, slot, stack, may_place, max_stack);
    }
}

pub(crate) fn player_chat(
    player: u128,
    name: &str,
    message: &str,
    replies: &mut Vec<horizon_client_core::text::Text>,
    cancel: &mut bool,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.player_chat)(player, name, message, replies, cancel);
    }
}

pub(crate) fn living_heal(entity: u32, amount: &mut f32, cancel: &mut bool) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.living_heal)(entity, amount, cancel);
    }
}

pub(crate) fn living_breathe(
    entity: u32,
    kind: &str,
    can_breathe: &mut bool,
    consume: &mut i32,
    refill: &mut i32,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.living_breathe)(entity, kind, can_breathe, consume, refill);
    }
}

pub(crate) fn living_drown(
    entity: u32,
    kind: &str,
    is_drowning: &mut bool,
    damage_amount: &mut f32,
    bubble_count: i32,
    cancel: &mut bool,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.living_drown)(
            entity,
            kind,
            is_drowning,
            damage_amount,
            bubble_count,
            cancel,
        );
    }
}

pub(crate) fn living_fall(
    entity: u32,
    kind: &str,
    distance: &mut f64,
    multiplier: &mut f32,
    cancel: &mut bool,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.living_fall)(entity, kind, distance, multiplier, cancel);
    }
}

pub(crate) fn mob_ai_register(mob: &mut Mob) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.mob_ai_register)(mob);
    }
}

pub(crate) fn living_jump(entity: u32, kind: &str) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.living_jump)(entity, kind);
    }
}

pub(crate) fn ingredient(kind: &str, data: &Map<String, Value>, items: &mut Vec<String>) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.ingredient)(kind, data, items);
    }
}

pub(crate) fn ingredient_test(
    kind: &str,
    data: &Map<String, Value>,
    stack: &ItemStack,
    accept: &mut bool,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.ingredient_test)(kind, data, stack, accept);
    }
}

/// The hooks the crate's tests install: one set for the process, so every
/// test that needs a hook installs the same one (a second `install` would
/// find the first).
#[cfg(test)]
pub(crate) mod test_hooks {
    use super::*;
    use std::sync::Mutex;

    static AI_REGISTERED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    /// Installs the test hooks, or finds them installed: the slot rules of
    /// the `test:rules` menu (slot 0 takes nothing, stacks of 7) and a
    /// record of the mobs whose AI `mob_ai_register` built.
    pub(crate) fn install() {
        let _ = super::install(NativeClientHooks {
            menu_slot_rules: |menu, slot, _, may_place, max_stack| {
                if menu == "test:rules" && slot == 0 {
                    *may_place = false;
                    *max_stack = 7;
                }
            },
            player_chat: |_, _, _, _, _| {},
            living_heal: |_, _, _| {},
            living_breathe: |_, _, _, _, _| {},
            living_drown: |_, _, _, _, _, _| {},
            living_fall: |_, _, _, _, _| {},
            living_jump: |_, _| {},
            ingredient: |_, _, _| {},
            ingredient_test: |_, _, _, _| {},
            mob_ai_register: |mob| {
                AI_REGISTERED
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(mob.id)
            },
        });
    }
    /// The ids of every mob `mob_ai_register` was told of so far.
    pub(crate) fn ai_registered() -> Vec<u32> {
        AI_REGISTERED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
