//! Tests of the scan: folders and archives, the ignored and refused entries, duplicate ids, the dev
//! paths, the enabled file's effect, dependencies, the cycle and size limits, the order, and the
//! translation keys. Packages are written in temp dirs: folders as they are, archives with the zip
//! writer.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::builtin::env_dev_paths;
use crate::discover::scan as run_scan;
use crate::discover::*;
use crate::hlua::{NameFault, Refusal};

/// The manifest text of a mod: id, version, both sides, and the dependency lines given.
fn manifest(id: &str, version: &str, deps: &str) -> String {
    format!("id = \"{id}\"\nversion = \"{version}\"\nside = \"both\"\n{deps}")
}

/// The files of a valid mod with `id` and `deps`.
fn mod_files(id: &str, deps: &str) -> Vec<(String, String)> {
    vec![
        (MANIFEST.to_string(), manifest(id, "1.0.0", deps)),
        (ENTRY.to_string(), "return {}\n".to_string()),
    ]
}

/// Writes the files as a folder at `dir`.
fn write_folder(dir: &Path, files: &[(String, String)]) -> PathBuf {
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir.to_path_buf()
}

/// `<root>/lua-mods/<name>/` as a folder mod with `id` and `deps`.
fn folder(root: &Path, name: &str, id: &str, deps: &str) -> PathBuf {
    write_folder(&root.join(DIR).join(name), &mod_files(id, deps))
}

/// `<root>/lua-mods/<name>.hlua` as a zip archive mod with `id` and `deps`.
fn archive(root: &Path, name: &str, id: &str, deps: &str) -> PathBuf {
    let path = root.join(DIR).join(format!("{name}.hlua"));
    write_archive(&path, &mod_files(id, deps));
    path
}

/// Writes `files` as a zip archive at `path`.
fn write_archive(path: &Path, files: &[(String, String)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for (name, text) in files {
        writer
            .start_file(name.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}

/// Writes `root/lua-mods/enabled.toml` with `text`.
fn enabled_file(root: &Path, text: &str) {
    std::fs::create_dir_all(root.join(DIR)).unwrap();
    std::fs::write(root.join(DIR).join("enabled.toml"), text).unwrap();
}

/// The ids of the loaded mods, in load order.
fn loaded_ids(scan: &Scan) -> Vec<String> {
    scan.loaded.iter().map(|m| m.found.id.clone()).collect()
}

/// Each refused path's file name and its translation key, in scan order.
fn refused_keys(scan: &Scan) -> Vec<(String, &'static str)> {
    scan.refused
        .iter()
        .map(|r| {
            let name = r.path.file_name().unwrap().to_string_lossy().into_owned();
            (name, r.fault.key())
        })
        .collect()
}

/// The refusal of the path named `name`, if any.
fn fault_of<'a>(scan: &'a Scan, name: &str) -> Option<&'a Fault> {
    scan.refused
        .iter()
        .find(|r| r.path.file_name().is_some_and(|n| n == name))
        .map(|r| &r.fault)
}

#[test]
fn folders_and_archives_load_together_in_load_order() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path(), "a_folder", "zeta", "");
    archive(root.path(), "b_archive", "alpha", "");
    let scan = run_scan(root.path(), &[]);
    assert!(scan.refused.is_empty(), "{:?}", refused_keys(&scan));
    // No dependencies: ties by id, so alpha (an archive) before zeta (a folder).
    assert_eq!(loaded_ids(&scan), ["alpha", "zeta"]);
    assert_eq!(scan.loaded[0].index, 0);
    assert_eq!(scan.loaded[1].index, 1);
    assert!(!scan.loaded[0].dev);
    assert_eq!(scan.loaded[0].found.version, "1.0.0");
}

#[test]
fn a_missing_lua_mods_folder_is_empty() {
    let root = tempfile::tempdir().unwrap();
    let scan = run_scan(root.path(), &[]);
    assert!(scan.loaded.is_empty() && scan.refused.is_empty() && scan.disabled.is_empty());
    assert!(scan.notices.is_empty());
}

