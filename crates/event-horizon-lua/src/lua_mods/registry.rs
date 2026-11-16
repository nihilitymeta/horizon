//! The settings of the loaded Lua mods, for the game's own screens (#64): which mods declare a
//! `settings.json`, each setting with the value `horizon.config()` gives it now, and a write or a
//! reset of one value. `discover` fills the registry from the mods it loads, and replaces what it
//! held before: a second discovery does not add to the first. A successful `/reload` of a mod
//! re-reads its `settings.json` too, and `refresh` puts the new schema in its entry (or takes the
//! entry out when the file is gone or not valid), so the Options page shows what the reload read.

use super::settings::{Change, Entry, Settings};
use super::{LuaMod, LuaSide};
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard};

/// One schema per mod id, from the mods' states (both sides of a mod share its schema).
static SCHEMAS: Mutex<Vec<Arc<Settings>>> = Mutex::new(Vec::new());

fn schemas() -> MutexGuard<'static, Vec<Arc<Settings>>> {
    SCHEMAS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A mod that declares a schema, with its settings in display order.
#[derive(Clone, Debug)]
pub struct ModView {
    pub mod_id: String,
    pub entries: Vec<SettingView>,
}

/// A declared setting and the value `horizon.config()` gives it on its side.
#[derive(Clone, Debug)]
pub struct SettingView {
    pub entry: Entry,
    pub value: Value,
}

/// Replaces the registry with the schemas of `mods`: one per mod id, the first state's.
pub fn replace(mods: &[LuaMod]) {
    let mut found: Vec<Arc<Settings>> = Vec::new();
    for s in mods.iter().filter_map(LuaMod::schema) {
        if !found.iter().any(|f| f.mod_id() == s.mod_id()) {
            found.push(s);
        }
    }
    *schemas() = found;
}

/// Sets mod `mod_id`'s entry to `schema`, the schema its state has after a reload: a new one, or
/// none, which takes the mod out of the registry. The other mods' entries stay.
pub fn refresh(mod_id: &str, schema: Option<Arc<Settings>>) {
    let mut all = schemas();
    all.retain(|s| s.mod_id() != mod_id);
    all.extend(schema);
}

/// Every mod with a schema, sorted by id, each with its settings sorted by group, order and key
/// (a setting without one sorts first). Hidden settings are listed, flagged by their entry.
pub fn mod_settings() -> Vec<ModView> {
    let found: Vec<Arc<Settings>> = schemas().clone();
    let mut out: Vec<ModView> = found
        .iter()
        .map(|s| {
            let values = [LuaSide::Server, LuaSide::Client].map(|side| s.values(side));
            let mut entries: Vec<SettingView> = s
                .entries()
                .iter()
                .map(|e| {
                    let side = match e.side {
                        LuaSide::Server => 0,
                        LuaSide::Client => 1,
                    };
                    let value = values[side]
                        .get(&e.key)
                        .cloned()
                        .unwrap_or_else(|| e.default.clone());
                    SettingView {
                        entry: e.clone(),
                        value,
                    }
                })
                .collect();
            entries.sort_by(|a, b| {
                let key = |e: &Entry| {
                    (
                        e.group.clone().unwrap_or_default(),
                        e.order.clone().unwrap_or_default(),
                        e.key.clone(),
                    )
                };
                key(&a.entry).cmp(&key(&b.entry))
            });
            ModView {
                mod_id: s.mod_id().to_owned(),
                entries,
            }
        })
        .collect();
    out.sort_by(|a, b| a.mod_id.cmp(&b.mod_id));
    out
}

/// Writes setting `key` of mod `mod_id`'s `side` (see [`Settings::set`]); `notify` is told of the
/// change once. A mod that is not in the registry has no settings.
pub fn set(
    mod_id: &str,
    side: LuaSide,
    key: &str,
    value: Value,
    notify: &mut dyn FnMut(&Change),
) -> Result<(), String> {
    settings_of(mod_id)?.set(side, key, value, notify)
}

/// Removes setting `key` of mod `mod_id`'s `side` (see [`Settings::reset`]).
pub fn reset(
    mod_id: &str,
    side: LuaSide,
    key: &str,
    notify: &mut dyn FnMut(&Change),
) -> Result<(), String> {
    settings_of(mod_id)?.reset(side, key, notify)
}

fn settings_of(mod_id: &str) -> Result<Arc<Settings>, String> {
    schemas()
        .iter()
        .find(|s| s.mod_id() == mod_id)
        .cloned()
        .ok_or_else(|| format!("{mod_id} declares no settings"))
}

#[cfg(test)]
mod tests {
    use super::super::settings::{FILE, stored_zip};
    use super::super::{LuaSide, load_states};
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    /// Serialises the tests: the registry is one per process.
    static LOCK: Mutex<()> = Mutex::new(());

