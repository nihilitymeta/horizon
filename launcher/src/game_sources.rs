//! GitHub sources: game builds a repository publishes as GitHub releases, such as Event Horizon's
//! own (docs/event-horizon-releases.md). A release's archive for this platform
//! (`<name>-<platform>.tar.gz` or `.zip`, holding `horizon-client`) is downloaded, checked against
//! the SHA-256 GitHub lists for it, and installed on an installed version's game files as an
//! imported build (`build_import`). A source the player lets update itself installs each new
//! release when the launcher checks for updates: a download, never a compile.
//!
//! A source can also publish a content mod (`.hmod`), such as the modkit: its release goes into
//! every Horizon instance's `horizon-mods/` and is turned on there.
//!
//! The list lives in `game-sources.json` in the launcher's data folder. With no file, it is
//! [`DEFAULT`] and [`MODKIT`], both updating themselves, so a new launcher plays Event Horizon's
//! newest release with the modkit installed.
use crate::backend;
use crate::build_import;
use crate::github_mods::{self, Api, Target};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

/// The sources a launcher starts with: Event Horizon's releases, and the modkit's.
pub const DEFAULT: &str = "whos-carmen/horizon";
pub const MODKIT: &str = "whos-carmen/horizon-modkit";
const FILE: &str = "game-sources.json";
/// The most a release archive may be: a game build is ~100 MB, packed.
const MAX_ARCHIVE: u64 = 512 << 20;

/// A repository whose releases are installed here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// `owner/repo`.
    pub repo: String,
    #[serde(default)]
    pub kind: Kind,
    /// Install each new release when the launcher checks for updates.
    #[serde(default)]
    pub auto_update: bool,
    /// The release installed last, and the version it made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<Installed>,
}

/// What a source's releases are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Game builds, `<name>-<platform>.tar.gz` or `.zip`: installed as a version.
    #[default]
    Game,
    /// A content mod, `<id>.hmod`: installed into every Horizon instance and turned on.
    Mod,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub tag: String,
    pub version: String,
}

/// A source's newest release, as far as this platform goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub repo: String,
    pub kind: Kind,
    pub tag: String,
    /// The archive for this platform.
    pub asset: String,
    pub url: String,
    /// The SHA-256 GitHub lists for it, when it does: the download must match.
    pub sha256: Option<String>,
}

impl Release {
    /// What the Versions page says an installed build was made from.
    pub fn label(&self) -> String {
        format!("{} {}", self.repo, self.tag)
    }
}

/// This computer's name in a release archive's file name, or None where no source builds for it.
pub fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("windows", "x86_64") => Some("windows-x64"),
        ("macos", "aarch64") => Some("macos-arm64"),
        _ => None,
    }
}

fn path() -> PathBuf {
    backend::data_dir().join(FILE)
}

fn default_list() -> Vec<Source> {
    [(DEFAULT, Kind::Game), (MODKIT, Kind::Mod)]
        .into_iter()
        .map(|(repo, kind)| Source {
            repo: repo.into(),
            kind,
            auto_update: true,
            installed: None,
        })
        .collect()
}

/// The sources in `text`, or [`DEFAULT`] when there is no list yet.
fn parse(text: Option<&str>) -> Vec<Source> {
    match text {
        None => default_list(),
        Some(text) => serde_json::from_str(text).unwrap_or_else(|_| default_list()),
    }
}

pub fn load() -> Vec<Source> {
    parse(fs::read_to_string(path()).ok().as_deref())
}

pub fn save(sources: &[Source]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(sources).map_err(|e| e.to_string())?;
    fs::create_dir_all(backend::data_dir()).map_err(|e| e.to_string())?;
    fs::write(path(), text).map_err(|e| e.to_string())
}

/// The `owner/repo` a link or `owner/repo` names: a repository, not a branch, tag or commit.
pub fn repo_of(text: &str) -> Result<String, String> {
    let text = text.trim();
    let link = if text.starts_with("https://") {
        text.to_owned()
    } else {
        format!("https://github.com/{text}")
    };
    let link = github_mods::parse(&link)?;
    if link.target != Target::Latest {
        return Err(format!(
            "{text}: name the repository (https://github.com/<owner>/<repo>): its newest release is installed."
        ));
    }
    Ok(format!("{}/{}", link.owner, link.repo))
}