#[test]
fn other_entries_are_ignored_without_an_error() {
    let root = tempfile::tempdir().unwrap();
    let lua_mods = root.path().join(DIR);
    std::fs::create_dir_all(&lua_mods).unwrap();
    std::fs::write(lua_mods.join("notes.txt"), "hello").unwrap();
    std::fs::write(lua_mods.join("old.hlu"), "not a package").unwrap();
    std::fs::write(lua_mods.join("UPPER.HLUA"), "not a package").unwrap();
    std::fs::write(lua_mods.join("README"), "hi").unwrap();
    folder(root.path(), "only", "kept_mod", "");
    let scan = run_scan(root.path(), &[]);
    assert!(scan.refused.is_empty(), "{:?}", refused_keys(&scan));
    assert_eq!(loaded_ids(&scan), ["kept_mod"]);
}

#[test]
fn a_folder_without_a_manifest_is_refused_with_its_key() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(DIR).join("bare");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ENTRY), "return {}\n").unwrap();
    let scan = run_scan(root.path(), &[]);
    assert!(scan.loaded.is_empty());
    assert_eq!(
        refused_keys(&scan),
        [("bare".to_string(), "lua_mod.refused.manifest_missing")]
    );
}

#[test]
fn a_bad_archive_is_refused_with_the_reader_key_and_the_others_load() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(DIR).join("broken.hlua");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"this is not a zip archive at all").unwrap();
    folder(root.path(), "good", "good_mod", "");
    let scan = run_scan(root.path(), &[]);
    assert_eq!(loaded_ids(&scan), ["good_mod"]);
    assert_eq!(
        refused_keys(&scan),
        [("broken.hlua".to_string(), "lua_mod.refused.not_zip")]
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_folder_or_an_archive_is_refused_and_a_symlink_to_text_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    let lua_mods = root.path().join(DIR);
    std::fs::create_dir_all(&lua_mods).unwrap();
    let outside = write_folder(&root.path().join("outside"), &mod_files("outside_mod", ""));
    std::os::unix::fs::symlink(&outside, lua_mods.join("link_dir")).unwrap();
    let target = root.path().join("moved.hlua");
    write_archive(&target, &mod_files("pkg_mod", ""));
    std::os::unix::fs::symlink(&target, lua_mods.join("link.hlua")).unwrap();
    let note = root.path().join("note.txt");
    std::fs::write(&note, "x").unwrap();
    std::os::unix::fs::symlink(&note, lua_mods.join("note_link")).unwrap();
    let scan = run_scan(root.path(), &[]);
    assert!(scan.loaded.is_empty(), "{:?}", loaded_ids(&scan));
    assert_eq!(
        refused_keys(&scan),
        [
            ("link.hlua".to_string(), "lua_mod.refused.symlink"),
            ("link_dir".to_string(), "lua_mod.refused.symlink"),
        ]
    );
    assert_eq!(fault_of(&scan, "note_link"), None);
}

#[test]
fn a_duplicate_id_keeps_the_first_by_file_name() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path(), "a", "same", "");
    archive(root.path(), "b", "same", "");
    let scan = run_scan(root.path(), &[]);
    assert_eq!(loaded_ids(&scan), ["same"]);
    assert_eq!(scan.loaded[0].found.dir, root.path().join(DIR).join("a"));
    assert_eq!(
        fault_of(&scan, "b.hlua"),
        Some(&Fault::DuplicateId {
            id: "same".into(),
            first: "a".into(),
        })
    );
    assert_eq!(
        refused_keys(&scan),
        [("b.hlua".to_string(), "lua_mod.refused.duplicate_id")]
    );
}

