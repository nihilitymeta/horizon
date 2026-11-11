//! `cargo horizon-mod`: make Horizon content mods (`docs/mods.md`): blocks,
//! items, data and assets, no code. Code mods are native (docs/native-mods.md).
//!
//! - `new <path> [--name "Name"]`: a content mod from the template; with
//!   `--template lua`, a scripted mod written with the modkit; with
//!   `--template native [--horizon <commit>]`, a native trait mod's bundle.
//! - `build`: the `.hmod` in `target/horizon-mod/`, with the mod's `assets/`
//!   and `data/`.
//! - `run [--game <horizon-client>] [-- <game args>]`: build, then play with
//!   the mod.
//! - `check <file> [--json]`: what the registry checks of a submission
//!   (`.hmod`, `.hnmod`, `.hpack`, resource pack `.zip`, `.hshader`). For a
//!   scripted mod folder, also the script steps in order (issue 53): the
//!   game's headless load, StyLua formatting, selene lint, luau-lsp types.
//! - `fmt [folder]`: rewrite the mod's scripts to StyLua's format.
//! - `registry build|review|check|list`: the registry repository's CI.
use horizon_mods::{Capability, Content, Manifest, Package, manifest::MANIFEST};
mod check;
mod registry;
mod scripts;

use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    // Cargo runs `cargo-horizon-mod horizon-mod <args>`.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "horizon-mod") {
        args.remove(0);
    }
    let result = match args.first().map(String::as_str) {
        Some("new") => new(&args[1..]),
        Some("build") => build().map(|p| {
            println!("Built {}", p.display());
        }),
        Some("run") => run(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("fmt") => fmt(&args[1..]),
        Some("registry") => registry::main(&args[1..]),
        _ => Err("usage: cargo horizon-mod new <path> [--name \"Name\"] [--template content|lua|native [--horizon <commit>]] | build | run [--game <horizon-client>] [-- <game args>] | check <file or folder> [--kind <kind>] [--key <public key> --signature <file.minisig>] [--game <horizon-client>] [--dev-mod <library>]... [--resources <26.3 folder>] [--no-scripts] [--offline] [--luau-lsp <binary>] [--modkit <dir>] [--tools-cache <dir>] [--json] | fmt [folder] | registry build|review|check|list".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The value after `flag` in `args`.
fn flag<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).map(String::as_str)
}

/// The `check` flags that take a value in the next argument.
const CHECK_VALUE_FLAGS: &[&str] = &[
    "--kind",
    "--key",
    "--signature",
    "--game",
    "--dev-mod",
    "--resources",
    "--luau-lsp",
    "--modkit",
    "--tools-cache",
];

/// The `check` switches (flags without a value).
const CHECK_SWITCHES: &[&str] = &["--json", "--no-scripts", "--offline"];

/// Checks `args` against a subcommand's flags: an unknown `--flag` is an
/// error naming the known ones, and a value flag given twice is an error
/// unless it may repeat. A value flag's value is skipped, whatever it is
/// (a missing one is `check_path`'s error).
fn validate_flags(
    args: &[String],
    values: &[&str],
    switches: &[&str],
    repeatable: &[&str],
) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if !arg.starts_with("--") {
            continue;
        }
        if values.contains(&arg) {
            if seen.contains(&arg) && !repeatable.contains(&arg) {
                return Err(format!("{arg} is given twice"));
            }
            seen.push(arg);
            i += 1;
        } else if !switches.contains(&arg) {
            let known: Vec<&str> = values.iter().chain(switches).copied().collect();
            return Err(format!(
                "unknown flag {arg}: this command takes {}",
                known.join(", ")
            ));
        }
    }
    Ok(())
}

/// The mod's path in `check`'s arguments: the first argument that is neither
/// a flag nor the value of a value-taking flag, so `check --json <mod>` and
/// `check <mod> --modkit <dir>` both name the mod. A value-taking flag with
/// no value (none, or another flag next) is an error.
fn check_path(args: &[String]) -> Result<Option<&str>, String> {
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if CHECK_VALUE_FLAGS.contains(&arg) {
            match args.get(i + 1) {
                Some(value) if !value.starts_with("--") => i += 2,
                _ => return Err(format!("{arg} needs a value")),
            }
        } else {
            if !arg.starts_with("--") && path.is_none() {
                path = Some(arg);
            }
            i += 1;
        }
    }
    Ok(path)
}

fn new(args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("cargo horizon-mod new <path>")?;
    let dir = Path::new(path);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let base = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("the path needs a folder name")?;
    let id: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let name = flag(args, "--name").map_or_else(|| base.replace(['-', '_'], " "), str::to_string);
    let fill = |t: &str| t.replace("{id}", &id).replace("{name}", &name);
    match flag(args, "--template") {
        None | Some("content") => {}
        Some("lua") => return new_lua(dir, path, &id, fill),
        Some("native") => return new_native(dir, &id, flag(args, "--horizon"), fill),
        Some(_) => return Err("--template is content, lua or native".into()),
    }
    // A block copying a vanilla one, with its model, name, loot, recipe and tag.
    let files = [
        (
            MANIFEST,
            include_str!("../template/content/horizon-mod.toml"),
        ),
        (
            "data/{id}/horizon/block/glowing_amethyst.json",
            include_str!("../template/content/block.json"),
        ),
        (
            "assets/{id}/blockstates/glowing_amethyst.json",
            include_str!("../template/content/blockstate.json"),
        ),
        (
            "assets/{id}/models/block/glowing_amethyst.json",
            include_str!("../template/content/model.json"),
        ),
        (
            "assets/{id}/items/glowing_amethyst.json",
            include_str!("../template/content/item.json"),
        ),
        (
            "assets/{id}/lang/en_us.json",
            include_str!("../template/content/lang.json"),
        ),
        (
            "data/{id}/loot_table/blocks/glowing_amethyst.json",
            include_str!("../template/content/loot.json"),
        ),
        (
            "data/{id}/recipe/glowing_amethyst.json",
            include_str!("../template/content/recipe.json"),
        ),
        (
            "data/minecraft/tags/block/mineable/pickaxe.json",
            include_str!("../template/content/pickaxe.json"),
        ),
        (".gitignore", include_str!("../template/gitignore")),
    ];
    for (file, text) in files {
        let file = fill(file);
        let file = file.as_str();
        let to = dir.join(file);
        std::fs::create_dir_all(to.parent().expect("a file in the mod"))
            .map_err(|e| e.to_string())?;
        std::fs::write(&to, fill(text)).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    let manifest =
        Manifest::parse(&std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| e.to_string())?)?;
    Content::read(dir, &manifest)?;
    println!(
        "Created {} (mod id {id}). Next: cd {path} && cargo horizon-mod run",
        dir.display()
    );
    Ok(())
}

