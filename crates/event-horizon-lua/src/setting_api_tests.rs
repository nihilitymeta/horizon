//! `horizon.setting`, `horizon.on_setting_changed` and the delivery of a change (setting_api.rs), with
//! the provider calls of `LuaHost` that make a change.

use std::path::Path;
use std::sync::Arc;

use mlua::{Lua, Value as LuaValue};

use crate::discover::{Discovered, ENTRY, Side};
use crate::host::collect_load_notices;
use crate::host::tests::{captured_with_prefix, install_capture};
use crate::host::{LuaHost, LuaMod};
use crate::quarantine;
use crate::setting_api;
use crate::settings::{Change, Role, Schema, SettingError, Settings, Value};

/// The schema every test mod declares: an integer slider, a toggle and a text field.
const SCHEMA: &str = r#"
[[settings]]
key = "level"
type = "slider"
label = "Level"
min = 0
max = 10
step = 1
integer = true
default = 1

[[settings]]
key = "on"
type = "toggle"
label = "On"
default = true

[[settings]]
key = "name"
type = "text"
label = "Name"
default = "wolf"
"#;

fn schema() -> Schema {
    let table: toml::Table = SCHEMA.parse().expect("test TOML");
    let tables: Vec<toml::Table> = table["settings"]
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_table().expect("table").clone())
        .collect();
    Schema::parse(&tables).expect("valid schema")
}

/// Writes `<game>/config/<id>/settings.toml` with `text`.
fn write_file(game: &Path, id: &str, text: &str) {
    let dir = game.join("config").join(id);
    std::fs::create_dir_all(&dir).expect("config folder");
    std::fs::write(dir.join("settings.toml"), text).expect("settings file");
}

/// Loads mod `id` whose main.luau is `script`, with its settings read from `game`. The mod's folder
/// is only read while it loads. The load error, when there is one, is the text.
fn load(game: &Path, id: &str, script: &str) -> Result<LuaMod, String> {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join(id);
    std::fs::create_dir_all(&dir).expect("mod folder");
    std::fs::write(dir.join(ENTRY), script).expect("script");
    let found = Discovered {
        dir,
        id: id.to_owned(),
        version: "1".to_owned(),
        side: Side::Both,
    };
    let (settings, notices) =
        collect_load_notices(|| Settings::load(game, id, schema(), Role::ServerRuns));
    LuaMod::load_with(&found, settings, notices)
}

/// A handler on `block_receive_game` that logs what `body` returns, through `horizon.log`.
fn on_block(body: &str) -> String {
    format!("horizon.on(\"block_receive_game\", function()\n{body}\nend)")
}

/// Fires the test hook once and returns what the mod logged.
fn fire(m: LuaMod) -> Vec<String> {
    let host = LuaHost::new(vec![m]);
    host.fire("block_receive_game");
    host.mods()[0].log_lines()
}

#[test]
fn declaration_and_handler_read_the_defaults_then_the_file_and_a_changed_file() {
    let game = tempfile::tempdir().expect("tempdir");
    let script = format!(
        "local level = horizon.setting(\"level\")\n\
         local on = horizon.setting(\"on\")\n\
         DECLARED = level .. \"|\" .. tostring(on) .. \"|\" .. horizon.setting(\"name\")\n{}",
        on_block(
            "horizon.log(DECLARED)\n\
             horizon.log(horizon.setting(\"name\") .. \"|\" .. tostring(horizon.setting(\"on\")))"
        )
    );
    // No file: the defaults, at declaration and in the handler.
    let lines = fire(load(game.path(), "setting_probe_a", &script).expect("loads"));
    assert_eq!(lines, ["1|true|wolf", "wolf|true"]);

    // A file, then a changed file: each load reads what the file says then.
    write_file(
        game.path(),
        "setting_probe_a",
        "level = 7\non = false\nname = \"fox\"\n",
    );
    let lines = fire(load(game.path(), "setting_probe_a", &script).expect("loads"));
    assert_eq!(lines, ["7|false|fox", "fox|false"]);
    write_file(game.path(), "setting_probe_a", "level = 2\n");
    let lines = fire(load(game.path(), "setting_probe_a", &script).expect("loads"));
    assert_eq!(lines, ["2|true|wolf", "wolf|true"]);
}

