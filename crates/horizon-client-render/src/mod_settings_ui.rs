//! A Lua mod's settings page (#64): the settings its `settings.json` declares, one control each,
//! on the settings shell (`settings_shell`). The game reads the mods' schemas and values into
//! `ModList::config` (a [`Snapshot`]); each change goes back as an `Event`, which the game writes
//! through its provider and then reads the snapshot again. A refused write therefore leaves the
//! control on the stored value.
//!
//! A text, and a number without both bounds, is typed in: the shell opens an edit on its row
//! (`Value::Field`), and Enter hands the text to `Screen::field`, which sends it only if it fits.

use crate::{
    lang,
    options_ui::Event,
    settings::Settings,
    settings_shell::{self as shell, Group, Output, Row, Screen, Sheet, Value},
};
use serde_json::{Value as Json, json};
use std::sync::Mutex;

/// The page's title (the Mods page's Settings button) and the line under it.
const TITLE: &str = "horizon.mods.settings";
const ABOUT: &str = "horizon.mods.settings.about";
/// The heading of the settings that name no group.
const GENERAL: &str = "horizon.mods.settings.general";
/// The line over a client's page for the settings a remote server holds.
const REMOTE: &str = "horizon.mods.settings.remote";
const RESET: &str = "horizon.mods.settings.reset";
const RESET_ALL: &str = "horizon.mods.settings.reset_all";
/// A slider of whole numbers steps by each whole number up to this many stops; a wider range
/// steps by the pixel.
const MAX_STEPS: f64 = 1000.0;
/// A slider of decimals has this many stops.
const FLOAT_STEPS: u32 = 100;

/// What a setting holds (`horizon_client::mod_settings::Kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Text,
    Choice,
}

/// The side whose file holds a setting (`horizon_client::mod_settings::Side`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Server,
    Client,
}

/// One declared setting, with the value it has now.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub kind: Kind,
    pub side: Side,
    pub group: Option<String>,
    pub hidden: bool,
    /// Whether the provider lets it be written at all.
    pub editable: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Vec<String>,
    pub default: Json,
    pub value: Json,
}

/// A mod that declares settings, its entries in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct ModConfig {
    pub mod_id: String,
    pub entries: Vec<Entry>,
}

/// Every mod's settings as the game last read them. `remote`: this game is a client of a server,
/// whose settings are the server's to change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub mods: Vec<ModConfig>,
    pub remote: bool,
}

impl Entry {
    /// Whether this game may change the setting now: the provider allows it, and a remote
    /// server's settings are not the player's to change.
    pub fn writable(&self, remote: bool) -> bool {
        self.editable && !(remote && self.side == Side::Server)
    }
    /// The bounds of a slider: a number with both a minimum and a larger maximum.
    fn range(&self) -> Option<(f64, f64)> {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) if matches!(self.kind, Kind::Int | Kind::Float) && hi > lo => {
                Some((lo, hi))
            }
            _ => None,
        }
    }
    /// The value a slider at `position` (0 to 1) stands for.
    fn slider_value(&self, position: f64) -> Option<Json> {
        let (lo, hi) = self.range()?;
        let x = lo + position.clamp(0.0, 1.0) * (hi - lo);
        Some(match self.kind {
            Kind::Int => json!(x.round() as i64),
            _ => json!(x),
        })
    }
    /// The slider the setting draws as, when it has a range.
    fn slider(&self) -> Option<Value> {
        let (lo, hi) = self.range()?;
        let now = self.value.as_f64().unwrap_or(lo);
        let steps = match self.kind {
            Kind::Int if hi - lo <= MAX_STEPS => Some((hi - lo) as u32),
            Kind::Int => None,
            _ => Some(FLOAT_STEPS),
        };
        Some(Value::Slider {
            position: ((now - lo) / (hi - lo)).clamp(0.0, 1.0) as f32,
            steps,
            text: match self.kind {
                Kind::Int => (now.round() as i64).to_string(),
                _ => now.to_string(),
            },
        })
    }
    /// The value a typed text sets, if it is one this setting takes: a whole number for an int, a
    /// number for a float, within the bounds that exist; any text for a text.
    fn typed_value(&self, text: &str) -> Option<Json> {
        let within =
            |x: f64| self.min.is_none_or(|lo| x >= lo) && self.max.is_none_or(|hi| x <= hi);
        match self.kind {
            Kind::Text => Some(Json::String(text.to_owned())),
            Kind::Int => {
                let whole: i64 = text.trim().parse().ok()?;
                within(whole as f64).then(|| json!(whole))
            }
            Kind::Float => {
                let x: f64 = text.trim().parse().ok()?;
                (x.is_finite() && within(x)).then(|| json!(x))
            }
            Kind::Bool | Kind::Choice => None,
        }
    }
    /// Whether the value differs from the default: its reset row shows then.
    pub fn changed(&self) -> bool {
        self.value != self.default
    }
}

