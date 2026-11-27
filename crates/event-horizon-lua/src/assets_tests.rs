//! Tests of the publication of a Lua mod's content (`assets.rs`): what a published pack holds, that
//! a folder and an archive give the same pack, that an existing pack is reused or refused, that a
//! failure leaves nothing, that hostile names never reach the disk, and that stale packs go.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use zip::write::{SimpleFileOptions, ZipWriter};

use crate::assets::{
    self, AssetError, CACHE_DIR, DIR, KEY, MANIFEST, MAX_REMOVALS, check_target, manifest_text,
    pack_id, pack_side, publish, publish_with,
};
use crate::hlua::Package;
use crate::manifest::Side;

const LUA_MANIFEST: &str =
    "id = \"alpha\"\nname = \"Alpha\"\nversion = \"1.0.0\"\nside = \"both\"\n";

/// Writes a Lua mod folder: its manifest, a script, and `files` (relative paths and bytes).
fn folder_mod(root: &Path, manifest: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = root.join("mod");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lua-mod.toml"), manifest).unwrap();
    std::fs::write(dir.join("main.luau"), b"return {}\n").unwrap();
    for (rel, bytes) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    dir
}

/// The same mod as a `.hlua` archive with the same entries, written by the zip writer.
fn archive_mod(root: &Path, manifest: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let path = root.join("alpha.hlua");
    let mut writer = ZipWriter::new(std::fs::File::create(&path).unwrap());
    let options = SimpleFileOptions::default();
    writer.start_file("lua-mod.toml", options).unwrap();
    writer.write_all(manifest.as_bytes()).unwrap();
    writer.start_file("main.luau", options).unwrap();
    writer.write_all(b"return {}\n").unwrap();
    for (rel, bytes) in files {
        writer.start_file(*rel, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    path
}

fn open(dir: &Path) -> Package {
    Package::open_dir(dir).unwrap()
}

fn sample() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("assets/alpha/textures/item/x.png", b"texture bytes"),
        ("data/alpha/recipe/r.json", b"{\"type\": \"crafting\"}"),
        ("lang/en_us.json", b"{\"item.alpha.x\": \"X\"}"),
    ]
}

/// Every file and folder under `root`, relative and sorted, with `/` separators.
fn walk(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let meta = std::fs::symlink_metadata(entry.path()).unwrap();
            if meta.is_dir() {
                out.push(format!("{rel}/"));
                stack.push((entry.path(), rel));
            } else {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

fn lua_root(game: &Path) -> PathBuf {
    game.join(CACHE_DIR).join(DIR)
}

fn packs_of(game: &Path, prefix: &str) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(lua_root(game)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    names.sort();
    names
}

#[test]
fn a_mod_publishes_exactly_its_content_and_the_manifest() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let target = publish(game.path(), &open(&dir)).unwrap().unwrap();
    assert_eq!(
        walk(&target),
        [
            "assets/",
            "assets/alpha/",
            "assets/alpha/lang/",
            "assets/alpha/lang/en_us.json",
            "assets/alpha/textures/",
            "assets/alpha/textures/item/",
            "assets/alpha/textures/item/x.png",
            "data/",
            "data/alpha/",
            "data/alpha/recipe/",
            "data/alpha/recipe/r.json",
            "horizon-mod.toml",
        ]
    );
    // `lang/` moves to the mod's own namespace; the bytes are the package's.
    assert_eq!(
        std::fs::read(target.join("assets/alpha/lang/en_us.json")).unwrap(),
        b"{\"item.alpha.x\": \"X\"}"
    );
    assert_eq!(
        std::fs::read(target.join("assets/alpha/textures/item/x.png")).unwrap(),
        b"texture bytes"
    );
    assert_eq!(
        std::fs::read(target.join("data/alpha/recipe/r.json")).unwrap(),
        b"{\"type\": \"crafting\"}"
    );
    assert_eq!(
        std::fs::read_to_string(target.join(MANIFEST)).unwrap(),
        manifest_text("lua_alpha", "alpha", "Alpha", "1.0.0", "client")
    );
    let name = target.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("lua_alpha-"), "{name}");
    assert_eq!(name.len(), "lua_alpha-".len() + 16);
    // The game reads it as a content pack.
    let read = horizon_mod_package::Package::read(&target).unwrap();
    assert_eq!(read.manifest.id, "lua_alpha");
}

