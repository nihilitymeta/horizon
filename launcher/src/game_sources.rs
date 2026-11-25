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
use crate::github_mods::{self, Target};
use crate::i18n::{tr, trf};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
}