#[test]
fn an_unknown_key_is_an_error_naming_it_at_declaration_and_in_a_handler() {
    let game = tempfile::tempdir().expect("tempdir");
    let err = load(
        game.path(),
        "setting_probe_b",
        "local x = horizon.setting(\"nope\")",
    )
    .err()
    .expect("refused at declaration");
    assert!(err.contains("'nope'"), "{err}");

    install_capture();
    let m = load(
        game.path(),
        "setting_probe_b",
        &on_block("horizon.setting(\"nope\")"),
    )
    .expect("loads: the handler is the one that reads it");
    let host = LuaHost::new(vec![m]);
    host.fire("block_receive_game");
    let lines = captured_with_prefix("setting_probe_b: block_receive_game:");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("'nope'"), "{lines:?}");
}

#[test]
fn a_change_reaches_the_handler_once_with_its_key_new_and_old_value() {
    let game = tempfile::tempdir().expect("tempdir");
    let m = load(
        game.path(),
        "setting_probe_c",
        "horizon.on_setting_changed(function(key, value, old)\n\
         horizon.log(key .. \"=\" .. tostring(value) .. \" from \" .. tostring(old))\n\
         end)",
    )
    .expect("loads");
    let host = LuaHost::new(vec![m]);
    host.set_setting("setting_probe_c", "level", Value::Int(4))
        .expect("set");
    host.set_setting("setting_probe_c", "on", Value::Bool(false))
        .expect("set");
    assert_eq!(
        host.mods()[0].log_lines(),
        ["level=4 from 1", "on=false from true"]
    );
}

#[test]
fn reset_all_delivers_each_moved_value_in_schema_order() {
    let game = tempfile::tempdir().expect("tempdir");
    let m = load(
        game.path(),
        "setting_probe_d",
        "horizon.on_setting_changed(function(key, value, old)\n\
         horizon.log(key .. \"=\" .. tostring(value) .. \" from \" .. tostring(old))\n\
         end)",
    )
    .expect("loads");
    let host = LuaHost::new(vec![m]);
    host.set_setting("setting_probe_d", "name", Value::Str("fox".into()))
        .expect("set");
    host.set_setting("setting_probe_d", "level", Value::Int(4))
        .expect("set");
    host.reset_all_settings("setting_probe_d").expect("reset");
    let lines = host.mods()[0].log_lines();
    assert_eq!(
        lines[2..],
        ["level=1 from 4".to_owned(), "name=wolf from fox".to_owned()]
    );
    assert_eq!(lines.len(), 4, "{lines:?}");
}

#[test]
fn a_failing_handler_logs_once_and_the_change_stays() {
    install_capture();
    let game = tempfile::tempdir().expect("tempdir");
    let m = load(
        game.path(),
        "setting_probe_e",
        "horizon.on_setting_changed(function() error(\"boom\") end)",
    )
    .expect("loads");
    let host = LuaHost::new(vec![m]);
    host.set_setting("setting_probe_e", "level", Value::Int(2))
        .expect("set");
    host.set_setting("setting_probe_e", "level", Value::Int(3))
        .expect("set");
    assert_eq!(
        host.settings("setting_probe_e")
            .expect("loaded")
            .get("level"),
        Some(Value::Int(3))
    );
    let lines = captured_with_prefix("setting_probe_e: setting_changed:");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("boom"), "{lines:?}");
}

#[test]
fn a_panicking_delivery_is_contained_and_the_handler_is_quarantined() {
    install_capture();
    let game = tempfile::tempdir().expect("tempdir");
    let m = load(
        game.path(),
        "setting_probe_f",
        "horizon.on_setting_changed(function(key) horizon.log(\"called \" .. key) end)",
    )
    .expect("loads");
    let change = Change {
        key: "level".into(),
        value: Value::Int(5),
        old: Value::Int(1),
    };
    setting_api::deliver_with(&m, &change, &|_: &Lua,
                                             _: &Change|
     -> mlua::Result<(
        LuaValue,
        LuaValue,
        LuaValue,
    )> { panic!("the delivery broke") });
    assert!(quarantine::is_quarantined(
        "setting_probe_f",
        "setting_changed"
    ));
    let lines = captured_with_prefix("setting_probe_f: setting_changed: the game's host panicked");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("the delivery broke"), "{lines:?}");

    // The quarantined handler is not called again.
    setting_api::deliver(&m, &change);
    assert!(m.log_lines().is_empty());
}