#[test]
fn the_manifest_text_is_pinned() {
    assert_eq!(
        manifest_text("lua_alpha", "alpha", "Alpha", "1.0.0", "client"),
        "api = \"0.2\"\ncapabilities = [\"registries\"]\nid = \"lua_alpha\"\nname = \"Alpha\"\n\
         namespaces = [\"alpha\"]\nside = \"client\"\nversion = \"1.0.0\"\n"
    );
}

#[test]
fn the_manifest_has_only_the_content_keys() {
    let text = manifest_text("lua_alpha", "alpha", "Alpha", "1.0.0", "both");
    let table: toml::Table = text.parse().unwrap();
    let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "api",
            "capabilities",
            "id",
            "name",
            "namespaces",
            "side",
            "version"
        ]
    );
    assert!(!text.contains("native"));
    assert!(!text.contains("authors"));
}

#[test]
fn a_folder_and_an_archive_with_the_same_content_make_the_same_pack() {
    let work = tempfile::tempdir().unwrap();
    let game_a = tempfile::tempdir().unwrap();
    let game_b = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let archive = archive_mod(work.path(), LUA_MANIFEST, &sample());
    let a = publish(game_a.path(), &open(&dir)).unwrap().unwrap();
    let b = publish(game_b.path(), &Package::open_archive(&archive).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(a.file_name(), b.file_name());
    assert_eq!(walk(&a), walk(&b));
}

#[test]
fn a_changed_file_makes_another_pack() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let first = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let a = publish(game.path(), &open(&first)).unwrap().unwrap();
    let mut changed = sample();
    changed[0].1 = b"other";
    let second = folder_mod(&work.path().join("again"), LUA_MANIFEST, &changed);
    let b = publish(game.path(), &open(&second)).unwrap().unwrap();
    assert_ne!(a.file_name(), b.file_name());
}

#[test]
fn a_second_publish_reuses_the_pack_and_does_not_rewrite_it() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let target = publish(game.path(), &open(&dir)).unwrap().unwrap();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    std::fs::File::options()
        .write(true)
        .open(target.join(MANIFEST))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let again = publish(game.path(), &open(&dir)).unwrap().unwrap();
    assert_eq!(again, target);
    let modified = std::fs::metadata(target.join(MANIFEST))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(modified, old, "the pack was written again");
}

#[test]
fn a_pack_that_is_not_what_this_run_writes_is_refused_and_left_alone() {
    let cases: Vec<(&str, Box<dyn Fn(&Path)>)> = vec![
        (
            "an extra file",
            Box::new(|t: &Path| std::fs::write(t.join("extra.txt"), b"x").unwrap()),
        ),
        (
            "a changed byte",
            Box::new(|t: &Path| {
                std::fs::write(
                    t.join("data/alpha/recipe/r.json"),
                    b"{\"type\": \"crafting\"}!",
                )
                .unwrap()
            }),
        ),
        (
            "a changed byte of the same length",
            Box::new(|t: &Path| {
                std::fs::write(
                    t.join("data/alpha/recipe/r.json"),
                    b"{\"type\": \"crafting\"X",
                )
                .unwrap()
            }),
        ),
        (
            "a missing file",
            Box::new(|t: &Path| {
                std::fs::remove_file(t.join("assets/alpha/lang/en_us.json")).unwrap()
            }),
        ),
    ];
    for (what, damage) in cases {
        let work = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
        let target = publish(game.path(), &open(&dir)).unwrap().unwrap();
        damage(&target);
        let before = walk(&target);
        let error = publish(game.path(), &open(&dir)).unwrap_err();
        assert!(
            matches!(error, AssetError::Corrupt { .. }),
            "{what}: {error:?}"
        );
        assert_eq!(walk(&target), before, "{what}: the pack was changed");
        assert!(!error.reason().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_a_pack_is_refused_and_its_target_is_untouched() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"keep").unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let target = publish(game.path(), &open(&dir)).unwrap().unwrap();
    let link = target.join("assets/alpha/linked");
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    let error = publish(game.path(), &open(&dir)).unwrap_err();
    assert!(matches!(error, AssetError::Corrupt { .. }), "{error:?}");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read(outside.path().join("secret")).unwrap(),
        b"keep"
    );
}