#[test]
fn the_order_of_a_scan_is_the_same_every_time() {
    let root = tempfile::tempdir().unwrap();
    for (name, id, deps) in [
        ("d", "delta", "[dependencies]\nbeta = \"1\"\n"),
        ("c", "charlie", ""),
        ("b", "beta", "[dependencies]\ncharlie = \"1\"\n"),
        ("a", "alpha", ""),
    ] {
        folder(root.path(), name, id, deps);
    }
    let first = run_scan(root.path(), &[]);
    let order = loaded_ids(&first);
    assert_eq!(order, ["alpha", "charlie", "beta", "delta"]);
    for _ in 0..5 {
        assert_eq!(loaded_ids(&scan(root.path(), &[])), order);
    }
}

#[test]
fn a_dev_mod_replaces_the_installed_mod_with_its_id() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path(), "installed", "same", "");
    let dev = write_folder(
        &root.path().join("dev").join("same_dev"),
        &mod_files("same", ""),
    );
    let scan = run_scan(root.path(), std::slice::from_ref(&dev));
    assert_eq!(loaded_ids(&scan), ["same"]);
    assert!(scan.loaded[0].dev);
    assert_eq!(
        scan.loaded[0].found.dir,
        std::fs::canonicalize(&dev).unwrap()
    );
    assert_eq!(
        refused_keys(&scan),
        [("installed".to_string(), "lua_mod.refused.replaced_by_dev")]
    );
}

#[test]
fn a_dev_mod_bypasses_the_enabled_file_and_is_listed_first_among_equals() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path(), "off", "off_mod", "");
    folder(root.path(), "on", "on_mod", "");
    enabled_file(root.path(), "[mods]\noff_mod = false\n");
    let dev = write_folder(
        &root.path().join("dev").join("zz"),
        &mod_files("off_mod", ""),
    );
    let scan = run_scan(root.path(), std::slice::from_ref(&dev));
    // The dev mod is the one with the id: it loads, the disabled installed one is replaced.
    assert_eq!(loaded_ids(&scan), ["off_mod", "on_mod"]);
    assert!(scan.loaded[0].dev);
    assert!(scan.disabled.is_empty());

    // A dev mod with another id is first among equals with an installed one of the same id order.
    let root = tempfile::tempdir().unwrap();
    folder(root.path(), "aaa", "aaa_mod", "");
    let dev = write_folder(
        &root.path().join("dev").join("zzz"),
        &mod_files("zzz_mod", ""),
    );
    let scan = run_scan(root.path(), std::slice::from_ref(&dev));
    assert_eq!(loaded_ids(&scan), ["zzz_mod", "aaa_mod"]);
    assert!(scan.loaded[0].dev);
}

#[test]
fn a_dev_archive_loads_and_a_missing_or_non_archive_dev_path_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let dev_archive = root.path().join("dev").join("pkg.hlua");
    write_archive(&dev_archive, &mod_files("dev_pkg", ""));
    let missing = root.path().join("nowhere");
    let text = root.path().join("dev").join("notes.txt");
    std::fs::write(&text, "x").unwrap();
    let scan = run_scan(
        root.path(),
        &[dev_archive.clone(), missing.clone(), text.clone()],
    );
    assert_eq!(loaded_ids(&scan), ["dev_pkg"]);
    assert!(scan.loaded[0].dev);
    assert_eq!(
        refused_keys(&scan),
        [
            ("nowhere".to_string(), "lua_mod.refused.dev_path_missing"),
            ("notes.txt".to_string(), "lua_mod.refused.dev_path_not_hlua"),
        ]
    );
}

#[test]
fn more_than_eight_dev_paths_are_refused_past_the_eighth() {
    let root = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = (0..9)
        .map(|i| {
            write_folder(
                &root.path().join("dev").join(format!("d{i}")),
                &mod_files(&format!("dev_{i}"), ""),
            )
        })
        .collect();
    let scan = run_scan(root.path(), &paths);
    assert_eq!(scan.loaded.len(), 8);
    assert!(scan.loaded.iter().all(|m| m.dev));
    assert_eq!(
        refused_keys(&scan),
        [("d8".to_string(), "lua_mod.refused.dev_paths_too_many")]
    );
    assert_eq!(scan.refused[0].fault.args(), [("max", "8".to_string())]);
}

