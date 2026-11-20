//! Tests of the `.hlua` package reader: names, the layout, the limits and the refusals, on folders
//! and on zip archives. Archives are built in the test with the zip writer, or by patching bytes of
//! a valid archive, so every hostile shape is one the reader really meets.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::hlua::{
    MAX_ARCHIVE, MAX_COMMENT, MAX_ENTRIES, MAX_ENTRY, MAX_SCRIPT, MAX_TOTAL, NameFault, Package,
    Refusal, check_name, read_counted, same_file,
};

/// Writes `root/rel` with `bytes`, creating its folders.
fn put(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

const MANIFEST: &str = "id = \"alpha\"\nversion = \"1.0.0\"\nside = \"both\"\n";

/// A folder with a valid manifest and `main.luau`.
fn valid_folder() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "lua-mod.toml", MANIFEST.as_bytes());
    put(root.path(), "main.luau", b"return {}\n");
    root
}

fn refusal(result: Result<Package, Refusal>) -> Refusal {
    match result {
        Err(refusal) => refusal,
        Ok(_) => panic!("the package was accepted"),
    }
}

fn open(root: &Path) -> Result<Package, Refusal> {
    Package::open_dir(root)
}

#[test]
fn a_names_rules_split_good_from_bad() {
    let ok: Vec<String> = vec![
        "main.luau".into(),
        "assets/tex/a.png".into(),
        "data/x/".into(),
        "a-b_c.d".into(),
        "a".repeat(128),
        format!("{}/b", "a".repeat(100)),
        vec!["d"; 16].join("/"),
    ];
    for name in &ok {
        assert_eq!(check_name(name), Ok(()), "{name:?}");
    }
    let cases: Vec<(String, NameFault)> = vec![
        ("".into(), NameFault::EmptySegment),
        ("../x".into(), NameFault::Traversal),
        ("a/../../x".into(), NameFault::Traversal),
        ("a/./b".into(), NameFault::Traversal),
        ("/abs".into(), NameFault::Traversal),
        ("C:/x".into(), NameFault::Traversal),
        ("a:b".into(), NameFault::Traversal),
        ("x/a:b".into(), NameFault::Colon),
        ("a\\b".into(), NameFault::Backslash),
        ("a\0b".into(), NameFault::Control),
        ("a\nb".into(), NameFault::Control),
        ("a//b".into(), NameFault::EmptySegment),
        ("a".repeat(257), NameFault::TooLong),
        (format!("{}/b", "a".repeat(129)), NameFault::SegmentTooLong),
        (vec!["a"; 17].join("/"), NameFault::TooDeep),
    ];
    for (name, fault) in &cases {
        assert_eq!(check_name(name), Err(*fault), "{name:?}");
    }
}

#[test]
fn a_refusal_quotes_its_name_so_control_characters_show_escaped() {
    let text = Refusal::BadName {
        name: "a\nb".into(),
        fault: NameFault::Control,
    }
    .reason();
    assert!(text.contains("\\n") && !text.contains('\n'), "{text:?}");
}

#[test]
fn a_valid_folder_is_read_through_its_package() {
    let root = valid_folder();
    put(root.path(), "icon.png", b"\x89PNG");
    put(root.path(), "LICENSE.txt", b"MIT");
    put(root.path(), "assets/tex/a.png", b"tex");
    put(root.path(), "data/items.json", b"{}");
    put(root.path(), "lang/en_us.json", b"{}");
    let pkg = open(root.path()).unwrap();
    assert_eq!(pkg.manifest().id, "alpha");
    assert_eq!(pkg.entry_source("main.luau").unwrap(), "return {}\n");
    assert_eq!(pkg.asset_names(), vec!["assets/tex/a.png"]);
    assert_eq!(pkg.read_asset("assets/tex/a.png").unwrap(), b"tex");
}

#[test]
fn a_file_outside_the_layout_refuses_the_folder_by_name() {
    let root = valid_folder();
    put(root.path(), "notes.txt", b"hi");
    assert_eq!(
        refusal(open(root.path())),
        Refusal::NotAllowed {
            name: "notes.txt".into()
        }
    );
}

#[test]
fn a_script_is_allowed_only_where_the_manifest_lists_it() {
    let root = valid_folder();
    put(root.path(), "src/shop.luau", b"return 1\n");
    // The folder `src/` comes first and is refused: nothing lists a script under it.
    assert!(matches!(
        refusal(open(root.path())),
        Refusal::NotAllowed { .. }
    ));
    std::fs::remove_file(root.path().join("src/shop.luau")).unwrap();
    std::fs::remove_dir(root.path().join("src")).unwrap();
    put(root.path(), "src/shop.luau", b"return 1\n");
    let listed = format!("{MANIFEST}entries = [\"main.luau\", \"src/shop.luau\"]\n");
    put(root.path(), "lua-mod.toml", listed.as_bytes());
    let pkg = open(root.path()).unwrap();
    assert_eq!(pkg.entry_source("src/shop.luau").unwrap(), "return 1\n");
    // A script that exists but is not listed cannot be read as an entry.
    put(root.path(), "other.luau", b"return 2\n");
    assert_eq!(
        pkg.entry_source("other.luau"),
        Err(Refusal::NotAnEntry {
            name: "other.luau".into()
        })
    );
}