#[test]
fn a_mod_without_content_publishes_nothing() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &[]);
    assert_eq!(publish(game.path(), &open(&dir)).unwrap(), None);
    assert!(!game.path().join(CACHE_DIR).exists());
}

#[test]
fn a_failure_in_the_middle_leaves_no_folder_and_no_pack() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let error = publish_with(game.path(), &open(&dir), Some(1)).unwrap_err();
    assert!(matches!(error, AssetError::Io(_)), "{error:?}");
    assert!(
        packs_of(game.path(), "").is_empty(),
        "{:?}",
        packs_of(game.path(), "")
    );
    // The next publication works from the same package.
    assert!(publish(game.path(), &open(&dir)).unwrap().is_some());
}

#[test]
fn a_file_that_cannot_be_read_publishes_nothing() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let package = open(&dir);
    std::fs::remove_file(dir.join("data/alpha/recipe/r.json")).unwrap();
    let error = publish(game.path(), &package).unwrap_err();
    assert!(matches!(error, AssetError::Package(_)), "{error:?}");
    assert!(packs_of(game.path(), "").is_empty());
}

#[test]
fn a_name_the_reader_refuses_never_reaches_the_disk() {
    let work = tempfile::tempdir().unwrap();
    for bad in [
        "assets/alpha/../x.png",
        "/assets/alpha/x.png",
        "assets\\alpha\\x.png",
        "assets/alpha/C:x.png",
        "assets/alpha/a\u{0}b.png",
    ] {
        let path = archive_mod(work.path(), LUA_MANIFEST, &[(bad, b"x")]);
        assert!(Package::open_archive(&path).is_err(), "{bad:?} was read");
    }
}

#[test]
fn the_writer_refuses_names_that_leave_the_pack_or_are_not_plain() {
    for bad in [
        "../x",
        "/x",
        "a/../b",
        "a\\b",
        "a:b",
        "a\u{0}b",
        "a/./b",
        "a//b",
        "",
        "a/b.",
        "a/b ",
        "assets/alpha/CON.png",
        "assets/alpha/nul",
        "assets/alpha/Com1.txt",
        "assets/alpha/LPT9",
    ] {
        assert!(check_target(bad).is_err(), "{bad:?} was accepted");
    }
    for good in [
        "assets/alpha/x.png",
        "assets/alpha/console.png",
        "assets/alpha/con_x.png",
        "data/alpha/recipe/a-b_c.d.json",
    ] {
        assert_eq!(check_target(good), Ok(()), "{good:?}");
    }
}

#[test]
fn a_reserved_windows_name_in_a_pack_is_refused_at_publication() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(
        work.path(),
        LUA_MANIFEST,
        &[("assets/alpha/textures/CON.png", b"x")],
    );
    let error = publish(game.path(), &open(&dir)).unwrap_err();
    assert!(matches!(error, AssetError::Name { .. }), "{error:?}");
    assert!(packs_of(game.path(), "").is_empty());
}

#[test]
fn the_writer_keeps_every_file_under_the_pack() {
    let root = tempfile::tempdir().unwrap();
    for bad in ["../escape", "/abs", "a/../../escape"] {
        assert!(matches!(
            assets::contained(root.path(), bad),
            Err(AssetError::Name { .. })
        ));
    }
    assert!(assets::contained(root.path(), "assets/alpha/x.png").is_ok());
}

