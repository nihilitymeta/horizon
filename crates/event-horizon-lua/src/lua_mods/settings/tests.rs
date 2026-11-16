use super::super::{LuaMod, LuaSide};
use super::*;
use crate::lua_mods::json::lua_to_json;
use horizon_client::mod_api::{Hooks, dispatch_config_changed};
use serde_json::json;

const MANIFEST: &str = r#"
id = "ruby"
name = "Ruby"
version = "0.1.0"
api = "0.2"
side = "both"
capabilities = ["scripts"]
experimental = ["scripts"]
"#;

const SCHEMA: &str = r#"{"settings": [
  {"key": "volume", "type": "int", "default": 5, "min": 0, "max": 10, "group": "audio", "order": "a"},
  {"key": "ratio", "type": "float", "default": 0.5, "min": 0, "max": 1},
  {"key": "name", "type": "string", "default": "steve"},
  {"key": "on", "type": "bool", "default": true, "hidden": true},
  {"key": "mode", "type": "choice", "default": "fast", "choices": [{"value": "fast"}, {"value": "slow"}]},
  {"key": "hud.scale", "type": "int", "default": 2, "side": "client"}
]}"#;

/// A mod folder: its manifest, its server script, and its settings file when given.
fn folder(name: &str, schema: Option<&str>, script: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("horizon-settings-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts/server")).unwrap();
    std::fs::write(dir.join("horizon-mod.toml"), MANIFEST).unwrap();
    std::fs::write(dir.join("scripts/server/init.lua"), script).unwrap();
    if let Some(schema) = schema {
        std::fs::write(dir.join(FILE), schema).unwrap();
    }
    dir
}

fn load(dir: &Path) -> LuaMod {
    LuaMod::load_with(dir, LuaSide::Server, &[], Some(&dir.join("config"))).unwrap()
}

/// The table `horizon.config()` gave the script at load, as JSON.
fn config_seen(m: &LuaMod) -> serde_json::Value {
    lua_to_json(&m.lua.globals().get::<mlua::Value>("CFG").unwrap()).unwrap()
}

fn saved(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("config/ruby.json")).unwrap()).unwrap()
}

#[test]
fn settings_schema_entries_come_in_declared_order() {
    let entries = parse(SCHEMA, "ruby").unwrap();
    let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["volume", "ratio", "name", "on", "mode", "hud.scale"]);
    assert_eq!(entries[0].group.as_deref(), Some("audio"));
    assert_eq!(entries[0].side, LuaSide::Server, "side defaults to server");
    assert!(entries[3].hidden);
    assert_eq!(entries[4].choices, ["fast", "slow"]);
    assert_eq!(entries[5].side, LuaSide::Client);
}

#[test]
fn settings_bad_entries_are_skipped_and_the_good_ones_stay() {
    let text = r#"{"settings": [
  {"key": "good", "type": "int", "default": 1},
  {"key": "Upper", "type": "int", "default": 1},
  {"key": "kind", "type": "color", "default": "red"},
  {"key": "nodefault", "type": "bool"},
  {"key": "range", "type": "int", "default": 11, "max": 10},
  {"key": "frac", "type": "int", "default": 1.5},
  {"key": "pick", "type": "choice", "default": "x", "choices": [{"value": "a"}]},
  {"key": "good", "type": "bool", "default": false},
  {"key": "last", "type": "string", "default": "ok"}
]}"#;
    let entries = parse(text, "ruby").unwrap();
    let keys: Vec<(&str, Kind)> = entries.iter().map(|e| (e.key.as_str(), e.kind)).collect();
    assert_eq!(keys, [("good", Kind::Int), ("last", Kind::Str)]);
}