#[test]
fn a_folder_at_the_root_outside_the_layout_is_refused() {
    let root = valid_folder();
    std::fs::create_dir_all(root.path().join("scripts")).unwrap();
    assert_eq!(
        refusal(open(root.path())),
        Refusal::NotAllowed {
            name: "scripts/".into()
        }
    );
}

#[test]
fn a_file_named_like_a_layout_folder_is_refused() {
    let root = valid_folder();
    put(root.path(), "data", b"not a folder");
    assert_eq!(
        refusal(open(root.path())),
        Refusal::NotAllowed {
            name: "data".into()
        }
    );
}

#[test]
fn the_root_documents_are_allowed_in_any_case_but_not_lookalikes() {
    let root = valid_folder();
    put(root.path(), "readme.md", b"# hi");
    put(root.path(), "License", b"MIT");
    assert!(open(root.path()).is_ok());
    let other = valid_folder();
    put(other.path(), "readmex.md", b"no");
    assert_eq!(
        refusal(open(other.path())),
        Refusal::NotAllowed {
            name: "readmex.md".into()
        }
    );
}

#[test]
fn the_manifest_is_required_and_capped() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), "main.luau", b"return {}\n");
    assert_eq!(refusal(open(root.path())), Refusal::ManifestMissing);

    let big = format!("{MANIFEST}# {}\n", "x".repeat(70 * 1024));
    let root = valid_folder();
    put(root.path(), "lua-mod.toml", big.as_bytes());
    assert_eq!(refusal(open(root.path())), Refusal::ManifestTooLarge);

    let root = valid_folder();
    put(
        root.path(),
        "lua-mod.toml",
        b"id = \"alpha\"\nversion = \"1\"\nside = \"both\"\ncolour = 1\n",
    );
    assert_eq!(
        refusal(open(root.path())),
        Refusal::UnknownKey {
            key: "colour".into()
        }
    );
}

#[test]
fn a_listed_entry_or_icon_that_is_missing_refuses_the_folder() {
    let root = valid_folder();
    let listed = format!("{MANIFEST}entries = [\"main.luau\", \"gone.luau\"]\n");
    put(root.path(), "lua-mod.toml", listed.as_bytes());
    assert_eq!(
        refusal(open(root.path())),
        Refusal::EntryMissing {
            name: "gone.luau".into()
        }
    );
    let root = valid_folder();
    let iconed = format!("{MANIFEST}icon = \"icon.png\"\n");
    put(root.path(), "lua-mod.toml", iconed.as_bytes());
    assert_eq!(refusal(open(root.path())), Refusal::BadIcon);
}

#[test]
fn a_script_over_one_mebibyte_or_not_utf8_is_refused_when_read() {
    let root = valid_folder();
    put(
        root.path(),
        "main.luau",
        &vec![b'a'; MAX_SCRIPT as usize + 1],
    );
    let pkg = open(root.path()).unwrap();
    assert_eq!(
        pkg.entry_source("main.luau"),
        Err(Refusal::EntryTooLarge {
            name: "main.luau".into(),
            max: MAX_SCRIPT
        })
    );
    let root = valid_folder();
    put(root.path(), "main.luau", b"\xff\xfe");
    let pkg = open(root.path()).unwrap();
    assert_eq!(
        pkg.entry_source("main.luau"),
        Err(Refusal::ScriptNotUtf8 {
            name: "main.luau".into()
        })
    );
}

#[test]
fn an_asset_is_read_only_under_assets_and_capped_per_entry() {
    let root = valid_folder();
    put(root.path(), "data/d.json", b"{}");
    put(
        root.path(),
        "assets/big.bin",
        &vec![0u8; MAX_ENTRY as usize + 1],
    );
    // An asset over the per-entry cap refuses the whole folder, before any read.
    assert_eq!(
        refusal(open(root.path())),
        Refusal::EntryTooLarge {
            name: "assets/big.bin".into(),
            max: MAX_ENTRY
        }
    );
    let root = valid_folder();
    put(root.path(), "data/d.json", b"{}");
    let pkg = open(root.path()).unwrap();
    assert_eq!(
        pkg.read_asset("data/d.json"),
        Err(Refusal::NotAnAsset {
            name: "data/d.json".into()
        })
    );
    assert_eq!(
        pkg.read_asset("assets/none.png"),
        Err(Refusal::EntryMissing {
            name: "assets/none.png".into()
        })
    );
}

