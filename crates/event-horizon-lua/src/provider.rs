//! The Lua host's side of the Mods screen's provider (`horizon_hooks::mod_entries`). It answers from
//! data the host already holds: the loaded mods' manifests and settings, the ids the enabled file
//! turns off, and the refusals `builtin` kept. Reads call no Lua state. A setting write goes through
//! the mod's `Settings` (checked, written atomically) and then reaches the mod's change handler the
//! way `LuaHost::set_setting` delivers it.
//!
//! The snapshot is taken when the game loads its mods: the enabled file is read again only at the
//! next start, so [`Provider::set_enabled`] says so in the file, not in the list.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use horizon_hooks::mod_entries::keys::{
    BUSY, FILE, INVALID_VALUE, NOT_EDITABLE, UNKNOWN_MOD, UNKNOWN_SETTING,
};
use horizon_hooks::mod_entries::{
    Entry, Fault, Provider, SettingView, Side as ViewSide, Status, ViewKind, ViewValue, fault,
};

use crate::builtin::RefusedLine;
use crate::discover::Side;
use crate::enabled;
use crate::host::LuaMod;
use crate::setting_api;
use crate::settings::{Change, Kind, Setting, SettingError, SettingSide, Value};

/// The Mods screen's provider for the Lua mods of one game.
pub struct LuaProvider {
    /// The loaded mods, shared with the host (`LuaHost::shared_mods`).
    mods: Arc<[LuaMod]>,
    game_dir: PathBuf,
    /// The installed mods the enabled file turns off: (id, version).
    disabled: Vec<(String, String)>,
    /// The refused packages: the name before the reason, the reason, and the key with its args.
    refused: Vec<Refused>,
}

/// A refused package as the list shows it.
struct Refused {
    name: String,
    reason: String,
    key: String,
    args: Vec<(String, String)>,
}

impl LuaProvider {
    /// `refused` are the lines `builtin` keeps, `name: reason`, with their keys. A refused package is
    /// named by its file name, which is not always a mod id, so the list shows it as the name it was
    /// found by.
    pub fn new(
        mods: Arc<[LuaMod]>,
        game_dir: &Path,
        disabled: Vec<(String, String)>,
        refused: &[RefusedLine],
    ) -> LuaProvider {
        let refused = refused
            .iter()
            .map(|line| {
                let (name, reason) = match line.text.split_once(": ") {
                    Some((name, reason)) => (name.to_owned(), reason.to_owned()),
                    None => (line.text.clone(), line.text.clone()),
                };
                Refused {
                    name,
                    reason,
                    key: line.key.clone(),
                    args: line.args.clone(),
                }
            })
            .collect();
        LuaProvider {
            mods,
            game_dir: game_dir.to_owned(),
            disabled,
            refused,
        }
    }

    /// The loaded mod `id`.
    fn loaded(&self, id: &str) -> Result<&LuaMod, Fault> {
        self.mods
            .iter()
            .find(|m| m.id() == id)
            .ok_or_else(|| fault(UNKNOWN_MOD, &[("mod", id)]))
    }

    /// The loaded mod `id` when this thread may change its settings: the same check as
    /// `LuaHost::setting_owner`, so a call from one of the mod's own handlers does not wait on itself.
    fn owner(&self, id: &str) -> Result<&LuaMod, Fault> {
        let m = self.loaded(id)?;
        if m.turn().held_here() {
            return Err(fault(BUSY, &[("mod", id)]));
        }
        Ok(m)
    }

    fn known(&self, id: &str) -> bool {
        self.mods.iter().any(|m| m.id() == id)
            || self.disabled.iter().any(|(d, _)| d == id)
            || self.refused.iter().any(|r| r.name == id)
    }
}