fn edit(change: impl FnOnce(&mut Vec<Source>) -> Result<(), String>) -> Result<(), String> {
    let mut sources = load();
    change(&mut sources)?;
    save(&sources)
}

/// Adds the repository `text` names, of `kind`, updating itself when `auto_update`.
pub fn add(text: &str, kind: Kind, auto_update: bool) -> Result<String, String> {
    let repo = repo_of(text)?;
    edit(|sources| {
        if sources.iter().any(|s| s.repo.eq_ignore_ascii_case(&repo)) {
            return Err(format!("{repo} is a source already."));
        }
        sources.push(Source {
            repo: repo.clone(),
            kind,
            auto_update,
            installed: None,
        });
        Ok(())
    })?;
    Ok(repo)
}

/// Forgets a source. The versions it installed stay: remove them on the Versions page.
pub fn remove(repo: &str) -> Result<(), String> {
    edit(|sources| {
        let before = sources.len();
        sources.retain(|s| !s.repo.eq_ignore_ascii_case(repo));
        if sources.len() == before {
            return Err(format!("{repo} is not a source."));
        }
        Ok(())
    })
}

/// Turns a source's automatic updates on or off.
pub fn set_auto_update(repo: &str, on: bool) -> Result<(), String> {
    edit(|sources| {
        let source = sources
            .iter_mut()
            .find(|s| s.repo.eq_ignore_ascii_case(repo))
            .ok_or_else(|| format!("{repo} is not a source."))?;
        source.auto_update = on;
        Ok(())
    })
}