#[test]
fn the_entry_count_is_capped() {
    let root = valid_folder();
    for i in 0..MAX_ENTRIES {
        put(root.path(), &format!("lang/f{i}.json"), b"{}");
    }
    // main.luau, lua-mod.toml and the folder `lang/` come on top of the 4096 files.
    assert_eq!(refusal(open(root.path())), Refusal::TooManyEntries);
}

#[test]
fn names_that_collide_by_case_are_refused() {
    let root = valid_folder();
    put(root.path(), "lang/Ab.json", b"{}");
    put(root.path(), "lang/ab.json", b"{}");
    assert_eq!(
        refusal(open(root.path())),
        Refusal::Collision {
            name: "lang/ab.json".into(),
            other: "lang/Ab.json".into()
        }
    );
}

#[test]
fn a_name_with_a_backslash_is_refused_in_a_folder_too() {
    let root = valid_folder();
    put(root.path(), "lang/a\\b.json", b"{}");
    assert_eq!(
        refusal(open(root.path())),
        Refusal::BadName {
            name: "lang/a\\b.json".into(),
            fault: NameFault::Backslash
        }
    );
}

#[test]
fn a_folder_deeper_than_sixteen_segments_is_refused() {
    let root = valid_folder();
    let deep = format!("lang/{}/f.json", vec!["d"; 15].join("/"));
    put(root.path(), &deep, b"{}");
    assert_eq!(
        refusal(open(root.path())),
        Refusal::BadName {
            name: deep.clone(),
            fault: NameFault::TooDeep
        }
    );
}

#[test]
fn a_symlink_inside_the_folder_is_refused_by_name() {
    let root = valid_folder();
    let outside = put(root.path(), "outside.txt", b"secret");
    std::fs::create_dir_all(root.path().join("lang")).unwrap();
    std::os::unix::fs::symlink(&outside, root.path().join("lang/link.json")).unwrap();
    assert_eq!(
        refusal(open(root.path())),
        Refusal::EntrySymlink {
            name: "lang/link.json".into()
        }
    );
}

#[test]
fn a_symlinked_folder_or_package_root_is_refused() {
    let root = valid_folder();
    let link = root.path().join("linked");
    std::os::unix::fs::symlink(root.path(), &link).unwrap();
    assert_eq!(refusal(open(&link)), Refusal::Symlink);
}

#[test]
fn a_folder_swapped_for_a_symlink_after_the_walk_is_refused_on_read() {
    let root = valid_folder();
    put(root.path(), "assets/tex/a.png", b"tex");
    let pkg = open(root.path()).unwrap();
    // The walk saw a real `assets/`. Now it is a symlink to a folder that holds a file of the
    // same name: the read must refuse, not follow it.
    let elsewhere = tempfile::tempdir().unwrap();
    put(elsewhere.path(), "tex/a.png", b"evil");
    std::fs::rename(root.path().join("assets"), root.path().join("assets.moved")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), root.path().join("assets")).unwrap();
    assert_eq!(pkg.read_asset("assets/tex/a.png"), Err(Refusal::Symlink));
}

#[test]
fn a_file_swapped_after_the_open_is_caught_by_the_same_file_check() {
    let root = tempfile::tempdir().unwrap();
    let first = put(root.path(), "a.bin", b"first");
    let second = put(root.path(), "b.bin", b"second");
    let opened = std::fs::File::open(&first).unwrap();
    assert!(crate::hlua::same_file(&opened, &first));
    assert!(!crate::hlua::same_file(&opened, &second));
    std::fs::remove_file(&first).unwrap();
    std::os::unix::fs::symlink(&second, &first).unwrap();
    assert!(!crate::hlua::same_file(&opened, &first));
}

#[test]
fn a_folder_that_is_not_a_folder_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let file = put(root.path(), "plain.hlua", b"not a package");
    assert_eq!(refusal(open(&file)), Refusal::NotPlain);
}

// ---- archives ----

/// A package's files as (name, bytes). A name ending in `/` is a folder entry.
type Files = Vec<(String, Vec<u8>)>;

const ARCHIVE_MANIFEST: &str = "id = \"alpha\"\nversion = \"1.0.0\"\nside = \"both\"\n";

/// A valid package: the manifest, the script, the icon, a document and one file per folder.
fn good() -> Files {
    vec![
        ("lua-mod.toml".into(), ARCHIVE_MANIFEST.as_bytes().to_vec()),
        ("main.luau".into(), b"return {}\n".to_vec()),
        ("icon.png".into(), b"\x89PNG".to_vec()),
        ("LICENSE.txt".into(), b"MIT".to_vec()),
        ("assets/".into(), Vec::new()),
        ("assets/tex/a.png".into(), b"tex".to_vec()),
        ("data/items.json".into(), b"{}".to_vec()),
        ("lang/en_us.json".into(), b"{}".to_vec()),
    ]
}