impl Provider for LuaProvider {
    fn entries(&self) -> Vec<Entry> {
        let loaded = self.mods.iter().map(|m| {
            let described = m.described();
            Entry {
                id: m.id().to_owned(),
                name: described.name.clone(),
                version: m.version().to_owned(),
                authors: described.authors.clone(),
                description: described.description.clone(),
                side: mod_side(m.side()),
                enabled: true,
                status: Status::Loaded,
                settings: !m.settings().schema().settings().is_empty(),
                icon_png: m.icon().map(<[u8]>::to_vec),
            }
        });
        // The enabled file turns these off; their manifests are not read, so the id and the version
        // the scan found are known, and the name is the id.
        let disabled = self.disabled.iter().map(|(id, version)| Entry {
            id: id.clone(),
            name: id.clone(),
            version: version.clone(),
            authors: Vec::new(),
            description: String::new(),
            side: ViewSide::Both,
            enabled: false,
            status: Status::Disabled,
            settings: false,
            icon_png: None,
        });
        let refused = self.refused.iter().map(|r| Entry {
            id: r.name.clone(),
            name: r.name.clone(),
            version: String::new(),
            authors: Vec::new(),
            description: String::new(),
            side: ViewSide::Both,
            enabled: true,
            status: Status::Refused {
                key: r.key.clone(),
                args: r.args.clone(),
                reason: r.reason.clone(),
            },
            settings: false,
            icon_png: None,
        });
        loaded.chain(disabled).chain(refused).collect()
    }

    fn settings(&self, id: &str) -> Vec<SettingView> {
        let Ok(m) = self.loaded(id) else {
            return Vec::new();
        };
        m.settings()
            .schema()
            .settings()
            .iter()
            .map(|setting| setting_view(m, setting))
            .collect()
    }

    fn set(&self, id: &str, key: &str, value: ViewValue) -> Result<(), Fault> {
        let m = self.owner(id)?;
        let change = m
            .settings()
            .set(key, to_value(value))
            .map_err(|e| refusal(id, e))?;
        setting_api::deliver(m, &change);
        Ok(())
    }

    fn reset(&self, id: &str, key: &str) -> Result<(), Fault> {
        let m = self.owner(id)?;
        let change = m.settings().reset(key).map_err(|e| refusal(id, e))?;
        setting_api::deliver(m, &change);
        Ok(())
    }

    fn reset_all(&self, id: &str) -> Result<(), Fault> {
        let m = self.owner(id)?;
        let changes: Vec<Change> = m.settings().reset_all().map_err(|e| refusal(id, e))?;
        for change in &changes {
            setting_api::deliver(m, change);
        }
        Ok(())
    }

    fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), Fault> {
        if !self.known(id) {
            return Err(fault(UNKNOWN_MOD, &[("mod", id)]));
        }
        enabled::write(&self.game_dir, id, enabled)
            .map_err(|reason| fault(FILE, &[("mod", id), ("reason", reason.as_str())]))
    }
}

fn mod_side(side: Side) -> ViewSide {
    match side {
        Side::Client => ViewSide::Client,
        Side::Server => ViewSide::Server,
        Side::Both => ViewSide::Both,
    }
}

fn setting_side(side: SettingSide) -> ViewSide {
    match side {
        SettingSide::Client => ViewSide::Client,
        SettingSide::Server => ViewSide::Server,
        SettingSide::Both => ViewSide::Both,
    }
}

fn view_kind(kind: &Kind) -> ViewKind {
    match kind {
        Kind::Toggle => ViewKind::Toggle,
        Kind::Choice { options } => ViewKind::Choice {
            options: options.clone(),
        },
        Kind::Slider {
            min,
            max,
            step,
            integer,
        } => ViewKind::Slider {
            min: *min,
            max: *max,
            step: *step,
            integer: *integer,
        },
        Kind::Colour => ViewKind::Colour,
        Kind::Text { max_length } => ViewKind::Text {
            max_length: *max_length,
        },
        Kind::Keybind => ViewKind::Keybind,
    }
}

fn to_view(value: Value) -> ViewValue {
    match value {
        Value::Bool(b) => ViewValue::Bool(b),
        Value::Int(i) => ViewValue::Int(i),
        Value::Float(f) => ViewValue::Float(f),
        Value::Str(s) => ViewValue::Text(s),
    }
}

