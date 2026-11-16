//! Callbacks into the executable for data-loading native trait mods.
use crate::loot::{LootStack, NativeLootContext};
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Data-loading hooks, installed before any data loads. Earlier loads see no mods.
pub struct NativeLoadHooks {
    pub loot_table_load: fn(&str, &mut Value),
    pub modify_default_components: fn(&str, &mut Map<String, Value>),
    pub loot_function: fn(&str, &Value, &NativeLootContext<'_>, &mut LootStack),
    pub loot_condition: fn(&str, &Value, &NativeLootContext<'_>, &mut bool),
}

static HOOKS: OnceLock<NativeLoadHooks> = OnceLock::new();

/// Install the process-wide callbacks once; a second installation keeps the first.
pub fn install(hooks: NativeLoadHooks) -> Result<(), NativeLoadHooks> {
    HOOKS.set(hooks)
}

pub(crate) fn loot_table_load(id: &str, table: &mut Value) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.loot_table_load)(id, table);
    }
}

pub(crate) fn modify_default_components(item: &str, components: &mut Map<String, Value>) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.modify_default_components)(item, components);
    }
}

pub(crate) fn loot_function(
    kind: &str,
    data: &Value,
    ctx: &NativeLootContext<'_>,
    stack: &mut LootStack,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.loot_function)(kind, data, ctx, stack);
    }
}

pub(crate) fn loot_condition(
    kind: &str,
    data: &Value,
    ctx: &NativeLootContext<'_>,
    holds: &mut bool,
) {
    if let Some(hooks) = HOOKS.get() {
        (hooks.loot_condition)(kind, data, ctx, holds);
    }
}

/// The one test stub this crate's lib test binary installs.
///
/// CI runs `cargo test --locked` (tools/pr_ready.py too), so every lib test of
/// horizon-world shares one process, and `install` is a process-wide OnceLock:
/// the first installer wins and a later one is refused. So every test that
/// needs a hook installs this stub, and it answers all of them: the item
/// defaults' mod sizes (`modify_default_components`) and the `ruby:` loot kinds
/// (`loot_condition`, `loot_function`). Other kinds and items are left alone,
/// so a test that does not use them still sees the hook as a no-op.
#[cfg(test)]
pub(crate) mod test_hooks {
    use super::{NativeLoadHooks, install};
    use crate::loot::{LootStack, NativeLootContext};
    use serde_json::{Map, Value};
    use std::cell::RefCell;

    thread_local! {
        /// The `(kind, data)` pairs of the `ruby:` loot kinds seen on this
        /// thread. A roll runs on its test's thread, so each test reads its own.
        static SEEN: RefCell<Vec<(String, Value)>> = const { RefCell::new(Vec::new()) };
    }

    /// The `ruby:` loot kinds this thread's tests have seen since the last call.
    pub(crate) fn take_seen_ruby_kinds() -> Vec<(String, Value)> {
        SEEN.with_borrow_mut(std::mem::take)
    }

    /// Install the shared stub. A second installer in the binary is refused,
    /// which is fine: the first one already is this stub.
    pub(crate) fn install_test_hooks() {
        let _ = install(NativeLoadHooks {
            loot_table_load: |_, _| {},
            modify_default_components,
            loot_function,
            loot_condition,
        });
    }

    /// The mod sizes `item_defaults::read` sanitises: a raised, zero, huge,
    /// string or removed `minecraft:max_stack_size`.
    fn modify_default_components(item: &str, components: &mut Map<String, Value>) {
        const STACK_SIZE: &str = "minecraft:max_stack_size";
        match item {
            "a:raised" => {
                components.insert(STACK_SIZE.into(), 500.into());
            }
            "a:zero" => {
                components.insert(STACK_SIZE.into(), 0.into());
            }
            "a:huge" => {
                components.insert(STACK_SIZE.into(), u64::MAX.into());
            }
            "a:string" => {
                components.insert(STACK_SIZE.into(), "500".into());
            }
            "a:removed" => {
                components.remove(STACK_SIZE);
            }
            _ => {}
        }
    }

    /// A `ruby:` condition answers with its own `holds`.
    fn loot_condition(kind: &str, data: &Value, _: &NativeLootContext<'_>, holds: &mut bool) {
        if !kind.starts_with("ruby:") {
            return;
        }
        SEEN.with_borrow_mut(|seen| seen.push((kind.to_owned(), data.clone())));
        *holds = data["holds"].as_bool().unwrap_or(false);
    }

    /// A `ruby:` function multiplies the stack's count by its own `factor`,
    /// and sets its item to its own `item` when it names one.
    fn loot_function(kind: &str, data: &Value, _: &NativeLootContext<'_>, stack: &mut LootStack) {
        if !kind.starts_with("ruby:") {
            return;
        }
        SEEN.with_borrow_mut(|seen| seen.push((kind.to_owned(), data.clone())));
        if let Some(item) = data["item"].as_str() {
            stack.item = item.to_owned();
        }
        stack.count *= data["factor"].as_i64().unwrap_or(1) as i32;
    }
}