/// `files` with `name` set to `bytes`, replacing a file of that name.
fn with(mut files: Files, name: &str, bytes: &[u8]) -> Files {
    files.retain(|(n, _)| n != name);
    files.push((name.into(), bytes.to_vec()));
    files
}

/// Adds the files to a zip, a name ending in `/` as a folder.
fn write_files(writer: &mut ZipWriter<Cursor<Vec<u8>>>, files: &Files, method: CompressionMethod) {
    for (name, bytes) in files {
        let options = SimpleFileOptions::default().compression_method(method);
        if name.ends_with('/') {
            writer.add_directory(name.as_str(), options).unwrap();
        } else {
            writer.start_file(name.as_str(), options).unwrap();
            writer.write_all(bytes).unwrap();
        }
    }
}

/// The files as a zip, deflated, or stored when `stored`.
fn zip(files: &Files, stored: bool) -> Vec<u8> {
    let method = if stored {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    write_files(&mut writer, files, method);
    writer.finish().unwrap().into_inner()
}

/// Writes `bytes` as a `.hlua` file and opens it as an archive.
fn open_bytes(bytes: &[u8]) -> Result<Package, Refusal> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mod.hlua");
    std::fs::write(&path, bytes).unwrap();
    Package::open_archive(&path)
}

fn refused_bytes(bytes: &[u8]) -> Refusal {
    refusal(open_bytes(bytes))
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

/// The offset of the central-directory record (`central`) or the local header of `name`, found by
/// its signature and its name.
fn record_of(bytes: &[u8], name: &str, central: bool) -> usize {
    let (sig, len_at, name_at): (&[u8], usize, usize) = if central {
        (b"PK\x01\x02", 28, 46)
    } else {
        (b"PK\x03\x04", 26, 30)
    };
    (0..bytes.len().saturating_sub(name_at))
        .find(|&i| {
            bytes[i..].starts_with(sig)
                && usize::from(le16(bytes, i + len_at)) == name.len()
                && bytes.get(i + name_at..i + name_at + name.len()) == Some(name.as_bytes())
        })
        .unwrap_or_else(|| panic!("no record for {name:?}"))
}

fn poke(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

/// Sets the flags of `name` in both of its headers (bit 0 is "encrypted").
fn set_flags(bytes: &mut [u8], name: &str, flags: u16) {
    let central = record_of(bytes, name, true);
    poke(bytes, central + 8, &flags.to_le_bytes());
    let local = record_of(bytes, name, false);
    poke(bytes, local + 6, &flags.to_le_bytes());
}

/// Sets the compression method of `name` in both of its headers.
fn set_method(bytes: &mut [u8], name: &str, method: u16) {
    let central = record_of(bytes, name, true);
    poke(bytes, central + 10, &method.to_le_bytes());
    let local = record_of(bytes, name, false);
    poke(bytes, local + 8, &method.to_le_bytes());
}

/// Sets the compressed and the uncompressed size `name`'s central record declares.
fn set_sizes(bytes: &mut [u8], name: &str, compressed: u32, size: u32) {
    let central = record_of(bytes, name, true);
    poke(bytes, central + 20, &compressed.to_le_bytes());
    poke(bytes, central + 24, &size.to_le_bytes());
}

/// Sets the external attributes of `name` to a unix mode (the high 16 bits), made by Unix.
fn set_unix_mode(bytes: &mut [u8], name: &str, mode: u32) {
    let central = record_of(bytes, name, true);
    poke(bytes, central + 5, &[3]);
    poke(bytes, central + 38, &(mode << 16).to_le_bytes());
}

/// Renames every occurrence of `from` to `to`, which must be as long, in both headers.
fn rename(bytes: &mut [u8], from: &str, to: &str) {
    assert_eq!(from.len(), to.len());
    let mut at = 0;
    while let Some(found) = bytes[at..]
        .windows(from.len())
        .position(|w| w == from.as_bytes())
    {
        let start = at + found;
        poke(bytes, start, to.as_bytes());
        at = start + from.len();
    }
}

/// The offset of the last end-of-central-directory record.
fn end_record(bytes: &[u8]) -> usize {
    (0..bytes.len() - 3)
        .rev()
        .find(|&i| bytes[i..].starts_with(b"PK\x05\x06"))
        .expect("an end record")
}

#[test]
fn a_valid_archive_loads_to_the_same_package_as_its_folder() {
    let mut files = good();
    files.push(("lang/".into(), Vec::new()));
    files.push(("assets/tex/inner.hlua".into(), zip(&good(), true)));
    let bytes = zip(&files, false);
    let archive = open_bytes(&bytes).unwrap();

    let root = tempfile::tempdir().unwrap();
    for (name, body) in &files {
        if name.ends_with('/') {
            std::fs::create_dir_all(root.path().join(name)).unwrap();
        } else {
            put(root.path(), name, body);
        }
    }
    let folder = open(root.path()).unwrap();

    assert_eq!(archive.manifest(), folder.manifest());
    assert_eq!(
        archive.entry_source("main.luau"),
        folder.entry_source("main.luau")
    );
    assert_eq!(archive.asset_names(), folder.asset_names());
    assert_eq!(
        archive.asset_names(),
        vec!["assets/tex/a.png", "assets/tex/inner.hlua"]
    );
    for asset in archive.asset_names() {
        assert_eq!(archive.read_asset(&asset), folder.read_asset(&asset));
    }
    // The nested archive is just an asset: its bytes come back, and it is never opened.
    assert_eq!(
        archive.read_asset("assets/tex/inner.hlua").unwrap(),
        zip(&good(), true)
    );
}

#[test]
fn a_zip64_archive_within_the_same_limits_loads() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    // Extensible data forces the zip64 end record; the archive is otherwise the same.
    writer.set_raw_zip64_extensible_data_sector(vec![0u8; 8].into_boxed_slice());
    write_files(&mut writer, &good(), CompressionMethod::Deflated);
    let bytes = writer.finish().unwrap().into_inner();
    assert_eq!(open_bytes(&bytes).err(), None);
}

#[test]
fn an_archive_path_names_the_same_file_or_it_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let first = put(dir.path(), "a.hlua", &zip(&good(), false));
    let second = put(dir.path(), "b.hlua", &zip(&good(), false));
    let opened = std::fs::File::open(&first).unwrap();
    assert!(same_file(&opened, &first));
    // Replaced by another file, or by a link to one, after the handle was opened: no match, and
    // a link at the path is refused when the path is opened.
    std::fs::remove_file(&first).unwrap();
    std::os::unix::fs::symlink(&second, &first).unwrap();
    assert!(!same_file(&opened, &first));
    assert_eq!(refusal(Package::open_archive(&first)), Refusal::Symlink);
    std::fs::remove_file(&first).unwrap();
    std::fs::write(&first, zip(&good(), false)).unwrap();
    assert!(!same_file(&opened, &first));
}

