//! Tests of the enabled-state file: what a missing, bad, oversized or symlinked file means for the
//! mods; that a write keeps the other entries and leaves no partial file; and that two writers at
//! once lose no entry.

use std::path::{Path, PathBuf};

use crate::discover::DIR;
use crate::enabled::{FILE, MAX_ENABLED, read, write};

fn file_path(root: &Path) -> PathBuf {
    root.join(DIR).join(FILE)
}

fn put(root: &Path, text: &str) {
    std::fs::create_dir_all(root.join(DIR)).unwrap();
    std::fs::write(file_path(root), text).unwrap();
}

/// The file's entries as the scan sees them after a fresh read.
fn states(root: &Path) -> std::collections::BTreeMap<String, bool> {
    read(root).states.into_iter().collect()
}

/// The file names in `lua-mods` that are not `enabled.toml`: a temporary left behind shows here.
fn other_files(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join(DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != FILE)
        .collect();
    names.sort();
    names
}

#[test]
fn an_absent_file_enables_every_mod_with_no_notice() {
    let root = tempfile::tempdir().unwrap();
    let enabled = read(root.path());
    assert!(enabled.states.is_empty());
    assert_eq!(enabled.notice, None);
    assert!(enabled.is_enabled("anything"));
}

#[test]
fn a_false_entry_disables_and_a_true_or_missing_one_does_not() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "[mods]\nkeep = true\nstop = false\n");
    let enabled = read(root.path());
    assert_eq!(enabled.notice, None);
    assert!(enabled.is_enabled("keep"));
    assert!(!enabled.is_enabled("stop"));
    assert!(enabled.is_enabled("unlisted"));
}

#[test]
fn bad_toml_is_one_notice_and_every_mod_stays_enabled() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "[mods\nstop = false\n");
    let enabled = read(root.path());
    assert!(enabled.states.is_empty());
    assert!(enabled.is_enabled("stop"));
    let notice = enabled.notice.expect("one notice");
    assert!(notice.contains("not valid TOML"), "{notice}");
}

#[test]
fn an_oversized_file_is_ignored_with_a_notice() {
    let root = tempfile::tempdir().unwrap();
    let mut text = String::from("[mods]\nstop = false\n");
    text.push_str(&"#".repeat(MAX_ENABLED as usize));
    put(root.path(), &text);
    let enabled = read(root.path());
    assert!(enabled.states.is_empty());
    assert!(enabled.is_enabled("stop"));
    assert!(enabled.notice.unwrap().contains("over 64 KiB"));
}

#[test]
fn an_unknown_id_is_ignored_and_invalid_entries_are_counted_once() {
    let root = tempfile::tempdir().unwrap();
    // "ghost" names no installed mod: it is kept as a state but changes nothing. "Bad-Id" and a
    // string value are invalid: they are ignored and reported in one notice.
    put(
        root.path(),
        "[mods]\nghost = false\n\"Bad-Id\" = false\nwrong = \"yes\"\n",
    );
    let enabled = read(root.path());
    assert!(enabled.is_enabled("Bad-Id") && enabled.is_enabled("wrong"));
    assert!(!enabled.is_enabled("ghost"));
    assert_eq!(
        enabled.notice.as_deref(),
        Some("enabled.toml has 2 entries that are not valid; they are ignored")
    );
}

#[test]
fn a_mods_table_that_is_not_a_table_enables_everything() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "mods = 3\n");
    let enabled = read(root.path());
    assert!(enabled.states.is_empty());
    assert!(enabled.notice.is_some());
}

#[cfg(unix)]
#[test]
fn a_symlinked_file_is_not_read_and_not_written() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside.toml");
    std::fs::write(&outside, "[mods]\nstop = false\n").unwrap();
    std::fs::create_dir_all(root.path().join(DIR)).unwrap();
    std::os::unix::fs::symlink(&outside, file_path(root.path())).unwrap();

    let enabled = read(root.path());
    assert!(enabled.is_enabled("stop"));
    assert!(enabled.notice.unwrap().contains("symlink"));

    let err = write(root.path(), "stop", true).unwrap_err();
    assert!(err.contains("symlink"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "[mods]\nstop = false\n"
    );
    assert!(other_files(root.path()).is_empty());
}

#[test]
fn a_write_keeps_the_other_entries_and_leaves_no_temporary_file() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "alpha", false).unwrap();
    write(root.path(), "beta", true).unwrap();
    write(root.path(), "alpha", true).unwrap();
    assert_eq!(
        states(root.path()).into_iter().collect::<Vec<_>>(),
        [("alpha".to_string(), true), ("beta".to_string(), true)]
    );
    assert!(
        other_files(root.path()).is_empty(),
        "{:?}",
        other_files(root.path())
    );
    let text = std::fs::read_to_string(file_path(root.path())).unwrap();
    assert!(text.parse::<toml::Table>().is_ok(), "{text}");
}

#[test]
fn a_write_keeps_entries_it_does_not_change_and_unrelated_keys() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "note = 1\n[mods]\nkeep = false\n");
    write(root.path(), "new_mod", false).unwrap();
    let table: toml::Table = std::fs::read_to_string(file_path(root.path()))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(table["note"].as_integer(), Some(1));
    let mods = table["mods"].as_table().unwrap();
    assert_eq!(mods["keep"].as_bool(), Some(false));
    assert_eq!(mods["new_mod"].as_bool(), Some(false));
}

#[test]
fn a_write_refuses_a_file_it_cannot_parse_and_does_not_overwrite_it() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "[mods\nkeep = false\n");
    let err = write(root.path(), "stop", false).unwrap_err();
    assert!(err.contains("not valid TOML"), "{err}");
    assert_eq!(
        std::fs::read_to_string(file_path(root.path())).unwrap(),
        "[mods\nkeep = false\n"
    );
}

#[test]
fn a_write_refuses_an_invalid_id() {
    let root = tempfile::tempdir().unwrap();
    assert!(write(root.path(), "Bad-Id", false).is_err());
    assert!(write(root.path(), "", false).is_err());
    assert!(!file_path(root.path()).exists());
}

#[test]
fn two_writers_at_once_lose_no_entry_and_leave_no_partial_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_path_buf();
    let writers: Vec<_> = ["aaa", "bbb"]
        .into_iter()
        .map(|prefix| {
            let path = path.clone();
            std::thread::spawn(move || {
                for i in 0..30 {
                    write(&path, &format!("{prefix}_{i:02}"), i % 2 == 0).unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let found = states(root.path());
    assert_eq!(found.len(), 60, "{found:?}");
    for prefix in ["aaa", "bbb"] {
        for i in 0..30 {
            assert_eq!(
                found.get(&format!("{prefix}_{i:02}")),
                Some(&(i % 2 == 0)),
                "{prefix}_{i:02}"
            );
        }
    }
    assert!(
        other_files(root.path()).is_empty(),
        "{:?}",
        other_files(root.path())
    );
}
