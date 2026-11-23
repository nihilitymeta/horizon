//! The binary, run as its own process for each case: the load touches process-wide state, so one
//! check per process is the honest shape.

use std::path::{Path, PathBuf};
use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_event-horizon-lua-check"))
        .args(args)
        .output()
        .expect("the checker runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A mod folder with a `lua-mod.toml` and one script.
fn mod_folder(root: &Path, name: &str, id: &str, script: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("lua-mod.toml"),
        format!("id = \"{id}\"\nversion = \"1.0.0\"\nside = \"both\"\n"),
    )
    .unwrap();
    std::fs::write(dir.join("main.luau"), script).unwrap();
    dir
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 test path")
}

const GOOD: &str = "horizon.on(\"server_tick\", function() end)\n";

#[test]
fn a_good_mod_loads_and_exits_zero() {
    let root = tempfile::tempdir().unwrap();
    let dir = mod_folder(root.path(), "good", "good_mod", GOOD);
    let (code, stdout, stderr) = run(&["check", path_str(&dir)]);
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("load: loaded"), "{stdout}");
    assert!(!stdout.contains("undefined_variable"), "{stdout}");
    assert!(stdout.contains("result: ok"), "{stdout}");
}

#[test]
fn a_syntax_error_fails_and_names_the_file_and_line() {
    let root = tempfile::tempdir().unwrap();
    let dir = mod_folder(
        root.path(),
        "syntax",
        "syntax_mod",
        "local x = 1\nlocal = 2\n",
    );
    let (code, stdout, _) = run(&["check", path_str(&dir)]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("main.luau:2:"), "{stdout}");
    assert!(stdout.contains("syntax error"), "{stdout}");
}

#[test]
fn a_refused_manifest_fails_and_prints_its_key() {
    let root = tempfile::tempdir().unwrap();
    let dir = mod_folder(root.path(), "bad", "Bad Id", GOOD);
    let (code, stdout, _) = run(&["check", path_str(&dir)]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("load: error: refused:"), "{stdout}");
    assert!(stdout.contains('('), "the refusal key is printed: {stdout}");
}

#[test]
fn an_unused_local_warns_and_fails_only_under_strict() {
    let root = tempfile::tempdir().unwrap();
    let dir = mod_folder(
        root.path(),
        "unused",
        "unused_mod",
        "local ok = 1\nlocal unused = 2\nprint(ok)\n",
    );
    let (code, stdout, _) = run(&["check", path_str(&dir)]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("main.luau:2:7: warning"), "{stdout}");
    assert!(stdout.contains("unused_variable"), "{stdout}");
    let (strict, stdout, _) = run(&["check", path_str(&dir), "--strict"]);
    assert_eq!(strict, 1, "{stdout}");
}

#[test]
fn an_unformatted_script_would_reformat_and_is_left_on_disk() {
    let root = tempfile::tempdir().unwrap();
    let text = "local   x=1\nprint( x )\n";
    let dir = mod_folder(root.path(), "ugly", "ugly_mod", text);
    let (code, stdout, _) = run(&["check", path_str(&dir)]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("main.luau: warning: would reformat"),
        "{stdout}"
    );
    let (strict, _, _) = run(&["check", path_str(&dir), "--strict"]);
    assert_eq!(strict, 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("main.luau")).unwrap(),
        text
    );
}

#[test]
fn a_misspelt_api_member_fails_and_names_the_file_line_and_member() {
    let root = tempfile::tempdir().unwrap();
    let dir = mod_folder(
        root.path(),
        "misspelt",
        "misspelt_mod",
        "horizon.on_ticks(\"server_tick\", function() end)\n",
    );
    let (code, stdout, _) = run(&["check", path_str(&dir)]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("main.luau:1:1: error"), "{stdout}");
    assert!(stdout.contains("horizon.on_ticks"), "{stdout}");
}

#[test]
fn an_undefined_plain_global_fails() {
    let root = tempfile::tempdir().unwrap();
    let dir = mod_folder(root.path(), "undef", "undef_mod", "foo()\n");
    let (code, stdout, _) = run(&["check", path_str(&dir)]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("undefined_variable"), "{stdout}");
}

#[test]
fn list_prints_loaded_refused_and_disabled_mods() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    let lua_mods = game.join("lua-mods");
    std::fs::create_dir_all(&lua_mods).unwrap();
    mod_folder(&lua_mods, "good", "good_mod", GOOD);
    mod_folder(&lua_mods, "bad", "Bad Id", GOOD);
    mod_folder(&lua_mods, "off", "off_mod", GOOD);
    std::fs::write(lua_mods.join("enabled.toml"), "[mods]\noff_mod = false\n").unwrap();
    let (code, stdout, stderr) = run(&["--list", "--game-dir", path_str(&game)]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("good_mod 1.0.0 both loaded"), "{stdout}");
    assert!(stdout.contains("bad refused:"), "{stdout}");
    assert!(stdout.contains("off_mod 1.0.0 both disabled"), "{stdout}");
}

#[test]
fn emit_types_writes_both_files_and_refuses_a_changed_one_without_force() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("types");
    let (code, _, stderr) = run(&["emit-types", path_str(&out)]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let luaurc = std::fs::read_to_string(out.join(".luaurc")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&luaurc).expect("the .luaurc parses");
    assert_eq!(value["globals"][0], "horizon");
    assert!(out.join("horizon.d.luau").is_file());
    std::fs::write(out.join(".luaurc"), "{}\n").unwrap();
    let (refused, _, _) = run(&["emit-types", path_str(&out)]);
    assert_eq!(refused, 1);
    let (forced, _, _) = run(&["emit-types", path_str(&out), "--force"]);
    assert_eq!(forced, 0);
}

#[test]
fn usage_errors_and_unreadable_paths_exit_two() {
    assert_eq!(run(&[]).0, 2);
    assert_eq!(run(&["check"]).0, 2);
    assert_eq!(run(&["fly"]).0, 2);
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("nowhere");
    assert_eq!(run(&["check", path_str(&missing)]).0, 2);
    assert_eq!(run(&["--help"]).0, 0);
}

#[test]
fn list_prints_a_mod_the_host_refuses_after_discovery_as_refused() {
    // Discovery accepts this mod (its manifest and script read fine); the host refuses it when its
    // declaration pass runs `horizon.on` with a hook name it does not know, so the line comes from
    // the host's refusal, not from the scan.
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    let lua_mods = game.join("lua-mods");
    std::fs::create_dir_all(&lua_mods).unwrap();
    mod_folder(&lua_mods, "good", "good_mod", GOOD);
    mod_folder(
        &lua_mods,
        "host_bad",
        "host_bad_mod",
        "horizon.on(\"no_such_hook\", function() end)\n",
    );
    let (code, stdout, stderr) = run(&["--list", "--game-dir", path_str(&game)]);
    assert_eq!(code, 0, "stderr: {stderr}\nstdout: {stdout}");
    assert!(stdout.contains("good_mod 1.0.0 both loaded"), "{stdout}");
    let line = stdout
        .lines()
        .find(|line| line.starts_with("host_bad_mod 1.0.0 both refused: "))
        .unwrap_or_else(|| panic!("the host's refusal is listed: {stdout}"));
    assert!(
        line.contains("host_bad_mod: "),
        "the host's reason text: {line}"
    );
    assert!(!stdout.contains("(host) refused"), "{stdout}");
}