#[test]
fn a_disabled_installed_mod_is_listed_and_not_loaded() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path(), "a", "alpha", "");
    folder(root.path(), "b", "beta", "");
    folder(root.path(), "c", "gamma", "");
    folder(root.path(), "d", "delta", "");
    enabled_file(root.path(), "[mods]\nbeta = false\n");
    let scan = run_scan(root.path(), &[]);
    assert_eq!(loaded_ids(&scan), ["alpha", "delta", "gamma"]);
    let disabled: Vec<&str> = scan.disabled.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(disabled, ["beta"]);
    assert!(scan.notices.is_empty());
}

#[test]
fn a_dependency_chain_loads_in_dependency_order() {
    let root = tempfile::tempdir().unwrap();
    folder(
        root.path(),
        "top",
        "top_mod",
        "[dependencies]\nmid_mod = \"^1\"\n",
    );
    folder(
        root.path(),
        "mid",
        "mid_mod",
        "[dependencies]\nbase_mod = \"1.0\"\n",
    );
    folder(root.path(), "base", "base_mod", "");
    let scan = run_scan(root.path(), &[]);
    assert!(scan.refused.is_empty(), "{:?}", refused_keys(&scan));
    assert_eq!(loaded_ids(&scan), ["base_mod", "mid_mod", "top_mod"]);
    assert_eq!(scan.loaded[2].index, 2);
}

#[test]
fn missing_wrong_version_disabled_and_transitive_dependencies_refuse_with_keys() {
    let root = tempfile::tempdir().unwrap();
    folder(root.path(), "base", "base_mod", "");
    folder(
        root.path(),
        "gone",
        "gone_user",
        "[dependencies]\nnobody = \">=2, <3\"\n",
    );
    folder(
        root.path(),
        "vers",
        "vers_user",
        "[dependencies]\nbase_mod = \"^2\"\n",
    );
    folder(
        root.path(),
        "off",
        "off_user",
        "[dependencies]\noff_mod = \"1\"\n",
    );
    folder(
        root.path(),
        "chain",
        "chain_user",
        "[dependencies]\ngone_user = \"1\"\n",
    );
    folder(root.path(), "off_dep", "off_mod", "");
    enabled_file(root.path(), "[mods]\noff_mod = false\n");
    let scan = run_scan(root.path(), &[]);
    assert_eq!(loaded_ids(&scan), ["base_mod"]);
    assert_eq!(
        fault_of(&scan, "gone"),
        Some(&Fault::DependencyMissing {
            id: "nobody".into(),
            requirement: ">=2, <3".into(),
        })
    );
    assert_eq!(
        fault_of(&scan, "vers"),
        Some(&Fault::DependencyVersion {
            id: "base_mod".into(),
            requirement: "^2".into(),
            found: "1.0.0".into(),
        })
    );
    assert_eq!(
        fault_of(&scan, "off"),
        Some(&Fault::DependencyDisabled {
            id: "off_mod".into(),
        })
    );
    assert_eq!(
        fault_of(&scan, "chain"),
        Some(&Fault::DependencyRefused {
            id: "gone_user".into(),
        })
    );
    assert_eq!(scan.disabled.len(), 1);
}

#[test]
fn an_optional_dependency_that_is_absent_is_ignored_and_a_present_one_loads_first() {
    let root = tempfile::tempdir().unwrap();
    folder(
        root.path(),
        "host",
        "host_mod",
        "[dependencies]\nextra_mod = { version = \"1\", optional = true }\nghost_mod = { version = \"1\", optional = true }\n",
    );
    folder(root.path(), "extra", "extra_mod", "");
    let scan = run_scan(root.path(), &[]);
    assert!(scan.refused.is_empty(), "{:?}", refused_keys(&scan));
    assert_eq!(loaded_ids(&scan), ["extra_mod", "host_mod"]);
}

