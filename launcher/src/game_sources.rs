//! GitHub sources: game builds a repository publishes as GitHub releases, such as Event Horizon's
//! own (docs/event-horizon-releases.md). This module holds the list of sources, the saved file
//! that keeps it, the repository names, and the newest release of a source for this computer.
//! A release's archive for this platform (`<name>-<platform>.tar.gz` or `.zip`) is what a later
//! step downloads and installs.
//!
//! The list lives in `game-sources.json` in the launcher's data folder. With no file, it is
//! [`DEFAULT`], updating itself, so a new launcher plays Event Horizon's newest release.
// 6b2 to 6b4 (download, install, the page and the CLI) use the rest of this module.
#![allow(dead_code)]

use crate::backend;
use crate::build_import;
use crate::github_mods::{self, Target};
use crate::i18n::{tr, trf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// What GitHub's JSON for one resource is read through: `Ok(None)` for a 404, as
/// `github::json_optional` gives it.
pub type Api<'a> = &'a dyn Fn(&str) -> Result<Option<serde_json::Value>, String>;

/// The source a launcher starts with: Event Horizon's releases (`nihilitymeta/horizon`, moved there
/// from `whos-carmen/horizon` on 2026-11-11).
pub const DEFAULT: &str = "nihilitymeta/horizon";
/// Repositories that moved, old name then new: a saved list that names an old one is renamed to the
/// new one when it loads (`migrate`).
const RENAMED: &[(&str, &str)] = &[
    ("whos-carmen/horizon", "nihilitymeta/horizon"),
    ("whos-carmen/horizon-modkit", "nihilitymeta/horizon-modkit"),
];
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
        Some(text) => serde_json::from_str(text)
            .map(migrate)
            .unwrap_or_else(|_| default_list()),
    }
}

/// `repo` under its new name when it moved (`RENAMED`), else as it is.
fn renamed(repo: &str) -> String {
    RENAMED
        .iter()
        .find(|(old, _)| same_repo(old, repo))
        .map_or_else(|| repo.to_owned(), |(_, new)| (*new).to_owned())
}

/// A saved list with its moved repositories renamed. Where that leaves two entries for one
/// repository, one stays: the one with an install, else the first.
fn migrate(sources: Vec<Source>) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::new();
    for mut source in sources {
        source.repo = renamed(&source.repo);
        match out.iter().position(|s| same_repo(&s.repo, &source.repo)) {
            None => out.push(source),
            Some(i) if out[i].installed.is_none() && source.installed.is_some() => out[i] = source,
            Some(_) => {}
        }
    }
    out
}

/// The saved source list, or [`DEFAULT`] when there is none.
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
        return Err(trf(
            "{text}: name the repository (https://github.com/<owner>/<repo>): its newest release is installed.",
            &[("text", text)],
        ));
    }
    Ok(format!("{}/{}", link.owner, link.repo))
}

/// Whether two `owner/repo` names are the same repository: GitHub's names do not depend on case.
pub fn same_repo(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Loads the list, changes it with `change`, and saves it.
fn edit(change: impl FnOnce(&mut Vec<Source>) -> Result<(), String>) -> Result<(), String> {
    let mut sources = load();
    change(&mut sources)?;
    save(&sources)
}

/// The first of a release's files whose name ends with one of `ends`, one of `repo`'s own or of the
/// repository the release's own page names (a moved repository's releases still link the new name);
/// a release can't point at another repository's file.
pub(crate) fn asset(repo: &str, release: &serde_json::Value, ends: &[String]) -> Option<Release> {
    let tag = release["tag_name"].as_str()?.to_owned();
    let home = html_repo(release);
    let assets = release["assets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    assets.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        let url = a["browser_download_url"].as_str()?;
        let ours = in_repo(url, repo) || home.as_deref().is_some_and(|h| in_repo(url, h));
        (ends.iter().any(|e| name.ends_with(e.as_str())) && ours).then(|| Release {
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
        })
    })
}