/// A scripted mod in `dir`, written with the modkit (docs/lua-bindings.md): a
/// manifest with capabilities `registries` and `scripts` that depends on
/// `horizon_modkit`, a registry script with one item (a wand) and its look,
/// an init script with the wand's `on_use`, a `.luaurc` naming the modkit
/// checkout beside it for an editor, and a README that says how to run it.
fn new_lua(dir: &Path, path: &str, id: &str, fill: impl Fn(&str) -> String) -> Result<(), String> {
    let files = [
        (MANIFEST, include_str!("../template/lua/horizon-mod.toml")),
        (
            "scripts/registry.lua",
            include_str!("../template/lua/registry.lua"),
        ),
        (
            "scripts/server/init.lua",
            include_str!("../template/lua/server_init.lua"),
        ),
        (
            "scripts/client/init.lua",
            include_str!("../template/lua/client_init.lua"),
        ),
        (
            "scripts/shared/util.lua",
            include_str!("../template/lua/shared_util.lua"),
        ),
        (
            "assets/{id}/lang/en_us.json",
            include_str!("../template/lua/lang.json"),
        ),
        (".luaurc", include_str!("../template/lua/luaurc.json")),
        ("README.md", include_str!("../template/lua/README.md")),
        (".gitignore", include_str!("../template/gitignore")),
    ];
    for (file, text) in files {
        let file = fill(file);
        let to = dir.join(file.as_str());
        std::fs::create_dir_all(to.parent().expect("a file in the mod"))
            .map_err(|e| e.to_string())?;
        std::fs::write(&to, fill(text)).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    let manifest =
        Manifest::parse(&std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| e.to_string())?)?;
    Content::read(dir, &manifest)?;
    println!(
        "Created {} (mod id {id}). Next: cd {path} && cargo horizon-mod run -- --dev-mod /path/to/horizon-modkit",
        dir.display()
    );
    Ok(())
}

