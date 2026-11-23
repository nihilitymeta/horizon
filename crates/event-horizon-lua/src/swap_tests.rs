//! `LuaMod::swap_state`, `rebuild_state` and the declaration fingerprint (5f-c1): a mod's state can
//! be replaced by a freshly built one that declares the same things, and a state that declares
//! something else is refused.

use std::path::Path;

use mlua::Lua;

use crate::discover::{Discovered, ENTRY, Side};
use crate::dispatch::new_event;
use crate::fingerprint::{Fingerprint, SwapRefused};
use crate::host::{LuaHost, LuaMod};

const HOOK: &str = "block_receive_game";

fn found(root: &Path, id: &str) -> Discovered {
    Discovered {
        dir: root.join(id),
        id: id.to_string(),
        version: "1".to_string(),
        side: Side::Both,
    }
}

/// Loads a mod named `id` whose main.luau is `source`.
fn load(root: &Path, id: &str, source: &str) -> LuaMod {
    let f = found(root, id);
    std::fs::create_dir_all(&f.dir).unwrap();
    std::fs::write(f.dir.join(ENTRY), source).unwrap();
    LuaMod::load(&f).unwrap()
}

/// A fresh state for `m`, built from `source` as `m` was built, with its fingerprint.
fn fresh(m: &LuaMod, root: &Path, source: &str) -> Result<(Lua, Fingerprint), String> {
    let f = found(root, m.id());
    m.rebuild_state(&f, &[(ENTRY.to_string(), source.to_string())])
}

/// Swaps the state of the host's only mod, holding its turn as the reload will.
fn swap(host: &LuaHost, lua: Lua, fingerprint: Fingerprint) -> Result<(), SwapRefused> {
    let m = &host.mods()[0];
    let _turn = m.turn().enter();
    m.swap_state(lua, fingerprint)
}

/// Fires the hook once, with the event every test uses.
fn fire(host: &LuaHost) {
    host.dispatch(
        HOOK,
        &|lua| new_event(lua, lua.create_table()?, &[]),
        &mut |_, _| true,
    );
}

fn handler(log: &str) -> String {
    format!("horizon.on(\"{HOOK}\", function() horizon.log(\"{log}\") end)")
}

#[test]
fn a_swapped_in_state_runs_its_own_handlers() {
    let root = tempfile::tempdir().unwrap();
    let m = load(root.path(), "mod_a", &handler("one"));
    let (lua, fingerprint) = fresh(&m, root.path(), &handler("two")).unwrap();
    let host = LuaHost::new(vec![m]);

    fire(&host);
    assert_eq!(host.mods()[0].log_lines(), ["one"]);

    swap(&host, lua, fingerprint).unwrap();
    fire(&host);
    assert_eq!(host.mods()[0].log_lines(), ["one", "two"]);
}

#[test]
fn a_fresh_state_that_adds_a_block_is_refused_and_the_old_state_still_runs() {
    let root = tempfile::tempdir().unwrap();
    let m = load(root.path(), "mod_b", &handler("one"));
    let added = format!(
        "{}\nhorizon.block{{id = \"mod_b:thing\", template = \"minecraft:stone\"}}",
        handler("two")
    );
    let (lua, fingerprint) = fresh(&m, root.path(), &added).unwrap();
    let host = LuaHost::new(vec![m]);

    let refused = swap(&host, lua, fingerprint).unwrap_err();
    assert_eq!(
        refused.what,
        ["blocks: added `mod_b:thing (minecraft:stone)`"]
    );
    fire(&host);
    assert_eq!(host.mods()[0].log_lines(), ["one"]);
}

#[test]
fn a_fresh_state_that_removes_a_command_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let declared = "horizon.command{usage = \"bravo\"}\n";
    let m = load(root.path(), "mod_c", declared);
    let (lua, fingerprint) = fresh(&m, root.path(), "").unwrap();

    let refused = swap(&LuaHost::new(vec![m]), lua, fingerprint).unwrap_err();
    assert_eq!(refused.what, ["commands: removed `bravo level 0`"]);
}