/// A row of the page: a setting by its index in the mod's entries, or one of the page's own rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingRow {
    /// The line that the server's settings are changed on the server.
    Remote,
    /// Every setting of the mod back to its default.
    ResetAll,
    /// Setting `.0` of the mod's entries.
    Setting(usize),
    /// Setting `.0`'s reset, under it.
    Reset(usize),
}

/// The settings page of the mod at `index` in `mods.installed`.
#[derive(Default)]
pub(crate) struct ModSettingsScreen {
    pub(crate) index: usize,
    /// A slider's change, sent when the slider is let go.
    pending: Option<Event>,
}

/// The text a value shows as: a string as it is, anything else as JSON.
fn text_of(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The label of setting `key`: its translation, else its own key.
fn label_of(mod_id: &str, key: &str) -> String {
    let tr = format!("mod.{mod_id}.setting.{key}");
    if lang::has(&tr) {
        lang::get(&tr)
    } else {
        key.to_owned()
    }
}

/// The line the shell's strip shows for setting `key`: its description's translation, else none.
fn about_of(mod_id: &str, key: &str) -> String {
    let tr = format!("mod.{mod_id}.setting.{key}.desc");
    if lang::has(&tr) {
        lang::get(&tr)
    } else {
        String::new()
    }
}

/// The heading of a group: its translation, else its own name.
fn group_of(mod_id: &str, group: Option<&str>) -> &'static str {
    match group {
        None => GENERAL,
        Some(name) => {
            let tr = format!("mod.{mod_id}.group.{name}");
            intern(if lang::has(&tr) { tr } else { name.to_owned() })
        }
    }
}

/// A group name as the shell takes it (`&'static str`). Each distinct name is leaked once, so a
/// mod's groups cost a few bytes for the whole run, however often the page is drawn.
fn intern(name: String) -> &'static str {
    static NAMES: Mutex<std::collections::BTreeSet<&'static str>> =
        Mutex::new(std::collections::BTreeSet::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = names.get(name.as_str()) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.into_boxed_str());
    names.insert(leaked);
    leaked
}

/// The value a setting's row shows, and draws as: a control where the setting can be changed and
/// the shell can draw it, else its text.
fn value_of(entry: &Entry, writable: bool) -> Value {
    let text = text_of(&entry.value);
    if !writable {
        return Value::Read(text);
    }
    match entry.kind {
        Kind::Bool => Value::Switch(entry.value.as_bool().unwrap_or(false)),
        Kind::Choice if !entry.choices.is_empty() => Value::Choice(choice_now(entry)),
        // A number without both bounds, and a text, are typed in (`Screen::field`).
        Kind::Int | Kind::Float => entry.slider().unwrap_or(Value::Field(text)),
        Kind::Text => Value::Field(text),
        Kind::Choice => Value::Read(text),
    }
}

