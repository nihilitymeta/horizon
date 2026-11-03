//! The toggleable features this build carries, declared by `registry/<id>.json` files beside the
//! crate (`registry/README.md`) and collected by `build.rs`: the built-in native hook mods and the
//! engine patches. Each names the setting that switches it (`horizon-graphics.txt` or
//! `horizon-extras.txt`, a key, its on and off values), so the feature itself, Options > Mods and
//! the launcher all agree on one switch: the mod list's `mod.<id>` line mirrors the setting
//! (`sync_to_mods`), and a line written while the game was closed, by the launcher, is applied to
//! the setting at the next start (`sync_from_mods`); a toggle in Options > Mods moves only its own
//! switch (`follow_toggle`). `horizon-client --list-mods` prints the list for the launcher.
use crate::{extras::Extras, graphics::Graphics, mod_list::ModList, settings::Settings};
use serde::{Deserialize, Serialize};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/mod_registry.rs"));
}

/// What a feature is, for the Mods page's and the launcher's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A native hook mod built into the game (zoom, minimap, the HUD counters).
    Hook,
    /// An engine patch: a renderer or platform feature with a switch.
    Engine,
}

/// Which settings file holds a feature's switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum File {
    Graphics,
    Extras,
}

/// A feature's switch: the line `key:<on>` or `key:<off>` of `file`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub file: File,
    pub key: String,
    pub on: String,
    pub off: String,
}

/// One registered feature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// `Name 1.0`, as Options > Mods titles mods.
    pub title: String,
    pub description: String,
    pub kind: Kind,
    #[serde(default)]
    pub default_on: bool,
    pub setting: Setting,
}

/// The build's features, by id. A file that does not parse is skipped with a warning: a
/// feature's switch is never worth a crash.
pub fn entries() -> Vec<Entry> {
    let mut entries: Vec<Entry> = generated::RAW
        .iter()
        .filter_map(|(file, text)| match serde_json::from_str::<Entry>(text) {
            Ok(entry) => Some(entry),
            Err(e) => {
                log::warn!("registry/{file}.json: {e}");
                None
            }
        })
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries
}

/// The list as `--list-mods` prints it: a JSON array of the entries.
pub fn to_json() -> String {
    serde_json::to_string_pretty(&entries()).unwrap_or_else(|_| "[]".to_owned())
}

/// The value of `key` in a `key:value` file's text.
fn value_of(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_owned())
}

