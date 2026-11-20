//! The settings schema, the file a player edits, and the provider's `set`, `reset` and `reset_all`.

use std::path::Path;
use std::sync::Arc;

use crate::host::collect_load_notices;
use crate::settings::{
    CONFIG_DIR, Change, FILE, Kind, MAX_FILE, Reason, Role, Schema, Setting, SettingError,
    SettingFault, SettingSide, Settings, Value,
};

/// The schema of one setting with the given fields after `key` and `label`.
fn one(body: &str) -> Result<Schema, SettingFault> {
    let text = format!("[[settings]]\nkey = \"k\"\nlabel = \"L\"\n{body}\n");
    schema_of(&text)
}

/// The schema of every `[[settings]]` table in `text`.
fn schema_of(text: &str) -> Result<Schema, SettingFault> {
    let table: toml::Table = text.parse().expect("test TOML");
    let tables: Vec<toml::Table> = table
        .get("settings")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_table().expect("table").clone())
                .collect()
        })
        .unwrap_or_default();
    Schema::parse(&tables)
}

fn reason(result: Result<Schema, SettingFault>) -> Reason {
    result.expect_err("refused").reason
}

fn setting_of(body: &str) -> Setting {
    one(body).expect("valid").get("k").expect("k").clone()
}

fn fixture_schema() -> Schema {
    schema_of(
        r##"
[[settings]]
key = "on"
label = "On"
type = "toggle"
default = true

[[settings]]
key = "volume"
label = "Volume"
type = "slider"
min = 0
max = 10
step = 1
integer = true
default = 3

[[settings]]
key = "ratio"
label = "Ratio"
type = "slider"
min = 0.0
max = 1.0
step = 0.25
default = 0.5

[[settings]]
key = "mode"
label = "Mode"
type = "choice"
options = ["fast", "fancy"]
default = "fast"
side = "server"

[[settings]]
key = "tint"
label = "Tint"
type = "colour"
default = "#ff8800"

[[settings]]
key = "name"
label = "Name"
type = "text"
max_length = 8
default = "steve"

[[settings]]
key = "fire"
label = "Fire"
type = "keybind"
default = "F"
"##,
    )
    .expect("fixture schema is valid")
}

/// A fresh game folder with `config/<id>/settings.toml` holding `text` (when given).
fn game_with(id: &str, text: Option<&str>) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("tempdir");
    if let Some(text) = text {
        let dir = root.path().join(CONFIG_DIR).join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), text).unwrap();
    }
    root
}

fn load_notices(root: &Path, id: &str, role: Role) -> (Settings, Vec<String>) {
    let (settings, notices) =
        collect_load_notices(|| Settings::load(root, id, fixture_schema(), role));
    (
        settings,
        notices.into_iter().map(|(_, line)| line).collect(),
    )
}

fn file_text(root: &Path, id: &str) -> String {
    std::fs::read_to_string(root.join(CONFIG_DIR).join(id).join(FILE)).unwrap()
}

// --- Schema: each refusal -------------------------------------------------------------------

#[test]
fn every_type_parses_with_its_defaults() {
    let schema = fixture_schema();
    let names: Vec<&str> = schema.settings().iter().map(|s| s.key.as_str()).collect();
    assert_eq!(
        names,
        ["on", "volume", "ratio", "mode", "tint", "name", "fire"]
    );
    assert_eq!(schema.get("on").unwrap().default, Value::Bool(true));
    assert_eq!(schema.get("volume").unwrap().default, Value::Int(3));
    assert_eq!(schema.get("ratio").unwrap().default, Value::Float(0.5));
    assert_eq!(schema.get("mode").unwrap().side, SettingSide::Server);
    assert_eq!(schema.get("tint").unwrap().side, SettingSide::Both);
    assert_eq!(
        schema.get("name").unwrap().kind,
        Kind::Text { max_length: 8 }
    );
}

#[test]
fn a_text_setting_without_max_length_gets_64() {
    let setting = setting_of("type = \"text\"\ndefault = \"x\"");
    assert_eq!(setting.kind, Kind::Text { max_length: 64 });
}