#[cfg(unix)]
#[test]
fn a_symlinked_cache_folder_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());

    let game = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), game.path().join(CACHE_DIR)).unwrap();
    let error = publish(game.path(), &open(&dir)).unwrap_err();
    assert!(matches!(error, AssetError::Cache { .. }), "{error:?}");
    assert!(walk(elsewhere.path()).is_empty());

    let game = tempfile::tempdir().unwrap();
    std::fs::create_dir(game.path().join(CACHE_DIR)).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), lua_root(game.path())).unwrap();
    let error = publish(game.path(), &open(&dir)).unwrap_err();
    assert!(matches!(error, AssetError::Cache { .. }), "{error:?}");
    assert!(walk(elsewhere.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn stale_packs_of_the_same_mod_go_within_the_limit_and_nothing_else() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let root = lua_root(game.path());
    std::fs::create_dir_all(&root).unwrap();
    // Ten stale packs of this mod, and the ones that must stay.
    for i in 1..=10 {
        let stale = root.join(format!("lua_alpha-{i:016x}"));
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join(MANIFEST), b"old").unwrap();
    }
    for keep in [
        "lua_beta-0123456789abcdef",
        "lua_alpha-zz",
        "lua_alpha-0123456789ABCDEF",
    ] {
        std::fs::create_dir(root.join(keep)).unwrap();
    }
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("precious"), b"keep").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("lua_alpha-aaaaaaaaaaaaaaaa")).unwrap();

    let target = publish(game.path(), &open(&dir)).unwrap().unwrap();
    assert!(target.is_dir());
    let current = target.file_name().unwrap().to_string_lossy().into_owned();
    let stale_left = packs_of(game.path(), "lua_alpha-")
        .into_iter()
        .filter(|n| *n != current)
        .filter(|n| std::fs::symlink_metadata(root.join(n)).is_ok_and(|m| m.is_dir()))
        .filter(|n| {
            let hash = &n["lua_alpha-".len()..];
            hash.len() == 16
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        .count();
    // Ten stale packs: eight removed, two left for the next run.
    assert_eq!(stale_left, 10 - MAX_REMOVALS);
    for keep in [
        "lua_beta-0123456789abcdef",
        "lua_alpha-zz",
        "lua_alpha-0123456789ABCDEF",
    ] {
        assert!(root.join(keep).is_dir(), "{keep} was removed");
    }
    assert!(
        std::fs::symlink_metadata(root.join("lua_alpha-aaaaaaaaaaaaaaaa"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read(outside.path().join("precious")).unwrap(),
        b"keep"
    );
}

#[test]
fn a_pack_the_fence_refuses_is_not_published() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    // An upper-case file name is not a content path (`horizon_mod_package::content::check_path`).
    let dir = folder_mod(
        work.path(),
        LUA_MANIFEST,
        &[("assets/alpha/textures/Bad.png", b"x")],
    );
    let error = publish(game.path(), &open(&dir)).unwrap_err();
    assert!(matches!(error, AssetError::Fence(_)), "{error:?}");
    assert!(packs_of(game.path(), "").is_empty());
}

#[test]
fn a_server_mod_with_registry_content_is_refused_and_a_both_mod_keeps_it() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let registry: &[(&str, &[u8])] = &[("data/alpha/horizon/mob_effect/e.json", b"{}")];
    let server = folder_mod(
        &work.path().join("server"),
        "id = \"alpha\"\nname = \"Alpha\"\nversion = \"1.0.0\"\nside = \"server\"\n",
        registry,
    );
    assert!(matches!(
        publish(game.path(), &open(&server)),
        Err(AssetError::Fence(_))
    ));
    assert!(packs_of(game.path(), "").is_empty());

    let plan = assets::plan(&open(&folder_mod(
        &work.path().join("both"),
        LUA_MANIFEST,
        registry,
    )))
    .unwrap()
    .unwrap();
    assert!(
        plan.manifest.contains("side = \"both\""),
        "{}",
        plan.manifest
    );
}

#[test]
fn the_pack_side_follows_the_mod_and_its_registry_content() {
    assert_eq!(pack_side(Side::Both, false), "client");
    assert_eq!(pack_side(Side::Both, true), "both");
    assert_eq!(pack_side(Side::Client, false), "client");
    assert_eq!(pack_side(Side::Client, true), "client");
    assert_eq!(pack_side(Side::Server, false), "server");
    assert_eq!(pack_side(Side::Server, true), "server");
}

#[test]
fn a_lang_file_and_an_asset_with_the_same_name_collide() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(
        work.path(),
        LUA_MANIFEST,
        &[
            ("lang/en_us.json", b"{}"),
            ("assets/alpha/lang/en_us.json", b"{}"),
        ],
    );
    let error = publish(game.path(), &open(&dir)).unwrap_err();
    assert!(matches!(error, AssetError::Collision { .. }), "{error:?}");
}