#[test]
fn an_archive_over_the_file_cap_is_refused_before_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.hlua");
    // A sparse file: its length is the cap plus one, and it holds no bytes that are read.
    std::fs::File::create(&path)
        .unwrap()
        .set_len(MAX_ARCHIVE + 1)
        .unwrap();
    assert_eq!(
        refusal(Package::open_archive(&path)),
        Refusal::ArchiveTooLarge
    );
}

#[test]
fn a_folder_or_missing_path_is_not_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        refusal(Package::open_archive(dir.path())),
        Refusal::NotPlain
    );
    assert_eq!(
        refusal(Package::open_archive(&dir.path().join("none.hlua"))),
        Refusal::Unreadable
    );
}

#[test]
fn a_name_that_leaves_the_package_or_is_not_plain_is_refused_by_name() {
    let cases: Vec<(String, NameFault)> = vec![
        ("../x".into(), NameFault::Traversal),
        ("lang/../../x".into(), NameFault::Traversal),
        ("/abs.json".into(), NameFault::Traversal),
        ("C:/x.json".into(), NameFault::Traversal),
        ("lang/a\\b.json".into(), NameFault::Backslash),
        ("lang/a\0b.json".into(), NameFault::Control),
        ("lang/a\nb.json".into(), NameFault::Control),
        ("lang//b.json".into(), NameFault::EmptySegment),
        (format!("lang/{}", "a".repeat(257)), NameFault::TooLong),
        (
            format!("lang/{}/b.json", "a".repeat(129)),
            NameFault::SegmentTooLong,
        ),
        (
            format!("lang/{}.json", vec!["d"; 16].join("/")),
            NameFault::TooDeep,
        ),
    ];
    for (name, fault) in cases {
        // The writer is given a placeholder of the same length, which is then renamed in both
        // headers, so the reader meets the name byte for byte.
        let placeholder = "x".repeat(name.len());
        let mut files = good();
        files.push((placeholder.clone(), b"{}".to_vec()));
        let mut bytes = zip(&files, true);
        rename(&mut bytes, &placeholder, &name);
        assert_eq!(
            refused_bytes(&bytes),
            Refusal::BadName {
                name: name.clone(),
                fault
            },
            "{name:?}"
        );
    }
}