#[test]
fn settings_a_file_that_is_not_json_gives_no_schema() {
    assert!(parse("{not json", "ruby").is_err());
    let dir = folder("bad_file", Some("{not json"), "");
    assert!(read(&dir, "ruby").is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn config_fills_defaults_and_refuses_values_that_do_not_fit() {
    let dir = folder("config_fill", Some(SCHEMA), "CFG = horizon.config()");
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(
        dir.join("config/ruby.json"),
        r#"{"volume": 11, "ratio": 0.25, "name": 7, "on": "yes", "mode": "medium", "extra": [1, 2]}"#,
    )
    .unwrap();
    let m = load(&dir);
    assert_eq!(
        config_seen(&m),
        json!({"volume": 5, "ratio": 0.25, "name": "steve", "on": true, "mode": "fast", "extra": [1, 2]}),
        "a stored value that does not fit gives its default; unknown keys stay"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn config_without_a_schema_returns_the_file_as_it_is() {
    let dir = folder("config_plain", None, "CFG = horizon.config()");
    std::fs::create_dir_all(dir.join("config")).unwrap();
    let stored = json!({"limit": 3, "kinds": ["a"], "volume": 99});
    std::fs::write(dir.join("config/ruby.json"), stored.to_string()).unwrap();
    let m = load(&dir);
    assert!(m.schema().is_none());
    assert_eq!(config_seen(&m), stored);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_set_writes_one_key_and_keeps_the_others() {
    let dir = folder("set_one", Some(SCHEMA), "CFG = horizon.config()");
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(
        dir.join("config/ruby.json"),
        r#"{"name": "alex", "extra": 1}"#,
    )
    .unwrap();
    let m = load(&dir);
    let mut changes = Vec::new();
    m.set_setting(LuaSide::Server, "volume", json!(7), &mut |c| {
        changes.push(c.clone())
    })
    .unwrap();
    assert_eq!(
        saved(&dir),
        json!({"name": "alex", "extra": 1, "volume": 7})
    );
    assert_eq!(
        changes,
        [Change {
            mod_id: "ruby".into(),
            key: "volume".into(),
            old: json!(5),
            new: json!(7),
        }],
        "old is what config() gave before the write: the default"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_set_refuses_a_bad_value_and_writes_nothing() {
    let dir = folder("set_refused", Some(SCHEMA), "CFG = horizon.config()");
    std::fs::create_dir_all(dir.join("config")).unwrap();
    let before = r#"{"name": "alex"}"#;
    std::fs::write(dir.join("config/ruby.json"), before).unwrap();
    let m = load(&dir);
    let cases = [
        ("volume", json!("loud")),
        ("volume", json!(11)),
        ("volume", json!(2.5)),
        ("mode", json!("medium")),
        ("nope", json!(1)),
        ("hud.scale", json!(3)),
    ];
    for (key, value) in cases {
        let mut told = 0;
        let result = m.set_setting(LuaSide::Server, key, value.clone(), &mut |_| told += 1);
        assert!(result.is_err(), "{key} = {value} should be refused");
        assert_eq!(told, 0, "{key}: a refused write tells nobody");
        assert_eq!(
            std::fs::read_to_string(dir.join("config/ruby.json")).unwrap(),
            before,
            "{key} = {value}: the file is unchanged"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_set_the_same_value_writes_and_tells_nothing() {
    let dir = folder("set_same", Some(SCHEMA), "CFG = horizon.config()");
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(dir.join("config/ruby.json"), r#"{"volume": 7}"#).unwrap();
    let m = load(&dir);
    let mut told = 0;
    m.set_setting(LuaSide::Server, "volume", json!(7), &mut |_| told += 1)
        .unwrap();
    assert_eq!(told, 0);
    assert_eq!(saved(&dir), json!({"volume": 7}));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_reset_removes_the_key_and_config_gives_the_default() {
    let dir = folder("reset", Some(SCHEMA), "CFG = horizon.config()");
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(dir.join("config/ruby.json"), r#"{"volume": 9, "x": 1}"#).unwrap();
    let m = load(&dir);
    let mut changes = Vec::new();
    m.reset_setting(LuaSide::Server, "volume", &mut |c| changes.push(c.clone()))
        .unwrap();
    assert_eq!(
        saved(&dir),
        json!({"x": 1}),
        "the key is removed, the rest kept"
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (changes[0].old.clone(), changes[0].new.clone()),
        (json!(9), json!(5))
    );
    let m = load(&dir);
    assert_eq!(
        config_seen(&m)["volume"],
        5,
        "config() gives the default again"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn config_changed_hook_is_told_once_per_real_change() {
    let dir = folder(
        "hook",
        Some(SCHEMA),
        r#"
        CHANGES = {}
        horizon.on("config_changed", function(e)
          CHANGES[#CHANGES + 1] = e.mod_id .. " " .. e.key .. " " .. tostring(e.old) .. " " .. tostring(e.new)
        end)
        "#,
    );
    let m = load(&dir);
    let lua = m.lua.clone();
    let mut changes = Vec::new();
    {
        let mut collect = |c: &Change| changes.push(c.clone());
        m.set_setting(LuaSide::Server, "volume", json!(7), &mut collect)
            .unwrap();
        m.set_setting(LuaSide::Server, "volume", json!(7), &mut collect)
            .unwrap();
        m.reset_setting(LuaSide::Server, "volume", &mut collect)
            .unwrap();
        m.reset_setting(LuaSide::Server, "volume", &mut collect)
            .unwrap();
    }
    assert_eq!(changes.len(), 2, "one per real change");
    let mods: Vec<Box<dyn Hooks>> = vec![Box::new(m)];
    for c in &changes {
        dispatch_config_changed(&mods, &c.mod_id, &c.key, &c.old, &c.new);
    }
    let told: Vec<String> = lua
        .globals()
        .get::<mlua::Table>("CHANGES")
        .unwrap()
        .sequence_values()
        .map(|v| v.unwrap())
        .collect();
    assert_eq!(told, ["ruby volume 5 7", "ruby volume 7 5"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What `horizon.config()` gives now, by a fresh call (not the global the script set at load).
fn config_now(m: &LuaMod) -> serde_json::Value {
    let value: mlua::Value = m.lua.load("return horizon.config()").eval().unwrap();
    lua_to_json(&value).unwrap()
}

#[test]
fn settings_packaged_mod_reads_its_root_schema_as_its_folder_does() {
    let dir = folder("packaged", Some(SCHEMA), "CFG = horizon.config()");
    let packed = std::env::temp_dir().join(format!(
        "horizon-settings-{}-packaged.hmod",
        std::process::id()
    ));
    std::fs::write(
        &packed,
        stored_zip(&[
            ("horizon-mod.toml", MANIFEST),
            ("scripts/server/init.lua", "CFG = horizon.config()"),
            (FILE, SCHEMA),
        ]),
    )
    .unwrap();
    let from_folder = read(&dir, "ruby").unwrap();
    let from_package = read(&packed, "ruby").unwrap();
    assert_eq!(from_package, from_folder, "the same entries as the folder");
    assert_eq!(from_package.len(), 6);
    let m = LuaMod::load_with(&packed, LuaSide::Server, &[], Some(&dir.join("config"))).unwrap();
    assert_eq!(
        config_now(&m),
        json!({"volume": 5, "ratio": 0.5, "name": "steve", "on": true, "mode": "fast"}),
        "the declared defaults, the client-side hud.scale left out"
    );
    std::fs::remove_file(&packed).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_reload_reads_the_edited_schema() {
    let dir = folder("reload_edit", Some(SCHEMA), "CFG = horizon.config()");
    let m = load(&dir);
    assert_eq!(config_now(&m)["volume"], 5);
    std::fs::write(
        dir.join(FILE),
        r#"{"settings": [
          {"key": "volume", "type": "int", "default": 8, "min": 0, "max": 10},
          {"key": "added", "type": "int", "default": 3}
        ]}"#,
    )
    .unwrap();
    m.reload().unwrap();
    assert_eq!(config_now(&m), json!({"volume": 8, "added": 3}));
    m.set_setting(LuaSide::Server, "added", json!(4), &mut |_| {})
        .unwrap();
    assert_eq!(saved(&dir), json!({"added": 4}));
    assert!(
        m.set_setting(LuaSide::Server, "name", json!("alex"), &mut |_| {})
            .is_err(),
        "name is no longer in the schema, so it is refused"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_reload_without_the_file_drops_the_schema() {
    let dir = folder("reload_gone", Some(SCHEMA), "CFG = horizon.config()");
    let m = load(&dir);
    std::fs::remove_file(dir.join(FILE)).unwrap();
    m.reload().unwrap();
    assert!(m.schema().is_none());
    assert_eq!(config_now(&m), json!({}), "no schema, no saved values");
    assert!(
        m.set_setting(LuaSide::Server, "volume", json!(7), &mut |_| {})
            .is_err()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_reload_with_an_invalid_file_succeeds_without_a_schema() {
    let dir = folder("reload_invalid", Some(SCHEMA), "CFG = horizon.config()");
    let m = load(&dir);
    std::fs::write(dir.join(FILE), "{not json").unwrap();
    m.reload()
        .expect("a bad settings.json never stops a reload");
    assert!(m.schema().is_none());
    assert_eq!(config_now(&m), json!({}));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_failed_reload_keeps_the_schema_it_had() {
    let dir = folder("reload_failed", Some(SCHEMA), "CFG = horizon.config()");
    let m = load(&dir);
    std::fs::write(
        dir.join(FILE),
        r#"{"settings": [{"key": "volume", "type": "int", "default": 8}]}"#,
    )
    .unwrap();
    // The script does not compile, so the reload fails with the scripts beside the old schema.
    std::fs::write(dir.join("scripts/server/init.lua"), "CFG = horizon.config(").unwrap();
    assert!(m.reload().is_err());
    assert_eq!(config_now(&m)["volume"], 5, "the old schema's default");
    assert_eq!(m.schema().unwrap().entries().len(), 6);
    std::fs::remove_dir_all(&dir).unwrap();
}