#[test]
fn an_equal_fingerprint_swaps_and_a_handler_error_leaves_the_mod_working() {
    let root = tempfile::tempdir().unwrap();
    let m = load(root.path(), "mod_d", &handler("one"));
    let broken = format!("horizon.on(\"{HOOK}\", function() error(\"boom\") end)");
    let (broken_lua, broken_fp) = fresh(&m, root.path(), &broken).unwrap();
    let (good_lua, good_fp) = fresh(&m, root.path(), &handler("three")).unwrap();
    let host = LuaHost::new(vec![m]);

    swap(&host, broken_lua, broken_fp).unwrap();
    fire(&host);
    assert_eq!(host.mods()[0].log_lines(), Vec::<String>::new());

    swap(&host, good_lua, good_fp).unwrap();
    fire(&host);
    assert_eq!(host.mods()[0].log_lines(), ["three"]);
}

#[test]
fn the_fingerprint_is_stable_and_does_not_depend_on_declaration_order() {
    let root = tempfile::tempdir().unwrap();
    let alpha_then_bravo =
        "horizon.command{usage = \"alpha\"}\nhorizon.command{usage = \"bravo\"}\n";
    let bravo_then_alpha =
        "horizon.command{usage = \"bravo\"}\nhorizon.command{usage = \"alpha\"}\n";
    let first = load(&root.path().join("first"), "mod_e", alpha_then_bravo);
    let second = load(&root.path().join("second"), "mod_e", bravo_then_alpha);

    assert_eq!(first.fingerprint(), first.fingerprint());
    assert_eq!(first.fingerprint(), second.fingerprint());
    let (_, rebuilt) = fresh(&first, &root.path().join("first"), alpha_then_bravo).unwrap();
    assert_eq!(&rebuilt, first.fingerprint());
}

/// A villager part whose one behavior `mymod:farm` has activity `activity`, with a tick handler.
fn farm_behavior(activity: &str) -> String {
    format!(
        r#"horizon.entity_part("minecraft:villager", {{behaviors = {{{{name = "mymod:farm", activity = "{activity}", priority = 1, tick = function(e) end}}}}}})"#
    )
}

#[test]
fn a_behavior_activity_change_is_refused_by_the_fingerprint() {
    // The fingerprint names a behavior's activity (74-b2 gave behaviors one, 5f-c2 added it to the
    // fingerprint): a swap that moves the behavior to another activity is a declaration change.
    let root = tempfile::tempdir().unwrap();
    let m = load(root.path(), "mymod", &farm_behavior("work"));
    let (fresh_state, fingerprint) = fresh(&m, root.path(), &farm_behavior("rest")).unwrap();
    let _turn = m.turn().enter();
    let refused = m.swap_state(fresh_state, fingerprint).unwrap_err();
    assert!(
        refused
            .what
            .iter()
            .any(|line| line.starts_with("behaviors:") && line.contains("mymod:farm")),
        "the behavior's change is named: {:?}",
        refused.what
    );
}

#[test]
fn a_behavior_with_the_same_activity_is_accepted() {
    let root = tempfile::tempdir().unwrap();
    let m = load(root.path(), "mymod", &farm_behavior("work"));
    let (fresh_state, fingerprint) = fresh(&m, root.path(), &farm_behavior("work")).unwrap();
    assert_eq!(&fingerprint, m.fingerprint());
    let _turn = m.turn().enter();
    m.swap_state(fresh_state, fingerprint).unwrap();
}

#[test]
fn two_behaviors_declared_in_either_order_have_equal_fingerprints() {
    let root = tempfile::tempdir().unwrap();
    let farm = r#"{name = "mymod:farm", activity = "work", priority = 1, tick = function(e) end}"#;
    let rest = r#"{name = "mymod:rest", activity = "rest", priority = 2, start = function(e) end}"#;
    let farm_then_rest =
        format!(r#"horizon.entity_part("minecraft:villager", {{behaviors = {{{farm}, {rest}}}}})"#);
    let rest_then_farm =
        format!(r#"horizon.entity_part("minecraft:villager", {{behaviors = {{{rest}, {farm}}}}})"#);
    let first = load(&root.path().join("first"), "mymod", &farm_then_rest);
    let second = load(&root.path().join("second"), "mymod", &rest_then_farm);
    assert_eq!(first.fingerprint(), second.fingerprint());
}