#[test]
fn a_bad_key_is_refused() {
    for key in ["Volume", "1st", "a-b", "", &"a".repeat(33)] {
        let text = format!(
            "[[settings]]\nkey = {key:?}\nlabel = \"L\"\ntype = \"toggle\"\ndefault = true\n"
        );
        assert_eq!(reason(schema_of(&text)), Reason::BadKey, "{key:?}");
    }
}

#[test]
fn a_duplicate_key_is_refused() {
    let text = "[[settings]]\nkey = \"a\"\nlabel = \"L\"\ntype = \"toggle\"\ndefault = true\n\
                [[settings]]\nkey = \"a\"\nlabel = \"L\"\ntype = \"toggle\"\ndefault = false\n";
    assert_eq!(reason(schema_of(text)), Reason::DuplicateKey);
}

#[test]
fn choice_options_must_be_two_to_thirty_two_distinct_short_strings() {
    assert_eq!(
        reason(one("type = \"choice\"\noptions = [\"a\"]\ndefault = \"a\"")),
        Reason::BadOptions
    );
    let many: Vec<String> = (0..33).map(|i| format!("\"o{i}\"")).collect();
    let body = format!(
        "type = \"choice\"\noptions = [{}]\ndefault = \"o0\"",
        many.join(", ")
    );
    assert_eq!(reason(one(&body)), Reason::BadOptions);
    assert_eq!(
        reason(one(
            "type = \"choice\"\noptions = [\"a\", \"a\"]\ndefault = \"a\""
        )),
        Reason::BadOptions
    );
    assert_eq!(
        reason(one(
            "type = \"choice\"\noptions = [\"a\", \"b\"]\ndefault = \"c\""
        )),
        Reason::BadDefault
    );
}

#[test]
fn slider_bounds_and_steps_are_checked() {
    let base = |min: &str, max: &str, step: &str, default: &str| {
        format!("type = \"slider\"\nmin = {min}\nmax = {max}\nstep = {step}\ndefault = {default}")
    };
    assert_eq!(reason(one(&base("5", "5", "1", "5"))), Reason::BadRange);
    assert_eq!(reason(one(&base("5", "1", "1", "5"))), Reason::BadRange);
    assert_eq!(reason(one(&base("0", "1", "0", "0"))), Reason::BadRange);
    assert_eq!(reason(one(&base("0", "1", "-1", "0"))), Reason::BadRange);
    assert_eq!(reason(one(&base("0", "nan", "1", "0"))), Reason::BadRange);
    assert_eq!(
        reason(one(&base("0", "1000000", "1", "0"))),
        Reason::BadRange
    );
    // 3 is not on the grid 0, 2, 4 ...
    assert_eq!(reason(one(&base("0", "10", "2", "3"))), Reason::BadDefault);
    // 11 is past the maximum.
    assert_eq!(reason(one(&base("0", "10", "1", "11"))), Reason::BadDefault);
    assert!(one(&base("0", "10", "2", "4")).is_ok());
}

#[test]
fn an_integer_slider_needs_integer_bounds() {
    assert_eq!(
        reason(one(
            "type = \"slider\"\nmin = 0.5\nmax = 10\nstep = 1\ninteger = true\ndefault = 1"
        )),
        Reason::IntegerMismatch
    );
}

#[test]
fn a_colour_must_be_lowercase_hex_of_six_or_eight_digits() {
    assert!(one("type = \"colour\"\ndefault = \"#aabbcc\"").is_ok());
    assert!(one("type = \"colour\"\ndefault = \"#aabbccdd\"").is_ok());
    for bad in ["#AABBCC", "#aabbc", "aabbcc", "#aabbccd", "#ggbbcc"] {
        let body = format!("type = \"colour\"\ndefault = {bad:?}");
        assert_eq!(reason(one(&body)), Reason::BadDefault, "{bad}");
    }
}

#[test]
fn a_text_default_must_fit_and_hold_no_control_character() {
    assert_eq!(
        reason(one("type = \"text\"\nmax_length = 3\ndefault = \"abcd\"")),
        Reason::BadDefault
    );
    assert_eq!(
        reason(one("type = \"text\"\nmax_length = 3\ndefault = \"a\\nb\"")),
        Reason::BadDefault
    );
    assert_eq!(
        reason(one("type = \"text\"\nmax_length = 0\ndefault = \"\"")),
        Reason::BadTextLength
    );
    assert_eq!(
        reason(one("type = \"text\"\nmax_length = 257\ndefault = \"\"")),
        Reason::BadTextLength
    );
}