#[test]
fn a_duplicate_name_refuses_the_archive() {
    let mut files = good();
    files.push(("lang/a.json".into(), b"1".to_vec()));
    files.push(("lang/b.json".into(), b"2".to_vec()));
    let mut bytes = zip(&files, true);
    rename(&mut bytes, "lang/b.json", "lang/a.json");
    assert_eq!(
        refused_bytes(&bytes),
        Refusal::Duplicate {
            name: "lang/a.json".into()
        }
    );
}

#[test]
fn names_that_differ_only_by_case_collide() {
    let files = with(with(good(), "lang/Ab.json", b"{}"), "lang/ab.json", b"{}");
    assert_eq!(
        refused_bytes(&zip(&files, true)),
        Refusal::Collision {
            name: "lang/ab.json".into(),
            other: "lang/Ab.json".into()
        }
    );
}

#[test]
fn the_entry_count_is_inclusive_at_4096_and_refused_at_4097() {
    let base = good();
    let mut at_cap = good();
    for i in 0..MAX_ENTRIES - base.len() {
        at_cap.push((format!("lang/f{i}.json"), b"{}".to_vec()));
    }
    assert_eq!(at_cap.len(), MAX_ENTRIES);
    assert!(open_bytes(&zip(&at_cap, true)).is_ok());
    at_cap.push(("lang/over.json".into(), b"{}".to_vec()));
    assert_eq!(refused_bytes(&zip(&at_cap, true)), Refusal::TooManyEntries);
}

#[test]
fn a_deflate_bomb_is_refused_by_ratio_after_its_header_is_read() {
    // A megabyte of zeros deflates to about a kilobyte: over 100 to 1, refused without a read.
    let bomb = with(good(), "lang/bomb.bin", &vec![0u8; 1 << 20]);
    assert_eq!(
        refused_bytes(&zip(&bomb, false)),
        Refusal::Ratio {
            name: "lang/bomb.bin".into()
        }
    );
}

#[test]
fn a_header_claiming_more_than_the_entry_cap_is_refused_without_a_read() {
    // The bytes are three; the header claims 64 MiB, compressed as much. The size cap is checked
    // on the header, so nothing is allocated for it.
    let mut bytes = zip(&with(good(), "lang/big.bin", b"abc"), true);
    set_sizes(&mut bytes, "lang/big.bin", 64 << 20, 64 << 20);
    assert_eq!(
        refused_bytes(&bytes),
        Refusal::EntryTooLarge {
            name: "lang/big.bin".into(),
            max: MAX_ENTRY
        }
    );
}

#[test]
fn a_header_claiming_a_bomb_ratio_is_refused_without_a_read() {
    // Eight mebibytes from three stored bytes: under the entry cap, but far past 100 to 1.
    let mut bytes = zip(&with(good(), "lang/r.bin", b"abc"), true);
    set_sizes(&mut bytes, "lang/r.bin", 1000, MAX_ENTRY as u32);
    assert_eq!(
        refused_bytes(&bytes),
        Refusal::Ratio {
            name: "lang/r.bin".into()
        }
    );
}

#[test]
fn an_empty_file_is_not_a_zip() {
    assert_eq!(refused_bytes(&[]), Refusal::NotZip);
}

#[test]
fn headers_that_add_up_past_the_total_cap_are_refused_without_a_read() {
    // Nine entries of exactly the per-entry cap: each is allowed, the nine together are not.
    let mut files = good();
    for i in 0..9 {
        files.push((format!("lang/t{i}.json"), b"{}".to_vec()));
    }
    let mut bytes = zip(&files, true);
    for i in 0..9 {
        set_sizes(
            &mut bytes,
            &format!("lang/t{i}.json"),
            MAX_ENTRY as u32,
            MAX_ENTRY as u32,
        );
    }
    assert_eq!(MAX_TOTAL, 64 << 20);
    assert_eq!(
        refused_bytes(&bytes),
        Refusal::TotalTooLarge { max: MAX_TOTAL }
    );
}

#[test]
fn a_header_that_lies_about_its_size_is_refused_when_read() {
    // Stored bytes, with a header that says five where there are twelve.
    let mut bytes = zip(&good(), true);
    set_sizes(&mut bytes, "main.luau", 10, 5);
    let refusal = refused_bytes(&bytes);
    assert!(
        matches!(
            refusal,
            Refusal::Corrupt { .. } | Refusal::SizeMismatch { .. }
        ),
        "{refusal:?}"
    );
}

#[test]
fn a_stream_is_counted_by_what_it_produces_not_by_its_header() {
    // The header says a megabyte, the stream runs on past the entry cap: refused at the cap.
    let endless = vec![0u8; (MAX_ENTRY as usize) + 1024];
    assert_eq!(
        read_counted(Cursor::new(&endless), "x", 1 << 20, MAX_ENTRY),
        Err(Refusal::EntryTooLarge {
            name: "x".into(),
            max: MAX_ENTRY
        })
    );
    // The header says less than the stream holds, under the cap: a size mismatch.
    assert_eq!(
        read_counted(Cursor::new(vec![0u8; 100]), "x", 10, MAX_ENTRY),
        Err(Refusal::SizeMismatch { name: "x".into() })
    );
    // Exactly the cap, as the header says: accepted.
    assert!(
        read_counted(
            Cursor::new(vec![0u8; MAX_ENTRY as usize]),
            "x",
            MAX_ENTRY,
            MAX_ENTRY
        )
        .is_ok()
    );
}

