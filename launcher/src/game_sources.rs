//! GitHub sources: game builds a repository publishes as GitHub releases, such as Event Horizon's
//! own (docs/event-horizon-releases.md). A release's archive for this platform
//! (`<name>-<platform>.tar.gz` or `.zip`, holding `horizon-client`) is downloaded, checked against
//! the SHA-256 GitHub lists for it, and installed on an installed version's game files as an
//! imported build (`build_import`). A source the player lets update itself installs each new
//! release when the launcher checks for updates: a download, never a compile.
//!
//! The list lives in `game-sources.json` in the launcher's data folder. With no file, it is
//! [`DEFAULT`], updating itself, so a new launcher plays Event Horizon's newest release. The
//! modkit, which mods need beside the game, is installed and tracked apart (`modkit`).
use crate::backend;
use crate::build_import;
use crate::github_mods::{self, Api, Target};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

/// The source a launcher starts with: Event Horizon's releases.
pub const DEFAULT: &str = "whos-carmen/horizon";
const FILE: &str = "game-sources.json";
/// The most a release archive may be: a game build is ~100 MB, packed.
const MAX_ARCHIVE: u64 = 512 << 20;

/// A repository whose releases are installed here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// `owner/repo`.
    pub repo: String,
    /// Install each new release when the launcher checks for updates.
    #[serde(default)]
    pub auto_update: bool,
    /// The release installed last, and the version it made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<Installed>,
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
    pub tag: String,
    /// The archive for this platform.
    pub asset: String,
    pub url: String,
    /// Its size in bytes, as GitHub lists it.
    pub size: u64,
    /// When the release was published (`2026-11-05T09:12:00Z`), as GitHub says.
    pub published: String,
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
    vec![Source {
        repo: DEFAULT.into(),
        auto_update: true,
        installed: None,
    }]
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

