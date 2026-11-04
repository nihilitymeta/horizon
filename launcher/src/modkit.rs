//! The modkit (`whos-carmen/horizon-modkit`): the Lua library Event Horizon's mods are written
//! with, so a prerequisite of playing them rather than a game build. Only Event Horizon runs its
//! scripts, so its GitHub release, `horizon_modkit.hmod`, goes into the `horizon-mods/` of every
//! instance that plays an Event Horizon build (`game_sources::is_event_horizon`), turned on; a copy
//! is kept, so an instance that starts playing one gets it at the next update check. Which version is installed,
//! and whether it updates itself, live in `modkit.json` in the launcher's data folder.
use crate::backend;
use crate::game_sources::{self, Release};
use crate::github_mods::Api;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const REPO: &str = "whos-carmen/horizon-modkit";
/// The mod id in its manifest, and the file it installs as.
pub const ID: &str = "horizon_modkit";
const FILE: &str = "modkit.json";
/// The most the `.hmod` may be: it is a few files of Lua.
const MAX_HMOD: u64 = 64 << 20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Install each new release when the launcher checks for updates.
    #[serde(default = "on")]
    pub auto_update: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<Installed>,
}

fn on() -> bool {
    true
}

impl Default for State {
    fn default() -> Self {
        Self {
            auto_update: true,
            installed: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// The manifest's version (`0.7.1`).
    pub version: String,
    /// The release it came from (`v0.7.1`).
    pub tag: String,
}

fn path() -> PathBuf {
    backend::data_dir().join(FILE)
}

fn parse(text: Option<&str>) -> State {
    text.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_default()
}

pub fn load() -> State {
    parse(fs::read_to_string(path()).ok().as_deref())
}

fn save(state: &State) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::create_dir_all(backend::data_dir()).map_err(|e| e.to_string())?;
    fs::write(path(), text).map_err(|e| e.to_string())
}

pub fn set_auto_update(on: bool) -> Result<(), String> {
    let mut state = load();
    state.auto_update = on;
    save(&state)
}

/// The modkit's newest release, through `api`.
pub fn latest(api: Api) -> Result<Release, String> {
    let release =
        api(&format!("repos/{REPO}/releases/latest"))?.ok_or("The modkit has no releases yet.")?;
    pick(&release)
}

fn pick(release: &serde_json::Value) -> Result<Release, String> {
    game_sources::asset(REPO, release, &[".hmod".to_owned()])
        .ok_or_else(|| "The modkit's newest release has no .hmod.".to_owned())
}

/// The copy kept for instances made later.
fn kept() -> PathBuf {
    backend::data_dir()
        .join("modkit")
        .join(format!("{ID}.hmod"))
}

/// The game folders of the instances that play an Event Horizon build: pinned to one, or following
/// the launcher's selection while that is one. Horizon's own builds don't run the modkit's scripts.
fn homes() -> Vec<PathBuf> {
    let selected = backend::Settings::load().selected;
    crate::instances::list()
        .into_iter()
        .filter(|i| i.java.is_none())
        .filter(|i| {
            i.version
                .as_deref()
                .or(selected.as_deref())
                .is_some_and(game_sources::is_event_horizon)
        })
        .map(|i| i.home)
        .collect()
}

/// Whether an instance with these settings plays an Event Horizon build (the Mods page's card).
pub fn plays_event_horizon(pinned: Option<&str>) -> bool {
    pinned
        .map(str::to_owned)
        .or(backend::Settings::load().selected)
        .is_some_and(|v| game_sources::is_event_horizon(&v))
}

/// Puts `hmod` into `home`'s `horizon-mods/` and turns it on.
fn place(hmod: &Path, home: &Path) -> Result<(), String> {
    let dir = home.join("horizon-mods");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let partial = dir.join(format!(".{ID}.hmod.partial"));
    fs::copy(hmod, &partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    fs::rename(&partial, dir.join(format!("{ID}.hmod"))).map_err(|e| e.to_string())?;
    crate::browser::java::set_enabled(home, ID, true)
}

/// Installs `release` into every Horizon instance, turned on, and keeps a copy for instances made
/// later. Returns the version installed.
pub fn install(release: &Release) -> Result<String, String> {
    let file = game_sources::fetch(release, MAX_HMOD)?;
    let made = (|| {
        let manifest = horizon_mods::Package::read(&file)?.manifest;
        if manifest.id != ID {
            return Err(format!(
                "{} is the mod {}, not the modkit.",
                release.asset, manifest.id
            ));
        }
        let keep = kept();
        fs::create_dir_all(keep.parent().unwrap_or(&keep)).map_err(|e| e.to_string())?;
        fs::copy(&file, &keep).map_err(|e| e.to_string())?;
        for home in homes() {
            place(&keep, &home)?;
        }
        Ok(manifest.version)
    })();
    let _ = fs::remove_dir_all(file.parent().unwrap_or(&file));
    let version = made?;
    let mut state = load();
    state.installed = Some(Installed {
        version: version.clone(),
        tag: release.tag.clone(),
    });
    save(&state)?;
    Ok(version)
}

/// Gives each Horizon instance that lacks it the modkit installed: an instance made after the
/// release has it at the next check. Says which instances it went to.
fn fill_new_instances(state: &State) -> Vec<Result<String, String>> {
    let keep = kept();
    if state.installed.is_none() || !keep.is_file() {
        return Vec::new();
    }
    homes()
        .into_iter()
        .filter(|home| {
            !home
                .join("horizon-mods")
                .join(format!("{ID}.hmod"))
                .is_file()
        })
        .map(|home| place(&keep, &home).map(|()| format!("the modkit into {}", home.display())))
        .collect()
}

/// Whether `latest` is a release other than the one installed.
fn is_update(state: &State, latest: &Release) -> bool {
    state.installed.as_ref().is_none_or(|i| i.tag != latest.tag)
}

/// What the launcher's update check does: the modkit into new instances, and its new release when
/// it updates itself.
pub fn update_automatic() -> Vec<Result<String, String>> {
    let state = load();
    let mut done = fill_new_instances(&state);
    if state.auto_update {
        match latest(&crate::github_mods::api) {
            Ok(release) if is_update(&state, &release) => done.push(
                install(&release)
                    .map(|v| format!("the modkit {v}"))
                    .map_err(|e| format!("the modkit: {e}")),
            ),
            Ok(_) => {}
            Err(e) => done.push(Err(format!("the modkit: {e}"))),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modkit_updates_itself_until_turned_off() {
        assert!(parse(None).auto_update);
        assert!(parse(Some("{}")).auto_update);
        assert!(!parse(Some(r#"{"auto_update": false}"#)).auto_update);
        assert_eq!(parse(Some("not json")), State::default());
    }

    #[test]
    fn its_release_is_the_hmod() {
        let json = serde_json::json!({ "tag_name": "v0.7.1", "assets": [
            { "name": "SHA256SUMS", "browser_download_url": format!("https://github.com/{REPO}/releases/download/v0.7.1/SHA256SUMS") },
            { "name": "horizon_modkit.hmod", "browser_download_url": format!("https://github.com/{REPO}/releases/download/v0.7.1/horizon_modkit.hmod") },
        ]});
        let found = pick(&json).unwrap();
        assert_eq!(
            (found.asset.as_str(), found.tag.as_str()),
            ("horizon_modkit.hmod", "v0.7.1")
        );
        let none = serde_json::json!({ "tag_name": "v0.7.1", "assets": [] });
        assert!(pick(&none).is_err());
    }

    #[test]
    fn a_new_release_is_an_update() {
        let release = Release {
            repo: REPO.into(),
            tag: "v0.7.2".into(),
            asset: "horizon_modkit.hmod".into(),
            url: String::new(),
            size: 0,
            sha256: None,
        };
        let on = |tag: &str| State {
            auto_update: true,
            installed: Some(Installed {
                version: tag.trim_start_matches('v').into(),
                tag: tag.into(),
            }),
        };
        assert!(is_update(&State::default(), &release));
        assert!(is_update(&on("v0.7.1"), &release));
        assert!(!is_update(&on("v0.7.2"), &release));
    }
}