/// The file to install in a release's JSON, one of this repository's own: for a game source the
/// archive for `platform`, `<name>-<platform>.tar.gz` or `.zip`; for a mod source its `.hmod`.
fn pick(
    repo: &str,
    kind: Kind,
    release: &serde_json::Value,
    platform: &str,
) -> Result<Release, String> {
    let tag = release["tag_name"]
        .as_str()
        .ok_or("GitHub did not name the release's tag.")?
        .to_owned();
    let own = format!("https://github.com/{repo}/releases/download/");
    let ends = match kind {
        Kind::Game => vec![format!("-{platform}.tar.gz"), format!("-{platform}.zip")],
        Kind::Mod => vec![".hmod".to_owned()],
    };
    let assets = release["assets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    assets
        .iter()
        .find_map(|a| {
            let name = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str()?;
            (ends.iter().any(|e| name.ends_with(e.as_str())) && url.starts_with(&own)).then(|| {
                Release {
                    repo: repo.to_owned(),
                    kind,
                    tag: tag.clone(),
                    asset: name.to_owned(),
                    url: url.to_owned(),
                    sha256: a["digest"]
                        .as_str()
                        .and_then(|d| d.strip_prefix("sha256:"))
                        .filter(|d| d.len() == 64)
                        .map(str::to_ascii_lowercase),
                }
            })
        })
        .ok_or_else(|| match kind {
            Kind::Game => format!("{repo} {tag} has no build for this computer ({platform})."),
            Kind::Mod => format!("{repo} {tag} has no .hmod."),
        })
}

/// `repo`'s newest release for this computer, through `api`; None when it has no build for this
/// computer (Event Horizon publishes Windows builds only). A mod is the same everywhere.
pub fn latest(repo: &str, kind: Kind, api: Api) -> Result<Option<Release>, String> {
    let platform = match (kind, platform()) {
        (Kind::Game, None) => return Ok(None),
        (_, p) => p.unwrap_or_default(),
    };
    let release = api(&format!("repos/{repo}/releases/latest"))?
        .ok_or_else(|| format!("{repo} has no releases yet, or is private."))?;
    match kind {
        Kind::Game => Ok(pick(repo, kind, &release, platform).ok()),
        Kind::Mod => pick(repo, kind, &release, platform).map(Some),
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn download(release: &Release) -> Result<PathBuf, String> {
    let mut body = Vec::new();
    backend::get(&release.url)?
        .into_reader()
        .take(MAX_ARCHIVE + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("Download interrupted: {e}"))?;
    if body.len() as u64 > MAX_ARCHIVE {
        return Err(format!("{} is bigger than a game build.", release.asset));
    }
    if let Some(expected) = &release.sha256 {
        if sha256(&body) != *expected {
            return Err(format!(
                "{} does not match the checksum GitHub lists for it.",
                release.asset
            ));
        }
    }
    let dir = std::env::temp_dir().join(format!("horizon-source-{}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // The archive keeps its name: the import reads a .tar.gz or a .zip by it.
    let file = dir.join(&release.asset);
    fs::write(&file, body).map_err(|e| e.to_string())?;
    Ok(file)
}

/// Whether installing a source's new build moves the selection to it: when the player has none,
/// is on a version Horizon published (so a new launcher moves to the source's build), or on the
/// build the source installed before. A version picked by hand stays.
fn selects(selected: Option<&str>, before: Option<&Installed>) -> bool {
    match selected {
        None => true,
        Some(s) if backend::is_game_tag(s) || crate::nightly::date_of(s).is_some() => true,
        Some(s) => before.is_some_and(|b| b.version == s),
    }
}

/// Installs `release` and records it as its source's; selects it as [`selects`] says, or always
/// when `select`. Horizon's newest release is installed first when there are no game files yet.
/// Returns the version it made.
pub fn install(release: &Release, select: bool) -> Result<String, String> {
    if release.kind == Kind::Mod {
        return install_mod(release);
    }
    let before = load()
        .into_iter()
        .find(|s| s.repo.eq_ignore_ascii_case(&release.repo))
        .and_then(|s| s.installed);
    let settings = backend::Settings::load();
    let select = select || selects(settings.selected.as_deref(), before.as_ref());
    let has_game_files = backend::installed()
        .iter()
        .any(|v| backend::is_game_tag(v) || crate::nightly::date_of(v).is_some());
    if !has_game_files {
        backend::install_tag("latest", true, |_| {})?;
    }
    let file = download(release)?;
    let made = build_import::import_as(&file, None, &release.label(), select);
    let _ = fs::remove_dir_all(file.parent().unwrap_or(&file));
    let version = made?;
    edit(|sources| {
        if let Some(source) = sources
            .iter_mut()
            .find(|s| s.repo.eq_ignore_ascii_case(&release.repo))
        {
            source.installed = Some(Installed {
                tag: release.tag.clone(),
                version: version.clone(),
            });
        }
        Ok(())
    })?;
    // An instance pinned to the build it replaces plays the new one; the old one goes, unless the
    // player still has it selected.
    if let Some(old) = before.filter(|b| b.version != version) {
        for instance in crate::instances::list() {
            if instance.version.as_deref() == Some(old.version.as_str()) {
                let _ = crate::instances::edit(&instance.id, |meta| {
                    meta.version = Some(version.clone())
                });
            }
        }
        let still = backend::Settings::load().selected.as_deref() == Some(old.version.as_str());
        if !still {
            let _ = backend::remove_version(&old.version);
        }
    }
    Ok(version)
}

/// Where a mod source keeps the `.hmod` it installed, for instances made after it.
fn kept(repo: &str) -> PathBuf {
    backend::data_dir()
        .join("game-sources")
        .join(repo.replace('/', "-"))
}

/// The Horizon instances' game folders, where content mods go.
fn horizon_homes() -> Vec<PathBuf> {
    crate::instances::list()
        .into_iter()
        .filter(|i| i.java.is_none())
        .map(|i| i.home)
        .collect()
}

/// Puts `hmod` into `home`'s `horizon-mods/` as `<id>.hmod` and turns it on.
fn place(hmod: &std::path::Path, id: &str, home: &std::path::Path) -> Result<(), String> {
    let dir = home.join("horizon-mods");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let partial = dir.join(format!(".{id}.hmod.partial"));
    fs::copy(hmod, &partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    fs::rename(&partial, dir.join(format!("{id}.hmod"))).map_err(|e| e.to_string())?;
    crate::browser::java::set_enabled(home, id, true)
}

/// Installs a mod source's release into every Horizon instance, turned on, and keeps a copy for
/// instances made later ([`fill_new_instances`]).
fn install_mod(release: &Release) -> Result<String, String> {
    let file = download(release)?;
    let id = horizon_mods::Package::read(&file).map(|p| p.manifest.id);
    let made = id.and_then(|id| {
        let keep = kept(&release.repo);
        fs::create_dir_all(&keep).map_err(|e| e.to_string())?;
        let copy = keep.join(format!("{id}.hmod"));
        fs::copy(&file, &copy).map_err(|e| e.to_string())?;
        let homes = horizon_homes();
        for home in &homes {
            place(&copy, &id, home)?;
        }
        Ok((id, homes.len()))
    });
    let _ = fs::remove_dir_all(file.parent().unwrap_or(&file));
    let (id, instances) = made?;
    edit(|sources| {
        if let Some(source) = sources
            .iter_mut()
            .find(|s| s.repo.eq_ignore_ascii_case(&release.repo))
        {
            source.installed = Some(Installed {
                tag: release.tag.clone(),
                version: id.clone(),
            });
        }
        Ok(())
    })?;
    Ok(format!("{id} in {instances} instance(s)"))
}

/// Gives each Horizon instance that lacks it the mod a mod source installed: an instance made
/// after the release has it at the next check, without waiting for the next release.
fn fill_new_instances(sources: &[Source]) -> Vec<Result<String, String>> {
    let mut done = Vec::new();
    for source in sources.iter().filter(|s| s.kind == Kind::Mod) {
        let Some(installed) = &source.installed else {
            continue;
        };
        let copy = kept(&source.repo).join(format!("{}.hmod", installed.version));
        if !copy.is_file() {
            continue;
        }
        for home in horizon_homes() {
            let there = home
                .join("horizon-mods")
                .join(format!("{}.hmod", installed.version));
            if !there.is_file() {
                done.push(
                    place(&copy, &installed.version, &home)
                        .map(|()| format!("{} into {}", installed.version, home.display())),
                );
            }
        }
    }
    done
}

/// The sources with a release newer than the one installed (or none installed), with it. A
/// source with no build for this computer has nothing to install.
pub fn updates(sources: &[Source], api: Api) -> Vec<(Source, Result<Release, String>)> {
    sources
        .iter()
        .filter_map(|s| match latest(&s.repo, s.kind, api) {
            Ok(None) => None,
            Ok(Some(r)) if s.installed.as_ref().is_some_and(|i| i.tag == r.tag) => None,
            Ok(Some(r)) => Some((s.clone(), Ok(r))),
            Err(e) => Some((s.clone(), Err(e))),
        })
        .collect()
}

/// What the launcher's update check does: installs each new release of a source that updates
/// itself. Returns what it installed, and what failed.
pub fn update_automatic() -> Vec<Result<String, String>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    // One update at a time: a check that comes while one installs leaves it to that one.
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Vec::new();
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let _done = Done;
    let sources: Vec<Source> = load().into_iter().filter(|s| s.auto_update).collect();
    let mut done = fill_new_instances(&sources);
    done.extend(
        updates(&sources, &github_mods::api)
            .into_iter()
            .map(|(source, latest)| {
                let release = latest.map_err(|e| format!("{}: {e}", source.repo))?;
                install(&release, false)
                    .map(|v| format!("{} → {v}", release.label()))
                    .map_err(|e| format!("{}: {e}", release.label()))
            }),
    );
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_launcher_follows_event_horizon_by_itself() {
        let list = parse(None);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].repo.as_str(), list[0].kind), (DEFAULT, Kind::Game));
        assert_eq!((list[1].repo.as_str(), list[1].kind), (MODKIT, Kind::Mod));
        assert!(list.iter().all(|s| s.auto_update));
        // A list the player saved, even an empty one, is theirs.
        assert!(parse(Some("[]")).is_empty());
        // A list saved before sources had kinds holds game sources.
        let saved = r#"[{"repo": "a/b", "auto_update": false}]"#;
        assert!(!parse(Some(saved))[0].auto_update);
        assert_eq!(parse(Some(saved))[0].kind, Kind::Game);
    }

    #[test]
    fn a_source_is_a_repository() {
        assert_eq!(
            repo_of("https://github.com/whos-carmen/horizon").unwrap(),
            "whos-carmen/horizon"
        );
        assert_eq!(
            repo_of("whos-carmen/horizon").unwrap(),
            "whos-carmen/horizon"
        );
        assert_eq!(repo_of("https://github.com/a/b/releases").unwrap(), "a/b");
        assert!(repo_of("https://github.com/a/b/tree/main").is_err());
        assert!(repo_of("https://gitlab.com/a/b").is_err());
    }

    fn release(assets: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "tag_name": "eh-v0.1.0", "assets": assets })
    }

    #[test]
    fn the_archive_for_this_platform_is_picked() {
        let digest = "a".repeat(64);
        let json = release(serde_json::json!([
            { "name": "SHA256SUMS", "browser_download_url": "https://github.com/a/b/releases/download/eh-v0.1.0/SHA256SUMS" },
            { "name": "event-horizon-windows-x64.zip", "digest": format!("sha256:{digest}"),
              "browser_download_url": "https://github.com/a/b/releases/download/eh-v0.1.0/event-horizon-windows-x64.zip" },
            { "name": "event-horizon-linux-x86_64.tar.gz",
              "browser_download_url": "https://github.com/a/b/releases/download/eh-v0.1.0/event-horizon-linux-x86_64.tar.gz" },
        ]));
        let windows = pick("a/b", Kind::Game, &json, "windows-x64").unwrap();
        assert_eq!(windows.asset, "event-horizon-windows-x64.zip");
        assert_eq!(windows.sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(windows.label(), "a/b eh-v0.1.0");
        let linux = pick("a/b", Kind::Game, &json, "linux-x86_64").unwrap();
        assert_eq!(linux.sha256, None);
        assert!(pick("a/b", Kind::Game, &json, "macos-arm64")
            .unwrap_err()
            .contains("no build for this computer"));
    }

    #[test]
    fn a_mod_sources_release_is_its_hmod() {
        let json = release(serde_json::json!([
            { "name": "SHA256SUMS", "browser_download_url": "https://github.com/a/m/releases/download/v0.7.1/SHA256SUMS" },
            { "name": "horizon_modkit.hmod", "browser_download_url": "https://github.com/a/m/releases/download/v0.7.1/horizon_modkit.hmod" },
        ]));
        let found = pick("a/m", Kind::Mod, &json, "").unwrap();
        assert_eq!(
            (found.asset.as_str(), found.kind),
            ("horizon_modkit.hmod", Kind::Mod)
        );
        let none = release(serde_json::json!([]));
        assert!(pick("a/m", Kind::Mod, &none, "")
            .unwrap_err()
            .contains("no .hmod"));
    }

    #[test]
    fn another_repositorys_file_is_not_picked() {
        let json = release(serde_json::json!([
            { "name": "event-horizon-linux-x86_64.tar.gz",
              "browser_download_url": "https://github.com/someone/else/releases/download/x/event-horizon-linux-x86_64.tar.gz" },
        ]));
        assert!(pick("a/b", Kind::Game, &json, "linux-x86_64").is_err());
    }

    #[test]
    fn an_update_moves_only_a_player_who_was_on_it() {
        let before = Installed {
            tag: "eh-v0.1.0".into(),
            version: "imported-2026-11-04-abcdef1".into(),
        };
        // No selection, or a version Horizon published: a new launcher plays the source's build.
        assert!(selects(None, None));
        assert!(selects(Some("v2026.10.13"), Some(&before)));
        // The source's own build before: the update takes its place.
        assert!(selects(Some("imported-2026-11-04-abcdef1"), Some(&before)));
        // Something picked by hand (a local build, another import) stays.
        assert!(!selects(Some("local-2026-11-04-d164a4523"), Some(&before)));
        assert!(!selects(Some("imported-2026-11-01-1234567"), Some(&before)));
    }

    #[test]
    fn a_source_with_the_newest_release_has_no_update() {
        let installed = Source {
            repo: "a/b".into(),
            kind: Kind::Game,
            auto_update: true,
            installed: Some(Installed {
                tag: "eh-v0.1.0".into(),
                version: "imported-x".into(),
            }),
        };
        let new = Source {
            repo: "c/d".into(),
            kind: Kind::Game,
            auto_update: true,
            installed: None,
        };
        let api = |path: &str| -> Result<Option<serde_json::Value>, String> {
            let repo = path
                .trim_start_matches("repos/")
                .trim_end_matches("/releases/latest");
            let p = platform().unwrap_or("linux-x86_64");
            Ok(Some(
                serde_json::json!({ "tag_name": "eh-v0.1.0", "assets": [{
                    "name": format!("event-horizon-{p}.tar.gz"),
                    "browser_download_url": format!("https://github.com/{repo}/releases/download/eh-v0.1.0/event-horizon-{p}.tar.gz"),
                }]}),
            ))
        };
        if platform().is_none() {
            return;
        }
        let found = updates(&[installed, new], &api);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.repo, "c/d");
        assert_eq!(found[0].1.as_ref().unwrap().tag, "eh-v0.1.0");
    }
}
