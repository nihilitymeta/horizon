//! Menu parts (#74, B): what a Lua `menu_still_valid` handler sees of a mod menu's validity check, what
//! its answer does, and how a refused, failing or panicking handler is kept. The menu is another mod's
//! (the game's own menu types are never asked), so the part names it by id and nothing here needs the
//! game. The level and the loop context are empty doubles: the hook reads nothing from them.

use std::path::Path;
use std::sync::Arc;

use mlua::Lua;
use mod_api::{ItemKey, LoadedMods, LoopContext, LoopSide, MenuPart, MenuStillValidEvent};

use crate::discover::{Discovered, ENTRY, Side};
use crate::host::{LuaMenuPart, LuaMod, LuaPart};

/// A loop context that knows nothing: `menu_still_valid` asks the context for nothing.
struct Nothing;

impl LoopContext for Nothing {
    fn mod_recipe_property_contains(&self, _: &str, _: ItemKey) -> bool {
        false
    }

    fn side(&self) -> LoopSide {
        LoopSide::Server
    }

    fn registry(&self, _: &str) -> Option<Arc<[String]>> {
        None
    }

    fn setting(&self, _: &str, _: &str) -> Option<String> {
        None
    }

    fn mods(&self) -> LoadedMods {
        unreachable!("menu_still_valid does not read the loaded mods")
    }
}

/// The player whose menu is asked: the UUID `01234567-89ab-cdef-0123-456789abcdef`.
const PLAYER: u128 = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef;

/// Loads one mod `id` whose main.luau is `source`, from its own folder under `root`.
fn load_one(root: &Path, id: &str, source: &str) -> LuaMod {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ENTRY), source).unwrap();
    LuaMod::load(&Discovered {
        dir,
        id: id.to_owned(),
        version: "1".to_owned(),
        side: Side::Server,
    })
    .unwrap()
}

/// The menu part `index` of the mod at `mod_index` in `mods`.
fn part(mods: &Arc<[LuaMod]>, mod_index: usize, index: usize) -> LuaMenuPart {
    LuaPart::new(mods.clone(), mod_index, index).unwrap()
}

/// The event the game asks for `other:press_menu` at block (4, 64, -2), with the game's answer `valid`.
fn event(valid: bool) -> MenuStillValidEvent<'static> {
    MenuStillValidEvent {
        player: PLAYER,
        menu: "other:press_menu",
        pos: [4, 64, -2],
        eye: [4.5, 65.5, -1.5],
        valid,
    }
}

/// A mod with one menu part on `other:press_menu` whose `menu_still_valid` is `body`.
fn press_mod(root: &Path, id: &str, body: &str) -> Arc<[LuaMod]> {
    let source = format!(
        "horizon.menu_part(\"other:press_menu\", {{menu_still_valid = function(event) {body} end}})"
    );
    Arc::from(vec![load_one(root, id, &source)])
}

#[test]
fn a_handler_sees_the_menu_the_player_the_block_the_eye_and_the_answer() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = press_mod(
        root.path(),
        "mymod",
        "horizon.log(event.menu .. \" \" .. tostring(event.pos.y == 64) .. \" \" .. tostring(event.eye.z == -1.5) \
         .. \" \" .. tostring(event.valid) .. \" \" .. event.player)",
    );
    let mut answer = event(true);
    part(&mods, 0, 0).menu_still_valid(&Nothing, &mut answer);
    assert_eq!(
        mods[0].log_lines(),
        ["other:press_menu true true true 01234567-89ab-cdef-0123-456789abcdef"]
    );
    assert!(answer.valid);
}

#[test]
fn a_true_or_false_answer_reaches_the_output() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let closes = press_mod(root.path(), "closes", "event.valid = false");
    let mut answer = event(true);
    part(&closes, 0, 0).menu_still_valid(&Nothing, &mut answer);
    assert!(!answer.valid);

    let keeps = press_mod(root.path(), "keeps", "event.valid = true");
    let mut answer = event(false);
    part(&keeps, 0, 0).menu_still_valid(&Nothing, &mut answer);
    assert!(answer.valid);
}