#[test]
fn on_setting_changed_is_refused_twice_and_after_load() {
    let game = tempfile::tempdir().expect("tempdir");
    let err = load(
        game.path(),
        "setting_probe_g",
        "horizon.on_setting_changed(function() end)\n\
         horizon.on_setting_changed(function() end)",
    )
    .err()
    .expect("refused");
    assert!(err.contains("may be called only once per mod"), "{err}");

    install_capture();
    let m = load(
        game.path(),
        "setting_probe_g2",
        &on_block("horizon.on_setting_changed(function() end)"),
    )
    .expect("loads");
    let host = LuaHost::new(vec![m]);
    host.fire("block_receive_game");
    let lines = captured_with_prefix("setting_probe_g2: block_receive_game:");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("can only be used while the mod loads"),
        "{lines:?}"
    );
}

#[test]
fn a_mod_without_a_handler_takes_a_set_silently() {
    let game = tempfile::tempdir().expect("tempdir");
    let m = load(game.path(), "setting_probe_h", "local x = 1").expect("loads");
    let host = LuaHost::new(vec![m]);
    host.set_setting("setting_probe_h", "level", Value::Int(9))
        .expect("set");
    host.reset_setting("setting_probe_h", "level")
        .expect("reset");
    assert_eq!(
        host.settings("setting_probe_h")
            .expect("loaded")
            .get("level"),
        Some(Value::Int(1))
    );
    assert!(host.mods()[0].log_lines().is_empty());
}

#[test]
fn a_call_from_inside_the_mods_own_turn_is_busy_and_writes_nothing() {
    let game = tempfile::tempdir().expect("tempdir");
    let m = load(game.path(), "setting_probe_i", "local x = 1").expect("loads");
    let host = LuaHost::new(vec![m]);
    let held = host.mods()[0].turn().enter();
    assert_eq!(
        host.set_setting("setting_probe_i", "level", Value::Int(6)),
        Err(SettingError::Busy)
    );
    assert_eq!(
        host.reset_setting("setting_probe_i", "level"),
        Err(SettingError::Busy)
    );
    assert_eq!(
        host.reset_all_settings("setting_probe_i"),
        Err(SettingError::Busy)
    );
    drop(held);
    assert!(!game.path().join("config").exists());
    assert_eq!(
        host.settings("setting_probe_i")
            .expect("loaded")
            .get("level"),
        Some(Value::Int(1))
    );
}

#[test]
fn the_host_hands_out_the_settings_the_mod_reads_and_names_unknown_mods() {
    let game = tempfile::tempdir().expect("tempdir");
    let m = load(game.path(), "setting_probe_j", "local x = 1").expect("loads");
    let host = LuaHost::new(vec![m]);
    let shared = host.settings("setting_probe_j").expect("loaded");
    assert!(Arc::ptr_eq(&shared, host.mods()[0].settings()));
    assert!(host.settings("setting_probe_nobody").is_none());
    assert_eq!(
        host.set_setting("setting_probe_nobody", "level", Value::Int(2)),
        Err(SettingError::UnknownMod {
            mod_id: "setting_probe_nobody".into()
        })
    );
}

#[test]
fn a_load_notice_of_the_settings_file_is_logged_once_through_report() {
    install_capture();
    let game = tempfile::tempdir().expect("tempdir");
    write_file(game.path(), "setting_probe_k", "level = \"high\"\n");
    let m = load(game.path(), "setting_probe_k", "local x = 1").expect("loads");
    let host = LuaHost::new(vec![m]);
    let text = "settings.toml: level has the wrong type; its default is used";
    assert!(captured_with_prefix(text).is_empty());
    host.report();
    host.report();
    assert_eq!(captured_with_prefix(text).len(), 1);
    assert_eq!(
        host.settings("setting_probe_k")
            .expect("loaded")
            .get("level"),
        Some(Value::Int(1))
    );
}