/// Adds the repository `text` names, updating itself when `auto_update`.
pub fn add(text: &str, auto_update: bool) -> Result<String, String> {
    let repo = repo_of(text)?;
    edit(|sources| {
        if sources.iter().any(|s| s.repo.eq_ignore_ascii_case(&repo)) {
            return Err(format!("{repo} is a source already."));
        }
        sources.push(Source {
            repo: repo.clone(),
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

/// The first of a release's files whose name ends with one of `ends`, one of `repo`'s own (a
/// release can't point at another repository's file). Shared with `modkit`.
pub(crate) fn asset(repo: &str, release: &serde_json::Value, ends: &[String]) -> Option<Release> {
    let tag = release["tag_name"].as_str()?.to_owned();
    let own = format!("https://github.com/{repo}/releases/download/");
    let assets = release["assets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    assets.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        let url = a["browser_download_url"].as_str()?;
        (ends.iter().any(|e| name.ends_with(e.as_str())) && url.starts_with(&own)).then(|| {
            Release {
                repo: repo.to_owned(),
                tag: tag.clone(),
                asset: name.to_owned(),
                url: url.to_owned(),
                size: a["size"].as_u64().unwrap_or(0),
                published: release["published_at"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                sha256: a["digest"]
                    .as_str()
                    .and_then(|d| d.strip_prefix("sha256:"))
                    .filter(|d| d.len() == 64)
                    .map(str::to_ascii_lowercase),
            }
        })
    })
}

/// The archive for `platform` in a release's JSON: `<name>-<platform>.tar.gz` or `.zip`.
fn pick(repo: &str, release: &serde_json::Value, platform: &str) -> Result<Release, String> {
    let ends = [format!("-{platform}.tar.gz"), format!("-{platform}.zip")];
    asset(repo, release, &ends).ok_or_else(|| {
        let tag = release["tag_name"].as_str().unwrap_or("its newest release");
        format!("{repo} {tag} has no build for this computer ({platform}).")
    })
}

/// `repo`'s newest release for this computer, through `api`; None when it has no build for this
/// computer (Event Horizon publishes Windows builds only).
pub fn latest(repo: &str, api: Api) -> Result<Option<Release>, String> {
    let Some(platform) = platform() else {
        return Ok(None);
    };
    let release = api(&format!("repos/{repo}/releases/latest"))?
        .ok_or_else(|| format!("{repo} has no releases yet, or is private."))?;
    Ok(pick(repo, &release, platform).ok())
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Downloads `release`'s file into a scratch folder, under its own name, at most `max` bytes, and
/// checks it against the SHA-256 GitHub lists for it. Shared with `modkit`.
pub(crate) fn fetch(release: &Release, max: u64) -> Result<PathBuf, String> {
    let mut body = Vec::new();
    backend::get(&release.url)?
        .into_reader()
        .take(max + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("Download interrupted: {e}"))?;
    if body.len() as u64 > max {
        return Err(format!("{} is bigger than it can be.", release.asset));
    }
    if let Some(expected) = &release.sha256 {
        if sha256(&body) != *expected {
            return Err(format!(
                "{} does not match the checksum GitHub lists for it.",
                release.asset
            ));
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "horizon-source-{}-{}",
        std::process::id(),
        sha256(release.url.as_bytes()).get(..8).unwrap_or_default()
    ));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // The file keeps its name: the import reads a .tar.gz or a .zip by it.
    let file = dir.join(&release.asset);
    fs::write(&file, body).map_err(|e| e.to_string())?;
    Ok(file)
}

fn download(release: &Release) -> Result<PathBuf, String> {
    fetch(release, MAX_ARCHIVE)
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

/// The sources with a release newer than the one installed (or none installed), with it. A
/// source with no build for this computer has nothing to install.
pub fn updates(sources: &[Source], api: Api) -> Vec<(Source, Result<Release, String>)> {
    sources
        .iter()
        .filter_map(|s| match latest(&s.repo, api) {
            Ok(None) => None,
            Ok(Some(r)) if s.installed.as_ref().is_some_and(|i| i.tag == r.tag) => None,
            Ok(Some(r)) => Some((s.clone(), Ok(r))),
            Err(e) => Some((s.clone(), Err(e))),
        })
        .collect()
}

/// What `tools/deploy_local_mac.sh` writes in a version it installs from a checkout.
#[derive(Deserialize)]
struct LocalBuild {
    #[serde(default)]
    commit: String,
}

/// A version built here from Event Horizon's checkout (`tools/deploy_local_mac.sh`): its commit.
fn local_build(version: &str) -> Option<String> {
    let text = fs::read_to_string(backend::version_dir(version).join(".local_build")).ok()?;
    let build: LocalBuild = serde_json::from_str(&text).ok()?;
    Some(build.commit.chars().take(7).collect())
}

/// An Event Horizon release installed from its GitHub source: the release as a player reads it
/// (`0.1.0`, `Nightly 2026-11-05`).
fn release_build(version: &str) -> Option<String> {
    let about = build_import::about(version)?;
    let tag = about.file.strip_prefix(DEFAULT)?.strip_prefix(' ')?;
    Some(release_name(tag))
}

/// `eh-v0.1.0` → `0.1.0`, `eh-nightly-2026-11-05` → `Nightly 2026-11-05`.
fn release_name(tag: &str) -> String {
    if let Some(date) = tag.strip_prefix(NIGHTLY_PREFIX) {
        return format!("Nightly {date}");
    }
    tag.strip_prefix(TAG_PREFIX).unwrap_or(tag).to_owned()
}

/// Whether an installed version is Event Horizon's: built here, or one of its releases.
pub fn is_event_horizon(version: &str) -> bool {
    local_build(version).is_some() || release_build(version).is_some()
}

/// An imported build that is not one of Event Horizon's releases (the Versions page's "Imported").
pub fn is_other_import(version: &str) -> bool {
    build_import::is_imported(version) && release_build(version).is_none()
}

/// A version as a player reads it: `Event Horizon 0.1.0`, `Event Horizon · local 752a5a5`, else
/// what Horizon calls it (`nightly::label`).
pub fn label(version: &str) -> Option<String> {
    if let Some(release) = release_build(version) {
        return Some(format!("Event Horizon {release}"));
    }
    if let Some(commit) = local_build(version) {
        return Some(match commit.is_empty() {
            true => "Event Horizon · local".to_owned(),
            false => format!("Event Horizon · local {commit}"),
        });
    }
    crate::nightly::label(version)
}

/// Event Horizon's release tags: `eh-v<version>` for a version of the owner's choosing,
/// `eh-nightly-<date>` for a nightly (event-horizon-release.yml).
pub const TAG_PREFIX: &str = "eh-v";
pub const NIGHTLY_PREFIX: &str = "eh-nightly-";

pub fn is_event_horizon_tag(tag: &str) -> bool {
    tag.starts_with(TAG_PREFIX) || tag.starts_with(NIGHTLY_PREFIX)
}

/// An `eh-v<major>.<minor>.<patch>` tag's numbers, missing ones as zero; None for any other tag.
fn numbers(tag: &str) -> Option<[u64; 3]> {
    let version = tag.strip_prefix(TAG_PREFIX)?;
    let version = version.split(['-', '+']).next().unwrap_or(version);
    let mut out = [0; 3];
    for (slot, part) in out.iter_mut().zip(version.split('.')) {
        *slot = part.parse().ok()?;
    }
    Some(out)
}

/// The release this launcher was built for, when the release workflow built it
/// (`HORIZON_LAUNCHER_TAG`); None for a launcher built by hand, which never updates itself.
fn own_tag() -> Option<&'static str> {
    option_env!("HORIZON_LAUNCHER_TAG").filter(|t| is_event_horizon_tag(t))
}

/// When the release workflow built this launcher (`HORIZON_LAUNCHER_BUILT`, `2026-11-05T09:03:00Z`).
fn own_built() -> &'static str {
    option_env!("HORIZON_LAUNCHER_BUILT").unwrap_or_default()
}

/// Whether release `latest`, published at `published`, is newer than this launcher's release
/// `own`, built at `built`: two versions compare by their numbers; otherwise (a nightly on either
/// side) a release published after this launcher was built is newer.
fn newer(latest: &str, published: &str, own: Option<&str>, built: &str) -> bool {
    let Some(own) = own else {
        return false;
    };
    if latest == own {
        return false;
    }
    match (numbers(latest), numbers(own)) {
        (Some(latest), Some(own)) => latest > own,
        _ => !published.is_empty() && !built.is_empty() && published > built,
    }
}

/// The launcher's own update, from Event Horizon's releases, never Horizon's: the newest release
/// with an archive for this computer, when it is newer than the one this launcher came with.
pub fn launcher_update(api: Api) -> Option<backend::Release> {
    let found = latest(DEFAULT, api).ok().flatten()?;
    newer(&found.tag, &found.published, own_tag(), own_built()).then(|| backend::Release {
        name: format!("Event Horizon {}", release_name(&found.tag)),
        body: String::new(),
        date: String::new(),
        time: String::new(),
        asset: Some((found.url.clone(), found.size)),
        prerelease: false,
        nightly: None,
        tag: found.tag,
    })
}

/// The SHA-256 GitHub lists for Event Horizon release `tag`'s archive for this computer: what a
/// launcher update from it is checked against (Horizon's releases are checked against their signed
/// checksums instead).
pub fn launcher_digest(tag: &str, api: Api) -> Result<String, String> {
    let platform = platform().ok_or("Event Horizon has no launcher for this computer.")?;
    let release = api(&format!("repos/{DEFAULT}/releases/tags/{tag}"))?
        .ok_or_else(|| format!("Event Horizon has no release {tag}."))?;
    pick(DEFAULT, &release, platform)?
        .sha256
        .ok_or_else(|| format!("GitHub lists no checksum for Event Horizon {tag}: not installed."))
}

/// The installed versions that a newer release of their source would replace, with that release:
/// what the Play button offers as an update for a version a source installed. A version no source
/// installed (a local build, one of Horizon's) has none here.
pub fn pending() -> Vec<(String, Release)> {
    let sources: Vec<Source> = load()
        .into_iter()
        .filter(|s| s.installed.is_some())
        .collect();
    updates(&sources, &github_mods::api)
        .into_iter()
        .filter_map(|(source, latest)| Some((source.installed?.version, latest.ok()?)))
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
    let mut done: Vec<Result<String, String>> = updates(&sources, &github_mods::api)
        .into_iter()
        .map(|(source, latest)| {
            let release = latest.map_err(|e| format!("{}: {e}", source.repo))?;
            install(&release, false)
                .map(|v| format!("{} → {v}", release.label()))
                .map_err(|e| format!("{}: {e}", release.label()))
        })
        .collect();
    // The modkit, which mods need beside the game, updates at the same check.
    done.extend(crate::modkit::update_automatic());
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_updates_only_to_a_newer_event_horizon_release() {
        let built = "2026-11-05T09:03:00Z";
        // Two versions: by their numbers.
        assert!(newer("eh-v0.2.0", "", Some("eh-v0.1.9"), built));
        assert!(newer("eh-v0.10.0", "", Some("eh-v0.9.0"), built));
        assert!(!newer("eh-v0.1.0", "", Some("eh-v0.1.0"), built));
        assert!(!newer("eh-v0.1.0", "", Some("eh-v0.2.0"), built));
        // A nightly on either side: by when it was published, after this launcher was built.
        assert!(newer(
            "eh-nightly-2026-11-06",
            "2026-11-06T09:20:00Z",
            Some("eh-nightly-2026-11-05"),
            built
        ));
        assert!(newer(
            "eh-nightly-2026-11-06",
            "2026-11-06T09:20:00Z",
            Some("eh-v0.1.0"),
            built
        ));
        assert!(!newer(
            "eh-nightly-2026-11-05",
            "2026-11-05T09:01:00Z",
            Some("eh-v0.1.0"),
            built
        ));
        assert!(!newer(
            "eh-nightly-2026-11-05",
            "2026-11-05T09:20:00Z",
            Some("eh-nightly-2026-11-05"),
            built
        ));
        // A launcher built by hand never updates from here.
        assert!(!newer("eh-v0.2.0", "2027-01-01T00:00:00Z", None, built));
        assert!(is_event_horizon_tag("eh-v0.1.0") && is_event_horizon_tag("eh-nightly-2026-11-05"));
        assert!(!is_event_horizon_tag("launcher@0.2.40"));
        assert_eq!(release_name("eh-nightly-2026-11-05"), "Nightly 2026-11-05");
        assert_eq!(release_name("eh-v0.1.0"), "0.1.0");
    }

    #[test]
    fn a_new_launcher_follows_event_horizon_by_itself() {
        let list = parse(None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].repo, DEFAULT);
        assert!(list[0].auto_update);
        // A list the player saved, even an empty one, is theirs.
        assert!(parse(Some("[]")).is_empty());
        let saved = r#"[{"repo": "a/b", "auto_update": false}]"#;
        assert!(!parse(Some(saved))[0].auto_update);
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
        let windows = pick("a/b", &json, "windows-x64").unwrap();
        assert_eq!(windows.asset, "event-horizon-windows-x64.zip");
        assert_eq!(windows.sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(windows.label(), "a/b eh-v0.1.0");
        let linux = pick("a/b", &json, "linux-x86_64").unwrap();
        assert_eq!(linux.sha256, None);
        assert!(pick("a/b", &json, "macos-arm64")
            .unwrap_err()
            .contains("no build for this computer"));
    }

    #[test]
    fn another_repositorys_file_is_not_picked() {
        let json = release(serde_json::json!([
            { "name": "event-horizon-linux-x86_64.tar.gz",
              "browser_download_url": "https://github.com/someone/else/releases/download/x/event-horizon-linux-x86_64.tar.gz" },
        ]));
        assert!(pick("a/b", &json, "linux-x86_64").is_err());
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
            auto_update: true,
            installed: Some(Installed {
                tag: "eh-v0.1.0".into(),
                version: "imported-x".into(),
            }),
        };
        let new = Source {
            repo: "c/d".into(),
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