#[test]
fn a_nil_or_non_boolean_answer_is_no_opinion_and_leaves_the_game_answer() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let silent = press_mod(root.path(), "silent", "event.valid = nil");
    let mut answer = event(false);
    part(&silent, 0, 0).menu_still_valid(&Nothing, &mut answer);
    assert!(!answer.valid);

    let numeric = press_mod(root.path(), "numeric", "event.valid = 1");
    let mut answer = event(false);
    part(&numeric, 0, 0).menu_still_valid(&Nothing, &mut answer);
    assert!(!answer.valid);
    let refused = crate::host::tests::captured_with_prefix(
        "numeric: menu_still_valid: output `valid` refused",
    );
    assert_eq!(refused.len(), 1, "got {refused:?}");
}

#[test]
fn a_failing_handler_voids_its_turn_and_keeps_the_game_answer() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = press_mod(
        root.path(),
        "failing",
        "event.valid = false error(\"boom\")",
    );
    let mut answer = event(true);
    part(&mods, 0, 0).menu_still_valid(&Nothing, &mut answer);
    assert!(answer.valid);
}

#[test]
fn another_mods_part_on_the_same_menu_is_not_called() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mine = load_one(
        root.path(),
        "mymod",
        "horizon.menu_part(\"other:press_menu\", {menu_still_valid = function(event) event.valid = false end})",
    );
    let other = load_one(
        root.path(),
        "other",
        "horizon.menu_part(\"other:press_menu\", {menu_still_valid = function(event) horizon.log(\"ran\") event.valid = true end})",
    );
    let mods: Arc<[LuaMod]> = Arc::from(vec![mine, other]);
    let mut answer = event(true);
    part(&mods, 0, 0).menu_still_valid(&Nothing, &mut answer);
    assert!(!answer.valid);
    assert!(mods[1].log_lines().is_empty());
}

#[test]
fn a_panic_in_the_menu_part_build_ends_the_turn_and_quarantines_the_hook() {
    crate::host::tests::install_capture();
    let root = tempfile::tempdir().unwrap();
    let mods = press_mod(root.path(), "panic_menu", "event.valid = false");
    let build = |_: &Lua,
                 _: &crate::hostcall::TurnHost<'_>|
     -> mlua::Result<crate::dispatch::Event> { panic!("the part build broke") };
    let mut answered = 0;
    part(&mods, 0, 0)
        .dispatch_turns(
            "menu_still_valid",
            crate::hostcall::TurnHost::None,
            &|| false,
            &build,
            &mut |_, _, _| {
                answered += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(answered, 0);
    assert_eq!(
        crate::host::tests::captured_with_prefix(
            "panic_menu: menu_still_valid: the game's host panicked"
        ),
        [
            "panic_menu: menu_still_valid: the game's host panicked: the part build broke (this mod no longer receives this hook)"
        ]
    );
    assert!(crate::quarantine::is_quarantined(
        "panic_menu",
        "menu_still_valid"
    ));
}

/// Cost of one `menu_still_valid` delivery to a subscribed part (#74, B), in ns per call: the handler
/// reads three fields and answers. It asserts only that the answer arrives, never a time.
/// Run: `heavy cargo nextest run -p event-horizon-lua --run-ignored only menu_still_valid_bench --nocapture`
#[test]
#[ignore]
fn menu_still_valid_bench() {
    const N: u32 = 200_000;
    let root = tempfile::tempdir().unwrap();
    let mods = press_mod(
        root.path(),
        "bench",
        "event.valid = event.pos.y == 64 and event.eye.y > 65 and event.menu == \"other:press_menu\"",
    );
    let menu = part(&mods, 0, 0);
    let start = std::time::Instant::now();
    for _ in 0..N {
        let mut answer = event(false);
        menu.menu_still_valid(&Nothing, std::hint::black_box(&mut answer));
        assert!(
            std::hint::black_box(answer.valid),
            "the handler's answer did not arrive"
        );
    }
    let ns = start.elapsed().as_nanos() as f64 / f64::from(N);
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    eprintln!("\nmenu_still_valid bench (#74, B): profile {profile}, N = {N}");
    eprintln!("subscribed, handler reads three fields: {ns:.1} ns/call");
}