#[test]
fn a_pattern_is_refused_by_name() {
    assert_eq!(
        reason(one(
            "type = \"text\"\ndefault = \"a\"\npattern = \"[a-z]+\""
        )),
        Reason::PatternUnsupported
    );
}

#[test]
fn an_unknown_field_or_type_is_refused() {
    assert_eq!(
        reason(one("type = \"toggle\"\ndefault = true\nmystery = 1")),
        Reason::UnknownField
    );
    // `options` belongs to a choice only.
    assert_eq!(
        reason(one(
            "type = \"toggle\"\ndefault = true\noptions = [\"a\", \"b\"]"
        )),
        Reason::UnknownField
    );
    assert_eq!(
        reason(one("type = \"dial\"\ndefault = 1")),
        Reason::UnknownType
    );
}

#[test]
fn a_missing_default_or_label_or_bad_side_is_refused() {
    assert_eq!(reason(one("type = \"toggle\"")), Reason::MissingField);
    assert_eq!(
        reason(schema_of(
            "[[settings]]\nkey = \"k\"\ntype = \"toggle\"\ndefault = true\n"
        )),
        Reason::MissingField
    );
    assert_eq!(
        reason(one(
            "type = \"toggle\"\ndefault = true\nside = \"everyone\""
        )),
        Reason::BadSide
    );
    assert_eq!(
        reason(one("type = \"toggle\"\ndefault = 1")),
        Reason::BadDefault
    );
}

#[test]
fn a_control_character_in_a_label_is_refused() {
    let text =
        "[[settings]]\nkey = \"k\"\nlabel = \"a\\u0007b\"\ntype = \"toggle\"\ndefault = true\n";
    assert_eq!(reason(schema_of(text)), Reason::BadText);
}

#[test]
fn every_schema_refusal_has_an_english_entry() {
    let text = include_str!("../../horizon-client/assets/horizon/lang/en_us.json");
    let lang: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text).unwrap();
    for reason in [
        Reason::BadKey,
        Reason::DuplicateKey,
        Reason::UnknownField,
        Reason::PatternUnsupported,
        Reason::UnknownType,
        Reason::MissingField,
        Reason::WrongType,
        Reason::BadText,
        Reason::BadSide,
        Reason::BadOptions,
        Reason::BadRange,
        Reason::IntegerMismatch,
        Reason::BadDefault,
        Reason::BadColour,
        Reason::BadTextLength,
        Reason::BadKeybind,
    ] {
        let fault = SettingFault {
            reason,
            key: "k".into(),
            field: "f".into(),
        };
        let key = fault.key();
        assert!(
            lang.get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.is_empty()),
            "en_us.json has no entry for {key}"
        );
        assert!(!fault.reason().is_empty());
    }
}

// --- Load ------------------------------------------------------------------------------------

#[test]
fn an_absent_file_gives_the_defaults_with_no_notice() {
    let root = game_with("m", None);
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert!(notices.is_empty());
    assert_eq!(settings.get("volume"), Some(Value::Int(3)));
    assert_eq!(settings.get("nope"), None);
}

#[test]
fn a_valid_file_gives_its_values() {
    let root = game_with(
        "m",
        Some("on = false\nvolume = 7\nratio = 0.75\nmode = \"fancy\"\nname = \"alex\"\n"),
    );
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(settings.get("on"), Some(Value::Bool(false)));
    assert_eq!(settings.get("volume"), Some(Value::Int(7)));
    assert_eq!(settings.get("ratio"), Some(Value::Float(0.75)));
    assert_eq!(settings.get("mode"), Some(Value::Str("fancy".into())));
    assert_eq!(settings.get("name"), Some(Value::Str("alex".into())));
}

#[test]
fn a_float_slider_takes_an_integer_as_a_float() {
    let root = game_with("m", Some("ratio = 1\n"));
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert!(notices.is_empty());
    assert_eq!(settings.get("ratio"), Some(Value::Float(1.0)));
}

#[test]
fn a_wrong_type_gives_the_default_with_one_notice_for_the_key() {
    let root = game_with("m", Some("on = \"yes\"\nvolume = 2\n"));
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert_eq!(settings.get("on"), Some(Value::Bool(true)));
    assert_eq!(settings.get("volume"), Some(Value::Int(2)));
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("on"), "{notices:?}");
}