#[test]
fn a_symlink_entry_is_refused_by_name() {
    let mut files = good();
    files.push(("lang/link.json".into(), b"/etc/passwd".to_vec()));
    let mut bytes = zip(&files, true);
    set_unix_mode(&mut bytes, "lang/link.json", 0o120777);
    assert_eq!(
        refused_bytes(&bytes),
        Refusal::EntrySymlink {
            name: "lang/link.json".into()
        }
    );
}

#[test]
fn an_encrypted_entry_is_refused_by_name() {
    let mut bytes = zip(&with(good(), "lang/secret.json", b"{}"), true);
    set_flags(&mut bytes, "lang/secret.json", 1);
    assert_eq!(
        refused_bytes(&bytes),
        Refusal::Encrypted {
            name: "lang/secret.json".into()
        }
    );
}

#[test]
fn an_unsupported_method_is_refused_by_name() {
    let mut bytes = zip(&with(good(), "lang/x.json", b"{}"), true);
    set_method(&mut bytes, "lang/x.json", 99);
    assert_eq!(
        refused_bytes(&bytes),
        Refusal::Method {
            name: "lang/x.json".into()
        }
    );
}

#[test]
fn a_file_outside_the_layout_or_a_folder_outside_it_is_refused() {
    assert_eq!(
        refused_bytes(&zip(&with(good(), "notes.txt", b"hi"), true)),
        Refusal::NotAllowed {
            name: "notes.txt".into()
        }
    );
    assert_eq!(
        refused_bytes(&zip(&with(good(), "scripts/", b""), true)),
        Refusal::NotAllowed {
            name: "scripts/".into()
        }
    );
}

#[test]
fn an_archive_without_a_manifest_or_with_none_is_refused() {
    let files: Files = good()
        .into_iter()
        .filter(|(n, _)| n != "lua-mod.toml")
        .collect();
    assert_eq!(refused_bytes(&zip(&files, true)), Refusal::ManifestMissing);
    assert_eq!(
        refused_bytes(&zip(&Vec::new(), true)),
        Refusal::ManifestMissing
    );
}

#[test]
fn manifests_inside_an_archive_are_refused_as_the_folder_would_refuse_them() {
    let cases: Vec<(Vec<u8>, Refusal)> = vec![
        (
            b"id = \"alpha\"\nversion = \"1\"\nside = \"both\"\ncolour = 1\n".to_vec(),
            Refusal::UnknownKey {
                key: "colour".into(),
            },
        ),
        (
            b"id = \"Bad\"\nversion = \"1\"\nside = \"both\"\n".to_vec(),
            Refusal::BadId,
        ),
        (
            b"id = \"horizon\"\nversion = \"1\"\nside = \"both\"\n".to_vec(),
            Refusal::ReservedId,
        ),
        (
            b"id = \"alpha\"\nversion = \"1\"\nside = \"Both\"\n".to_vec(),
            Refusal::BadSide,
        ),
        (
            b"id = \"alpha\"\nversion = \"1\"\nside = \"both\"\npowers = [\"fly\"]\n".to_vec(),
            Refusal::UnknownPower { name: "fly".into() },
        ),
        (
            b"id = \"alpha\"\nversion = \"1\"\nside = \"both\"\nassets = []\n".to_vec(),
            Refusal::AssetsReserved,
        ),
        (b"id = \"a\xff\"\n".to_vec(), Refusal::ManifestNotUtf8),
        (
            format!("{ARCHIVE_MANIFEST}# {}\n", "x".repeat(70 * 1024)).into_bytes(),
            Refusal::ManifestTooLarge,
        ),
    ];
    for (manifest, expected) in cases {
        let bytes = zip(&with(good(), "lua-mod.toml", &manifest), true);
        assert_eq!(refused_bytes(&bytes), expected);
    }
}

#[test]
fn a_script_is_capped_and_must_be_utf8() {
    let big = with(good(), "main.luau", &vec![b'a'; MAX_SCRIPT as usize + 1]);
    let pkg = open_bytes(&zip(&big, true)).unwrap();
    assert_eq!(
        pkg.entry_source("main.luau"),
        Err(Refusal::EntryTooLarge {
            name: "main.luau".into(),
            max: MAX_SCRIPT
        })
    );
    let bad = with(good(), "main.luau", b"\xff\xfe");
    let pkg = open_bytes(&zip(&bad, true)).unwrap();
    assert_eq!(
        pkg.entry_source("main.luau"),
        Err(Refusal::ScriptNotUtf8 {
            name: "main.luau".into()
        })
    );
}