#[test]
fn a_cycle_of_two_and_a_cycle_of_three_are_refused_and_the_scan_finishes() {
    let root = tempfile::tempdir().unwrap();
    folder(
        root.path(),
        "a",
        "ping_mod",
        "[dependencies]\npong_mod = \"1\"\n",
    );
    folder(
        root.path(),
        "b",
        "pong_mod",
        "[dependencies]\nping_mod = \"1\"\n",
    );
    folder(
        root.path(),
        "c",
        "one_mod",
        "[dependencies]\ntwo_mod = \"1\"\n",
    );
    folder(
        root.path(),
        "d",
        "two_mod",
        "[dependencies]\nthree_mod = \"1\"\n",
    );
    folder(
        root.path(),
        "e",
        "three_mod",
        "[dependencies]\none_mod = \"1\"\n",
    );
    folder(root.path(), "f", "alone_mod", "");
    let scan = run_scan(root.path(), &[]);
    assert_eq!(loaded_ids(&scan), ["alone_mod"]);
    assert_eq!(scan.refused.len(), 5);
    assert!(
        scan.refused
            .iter()
            .all(|r| r.fault == Fault::DependencyCycle)
    );
}

#[test]
fn a_mod_that_needs_itself_is_refused_as_a_cycle() {
    let root = tempfile::tempdir().unwrap();
    folder(
        root.path(),
        "self",
        "self_mod",
        "[dependencies]\nself_mod = \"1\"\n",
    );
    let scan = run_scan(root.path(), &[]);
    assert!(scan.loaded.is_empty());
    assert_eq!(
        refused_keys(&scan),
        [("self".to_string(), "lua_mod.refused.dependency_cycle")]
    );
}

#[test]
fn the_1025th_mod_is_refused_with_its_key_and_the_first_1024_load() {
    let root = tempfile::tempdir().unwrap();
    for i in 0..=1024 {
        folder(root.path(), &format!("m{i:04}"), &format!("mod_{i:04}"), "");
    }
    let scan = run_scan(root.path(), &[]);
    assert_eq!(scan.loaded.len(), 1024);
    assert_eq!(
        refused_keys(&scan),
        [("m1024".to_string(), "lua_mod.refused.too_many_mods")]
    );
    assert_eq!(scan.refused[0].fault.args(), [("max", "1024".to_string())]);
}

#[test]
fn a_diamond_loads_its_base_once_and_first() {
    let root = tempfile::tempdir().unwrap();
    folder(
        root.path(),
        "top",
        "top_mod",
        "[dependencies]\nleft_mod = \"1\"\nright_mod = \"1\"\n",
    );
    folder(
        root.path(),
        "left",
        "left_mod",
        "[dependencies]\nbase_mod = \"1\"\n",
    );
    folder(
        root.path(),
        "right",
        "right_mod",
        "[dependencies]\nbase_mod = \"1\"\n",
    );
    folder(root.path(), "base", "base_mod", "");
    let scan = run_scan(root.path(), &[]);
    assert!(scan.refused.is_empty(), "{:?}", refused_keys(&scan));
    assert_eq!(
        loaded_ids(&scan),
        ["base_mod", "left_mod", "right_mod", "top_mod"]
    );
}

#[test]
fn the_dev_path_environment_variable_is_split_like_a_path_list() {
    assert!(env_dev_paths(None).is_empty());
    let joined = std::env::join_paths(["/games/a", "/games/b"]).unwrap();
    assert_eq!(
        env_dev_paths(Some(joined.as_os_str())),
        [PathBuf::from("/games/a"), PathBuf::from("/games/b")]
    );
    #[cfg(unix)]
    assert_eq!(
        env_dev_paths(Some(OsStr::new("/games/a::/games/b:"))),
        [PathBuf::from("/games/a"), PathBuf::from("/games/b")]
    );
}