#[test]
fn an_out_of_range_or_off_step_value_gives_the_default() {
    let root = game_with("m", Some("volume = 11\nratio = 0.3\nmode = \"slow\"\n"));
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert_eq!(settings.get("volume"), Some(Value::Int(3)));
    assert_eq!(settings.get("ratio"), Some(Value::Float(0.5)));
    assert_eq!(settings.get("mode"), Some(Value::Str("fast".into())));
    assert_eq!(notices.len(), 3, "{notices:?}");
}

#[test]
fn an_unknown_key_is_ignored_with_one_notice_for_the_file() {
    let root = game_with("m", Some("zzz = 1\nyyy = 2\nvolume = 5\n"));
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert_eq!(settings.get("volume"), Some(Value::Int(5)));
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("2 keys"), "{notices:?}");
}

#[test]
fn an_oversized_file_is_ignored_whole() {
    let big = format!("volume = 5\n# {}\n", "x".repeat(MAX_FILE as usize));
    let root = game_with("m", Some(&big));
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert_eq!(settings.get("volume"), Some(Value::Int(3)));
    assert_eq!(notices.len(), 1, "{notices:?}");
}

#[test]
fn garbage_toml_is_ignored_whole() {
    let root = game_with("m", Some("volume = = 5 ]["));
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert_eq!(settings.get("volume"), Some(Value::Int(3)));
    assert_eq!(notices.len(), 1, "{notices:?}");
}

#[cfg(unix)]
#[test]
fn a_symlinked_file_is_ignored_and_not_read() {
    let root = game_with("m", None);
    let real = root.path().join("elsewhere.toml");
    std::fs::write(&real, "volume = 9\n").unwrap();
    let dir = root.path().join(CONFIG_DIR).join("m");
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&real, dir.join(FILE)).unwrap();
    let (settings, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert_eq!(settings.get("volume"), Some(Value::Int(3)));
    assert_eq!(notices.len(), 1, "{notices:?}");
}

// --- set, reset, reset_all -------------------------------------------------------------------

fn fresh(text: Option<&str>) -> (tempfile::TempDir, Settings) {
    let root = game_with("m", text);
    let settings = Settings::load(root.path(), "m", fixture_schema(), Role::ServerRuns);
    (root, settings)
}

#[test]
fn set_persists_and_survives_a_reload() {
    let (root, settings) = fresh(None);
    let change = settings.set("volume", Value::Int(8)).expect("set");
    assert_eq!(
        change,
        Change {
            key: "volume".into(),
            value: Value::Int(8),
            old: Value::Int(3),
        }
    );
    assert_eq!(settings.get("volume"), Some(Value::Int(8)));
    let (again, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(again.get("volume"), Some(Value::Int(8)));
}

#[test]
fn reset_removes_the_key_and_reports_the_default() {
    let (root, settings) = fresh(Some("volume = 8\nname = \"x\"\n"));
    let change = settings.reset("volume").expect("reset");
    assert_eq!(change.value, Value::Int(3));
    assert_eq!(change.old, Value::Int(8));
    assert!(!file_text(root.path(), "m").contains("volume"));
    assert_eq!(file_text(root.path(), "m").matches("name").count(), 1);
    assert_eq!(settings.get("volume"), Some(Value::Int(3)));
}

#[test]
fn reset_all_removes_every_setting_and_reports_only_real_changes() {
    let (root, settings) = fresh(Some("volume = 8\nname = \"x\"\n"));
    let changes = settings.reset_all().expect("reset_all");
    let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, ["volume", "name"]);
    let (again, _) = load_notices(root.path(), "m", Role::ServerRuns);
    assert_eq!(again.get("volume"), Some(Value::Int(3)));
    assert_eq!(again.get("name"), Some(Value::Str("steve".into())));
}

#[test]
fn an_invalid_value_is_refused_and_the_file_is_untouched() {
    let (root, settings) = fresh(Some("volume = 8\n"));
    let before = file_text(root.path(), "m");
    let err = settings.set("volume", Value::Int(99)).unwrap_err();
    assert!(matches!(err, SettingError::Invalid { .. }), "{err:?}");
    assert_eq!(file_text(root.path(), "m"), before);
    assert_eq!(settings.get("volume"), Some(Value::Int(8)));
}