/// `text` with `key`'s line set to `value`, appended when it had none.
fn with_value(text: &str, key: &str, value: &str) -> String {
    let mut out = String::new();
    let mut found = false;
    for line in text.lines() {
        match line.split_once(':') {
            Some((k, _)) if k.trim() == key => {
                found = true;
                out.push_str(&format!("{key}:{value}\n"));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !found {
        out.push_str(&format!("{key}:{value}\n"));
    }
    out
}

impl Setting {
    fn text(&self, s: &Settings) -> String {
        match self.file {
            File::Graphics => s.horizon.serialize(),
            File::Extras => s.extras.serialize(),
        }
    }
    /// The switch's current value, None when the file has no such line (a build without the
    /// feature, or a key it has not written yet).
    pub fn value(&self, s: &Settings) -> Option<String> {
        value_of(&self.text(s), &self.key)
    }
    fn set(&self, s: &mut Settings, value: &str) {
        let text = with_value(&self.text(s), &self.key, value);
        match self.file {
            File::Graphics => s.horizon = Graphics::parse(&text),
            File::Extras => s.extras = Extras::parse(&text),
        }
    }
}

impl Entry {
    /// Whether the feature is on: its switch holds anything but its off value (a mode other than
    /// off counts as on), or the default when the file has no such line.
    pub fn is_on(&self, s: &Settings) -> bool {
        self.setting
            .value(s)
            .map_or(self.default_on, |v| v != self.setting.off)
    }
    /// Turns the feature on or off. Turning it on leaves a mode already chosen alone.
    pub fn set_on(&self, s: &mut Settings, on: bool) {
        if on == self.is_on(s) && self.setting.value(s).is_some() {
            return;
        }
        let value = if on {
            &self.setting.on
        } else {
            &self.setting.off
        };
        self.setting.set(s, value);
    }
}

/// Applies the mod list's `mod.<id>` lines to the features' switches: a line the launcher wrote
/// while the game was closed takes effect. No line leaves the switch as it is.
pub fn sync_from_mods(s: &mut Settings) {
    sync_from_mods_with(&entries(), s);
}

fn sync_from_mods_with(entries: &[Entry], s: &mut Settings) {
    for entry in entries {
        if s.mods.disabled.contains(&entry.id) {
            entry.set_on(s, false);
        } else if s.mods.enabled.contains(&entry.id) {
            entry.set_on(s, true);
        }
    }
}

/// After Options > Mods toggled installed mod `i`'s line: that feature's switch follows its new
/// line, and every other feature's line follows its switch. Applying every line here
/// (`sync_from_mods`) would re-impose a `mod.<id>` line the launcher wrote before this session on
/// a switch moved since, in Extras or Video Settings: Fullbright turned on, then any mod toggled,
/// and Fullbright was off again. A mod that is not one of the build's features changes nothing.
pub fn follow_toggle(s: &mut Settings, i: usize) {
    let Some(id) = s.mods.installed.get(i).map(|m| m.id.clone()) else {
        return;
    };
    follow_toggle_with(&entries(), s, &id);
}

fn follow_toggle_with(entries: &[Entry], s: &mut Settings, id: &str) {
    // Only the toggled feature follows its line; the other lines are brought up to date when
    // the list is saved (`mods_mirroring`), so a stale one never moves a switch here, and a
    // plain installed mod's toggle leaves the list as the player set it.
    if let Some(entry) = entries.iter().find(|e| e.id == id) {
        sync_from_mods_with(std::slice::from_ref(entry), s);
    }
}

/// Writes every feature's state into the mod list as its `mod.<id>` line, so the list (and the
/// launcher reading it) says what the switches say.
pub fn sync_to_mods(s: &mut Settings) {
    let mut mods = std::mem::take(&mut s.mods);
    sync_to_mods_with(&entries(), &mut mods, s);
    s.mods = mods;
}

/// The mod list with every feature's line brought up to date, for saving.
pub fn mods_mirroring(s: &Settings) -> ModList {
    let mut mods = s.mods.clone();
    sync_to_mods_with(&entries(), &mut mods, s);
    mods
}

fn sync_to_mods_with(entries: &[Entry], mods: &mut ModList, s: &Settings) {
    for entry in entries {
        if entry.is_on(s) {
            mods.disabled.remove(&entry.id);
            mods.enabled.insert(entry.id.clone());
        } else {
            mods.enabled.remove(&entry.id);
            mods.disabled.insert(entry.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, file: File, key: &str, on: &str, off: &str, default_on: bool) -> Entry {
        Entry {
            id: id.into(),
            title: format!("{id} 1.0"),
            description: String::new(),
            kind: Kind::Engine,
            default_on,
            setting: Setting {
                file,
                key: key.into(),
                on: on.into(),
                off: off.into(),
            },
        }
    }

    /// A branch's `registry/<id>.json` must parse, or its feature silently has no switch.
    #[test]
    fn every_registry_file_of_this_build_parses_and_names_its_file() {
        for (file, text) in generated::RAW {
            let entry: Entry =
                serde_json::from_str(text).unwrap_or_else(|e| panic!("registry/{file}.json: {e}"));
            assert_eq!(
                &entry.id, file,
                "registry/{file}.json: id and file name differ"
            );
            assert!(!entry.title.is_empty(), "registry/{file}.json: no title");
        }
        assert_eq!(entries().len(), generated::RAW.len());
    }

    #[test]
    fn an_entry_reads_from_json_with_kind_and_switch() {
        let e: Entry = serde_json::from_str(
            r#"{"id":"cas","title":"AMD CAS 1.0","description":"Sharpening.","kind":"engine",
                "setting":{"file":"graphics","key":"cas","on":"always","off":"off"}}"#,
        )
        .unwrap();
        assert_eq!(e.kind, Kind::Engine);
        assert!(!e.default_on);
        assert_eq!(e.setting.file, File::Graphics);
        let hook: Entry = serde_json::from_str(
            r#"{"id":"zoom","title":"Zoom 1.0","description":"","kind":"hook","default_on":true,
                "setting":{"file":"extras","key":"zoom","on":"true","off":"false"}}"#,
        )
        .unwrap();
        assert_eq!((hook.kind, hook.default_on), (Kind::Hook, true));
    }

    #[test]
    fn a_switch_turns_a_graphics_mode_off_and_back_on_without_losing_other_lines() {
        let mut s = Settings::default();
        let e = entry(
            "shadow",
            File::Graphics,
            "shadowCache",
            "true",
            "false",
            false,
        );
        assert!(!e.is_on(&s));
        let before = s.horizon.serialize();
        e.set_on(&mut s, true);
        assert!(e.is_on(&s));
        assert!(s.horizon.shadow_cache);
        // Only that line moved.
        let after = s.horizon.serialize();
        let changed: Vec<_> = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(changed, vec![("shadowCache:false", "shadowCache:true")]);
        e.set_on(&mut s, false);
        assert!(!s.horizon.shadow_cache);
    }

    #[test]
    fn an_extras_switch_follows_the_mod_lines_and_writes_them_back() {
        let mut s = Settings::default();
        let e = entry(
            "fullbright",
            File::Extras,
            "fullbright",
            "true",
            "false",
            false,
        );
        s.extras.fullbright = true;
        // The launcher turned it off while the game was closed.
        s.mods.disabled.insert("fullbright".into());
        sync_from_mods_with(std::slice::from_ref(&e), &mut s);
        assert!(!s.extras.fullbright);
        // The player turns it on in Extras: the list follows at save.
        s.extras.fullbright = true;
        let mut mods = s.mods.clone();
        sync_to_mods_with(&[e], &mut mods, &s);
        assert!(mods.enabled.contains("fullbright") && !mods.disabled.contains("fullbright"));
    }

    /// A switch moved this session, in Extras or Video Settings, is not undone by a `mod.<id>`
    /// line the launcher wrote before it when the player toggles another mod in Options > Mods.
    #[test]
    fn a_feature_turned_on_this_session_stays_on_when_another_mod_is_toggled() {
        let mut s = Settings::default();
        let fullbright = entry(
            "fullbright",
            File::Extras,
            "fullbright",
            "true",
            "false",
            false,
        );
        let shadow = entry(
            "shadow",
            File::Graphics,
            "shadowCache",
            "true",
            "false",
            false,
        );
        let entries = [fullbright, shadow];
        // The launcher turned Fullbright off while the game was closed; the start applied it.
        s.mods.disabled.insert("fullbright".into());
        sync_from_mods_with(&entries, &mut s);
        assert!(!s.extras.fullbright);
        // The player turns Fullbright on in Extras, then toggles the shadow cache mod on in
        // Options > Mods (`ModList::toggle` moves its line).
        s.extras.fullbright = true;
        s.mods.enabled.insert("shadow".into());
        follow_toggle_with(&entries, &mut s, "shadow");
        assert!(
            s.horizon.shadow_cache,
            "the toggled mod's line switched its feature"
        );
        assert!(
            s.extras.fullbright,
            "the stale line did not undo Fullbright"
        );
        // The toggle moved only its own line; the save brings the rest up to date for the launcher.
        assert_eq!(
            s.mods.serialize(),
            "mod.shadow:true\nmod.fullbright:false\n"
        );
        let mut saved = s.mods.clone();
        sync_to_mods_with(&entries, &mut saved, &s);
        assert_eq!(saved.serialize(), "mod.fullbright:true\nmod.shadow:true\n");
        // Toggling it off again in Options > Mods switches it off, and nothing else.
        s.mods.enabled.remove("fullbright");
        s.mods.disabled.insert("fullbright".into());
        follow_toggle_with(&entries, &mut s, "fullbright");
        assert!(!s.extras.fullbright && s.horizon.shadow_cache);
    }

    #[test]
    fn the_mod_lines_mirror_the_switches() {
        let mut s = Settings::default();
        let on = entry("a", File::Extras, "fullbright", "true", "false", false);
        let off = entry("b", File::Graphics, "shadowCache", "true", "false", false);
        s.extras.fullbright = true;
        let mut mods = ModList::default();
        mods.disabled.insert("a".into());
        mods.enabled.insert("b".into());
        sync_to_mods_with(&[on, off], &mut mods, &s);
        assert_eq!(mods.serialize(), "mod.a:true\nmod.b:false\n");
    }

    #[test]
    fn a_default_on_feature_without_a_line_counts_as_on() {
        let s = Settings::default();
        let e = entry("x", File::Graphics, "noSuchKey", "true", "false", true);
        assert!(e.is_on(&s));
        assert!(!entry("y", File::Graphics, "noSuchKey", "true", "false", false).is_on(&s));
    }
}
