//! The Lua host's side of the Mods screen's provider (`horizon_hooks::mod_entries`). It answers from
//! data the host already holds: the loaded mods' manifests and settings, the ids the enabled file
//! turns off, and the refusals `builtin` kept. Reads call no Lua state. A setting write goes through
//! the mod's `Settings` (checked, written atomically) and then reaches the mod's change handler the
//! way `LuaHost::set_setting` delivers it.
//!
//! The snapshot is taken when the game loads its mods: the enabled file is read again only at the
//! next start, so [`Provider::set_enabled`] says so in the file, not in the list.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use horizon_hooks::mod_entries::keys::{
    BUSY, FILE, INVALID_VALUE, NOT_EDITABLE, POWER_NOT_REQUESTED, POWER_UNAVAILABLE, POWER_UNKNOWN,
    UNKNOWN_MOD, UNKNOWN_SETTING,
};
use horizon_hooks::mod_entries::{
    Entry, Fault, PowerView, Provider, SettingView, Side as ViewSide, Status, ViewKind, ViewValue,
    fault,
};

use crate::builtin::RefusedLine;
use crate::discover::Side;
use crate::enabled;
use crate::host::LuaMod;
use crate::manifest::Power;
use crate::powers;
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
    /// The powers each mod's manifest requests, by id (`discover::Scan::requested`). A mod with no
    /// entry requests none.
    requested: HashMap<String, Vec<Power>>,
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
        requested: HashMap<String, Vec<Power>>,
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
            requested,
        }
    }

    /// The powers mod `id` requests, in rank order; empty when it requests none or is unknown.
    fn requested_of(&self, id: &str) -> Vec<Power> {
        let mut powers = self.requested.get(id).cloned().unwrap_or_default();
        powers.sort_by_key(|p| p.rank());
        powers
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
                powers: !self.requested_of(m.id()).is_empty(),
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
            powers: !self.requested_of(id).is_empty(),
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
            powers: !self.requested_of(&r.name).is_empty(),
            icon_png: None,
        });
        loaded.chain(disabled).chain(refused).collect()
    }

    fn lookup(&self, index: usize) -> Option<(String, String)> {
        // The order of `entries`, without cloning any icon.
        let loaded = self
            .mods
            .iter()
            .map(|m| (m.id().to_owned(), m.described().name.clone()));
        let disabled = self.disabled.iter().map(|(id, _)| (id.clone(), id.clone()));
        let refused = self
            .refused
            .iter()
            .map(|r| (r.name.clone(), r.name.clone()));
        loaded.chain(disabled).chain(refused).nth(index)
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

    /// The powers mod `id` requests, in rank order, each with whether the file grants it now (read
    /// from the file, so a change shows at once). Empty for an unknown mod.
    fn powers(&self, id: &str) -> Vec<PowerView> {
        if !self.known(id) {
            return Vec::new();
        }
        let granted = enabled::read(&self.game_dir).granted(id);
        self.requested_of(id)
            .into_iter()
            .map(|power| {
                let reserved = powers::is_reserved(power);
                PowerView {
                    name: power.name().to_owned(),
                    label_key: format!("{}.label", power.label_key()),
                    description_key: format!("{}.description", power.label_key()),
                    requested: true,
                    granted: granted.contains(&power),
                    available: !reserved && powers::has_ops(power),
                    reserved,
                }
            })
            .collect()
    }

    /// Grants or revokes `power` for mod `id` in the enabled file. The change takes effect at the next
    /// start: a loaded mod keeps its powers until then, and the screen says so.
    ///
    /// A refusal writes nothing. Both directions are refused for an unknown mod and a name that is not
    /// a power. Granting is also refused for a power the mod does not request (so the screen can never
    /// write an inert grant), and for a reserved power or one with no write behind it in this build.
    /// Revoking is always allowed for a known mod and a real power name: it never adds privilege, and a
    /// dormant grant must not wake up when a later version requests the power. A revoke that finds no
    /// grant to remove succeeds without writing (and without creating the file), unless the file cannot
    /// be read, which is the file fault.
    fn set_power(&self, id: &str, power: &str, granted: bool) -> Result<(), Fault> {
        if !self.known(id) {
            return Err(fault(UNKNOWN_MOD, &[("mod", id)]));
        }
        let Some(p) = Power::from_name(power) else {
            return Err(fault(POWER_UNKNOWN, &[("mod", id), ("power", power)]));
        };
        if granted {
            if !self.requested_of(id).contains(&p) {
                return Err(fault(POWER_NOT_REQUESTED, &[("mod", id), ("power", power)]));
            }
            if powers::is_reserved(p) || !powers::has_ops(p) {
                return Err(fault(POWER_UNAVAILABLE, &[("mod", id), ("power", power)]));
            }
        } else {
            let state = enabled::read(&self.game_dir);
            // A clean file with no such grant has nothing to remove: leave it alone. Anything the reader
            // ignored goes through the writer, which refuses an unreadable file with the file fault.
            if state.notice.is_none() && !state.granted(id).contains(&p) {
                return Ok(());
            }
        }
        enabled::grant(&self.game_dir, id, p, granted)
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
            HashMap::new(),
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

    #[test]
    fn lookup_gives_the_id_and_name_of_each_entry_without_its_icon() {
        let (_root, _game, provider) = host();
        let entries = provider.entries();
        assert_eq!(entries.len(), 3, "a loaded, a disabled and a refused mod");
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                provider.lookup(i),
                Some((entry.id.clone(), entry.name.clone()))
            );
        }
        assert_eq!(provider.lookup(entries.len()), None);
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
        let provider = LuaProvider::new(
            Arc::from(vec![iconed, plain]),
            game.path(),
            Vec::new(),
            &[],
            HashMap::new(),
        );
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

    /// The provider's read calls with 50 mods whose icons are the largest the reader keeps
    /// (`MAX_ICON`): `entries` clones every icon, `lookup` and `settings` clone none. Prints the
    /// averages (`--nocapture`) and asserts generous bounds.
    #[test]
    fn provider_reads_with_fifty_max_size_icons_are_timed() {
        const REPS: u32 = 200;
        let root = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let mut icon = ICON.to_vec();
        icon.resize(crate::hlua::MAX_ICON as usize, 0x5a);
        let mods: Vec<LuaMod> = (0..50)
            .map(|i| {
                load_with(
                    root.path(),
                    game.path(),
                    &format!("m{i:02}"),
                    &format!("mod_{i:02}"),
                    "",
                    Some(&icon),
                )
            })
            .collect();
        let provider = LuaProvider::new(
            Arc::from(mods),
            game.path(),
            Vec::new(),
            &[],
            HashMap::new(),
        );
        let entries = provider.entries();
        assert_eq!(entries.len(), 50);
        assert!(
            entries
                .iter()
                .all(|e| e.icon_png.as_ref().map(Vec::len) == Some(icon.len()))
        );

        let start = std::time::Instant::now();
        for _ in 0..REPS {
            std::hint::black_box(provider.entries());
        }
        let entries_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(REPS);

        let start = std::time::Instant::now();
        for _ in 0..REPS {
            for i in 0..50 {
                std::hint::black_box(provider.lookup(i));
            }
        }
        let lookup_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(REPS * 50);

        let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
        let start = std::time::Instant::now();
        for _ in 0..REPS {
            for id in &ids {
                std::hint::black_box(provider.settings(id));
            }
        }
        let settings_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(REPS * 50);

        eprintln!(
            "provider (50 mods, {} byte icons, debug): entries() {entries_ms:.3} ms per call; \
             lookup(i) {lookup_us:.2} us per call; settings(id) {settings_us:.2} us per call",
            icon.len()
        );
        assert!(entries_ms < 100.0, "entries() takes {entries_ms:.2} ms");
        assert!(lookup_us < 100.0, "lookup() takes {lookup_us:.2} us");
        assert!(
            entries_ms * 1000.0 >= 20.0 * lookup_us,
            "lookup ({lookup_us:.2} us) is not 20x faster than entries ({entries_ms:.3} ms)"
        );
    }

    /// Installs a Lua mod in folder `folder` of `root` with id `id` and the manifest powers `powers`.
    fn install(root: &Path, folder: &str, id: &str, powers: &[&str]) {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let names: Vec<String> = powers.iter().map(|p| format!("\"{p}\"")).collect();
        std::fs::write(
            dir.join(MANIFEST),
            format!(
                "id = \"{id}\"\nname = \"Name of {id}\"\nversion = \"1.2.0\"\nauthors = [\"Ann\"]\n\
                 description = \"Does things\"\nlicense = \"MIT\"\nside = \"both\"\npowers = [{}]\n",
                names.join(", ")
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join(discover::ENTRY),
            "horizon.on(\"server_tick\", function() end)\n",
        )
        .unwrap();
    }

    /// A game folder with `alpha_mod` (requests world, commands, files; world granted), `gamma_mod`
    /// (requests world), `beta_mod` (requests nothing) and `off_mod` (requests world; turned off).
    fn game_with_powers() -> tempfile::TempDir {
        let game = tempfile::tempdir().unwrap();
        let lua = game.path().join(discover::DIR);
        install(&lua, "alpha", "alpha_mod", &["world", "commands", "files"]);
        install(&lua, "gamma", "gamma_mod", &["world"]);
        install(&lua, "beta", "beta_mod", &[]);
        install(&lua, "off", "off_mod", &["world"]);
        std::fs::write(
            lua.join(enabled::FILE),
            "[mods]\noff_mod = false\n\n[grants]\nalpha_mod = [\"world\"]\n",
        )
        .unwrap();
        game
    }

    /// The provider the game builds for `game` (and the dev paths `dev`): the real load.
    fn provider_for(game: &Path, dev: &[PathBuf]) -> LuaProvider {
        let (mods, refused, disabled, _, _, _, requested) =
            crate::builtin::load_mods(game, dev, false);
        LuaProvider::new(Arc::from(mods), game, disabled, &refused, requested)
    }

    fn enabled_file(game: &Path) -> PathBuf {
        game.join(discover::DIR).join(enabled::FILE)
    }

    /// The enabled file's bytes, or `None` when there is no file.
    fn snapshot(game: &Path) -> Option<Vec<u8>> {
        std::fs::read(enabled_file(game)).ok()
    }

    fn power_names(list: &[PowerView]) -> Vec<&str> {
        list.iter().map(|v| v.name.as_str()).collect()
    }

    fn view<'a>(list: &'a [PowerView], name: &str) -> &'a PowerView {
        list.iter().find(|v| v.name == name).unwrap()
    }

    #[test]
    fn powers_list_the_requested_powers_in_rank_order_with_their_flags() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        let list = provider.powers("alpha_mod");
        assert_eq!(power_names(&list), ["commands", "world", "files"]);
        let commands = view(&list, "commands");
        assert!(
            commands.requested && !commands.granted && !commands.available && !commands.reserved
        );
        assert_eq!(commands.label_key, "horizon.mods.power.commands.label");
        assert_eq!(
            commands.description_key,
            "horizon.mods.power.commands.description"
        );
        let world = view(&list, "world");
        assert!(world.granted && world.available && !world.reserved);
        let files = view(&list, "files");
        assert!(!files.granted && !files.available && files.reserved);
    }

    #[test]
    fn a_mod_requesting_nothing_lists_nothing_and_has_no_powers_entry_flag() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        assert!(provider.powers("beta_mod").is_empty());
        assert!(provider.powers("no_such_mod").is_empty());
        let entries = provider.entries();
        let flag = |id: &str| entries.iter().find(|e| e.id == id).unwrap().powers;
        assert!(!flag("beta_mod"));
        assert!(flag("alpha_mod") && flag("off_mod"));
    }

    #[test]
    fn a_grant_writes_the_grants_table_and_the_list_shows_it() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        assert_eq!(provider.set_power("gamma_mod", "world", true), Ok(()));
        let text = String::from_utf8(snapshot(game.path()).unwrap()).unwrap();
        assert!(text.contains("[grants]"), "{text}");
        assert!(text.contains("gamma_mod = [\"world\"]"), "{text}");
        assert!(
            text.contains("alpha_mod = [\"world\"]"),
            "the other grant is kept: {text}"
        );
        assert!(view(&provider.powers("gamma_mod"), "world").granted);
    }

    #[test]
    fn a_revoke_removes_the_power_and_a_mod_left_with_none_loses_its_key() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        assert_eq!(provider.set_power("alpha_mod", "world", false), Ok(()));
        let text = String::from_utf8(snapshot(game.path()).unwrap()).unwrap();
        assert!(!text.contains("alpha_mod"), "{text}");
        assert!(
            !text.contains("grants"),
            "the empty table is removed: {text}"
        );
        assert!(!view(&provider.powers("alpha_mod"), "world").granted);
    }

    #[test]
    fn a_grant_keeps_the_mods_table() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        assert_eq!(provider.set_power("gamma_mod", "world", true), Ok(()));
        let text = String::from_utf8(snapshot(game.path()).unwrap()).unwrap();
        assert!(text.contains("off_mod = false"), "{text}");
    }

    #[test]
    fn revoking_an_unavailable_leftover_grant_works() {
        let game = game_with_powers();
        std::fs::write(
            enabled_file(game.path()),
            "[grants]\nalpha_mod = [\"commands\", \"world\"]\n",
        )
        .unwrap();
        let provider = provider_for(game.path(), &[]);
        assert!(view(&provider.powers("alpha_mod"), "commands").granted);
        assert_eq!(provider.set_power("alpha_mod", "commands", false), Ok(()));
        let text = String::from_utf8(snapshot(game.path()).unwrap()).unwrap();
        assert!(!text.contains("commands"), "{text}");
        assert!(text.contains("world"), "{text}");
    }

    /// Each refusal gives its key and writes nothing: the file is byte-identical, or still absent.
    #[test]
    fn refusals_write_nothing() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        let before = snapshot(game.path());
        let cases = [
            (("nope", "world", true), UNKNOWN_MOD),
            (("gamma_mod", "flying", true), POWER_UNKNOWN),
            (("gamma_mod", "commands", true), POWER_NOT_REQUESTED),
            (("nope", "world", false), UNKNOWN_MOD),
            (("gamma_mod", "flying", false), POWER_UNKNOWN),
            (("alpha_mod", "commands", true), POWER_UNAVAILABLE),
            (("alpha_mod", "files", true), POWER_UNAVAILABLE),
        ];
        for ((id, power, granted), key) in cases {
            let refused = provider.set_power(id, power, granted).unwrap_err();
            assert_eq!(refused.key, key, "{id} {power}");
            assert_eq!(snapshot(game.path()), before, "{id} {power} wrote nothing");
        }
        let bare = tempfile::tempdir().unwrap();
        let none = provider_for(bare.path(), &[]);
        assert!(none.set_power("nope", "world", true).is_err());
        assert_eq!(
            snapshot(bare.path()),
            None,
            "no file is created by a refusal"
        );
    }

    #[test]
    fn revoking_a_power_the_mod_does_not_request_removes_the_dormant_grant() {
        let game = game_with_powers();
        std::fs::write(
            enabled_file(game.path()),
            "[mods]\noff_mod = false\n\n[grants]\nalpha_mod = [\"world\"]\ngamma_mod = [\"files\", \"world\"]\n",
        )
        .unwrap();
        let provider = provider_for(game.path(), &[]);
        assert_eq!(provider.set_power("gamma_mod", "files", false), Ok(()));
        let text = String::from_utf8(snapshot(game.path()).unwrap()).unwrap();
        assert!(!text.contains("files"), "{text}");
        assert!(text.contains("gamma_mod = [\"world\"]"), "{text}");
        assert!(text.contains("alpha_mod = [\"world\"]"), "{text}");
        assert!(text.contains("off_mod = false"), "{text}");
    }

    /// A revoke of a power that is not granted writes nothing: the file keeps its bytes, and no file is
    /// created.
    #[test]
    fn a_revoke_with_nothing_granted_leaves_the_file_alone() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        let before = snapshot(game.path());
        assert_eq!(provider.set_power("gamma_mod", "commands", false), Ok(()));
        assert_eq!(provider.set_power("alpha_mod", "files", false), Ok(()));
        assert_eq!(snapshot(game.path()), before);

        let bare = tempfile::tempdir().unwrap();
        install(
            &bare.path().join(discover::DIR),
            "alpha",
            "alpha_mod",
            &["world"],
        );
        let lone = provider_for(bare.path(), &[]);
        assert_eq!(lone.set_power("alpha_mod", "world", false), Ok(()));
        assert_eq!(snapshot(bare.path()), None);
    }

    #[test]
    fn a_symlinked_enabled_file_gives_the_file_fault_and_is_not_written() {
        let game = game_with_powers();
        let elsewhere = game.path().join("elsewhere.toml");
        std::fs::write(&elsewhere, "[mods]\n").unwrap();
        std::fs::remove_file(enabled_file(game.path())).unwrap();
        std::os::unix::fs::symlink(&elsewhere, enabled_file(game.path())).unwrap();
        let provider = provider_for(game.path(), &[]);
        let failed = provider.set_power("gamma_mod", "world", true).unwrap_err();
        assert_eq!(failed.key, FILE);
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"[mods]\n");
    }

    #[test]
    fn an_unparsable_enabled_file_is_not_overwritten() {
        let game = game_with_powers();
        let provider = provider_for(game.path(), &[]);
        std::fs::write(enabled_file(game.path()), "[mods\nbroken = ").unwrap();
        let failed = provider.set_power("gamma_mod", "world", true).unwrap_err();
        assert_eq!(failed.key, FILE);
        assert_eq!(snapshot(game.path()).unwrap(), b"[mods\nbroken = ");
    }

    #[test]
    fn a_disabled_mod_and_a_dev_mod_are_edited_by_id() {
        let game = game_with_powers();
        let dev_root = tempfile::tempdir().unwrap();
        install(dev_root.path(), "devmod", "dev_mod", &["world"]);
        let dev = dev_root.path().join("devmod");
        let provider = provider_for(game.path(), std::slice::from_ref(&dev));
        assert_eq!(provider.set_power("off_mod", "world", true), Ok(()));
        assert_eq!(provider.set_power("dev_mod", "world", true), Ok(()));
        let text = String::from_utf8(snapshot(game.path()).unwrap()).unwrap();
        assert!(text.contains("off_mod = [\"world\"]"), "{text}");
        assert!(text.contains("dev_mod = [\"world\"]"), "{text}");
    }
}