#[test]
fn an_unknown_key_is_refused_and_named() {
    let (_root, settings) = fresh(None);
    let err = settings.set("nope", Value::Bool(true)).unwrap_err();
    assert_eq!(err, SettingError::UnknownKey { key: "nope".into() });
    assert_eq!(err.to_string(), "unknown setting \"nope\"");
}

#[test]
fn a_server_setting_is_refused_on_a_pure_client() {
    let root = game_with("m", None);
    let settings = Settings::load(root.path(), "m", fixture_schema(), Role::PureClient);
    let err = settings
        .set("mode", Value::Str("fancy".into()))
        .unwrap_err();
    assert_eq!(err, SettingError::NotEditable { key: "mode".into() });
    // A client-side setting is still editable there.
    assert!(settings.set("on", Value::Bool(false)).is_ok());
    assert_eq!(
        load_notices(root.path(), "m", Role::PureClient)
            .0
            .get("mode"),
        Some(Value::Str("fast".into()))
    );
}

#[test]
fn an_unparsable_file_is_not_overwritten() {
    let (root, settings) = fresh(Some("volume = = 5 ]["));
    let err = settings.set("volume", Value::Int(4)).unwrap_err();
    assert!(matches!(err, SettingError::File(_)), "{err:?}");
    assert_eq!(file_text(root.path(), "m"), "volume = = 5 ][");
    assert_eq!(settings.get("volume"), Some(Value::Int(3)));
}

#[cfg(unix)]
#[test]
fn a_symlinked_file_is_not_written_through() {
    let root = game_with("m", None);
    let real = root.path().join("elsewhere.toml");
    std::fs::write(&real, "").unwrap();
    let dir = root.path().join(CONFIG_DIR).join("m");
    std::fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&real, dir.join(FILE)).unwrap();
    let settings = Settings::load(root.path(), "m", fixture_schema(), Role::ServerRuns);
    assert!(settings.set("volume", Value::Int(4)).is_err());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "");
}

#[test]
fn a_write_leaves_no_temporary_file_behind() {
    let (root, settings) = fresh(None);
    settings.set("on", Value::Bool(false)).unwrap();
    settings.set("volume", Value::Int(1)).unwrap();
    let names: Vec<String> = std::fs::read_dir(root.path().join(CONFIG_DIR).join("m"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [FILE.to_string()]);
}

#[test]
fn concurrent_setters_lose_nothing() {
    let root = game_with("m", None);
    let settings = Arc::new(Settings::load(
        root.path(),
        "m",
        fixture_schema(),
        Role::ServerRuns,
    ));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let settings = Arc::clone(&settings);
            std::thread::spawn(move || {
                let key = [
                    "on", "volume", "name", "tint", "fire", "ratio", "mode", "on",
                ][i];
                let value = match key {
                    "on" => Value::Bool(false),
                    "volume" => Value::Int(9),
                    "name" => Value::Str("kim".into()),
                    "tint" => Value::Str("#000000".into()),
                    "fire" => Value::Str("G".into()),
                    "ratio" => Value::Float(0.25),
                    _ => Value::Str("fancy".into()),
                };
                settings.set(key, value).expect("set");
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let (again, notices) = load_notices(root.path(), "m", Role::ServerRuns);
    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(again.get("volume"), Some(Value::Int(9)));
    assert_eq!(again.get("name"), Some(Value::Str("kim".into())));
    assert_eq!(again.get("tint"), Some(Value::Str("#000000".into())));
    assert_eq!(again.get("fire"), Some(Value::Str("G".into())));
    assert_eq!(again.get("ratio"), Some(Value::Float(0.25)));
    assert_eq!(again.get("mode"), Some(Value::Str("fancy".into())));
    assert_eq!(again.get("on"), Some(Value::Bool(false)));
}

#[test]
fn a_setting_is_stored_with_its_side_and_group_from_the_schema() {
    let setting =
        setting_of("type = \"toggle\"\ndefault = true\ngroup = \"audio\"\ndescription = \"d\"");
    assert_eq!(setting.group, "audio");
    assert_eq!(setting.description, "d");
    assert_eq!(setting.side, SettingSide::Both);
    assert!(matches!(setting.kind, Kind::Toggle));
}