/// The `owner/name` a release's `html_url` names (`https://github.com/<owner>/<name>/releases/tag/<tag>`);
/// None for any other link, or for a name with anything but letters, digits, `.`, `_` and `-`.
fn html_repo(release: &serde_json::Value) -> Option<String> {
    let link = release["html_url"].as_str()?;
    let (repo, _tag) = link
        .strip_prefix("https://github.com/")?
        .split_once("/releases/tag/")?;
    let (owner, name) = repo.split_once('/')?;
    let plain = |part: &str| {
        !matches!(part, "" | "." | "..")
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    (plain(owner) && plain(name)).then(|| format!("{owner}/{name}"))
}

/// Whether `url` is a plain download from `repo`'s releases:
/// `https://github.com/<owner>/<name>/releases/download/<tag>/<file>`, the owner and name compared
/// without case. The launcher has no URL parser of its own (its `url` is only ureq's), so this is
/// checked on the text: anything that could leave the path (`..`, a `%` escape, a backslash, a
/// userinfo, port, query or fragment, a third segment) is refused, and so is any character outside
/// a file name's set.
fn in_repo(url: &str, repo: &str) -> bool {
    if url.contains("..") {
        return false;
    }
    let Some((of, path)) = url
        .strip_prefix("https://github.com/")
        .and_then(|rest| rest.split_once("/releases/download/"))
    else {
        return false;
    };
    let mut segments = path.split('/');
    let (Some(tag), Some(file), None) = (segments.next(), segments.next(), segments.next()) else {
        return false;
    };
    same_repo(of, repo) && [tag, file].into_iter().all(plain_segment)
}

/// A path segment that is a tag or a file name: letters, digits and `. _ + -`, not `.` or `..`.
fn plain_segment(part: &str) -> bool {
    !matches!(part, "" | "." | "..")
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// The archive for `platform` in a release's JSON: `<name>-<platform>.tar.gz` or `.zip`.
fn pick(repo: &str, release: &serde_json::Value, platform: &str) -> Result<Release, String> {
    let ends = [format!("-{platform}.tar.gz"), format!("-{platform}.zip")];
    asset(repo, release, &ends).ok_or_else(|| {
        let tag = release["tag_name"]
            .as_str()
            .unwrap_or(tr("its newest release"));
        trf(
            "{repo} {tag} has no build for this computer ({platform}).",
            &[("repo", repo), ("tag", tag), ("platform", platform)],
        )
    })
}

/// `repo`'s newest release for this computer, through `api`; None when it has no build for this
/// computer (Event Horizon publishes Windows x64 and Linux x86_64 builds).
pub fn latest(repo: &str, api: Api) -> Result<Option<Release>, String> {
    let Some(platform) = platform() else {
        return Ok(None);
    };
    let release = api(&format!("repos/{repo}/releases/latest"))?.ok_or_else(|| {
        trf(
            "{repo} has no releases yet, or is private.",
            &[("repo", repo)],
        )
    })?;
    Ok(pick(repo, &release, platform).ok())
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Checks a downloaded `body` of `release`: at most `max` bytes, and the SHA-256 GitHub lists for
/// it. A release that lists no digest is refused, so nothing is installed unchecked.
pub(crate) fn verify(release: &Release, body: &[u8], max: u64) -> Result<(), String> {
    let asset = release.asset.as_str();
    if body.len() as u64 > max {
        return Err(trf(
            "{asset} is bigger than it can be.",
            &[("asset", asset)],
        ));
    }
    let Some(expected) = &release.sha256 else {
        return Err(trf(
            "{asset} lists no checksum, so it is not installed.",
            &[("asset", asset)],
        ));
    };
    if sha256(body) != *expected {
        return Err(trf(
            "{asset} does not match the checksum GitHub lists for it.",
            &[("asset", asset)],
        ));
    }
    Ok(())
}

/// Downloads `release`'s file into a scratch folder, under its own name, at most `max` bytes, and
/// checks it with [`verify`]. Shared with `modkit`.
pub(crate) fn fetch(release: &Release, max: u64) -> Result<PathBuf, String> {
    let mut body = Vec::new();
    backend::get(&release.url)?
        .into_reader()
        .take(max + 1)
        .read_to_end(&mut body)
        .map_err(|e| {
            let error = e.to_string();
            trf(
                "Download interrupted: {error}",
                &[("error", error.as_str())],
            )
        })?;
    verify(release, &body, max)?;
    stage_in(&std::env::temp_dir(), &release.asset, &body)
}

/// Writes `body` as `asset` into a new private folder under `base`; the file keeps its name (the
/// import reads a .tar.gz or a .zip by it). A name that is not a plain file name is refused before
/// any folder is made. The folder is removed again when the write fails.
fn stage_in(base: &Path, asset: &str, body: &[u8]) -> Result<PathBuf, String> {
    if !plain_segment(asset) {
        return Err(trf(
            "{asset} is not a plain file name, so it is not saved.",
            &[("asset", asset)],
        ));
    }
    let dir = private_scratch_dir_in(base)?;
    // Removes the folder on every error below; disarmed once the file is in place.
    let mut scratch = RemoveOnDrop(Some(dir.clone()));
    let file = dir.join(asset);
    fs::write(&file, body).map_err(|e| e.to_string())?;
    scratch.0 = None;
    Ok(file)
}

/// A folder removed when this is dropped, unless it was set to `None` first.
struct RemoveOnDrop(Option<PathBuf>);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// A new, empty folder in `base` that only this user can open. Its name is random, so nobody can
/// guess it and make it first; `create_dir` fails rather than use a folder that is already there,
/// and on unix it is created 0700.
fn private_scratch_dir_in(base: &Path) -> Result<PathBuf, String> {
    use std::hash::{BuildHasher, Hasher};
    for _ in 0..64 {
        // A `RandomState` is seeded from the operating system's randomness on every call.
        let nonce = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let dir = base.join(format!("horizon-source-{nonce:016x}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let error = e.to_string();
                let shown = dir.display().to_string();
                return Err(trf(
                    "{dir}: {error}",
                    &[("dir", shown.as_str()), ("error", error.as_str())],
                ));
            }
        }
    }
    let shown = base.display().to_string();
    Err(trf(
        "no free folder under {dir}",
        &[("dir", shown.as_str())],
    ))
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
        .find(|s| same_repo(&s.repo, &release.repo))
        .and_then(|s| s.installed);
    let settings = backend::Settings::load();
    let select = select || selects(settings.selected.as_deref(), before.as_ref());
    let has_game_files = backend::installed()
        .iter()
        .any(|v| backend::is_game_tag(v) || crate::nightly::date_of(v).is_some());
    if !has_game_files {
        backend::install_tag("latest", true, |_| {})?;
    }
    let file = fetch(release, MAX_ARCHIVE)?;
    let made = build_import::import_as(&file, None, &release.label(), select);
    let _ = fs::remove_dir_all(file.parent().unwrap_or(&file));
    let version = made?;
    edit(|sources| {
        if let Some(source) = sources
            .iter_mut()
            .find(|s| same_repo(&s.repo, &release.repo))
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
                    meta.version = Some(version.clone());
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

/// Adds `repo` to `sources`, updating itself when `auto_update`: the pure part of [`add`]. Returns
/// whether it takes over Horizon's own game updates (a source that updates itself, other than the
/// default one, which changes nothing else).
fn add_to(sources: &mut Vec<Source>, repo: &str, auto_update: bool) -> Result<bool, String> {
    if sources.iter().any(|s| same_repo(&s.repo, repo)) {
        return Err(trf("{repo} is a source already.", &[("repo", repo)]));
    }
    sources.push(Source {
        repo: repo.to_owned(),
        auto_update,
        installed: None,
    });
    // The fork's own default source is not "Horizon's own": following it changes nothing.
    Ok(auto_update && !same_repo(repo, DEFAULT))
}

/// Adds the repository `text` names, updating itself when `auto_update`. A source that updates
/// itself takes over the updates: Horizon's own automatic game updates turn off
/// ([`step_aside_for_horizon_updates`]). Returns the repository, and whether it took over.
pub fn add(text: &str, auto_update: bool) -> Result<(String, bool), String> {
    let repo = repo_of(text)?;
    let mut sources = load();
    let took_over = add_to(&mut sources, &repo, auto_update)?;
    // The settings change is saved before the source: were the source saved first and this save
    // to fail, a retry would hit "is a source already" and never step aside.
    if took_over {
        let mut settings = backend::Settings::load();
        if step_aside_for_horizon_updates(&mut settings) {
            settings.save()?;
        }
    }
    save(&sources)?;
    Ok((repo, took_over))
}

/// Turns Horizon's own automatic game updates off in `settings` (`Settings::install_new_versions`:
/// new Horizon game versions no longer install themselves). The launcher updating itself is not
/// one of them: it comes from Event Horizon's own releases, so `auto_update_launcher` stays as it
/// is. Returns whether anything changed; the player can turn it back on by hand. Nothing happens
/// when it is already off, so a settings file that never said stays as it was.
pub fn step_aside_for_horizon_updates(settings: &mut backend::Settings) -> bool {
    if settings.auto_install() {
        settings.install_new_versions = Some(false);
        true
    } else {
        false
    }
}

/// Forgets `repo` in `sources`: the pure part of [`remove`].
fn remove_from(sources: &mut Vec<Source>, repo: &str) -> Result<(), String> {
    let before = sources.len();
    sources.retain(|s| !same_repo(&s.repo, repo));
    if sources.len() == before {
        return Err(trf("{repo} is not a source.", &[("repo", repo)]));
    }
    Ok(())
}

/// Forgets a source. The versions it installed stay: remove them on the Versions page.
pub fn remove(repo: &str) -> Result<(), String> {
    edit(|sources| remove_from(sources, repo))
}

/// Sets `repo`'s automatic updates in `sources`: the pure part of [`set_auto_update`].
fn set_auto_update_in(sources: &mut [Source], repo: &str, on: bool) -> Result<(), String> {
    let source = sources
        .iter_mut()
        .find(|s| same_repo(&s.repo, repo))
        .ok_or_else(|| trf("{repo} is not a source.", &[("repo", repo)]))?;
    source.auto_update = on;
    Ok(())
}

/// Turns a source's automatic updates on or off.
pub fn set_auto_update(repo: &str, on: bool) -> Result<(), String> {
    edit(|sources| set_auto_update_in(sources, repo, on))
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
/// That script is not in this tree; the function only reads the file it would write.
fn local_build(version: &str) -> Option<String> {
    let text = fs::read_to_string(backend::version_dir(version).join(".local_build")).ok()?;
    let build: LocalBuild = serde_json::from_str(&text).ok()?;
    Some(build.commit.chars().take(7).collect())
}

/// An Event Horizon release installed from its GitHub source: the release as a player reads it
/// (`0.1.0`, `Nightly 2026-11-05`).
fn release_build(version: &str) -> Option<String> {
    let about = build_import::about(version)?;
    Some(release_name(tag_of_label(&about.file)?))
}

/// The release tag in a source's label (`nihilitymeta/horizon eh-nightly-2026-11-05`), for Event
/// Horizon's own repositories, under their old names too: an install made before the move keeps its
/// label. Only the game's old names count: a moved repository that is not `DEFAULT` (the modkit)
/// is no Event Horizon release.
fn tag_of_label(file: &str) -> Option<&str> {
    std::iter::once(DEFAULT)
        .chain(
            RENAMED
                .iter()
                .filter(|(_, new)| new.eq_ignore_ascii_case(DEFAULT))
                .map(|(old, _)| *old),
        )
        .find_map(|repo| file.strip_prefix(repo)?.strip_prefix(' '))
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
        return Some(trf(
            "Event Horizon {release}",
            &[("release", release.as_str())],
        ));
    }
    if let Some(commit) = local_build(version) {
        return Some(match commit.is_empty() {
            true => tr("Event Horizon · local").to_owned(),
            false => trf(
                "Event Horizon · local {commit}",
                &[("commit", commit.as_str())],
            ),
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
    launcher_update_from(api, own_tag(), own_built())
}

fn launcher_update_from(api: Api, own: Option<&str>, built: &str) -> Option<backend::Release> {
    // A launcher built by hand has no release to compare with: it makes no request at all.
    let own = own?;
    let found = latest(DEFAULT, api).ok().flatten()?;
    newer(&found.tag, &found.published, Some(own), built).then(|| backend::Release {
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

/// What a launcher update's download is checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestCheck {
    /// The SHA-256 GitHub lists for the archive (Event Horizon's releases).
    GitHubList,
    /// Horizon's signed checksums.
    Signed,
}

/// Which check a launcher update from release `tag` is verified with.
pub fn digest_check(tag: &str) -> DigestCheck {
    if is_event_horizon_tag(tag) {
        DigestCheck::GitHubList
    } else {
        DigestCheck::Signed
    }
}

/// Whether a launcher update from release `tag` first checks the launcher already installed (a
/// newer one there means nothing to do). Only Horizon's tags do: an Event Horizon tag has no version
/// to compare, so its update always goes on to the download.
pub fn runs_installed_precheck(tag: &str) -> bool {
    digest_check(tag) == DigestCheck::Signed
}

/// The SHA-256 GitHub lists for Event Horizon release `tag`'s archive for this computer: what a
/// launcher update from it is checked against.
pub fn launcher_digest(tag: &str, api: Api) -> Result<String, String> {
    let platform = platform()
        .ok_or_else(|| tr("Event Horizon has no launcher for this computer.").to_owned())?;
    let release = api(&format!("repos/{DEFAULT}/releases/tags/{tag}"))?
        .ok_or_else(|| trf("Event Horizon has no release {tag}.", &[("tag", tag)]))?;
    pick(DEFAULT, &release, platform)?.sha256.ok_or_else(|| {
        trf(
            "GitHub lists no checksum for Event Horizon {tag}: not installed.",
            &[("tag", tag)],
        )
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_default_source_is_the_moved_repository_and_old_names_are_renamed() {
        assert_eq!(DEFAULT, "nihilitymeta/horizon");
        let saved = r#"[{"repo":"WHOS-CARMEN/Horizon","auto_update":true,"installed":{"tag":"eh-nightly-2026-11-08-2","version":"imported-2026-11-07-a1fbfaa"}}]"#;
        let list = parse(Some(saved));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].repo, "nihilitymeta/horizon");
        assert!(list[0].auto_update);
        assert_eq!(
            list[0].installed,
            Some(Installed {
                tag: "eh-nightly-2026-11-08-2".into(),
                version: "imported-2026-11-07-a1fbfaa".into(),
            })
        );
        // A repository that did not move is left alone.
        assert_eq!(parse(Some(r#"[{"repo":"a/b"}]"#))[0].repo, "a/b");
    }

    #[test]
    fn no_saved_list_means_the_default_source_updating_itself() {
        let list = parse(None);
        assert_eq!(list, default_list());
        assert_eq!(list[0].repo, DEFAULT);
        assert!(list[0].auto_update);
        assert!(list[0].installed.is_none());
    }

    #[test]
    fn an_old_and_a_new_entry_for_one_repository_collapse_to_the_installed_one() {
        let installed =
            r#""installed":{"tag":"eh-v0.1.0","version":"imported-2026-11-04-abcdef1"}"#;
        let old_first = format!(
            r#"[{{"repo":"whos-carmen/horizon","auto_update":true}},{{"repo":"nihilitymeta/horizon","auto_update":false,{installed}}}]"#
        );
        let new_first = format!(
            r#"[{{"repo":"nihilitymeta/horizon","auto_update":false,{installed}}},{{"repo":"whos-carmen/horizon","auto_update":true}}]"#
        );
        for saved in [old_first, new_first] {
            let list = parse(Some(saved.as_str()));
            assert_eq!(list.len(), 1, "{saved}");
            assert_eq!(list[0].repo, "nihilitymeta/horizon");
            assert_eq!(list[0].installed.as_ref().unwrap().tag, "eh-v0.1.0");
            assert_eq!(
                list[0].installed.as_ref().unwrap().version,
                "imported-2026-11-04-abcdef1"
            );
        }
    }

    #[test]
    fn a_saved_list_that_does_not_parse_falls_back_to_the_default_source() {
        assert_eq!(parse(Some("not json")), default_list());
    }

    fn release(assets: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "tag_name": "eh-v0.1.0", "assets": assets })
    }

    /// A release as GitHub reports it through the moved repository: its files and its page name
    /// `nihilitymeta/horizon`, whatever repository was asked for.
    fn moved_release(platform: &str) -> serde_json::Value {
        serde_json::json!({
            "tag_name": "eh-nightly-2026-11-13-2",
            "html_url": "https://github.com/nihilitymeta/horizon/releases/tag/eh-nightly-2026-11-13-2",
            "assets": [
                { "name": format!("event-horizon-{platform}.tar.gz"),
                  "browser_download_url": format!("https://github.com/nihilitymeta/horizon/releases/download/eh-nightly-2026-11-13-2/event-horizon-{platform}.tar.gz") },
            ],
        })
    }

    #[test]
    fn a_release_of_a_moved_repository_is_accepted_under_its_old_name() {
        let ends = [".tar.gz".to_owned()];
        let json = moved_release("linux-x86_64");
        let found = asset("whos-carmen/horizon", &json, &ends).unwrap();
        assert_eq!(found.repo, "whos-carmen/horizon");
        assert_eq!(found.tag, "eh-nightly-2026-11-13-2");
        assert_eq!(
            found.url,
            "https://github.com/nihilitymeta/horizon/releases/download/eh-nightly-2026-11-13-2/event-horizon-linux-x86_64.tar.gz"
        );
        // Under the new name too, and with the owner and name in another case.
        assert!(asset("nihilitymeta/horizon", &json, &ends).is_some());
        assert!(asset("NihilityMeta/Horizon", &json, &ends).is_some());
        let shouting = serde_json::json!({
            "tag_name": "eh-v0.1.0",
            "html_url": "https://github.com/NIHILITYMETA/HORIZON/releases/tag/eh-v0.1.0",
            "assets": [
                { "name": "event-horizon-linux-x86_64.tar.gz",
                  "browser_download_url": "https://github.com/nihilitymeta/horizon/releases/download/eh-v0.1.0/event-horizon-linux-x86_64.tar.gz" },
            ],
        });
        assert!(asset("whos-carmen/horizon", &shouting, &ends).is_some());
    }

    #[test]
    fn a_file_of_a_third_repository_is_refused_whatever_the_release_says() {
        let ends = [".tar.gz".to_owned()];
        let elsewhere = serde_json::json!({
            "tag_name": "eh-v0.1.0",
            "html_url": "https://github.com/nihilitymeta/horizon/releases/tag/eh-v0.1.0",
            "assets": [
                { "name": "event-horizon-linux-x86_64.tar.gz",
                  "browser_download_url": "https://github.com/someone/else/releases/download/x/event-horizon-linux-x86_64.tar.gz" },
            ],
        });
        assert!(asset("whos-carmen/horizon", &elsewhere, &ends).is_none());
        assert!(asset("nihilitymeta/horizon", &elsewhere, &ends).is_none());
    }

    #[test]
    fn a_page_that_is_not_a_release_of_a_plain_repository_adds_no_accepted_repository() {
        let ends = [".tar.gz".to_owned()];
        let file = "https://github.com/x/y/releases/download/t/event-horizon-linux-x86_64.tar.gz";
        // Not a github.com release page at all.
        let not_github = serde_json::json!({
            "tag_name": "t",
            "html_url": "https://example.com/x/y/releases/tag/t",
            "assets": [{ "name": "event-horizon-linux-x86_64.tar.gz", "browser_download_url": file }],
        });
        assert!(asset("a/b", &not_github, &ends).is_none());
        // A name with more than one slash in it.
        let three_parts = serde_json::json!({
            "tag_name": "t",
            "html_url": "https://github.com/x/y/z/releases/tag/t",
            "assets": [{ "name": "event-horizon-linux-x86_64.tar.gz", "browser_download_url": file }],
        });
        assert!(asset("a/b", &three_parts, &ends).is_none());
        // A name with characters GitHub's names do not use.
        let odd_name = serde_json::json!({
            "tag_name": "t",
            "html_url": "https://github.com/x/y%20z/releases/tag/t",
            "assets": [{ "name": "event-horizon-linux-x86_64.tar.gz",
                         "browser_download_url": "https://github.com/x/y%20z/releases/download/t/event-horizon-linux-x86_64.tar.gz" }],
        });
        assert!(asset("a/b", &odd_name, &ends).is_none());
    }

    #[test]
    fn a_url_that_only_starts_like_a_release_of_its_repository_is_refused() {
        let ends = [".tar.gz".to_owned()];
        let file = "event-horizon-linux-x86_64.tar.gz";
        // Each starts with the repository's release prefix, then leaves it: `..` segments, percent
        // escapes of dots, backslashes. The URL parser sends each one to another repository.
        let crafted = [
            format!("https://github.com/nihilitymeta/horizon/releases/download/../../../../evil/malware/releases/download/t/{file}"),
            format!("https://github.com/nihilitymeta/horizon/releases/download/%2e%2e/%2e%2e/%2e%2e/%2e%2e/evil/malware/releases/download/t/{file}"),
            format!("https://github.com/nihilitymeta/horizon/releases/download/..\\..\\..\\..\\evil\\malware\\releases\\download\\t\\{file}"),
        ];
        let release = |url: &str, page: &str| {
            serde_json::json!({
                "tag_name": "t",
                "html_url": format!("https://github.com/{page}/releases/tag/t"),
                "assets": [{ "name": file, "browser_download_url": url }],
            })
        };
        for url in &crafted {
            // Through the configured repository's own name...
            let by_config = release(url, "nihilitymeta/horizon");
            assert!(
                asset("nihilitymeta/horizon", &by_config, &ends).is_none(),
                "{url}"
            );
            // ...and through the name the release's page gives (the repository moved).
            let by_page = release(url, "nihilitymeta/horizon");
            assert!(
                asset("whos-carmen/horizon", &by_page, &ends).is_none(),
                "{url}"
            );
        }
    }

    #[test]
    fn a_plain_release_url_is_accepted_under_the_configured_and_the_page_repository() {
        let ends = [".tar.gz".to_owned()];
        let file = "event-horizon-linux-x86_64.tar.gz";
        for tag in ["eh-v0.1.0", "eh-nightly-2026-11-13-2", "eh-v0.1.0+1"] {
            let url =
                format!("https://github.com/nihilitymeta/horizon/releases/download/{tag}/{file}");
            let release = serde_json::json!({
                "tag_name": tag,
                "html_url": format!("https://github.com/nihilitymeta/horizon/releases/tag/{tag}"),
                "assets": [{ "name": file, "browser_download_url": url }],
            });
            let found = asset("nihilitymeta/horizon", &release, &ends).unwrap();
            assert_eq!(found.url, url);
            assert!(
                asset("whos-carmen/horizon", &release, &ends).is_some(),
                "{tag}"
            );
        }
    }

    #[test]
    fn the_game_source_takes_a_moved_release_through_the_api() {
        let Some(platform) = platform() else {
            return;
        };
        let json = moved_release(platform);
        let api = |_: &str| -> Result<Option<serde_json::Value>, String> { Ok(Some(json.clone())) };
        let found = latest("whos-carmen/horizon", &api).unwrap().unwrap();
        assert_eq!(found.repo, "whos-carmen/horizon");
        assert_eq!(found.tag, "eh-nightly-2026-11-13-2");
    }

    #[test]
    fn a_source_with_no_release_is_an_error_naming_it() {
        let Some(_) = platform() else {
            return;
        };
        let api = |_: &str| -> Result<Option<serde_json::Value>, String> { Ok(None) };
        let error = latest("a/b", &api).unwrap_err();
        assert!(error.contains("a/b"), "{error}");
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
        // The platform name is in the message; the words around it may be translated.
        let missing = pick("a/b", &json, "macos-arm64").unwrap_err();
        assert!(missing.contains("macos-arm64"), "{missing}");
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
    fn repository_names_compare_without_case() {
        assert!(same_repo("Owner/Repo", "owner/repo"));
        assert!(same_repo("whos-carmen/horizon", "WHOS-CARMEN/HORIZON"));
        assert!(!same_repo("owner/repo", "owner/repo2"));
        assert!(!same_repo("owner/repo", "other/repo"));
    }

    #[test]
    fn the_same_repository_in_two_cases_is_one_entry_when_the_list_is_read() {
        let saved = r#"[{"repo":"Owner/Repo"},{"repo":"owner/repo","auto_update":true}]"#;
        let list = parse(Some(saved));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].repo, "Owner/Repo");
    }

    /// Scratch folders for these tests: under the cargo target dir, never the system temp folder
    /// (a small tmpfs on some machines), and removed with the `TempDir`.
    fn scratch_root() -> PathBuf {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"));
        let root = target.join("launcher-test-scratch");
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn release_with(sha256: Option<String>) -> Release {
        Release {
            repo: "a/b".into(),
            tag: "eh-v0.1.0".into(),
            asset: "event-horizon-linux-x86_64.tar.gz".into(),
            url: "https://github.com/a/b/releases/download/eh-v0.1.0/event-horizon-linux-x86_64.tar.gz".into(),
            size: 3,
            published: String::new(),
            sha256,
        }
    }

    /// The FIPS 180-2 test vector for "abc": the expected value is written out, not computed.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_is_the_standard_digest() {
        assert_eq!(sha256(b"abc"), ABC);
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn a_release_that_lists_no_digest_is_refused() {
        let error = verify(&release_with(None), b"abc", MAX_ARCHIVE).unwrap_err();
        assert!(
            error.contains("event-horizon-linux-x86_64.tar.gz"),
            "{error}"
        );
    }

    #[test]
    fn a_download_with_the_wrong_digest_is_refused() {
        let wrong = "0".repeat(64);
        assert!(verify(&release_with(Some(wrong)), b"abc", MAX_ARCHIVE).is_err());
    }

    #[test]
    fn a_download_with_the_listed_digest_is_accepted() {
        assert_eq!(
            verify(&release_with(Some(ABC.into())), b"abc", MAX_ARCHIVE),
            Ok(())
        );
    }

    #[test]
    fn a_download_bigger_than_the_cap_is_refused_even_with_the_right_digest() {
        let body = b"abcd";
        let release = release_with(Some(sha256(body)));
        assert!(verify(&release, body, 3).is_err());
        assert_eq!(verify(&release, body, 4), Ok(()));
    }

    #[test]
    fn a_download_keeps_its_name_in_a_new_folder_only_its_owner_can_open() {
        let root = tempfile::tempdir_in(scratch_root()).unwrap();
        let file = stage_in(root.path(), "event-horizon-linux-x86_64.tar.gz", b"bytes").unwrap();
        assert_eq!(
            file.file_name().unwrap(),
            "event-horizon-linux-x86_64.tar.gz"
        );
        assert_eq!(fs::read(&file).unwrap(), b"bytes");
        let dir = file.parent().unwrap();
        assert_eq!(dir.parent(), Some(root.path()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn a_download_that_cannot_be_written_leaves_no_folder_behind() {
        let root = tempfile::tempdir_in(scratch_root()).unwrap();
        // A name longer than a file system allows: the folder is made, then the write fails.
        let long = "a".repeat(300);
        assert!(stage_in(root.path(), &long, b"bytes").is_err());
        let left: Vec<_> = fs::read_dir(root.path()).unwrap().flatten().collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn a_name_that_is_not_a_plain_file_name_is_refused_before_a_folder_is_made() {
        let root = tempfile::tempdir_in(scratch_root()).unwrap();
        for name in [
            "",
            "../escape.tar.gz",
            "sub/archive.tar.gz",
            "..\\archive.zip",
        ] {
            assert!(stage_in(root.path(), name, b"bytes").is_err(), "{name}");
        }
        let left: Vec<_> = fs::read_dir(root.path()).unwrap().flatten().collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    #[test]
    fn each_download_gets_its_own_new_folder() {
        let root = tempfile::tempdir_in(scratch_root()).unwrap();
        let a = private_scratch_dir_in(root.path()).unwrap();
        let b = private_scratch_dir_in(root.path()).unwrap();
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        assert_eq!(a.parent(), Some(root.path()));
    }

    #[test]
    fn a_new_build_is_selected_unless_the_player_picked_another_version_by_hand() {
        let before = Installed {
            tag: "eh-v0.1.0".into(),
            version: "imported-2026-11-04-abcdef1".into(),
        };
        assert!(selects(None, None));
        assert!(selects(Some("v2026.10.13"), Some(&before)));
        assert!(selects(Some("nightly-2026-11-08-0c02067"), None));
        assert!(selects(Some("imported-2026-11-04-abcdef1"), Some(&before)));
        assert!(!selects(Some("imported-2026-10-01-0a0a0a0"), Some(&before)));
        assert!(!selects(Some("imported-2026-10-01-0a0a0a0"), None));
    }

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

    /// Event Horizon's release `tag` with one archive for `platform`, listing `digest` if given.
    fn eh_release(tag: &str, platform: &str, digest: Option<&str>) -> serde_json::Value {
        let mut archive = serde_json::json!({
            "name": format!("event-horizon-{platform}.tar.gz"),
            "size": 1234,
            "browser_download_url": format!("https://github.com/nihilitymeta/horizon/releases/download/{tag}/event-horizon-{platform}.tar.gz"),
        });
        if let Some(digest) = digest {
            archive["digest"] = serde_json::json!(format!("sha256:{digest}"));
        }
        serde_json::json!({
            "tag_name": tag,
            "published_at": "2026-11-20T09:00:00Z",
            "html_url": format!("https://github.com/nihilitymeta/horizon/releases/tag/{tag}"),
            "assets": [archive],
        })
    }

    #[test]
    fn the_launcher_update_is_the_newer_event_horizon_archive_and_no_request_for_a_hand_build() {
        let Some(platform) = platform() else {
            return;
        };
        let json = eh_release("eh-v0.2.0", platform, None);
        let calls = std::cell::Cell::new(0);
        let api = |_: &str| -> Result<Option<serde_json::Value>, String> {
            calls.set(calls.get() + 1);
            Ok(Some(json.clone()))
        };
        let built = "2026-11-05T09:03:00Z";
        let update = launcher_update_from(&api, Some("eh-v0.1.0"), built).unwrap();
        assert_eq!(update.tag, "eh-v0.2.0");
        assert_eq!(update.name, "Event Horizon 0.2.0");
        assert_eq!(update.asset.as_ref().map(|a| a.1), Some(1234));
        assert!(update
            .asset
            .unwrap()
            .0
            .starts_with("https://github.com/nihilitymeta/horizon/"));
        // Same version, or older: no update.
        assert!(launcher_update_from(&api, Some("eh-v0.2.0"), built).is_none());
        assert!(launcher_update_from(&api, Some("eh-v0.3.0"), built).is_none());
        // A launcher built by hand never updates, and asks GitHub nothing.
        let before = calls.get();
        assert!(launcher_update_from(&api, None, built).is_none());
        assert_eq!(calls.get(), before);
        // A newer release with no archive for this computer is no update.
        let other = if platform == "linux-x86_64" {
            "windows-x64"
        } else {
            "linux-x86_64"
        };
        let elsewhere = eh_release("eh-v0.9.0", other, None);
        let api =
            |_: &str| -> Result<Option<serde_json::Value>, String> { Ok(Some(elsewhere.clone())) };
        assert!(launcher_update_from(&api, Some("eh-v0.1.0"), built).is_none());
    }

    #[test]
    fn the_launcher_digest_is_the_listed_sha256_and_a_release_without_one_is_refused() {
        let Some(platform) = platform() else {
            return;
        };
        let digest = "b".repeat(64);
        let listed = eh_release("eh-v0.2.0", platform, Some(&digest));
        let api = |path: &str| -> Result<Option<serde_json::Value>, String> {
            assert_eq!(path, "repos/nihilitymeta/horizon/releases/tags/eh-v0.2.0");
            Ok(Some(listed.clone()))
        };
        assert_eq!(launcher_digest("eh-v0.2.0", &api).unwrap(), digest);

        let unlisted = eh_release("eh-v0.2.0", platform, None);
        let api =
            |_: &str| -> Result<Option<serde_json::Value>, String> { Ok(Some(unlisted.clone())) };
        let error = launcher_digest("eh-v0.2.0", &api).unwrap_err();
        assert!(error.contains("eh-v0.2.0"), "{error}");

        let other = if platform == "linux-x86_64" {
            "windows-x64"
        } else {
            "linux-x86_64"
        };
        let elsewhere = eh_release("eh-v0.2.0", other, Some(&digest));
        let api =
            |_: &str| -> Result<Option<serde_json::Value>, String> { Ok(Some(elsewhere.clone())) };
        let error = launcher_digest("eh-v0.2.0", &api).unwrap_err();
        assert!(error.contains(platform), "{error}");

        let api = |_: &str| -> Result<Option<serde_json::Value>, String> { Ok(None) };
        let error = launcher_digest("eh-v0.2.0", &api).unwrap_err();
        assert!(error.contains("eh-v0.2.0"), "{error}");
    }

    #[test]
    fn an_event_horizon_tag_is_checked_against_the_github_lists_and_any_other_against_signatures() {
        assert_eq!(digest_check("eh-v0.2.0"), DigestCheck::GitHubList);
        assert_eq!(
            digest_check("eh-nightly-2026-11-06"),
            DigestCheck::GitHubList
        );
        // Horizon's own tags keep their signed checksums.
        assert_eq!(digest_check("launcher@0.2.40"), DigestCheck::Signed);
        assert_eq!(digest_check("launcher-v0.2.40"), DigestCheck::Signed);
    }

    #[test]
    fn only_a_horizon_tag_runs_the_installed_launcher_precheck() {
        assert!(runs_installed_precheck("launcher@0.2.40"));
        assert!(!runs_installed_precheck("eh-v0.2.0"));
    }

    #[test]
    fn a_label_from_before_the_move_still_names_its_release() {
        assert_eq!(
            tag_of_label("whos-carmen/horizon eh-nightly-2026-11-08-2"),
            Some("eh-nightly-2026-11-08-2")
        );
        assert_eq!(
            tag_of_label("nihilitymeta/horizon eh-v0.1.0"),
            Some("eh-v0.1.0")
        );
        assert_eq!(tag_of_label("someone/else eh-v0.1.0"), None);
    }

    #[test]
    fn a_modkit_or_other_label_is_not_an_event_horizon_release() {
        assert_eq!(tag_of_label("whos-carmen/horizon-modkit v0.7.1"), None);
        assert_eq!(tag_of_label("nihilitymeta/horizon-modkit v0.7.1"), None);
        assert_eq!(tag_of_label("whos-carmen/horizon-evil eh-v0.1.0"), None);
    }

    #[test]
    fn a_source_that_updates_itself_takes_over_from_horizons_own_updates() {
        // Horizon's own automatic game updates on, as a saved settings file has them.
        let mut settings = backend::Settings {
            auto_update_launcher: Some(true),
            install_new_versions: Some(true),
            ..backend::Settings::default()
        };
        let mut sources = Vec::new();
        assert_eq!(add_to(&mut sources, "a/b", true), Ok(true));
        assert!(step_aside_for_horizon_updates(&mut settings));
        // The source itself updates; Horizon's own game updates are off, and the launcher
        // updating itself (Event Horizon's releases, not Horizon's) stays on.
        assert!(sources.iter().any(|s| s.repo == "a/b" && s.auto_update));
        assert_eq!(settings.auto_update_launcher, Some(true));
        assert_eq!(settings.install_new_versions, Some(false));
        // Nothing changes when Horizon's own updates are already off.
        assert!(!step_aside_for_horizon_updates(&mut settings));
        // A source added without its own updates takes nothing over.
        let mut settings = backend::Settings {
            install_new_versions: Some(true),
            ..backend::Settings::default()
        };
        assert_eq!(add_to(&mut sources, "c/d", false), Ok(false));
        assert_eq!(settings.install_new_versions, Some(true));
        assert!(step_aside_for_horizon_updates(&mut settings));
    }

    #[test]
    fn adding_the_default_source_changes_nothing_else() {
        let mut sources = Vec::new();
        assert_eq!(add_to(&mut sources, DEFAULT, true), Ok(false));
        assert!(sources.iter().any(|s| s.repo == DEFAULT && s.auto_update));
    }

    #[test]
    fn the_same_repository_in_two_cases_is_one_source() {
        let mut sources = Vec::new();
        assert_eq!(add_to(&mut sources, "Owner/Repo", false), Ok(false));
        assert!(add_to(&mut sources, "owner/repo", false).is_err());
        assert_eq!(sources.len(), 1);
        set_auto_update_in(&mut sources, "OWNER/REPO", true).unwrap();
        assert!(sources[0].auto_update);
        remove_from(&mut sources, "owner/REPO").unwrap();
        assert!(sources.is_empty());
        assert!(remove_from(&mut sources, "owner/repo").is_err());
        assert!(set_auto_update_in(&mut sources, "owner/repo", true).is_err());
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