    const RUBY: &str = r#"{"settings": [
      {"key": "volume", "type": "int", "default": 5, "min": 0, "max": 10, "order": "b"},
      {"key": "ratio", "type": "float", "default": 0.5, "min": 0, "max": 1},
      {"key": "name", "type": "string", "default": "steve", "order": "a"},
      {"key": "on", "type": "bool", "default": true, "hidden": true},
      {"key": "mode", "type": "choice", "default": "fast", "group": "g",
       "choices": [{"value": "fast"}, {"value": "slow"}]}
    ]}"#;

    /// A scripted mod folder with `id`, its server script and its schema when given.
    fn mod_folder(game: &Path, id: &str, schema: Option<&str>) -> PathBuf {
        let dir = game.join("mods").join(id);
        std::fs::create_dir_all(dir.join("scripts/server")).unwrap();
        std::fs::write(
            dir.join("horizon-mod.toml"),
            format!(
                "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.2\"\nside = \"both\"\n\
                 capabilities = [\"scripts\"]\nexperimental = [\"scripts\"]\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.join("scripts/server/init.lua"), "").unwrap();
        if let Some(schema) = schema {
            std::fs::write(dir.join(super::super::settings::FILE), schema).unwrap();
        }
        dir
    }

    /// A fresh game folder with mods `ruby` (schema), `moss` (schema) and `plain` (no schema),
    /// as discovery loads them from `--dev-mod` folders.
    fn discovered(name: &str) -> (PathBuf, Vec<LuaMod>) {
        let game = std::env::temp_dir().join(format!(
            "horizon-settings-registry-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&game);
        let ruby = mod_folder(&game, "ruby", Some(RUBY));
        let moss = mod_folder(
            &game,
            "moss",
            Some(r#"{"settings": [{"key": "speed", "type": "int", "default": 3}]}"#),
        );
        let plain = mod_folder(&game, "plain", None);
        let args = [
            "--game-dir".to_owned(),
            game.display().to_string(),
            "--dev-mod".to_owned(),
            plain.display().to_string(),
            "--dev-mod".to_owned(),
            ruby.display().to_string(),
            "--dev-mod".to_owned(),
            moss.display().to_string(),
        ];
        let mods = load_states(args.into_iter());
        (game, mods)
    }

    fn keys(view: &ModView) -> Vec<&str> {
        view.entries.iter().map(|e| e.entry.key.as_str()).collect()
    }

    #[test]
    fn settings_registry_lists_the_schema_mods_sorted_with_effective_values() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (game, mods) = discovered("list");
        std::fs::create_dir_all(game.join("config")).unwrap();
        // volume is valid, name is refused (a number), mode is a valid choice, ratio is absent.
        std::fs::write(
            game.join("config/ruby.json"),
            r#"{"volume": 7, "name": 5, "mode": "slow"}"#,
        )
        .unwrap();
        replace(&mods);
        let listed = mod_settings();
        // `plain` has no schema, so it is absent; the two others sort by id.
        assert_eq!(
            listed.iter().map(|m| m.mod_id.as_str()).collect::<Vec<_>>(),
            ["moss", "ruby"]
        );
        let ruby = &listed[1];
        // Group first (none before "g"), then order ("" before "a" before "b"), then key.
        assert_eq!(keys(ruby), ["on", "ratio", "name", "volume", "mode"]);
        let value = |key: &str| {
            ruby.entries
                .iter()
                .find(|e| e.entry.key == key)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(value("volume"), json!(7), "the stored value");
        assert_eq!(
            value("name"),
            json!("steve"),
            "a refused stored value gives the default"
        );
        assert_eq!(
            value("ratio"),
            json!(0.5),
            "an absent value gives the default"
        );
        assert_eq!(value("mode"), json!("slow"));
        assert!(ruby.entries[0].entry.hidden, "hidden entries are flagged");
        assert_eq!(
            listed[0].entries[0].value,
            json!(3),
            "moss gets its default"
        );
        std::fs::remove_dir_all(&game).unwrap();
    }

    #[test]
    fn settings_registry_set_writes_and_tells_once_and_refuses_bad_values() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (game, mods) = discovered("set");
        std::fs::create_dir_all(game.join("config")).unwrap();
        std::fs::write(game.join("config/ruby.json"), r#"{"volume": 7}"#).unwrap();
        replace(&mods);
        let file = game.join("config/ruby.json");
        let mut told: Vec<Change> = Vec::new();

        set("ruby", LuaSide::Server, "volume", json!(9), &mut |c| {
            told.push(c.clone())
        })
        .unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["volume"], json!(9));
        assert_eq!(
            told,
            [Change {
                mod_id: "ruby".to_owned(),
                key: "volume".to_owned(),
                old: json!(7),
                new: json!(9),
            }]
        );

        let before = std::fs::read_to_string(&file).unwrap();
        let err = set("ruby", LuaSide::Server, "volume", json!(99), &mut |c| {
            told.push(c.clone())
        })
        .unwrap_err();
        assert!(err.contains("volume"), "the error names the key: {err}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            before,
            "nothing written"
        );
        assert_eq!(told.len(), 1, "nothing told");

        assert!(set("plain", LuaSide::Server, "volume", json!(1), &mut |_| {}).is_err());
        std::fs::remove_dir_all(&game).unwrap();
    }

    #[test]
    fn settings_registry_reset_removes_the_key_and_replace_does_not_append() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (game, mods) = discovered("reset");
        std::fs::create_dir_all(game.join("config")).unwrap();
        std::fs::write(game.join("config/ruby.json"), r#"{"volume": 9}"#).unwrap();
        replace(&mods);
        let mut told: Vec<Change> = Vec::new();
        reset("ruby", LuaSide::Server, "volume", &mut |c| {
            told.push(c.clone())
        })
        .unwrap();
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(game.join("config/ruby.json")).unwrap())
                .unwrap();
        assert!(saved.get("volume").is_none(), "the key is removed");
        assert_eq!(told.len(), 1);
        assert_eq!(
            (told[0].old.clone(), told[0].new.clone()),
            (json!(9), json!(5))
        );
        let volume = mod_settings()[1]
            .entries
            .iter()
            .find(|e| e.entry.key == "volume")
            .unwrap()
            .value
            .clone();
        assert_eq!(volume, json!(5), "the default again");

        // Replacing with none empties the registry; replacing again with the same two gives two, not four.
        replace(&[]);
        assert!(mod_settings().is_empty());
        replace(&mods);
        assert_eq!(mod_settings().len(), 2);
        std::fs::remove_dir_all(&game).unwrap();
    }

    const RUBY_MANIFEST: &str = "id = \"ruby\"\nname = \"ruby\"\nversion = \"0.1.0\"\napi = \"0.2\"\n\
         side = \"both\"\ncapabilities = [\"scripts\"]\nexperimental = [\"scripts\"]\n";

    /// The keys the registry lists for mod `mod_id`, None when it is not listed.
    fn keys_of(mod_id: &str) -> Option<Vec<String>> {
        mod_settings()
            .into_iter()
            .find(|m| m.mod_id == mod_id)
            .map(|m| m.entries.into_iter().map(|e| e.entry.key).collect())
    }

    #[test]
    fn settings_registry_lists_a_packaged_mod_with_its_declared_defaults() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let game = std::env::temp_dir().join(format!(
            "horizon-settings-registry-{}-packaged",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&game);
        std::fs::create_dir_all(&game).unwrap();
        let packed = game.join("ruby.hmod");
        std::fs::write(
            &packed,
            stored_zip(&[
                ("horizon-mod.toml", RUBY_MANIFEST),
                ("scripts/server/init.lua", "-- settings only"),
                (FILE, RUBY),
            ]),
        )
        .unwrap();
        let args = [
            "--game-dir".to_owned(),
            game.display().to_string(),
            "--dev-mod".to_owned(),
            packed.display().to_string(),
        ];
        replace(&load_states(args.into_iter()));
        let listed = mod_settings();
        assert_eq!(
            listed.iter().map(|m| m.mod_id.as_str()).collect::<Vec<_>>(),
            ["ruby"]
        );
        assert_eq!(
            keys_of("ruby").unwrap(),
            ["on", "ratio", "name", "volume", "mode"]
        );
        let volume = listed[0]
            .entries
            .iter()
            .find(|e| e.entry.key == "volume")
            .unwrap();
        assert_eq!(
            volume.value,
            json!(5),
            "no saved file: the declared default"
        );
        replace(&[]);
        std::fs::remove_dir_all(&game).unwrap();
    }

    #[test]
    fn settings_registry_reload_refreshes_the_listed_schema() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (game, mods) = discovered("reload");
        replace(&mods);
        let ruby = mods.iter().find(|m| m.id() == "ruby").unwrap();
        let plain = mods.iter().find(|m| m.id() == "plain").unwrap();
        let mod_dir = |id: &str| game.join("mods").join(id);
        assert_eq!(keys_of("plain"), None);

        // ruby's file changes: the listing shows the new schema after its reload.
        std::fs::write(
            mod_dir("ruby").join(FILE),
            r#"{"settings": [{"key": "gain", "type": "int", "default": 4}]}"#,
        )
        .unwrap();
        ruby.reload().unwrap();
        assert_eq!(keys_of("ruby"), Some(vec!["gain".to_owned()]));
        assert!(keys_of("moss").is_some(), "the other mods stay listed");

        // plain had no schema: a reload that finds one lists it.
        std::fs::write(
            mod_dir("plain").join(FILE),
            r#"{"settings": [{"key": "level", "type": "int", "default": 2}]}"#,
        )
        .unwrap();
        plain.reload().unwrap();
        assert_eq!(keys_of("plain"), Some(vec!["level".to_owned()]));

        // ruby's file goes: a reload takes it out, and a write to it is refused.
        std::fs::remove_file(mod_dir("ruby").join(FILE)).unwrap();
        ruby.reload().unwrap();
        assert_eq!(keys_of("ruby"), None);
        assert!(set("ruby", LuaSide::Server, "gain", json!(1), &mut |_| {}).is_err());
        assert!(keys_of("moss").is_some());
        replace(&[]);
        std::fs::remove_dir_all(&game).unwrap();
    }
}