#[test]
fn a_long_mod_id_has_no_pack_id() {
    assert_eq!(pack_id(&"a".repeat(60)).map(|p| p.len()), Some(64));
    assert_eq!(pack_id(&"a".repeat(61)), None);
}

#[test]
fn the_pack_name_depends_on_the_content_and_the_manifest() {
    let work = tempfile::tempdir().unwrap();
    let a = folder_mod(&work.path().join("a"), LUA_MANIFEST, &sample());
    let renamed = LUA_MANIFEST.replace("Alpha", "Other");
    let b = folder_mod(&work.path().join("b"), &renamed, &sample());
    let pa = assets::plan(&open(&a)).unwrap().unwrap();
    let pb = assets::plan(&open(&b)).unwrap().unwrap();
    assert_ne!(pa.target_name, pb.target_name);
    let again = assets::plan(&open(&a)).unwrap().unwrap();
    assert_eq!(pa, again);
}

#[test]
fn the_key_has_an_english_entry_with_its_argument() {
    let text = include_str!("../../horizon-client/assets/horizon/lang/en_us.json");
    let lang: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text).unwrap();
    let english = lang
        .get(KEY)
        .and_then(|v| v.as_str())
        .expect("no en_us entry");
    assert!(english.contains("%1$s"), "{english}");
}

/// The folder name of a pack's target for `dir`, as `publish` would create it.
fn target_name_of(dir: &Path) -> String {
    assets::plan(&open(dir)).unwrap().unwrap().target_name
}

/// Makes a folder of the cache with one file in it, last changed `age` ago.
fn old_folder(root: &Path, name: &str, age: Duration) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("left-over"), b"partial").unwrap();
    let at = SystemTime::now() - age;
    std::fs::File::open(&dir).unwrap().set_modified(at).unwrap();
    dir
}

const AN_HOUR: Duration = Duration::from_secs(60 * 60);
const A_MINUTE: Duration = Duration::from_secs(60);

#[test]
fn an_old_temp_folder_of_this_pack_is_removed_by_a_publish() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let root = lua_root(game.path());
    std::fs::create_dir_all(&root).unwrap();
    let temp = old_folder(
        &root,
        &format!(".tmp-{}-4242-1700000000000000000-7", target_name_of(&dir)),
        AN_HOUR,
    );
    assert!(publish(game.path(), &open(&dir)).unwrap().is_some());
    assert!(!temp.exists(), "the old temp folder stayed");
}

#[test]
fn a_young_temp_folder_stays_because_another_game_may_be_extracting() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let root = lua_root(game.path());
    std::fs::create_dir_all(&root).unwrap();
    let temp = old_folder(
        &root,
        &format!(".tmp-{}-4242-1700000000000000000-8", target_name_of(&dir)),
        A_MINUTE,
    );
    assert!(publish(game.path(), &open(&dir)).unwrap().is_some());
    assert!(temp.is_dir(), "a young temp folder was removed");
}