#[test]
fn a_zip_comment_is_capped_at_one_kibibyte() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .set_raw_comment(vec![b'c'; MAX_COMMENT].into_boxed_slice())
        .unwrap();
    write_files(&mut writer, &good(), CompressionMethod::Deflated);
    let at_cap = writer.finish().unwrap().into_inner();
    assert_eq!(open_bytes(&at_cap).err(), None);

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .set_raw_comment(vec![b'c'; MAX_COMMENT + 1].into_boxed_slice())
        .unwrap();
    write_files(&mut writer, &good(), CompressionMethod::Deflated);
    let over = writer.finish().unwrap().into_inner();
    assert_eq!(refused_bytes(&over), Refusal::CommentTooLong);
}

#[test]
fn a_truncated_archive_is_not_a_zip() {
    let bytes = zip(&good(), false);
    assert_eq!(refused_bytes(&bytes[..bytes.len() - 10]), Refusal::NotZip);
}

#[test]
fn bytes_appended_after_the_archive_are_refused() {
    let mut bytes = zip(&good(), false);
    bytes.extend_from_slice(b"junk");
    assert_eq!(refused_bytes(&bytes), Refusal::TrailingData);
}

#[test]
fn bytes_in_front_of_the_archive_are_refused() {
    let mut bytes = b"junkjunkjunkjunk".to_vec();
    bytes.extend_from_slice(&zip(&good(), false));
    assert_eq!(refused_bytes(&bytes), Refusal::TrailingData);
}

#[test]
fn bytes_between_the_directory_and_its_end_record_are_refused() {
    let bytes = zip(&good(), false);
    let end = end_record(&bytes);
    let mut spliced = bytes[..end].to_vec();
    spliced.extend_from_slice(b"junk");
    spliced.extend_from_slice(&bytes[end..]);
    assert_eq!(refused_bytes(&spliced), Refusal::TrailingData);
}

#[test]
fn a_corrupt_central_directory_is_not_a_zip() {
    let mut bytes = zip(&good(), false);
    let central = record_of(&bytes, "main.luau", true);
    poke(&mut bytes, central, b"XXXX");
    assert_eq!(refused_bytes(&bytes), Refusal::NotZip);
}

#[test]
fn a_zero_entry_archive_has_no_manifest() {
    assert_eq!(
        refused_bytes(&zip(&Vec::new(), false)),
        Refusal::ManifestMissing
    );
}

#[test]
fn a_second_disk_is_refused() {
    let mut bytes = zip(&good(), false);
    let end = end_record(&bytes);
    poke(&mut bytes, end + 4, &1u16.to_le_bytes());
    assert_eq!(refused_bytes(&bytes), Refusal::MultiDisk);
}

#[test]
fn a_script_listed_under_a_folder_is_read_from_the_archive() {
    let listed = format!("{ARCHIVE_MANIFEST}entries = [\"main.luau\", \"src/shop.luau\"]\n");
    let files = with(
        with(good(), "lua-mod.toml", listed.as_bytes()),
        "src/shop.luau",
        b"return 1\n",
    );
    let pkg = open_bytes(&zip(&files, false)).unwrap();
    assert_eq!(pkg.entry_source("src/shop.luau").unwrap(), "return 1\n");
}

/// Every byte of a valid archive, flipped, inserted or removed at random, opens or is refused
/// with a reason: it never panics. The generator is a fixed xorshift, so the run is the same every
/// time.
#[test]
fn a_mutated_archive_never_panics_and_is_loaded_or_refused() {
    let base = zip(&with(good(), "lang/en_gb.json", b"{\"a\":1}"), false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fuzz.hlua");
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (mut loaded, mut refused) = (0u32, 0u32);
    for _ in 0..3000 {
        let mut bytes = base.clone();
        for _ in 0..1 + next() % 3 {
            let at = (next() as usize) % bytes.len();
            match next() % 3 {
                0 => bytes[at] ^= 1 << (next() % 8),
                1 => bytes.insert(at, next() as u8),
                _ => {
                    if bytes.len() > 1 {
                        bytes.remove(at);
                    }
                }
            }
        }
        std::fs::write(&path, &bytes).unwrap();
        match Package::open_archive(&path) {
            Ok(pkg) => {
                loaded += 1;
                let _ = pkg.entry_source("main.luau");
                for asset in pkg.asset_names() {
                    let _ = pkg.read_asset(&asset);
                }
            }
            Err(_) => refused += 1,
        }
    }
    assert_eq!(loaded + refused, 3000);
    assert!(refused > 0, "the mutations never reached a refusal");
}
