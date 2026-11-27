//! `reload` (5f-c2): a dev-path mod's scripts are read again and swapped in when their declaration is
//! unchanged; every other outcome leaves the old state running. The contexts are built here, not
//! through `builtin`, so no test sets the process-wide one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::discover::{self, ENTRY};
use crate::dispatch::new_event;
use crate::host::{LuaHost, LuaMod};
use crate::reload::{Context, ReloadError, Reloaded, reload_in, reloadable_in};

const HOOK: &str = "block_receive_game";

fn handler(log: &str) -> String {
    format!("horizon.on(\"{HOOK}\", function() horizon.log(\"{log}\") end)")
}

/// Writes `<dir>/` as a folder mod: its manifest and `main.luau`.
fn write_mod(dir: &Path, id: &str, source: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join(discover::MANIFEST),
        format!("id = \"{id}\"\nversion = \"1.0.0\"\nside = \"both\"\n"),
    )
    .unwrap();
    std::fs::write(dir.join(ENTRY), source).unwrap();
}

/// A game folder with one dev mod `id` (its folder is `dev`) whose script is `source`, loaded the
/// way the game loads its mods. The host and its context are returned.
fn setup(game: &Path, dev: &Path, id: &str, source: &str) -> (LuaHost, Context) {
    write_mod(dev, id, source);
    let (mods, refused, _, _, _, _, _) =
        crate::builtin::load_mods(game, &[dev.to_path_buf()], false);
    assert!(refused.is_empty(), "the mod loads: {refused:?}");
    let host = LuaHost::new(mods);
    let context = Context::new(host.shared_mods(), game, &[dev.to_path_buf()], false);
    (host, context)
}

fn fire(host: &LuaHost) {
    host.dispatch(
        HOOK,
        &|lua| new_event(lua, lua.create_table()?, &[]),
        &mut |_, _| true,
    );
}

fn log_of(host: &LuaHost) -> Vec<String> {
    host.mods()[0].log_lines()
}

fn write_script(dev: &Path, source: &str) {
    std::fs::write(dev.join(ENTRY), source).unwrap();
}

#[test]
fn an_edited_dev_script_is_the_one_that_runs_after_a_reload() {
    let game = tempfile::tempdir().unwrap();
    let dev = tempfile::tempdir().unwrap();
    let dir: PathBuf = dev.path().join("mod_a");
    let (host, context) = setup(game.path(), &dir, "mod_a", &handler("one"));
    fire(&host);
    assert_eq!(log_of(&host), ["one"]);

    write_script(&dir, &handler("two"));
    assert_eq!(
        reload_in(&context, "mod_a"),
        Ok(Reloaded {
            id: "mod_a".to_owned(),
            scripts: 1
        })
    );
    fire(&host);
    assert_eq!(log_of(&host), ["one", "two"]);
}

#[test]
fn an_installed_mod_is_not_a_dev_mod_and_nothing_changes() {
    let game = tempfile::tempdir().unwrap();
    let installed = game.path().join(discover::DIR).join("mod_b");
    write_mod(&installed, "mod_b", &handler("one"));
    let (mods, _, _, _, _, _, _) = crate::builtin::load_mods(game.path(), &[], false);
    let host = LuaHost::new(mods);
    let context = Context::new(host.shared_mods(), game.path(), &[], false);

    write_script(&installed, &handler("two"));
    assert_eq!(reload_in(&context, "mod_b"), Err(ReloadError::NotADevMod));
    fire(&host);
    assert_eq!(log_of(&host), ["one"]);
}

#[test]
fn a_reload_that_adds_a_block_is_refused_and_the_old_state_still_runs() {
    let game = tempfile::tempdir().unwrap();
    let dev = tempfile::tempdir().unwrap();
    let dir = dev.path().join("mod_c");
    let (host, context) = setup(game.path(), &dir, "mod_c", &handler("one"));

    write_script(
        &dir,
        &format!(
            "{}\nhorizon.block{{id = \"mod_c:thing\", template = \"minecraft:stone\"}}",
            handler("two")
        ),
    );
    match reload_in(&context, "mod_c") {
        Err(ReloadError::Declaration(what)) => {
            assert!(
                what.iter().any(|line| line.contains("mod_c:thing")),
                "the refusal names the new block: {what:?}"
            );
        }
        other => panic!("expected a declaration refusal, got {other:?}"),
    }
    fire(&host);
    assert_eq!(log_of(&host), ["one"]);
}