#[test]
fn a_folder_that_does_not_match_the_temp_format_stays() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let root = lua_root(game.path());
    std::fs::create_dir_all(&root).unwrap();
    let target = target_name_of(&dir);
    let other_mod_hash = "0123456789abcdef";
    let kept = [
        // Too few numbers, an upper-case hash, a short or non-hex hash, another mod's pack,
        // no leading dot, a non-number part, and a name that is only a prefix.
        format!(".tmp-{target}-4242-1700000000000000000"),
        ".tmp-lua_alpha-ABCDEF0123456789-1-2-3".to_string(),
        ".tmp-lua_alpha-zz-1-2-3".to_string(),
        format!(".tmp-lua_beta-{other_mod_hash}-1-2-3"),
        format!("tmp-{target}-1-2-3"),
        format!(".tmp-{target}-1-2-x"),
        format!(".tmp-{target}-1-2-3-4"),
        format!(".tmp-{target}"),
    ];
    let mut dirs = Vec::new();
    for name in &kept {
        dirs.push(old_folder(&root, name, AN_HOUR));
    }
    assert!(publish(game.path(), &open(&dir)).unwrap().is_some());
    for (name, dir) in kept.iter().zip(&dirs) {
        assert!(dir.is_dir(), "{name} was removed");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_named_like_a_temp_folder_is_neither_followed_nor_removed() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let root = lua_root(game.path());
    std::fs::create_dir_all(&root).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("precious"), b"keep").unwrap();
    let at = SystemTime::now() - AN_HOUR;
    std::fs::File::open(outside.path())
        .unwrap()
        .set_modified(at)
        .unwrap();
    let link = root.join(format!(".tmp-{}-4242-1-2", target_name_of(&dir)));
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    assert!(publish(game.path(), &open(&dir)).unwrap().is_some());
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the symlink was removed"
    );
    assert_eq!(
        std::fs::read(outside.path().join("precious")).unwrap(),
        b"keep"
    );
}

#[test]
fn at_most_eight_temp_folders_are_removed_per_publish() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let root = lua_root(game.path());
    std::fs::create_dir_all(&root).unwrap();
    let target = target_name_of(&dir);
    for seq in 0..10 {
        old_folder(&root, &format!(".tmp-{target}-4242-1-{seq}"), AN_HOUR);
    }
    assert!(publish(game.path(), &open(&dir)).unwrap().is_some());
    let left = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .count();
    assert_eq!(left, 10 - assets::MAX_TEMP_REMOVALS);
}

#[test]
fn a_leftover_with_the_same_pid_as_the_old_format_does_not_block_publishing() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let root = lua_root(game.path());
    std::fs::create_dir_all(&root).unwrap();
    // The name an earlier version wrote: `.tmp-<pack>-<hash>-<pid>`, with this process's pid.
    let leftover = format!(".tmp-{}-{}", target_name_of(&dir), std::process::id());
    std::fs::create_dir(root.join(&leftover)).unwrap();
    let published = publish(game.path(), &open(&dir)).unwrap().unwrap();
    assert!(published.is_dir());
    assert!(
        packs_of(game.path(), "")
            .iter()
            .any(|n| n == &target_name_of(&dir))
    );
}

#[test]
fn an_interrupted_extraction_leaves_no_temp_folder_behind() {
    let work = tempfile::tempdir().unwrap();
    let game = tempfile::tempdir().unwrap();
    let dir = folder_mod(work.path(), LUA_MANIFEST, &sample());
    let error = publish_with(game.path(), &open(&dir), Some(2)).unwrap_err();
    assert!(matches!(error, AssetError::Io(_)), "{error:?}");
    assert!(packs_of(game.path(), ".tmp-").is_empty());
    assert!(packs_of(game.path(), "lua_alpha").is_empty());
}