/// A native trait mod's bundle in `dir` (docs/native-mods.md, "Trait mods"): its
/// `horizon-mixin.toml` and an `[api]` crate with one hook. It is what the launcher
/// builds (`native-build <dir>`) and packs as `mixins/<id>/` of a `.hnmod`
/// (docs/api/mod-files.md). `horizon` pins the Horizon commit it is made for.
fn new_native(
    dir: &Path,
    id: &str,
    horizon: Option<&str>,
    fill: impl Fn(&str) -> String,
) -> Result<(), String> {
    // `[mod] id`, as `horizon-native.json` needs it: the template keeps to its characters.
    if id.len() > 64 {
        return Err(format!("mod id {id}: 64 characters at most"));
    }
    let (compatibility, rev) = match horizon {
        Some(commit) if horizon_mods::index::native::revision(commit) => (
            format!("[compatibility]\nhorizon_revision = \"{commit}\"\n"),
            format!(", rev = \"{commit}\""),
        ),
        Some(commit) => {
            return Err(format!(
                "--horizon {commit}: a Horizon commit is 40 lowercase hex digits"
            ));
        }
        // Unpinned, it builds on the instance's own Horizon.
        None => (
            "# The Horizon commit it is made for; unpinned, it builds on the instance's own.\n\
             # [compatibility]\n# horizon_revision = \"<40 hex digits>\"\n"
                .to_owned(),
            String::new(),
        ),
    };
    let files = [
        (
            "horizon-mixin.toml",
            include_str!("../template/native/horizon-mixin.toml"),
        ),
        (
            "api/Cargo.toml",
            include_str!("../template/native/api.toml"),
        ),
        ("api/src/lib.rs", include_str!("../template/native/lib.rs")),
        (".gitignore", include_str!("../template/gitignore")),
    ];
    for (file, text) in files {
        let to = dir.join(file);
        std::fs::create_dir_all(to.parent().expect("a file in the mod"))
            .map_err(|e| e.to_string())?;
        let text = fill(text)
            .replace("{compatibility}", &compatibility)
            .replace("{rev}", &rev);
        std::fs::write(&to, text).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    println!(
        "Created {} (native mod id {id}). Next: horizon-launcher native-build {} --instance <instance> --trust-native-code",
        dir.display(),
        dir.display()
    );
    Ok(())
}

/// Packs the content mod in the current folder into `target/horizon-mod/<id>.hmod`: the
/// manifest and the files in `assets/` and `data/`.
fn build() -> Result<PathBuf, String> {
    build_in(Path::new("."))
}

fn build_in(root: &Path) -> Result<PathBuf, String> {
    let manifest_text = std::fs::read_to_string(root.join(MANIFEST))
        .map_err(|e| format!("{MANIFEST}: {e} (run this in a mod's folder)"))?;
    let manifest = Manifest::parse(&manifest_text)?;
    if root.join("Cargo.toml").exists() {
        return Err("a Cargo.toml: code mods are native now (docs/native-mods.md)".into());
    }
    if !manifest
        .capabilities
        .iter()
        .any(|c| matches!(c, Capability::Registries | Capability::Scripts))
    {
        return Err(format!(
            "a content mod has capability registries (or scripts) in {MANIFEST}"
        ));
    }
    // The same checks as the game's, and the files in the order it lists them.
    let content = Content::read(root, &manifest)?;
    let mut reader = content.reader()?;
    let mut files = Vec::with_capacity(content.files.len() + content.scripts.len());
    // The registry's files are not packed: the game makes them again from
    // scripts/registry.lua when it reads the mod (and refuses a mod that
    // ships them as files too).
    for name in content
        .files
        .iter()
        .filter(|name| !content.is_registered(name))
        .chain(&content.scripts)
    {
        files.push((name.as_str(), reader.read(name)?));
    }
    let files: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let dir = root.join("target/horizon-mod");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.hmod", manifest.id));
    // Write then rename, so a running game never reads half a file.
    let partial = path.with_extension("hmod.partial");
    let bytes = Package::write_files(&manifest_text, None, &files)?;
    std::fs::write(&partial, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// `check <file>`: the registry's checks of a submission, printed; fails
/// when one does. With `--json`, one JSON object is printed once, at the end,
/// with the real overall result and each script step's pass/fail.
fn check(args: &[String]) -> Result<(), String> {
    let want_json = args.iter().any(|a| a == "--json");
    let (value, result) = check_inner(args, !want_json);
    if want_json {
        println!("{value}");
    }
    result
}

/// The check itself: `verbose` prints the human-readable text as it goes (off
/// with `--json`, where the returned value is the whole output). The value
/// always carries the overall result: `ok`, the registry `report` when its
/// checks passed, every script step's pass/fail in `steps`, and `error` when
/// the check failed.
fn check_inner(args: &[String], verbose: bool) -> (serde_json::Value, Result<(), String>) {
    if let Err(e) = validate_flags(args, CHECK_VALUE_FLAGS, CHECK_SWITCHES, &["--dev-mod"]) {
        return (check_value(None, &[], Some(&e), false), Err(e));
    }
    let file = match check_path(args) {
        Ok(Some(file)) => file.to_string(),
        Ok(None) => {
            let e = "cargo horizon-mod check <file or folder>".to_string();
            return (check_value(None, &[], Some(&e), false), Err(e));
        }
        Err(e) => return (check_value(None, &[], Some(&e), false), Err(e)),
    };
    // `--modkit` names the checkout the types step reads: one that is not
    // there is an error, not a silent downgrade (`$MODKIT`, an ambient
    // setting, still falls back to the notice).
    if let Some(dir) = flag(args, "--modkit")
        .map(Path::new)
        .filter(|d| !d.is_dir())
    {
        let e = format!(
            "--modkit {}: not a directory (no modkit checkout there)",
            dir.display()
        );
        return (check_value(None, &[], Some(&e), false), Err(e));
    }
    if let Err(e) = modkit_env_check_with(args, |name| std::env::var_os(name)) {
        return (check_value(None, &[], Some(&e), false), Err(e));
    }
    let path = Path::new(&file);
    // A mod folder: built to its target/horizon-mod/<id>.hmod for the
    // registry's checks, the folder itself for the scripts.
    let built;
    let (path, folder) = if path.is_dir() {
        match build_in(path) {
            Ok(hmod) => {
                built = hmod;
                (built.as_path(), Some(path))
            }
            Err(e) => return (check_value(None, &[], Some(&e), false), Err(e)),
        }
    } else {
        (path, None)
    };
    let kind = match flag(args, "--kind") {
        Some(k) => horizon_mods::index::Kind::from(k.to_string()),
        None if folder.is_some() => horizon_mods::index::Kind::Mod,
        None => match check::kind_of(path) {
            Some(k) => k,
            None => {
                let e =
                    "pass --kind mod|modpack|resourcepack: the extension doesn't say".to_string();
                return (check_value(None, &[], Some(&e), false), Err(e));
            }
        },
    };
    let signature = match flag(args, "--signature")
        .map(|p| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}")))
        .transpose()
    {
        Ok(s) => s,
        Err(e) => return (check_value(None, &[], Some(&e), false), Err(e)),
    };
    let report = match check::check(path, kind, flag(args, "--key"), signature.as_deref()) {
        Ok(r) => r,
        Err(e) => return (check_value(None, &[], Some(&e), false), Err(e)),
    };
    if verbose {
        print!("{}", report.text());
    }
    let report_json = Some(report.json());
    // A scripted mod's scripts: loaded headlessly by the game (`horizon-client
    // --check-mod`), with the libraries it depends on from --dev-mod; with
    // --resources, any mod's content is also layered on the vanilla folder as
    // the game does at start, so a file the game would refuse fails here.
    let resources = flag(args, "--resources");
    let scripted = report
        .facts
        .capabilities
        .contains(&horizon_mods::Capability::Scripts)
        && !args.iter().any(|a| a == "--no-scripts");
    if scripted || resources.is_some() {
        let game = match flag(args, "--game")
            .map(str::to_string)
            .or_else(|| std::env::var("HORIZON_CLIENT").ok())
        {
            Some(g) => g,
            None => {
                let e = "the mod has scripts or --resources was given: pass --game <path to horizon-client> (or set HORIZON_CLIENT) to load them headlessly, or --no-scripts".to_string();
                return (check_value(report_json, &[], Some(&e), false), Err(e));
            }
        };
        let target = folder.unwrap_or(path);
        let mut cmd = Command::new(&game);
        cmd.arg("--check-mod").arg(target);
        // With --json, stdout is the one JSON value: the game's output goes
        // to stderr.
        if !verbose {
            cmd.stdout(std::process::Stdio::from(std::io::stderr()));
        }
        for w in args.windows(2) {
            if w[0] == "--dev-mod" {
                cmd.arg("--dev-mod").arg(&w[1]);
            }
        }
        if let Some(r) = resources {
            cmd.arg("--resources").arg(r);
        }
        match cmd.status() {
            Ok(status) if status.success() => {}
            Ok(_) => {
                let e = "the game refused the mod (above)".to_string();
                return (check_value(report_json, &[], Some(&e), false), Err(e));
            }
            Err(e) => {
                let e = format!("{game}: {e}");
                return (check_value(report_json, &[], Some(&e), false), Err(e));
            }
        }
    }
    // The script steps of a mod folder, each reported on its own (issue 53):
    // StyLua format, selene lint, luau-lsp types. `--no-scripts` above skips
    // only the game's load; these three need nothing on the PATH (luau-lsp
    // comes from the user cache, or is skipped with a notice).
    let mut steps = Vec::new();
    if let Some(folder) = folder {
        let manifest = match std::fs::read_to_string(folder.join(MANIFEST)) {
            Ok(text) => match Manifest::parse(&text) {
                Ok(m) => m,
                Err(e) => {
                    let e = e.to_string();
                    return (check_value(report_json, &[], Some(&e), false), Err(e));
                }
            },
            Err(e) => {
                let e = e.to_string();
                return (check_value(report_json, &[], Some(&e), false), Err(e));
            }
        };
        let content = match Content::read(folder, &manifest) {
            Ok(c) => c,
            Err(e) => {
                let e = e.to_string();
                return (check_value(report_json, &[], Some(&e), false), Err(e));
            }
        };
        if !content.scripts.is_empty() {
            // Each side the runtime runs is analysed as it resolves requires;
            // a mod with neither folder keeps the single analysis as it was.
            let mut sides = Vec::new();
            if content.has_server() {
                sides.push(scripts::Side::Server);
            }
            if content.has_client() {
                sides.push(scripts::Side::Client);
            }
            let cache = match scripts::tools_cache(flag(args, "--tools-cache").map(Path::new)) {
                Ok(c) => c,
                Err(e) => return (check_value(report_json, &[], Some(&e), false), Err(e)),
            };
            match script_steps(
                folder,
                &content.scripts,
                &sides,
                &cache,
                offline(args),
                modkit_dir(args).as_deref(),
                flag(args, "--luau-lsp").map(Path::new),
            ) {
                Ok(run) => {
                    for step in &run {
                        if verbose {
                            for line in &step.lines {
                                println!("{line}");
                            }
                        }
                        // Notices are human output on stderr, in every mode.
                        for note in &step.notes {
                            eprintln!("{note}");
                        }
                    }
                    steps = run;
                }
                Err(e) => return (check_value(report_json, &[], Some(&e), false), Err(e)),
            }
            if steps.iter().any(|s| !s.ok) {
                let e = "a script check failed (above)".to_string();
                return (
                    check_value(report_json, &step_values(&steps), Some(&e), false),
                    Err(e),
                );
            }
        }
    }
    (
        check_value(report_json, &step_values(&steps), None, true),
        Ok(()),
    )
}

/// One script step's result, reported as text lines and as `--json` (which
/// carries the step's name). `notes` are notices for stderr, in every mode.
struct ScriptStep {
    ok: bool,
    lines: Vec<String>,
    notes: Vec<String>,
    json: serde_json::Value,
}

/// The script steps over a mod folder's scripts: StyLua format, selene lint,
/// luau-lsp types.
fn script_steps(
    folder: &Path,
    scripts: &[String],
    sides: &[scripts::Side],
    cache: &Path,
    offline: bool,
    modkit: Option<&Path>,
    explicit: Option<&Path>,
) -> Result<Vec<ScriptStep>, String> {
    let mut steps = Vec::new();
    let bad = scripts::unformatted(folder, scripts)?;
    steps.push(ScriptStep {
        ok: bad.is_empty(),
        lines: if bad.is_empty() {
            vec!["scripts format: ok".to_string()]
        } else {
            let mut lines = vec![format!(
                "scripts format: needs formatting ({}): run cargo horizon-mod fmt",
                bad.len()
            )];
            lines.extend(bad.iter().map(|name| format!("  {name}")));
            lines
        },
        notes: Vec::new(),
        json: serde_json::json!({"name": "format", "ok": bad.is_empty(), "bad": bad}),
    });
    let found = scripts::lints(folder, scripts)?;
    let errors: Vec<&String> = found.iter().filter(|l| l.error).map(|l| &l.text).collect();
    let warnings: Vec<&String> = found.iter().filter(|l| !l.error).map(|l| &l.text).collect();
    steps.push(ScriptStep {
        ok: errors.is_empty(),
        lines: if errors.is_empty() {
            vec![format!(
                "scripts lint: ok{}",
                warnings.iter().map(|w| format!("\n  {w}")).collect::<String>()
            )]
        } else {
            let mut lines = vec![format!("scripts lint: {} error(s)", errors.len())];
            lines.extend(found.iter().map(|lint| format!("  {}", lint.text)));
            lines
        },
        notes: Vec::new(),
        json: serde_json::json!({"name": "lint", "ok": errors.is_empty(), "errors": errors, "warnings": warnings}),
    });
    // A pass with part unchecked (no modkit: its `@modkit` requires) keeps
    // status `pass`, and its detail carries the notice for a parser.
    let (ok, status, detail, lines, notes) =
        match scripts::analyze(folder, scripts, sides, explicit, cache, offline, modkit)? {
            scripts::Outcome::Pass => (
                true,
                "pass",
                String::new(),
                vec!["scripts types: ok".to_string()],
                Vec::new(),
            ),
            scripts::Outcome::Unchecked(why) => (
                true,
                "pass",
                why.clone(),
                vec!["scripts types: ok".to_string()],
                vec![format!("scripts types: {why}")],
            ),
            scripts::Outcome::Skip(why) => (
                true,
                "skipped",
                why.clone(),
                Vec::new(),
                vec![format!("scripts types: skipped ({why})")],
            ),
            scripts::Outcome::Fail(why) => (
                false,
                "failed",
                why.clone(),
                vec![format!("scripts types: failed\n{why}")],
                Vec::new(),
            ),
        };
    steps.push(ScriptStep {
        ok,
        lines,
        notes,
        json: serde_json::json!({"name": "types", "ok": ok, "status": status, "detail": detail}),
    });
    Ok(steps)
}

fn step_values(steps: &[ScriptStep]) -> Vec<serde_json::Value> {
    steps.iter().map(|s| s.json.clone()).collect()
}

/// The single `--json` value: the overall result, the registry report when
/// its checks passed, every script step's pass/fail, and the error when the
/// check failed.
fn check_value(
    report: Option<serde_json::Value>,
    steps: &[serde_json::Value],
    error: Option<&str>,
    ok: bool,
) -> serde_json::Value {
    let mut value = serde_json::json!({"ok": ok});
    if let Some(r) = report {
        value["report"] = r;
    }
    if !steps.is_empty() {
        value["steps"] = serde_json::Value::Array(steps.to_vec());
    }
    if let Some(e) = error {
        value["error"] = serde_json::Value::String(e.to_string());
    }
    value
}

/// The modkit checkout for the types step: `--modkit <dir>` wins over the
/// `$MODKIT` environment (docs/lua-bindings.md), which wins over staging
/// with no checkout at all.
fn modkit_dir(args: &[String]) -> Option<PathBuf> {
    modkit_dir_with(args, |name| std::env::var_os(name))
}

/// `modkit_dir` with the environment as a parameter, so tests never write it.
fn modkit_dir_with(
    args: &[String],
    get: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    flag(args, "--modkit")
        .map(PathBuf::from)
        .or_else(|| get("MODKIT").map(PathBuf::from))
}

/// A `$MODKIT` that is set but names no directory is an error: left alone it
/// would look like no checkout at all. `--modkit` wins over it, so it is then
/// not read. The environment is a parameter, so tests never write it.
fn modkit_env_check_with(
    args: &[String],
    get: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<(), String> {
    if flag(args, "--modkit").is_some() {
        return Ok(());
    }
    match get("MODKIT") {
        None => Ok(()),
        Some(value) if value.is_empty() => {
            Err("$MODKIT is empty: unset it or name a modkit checkout".into())
        }
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_dir() {
                Ok(())
            } else {
                Err(format!("$MODKIT: {} is not a directory", path.display()))
            }
        }
    }
}

/// Whether the script steps may not fetch anything: `--offline`, or
/// `HORIZON_MOD_OFFLINE` set to `1`, `true`, `yes` or `on`
/// (case-insensitive). Anything else — empty, `0`, `false`, `no`, `off` —
/// means online, so `HORIZON_MOD_OFFLINE=false` does not turn offline on.
fn offline(args: &[String]) -> bool {
    offline_with(args, |name| std::env::var_os(name))
}

/// `offline` with the environment as a parameter, so tests never write it.
fn offline_with(args: &[String], get: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    if args.iter().any(|a| a == "--offline") {
        return true;
    }
    let value = get("HORIZON_MOD_OFFLINE")
        .map(|v| v.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(value.as_str(), "1" | "true" | "yes" | "on")
}

/// `fmt [folder]`: rewrite the mod's scripts in place to StyLua's format
/// (Luau, two-space indent) — what `check`'s format step asks for.
fn fmt(args: &[String]) -> Result<(), String> {
    validate_flags(args, &[], &["--check"], &[])?;
    let root = Path::new(
        args.iter()
            .find(|a| !a.starts_with("--"))
            .map_or(".", AsRef::as_ref),
    );
    let manifest_text = std::fs::read_to_string(root.join(MANIFEST))
        .map_err(|e| format!("{MANIFEST}: {e} (run this in a mod's folder)"))?;
    let manifest = Manifest::parse(&manifest_text)?;
    let content = Content::read(root, &manifest)?;
    if content.scripts.is_empty() {
        return Err("no scripts/ in the mod: nothing to format".into());
    }
    // `--check` writes nothing: it lists the scripts that need formatting.
    if args.iter().any(|a| a == "--check") {
        let bad = scripts::unformatted(root, &content.scripts)?;
        if bad.is_empty() {
            println!("{} script(s) already formatted", content.scripts.len());
            return Ok(());
        }
        for name in &bad {
            println!("needs formatting: {name}");
        }
        return Err(format!(
            "{} script(s) need formatting: run cargo horizon-mod fmt",
            bad.len()
        ));
    }
    let changed = scripts::format_scripts(root, &content.scripts)?;
    println!(
        "{} script(s) already formatted",
        content.scripts.len() - changed
    );
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let path = build()?;
    let game = flag(args, "--game")
        .map(str::to_string)
        .or_else(|| std::env::var("HORIZON_CLIENT").ok())
        .unwrap_or_else(|| "horizon-client".into());
    let extra = args.iter().skip_while(|a| *a != "--").skip(1);
    println!("Starting {game} with {}", path.display());
    let status = Command::new(&game)
        .arg("--dev-mod")
        .arg(&path)
        .args(extra)
        .status()
        .map_err(|e| {
            format!("{game}: {e} (pass --game <path to horizon-client> or set HORIZON_CLIENT)")
        })?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("the game exited with {status}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder of this test's own: the tests of this binary run side
    /// by side in one process, so a shared name would have them tread on
    /// each other's files. A leftover of an interrupted run is removed.
    fn scratch(test: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cargo-horizon-mod-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_content_template_builds_and_checks_with_no_code() {
        let dir = scratch("content-template");
        let path = dir.join("gems");
        let new_args = [
            path.display().to_string(),
            "--template".into(),
            "content".into(),
        ];
        new(&new_args).unwrap();
        assert!(!path.join("Cargo.toml").exists());
        let hmod = build_in(&path).unwrap();
        assert_eq!(hmod, path.join("target/horizon-mod/gems.hmod"));
        let package = Package::read(&hmod).unwrap();
        assert!(package.wasm.is_none());
        let content = Content::read(&hmod, &package.manifest).unwrap();
        assert!(
            content
                .files
                .contains(&"data/gems/horizon/block/glowing_amethyst.json".into())
        );
        assert_eq!(content.files.len(), 8, "{:?}", content.files);
        let report = check::check(&hmod, check::kind_of(&hmod).unwrap(), None, None).unwrap();
        assert!(
            report.text().contains("no code: content only"),
            "{}",
            report.text()
        );
        // Code is native now.
        std::fs::write(path.join("Cargo.toml"), "").unwrap();
        assert!(build_in(&path).unwrap_err().contains("native"));
        assert!(
            new(&[
                dir.join("x").display().to_string(),
                "--template".into(),
                "wasm".into()
            ])
            .is_err()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A content-only mod has no scripts, so the check runs no type analysis:
    /// it reports no `types` step, and a fake luau-lsp is never spawned.
    #[test]
    fn a_content_only_mod_runs_no_script_analysis() {
        let dir = scratch("content-only-steps");
        let path = dir.join("gems");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "content".into(),
        ])
        .unwrap();
        let fake = dir.join("fake-luau-lsp");
        std::fs::write(&fake, "#!/bin/sh\ntouch \"$0.ran\"\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (value, result) = check_inner(
            &[
                path.display().to_string(),
                "--offline".into(),
                "--tools-cache".into(),
                dir.join("cache").display().to_string(),
                "--luau-lsp".into(),
                fake.display().to_string(),
            ],
            false,
        );
        assert!(result.is_ok(), "{result:?}");
        let names: Vec<&str> = value["steps"]
            .as_array()
            .map(|steps| steps.iter().filter_map(|s| s["name"].as_str()).collect())
            .unwrap_or_default();
        assert!(!names.contains(&"types"), "a types step ran: {value}");
        assert!(!fake.with_extension("ran").exists(), "luau-lsp ran");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The Lua template is a scripted mod the modkit runs: its manifest
    /// depends on the modkit, its scripts are packed and read, the registry's
    /// check accepts the `.hmod`, and the scripts are the ones the template
    /// says (the wand, its `on_use`).
    #[test]
    fn the_lua_template_builds_and_checks_as_a_scripted_mod_on_the_modkit() {
        let dir = scratch("lua-template");
        let path = dir.join("my-wand");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
            "--name".into(),
            "My Wand".into(),
        ])
        .unwrap();
        let manifest =
            Manifest::parse(&std::fs::read_to_string(path.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest.id, "my_wand");
        assert_eq!(manifest.depends, ["horizon_modkit >= 0.7"]);
        assert!(
            manifest
                .capabilities
                .contains(&horizon_mods::Capability::Scripts)
        );
        let init = std::fs::read_to_string(path.join("scripts/server/init.lua")).unwrap();
        assert!(init.contains("modkit.mod(\"my_wand\")") && init.contains("function Wand:on_use"));
        assert!(
            std::fs::read_to_string(path.join(".luaurc"))
                .unwrap()
                .contains("horizon-modkit/scripts/server/modkit")
        );
        assert!(
            std::fs::read_to_string(path.join("README.md"))
                .unwrap()
                .contains("--dev-mod")
        );
        let hmod = build_in(&path).unwrap();
        let package = Package::read(&hmod).unwrap();
        let content = Content::read(&hmod, &package.manifest).unwrap();
        assert_eq!(
            content.scripts,
            [
                "scripts/client/init.lua",
                "scripts/registry.lua",
                "scripts/server/init.lua",
                "scripts/shared/util.lua"
            ]
        );
        assert!(
            content
                .files
                .contains(&"assets/my_wand/lang/en_us.json".into())
        );
        let report = check::check(&hmod, check::kind_of(&hmod).unwrap(), None, None).unwrap();
        assert!(report.text().contains("scripts"), "{}", report.text());
        // The folder checks too (built on the way); its scripts need the game.
        check(&[
            path.display().to_string(),
            "--no-scripts".into(),
            "--offline".into(),
            "--tools-cache".into(),
            dir.join("cache").display().to_string(),
        ])
        .unwrap();
        let e = check(&[path.display().to_string()]).unwrap_err();
        assert!(e.contains("--game"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The template creates the per-side layout: server, client and shared
    /// entries, both sides require the shared module, Content lists them, and
    /// the offline check (format and lint in-process) passes.
    #[test]
    fn the_lua_template_creates_per_side_layout_with_shared_util() {
        let dir = scratch("lua-template-per-side");
        let path = dir.join("per-side-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        for f in [
            "scripts/server/init.lua",
            "scripts/client/init.lua",
            "scripts/shared/util.lua",
        ] {
            assert!(path.join(f).exists(), "missing {f}");
        }
        let server = std::fs::read_to_string(path.join("scripts/server/init.lua")).unwrap();
        assert!(server.contains("require(\"util\")"), "{server}");
        let client = std::fs::read_to_string(path.join("scripts/client/init.lua")).unwrap();
        assert!(client.contains("require(\"util\")"), "{client}");
        let shared = std::fs::read_to_string(path.join("scripts/shared/util.lua")).unwrap();
        assert!(shared.contains("function M.greeting"), "{shared}");
        let manifest =
            Manifest::parse(&std::fs::read_to_string(path.join(MANIFEST)).unwrap()).unwrap();
        let content = Content::read(&path, &manifest).unwrap();
        assert_eq!(content.server_scripts, ["scripts/server/init.lua"]);
        assert_eq!(content.client_scripts, ["scripts/client/init.lua"]);
        assert_eq!(content.shared_scripts, ["scripts/shared/util.lua"]);
        check(&[
            path.display().to_string(),
            "--no-scripts".into(),
            "--offline".into(),
            "--tools-cache".into(),
            dir.join("cache").display().to_string(),
        ])
        .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `check` on the generated Lua template passes every step available
    /// offline: format and lint run in-process, the type step is skipped
    /// with a notice.
    #[test]
    fn check_passes_every_offline_step_on_the_lua_template() {
        let dir = scratch("lua-template-offline");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        check(&[
            path.display().to_string(),
            "--no-scripts".into(),
            "--offline".into(),
            "--tools-cache".into(),
            dir.join("cache").display().to_string(),
        ])
        .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `--json` reports one object at the end, with the real overall result:
    /// a failing format step gives a single `ok:false` value carrying every
    /// step's pass/fail (no early `ok:true` before the steps run).
    #[test]
    fn json_reports_a_failing_format_step_once_with_ok_false() {
        let dir = scratch("lua-json-fmt");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        let init = path.join("scripts/server/init.lua");
        let source = std::fs::read_to_string(&init).unwrap();
        std::fs::write(
            &init,
            source.replace(
                "local p = player.position",
                "local   p   =   player.position",
            ),
        )
        .unwrap();
        let args = [
            path.display().to_string(),
            "--no-scripts".into(),
            "--offline".into(),
            "--tools-cache".into(),
            dir.join("cache").display().to_string(),
        ];
        let (value, result) = check_inner(&args, false);
        assert!(result.is_err(), "a bad format fails the check");
        assert_eq!(value["ok"], serde_json::Value::Bool(false), "{value}");
        assert!(value["report"].is_object(), "{value}");
        let steps = value["steps"].as_array().expect("{value}");
        assert_eq!(steps.len(), 3, "{value}");
        let format = steps
            .iter()
            .find(|s| s["name"] == "format")
            .expect("{value}");
        assert_eq!(format["ok"], serde_json::Value::Bool(false), "{value}");
        assert!(
            format["bad"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b == "scripts/server/init.lua"),
            "{value}"
        );
        for step in steps {
            assert!(
                step["name"].is_string() && step["ok"].is_boolean(),
                "{value}"
            );
        }
        // And the passing side: formatted again, the same value says ok.
        fmt(&[path.display().to_string()]).unwrap();
        let (value, result) = check_inner(&args, false);
        assert!(result.is_ok(), "{value}");
        assert_eq!(value["ok"], serde_json::Value::Bool(true), "{value}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An unknown `--flag` is an error naming the known ones, and a value flag
    /// given twice is an error too (`--dev-mod` may repeat).
    #[test]
    fn unknown_flags_and_repeated_values_are_errors() {
        let dir = scratch("check-flags");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        let (_, result) = check_inner(
            &[
                path.display().to_string(),
                "--typo".into(),
                "--no-scripts".into(),
            ],
            false,
        );
        let e = result.unwrap_err();
        assert!(
            e.contains("unknown flag --typo") && e.contains("--modkit"),
            "{e}"
        );
        let (_, result) = check_inner(
            &[
                path.display().to_string(),
                "--modkit".into(),
                "/a".into(),
                "--modkit".into(),
                "/b".into(),
            ],
            false,
        );
        let e = result.unwrap_err();
        assert!(e.contains("--modkit is given twice"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `fmt --check` reports the scripts that need formatting, fails, and
    /// writes nothing; an unknown flag to `fmt` is an error.
    #[test]
    fn fmt_check_reports_unformatted_files_without_writing() {
        let dir = scratch("fmt-check");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        let init = path.join("scripts/server/init.lua");
        let source = std::fs::read_to_string(&init).unwrap();
        let messy = source.replace(
            "local p = player.position",
            "local   p   =   player.position",
        );
        std::fs::write(&init, &messy).unwrap();
        let e = fmt(&[path.display().to_string(), "--check".into()]).unwrap_err();
        assert!(e.contains("need formatting"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&init).unwrap(),
            messy,
            "--check wrote"
        );
        let e = fmt(&[path.display().to_string(), "--typo".into()]).unwrap_err();
        assert!(e.contains("unknown flag --typo"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The mod's path may follow a switch (`check --json <mod>`), and a
    /// value-taking flag with no value (or another flag next) is an error.
    #[test]
    fn the_check_path_may_follow_a_flag_and_flags_need_values() {
        let dir = scratch("check-flag-order");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        let args = [
            "--json".into(),
            path.display().to_string(),
            "--no-scripts".into(),
            "--offline".into(),
            "--tools-cache".into(),
            dir.join("cache").display().to_string(),
        ];
        let (value, result) = check_inner(&args, false);
        assert!(result.is_ok(), "{value}");
        assert_eq!(value["ok"], serde_json::Value::Bool(true), "{value}");
        let (_, result) = check_inner(
            &[
                path.display().to_string(),
                "--modkit".into(),
                "--json".into(),
            ],
            false,
        );
        let e = result.unwrap_err();
        assert!(e.contains("--modkit needs a value"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A badly formatted script fails `check` (non-zero exit); `fmt` fixes it
    /// and `check` passes again.
    #[test]
    fn check_fails_on_bad_formatting_and_fmt_fixes_it() {
        let dir = scratch("lua-fmt");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        let init = path.join("scripts/server/init.lua");
        let source = std::fs::read_to_string(&init).unwrap();
        std::fs::write(
            &init,
            source.replace(
                "local p = player.position",
                "local   p   =   player.position",
            ),
        )
        .unwrap();
        let args = |extra: &str| {
            [
                path.display().to_string(),
                "--no-scripts".into(),
                extra.into(),
                "--tools-cache".into(),
                dir.join("cache").display().to_string(),
            ]
        };
        let e = check(&args("--offline")).unwrap_err();
        assert!(e.contains("script check failed"), "{e}");
        fmt(&[path.display().to_string()]).unwrap();
        assert!(
            std::fs::read_to_string(&init)
                .unwrap()
                .contains("local p = player.position")
        );
        check(&args("--offline")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A script with a selene-detectable defect (an undefined global) fails
    /// the lint step, and so `check`.
    #[test]
    fn check_fails_on_a_selene_error() {
        let dir = scratch("lua-lint");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        let init = path.join("scripts/server/init.lua");
        let source = std::fs::read_to_string(&init).unwrap();
        std::fs::write(
            &init,
            source.replace("horizon.broadcast(", "no_such_api.broadcast("),
        )
        .unwrap();
        let e = check(&[
            path.display().to_string(),
            "--no-scripts".into(),
            "--offline".into(),
            "--tools-cache".into(),
            dir.join("cache").display().to_string(),
        ])
        .unwrap_err();
        assert!(e.contains("script check failed"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The real luau-lsp (downloaded into the user cache on first run, or
    /// already there) type-checks a fresh Lua template through `check`
    /// with only the modkit path passed, and every step passes. A
    /// deliberate type error (`horizon.set_block("a")`) then fails the
    /// types step. Run it with the modkit at hand:
    /// `MODKIT=/path/to/horizon-modkit cargo nextest run -p cargo-horizon-mod --run-ignored only`
    /// (needs the network once, or a cached luau-lsp binary).
    #[test]
    #[ignore]
    fn check_passes_the_types_step_with_the_real_luau_lsp_on_a_fresh_template() {
        let modkit = std::env::var_os("MODKIT")
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("set MODKIT=<horizon-modkit checkout> to run this test"));
        let dir = scratch("lua-template-luau-lsp");
        let path = dir.join("wand-mod");
        new(&[
            path.display().to_string(),
            "--template".into(),
            "lua".into(),
        ])
        .unwrap();
        check(&[
            path.display().to_string(),
            "--no-scripts".into(),
            "--modkit".into(),
            modkit.display().to_string(),
        ])
        .unwrap();
        // A type error the definitions catch: set_block takes numbers.
        let init = path.join("scripts/server/init.lua");
        let source = std::fs::read_to_string(&init).unwrap();
        std::fs::write(
            &init,
            source.replace(
                "local M = modkit.mod",
                "horizon.set_block(\"a\")\nlocal M = modkit.mod",
            ),
        )
        .unwrap();
        let e = check(&[
            path.display().to_string(),
            "--no-scripts".into(),
            "--modkit".into(),
            modkit.display().to_string(),
        ])
        .unwrap_err();
        assert!(e.contains("script check failed"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `$MODKIT` that is set but names no directory is an error, not the
    /// same as unset; `--modkit` wins over it, so then it is not read.
    #[test]
    fn a_set_but_bad_modkit_env_is_an_error_unless_the_flag_wins() {
        // The environment is a parameter: no test writes the real one.
        let env = |value: &str| {
            let value = std::ffi::OsString::from(value);
            move |name: &str| {
                assert_eq!(name, "MODKIT");
                Some(value.clone())
            }
        };
        let manifest = env!("CARGO_MANIFEST_DIR");
        let bare = ["check".to_owned(), "mod".to_owned()];
        let with_flag = [
            "check".to_owned(),
            "mod".to_owned(),
            "--modkit".to_owned(),
            "/no/such/modkit".to_owned(),
        ];
        assert_eq!(modkit_env_check_with(&bare, env(manifest)), Ok(()));
        assert_eq!(
            modkit_env_check_with(&bare, env("/no/such/modkit")).unwrap_err(),
            "$MODKIT: /no/such/modkit is not a directory"
        );
        assert_eq!(
            modkit_env_check_with(&bare, env(&format!("{manifest}/Cargo.toml"))).unwrap_err(),
            format!("$MODKIT: {manifest}/Cargo.toml is not a directory")
        );
        assert!(
            modkit_env_check_with(&bare, env(""))
                .unwrap_err()
                .starts_with("$MODKIT is empty"),
        );
        assert_eq!(
            modkit_env_check_with(&with_flag, env("/no/such/modkit")),
            Ok(())
        );
        assert_eq!(
            modkit_env_check_with(&bare, |_: &str| None),
            Ok(()),
            "an unset $MODKIT is no checkout, not an error"
        );
    }

    /// `--modkit` wins over `$MODKIT`, which wins over no checkout: the flag
    /// is read first, the environment only without it.
    #[test]
    fn the_modkit_flag_wins_over_the_env_which_wins_over_nothing() {
        // The environment is a parameter: no test writes the real one.
        let env = |value: Option<&str>| {
            let value = value.map(std::ffi::OsString::from);
            move |name: &str| {
                assert_eq!(name, "MODKIT");
                value.clone()
            }
        };
        let flag = |dir: &str| {
            modkit_dir_with(
                &["check".into(), "mod".into(), "--modkit".into(), dir.into()],
                env(Some("/from/env")),
            )
        };
        assert_eq!(flag("/from/flag"), Some(PathBuf::from("/from/flag")));
        let args = ["check".into(), "mod".into()];
        assert_eq!(
            modkit_dir_with(&args, env(Some("/from/env"))),
            Some(PathBuf::from("/from/env"))
        );
        assert_eq!(modkit_dir_with(&args, env(None)), None);
    }

    /// Only `1`, `true`, `yes` and `on` (case-insensitive) turn offline on:
    /// empty, `0`, `false`, `no` and `off` mean online.
    #[test]
    fn only_truthy_values_turn_offline_on() {
        // The environment is a parameter: no test writes the real one.
        let env = |value: Option<&str>| {
            let value = value.map(std::ffi::OsString::from);
            move |_: &str| value.clone()
        };
        for value in ["1", "true", "TRUE", "yes", "Yes", "on", "ON"] {
            assert!(
                offline_with(&[], env(Some(value))),
                "{value:?} means offline"
            );
        }
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("FALSE"),
            Some("no"),
            Some("off"),
            Some("2"),
            Some("banana"),
        ] {
            assert!(!offline_with(&[], env(value)), "{value:?} means online");
        }
        assert!(
            offline_with(&["--offline".into()], env(Some("false"))),
            "the flag still turns offline on"
        );
    }

    #[test]
    fn the_native_template_packs_into_an_hnmod_the_registry_accepts() {
        use std::io::Write;
        let dir = scratch("native-template");
        let path = dir.join("machine-mod");
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let args = [
            path.display().to_string(),
            "--template".into(),
            "native".into(),
            "--horizon".into(),
            commit.into(),
        ];
        new(&args).unwrap();
        let api = std::fs::read_to_string(path.join("api/Cargo.toml")).unwrap();
        assert!(api.contains(&format!("rev = \"{commit}\"")), "{api}");
        // Packed as the launcher packs a mod folder: the manifest, then the bundle under
        // `mixins/<id>/` (docs/api/mod-files.md).
        let manifest = serde_json::json!({
            "schema": 1, "id": "machine_mod", "version": "0.1.0", "game": "26.3",
            "horizon": commit, "mixins": "b".repeat(40),
        });
        let hnmod = dir.join("machine_mod.hnmod");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&hnmod).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("horizon-native.json", options).unwrap();
        zip.write_all(manifest.to_string().as_bytes()).unwrap();
        for file in [
            "horizon-mixin.toml",
            "api/Cargo.toml",
            "api/src/lib.rs",
            ".gitignore",
        ] {
            assert!(horizon_mods::index::native::allowed(file), "{file}");
            zip.start_file(format!("mixins/machine_mod/{file}"), options)
                .unwrap();
            zip.write_all(&std::fs::read(path.join(file)).unwrap())
                .unwrap();
        }
        zip.finish().unwrap();
        let report = check::check(&hnmod, check::kind_of(&hnmod).unwrap(), None, None).unwrap();
        assert!(report.text().contains("machine_mod"), "{}", report.text());
        // A pin that is not a commit is refused; with none, the folder builds on the
        // instance's Horizon.
        let bad = [
            dir.join("bad").display().to_string(),
            "--template".into(),
            "native".into(),
            "--horizon".into(),
            "main".into(),
        ];
        assert!(new(&bad).unwrap_err().contains("40 lowercase hex"));
        let free = dir.join("free");
        new(&[
            free.display().to_string(),
            "--template".into(),
            "native".into(),
        ])
        .unwrap();
        let mixin = std::fs::read_to_string(free.join("horizon-mixin.toml")).unwrap();
        assert!(!mixin.contains("\nhorizon_revision"), "{mixin}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