fn to_value(value: ViewValue) -> Value {
    match value {
        ViewValue::Bool(b) => Value::Bool(b),
        ViewValue::Int(i) => Value::Int(i),
        ViewValue::Float(f) => Value::Float(f),
        ViewValue::Text(s) => Value::Str(s),
    }
}

fn setting_view(m: &LuaMod, setting: &Setting) -> SettingView {
    let value = m
        .settings()
        .get(&setting.key)
        .unwrap_or_else(|| setting.default.clone());
    SettingView {
        key: setting.key.clone(),
        label: setting.label.clone(),
        description: setting.description.clone(),
        group: setting.group.clone(),
        side: setting_side(setting.side),
        kind: view_kind(&setting.kind),
        changed: value != setting.default,
        value: to_view(value),
        default: to_view(setting.default.clone()),
    }
}

/// The fault a refused `set`, `reset` or `reset_all` gives, for the mod `id`.
fn refusal(id: &str, error: SettingError) -> Fault {
    match error {
        SettingError::UnknownKey { key } => {
            fault(UNKNOWN_SETTING, &[("mod", id), ("key", key.as_str())])
        }
        SettingError::NotEditable { key } => {
            fault(NOT_EDITABLE, &[("mod", id), ("key", key.as_str())])
        }
        SettingError::Invalid { key, reason } => fault(
            INVALID_VALUE,
            &[
                ("mod", id),
                ("key", key.as_str()),
                ("reason", reason.as_str()),
            ],
        ),
        SettingError::Busy => fault(BUSY, &[("mod", id)]),
        SettingError::UnknownMod { mod_id } => fault(UNKNOWN_MOD, &[("mod", mod_id.as_str())]),
        SettingError::File(text) => fault(FILE, &[("mod", id), ("reason", text.as_str())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{self, Discovered};
    use crate::hlua::Package;
    use crate::manifest::MANIFEST;
    use crate::settings::{Role, Schema, Settings};
    use horizon_hooks::mod_entries::keys;
    use horizon_hooks::mod_entries::{ViewKind, ViewValue};

    /// Every setting type, with a default each.
    const SETTINGS: &str = "[[settings]]\nkey = \"on\"\ntype = \"toggle\"\nlabel = \"On\"\ndefault = true\n\
        [[settings]]\nkey = \"mode\"\ntype = \"choice\"\nlabel = \"Mode\"\noptions = [\"a\", \"b\"]\ndefault = \"a\"\n\
        [[settings]]\nkey = \"level\"\ntype = \"slider\"\nlabel = \"Level\"\nmin = 0\nmax = 10\nstep = 1\ninteger = true\ndefault = 1\n\
        [[settings]]\nkey = \"gain\"\ntype = \"slider\"\nlabel = \"Gain\"\nmin = 0\nmax = 1\nstep = 0.5\ninteger = false\ndefault = 0.5\n\
        [[settings]]\nkey = \"tint\"\ntype = \"colour\"\nlabel = \"Tint\"\ndefault = \"#ff0000\"\n\
        [[settings]]\nkey = \"word\"\ntype = \"text\"\nlabel = \"Word\"\nmax_length = 8\ndefault = \"hi\"\n\
        [[settings]]\nkey = \"fire\"\ntype = \"keybind\"\nlabel = \"Fire\"\ndefault = \"F\"\n";

    /// Writes a Lua mod `name` (folder) with id `id` under `root`, loads it as the loader does, with
    /// its settings read from `game_dir`.
    fn load(root: &Path, game_dir: &Path, name: &str, id: &str, settings: &str) -> LuaMod {
        load_with(root, game_dir, name, id, settings, None)
    }

    /// `load`, and with an icon file of `icon` bytes when given, named by the manifest.
    fn load_with(
        root: &Path,
        game_dir: &Path,
        name: &str,
        id: &str,
        settings: &str,
        icon: Option<&[u8]>,
    ) -> LuaMod {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let icon_line = if icon.is_some() {
            "icon = \"icon.png\"\n"
        } else {
            ""
        };
        std::fs::write(
            dir.join(MANIFEST),
            format!(
                "id = \"{id}\"\nname = \"Name of {id}\"\nversion = \"1.2.0\"\nauthors = [\"Ann\"]\n\
                 description = \"Does things\"\nlicense = \"MIT\"\nside = \"both\"\n{icon_line}{settings}"
            ),
        )
        .unwrap();
        if let Some(bytes) = icon {
            std::fs::write(dir.join("icon.png"), bytes).unwrap();
        }
        std::fs::write(
            dir.join(discover::ENTRY),
            "horizon.on(\"server_tick\", function() end)\n",
        )
        .unwrap();
        let package = Package::open_dir(&dir).unwrap();
        let found = Discovered {
            dir,
            id: id.to_owned(),
            version: "1.2.0".to_owned(),
            side: Side::Both,
        };
        let schema = Schema::parse(&package.manifest().settings).unwrap();
        let settings = Settings::load(game_dir, id, schema, Role::ServerRuns);
        LuaMod::load_package(&found, &package, Arc::new(settings), Vec::new()).unwrap()
    }

    /// A host with one loaded mod (`alpha`, six setting kinds), one turned off (`off_mod`) and one
    /// refused package (`bad.hlua`).
    fn host() -> (tempfile::TempDir, tempfile::TempDir, LuaProvider) {
        let root = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let alpha = load(root.path(), game.path(), "alpha", "alpha_mod", SETTINGS);
        let mods: Arc<[LuaMod]> = Arc::from(vec![alpha]);
        let provider = LuaProvider::new(
            mods,
            game.path(),
            vec![("off_mod".to_owned(), "0.4.0".to_owned())],
            &[RefusedLine {
                text: "bad.hlua: the package is not valid".into(),
                key: "lua_mod.refused.bad_id".into(),
                args: vec![("id".into(), "bad.hlua".into())],
            }],
        );
        (root, game, provider)
    }

    #[test]
    fn entries_show_loaded_disabled_and_refused_mods() {
        let (_root, _game, provider) = host();
        let entries = provider.entries();
        assert_eq!(entries.len(), 3);
        let alpha = &entries[0];
        assert_eq!(alpha.id, "alpha_mod");
        assert_eq!(alpha.name, "Name of alpha_mod");
        assert_eq!(alpha.version, "1.2.0");
        assert_eq!(alpha.authors, ["Ann"]);
        assert_eq!(alpha.description, "Does things");
        assert_eq!(alpha.side, ViewSide::Both);
        assert!(alpha.enabled && alpha.settings);
        assert_eq!(alpha.status, Status::Loaded);

        let off = &entries[1];
        assert_eq!(off.id, "off_mod");
        assert_eq!(off.version, "0.4.0");
        assert!(!off.enabled);
        assert_eq!(off.status, Status::Disabled);

        let bad = &entries[2];
        assert_eq!(bad.id, "bad.hlua");
        assert_eq!(
            bad.status,
            Status::Refused {
                key: "lua_mod.refused.bad_id".into(),
                args: vec![("id".into(), "bad.hlua".into())],
                reason: "the package is not valid".into(),
            }
        );
    }

    /// A PNG's signature and a little more: the provider hands the bytes on without decoding them.
    const ICON: &[u8] = b"\x89PNG\r\n\x1a\nicon bytes";

    #[test]
    fn a_mod_with_a_valid_icon_carries_its_bytes_and_one_without_has_none() {
        let root = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let iconed = load_with(
            root.path(),
            game.path(),
            "iconed",
            "iconed_mod",
            "",
            Some(ICON),
        );
        let plain = load(root.path(), game.path(), "plain", "plain_mod", "");
        let provider =
            LuaProvider::new(Arc::from(vec![iconed, plain]), game.path(), Vec::new(), &[]);
        let entries = provider.entries();
        assert_eq!(entries[0].icon_png.as_deref(), Some(ICON));
        assert_eq!(entries[1].icon_png, None);
    }

    #[test]
    fn an_icon_over_256_kib_or_missing_at_load_time_is_none_and_the_mod_still_loads() {
        let root = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let big = vec![0u8; 300 * 1024];
        let heavy = load_with(
            root.path(),
            game.path(),
            "heavy",
            "heavy_mod",
            "",
            Some(&big),
        );
        assert!(heavy.icon().is_none());
        assert_eq!(heavy.id(), "heavy_mod");

        // The icon file goes missing after the package is opened: the bounded read gives none.
        load_with(root.path(), game.path(), "gone", "gone_mod", "", Some(ICON));
        let dir = root.path().join("gone");
        let package = Package::open_dir(&dir).unwrap();
        std::fs::remove_file(dir.join("icon.png")).unwrap();
        assert_eq!(package.read_icon(), None);
    }

    #[test]
    fn settings_map_every_kind_with_its_value_and_default() {
        let (_root, _game, provider) = host();
        let views = provider.settings("alpha_mod");
        let kinds: Vec<&ViewKind> = views.iter().map(|v| &v.kind).collect();
        assert_eq!(
            kinds,
            [
                &ViewKind::Toggle,
                &ViewKind::Choice {
                    options: vec!["a".into(), "b".into()]
                },
                &ViewKind::Slider {
                    min: 0.0,
                    max: 10.0,
                    step: 1.0,
                    integer: true
                },
                &ViewKind::Slider {
                    min: 0.0,
                    max: 1.0,
                    step: 0.5,
                    integer: false
                },
                &ViewKind::Colour,
                &ViewKind::Text { max_length: 8 },
                &ViewKind::Keybind,
            ]
        );
        let level = views.iter().find(|v| v.key == "level").unwrap();
        assert_eq!(level.value, ViewValue::Int(1));
        assert_eq!(level.default, ViewValue::Int(1));
        assert!(!level.changed);
        assert_eq!(level.label, "Level");
        assert!(provider.settings("nobody").is_empty());
    }

    #[test]
    fn set_round_trips_through_the_store_and_a_bad_value_changes_nothing() {
        let (_root, game, provider) = host();
        provider
            .set("alpha_mod", "level", ViewValue::Int(7))
            .unwrap();
        let level = provider
            .settings("alpha_mod")
            .into_iter()
            .find(|v| v.key == "level")
            .unwrap();
        assert_eq!(level.value, ViewValue::Int(7));
        assert!(level.changed);
        let text =
            std::fs::read_to_string(game.path().join("config/alpha_mod/settings.toml")).unwrap();
        assert!(text.contains("level = 7"), "{text}");

        let fault = provider
            .set("alpha_mod", "level", ViewValue::Int(99))
            .unwrap_err();
        assert_eq!(fault.key, keys::INVALID_VALUE);
        let level = provider
            .settings("alpha_mod")
            .into_iter()
            .find(|v| v.key == "level")
            .unwrap();
        assert_eq!(level.value, ViewValue::Int(7));
    }

    #[test]
    fn reset_and_reset_all_return_the_defaults() {
        let (_root, _game, provider) = host();
        provider
            .set("alpha_mod", "on", ViewValue::Bool(false))
            .unwrap();
        provider
            .set("alpha_mod", "word", ViewValue::Text("yo".into()))
            .unwrap();
        provider.reset("alpha_mod", "on").unwrap();
        let on = provider
            .settings("alpha_mod")
            .into_iter()
            .find(|v| v.key == "on")
            .unwrap();
        assert_eq!(on.value, ViewValue::Bool(true));
        assert!(!on.changed);

        provider.reset_all("alpha_mod").unwrap();
        assert!(provider.settings("alpha_mod").iter().all(|v| !v.changed));
    }

    #[test]
    fn unknown_mod_and_key_give_a_fault_not_a_panic() {
        let (_root, _game, provider) = host();
        let no_mod = provider
            .set("nobody", "on", ViewValue::Bool(true))
            .unwrap_err();
        assert_eq!(no_mod.key, keys::UNKNOWN_MOD);
        assert_eq!(no_mod.args, [("mod", "nobody".to_owned())]);
        let no_key = provider
            .set("alpha_mod", "nope", ViewValue::Bool(true))
            .unwrap_err();
        assert_eq!(no_key.key, keys::UNKNOWN_SETTING);
        assert!(provider.reset("alpha_mod", "nope").is_err());
        assert!(provider.reset("nobody", "on").is_err());
        assert!(provider.reset_all("nobody").is_err());
    }

    #[test]
    fn a_refusal_keeps_its_key_and_args_for_the_screen() {
        let (_root, _game, provider) = host();
        let entries = provider.entries();
        let Status::Refused { key, args, .. } = &entries[2].status else {
            panic!("the third entry is the refused package");
        };
        assert_eq!(key, "lua_mod.refused.bad_id");
        assert_eq!(args, &[("id".to_owned(), "bad.hlua".to_owned())]);
    }

    #[test]
    fn enabling_writes_the_enabled_file_for_the_next_start() {
        let (_root, game, provider) = host();
        provider.set_enabled("off_mod", true).unwrap();
        assert!(enabled::read(game.path()).is_enabled("off_mod"));
        provider.set_enabled("alpha_mod", false).unwrap();
        assert!(!enabled::read(game.path()).is_enabled("alpha_mod"));
        let unknown = provider.set_enabled("nobody", false).unwrap_err();
        assert_eq!(unknown.key, keys::UNKNOWN_MOD);
    }

    /// The name of the variant `value` is.
    fn variant(value: &ViewValue) -> &'static str {
        match value {
            ViewValue::Bool(_) => "Bool",
            ViewValue::Int(_) => "Int",
            ViewValue::Float(_) => "Float",
            ViewValue::Text(_) => "Text",
        }
    }

    #[test]
    fn contract_each_kind_carries_its_variant() {
        // The table in `horizon_hooks::mod_entries::ViewKind`: which variant `value` and
        // `default` take for each setting of `SETTINGS`.
        let (_root, _game, provider) = host();
        let expected = [
            ("on", "Bool"),
            ("mode", "Text"),
            ("level", "Int"),
            ("gain", "Float"),
            ("tint", "Text"),
            ("word", "Text"),
            ("fire", "Text"),
        ];
        let views = provider.settings("alpha_mod");
        assert_eq!(views.len(), expected.len());
        for (view, (key, want)) in views.iter().zip(expected) {
            assert_eq!(view.key, key);
            assert_eq!(variant(&view.value), want, "{key} value");
            assert_eq!(variant(&view.default), want, "{key} default");
        }
        let tint = views.iter().find(|v| v.key == "tint").unwrap();
        assert!(matches!(&tint.value, ViewValue::Text(hex) if hex.starts_with('#')));
    }

    #[test]
    fn a_choice_is_text_and_round_trips_through_the_store() {
        let (_root, game, provider) = host();
        let mode = || {
            provider
                .settings("alpha_mod")
                .into_iter()
                .find(|v| v.key == "mode")
                .unwrap()
        };
        assert_eq!(mode().value, ViewValue::Text("a".into()));
        assert_eq!(mode().default, ViewValue::Text("a".into()));
        assert!(!mode().changed);

        provider
            .set("alpha_mod", "mode", ViewValue::Text("b".into()))
            .unwrap();
        assert_eq!(mode().value, ViewValue::Text("b".into()));
        assert!(mode().changed);
        let text =
            std::fs::read_to_string(game.path().join("config/alpha_mod/settings.toml")).unwrap();
        assert!(text.contains("mode = \"b\""), "{text}");

        let fault = provider
            .set("alpha_mod", "mode", ViewValue::Text("nope".into()))
            .unwrap_err();
        assert_eq!(fault.key, keys::INVALID_VALUE);
        assert_eq!(
            mode().value,
            ViewValue::Text("b".into()),
            "a refusal changes nothing"
        );
    }
}