/// The choice a stored value names, else the first choice (a value outside the list).
fn choice_now(entry: &Entry) -> String {
    let current = entry.value.as_str().unwrap_or_default();
    if entry.choices.iter().any(|c| c == current) {
        current.to_owned()
    } else {
        entry.choices.first().cloned().unwrap_or_default()
    }
}

/// The choice after (or, `back`, before) the one now, wrapping round.
fn step_choice(entry: &Entry, back: bool) -> Option<Json> {
    let n = entry.choices.len();
    if n == 0 {
        return None;
    }
    let now = choice_now(entry);
    let at = entry.choices.iter().position(|c| *c == now).unwrap_or(0);
    let next = if back { (at + n - 1) % n } else { (at + 1) % n };
    Some(Json::String(entry.choices[next].clone()))
}

/// The page's groups: the remote line and Reset all first, then the settings grouped as the
/// provider lists them, hidden ones left out, each setting with its reset under it when changed.
fn groups_of(config: &ModConfig, remote: bool) -> Vec<Group<SettingRow>> {
    let mut groups: Vec<Group<SettingRow>> = Vec::new();
    let mut top = Vec::new();
    if config.entries.iter().any(|e| !e.writable(remote)) {
        let note = Row::new(SettingRow::Remote, lang::get(REMOTE), Value::Note);
        top.push(note.live(false));
    }
    if config
        .entries
        .iter()
        .any(|e| e.writable(remote) && e.changed())
    {
        top.push(Row::new(
            SettingRow::ResetAll,
            lang::get(RESET_ALL),
            Value::Press,
        ));
    }
    if !top.is_empty() {
        groups.push(Group {
            name: TITLE,
            rows: top,
        });
    }
    for (i, entry) in config.entries.iter().enumerate() {
        if entry.hidden {
            continue;
        }
        let name = group_of(&config.mod_id, entry.group.as_deref());
        if !groups.last().is_some_and(|g| g.name == name) {
            groups.push(Group {
                name,
                rows: Vec::new(),
            });
        }
        let writable = entry.writable(remote);
        let row = Row::new(
            SettingRow::Setting(i),
            label_of(&config.mod_id, &entry.key),
            value_of(entry, writable),
        )
        .live(writable)
        .about(about_of(&config.mod_id, &entry.key));
        let Some(group) = groups.last_mut() else {
            continue;
        };
        group.rows.push(row);
        if writable && entry.changed() {
            let reset = Row::new(SettingRow::Reset(i), lang::get(RESET), Value::Press);
            group.rows.push(reset);
        }
    }
    groups
}

impl ModSettingsScreen {
    /// The mod's settings in the snapshot, found by the id of the installed mod at `index`.
    fn config<'a>(&self, s: &'a Settings) -> Option<&'a ModConfig> {
        let id = &s.mods.installed.get(self.index)?.id;
        s.mods.config.mods.iter().find(|m| &m.mod_id == id)
    }

    /// The write a row's press asks for, if any.
    fn event_of(&self, s: &Settings, row: SettingRow, back: bool) -> Option<Event> {
        let config = self.config(s)?;
        let remote = s.mods.config.remote;
        let mod_id = config.mod_id.clone();
        match row {
            SettingRow::Remote => None,
            SettingRow::ResetAll => Some(Event::ModSettingsResetAll { mod_id }),
            SettingRow::Reset(i) => {
                let entry = config.entries.get(i)?;
                Some(Event::ModSettingReset {
                    mod_id,
                    side: entry.side,
                    key: entry.key.clone(),
                })
            }
            SettingRow::Setting(i) => {
                let entry = config.entries.get(i)?;
                if !entry.writable(remote) {
                    return None;
                }
                let value = match entry.kind {
                    Kind::Bool => json!(!entry.value.as_bool().unwrap_or(false)),
                    Kind::Choice => step_choice(entry, back)?,
                    // A number is set by its slider, a text has no control yet.
                    Kind::Int | Kind::Float | Kind::Text => return None,
                };
                Some(Event::ModSetting {
                    mod_id,
                    side: entry.side,
                    key: entry.key.clone(),
                    value,
                })
            }
        }
    }
}