#[test]
fn a_script_with_a_syntax_error_fails_and_a_later_fixed_reload_works() {
    let game = tempfile::tempdir().unwrap();
    let dev = tempfile::tempdir().unwrap();
    let dir = dev.path().join("mod_d");
    let (host, context) = setup(game.path(), &dir, "mod_d", &handler("one"));

    write_script(&dir, "horizon.on(\"block_receive_game\", function(");
    assert!(matches!(
        reload_in(&context, "mod_d"),
        Err(ReloadError::Failed(why)) if !why.is_empty()
    ));
    fire(&host);
    assert_eq!(log_of(&host), ["one"], "the old handler still runs");

    write_script(&dir, &handler("two"));
    assert!(reload_in(&context, "mod_d").is_ok());
    fire(&host);
    assert_eq!(log_of(&host), ["one", "two"]);
}

#[test]
fn an_unknown_id_is_not_loaded() {
    let game = tempfile::tempdir().unwrap();
    let dev = tempfile::tempdir().unwrap();
    let (_host, context) = setup(
        game.path(),
        &dev.path().join("mod_e"),
        "mod_e",
        &handler("x"),
    );
    assert_eq!(reload_in(&context, "nobody"), Err(ReloadError::NotLoaded));
}

#[test]
fn a_turn_held_by_another_thread_is_busy_and_nothing_is_swapped() {
    let game = tempfile::tempdir().unwrap();
    let dev = tempfile::tempdir().unwrap();
    let dir = dev.path().join("mod_f");
    let (host, context) = setup(game.path(), &dir, "mod_f", &handler("one"));
    write_script(&dir, &handler("two"));

    let mods: Arc<[LuaMod]> = host.shared_mods();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = Arc::clone(&mods);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let _turn = holder[0].turn().enter();
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        held_rx.recv().unwrap();
        assert_eq!(reload_in(&context, "mod_f"), Err(ReloadError::Busy));
        release_tx.send(()).unwrap();
    });
    fire(&host);
    assert_eq!(log_of(&host), ["one"], "a busy reload swaps nothing");
}

#[test]
fn only_the_dev_mods_are_reloadable() {
    let game = tempfile::tempdir().unwrap();
    let dev = tempfile::tempdir().unwrap();
    let dir = dev.path().join("mod_g");
    let (host, context) = setup(game.path(), &dir, "mod_g", &handler("one"));
    assert_eq!(reloadable_in(&context), ["mod_g"]);
    drop(host);
}

#[test]
fn a_goal_flag_change_is_refused_by_the_fingerprint() {
    // The fingerprint names a goal's flags (5f-c2 closed the gap 5f-c1 left): a swap that moves a
    // goal to other flags is a declaration change.
    let goal = |flags: &str| {
        format!(
            "horizon.entity_part(\"minecraft:zombie\", {{goals = {{{{name = \"mymod:g\", priority = 1, flags = {{\"{flags}\"}}, can_use = function() return true end}}}}}})"
        )
    };
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("mymod");
    write_mod(&dir, "mymod", &goal("move"));
    let found = discover::Discovered {
        dir: dir.clone(),
        id: "mymod".to_owned(),
        version: "1.0.0".to_owned(),
        side: discover::Side::Both,
    };
    let m = LuaMod::load(&found).unwrap();
    let (fresh, fingerprint) = m
        .rebuild_state(&found, &[(ENTRY.to_owned(), goal("look"))])
        .unwrap();
    let _turn = m.turn().enter();
    let refused = m.swap_state(fresh, fingerprint).unwrap_err();
    assert!(
        refused.what.iter().any(|line| line.starts_with("goals:")),
        "the goal's change is named: {:?}",
        refused.what
    );
}