/// The translation key of every refusal a package reader or the scan can give, built from a
/// `match` over every variant: a new `Refusal` or `Fault` variant does not compile until it is
/// listed here, and then the en_us file must carry its key.
fn every_key() -> Vec<&'static str> {
    fn listed(refusal: &Refusal) {
        match refusal {
            Refusal::Symlink
            | Refusal::NotPlain
            | Refusal::Unreadable
            | Refusal::Changed
            | Refusal::ArchiveTooLarge
            | Refusal::NotZip
            | Refusal::TrailingData
            | Refusal::CommentTooLong
            | Refusal::MultiDisk
            | Refusal::TooManyEntries
            | Refusal::BadName { .. }
            | Refusal::Duplicate { .. }
            | Refusal::Collision { .. }
            | Refusal::NotAllowed { .. }
            | Refusal::EntrySymlink { .. }
            | Refusal::EntrySpecial { .. }
            | Refusal::KindMismatch { .. }
            | Refusal::Encrypted { .. }
            | Refusal::Method { .. }
            | Refusal::EntryTooLarge { .. }
            | Refusal::TotalTooLarge { .. }
            | Refusal::TotalSourceTooLarge { .. }
            | Refusal::Ratio { .. }
            | Refusal::TotalRatio
            | Refusal::SizeMismatch { .. }
            | Refusal::Corrupt { .. }
            | Refusal::EntryMissing { .. }
            | Refusal::NotAnEntry { .. }
            | Refusal::NotAnAsset { .. }
            | Refusal::ScriptNotUtf8 { .. }
            | Refusal::ManifestMissing
            | Refusal::ManifestTooLarge
            | Refusal::ManifestNotUtf8
            | Refusal::ManifestNotToml
            | Refusal::UnknownKey { .. }
            | Refusal::MissingKey { .. }
            | Refusal::WrongType { .. }
            | Refusal::TextTooLong { .. }
            | Refusal::BadText { .. }
            | Refusal::TooMany { .. }
            | Refusal::BadId
            | Refusal::ReservedId
            | Refusal::BadVersion
            | Refusal::BadSide
            | Refusal::UnknownPower { .. }
            | Refusal::DuplicatePower { .. }
            | Refusal::NoEntries
            | Refusal::BadEntry { .. }
            | Refusal::BadDependency { .. }
            | Refusal::BadRequirement { .. }
            | Refusal::BadIcon
            | Refusal::AssetsReserved
            | Refusal::SettingsTooDeep => {}
        }
    }
    fn listed_fault(fault: &Fault) {
        match fault {
            Fault::Package(_)
            | Fault::DuplicateId { .. }
            | Fault::ReplacedByDev { .. }
            | Fault::DevPathMissing
            | Fault::DevPathNotHlua
            | Fault::DevPathsTooMany { .. }
            | Fault::TooManyMods { .. }
            | Fault::DependencyMissing { .. }
            | Fault::DependencyVersion { .. }
            | Fault::DependencyDisabled { .. }
            | Fault::DependencyRefused { .. }
            | Fault::DependencyCycle => {}
        }
    }
    let mut refusals: Vec<Refusal> = vec![
        Refusal::Symlink,
        Refusal::NotPlain,
        Refusal::Unreadable,
        Refusal::Changed,
        Refusal::ArchiveTooLarge,
        Refusal::NotZip,
        Refusal::TrailingData,
        Refusal::CommentTooLong,
        Refusal::MultiDisk,
        Refusal::TooManyEntries,
        Refusal::Duplicate { name: "x".into() },
        Refusal::Collision {
            name: "x".into(),
            other: "y".into(),
        },
        Refusal::NotAllowed { name: "x".into() },
        Refusal::EntrySymlink { name: "x".into() },
        Refusal::EntrySpecial { name: "x".into() },
        Refusal::KindMismatch { name: "x".into() },
        Refusal::Encrypted { name: "x".into() },
        Refusal::Method { name: "x".into() },
        Refusal::EntryTooLarge {
            name: "x".into(),
            max: 1,
        },
        Refusal::TotalTooLarge { max: 1 },
        Refusal::TotalSourceTooLarge { max: 1 },
        Refusal::Ratio { name: "x".into() },
        Refusal::TotalRatio,
        Refusal::SizeMismatch { name: "x".into() },
        Refusal::Corrupt { name: "x".into() },
        Refusal::EntryMissing { name: "x".into() },
        Refusal::NotAnEntry { name: "x".into() },
        Refusal::NotAnAsset { name: "x".into() },
        Refusal::ScriptNotUtf8 { name: "x".into() },
        Refusal::ManifestMissing,
        Refusal::ManifestTooLarge,
        Refusal::ManifestNotUtf8,
        Refusal::ManifestNotToml,
        Refusal::UnknownKey { key: "x".into() },
        Refusal::MissingKey { key: "x".into() },
        Refusal::WrongType { key: "x".into() },
        Refusal::TextTooLong {
            field: "x".into(),
            max: 1,
        },
        Refusal::BadText { field: "x".into() },
        Refusal::TooMany {
            field: "x".into(),
            max: 1,
        },
        Refusal::BadId,
        Refusal::ReservedId,
        Refusal::BadVersion,
        Refusal::BadSide,
        Refusal::UnknownPower { name: "x".into() },
        Refusal::DuplicatePower { name: "x".into() },
        Refusal::NoEntries,
        Refusal::BadEntry { value: "x".into() },
        Refusal::BadDependency { id: "x".into() },
        Refusal::BadRequirement { id: "x".into() },
        Refusal::BadIcon,
        Refusal::AssetsReserved,
        Refusal::SettingsTooDeep,
    ];
    for fault in [
        NameFault::Utf8,
        NameFault::Control,
        NameFault::Backslash,
        NameFault::Traversal,
        NameFault::Colon,
        NameFault::EmptySegment,
        NameFault::TooLong,
        NameFault::SegmentTooLong,
        NameFault::TooDeep,
    ] {
        refusals.push(Refusal::BadName {
            name: "x".into(),
            fault,
        });
    }
    let mut keys: Vec<&'static str> = Vec::new();
    for refusal in &refusals {
        listed(refusal);
        keys.push(refusal.key());
    }
    let faults = vec![
        Fault::DuplicateId {
            id: "x".into(),
            first: "y".into(),
        },
        Fault::ReplacedByDev { id: "x".into() },
        Fault::DevPathMissing,
        Fault::DevPathNotHlua,
        Fault::DevPathsTooMany { max: 8 },
        Fault::TooManyMods { max: 1024 },
        Fault::DependencyMissing {
            id: "x".into(),
            requirement: "1".into(),
        },
        Fault::DependencyVersion {
            id: "x".into(),
            requirement: "1".into(),
            found: "2".into(),
        },
        Fault::DependencyDisabled { id: "x".into() },
        Fault::DependencyRefused { id: "x".into() },
        Fault::DependencyCycle,
    ];
    for fault in &faults {
        listed_fault(fault);
        keys.push(fault.key());
    }
    keys
}

#[test]
fn every_refusal_and_scanner_key_has_an_english_entry() {
    let text = include_str!("../../horizon-client/assets/horizon/lang/en_us.json");
    let lang: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text).unwrap();
    let keys = every_key();
    assert!(keys.len() >= 71, "{} keys", keys.len());
    for key in keys {
        assert!(key.starts_with("lua_mod.refused."), "{key}");
        let value = lang.get(key).and_then(|v| v.as_str());
        assert!(
            value.is_some_and(|v| !v.is_empty()),
            "en_us.json has no entry for {key}"
        );
    }
}

#[test]
fn a_fault_names_its_arguments_and_its_text() {
    let fault = Fault::DependencyVersion {
        id: "base".into(),
        requirement: "^2".into(),
        found: "1.4.0".into(),
    };
    assert_eq!(
        fault.args(),
        [
            ("id", "base".to_string()),
            ("requirement", "^2".to_string()),
            ("found", "1.4.0".to_string()),
        ]
    );
    assert_eq!(
        fault.reason(),
        "it needs \"base\" \"^2\", but \"1.4.0\" is installed"
    );
    assert_eq!(fault.key(), "lua_mod.refused.dependency_version");
}