impl Screen for ModSettingsScreen {
    type Page = ();
    type Row = SettingRow;

    fn title(&self) -> &'static str {
        TITLE
    }

    fn sheets(&self, s: &Settings) -> Vec<Sheet<(), SettingRow>> {
        let name = s
            .mods
            .installed
            .get(self.index)
            .map_or_else(String::new, |m| m.title.clone());
        let groups = self
            .config(s)
            .map_or_else(Vec::new, |config| groups_of(config, s.mods.config.remote));
        vec![Sheet::new((), name, lang::get(ABOUT), groups)]
    }

    /// The rows read the mod's settings and the snapshot's remote flag.
    fn stamp(&self, s: &Settings) -> u64 {
        let id = s.mods.installed.get(self.index).map(|m| &m.id);
        shell::stamp_of(&(self.index, id, &s.mods.config))
    }

    fn press(&mut self, s: &mut Settings, row: SettingRow, back: bool) -> Vec<Output> {
        self.event_of(s, row, back)
            .map_or_else(Vec::new, |event| vec![Output::Event(event)])
    }

    /// A slider's position is the change it asks for, sent when it is let go.
    fn slide(&mut self, s: &mut Settings, row: SettingRow, position: f32) {
        let SettingRow::Setting(i) = row else {
            return;
        };
        let remote = s.mods.config.remote;
        let Some(config) = self.config(s) else {
            return;
        };
        let Some(entry) = config.entries.get(i) else {
            return;
        };
        if !entry.writable(remote) {
            return;
        }
        if let Some(value) = entry.slider_value(f64::from(position)) {
            self.pending = Some(Event::ModSetting {
                mod_id: config.mod_id.clone(),
                side: entry.side,
                key: entry.key.clone(),
                value,
            });
        }
    }

    fn slider_released(&mut self) -> Vec<Output> {
        self.pending
            .take()
            .map_or_else(Vec::new, |event| vec![Output::Event(event)])
    }

    /// A typed value, submitted: it is written when it parses and fits; otherwise nothing is sent
    /// and the row keeps showing the stored value.
    fn field(&mut self, s: &mut Settings, row: SettingRow, text: String) -> Vec<Output> {
        let SettingRow::Setting(i) = row else {
            return Vec::new();
        };
        let remote = s.mods.config.remote;
        let Some(config) = self.config(s) else {
            return Vec::new();
        };
        let Some(entry) = config.entries.get(i) else {
            return Vec::new();
        };
        if !entry.writable(remote) {
            return Vec::new();
        }
        let Some(value) = entry.typed_value(&text) else {
            return Vec::new();
        };
        vec![Output::Event(Event::ModSetting {
            mod_id: config.mod_id.clone(),
            side: entry.side,
            key: entry.key.clone(),
            value,
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_shell::{Hosted, Shell};

    const SIZE: (f32, f32) = (854.0, 480.0);

    fn entry(key: &str, kind: Kind, value: Json, default: Json) -> Entry {
        Entry {
            key: key.to_owned(),
            kind,
            side: Side::Client,
            group: None,
            hidden: false,
            editable: true,
            min: None,
            max: None,
            choices: Vec::new(),
            default,
            value,
        }
    }

    /// Mod "ruby" with a hidden entry, and entries out of order across two groups.
    fn ruby() -> ModConfig {
        // At its default, so the page's Reset all (which counts the visible settings) is not for it.
        let mut hidden = entry("secret", Kind::Bool, json!(true), json!(true));
        hidden.hidden = true;
        let mut volume = entry("volume", Kind::Int, json!(3), json!(5));
        volume.min = Some(0.0);
        volume.max = Some(10.0);
        volume.group = Some("sound".to_owned());
        let mut mode = entry("mode", Kind::Choice, json!("loud"), json!("soft"));
        mode.choices = vec!["soft".to_owned(), "loud".to_owned(), "off".to_owned()];
        mode.group = Some("look".to_owned());
        let mut glow = entry("glow", Kind::Bool, json!(false), json!(false));
        glow.group = Some("look".to_owned());
        ModConfig {
            mod_id: "ruby".to_owned(),
            // Sorted as the provider sorts them: group, then order, then key.
            entries: vec![hidden, volume, glow, mode],
        }
    }

    fn settings_with(config: ModConfig, remote: bool) -> Settings {
        let mut s = Settings::default();
        s.mods.installed = vec![crate::mod_list::Installed {
            id: "ruby".to_owned(),
            kind: crate::mod_list::ModKind::Content,
            title: "Ruby 1.0".to_owned(),
            description: String::new(),
            problem: None,
            dev: false,
            native: None,
        }];
        s.mods.config = Snapshot {
            mods: vec![config],
            remote,
        };
        s
    }

    fn rows_of(s: &Settings) -> Vec<Row<SettingRow>> {
        let screen = ModSettingsScreen::default();
        screen
            .sheets(s)
            .into_iter()
            .flat_map(|sheet| sheet.groups)
            .flat_map(|group| group.rows)
            .collect()
    }

    fn group_names(s: &Settings) -> Vec<&'static str> {
        let screen = ModSettingsScreen::default();
        screen
            .sheets(s)
            .into_iter()
            .flat_map(|sheet| sheet.groups)
            .map(|group| group.name)
            .collect()
    }

    /// The rows follow the provider's order in their groups; a hidden setting has no row; a
    /// group's heading comes before its rows.
    #[test]
    fn mod_settings_page_shows_its_groups_and_rows_in_order_without_hidden_ones() {
        let s = settings_with(ruby(), false);
        // Reset all, then the sound group, then the look group (the hidden setting is in neither).
        assert_eq!(group_names(&s), vec![TITLE, "sound", "look"]);
        let ids: Vec<SettingRow> = rows_of(&s).iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![
                SettingRow::ResetAll,
                SettingRow::Setting(1),
                SettingRow::Reset(1),
                SettingRow::Setting(2),
                SettingRow::Setting(3),
                SettingRow::Reset(3),
            ]
        );
    }

    /// A choice shows the value stored, and a value outside its list shows the first choice.
    #[test]
    fn mod_settings_choice_shows_its_stored_value_or_the_first_choice() {
        let s = settings_with(ruby(), false);
        let mode = rows_of(&s)
            .into_iter()
            .find(|r| r.id == SettingRow::Setting(3));
        assert_eq!(mode.unwrap().value, Value::Choice("loud".to_owned()));

        let mut config = ruby();
        config.entries[3].value = json!("nowhere");
        let s = settings_with(config, false);
        let mode = rows_of(&s)
            .into_iter()
            .find(|r| r.id == SettingRow::Setting(3));
        assert_eq!(mode.unwrap().value, Value::Choice("soft".to_owned()));
    }

    /// A server's setting, seen from a remote client, is read-only and the page says why.
    #[test]
    fn mod_settings_remote_client_sees_server_settings_as_a_note_and_no_control() {
        let mut config = ruby();
        config.entries[1].side = Side::Server;
        let s = settings_with(config, true);
        let rows = rows_of(&s);
        assert_eq!(rows[0].id, SettingRow::Remote);
        assert_eq!(rows[0].value, Value::Note);
        let volume = rows
            .iter()
            .find(|r| r.id == SettingRow::Setting(1))
            .unwrap();
        assert!(!volume.live);
        assert_eq!(volume.value, Value::Read("3".to_owned()));
        // The same setting in single player is the player's to change.
        let s = settings_with(ruby(), false);
        assert!(rows_of(&s).iter().all(|r| r.id != SettingRow::Remote));
    }

    /// A setting with no translation shows its own key: no English is written here.
    #[test]
    fn mod_settings_label_without_a_translation_is_the_settings_key() {
        let s = settings_with(ruby(), false);
        let names: Vec<String> = rows_of(&s).iter().map(|r| r.name.clone()).collect();
        assert!(names.contains(&"volume".to_owned()));
        assert!(names.contains(&"mode".to_owned()));
    }

    /// A toggle press asks for the other value; a slider's drag asks for the value it stands
    /// for when let go, rounded for a whole number; a reset press asks for the reset.
    #[test]
    fn mod_settings_presses_and_slides_give_the_events_for_the_change() {
        let mut s = settings_with(ruby(), false);
        let mut screen = ModSettingsScreen::default();
        let glow = SettingRow::Setting(2);
        assert_eq!(
            screen.press(&mut s, glow, false),
            vec![Output::Event(Event::ModSetting {
                mod_id: "ruby".to_owned(),
                side: Side::Client,
                key: "glow".to_owned(),
                value: json!(true),
            })]
        );
        screen.slide(&mut s, SettingRow::Setting(1), 1.0);
        assert_eq!(
            screen.slider_released(),
            vec![Output::Event(Event::ModSetting {
                mod_id: "ruby".to_owned(),
                side: Side::Client,
                key: "volume".to_owned(),
                value: json!(10),
            })]
        );
        screen.slide(&mut s, SettingRow::Setting(1), 0.26);
        assert_eq!(
            screen.slider_released(),
            vec![Output::Event(Event::ModSetting {
                mod_id: "ruby".to_owned(),
                side: Side::Client,
                key: "volume".to_owned(),
                value: json!(3),
            })]
        );
        // The volume is 3, its default 5: its reset row is there under it.
        assert_eq!(
            screen.press(&mut s, SettingRow::Reset(1), false),
            vec![Output::Event(Event::ModSettingReset {
                mod_id: "ruby".to_owned(),
                side: Side::Client,
                key: "volume".to_owned(),
            })]
        );
    }

    /// Choice steps forward and back, wrapping round its list.
    #[test]
    fn mod_settings_choice_press_steps_through_its_choices() {
        let mut s = settings_with(ruby(), false);
        let mut screen = ModSettingsScreen::default();
        let mode = SettingRow::Setting(3);
        let value = |out: Vec<Output>| match out.as_slice() {
            [Output::Event(Event::ModSetting { value, .. })] => value.clone(),
            other => panic!("no setting event: {other:?}"),
        };
        // Stored "loud": forward is "off", back is "soft".
        assert_eq!(value(screen.press(&mut s, mode, false)), json!("off"));
        assert_eq!(value(screen.press(&mut s, mode, true)), json!("soft"));
    }

    /// A page's reset all is there while a setting differs from its default, and the page of a
    /// mod that is not installed is empty.
    #[test]
    fn mod_settings_reset_all_shows_only_while_something_differs() {
        let s = settings_with(ruby(), false);
        assert!(rows_of(&s).iter().any(|r| r.id == SettingRow::ResetAll));
        let mut config = ruby();
        config.entries[1].value = json!(5);
        config.entries[2].value = json!(false);
        config.entries[3].value = json!("soft");
        let s = settings_with(config, false);
        assert!(rows_of(&s).iter().all(|r| r.id != SettingRow::ResetAll));
        let mut s = Settings::default();
        s.mods.config = Snapshot::default();
        assert!(ModSettingsScreen::default().sheets(&s)[0].groups.is_empty());
    }

    /// The shell hands a press on a row to the screen: a click on the toggle gives its event.
    #[test]
    fn mod_settings_click_on_a_toggle_reaches_the_game() {
        let mut s = settings_with(ruby(), false);
        let mut shell = Shell::<ModSettingsScreen>::default();
        let row = SettingRow::Setting(2);
        let at = shell.point_at(&s, SIZE, row);
        let out = shell.click(&mut s, at, SIZE, false, false, false);
        assert!(out.iter().any(|o| matches!(
            o,
            Output::Event(Event::ModSetting { key, value, .. })
                if key == "glow" && *value == json!(true)
        )));
    }

    /// A text, a whole number with a minimum only, and a decimal with no bounds: the three that
    /// are typed in.
    fn free() -> ModConfig {
        let name = entry("name", Kind::Text, json!("ab"), json!(""));
        let mut count = entry("count", Kind::Int, json!(1), json!(0));
        count.min = Some(0.0);
        let ratio = entry("ratio", Kind::Float, json!(0.5), json!(1.0));
        ModConfig {
            mod_id: "ruby".to_owned(),
            entries: vec![name, count, ratio],
        }
    }

    /// A text row opens on a press with its text; what is typed adds to it, and Enter sets it.
    #[test]
    fn mod_settings_text_row_is_typed_in_and_enter_sends_the_text() {
        let mut s = settings_with(free(), false);
        let mut shell = Shell::<ModSettingsScreen>::default();
        let at = shell.point_at(&s, SIZE, SettingRow::Setting(0));
        shell.click(&mut s, at, SIZE, false, false, false);
        assert!(shell.typing(), "the press opens the edit");
        shell.type_text("c");
        let out = shell.key(&mut s, winit::keyboard::KeyCode::Enter, None, false, SIZE);
        assert_eq!(
            out,
            vec![Output::Event(Event::ModSetting {
                mod_id: "ruby".to_owned(),
                side: Side::Client,
                key: "name".to_owned(),
                value: json!("abc"),
            })]
        );
        assert!(!shell.typing(), "Enter ends the edit");
    }

    /// Escape leaves the edit as it was: nothing is sent, and the next Enter sends nothing.
    #[test]
    fn mod_settings_escape_cancels_a_typed_text() {
        use winit::keyboard::KeyCode as K;
        let mut s = settings_with(free(), false);
        let mut shell = Shell::<ModSettingsScreen>::default();
        let at = shell.point_at(&s, SIZE, SettingRow::Setting(0));
        shell.click(&mut s, at, SIZE, false, false, false);
        shell.type_text("c");
        assert!(shell.key(&mut s, K::Escape, None, false, SIZE).is_empty());
        assert!(!shell.typing());
        // Enter on the row (it keeps the focus) opens the edit again; it sends no setting.
        let out = shell.key(&mut s, K::Enter, None, false, SIZE);
        assert!(
            !out.iter()
                .any(|o| matches!(o, Output::Event(Event::ModSetting { .. })))
        );
    }

    /// A number with no bounds, or only a minimum, is set when it parses and fits: a whole number
    /// for an int, any finite number for a float. Anything else sends nothing.
    #[test]
    fn mod_settings_typed_number_is_set_only_when_it_parses_and_fits() {
        let mut s = settings_with(free(), false);
        let mut screen = ModSettingsScreen::default();
        let event = |key: &str, value: Json| {
            vec![Output::Event(Event::ModSetting {
                mod_id: "ruby".to_owned(),
                side: Side::Client,
                key: key.to_owned(),
                value,
            })]
        };
        assert_eq!(
            screen.field(&mut s, SettingRow::Setting(1), "7".to_owned()),
            event("count", json!(7))
        );
        assert!(
            screen
                .field(&mut s, SettingRow::Setting(1), "-1".to_owned())
                .is_empty()
        );
        assert!(
            screen
                .field(&mut s, SettingRow::Setting(1), "1.5".to_owned())
                .is_empty()
        );
        assert!(
            screen
                .field(&mut s, SettingRow::Setting(1), "x".to_owned())
                .is_empty()
        );
        assert_eq!(
            screen.field(&mut s, SettingRow::Setting(2), "2.5".to_owned()),
            event("ratio", json!(2.5))
        );
        assert!(
            screen
                .field(&mut s, SettingRow::Setting(2), "inf".to_owned())
                .is_empty()
        );
    }
}
