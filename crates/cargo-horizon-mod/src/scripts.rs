//! The script steps of `cargo horizon-mod check` and its `fmt` subcommand
//! (issue 53): StyLua formatting, selene linting and luau-lsp type analysis.
//! StyLua and selene run in-process (library crates, nothing on the PATH);
//! luau-lsp is C++ and runs as a pinned binary from the user cache dir.
use std::{
    collections::HashMap,
    io::Read as _,
    path::{Path, PathBuf},
    process::Command,
};

// The crate is published as `stylua`, its library as `stylua_lib`.
use stylua_lib as stylua;

/// StyLua as Horizon formats Lua mods: Luau syntax, two-space indent.
fn stylua_config() -> stylua::Config {
    stylua::Config {
        syntax: stylua::LuaVersion::Luau,
        indent_type: stylua::IndentType::Spaces,
        indent_width: 2,
        ..stylua::Config::default()
    }
}

/// The scripts' sources, in the manifest's order.
fn sources(root: &Path, scripts: &[String]) -> Result<Vec<(String, String)>, String> {
    scripts
        .iter()
        .map(|name| {
            std::fs::read_to_string(root.join(name))
                .map(|text| (name.clone(), text))
                .map_err(|e| format!("{name}: {e}"))
        })
        .collect()
}

/// `fmt`: rewrite each script to StyLua's output. Returns how many changed.
pub fn format_scripts(root: &Path, scripts: &[String]) -> Result<usize, String> {
    let mut changed = 0;
    for (name, source) in sources(root, scripts)? {
        let formatted = stylua::format_code(
            &source,
            stylua_config(),
            None,
            stylua::OutputVerification::None,
        )
        .map_err(|e| format!("{name}: {e}"))?;
        if formatted != source {
            std::fs::write(root.join(&name), &formatted).map_err(|e| format!("{name}: {e}"))?;
            println!("Formatted {name}");
            changed += 1;
        }
    }
    Ok(changed)
}

/// The format check step: the scripts whose text differs from StyLua's
/// output. An empty Vec is a pass.
pub fn unformatted(root: &Path, scripts: &[String]) -> Result<Vec<String>, String> {
    let mut bad = Vec::new();
    for (name, source) in sources(root, scripts)? {
        let formatted = stylua::format_code(
            &source,
            stylua_config(),
            None,
            stylua::OutputVerification::None,
        )
        .map_err(|e| format!("{name}: {e}"))?;
        if formatted != source {
            bad.push(name);
        }
    }
    Ok(bad)
}

/// The `horizon` globals selene lints against: a hand-written minimal std on
/// top of selene's built-in Luau one, until a std is generated from the API
/// model (follow-up on issue 53).
const SELENE_STD: &str = include_str!("../tools/selene-horizon-std.yml");

/// One selene finding: `scripts/init.lua:3: error(undefined_variable): ...`.
#[derive(Debug)]
pub struct Lint {
    pub error: bool,
    pub text: String,
}

/// The lint step: every selene diagnostic over the scripts, errors and
/// warnings both; the caller fails the step on the errors.
pub fn lints(root: &Path, scripts: &[String]) -> Result<Vec<Lint>, String> {
    use selene_lib::{Checker, CheckerConfig, standard_library::StandardLibrary};
    let mut horizon: StandardLibrary =
        serde_yaml::from_str(SELENE_STD).map_err(|e| format!("selene std: {e}"))?;
    let mut std = match horizon.base.take() {
        Some(base) => StandardLibrary::from_name(&base)
            .ok_or_else(|| format!("selene std: unknown base std {base}"))?,
        None => StandardLibrary::default(),
    };
    std.extend(horizon);
    let checker = Checker::<serde_json::Value>::new(CheckerConfig::default(), std)
        .map_err(|e| format!("selene: {e}"))?;
    let mut found = Vec::new();
    for (name, source) in sources(root, scripts)? {
        let ast = full_moon::parse(&source)
            .map_err(|errors| format!("{name}: cannot parse: {errors:?}"))?;
        for finding in checker.test_on(&ast) {
            let error = finding.severity == selene_lib::lints::Severity::Error;
            // Byte offset to 1-based line: count the newlines before it.
            let start = finding.diagnostic.primary_label.range.0 as usize;
            let line = 1 + source[..start.min(source.len())].matches('\n').count();
            let severity = if error { "error" } else { "warning" };
            let text = format!(
                "{name}:{line}: {severity}({}): {}",
                finding.diagnostic.code, finding.diagnostic.message
            );
            found.push(Lint { error, text });
        }
    }
    Ok(found)
}

/// The luau-lsp pin (tools/luau-lsp.toml) and the definitions file
/// (`cargo run -p xtask -- lua-types` writes docs/api/horizon.d.lua).
const LUAU_LSP_MANIFEST: &str = include_str!("../tools/luau-lsp.toml");
const HORIZON_DEFINITIONS: &str = include_str!("../../../docs/api/horizon.d.lua");

#[derive(serde::Deserialize)]
struct LuauLspManifest {
    version: String,
    needs_pin: bool,
    target: HashMap<String, LuauLspTarget>,
}

#[derive(serde::Deserialize)]
struct LuauLspTarget {
    url: String,
    sha256: String,
}

/// What one pipeline step came to.
#[derive(Debug)]
pub enum Outcome {
    Pass,
    /// Passed, but part of the step was not checked: the text says what.
    Unchecked(String),
    Skip(String),
    Fail(String),
}

/// A side a mod runs on: the folder the runtime searches before `scripts/shared/`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Server,
    Client,
}

impl Side {
    fn folder(self) -> &'static str {
        match self {
            Side::Server => "server",
            Side::Client => "client",
        }
    }
}

pub fn analyze(
    root: &Path,
    scripts: &[String],
    sides: &[Side],
    explicit: Option<&Path>,
    cache: &Path,
    offline: bool,
    modkit: Option<&Path>,
) -> Result<Outcome, String> {
    analyze_with(
        LUAU_LSP_MANIFEST,
        root,
        scripts,
        sides,
        explicit,
        cache,
        offline,
        modkit,
    )
}

/// The type-analysis step: `luau-lsp analyze --definitions:@horizon=...` over
/// the scripts. `explicit` is `--luau-lsp <path>`; the pinned binary is
/// downloaded into `cache` (SHA-256 verified) unless it is already there.
///
/// `sides` are the states the mod runs: when it has any, it is analysed once
/// per side from a staged view with `scripts/shared/` and `scripts/<side>/`
/// flattened into `scripts/`, as the runtime resolves requires.
///
/// `modkit` is the modkit checkout the scripts' `@modkit` require names
/// (`--modkit <dir>` or `$MODKIT`). The mod's own `.luaurc` aliases it to a
/// sibling checkout: when that sibling is there, luau-lsp runs in the mod;
/// when it is not, the analysis runs from a staged copy in the cache whose
/// `.luaurc` points the alias at the real checkout — the mod's `.luaurc` is
/// never touched, and `--base-luaurc` cannot do this (a mod's own `.luaurc`
/// overrides its base, as the real binary shows). With no modkit anywhere,
/// the `@modkit` require cannot resolve: only its `Unknown require`
/// diagnostics are dropped, with a notice, and everything else still fails.
#[allow(clippy::too_many_arguments)]
pub fn analyze_with(
    manifest_text: &str,
    root: &Path,
    scripts: &[String],
    sides: &[Side],
    explicit: Option<&Path>,
    cache: &Path,
    offline: bool,
    modkit: Option<&Path>,
) -> Result<Outcome, String> {
    let manifest: LuauLspManifest =
        toml::from_str(manifest_text).map_err(|e| format!("luau-lsp manifest: {e}"))?;
    let binary = match explicit {
        Some(path) => path.to_path_buf(),
        None => {
            let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
            let target = manifest
                .target
                .get(&platform)
                .ok_or(format!("luau-lsp manifest: no target for {platform}"))?;
            let dir = cache
                .join("luau-lsp")
                .join(&manifest.version)
                .join(&platform);
            let exe = dir.join(exe_name());
            // A cached binary runs only when its bytes still match the
            // SHA-256 its verified download recorded: a run killed mid-write
            // or two runs racing leave a truncated binary that no later run
            // (not even --offline) may trust.
            let cached = check_cached(&exe, &target.sha256);
            if !matches!(cached, Cached::Good) {
                if manifest.needs_pin {
                    return Ok(Outcome::Skip(
                        "luau-lsp pin is a placeholder: needs pin (issue 53); \
                         pass --luau-lsp <path> to use a local binary"
                            .into(),
                    ));
                }
                if offline {
                    return Ok(match cached {
                        // Not run, and not a failure: the same as no cache.
                        Cached::Bad(why) => Outcome::Skip(format!(
                            "offline: cached luau-lsp {} is unusable ({why}); skipped \
                             (re-run without --offline to fetch it again)",
                            exe.display()
                        )),
                        _ => Outcome::Skip(format!(
                            "offline: luau-lsp {} is not cached; skipped (run once \
                             without --offline to fetch it)",
                            manifest.version
                        )),
                    });
                }
                let bytes = fetch(&target.url)?;
                let digest = sha256(&bytes);
                if digest != target.sha256 {
                    return Ok(Outcome::Fail(format!(
                        "luau-lsp download: SHA-256 mismatch for {} (expected {}, got \
                         {digest}) — refusing to run it",
                        target.url, target.sha256
                    )));
                }
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let contents = unpack(&bytes)?;
                if let Err(e) = install_binary(&exe, &contents, &target.sha256) {
                    // Windows cannot replace a binary that is running: when the
                    // cached pair is still whole, keep using it.
                    if !matches!(check_cached(&exe, &target.sha256), Cached::Good) {
                        return Err(e);
                    }
                    eprintln!("note: could not replace the cached luau-lsp ({e}); using it");
                }
            }
            exe
        }
    };
    // The definitions go next to the cache, never into the repo.
    std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    // Rewritten only when it differs, and atomically: another check may be
    // reading it right now (a truncated read reports `horizon` as unknown).
    let definitions = cache.join("horizon.d.lua");
    if std::fs::read(&definitions).ok().as_deref() != Some(HORIZON_DEFINITIONS.as_bytes()) {
        write_atomically(&definitions, HORIZON_DEFINITIONS.as_bytes(), false)?;
    }
    // Where the `@modkit` alias of the mod's `.luaurc` points, and whether
    // that resolves to a checkout the analysis can read. When it does not,
    // `--modkit <dir>` (or `$MODKIT`) names one elsewhere.
    let alias = modkit_alias(root)?;
    let checkout = alias
        .clone()
        .filter(|p| require_target_exists(p))
        .or_else(|| {
            modkit
                .map(Path::new)
                .filter(|m| m.is_dir())
                .and_then(|m| std::fs::canonicalize(m).ok())
                .map(normalize)
        });
    // The modkit's own scripts are a dependency, not the mod's code: their
    // type errors (against the API version the mod pins) are not this mod's.
    let ignore = checkout
        .as_ref()
        .map(|_| format!("**/{RESERVED_MODKIT}/**"));
    // No modkit to be found: the `@modkit` require cannot resolve, so its
    // diagnostics are dropped (below) with a one-line notice.
    let unresolvable = match &checkout {
        Some(_) => None,
        None => alias.as_ref().map(|p| unresolvable_prefix(p)),
    };
    // A mod with per-side folders is analysed once per side from a staged
    // view flattened the way the runtime resolves requires; one without them
    // keeps the single analysis over its scripts as it was.
    let mut runs = Vec::new();
    if sides.is_empty() {
        let staged = match &checkout {
            Some(kit) => Some(stage_analysis(root, cache, Some(kit), None)?),
            None => None,
        };
        let workdir = staged
            .as_ref()
            .map_or_else(|| root.to_path_buf(), |s| s.path().to_path_buf());
        runs.push(Run {
            side: None,
            staged,
            workdir,
            args: scripts.to_vec(),
        });
    } else {
        for side in sides {
            let staged = stage_analysis(root, cache, checkout.as_deref(), Some(*side))?;
            runs.push(Run {
                side: Some(*side),
                workdir: staged.path().to_path_buf(),
                args: vec!["scripts".into()],
                staged: Some(staged),
            });
        }
    }
    let mut failures = Vec::new();
    let mut unchecked = false;
    for run in &runs {
        let mut cmd = Command::new(&binary);
        cmd.arg("analyze")
            .arg(format!("--definitions:@horizon={}", definitions.display()));
        if let Some(pattern) = &ignore {
            cmd.arg(format!("--ignore={pattern}"));
        }
        cmd.args(&run.args).current_dir(&run.workdir);
        // The binary may have just been written (a download): see `retry_busy`.
        let output =
            retry_busy(|| cmd.output()).map_err(|e| format!("{}: {e}", binary.display()))?;
        if output.status.success() {
            continue;
        }
        let raw_text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // luau-lsp on Windows prints paths with backslashes: the path parts
        // are normalised once, before both the drop test and the rewrite.
        let raw: Vec<String> = raw_text.lines().map(normalize_diagnostic_paths).collect();
        let mut kept: Vec<&str> = match &unresolvable {
            Some(expected) => raw
                .iter()
                .map(String::as_str)
                .filter(|l| !is_unresolvable_require(l, expected, std::env::consts::OS))
                .collect(),
            None => raw.iter().map(String::as_str).collect(),
        };
        let dropped = raw.len() - kept.len();
        // A require the side's view resolves is not missing: luau-lsp looked
        // for it in the requiring file's folder (or as a literal dotted file
        // name), the runtime at the state root. The target is analysed as its
        // own file anyway, so only a real absence fails.
        let mut resolved = 0;
        if let (Some(staged), Some(_)) = (&run.staged, run.side) {
            let before = kept.len();
            kept.retain(|line| !is_view_require(line, staged));
            resolved = before - kept.len();
        }
        let text = match &run.staged {
            Some(staged) => rewrite_paths(&kept.join("\n"), staged),
            None => kept.join("\n"),
        };
        // `[INFO]` lines are noise around the diagnostics; a run that prints
        // no diagnostic at all still failed, though (e.g. `Cannot get <path>:
        // path does not exist`), unless its only report was a require the
        // view resolves.
        let remaining: Vec<&str> = text.lines().filter(|l| !l.starts_with("[INFO]")).collect();
        if remaining.is_empty() {
            if unresolvable.is_some() && dropped > 0 {
                unchecked = true;
                continue;
            }
            if resolved > 0 {
                continue;
            }
            failures.push(match run.side {
                Some(side) => format!("[{}] luau-lsp analyze failed", side.folder()),
                None => {
                    // A run that only printed `[INFO]` lines still failed:
                    // name its exit code and keep the output.
                    let code = output
                        .status
                        .code()
                        .map_or_else(|| "signal".to_string(), |c| c.to_string());
                    let output = raw_text.trim_end();
                    if output.is_empty() {
                        format!("luau-lsp analyze failed (exit {code})")
                    } else {
                        format!("luau-lsp analyze failed (exit {code})\n{output}")
                    }
                }
            });
            continue;
        }
        for line in remaining {
            failures.push(match run.side {
                Some(side) => format!("[{}] {line}", side.folder()),
                None => line.to_string(),
            });
        }
    }
    let outcome = if !failures.is_empty() {
        Outcome::Fail(format!("luau-lsp analyze:\n{}", failures.join("\n")))
    } else if unchecked {
        Outcome::Unchecked(
            "no modkit checkout found: @modkit requires are unchecked (pass --modkit <dir>)".into(),
        )
    } else {
        Outcome::Pass
    };
    // The runs' staged copies (their guards) are removed on return: each
    // served its run alone, pass or fail.
    Ok(outcome)
}

/// One luau-lsp run: the whole mod, or one side's flattened staged view.
struct Run {
    side: Option<Side>,
    staged: Option<StagedDir>,
    workdir: PathBuf,
    args: Vec<String>,
}

/// The `@modkit` alias of the mod's `.luaurc`, resolved against the mod's
/// root the way luau resolves it (`../horizon-modkit/scripts/modkit` is the
/// sibling checkout's scripts). None when the mod has no such alias.
fn modkit_alias(root: &Path) -> Result<Option<PathBuf>, String> {
    let Some(luaurc) = read_luaurc(root)? else {
        return Ok(None);
    };
    let Some(alias) = luaurc
        .get("aliases")
        .and_then(|a| a.get("modkit"))
        .and_then(|m| m.as_str())
    else {
        return Ok(None);
    };
    let path = Path::new(alias);
    Ok(Some(normalize(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })))
}

/// The mod's `.luaurc` as a JSON value: none when there is no file. luau-lsp
/// reads it as JSON with comments and trailing commas, so those are allowed;
/// anything else that does not parse is an error naming the file.
fn read_luaurc(root: &Path) -> Result<Option<serde_json::Value>, String> {
    let path = root.join(".luaurc");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    serde_json::from_str(&strip_jsonc(&text))
        .map(Some)
        .map_err(|e| {
            format!(
                "{}: not valid JSON (// and /* */ comments and trailing commas are allowed): {e}",
                path.display()
            )
        })
}

/// Removes `//` and `/* */` comments and trailing commas before `}` or `]`
/// from JSON text. String literals are copied as they are, so a `//` or a
/// comma inside one is text.
fn strip_jsonc(text: &str) -> String {
    // Pass 1: comments.
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            plain.push(c);
            if c == '\\' {
                plain.extend(chars.next());
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            plain.push(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            while chars.peek().is_some_and(|&n| n != '\n') {
                chars.next();
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut last = '\0';
            for n in chars.by_ref() {
                if last == '*' && n == '/' {
                    break;
                }
                last = n;
            }
            plain.push(' ');
        } else {
            plain.push(c);
        }
    }
    // Pass 2: trailing commas.
    let mut out = String::with_capacity(plain.len());
    let mut chars = plain.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                out.extend(chars.next());
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == ',' {
            let rest = chars.clone().find(|n| !n.is_whitespace());
            if !matches!(rest, Some('}' | ']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A require target resolves as `<path>.lua` or `<path>/init.lua`.
fn require_target_exists(path: &Path) -> bool {
    path.with_extension("lua").is_file() || path.join("init.lua").is_file()
}

/// Lexically normalizes `..` and `.` out of a path (no filesystem access,
/// so an alias to a missing sibling still comes out as one clean path).
fn normalize(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The prefix of an unresolvable `@modkit` alias's paths in luau-lsp's
/// `Unknown require:` diagnostics: the alias with forward slashes and no
/// `.lua` suffix (`.../modkit` for a file or a directory alias alike).
fn unresolvable_prefix(alias: &Path) -> String {
    let text = alias.display().to_string().replace('\\', "/");
    text.strip_suffix(".lua").unwrap_or(&text).to_string()
}

/// Rewrites the separators of the path parts of one luau-lsp diagnostic line
/// to `/`, so a Windows run's `scripts\util.lua(1,1)` and `Unknown require:
/// C:\...\util.lua` match the view's forward-slash paths. Only the path
/// before the `(line,col)` and the path after `Unknown require: ` change: the
/// rest of the message is text (`bad escape \q` stays as it is).
fn normalize_diagnostic_paths(line: &str) -> String {
    let mut out = line.to_string();
    if let Some(at) = line_col_open(line) {
        let path = line[..at].replace('\\', "/");
        out.replace_range(..at, &path);
    }
    if let Some(at) = out.find("Unknown require: ") {
        let start = at + "Unknown require: ".len();
        let path = out[start..].replace('\\', "/");
        out.replace_range(start.., &path);
    }
    out
}

/// Where the `file(line,col)` shape of a diagnostic line opens its `(`: the
/// first `(` followed by `<digits>,<digits>)`.
fn line_col_open(line: &str) -> Option<usize> {
    let at = line.find('(')?;
    let numbers = |text: &str| text.bytes().all(|b| b.is_ascii_digit()) && !text.is_empty();
    let rest = &line[at + 1..];
    let (line_no, rest) = rest.split_once(',')?;
    let (col, _) = rest.split_once(')')?;
    (numbers(line_no) && numbers(col)).then_some(at)
}

/// Whether a luau-lsp line is an `Unknown require` of an unresolvable
/// `@modkit` path: it names `prefix` followed by `.lua` (the alias itself) or
/// by `/<...>.lua` (a file in the alias's directory, e.g. `init.lua` or
/// `util.lua` for `require("@modkit/util")`). Separators are normalised on
/// both sides, and case is folded on the OS whose file system ignores it
/// (Windows, macOS). Any other path, or a name that only starts with the
/// prefix (`modkit2.lua`), is not dropped.
fn is_unresolvable_require(line: &str, prefix: &str, os: &str) -> bool {
    if !line.contains("Unknown require") {
        return false;
    }
    let fold = |text: &str| {
        let text = text.replace('\\', "/");
        if matches!(os, "windows" | "macos") {
            text.to_lowercase()
        } else {
            text
        }
    };
    let line = fold(line);
    let prefix = fold(prefix);
    line.match_indices(prefix.as_str()).any(|(at, _)| {
        let rest = &line[at + prefix.len()..];
        let after_lua = rest.strip_prefix(".lua");
        match after_lua {
            Some(tail) => !tail.starts_with(|c: char| c.is_alphanumeric() || c == '_'),
            None => rest.starts_with('/') && rest.contains(".lua"),
        }
    })
}

/// Whether a luau-lsp `Unknown require` names a file the runtime resolves in
/// the side's flattened view. luau-lsp resolves a require from the requiring
/// file's folder (one folder up for an `init.lua`, where it looks for the
/// sibling modules of the folder the runtime's state root is), and takes a
/// dotted name as a literal file name; the runtime resolves both from the
/// state root, which the view's `scripts/` is. Such a target is analysed as
/// its own file, so its report is a resolution quirk, not a missing file.
fn is_view_require(line: &str, staged: &StagedDir) -> bool {
    if !line.contains("Unknown require") {
        return false;
    }
    let Some((file, _)) = line.split_once('(') else {
        return false;
    };
    if !file.starts_with("scripts/") {
        return false;
    }
    let Some((_, unknown)) = line.rsplit_once("Unknown require: ") else {
        return false;
    };
    let mut roots = vec![staged.path().display().to_string().replace('\\', "/")];
    let canonical = canonical(staged.path())
        .display()
        .to_string()
        .replace('\\', "/");
    if !roots.contains(&canonical) {
        roots.push(canonical);
    }
    let unknown = unknown.trim().replace('\\', "/");
    let Some(rel) = roots
        .iter()
        .find_map(|root| unknown.strip_prefix(&format!("{root}/")))
    else {
        return false;
    };
    let file = Path::new(file);
    let dir = file.parent().unwrap_or_else(|| Path::new(""));
    // An `init.lua` is its folder's module: luau-lsp resolves its requires
    // from the folder's parent.
    let base = if file.file_name().is_some_and(|name| name == "init.lua") {
        dir.parent().unwrap_or_else(|| Path::new(""))
    } else {
        dir
    };
    let base = base.to_string_lossy().replace('\\', "/");
    let tail = if base.is_empty() {
        rel
    } else {
        let Some(tail) = rel.strip_prefix(&format!("{base}/")) else {
            return false;
        };
        tail
    };
    let Some(name) = tail.strip_suffix(".lua") else {
        return false;
    };
    let name = name.replace('.', "/");
    // Only the files the flatten copied for this side count (the `map`): the
    // registry in the view is the separate data stage, not a side module.
    staged.map.contains_key(&format!("scripts/{name}.lua"))
        || staged.map.contains_key(&format!("scripts/{name}/init.lua"))
}

/// Stages the analysis in a unique per-run directory under the cache: the
/// mod's files, and a `.luaurc` of the mod's own with the `@modkit` alias
/// pointing at the checkout (or, with no checkout, at the mod's own alias
/// target, which does not resolve and is handled as before). A shared
/// directory lets two checks in parallel corrupt each other (and its removal
/// error was ignored). Running there keeps luau-lsp from reading the mod's own
/// `.luaurc` (whose sibling alias does not resolve) without editing it.
///
/// With a `side`, the staged `scripts/` is rebuilt as that side sees it:
/// `scripts/shared/**` flattened in, then `scripts/<side>/**` over it, then
/// the registry as it is. The returned `map` names the real file of each
/// flattened one. The guard removes the directory when it is dropped.
fn stage_analysis(
    root: &Path,
    cache: &Path,
    modkit: Option<&Path>,
    side: Option<Side>,
) -> Result<StagedDir, String> {
    static STAGES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = STAGES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The guard exists before the directory does, so every failure below
    // (including one part-way through creating it) removes what is there.
    let mut staged = StagedDir {
        path: cache.join(format!("analysis-{}-{n}", std::process::id())),
        map: HashMap::new(),
    };
    let dir = staged.path().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // The whole mod, not only its scripts: a `.luaurc` alias or a require can
    // reach any folder of the mod. Its own folder of the reserved name would
    // be taken for the modkit's, so such a mod is refused.
    if copy_mod(root, &dir, cache) {
        return Err(format!(
            "the mod has a folder named {RESERVED_MODKIT}, which the check uses for the \
             modkit: rename it"
        ));
    }
    if let Some(modkit) = modkit {
        // The checkout's scripts, under the reserved name: the module itself
        // when the checkout is a whole repository (its `scripts/modkit`, the
        // layout the template's sibling alias names, or `scripts/server/modkit`
        // as the modkit repository has it), else the checkout.
        let module = ["scripts/modkit", "scripts/server/modkit"]
            .iter()
            .map(|path| modkit.join(path))
            .find(|path| path.is_dir())
            .unwrap_or_else(|| modkit.to_path_buf());
        let reserved = dir.join(RESERVED_MODKIT);
        std::fs::create_dir_all(&reserved).map_err(|e| e.to_string())?;
        copy_mod(&module, &reserved, cache);
    }
    if let Some(side) = side {
        flatten_scripts(root, &dir, side, &mut staged.map)?;
    }
    // A checkout is staged under the reserved name; without one, the mod's
    // own alias target is kept (absolute), so the unresolvable `@modkit`
    // require still names the path the caller knows.
    let alias = match modkit {
        Some(_) => Some(RESERVED_MODKIT.to_string()),
        None => modkit_alias(root)?.map(|p| p.display().to_string().replace('\\', "/")),
    };
    let luaurc = staged_luaurc_text(root, alias.as_deref(), side)?;
    std::fs::write(dir.join(".luaurc"), luaurc).map_err(|e| e.to_string())?;
    Ok(staged)
}

/// Rebuilds `stage/scripts/` as `side` sees it: the shared files flattened
/// in, the side's files over them (a side file wins a same name), then the
/// registry as it is. `map` records, for every flattened file, its view path
/// (`scripts/util.lua`) and the real one (`scripts/shared/util.lua`); the
/// registry is not in it, as it is the separate data stage.
fn flatten_scripts(
    root: &Path,
    stage: &Path,
    side: Side,
    map: &mut HashMap<String, String>,
) -> Result<(), String> {
    let scripts = stage.join("scripts");
    if scripts.exists() {
        std::fs::remove_dir_all(&scripts).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&scripts).map_err(|e| e.to_string())?;
    let source = root.join("scripts");
    // The side's folder is over the shared one: a same-named file wins.
    flatten_tree(
        &source.join("shared"),
        &scripts,
        "scripts",
        "scripts/shared",
        map,
    )?;
    flatten_tree(
        &source.join(side.folder()),
        &scripts,
        "scripts",
        &format!("scripts/{}", side.folder()),
        map,
    )?;
    // The registry is the data stage: it keeps its own paths (`@registry`),
    // so it is not in the view map (`is_view_require` resolves only the
    // flatten) and its files are not a side's modules.
    forget_flat(map, "scripts/registry.lua");
    forget_flat(map, "scripts/registry");
    let registry = source.join("registry.lua");
    if registry.is_file() {
        std::fs::copy(&registry, scripts.join("registry.lua"))
            .map_err(|e| format!("{}: {e}", registry.display()))?;
    }
    let mut data = HashMap::new();
    flatten_tree(
        &source.join("registry"),
        &scripts.join("registry"),
        "scripts/registry",
        "scripts/registry",
        &mut data,
    )?;
    // Anything else at the top of scripts/ (an alias target, the old
    // single entry) stays where it is.
    if source.is_dir() {
        let entries =
            std::fs::read_dir(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", source.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if matches!(
                name.as_str(),
                "shared" | "server" | "client" | "registry" | "registry.lua"
            ) {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
            let flat = format!("scripts/{name}");
            if kind.is_dir() {
                flatten_tree(&entry.path(), &scripts.join(&name), &flat, &flat, map)?;
            } else if kind.is_file() {
                std::fs::copy(entry.path(), scripts.join(&name))
                    .map_err(|e| format!("{}: {e}", entry.path().display()))?;
                map.insert(flat.clone(), flat);
            }
        }
    }
    Ok(())
}

/// Copies the files under `from` into `to`, flattening `flat` to the mod's
/// real `real`: `shared/util.lua` copied with `flat` `scripts` and `real`
/// `scripts/shared` becomes `scripts/util.lua` and maps to
/// `scripts/shared/util.lua`. A missing `from` is nothing to flatten. A
/// failed read, create or copy fails the check, naming the path: a truncated
/// view must not pass or fail for the wrong reason. The side's file wins a
/// same-named shared directory and the reverse, and the map drops the paths
/// the loser left behind.
fn flatten_tree(
    from: &Path,
    to: &Path,
    flat: &str,
    real: &str,
    map: &mut HashMap<String, String>,
) -> Result<(), String> {
    if !from.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if to.is_file() {
        std::fs::remove_file(to).map_err(|e| format!("{}: {e}", to.display()))?;
        forget_flat(map, flat);
    }
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        let dest = to.join(&name);
        let flat_name = format!("{flat}/{name}");
        if kind.is_dir() {
            if dest.is_file() {
                std::fs::remove_file(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                forget_flat(map, &flat_name);
            }
            flatten_tree(
                &entry.path(),
                &dest,
                &flat_name,
                &format!("{real}/{name}"),
                map,
            )?;
        } else if kind.is_file() {
            if dest.is_dir() {
                std::fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                forget_flat(map, &flat_name);
            }
            std::fs::copy(entry.path(), &dest)
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
            map.insert(flat_name, format!("{real}/{name}"));
        }
    }
    Ok(())
}

/// Drops `flat` and every path under it from the view map.
fn forget_flat(map: &mut HashMap<String, String>, flat: &str) {
    map.remove(flat);
    let prefix = format!("{flat}/");
    map.retain(|key, _| !key.starts_with(&prefix));
}

/// Rewrites the flattened paths of a staged view's diagnostics back to the
/// mod's real files, line and column unchanged (`scripts/util.lua(1,1)`
/// becomes `scripts/shared/util.lua(1,1)`).
fn rewrite_paths(text: &str, staged: &StagedDir) -> String {
    let mut roots = vec![staged.path().display().to_string()];
    let canonical = canonical(staged.path()).display().to_string();
    if !roots.contains(&canonical) {
        roots.push(canonical);
    }
    let roots: Vec<String> = roots.into_iter().map(|r| r.replace('\\', "/")).collect();
    text.lines()
        .map(|line| {
            let mut line = line.to_string();
            for (flat, real) in &staged.map {
                for root in &roots {
                    line = replace_path(&line, &format!("{root}/{flat}"), real);
                }
                line = replace_path(&line, flat, real);
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replaces every whole `from` path in `line` with `to`: the match must not
/// continue a path before it or a name after it, so `scripts/a.lua` changes
/// neither `scripts/a.lua.bak` nor `x/scripts/a.lua`.
fn replace_path(line: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find(from) {
        out.push_str(&rest[..at]);
        let before_ok =
            !out.ends_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | '/'));
        let after = &rest[at + from.len()..];
        let after_ok = !after.starts_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | '.'));
        out.push_str(if before_ok && after_ok { to } else { from });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// A staged analysis directory, removed when its guard is dropped: when the
/// run ends, pass or fail, and when it returns early (a luau-lsp that cannot
/// be spawned, a staging step that fails) or panics.
struct StagedDir {
    path: PathBuf,
    /// Flattened side/shared files: `scripts/<name>` to the mod's real
    /// `scripts/...` (the registry, the data stage, is not in it).
    map: HashMap<String, String>,
}

impl StagedDir {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The staged `.luaurc`: the mod's own, with every setting kept and, when
/// `alias` is given, its `modkit` alias pointing at that target (an
/// `aliases` object is added when the mod has none). A mod without a
/// `.luaurc` gets only the alias. A mod without the alias keeps its own file.
///
/// With a `side`, every alias that points into this side's or the shared
/// folder is rewritten to where the flatten moved it: `scripts/server/x`
/// becomes `scripts/x`, `scripts/shared/x` becomes `scripts/x`. An alias into
/// the other side stays, and so does anything else (`scripts/registry`, the
/// modkit alias): those are not paths this view holds.
fn staged_luaurc_text(
    root: &Path,
    alias: Option<&str>,
    side: Option<Side>,
) -> Result<String, String> {
    let mut value = read_luaurc(root)?.unwrap_or_else(|| serde_json::json!({}));
    if let Some(alias) = alias {
        let not_an_object =
            || format!("{}: expected a JSON object", root.join(".luaurc").display());
        let object = value.as_object_mut().ok_or_else(not_an_object)?;
        let aliases = object
            .entry("aliases")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or_else(not_an_object)?;
        aliases.insert("modkit".into(), alias.into());
    }
    if let Some(side) = side
        && let Some(aliases) = value.get_mut("aliases").and_then(|a| a.as_object_mut())
    {
        for target in aliases.values_mut() {
            if let Some(text) = target.as_str() {
                let flat = flatten_alias(text, side);
                if flat != text {
                    *target = serde_json::Value::String(flat);
                }
            }
        }
    }
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    Ok(text + "\n")
}

/// Where an alias target points once `side`'s view is flattened: a target
/// into `scripts/<side>/` or `scripts/shared/` loses that folder (a leading
/// `./` and a trailing `/` are kept out of the way). Anything else is
/// returned unchanged.
fn flatten_alias(target: &str, side: Side) -> String {
    let rest = target.strip_prefix("./").unwrap_or(target);
    for folder in [side.folder(), "shared"] {
        let prefix = format!("scripts/{folder}");
        if let Some(tail) = rest.strip_prefix(&prefix)
            && (tail.is_empty() || tail.starts_with('/'))
        {
            return format!("scripts{tail}");
        }
    }
    target.to_string()
}

/// Folders a copy of a mod leaves out: build output, VCS and dependency and
/// editor folders.
const SKIPPED_FOLDERS: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    ".venv",
    ".idea",
    ".vscode",
];

/// Copies a mod's files into `to`, best effort: the folders of
/// `SKIPPED_FOLDERS`, the cache, and symlinks are left out, and a file that
/// cannot be copied is skipped rather than failing the check (luau-lsp then
/// reports whatever it actually needs). The cache and `to` are compared by
/// their real paths, so another spelling of either (a symlink, `./cache`)
/// cannot slip past; a folder inside `to` is never copied into `to`. Returns
/// whether the tree holds a folder named `RESERVED_MODKIT`.
fn copy_mod(from: &Path, to: &Path, cache: &Path) -> bool {
    copy_entries(from, to, &canonical(cache), &canonical(to))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn copy_entries(from: &Path, to: &Path, cache: &Path, staged: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(from) else {
        return false;
    };
    let mut reserved = false;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if SKIPPED_FOLDERS
            .iter()
            .any(|skip| name.as_os_str() == std::ffi::OsStr::new(skip))
        {
            continue;
        }
        let real = canonical(&path);
        if real.starts_with(cache) || real.starts_with(staged) {
            continue;
        }
        reserved |= name.as_os_str() == std::ffi::OsStr::new(RESERVED_MODKIT);
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let target = to.join(&name);
        if kind.is_dir() {
            if std::fs::create_dir_all(&target).is_ok() {
                reserved |= copy_entries(&path, &target, cache, staged);
            }
        } else if kind.is_file() {
            let _ = std::fs::copy(&path, &target);
        }
    }
    reserved
}

/// The folder a staged analysis holds the modkit checkout's scripts in. No
/// real mod uses this name, and the ignore pattern names it alone.
const RESERVED_MODKIT: &str = "__horizon_modkit__";

/// How many times a spawn that finds its executable busy is tried, 10 ms apart.
const BUSY_TRIES: u32 = 20;

/// Runs `spawn`, retrying while it fails with ETXTBSY ("text file busy"): a
/// binary just written can still be held open for writing by a fork in
/// another thread (its child has not exec'd yet), so the first exec may
/// find it busy. Any other error, or busy after `BUSY_TRIES`, is returned.
fn retry_busy<T>(mut spawn: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut tries = 1;
    loop {
        match spawn() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < BUSY_TRIES => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            done => return done,
        }
    }
}

/// Reads `url`: an http(s) URL over the network, anything else (`file://` or
/// a plain path) straight off the disk — tests serve fakes from a local path.
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    if let Some(path) = url
        .strip_prefix("file://")
        .or_else(|| (!url.starts_with("https://") && !url.starts_with("http://")).then_some(url))
    {
        return std::fs::read(path).map_err(|e| format!("{path}: {e}"));
    }
    // A stalled or endless server must not hang `check`: bounded connect and
    // overall time, and a size cap (the luau-lsp archive is a few MiB).
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(120))
        .build();
    let response = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    read_capped(response.into_reader(), MAX_DOWNLOAD).map_err(|e| format!("{url}: {e}"))
}

/// The largest download `fetch` accepts.
const MAX_DOWNLOAD: u64 = 64 << 20;

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Writes `contents` to `path` atomically: a temp file beside it, named
/// uniquely for this call (pid and a counter, so two threads never share
/// one), then renamed over `path`. A reader sees the old file or the new one,
/// never a truncated one. `executable` marks the temp file before the rename.
fn write_atomically(path: &Path, contents: &[u8], executable: bool) -> Result<(), String> {
    static TEMPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = TEMPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, contents)
        .map_err(|e| format!("{}: {e}", tmp.display()))
        .and_then(|()| {
            if executable {
                make_executable(&tmp);
            }
            std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Reads all of `reader` up to `cap` bytes; a longer body is refused.
fn read_capped(reader: impl std::io::Read, cap: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > cap {
        return Err(format!("larger than {cap} bytes: refusing to read it"));
    }
    Ok(bytes)
}

/// The file name of the luau-lsp binary on this platform.
fn exe_name() -> String {
    format!("luau-lsp{}", std::env::consts::EXE_SUFFIX)
}

/// The hash sidecar next to a cached binary (`luau-lsp.sha256`).
fn sidecar_path(exe: &Path) -> PathBuf {
    exe.with_extension("sha256")
}

/// What the cache holds for the pinned binary.
enum Cached {
    Missing,
    Good,
    Bad(String),
}

/// Whether the cached binary may run: missing when nothing is cached or it
/// has no record, good when its bytes match the SHA-256 its verified download
/// recorded and that record names the manifest's current pin, bad with the
/// reason otherwise.
fn check_cached(exe: &Path, pin: &str) -> Cached {
    if !exe.exists() {
        return Cached::Missing;
    }
    // No record: nothing vouches for the binary, so it is not cached. The
    // binary is left alone (another process may have just installed it) and
    // the download replaces it.
    let text = match std::fs::read_to_string(sidecar_path(exe)) {
        Ok(text) => text,
        Err(_) => return Cached::Missing,
    };
    let mut parts = text.split_whitespace();
    let (recorded, recorded_pin) = match (parts.next(), parts.next()) {
        (Some(binary), Some(pin)) => (binary, pin),
        _ => return Cached::Bad("its SHA-256 sidecar is malformed".into()),
    };
    if recorded_pin != pin {
        return Cached::Bad("the manifest pin changed since it was fetched".into());
    }
    let bytes = match std::fs::read(exe) {
        Ok(bytes) => bytes,
        Err(e) => return Cached::Bad(format!("it cannot be read: {e}")),
    };
    if sha256(&bytes) != recorded {
        return Cached::Bad("its bytes do not match its recorded SHA-256".into());
    }
    Cached::Good
}

/// Installs a verified binary: its sidecar first, then the binary, each
/// renamed into place. A reader that finds the binary finds a sidecar, and
/// one whose record names another binary (the old one, mid-install) is
/// refused and downloaded again, never trusted.
///
/// The sidecar records the binary's own SHA-256 and the manifest pin it was
/// verified against, `<binary-hex> <pin-hex>`. Only a download whose bytes
/// matched the pin writes it, so a matching record means: the binary on disk
/// equals the bytes such a download wrote. That protects against truncation,
/// partial writes, races, and replacement of the binary alone. It does not
/// protect against someone who can write the whole cache directory: they can
/// forge the binary and its record together.
fn install_binary(exe: &Path, contents: &[u8], pin: &str) -> Result<(), String> {
    let record = format!("{} {pin}\n", sha256(contents));
    write_atomically(&sidecar_path(exe), record.as_bytes(), false)?;
    write_atomically(exe, contents, true)
}

/// The largest entry unpacked: the binary is a few MiB.
const MAX_ENTRY: u64 = 256 << 20;

fn unpack(bytes: &[u8]) -> Result<Vec<u8>, String> {
    unpack_capped(bytes, MAX_ENTRY)
}

/// A `.zip` archive yields the bytes of its first entry whose file name is
/// the binary's (`luau-lsp`, or the exe name), refused past `cap` bytes.
/// Anything that does not parse as a zip, even one with bytes in front (a
/// self-extractor's stub), is the binary itself: its SHA-256 was verified.
fn unpack_capped(bytes: &[u8], cap: u64) -> Result<Vec<u8>, String> {
    let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) else {
        return Ok(bytes.to_vec());
    };
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("luau-lsp zip: {e}"))?;
        let name = entry.name().unwrap_or_default().to_string();
        // The file name (last component) must be the binary's, exactly.
        let file = name.rsplit(['/', '\\']).next().unwrap_or_default();
        if entry.is_dir() || (file != "luau-lsp" && file != exe_name()) {
            continue;
        }
        return read_capped(&mut entry, cap).map_err(|e| format!("luau-lsp zip entry: {e}"));
    }
    Err("luau-lsp zip: no luau-lsp entry in the archive".into())
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// The user cache dir for the pinned tools, `horizon-mod/tools` under it:
/// `$XDG_CACHE_HOME` (or `$HOME/.cache`), never inside the repo.
pub fn tools_cache(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        return Ok(dir.to_path_buf());
    }
    if let Some(dir) = std::env::var_os("HORIZON_MOD_TOOLS_CACHE") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        return Ok(dir);
    }
    let base = cache_base(std::env::consts::OS, |name| std::env::var_os(name)).ok_or(
        "no user cache dir: set XDG_CACHE_HOME or HOME (Windows: LOCALAPPDATA or USERPROFILE)",
    )?;
    let dir = base.join("horizon-mod").join("tools");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// The user cache base for `os` (`std::env::consts::OS`), read through `get`:
/// `XDG_CACHE_HOME` when set; on Windows `LOCALAPPDATA`, else
/// `USERPROFILE\AppData\Local`; elsewhere `HOME/.cache`.
fn cache_base(os: &str, get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(xdg) = get("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg));
    }
    if os == "windows" {
        let set = |name: &str| get(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        return set("LOCALAPPDATA")
            .or_else(|| set("USERPROFILE").map(|profile| profile.join("AppData").join("Local")));
    }
    get("HOME").map(|home| PathBuf::from(home).join(".cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake luau-lsp binary: a shell script that records its arguments next
    /// to itself (`<binary>.args`) and exits with `exit`.
    #[cfg(unix)]
    fn fake_binary(dir: &Path, exit: i32) -> PathBuf {
        let path = dir.join(format!("fake-luau-lsp-{exit}"));
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nexit {exit}\n"),
        )
        .unwrap();
        make_executable(&path);
        path
    }

    fn manifest(url: &str, sha256: &str) -> String {
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        format!(
            "version = \"1.2.3-test\"\nneeds_pin = false\n[target.{platform}]\nurl = \"{url}\"\nsha256 = \"{sha256}\"\n"
        )
    }

    /// Each test's own directories: tests run side by side in threads of one
    /// process (one pid), so a shared directory has them tread on each
    /// other's files and delete them mid-run. A leftover of an interrupted
    /// run is removed.
    fn case(test: &str) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("horizon-mod-scripts-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/init.lua"), "local ok = 1\n").unwrap();
        let cache = dir.join("cache");
        (dir, cache)
    }

    /// A mod with both sides and a shared module both entries require, and a
    /// registry script. Its own directory, no `scripts/init.lua`.
    fn per_side_mod(dir: &Path) {
        for (name, text) in [
            (
                "scripts/server/init.lua",
                "-- server entry\nlocal util = require(\"util\")\nreturn util\n",
            ),
            (
                "scripts/client/init.lua",
                "-- client entry\nlocal util = require(\"util\")\nreturn util\n",
            ),
            (
                "scripts/shared/util.lua",
                "-- shared util\nlocal M = {}\nreturn M\n",
            ),
            ("scripts/registry.lua", "return {}\n"),
        ] {
            let file = dir.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    }

    fn per_side_case(test: &str) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("horizon-mod-sides-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        per_side_mod(&dir);
        let cache = dir.join("cache");
        (dir, cache)
    }

    /// Every file under `dir`, named from `prefix` with `/` separators.
    fn files_under(dir: &Path, prefix: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = format!("{prefix}/{name}");
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                out.extend(files_under(&entry.path(), &path));
            } else {
                out.push(path);
            }
        }
        out.sort();
        out
    }

    fn fake_script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-luau-lsp");
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        make_executable(&path);
        path
    }

    /// A matching SHA-256 is accepted: the fake binary is fetched from a
    /// local path, cached under the cache dir (never the repo), and run with
    /// the horizon definitions.
    #[cfg(unix)]
    #[test]
    fn the_download_accepts_a_matching_sha256_and_caches_the_binary() {
        let (dir, cache) = case("matching-sha256");
        let binary = fake_binary(&dir, 0);
        let bytes = std::fs::read(&binary).unwrap();
        let text = manifest(&binary.display().to_string(), &sha256(&bytes));
        let outcome = analyze_with(
            &text,
            &dir,
            &["scripts/init.lua".into()],
            &[],
            None,
            &cache,
            false,
            None,
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Pass), "{outcome:?}");
        let cached = cache
            .join("luau-lsp")
            .join("1.2.3-test")
            .join(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .join(exe_name());
        assert!(cached.exists(), "not cached at {}", cached.display());
        assert!(cached.starts_with(&cache));
        // The cached copy ran (it records its own arguments beside itself).
        let args = std::fs::read_to_string(cached.with_extension("args")).unwrap();
        assert!(args.contains("--definitions:@horizon="), "{args}");
        assert!(args.contains("scripts/init.lua"), "{args}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A truncated or replaced cached binary is refused, not run: its bytes
    /// no longer match the sidecar its verified download wrote. Offline the
    /// step fails with a clear message (and never executes it); online the
    /// bad copy is deleted and fetched again.
    #[cfg(unix)]
    #[test]
    fn a_corrupt_cached_binary_is_refused_not_run() {
        let (dir, cache) = case("corrupt-cache");
        let binary = fake_binary(&dir, 0);
        let bytes = std::fs::read(&binary).unwrap();
        let text = manifest(&binary.display().to_string(), &sha256(&bytes));
        let scripts = vec!["scripts/init.lua".to_string()];
        assert!(matches!(
            analyze_with(&text, &dir, &scripts, &[], None, &cache, false, None).unwrap(),
            Outcome::Pass
        ));
        let cached = cache
            .join("luau-lsp")
            .join("1.2.3-test")
            .join(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .join(exe_name());
        assert!(sidecar_path(&cached).exists(), "no SHA-256 sidecar written");
        // Replace the binary with different (valid) bytes, as a truncated
        // download or a racing run would: the hash no longer matches.
        std::fs::write(&cached, "#!/bin/sh\nexit 0\n").unwrap();
        let _ = std::fs::remove_file(cached.with_extension("args"));
        // Offline, a bad cache is a skip with a notice (the same as no cache):
        // it is never run, and an online run fetches it again.
        match analyze_with(&text, &dir, &scripts, &[], None, &cache, true, None).unwrap() {
            Outcome::Skip(why) => {
                assert!(why.contains("unusable") && why.contains("SHA-256"), "{why}")
            }
            other => panic!("a corrupt cached binary is skipped offline, got {other:?}"),
        }
        // Refused, not run: the fake records its arguments beside itself
        // when it executes, and nothing did.
        assert!(
            !cached.with_extension("args").exists(),
            "the corrupt binary ran"
        );
        // Online the bad copy is fetched again and the step passes.
        assert!(matches!(
            analyze_with(&text, &dir, &scripts, &[], None, &cache, false, None).unwrap(),
            Outcome::Pass
        ));
        assert!(cached.with_extension("args").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Staging gives every run its own directory under the cache, holding
    /// that run's scripts: two stages never share (the old shared
    /// `cache/analysis` had parallel runs corrupt each other), and two
    /// staged analyses in parallel threads each see their own mod and leave
    /// no staged directory behind, pass or fail.
    #[cfg(unix)]
    #[test]
    fn staged_runs_each_get_their_own_directory() {
        let base = std::env::temp_dir().join(format!("horizon-mod-staged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let kit = base.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        let root = |marker: &str| {
            let root = base.join(format!("mod-{marker}"));
            std::fs::create_dir_all(root.join("scripts")).unwrap();
            std::fs::write(
                root.join("scripts/init.lua"),
                format!("local marker = \"{marker}\"\n"),
            )
            .unwrap();
            root
        };
        let cache = base.join("cache");
        // Sequential stages do not overwrite each other.
        let root_a = root("alpha");
        let root_b = root("beta");
        let dir_a = stage_analysis(&root_a, &cache, Some(&kit), None).unwrap();
        let dir_b = stage_analysis(&root_b, &cache, Some(&kit), None).unwrap();
        assert_ne!(dir_a.path(), dir_b.path(), "staged runs share a directory");
        let staged = |dir: &Path| std::fs::read_to_string(dir.join("scripts/init.lua")).unwrap();
        assert!(
            staged(dir_a.path()).contains("alpha"),
            "{}",
            staged(dir_a.path())
        );
        assert!(
            staged(dir_b.path()).contains("beta"),
            "{}",
            staged(dir_b.path())
        );
        // Dropping a guard removes its directory.
        let (path_a, path_b) = (dir_a.path().to_path_buf(), dir_b.path().to_path_buf());
        drop(dir_a);
        drop(dir_b);
        assert!(!path_a.exists() && !path_b.exists());
        // Parallel staged analyses through the whole step, sharing one
        // cache as two `check` runs do: each sees its own mod.
        let fake = |marker: &str| {
            let path = base.join(format!("fake-luau-lsp-{marker}"));
            std::fs::write(
                &path,
                "#!/bin/sh\npwd > \"$0.pwd\"\ncat scripts/init.lua > \"$0.content\"\nexit 0\n",
            )
            .unwrap();
            make_executable(&path);
            path
        };
        std::thread::scope(|s| {
            for marker in ["alpha", "beta"] {
                let root = root(marker);
                let binary = fake(marker);
                let cache = cache.clone();
                let kit = kit.clone();
                s.spawn(move || {
                    let scripts = vec!["scripts/init.lua".to_string()];
                    match analyze_with(
                        &manifest("unused", &"0".repeat(64)),
                        &root,
                        &scripts,
                        &[],
                        Some(&binary),
                        &cache,
                        false,
                        Some(&kit),
                    )
                    .unwrap()
                    {
                        Outcome::Pass => {}
                        other => panic!("staged {marker} passes, got {other:?}"),
                    }
                    let seen = std::fs::read_to_string(binary.with_extension("content")).unwrap();
                    assert!(
                        seen.contains(marker)
                            && !seen.contains(if marker == "alpha" { "beta" } else { "alpha" }),
                        "{marker} saw another run's scripts: {seen}"
                    );
                });
            }
        });
        // Nothing staged is left behind.
        for entry in std::fs::read_dir(&cache).unwrap() {
            let name = entry.unwrap().file_name();
            assert!(
                !name.to_str().is_some_and(|n| n.starts_with("analysis")),
                "staged directory left behind: {name:?}"
            );
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A mismatching SHA-256 is refused loudly and nothing is cached.
    #[test]
    fn the_download_refuses_a_mismatching_sha256() {
        let (dir, cache) = case("mismatching-sha256");
        let binary = dir.join("not-a-binary");
        std::fs::write(&binary, b"these bytes are never run").unwrap();
        let text = manifest(&binary.display().to_string(), &"f".repeat(64));
        match analyze_with(
            &text,
            &dir,
            &[] as &[String],
            &[],
            None,
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Fail(why) => {
                assert!(why.contains("SHA-256 mismatch"), "{why}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(!cache.join("luau-lsp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The types step with the real luau-lsp 1.70.1 and the modkit at a deep
    /// path: the modkit's own type errors are ignored, a real error in the
    /// mod still fails. The cache sits under the checkout's grandparent (as
    /// the user cache does under `$HOME`), so the relative paths luau-lsp
    /// reports hold no ancestor segment for an ancestor-named glob: only the
    /// checkout's own name matches. Run it with the modkit at hand:
    /// `MODKIT=/path/to/horizon-modkit cargo nextest run -p cargo-horizon-mod --run-ignored only`
    /// (needs the network once, or a cached luau-lsp binary).
    #[test]
    #[ignore]
    fn types_step_ignores_a_deep_modkit_checkout_but_reports_mod_errors() {
        let modkit = std::env::var_os("MODKIT")
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("set MODKIT=<horizon-modkit checkout> to run this test"));
        let base =
            std::env::temp_dir().join(format!("horizon-mod-deep-ignore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // The mod: the Lua template's server entry and shared module with a
        // fixed id, and a `.luaurc` whose sibling `@modkit` alias is missing,
        // so the check stages the analysis through the `--modkit` checkout
        // below.
        let root = base.join("mod");
        let template = |name: &str| {
            std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("template/lua")
                    .join(name),
            )
            .unwrap()
        };
        std::fs::create_dir_all(root.join("scripts/server")).unwrap();
        std::fs::create_dir_all(root.join("scripts/shared")).unwrap();
        std::fs::write(
            root.join("scripts/server/init.lua"),
            template("server_init.lua").replace("{id}", "deep_probe"),
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/shared/util.lua"),
            template("shared_util.lua"),
        )
        .unwrap();
        std::fs::write(
            root.join(".luaurc"),
            "{\"languageMode\": \"nonstrict\", \"aliases\": \
             {\"modkit\": \"../horizon-modkit/scripts/server/modkit\", \
             \"registry\": \"scripts/registry\"}}",
        )
        .unwrap();
        // The modkit at a deep path, the cache under its grandparent.
        let deep = base.join("u/src/a/b/horizon-modkit");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        copy_mod(&modkit, &deep, Path::new("/nonexistent-cache"));
        let cache = base.join("u/src/a/cache");
        let scripts = vec![
            "scripts/server/init.lua".to_string(),
            "scripts/shared/util.lua".to_string(),
        ];
        match analyze(
            &root,
            &scripts,
            &[Side::Server],
            None,
            &cache,
            false,
            Some(&deep),
        )
        .unwrap()
        {
            Outcome::Pass => {}
            other => panic!("a clean mod with a deep modkit passes, got {other:?}"),
        }
        // A real error in the mod still fails, naming the mod's file and
        // none of the modkit's.
        let init = root.join("scripts/server/init.lua");
        let source = std::fs::read_to_string(&init).unwrap();
        std::fs::write(&init, format!("horizon.set_block(\"a\")\n{source}")).unwrap();
        match analyze(
            &root,
            &scripts,
            &[Side::Server],
            None,
            &cache,
            false,
            Some(&deep),
        )
        .unwrap()
        {
            Outcome::Fail(why) => {
                // `[INFO]` lines name files luau-lsp merely read (including
                // the checkout); only diagnostic lines count.
                let diags: Vec<&str> = why.lines().filter(|l| !l.starts_with("[INFO]")).collect();
                assert!(
                    diags.iter().any(|l| l.contains("scripts/server/init.lua")),
                    "{why}"
                );
                assert!(!diags.iter().any(|l| l.contains("horizon-modkit")), "{why}");
            }
            other => panic!("a type error in the mod fails, got {other:?}"),
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The staged `.luaurc` keeps every setting of the mod's own, and points
    /// the `modkit` alias at the staged checkout, with or without an
    /// `aliases` object to merge into.
    #[test]
    fn a_staged_luaurc_keeps_the_mods_settings() {
        let (dir, cache) = case("luaurc-settings");
        let kit = dir.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        let staged_luaurc = |text: &str| {
            std::fs::write(dir.join(".luaurc"), text).unwrap();
            let staged = stage_analysis(&dir, &cache, Some(&kit), None).unwrap();
            let text = std::fs::read_to_string(staged.path().join(".luaurc")).unwrap();
            serde_json::from_str::<serde_json::Value>(&text).unwrap()
        };
        let no_aliases = staged_luaurc("{\"languageMode\": \"strict\"}");
        assert_eq!(no_aliases["languageMode"], "strict", "{no_aliases}");
        assert_eq!(
            no_aliases["aliases"]["modkit"], RESERVED_MODKIT,
            "{no_aliases}"
        );
        // luau-lsp accepts comments and trailing commas; the `//` inside the
        // string is text, not a comment.
        let jsonc = staged_luaurc(
            "{\n  // the mode\n  \"languageMode\": \"nonstrict\", /* block */\n  \
             \"aliases\": {\"lib\": \"shared\",},\n  \"note\": \"a // b\",\n}\n",
        );
        assert_eq!(jsonc["languageMode"], "nonstrict", "{jsonc}");
        assert_eq!(jsonc["aliases"]["lib"], "shared", "{jsonc}");
        assert_eq!(jsonc["aliases"]["modkit"], RESERVED_MODKIT, "{jsonc}");
        assert_eq!(jsonc["note"], "a // b", "{jsonc}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `.luaurc` that does not parse (even after the JSONC allowances) is an
    /// error naming the file, not a silent fallback that drops its settings.
    #[test]
    fn a_luaurc_that_does_not_parse_is_an_error_naming_it() {
        let (dir, cache) = case("luaurc-broken");
        std::fs::write(dir.join(".luaurc"), "{ \"aliases\": ").unwrap();
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/init.lua".into()],
            &[],
            Some(Path::new("/no/such/binary")),
            &cache,
            false,
            None,
        );
        match outcome {
            Err(e) => assert!(e.contains(".luaurc"), "{e}"),
            other => panic!("expected an error naming .luaurc, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `copy_mod` leaves out the cache even when it is reached through another
    /// spelling of its path (a symlink here): the copy never holds a copy of
    /// its own cache.
    #[cfg(unix)]
    #[test]
    fn copy_mod_leaves_out_the_cache_whatever_its_spelling() {
        let base =
            std::env::temp_dir().join(format!("horizon-mod-copy-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("mod");
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join("cache/horizon.d.lua"), "x").unwrap();
        std::fs::write(root.join("keep.lua"), "y").unwrap();
        std::os::unix::fs::symlink(&root, base.join("alias")).unwrap();
        let to = base.join("stage");
        std::fs::create_dir_all(&to).unwrap();
        copy_mod(&root, &to, &base.join("alias").join("cache"));
        assert!(to.join("keep.lua").is_file());
        assert!(!to.join("cache").exists(), "the cache was copied");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Dependency and editor folders are not copied with the mod.
    #[test]
    fn copy_mod_leaves_out_dependency_and_editor_folders() {
        let base =
            std::env::temp_dir().join(format!("horizon-mod-copy-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let from = base.join("mod");
        for folder in [
            "node_modules",
            ".venv",
            ".idea",
            ".vscode",
            "target",
            ".git",
        ] {
            std::fs::create_dir_all(from.join(folder)).unwrap();
            std::fs::write(from.join(folder).join("x"), "x").unwrap();
        }
        std::fs::write(from.join("keep.lua"), "y").unwrap();
        let to = base.join("stage");
        std::fs::create_dir_all(&to).unwrap();
        copy_mod(&from, &to, &base.join("cache"));
        assert!(to.join("keep.lua").is_file());
        for folder in [
            "node_modules",
            ".venv",
            ".idea",
            ".vscode",
            "target",
            ".git",
        ] {
            assert!(!to.join(folder).exists(), "{folder} was copied");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A luau-lsp failure without the word `Error` still fails the step: with
    /// no modkit, only the `Unknown require` diagnostics for the unresolvable
    /// `@modkit` alias are dropped, and a remaining line like
    /// `Cannot get <path>: path does not exist` (exit 1) is a failure.
    #[cfg(unix)]
    #[test]
    fn a_non_error_luau_lsp_failure_still_fails_the_step() {
        let (dir, cache) = case("unresolvable-error");
        let binary = dir.join("fake-luau-lsp-missing");
        std::fs::write(
            &binary,
            "#!/bin/sh\necho 'Cannot get x: path does not exist'\nexit 1\n",
        )
        .unwrap();
        make_executable(&binary);
        std::fs::write(
            dir.join(".luaurc"),
            "{\"aliases\": {\"modkit\": \"../horizon-modkit/scripts/modkit\"}}",
        )
        .unwrap();
        match analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/init.lua".into()],
            &[],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Fail(why) => assert!(why.contains("Cannot get"), "{why}"),
            other => panic!("a missing-path failure fails, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A luau-lsp that fails with only `[INFO]` lines still fails the step:
    /// the failure names its exit code and keeps the output.
    #[cfg(unix)]
    #[test]
    fn an_info_only_luau_lsp_failure_names_its_exit_code() {
        let (dir, cache) = case("info-only-failure");
        let binary = fake_script(
            &dir,
            "printf '%s\\n' '[INFO] read scripts/init.lua'\nexit 3\n",
        );
        match analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/init.lua".into()],
            &[],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Fail(why) => {
                assert!(why.contains("luau-lsp analyze failed (exit 3)"), "{why}");
                assert!(why.contains("[INFO] read scripts/init.lua"), "{why}");
            }
            other => panic!("an INFO-only failure fails the step, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `.zip` release archive is unpacked to the binary.
    #[cfg(unix)]
    #[test]
    fn the_download_unpacks_a_zip_archive() {
        use std::io::Write as _;
        let (dir, cache) = case("zip-archive");
        let mut archive = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut archive);
            zip.start_file("luau-lsp", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
            zip.finish().unwrap();
        }
        let archive_path = dir.join("luau-lsp.zip");
        std::fs::write(&archive_path, archive.into_inner()).unwrap();
        let text = manifest(
            &archive_path.display().to_string(),
            &sha256(&std::fs::read(&archive_path).unwrap()),
        );
        assert!(matches!(
            analyze_with(
                &text,
                &dir,
                &[] as &[String],
                &[],
                None,
                &cache,
                false,
                None
            )
            .unwrap(),
            Outcome::Pass
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A checkout works whatever its folder is called (`scripts`, `shared`,
    /// `modkit`, `Scripts`): it is staged under the reserved name, and the
    /// ignore pattern names that folder only.
    #[cfg(unix)]
    #[test]
    fn a_checkout_of_any_name_is_staged_under_the_reserved_name() {
        let (dir, cache) = case("any-checkout-name");
        let binary = fake_binary(&dir, 0);
        let scripts = vec!["scripts/init.lua".to_string()];
        for name in ["scripts", "shared", "modkit", "Scripts"] {
            let kit = dir.join("kits").join(name);
            std::fs::create_dir_all(&kit).unwrap();
            std::fs::write(kit.join("init.lua"), "return {}\n").unwrap();
            let staged = stage_analysis(&dir, &cache, Some(&kit), None).unwrap();
            assert!(
                staged
                    .path()
                    .join(RESERVED_MODKIT)
                    .join("init.lua")
                    .is_file(),
                "{name}: the checkout is not staged"
            );
            drop(staged);
            let outcome = analyze_with(
                &manifest("unused", &"0".repeat(64)),
                &dir,
                &scripts,
                &[],
                Some(&binary),
                &cache,
                false,
                Some(&kit),
            );
            assert!(matches!(outcome, Ok(Outcome::Pass)), "{name}: {outcome:?}");
            let args = std::fs::read_to_string(binary.with_extension("args")).unwrap();
            assert!(
                args.lines()
                    .any(|l| l == format!("--ignore=**/{RESERVED_MODKIT}/**")),
                "{name}: {args}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A failing analysis fails the step; offline without the binary skips.
    #[cfg(unix)]
    #[test]
    fn a_failing_analysis_fails_the_step() {
        let (dir, cache) = case("failing-analysis");
        let binary = fake_binary(&dir, 1);
        match analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &[] as &[String],
            &[],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Fail(why) => assert!(why.contains("luau-lsp analyze"), "{why}"),
            other => panic!("expected a failure, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_placeholder_pin_and_offline_skip_instead_of_failing() {
        let (dir, cache) = case("offline-skip");
        // The checked-in manifest is a real pin: offline with nothing cached skips.
        match analyze(&dir, &[] as &[String], &[], None, &cache, true, None).unwrap() {
            Outcome::Skip(why) => assert!(why.contains("offline"), "{why}"),
            other => panic!("expected a skip, got {other:?}"),
        }
        // A manifest that still needs a pin skips with the pin notice.
        let placeholder = format!(
            "version = \"0\"\nneeds_pin = true\n[target.{}-{}]\nurl = \"x\"\nsha256 = \"{}\"\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            "0".repeat(64)
        );
        match analyze_with(
            &placeholder,
            &dir,
            &[] as &[String],
            &[],
            None,
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Skip(why) => assert!(why.contains("needs pin"), "{why}"),
            other => panic!("expected a skip, got {other:?}"),
        }
        // A real pin, but offline and not cached.
        let text = manifest("https://example.invalid/luau-lsp.zip", &"0".repeat(64));
        match analyze_with(&text, &dir, &[] as &[String], &[], None, &cache, true, None).unwrap() {
            Outcome::Skip(why) => assert!(why.contains("offline"), "{why}"),
            other => panic!("expected a skip, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A staged directory is removed on every way out of the run: a luau-lsp
    /// that cannot be spawned, and a staging that fails after its directory
    /// was created both leave nothing under the cache.
    #[test]
    fn a_failed_run_leaves_no_staged_directory() {
        let (dir, cache) = case("staged-cleanup");
        let kit = dir.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        let result = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/init.lua".into()],
            &[],
            Some(Path::new("/no/such/binary")),
            &cache,
            false,
            Some(&kit),
        );
        assert!(result.is_err(), "{result:?}");
        let leftovers: Vec<_> = std::fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_str().is_some_and(|n| n.starts_with("analysis")))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staged directories left: {leftovers:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A busy executable (ETXTBSY: another thread's fork still holds a copy of
    /// the write descriptor of the binary just written) is retried: two busy
    /// answers then success is a success; busy forever is the error, after
    /// the cap and no sooner.
    #[test]
    fn a_busy_executable_is_retried_up_to_the_cap() {
        let busy = || std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy);
        let mut calls = 0;
        let out = retry_busy(|| {
            calls += 1;
            if calls <= 2 { Err(busy()) } else { Ok(calls) }
        });
        assert_eq!(out.unwrap(), 3, "two busy answers, then the spawn");
        let mut calls = 0;
        let out: std::io::Result<()> = retry_busy(|| {
            calls += 1;
            Err(busy())
        });
        assert_eq!(
            out.unwrap_err().kind(),
            std::io::ErrorKind::ExecutableFileBusy
        );
        assert_eq!(calls, BUSY_TRIES, "retried up to the cap, then given up");
        // Any other error is not retried.
        let mut calls = 0;
        let out: std::io::Result<()> = retry_busy(|| {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert!(out.is_err());
        assert_eq!(calls, 1);
    }

    /// Two threads run the download path at once against one cache and a
    /// local file URL. Both succeed, and the cache ends valid: its binary
    /// matches the sidecar the download recorded. Each round starts empty.
    #[cfg(unix)]
    #[test]
    fn racing_downloads_both_succeed_and_leave_a_valid_cache() {
        let (dir, cache) = case("racing-download");
        let binary = fake_binary(&dir, 0);
        let sha = sha256(&std::fs::read(&binary).unwrap());
        let text = manifest(&binary.display().to_string(), &sha);
        let scripts = vec!["scripts/init.lua".to_string()];
        let exe = cache
            .join("luau-lsp")
            .join("1.2.3-test")
            .join(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .join(exe_name());
        for round in 0..20 {
            let _ = std::fs::remove_dir_all(cache.join("luau-lsp"));
            std::thread::scope(|s| {
                let runs: Vec<_> = (0..2)
                    .map(|_| {
                        s.spawn(|| {
                            analyze_with(&text, &dir, &scripts, &[], None, &cache, false, None)
                        })
                    })
                    .collect();
                for run in runs {
                    match run.join().unwrap() {
                        Ok(Outcome::Pass) => {}
                        other => panic!("round {round}: {other:?}"),
                    }
                }
            });
            assert!(
                matches!(check_cached(&exe, &sha), Cached::Good),
                "round {round}: the cache does not end valid"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A cached binary whose sidecar is missing is not cached: it is fetched
    /// again, and the binary is not deleted on the way.
    #[test]
    fn a_binary_without_its_sidecar_is_not_cached() {
        let dir =
            std::env::temp_dir().join(format!("horizon-mod-no-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(exe_name());
        std::fs::write(&exe, b"bytes").unwrap();
        assert!(matches!(
            check_cached(&exe, &"0".repeat(64)),
            Cached::Missing
        ));
        assert!(exe.exists(), "the binary was deleted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An atomic write leaves exactly the file behind: its temp file is
    /// renamed into place, never kept beside it.
    #[test]
    fn an_atomic_write_leaves_only_the_file() {
        let dir = std::env::temp_dir().join(format!("horizon-mod-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("horizon.d.lua");
        write_atomically(&path, b"first", false).unwrap();
        write_atomically(&path, b"second", false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["horizon.d.lua"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The download is read up to its cap: a body of exactly the cap is read
    /// whole, one byte more is refused.
    #[test]
    fn a_download_is_read_up_to_the_cap_only() {
        assert_eq!(read_capped(&b"abcd"[..], 4).unwrap(), b"abcd");
        let err = read_capped(&b"abcde"[..], 4).unwrap_err();
        assert!(err.contains("larger than 4 bytes"), "{err}");
    }

    /// The zip entry is the one whose file name is `luau-lsp` (or the exe
    /// name), not the first whose name merely contains it: a README listed
    /// first is skipped.
    #[test]
    fn the_zip_entry_is_named_luau_lsp_not_merely_contains_it() {
        use std::io::Write as _;
        let mut archive = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut archive);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("README-luau-lsp.txt", options).unwrap();
            zip.write_all(b"a decoy").unwrap();
            zip.start_file("luau-lsp-1.70.1/bin/luau-lsp", options)
                .unwrap();
            zip.write_all(b"the binary").unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(unpack(&archive.into_inner()).unwrap(), b"the binary");
    }

    /// A staged analysis sees the whole mod, not only its scripts: a
    /// `.luaurc` alias to another folder (`shared/`) resolves as it does in
    /// place, and a dangling symlink in the mod does not fail the staging.
    #[cfg(unix)]
    #[test]
    fn a_staged_analysis_sees_the_whole_mod() {
        let (dir, cache) = case("staged-whole-mod");
        std::fs::create_dir_all(dir.join("shared")).unwrap();
        std::fs::write(dir.join("shared/util.lua"), "return {}\n").unwrap();
        std::fs::write(dir.join(".luaurc"), "{\"aliases\": {\"lib\": \"shared\"}}").unwrap();
        std::os::unix::fs::symlink(dir.join("nowhere.lua"), dir.join("scripts/dangling.lua"))
            .unwrap();
        // Stands in for luau-lsp: it needs `shared/util.lua` from its folder.
        let fake = dir.join("fake-luau-lsp");
        std::fs::write(
            &fake,
            "#!/bin/sh\n[ -f shared/util.lua ] || { echo 'TypeError: Unknown require: shared/util.lua'; exit 1; }\nexit 0\n",
        )
        .unwrap();
        make_executable(&fake);
        let kit = dir.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/init.lua".into()],
            &[],
            Some(&fake),
            &cache,
            false,
            Some(&kit),
        );
        match outcome.unwrap() {
            Outcome::Pass => {}
            other => panic!("a staged mod with an alias to shared/ passes, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A side's staged view holds the shared files flattened in and the
    /// side's own files over them: a same-named side file wins, and only the
    /// mod's real layout is left in the map.
    #[test]
    fn a_side_view_flattens_shared_under_the_side_files() {
        let (dir, cache) = per_side_case("view");
        std::fs::write(
            dir.join("scripts/server/util.lua"),
            "-- server utility\nreturn { server = true }\n",
        )
        .unwrap();
        let kit = dir.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        let expected = [
            "scripts/init.lua",
            "scripts/registry.lua",
            "scripts/util.lua",
        ];
        let server = stage_analysis(&dir, &cache, Some(&kit), Some(Side::Server)).unwrap();
        assert_eq!(
            files_under(&server.path().join("scripts"), "scripts"),
            expected
        );
        let util = std::fs::read_to_string(server.path().join("scripts/util.lua")).unwrap();
        assert!(util.contains("server utility"), "{util}");
        assert_eq!(
            server.map["scripts/util.lua"], "scripts/server/util.lua",
            "a side file must win the shared one"
        );
        assert_eq!(server.map["scripts/init.lua"], "scripts/server/init.lua");
        drop(server);
        let client = stage_analysis(&dir, &cache, Some(&kit), Some(Side::Client)).unwrap();
        assert_eq!(
            files_under(&client.path().join("scripts"), "scripts"),
            expected
        );
        let util = std::fs::read_to_string(client.path().join("scripts/util.lua")).unwrap();
        assert!(util.contains("shared util"), "{util}");
        assert_eq!(client.map["scripts/util.lua"], "scripts/shared/util.lua");
        assert_eq!(client.map["scripts/init.lua"], "scripts/client/init.lua");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A side's file wins a shared folder of the same name, and a side's
    /// folder wins a shared file: the flatten removes what is in the way and
    /// the map drops the loser's paths.
    #[cfg(unix)]
    #[test]
    fn a_side_file_wins_a_shared_directory_and_the_reverse() {
        let (dir, cache) = per_side_case("file-vs-dir");
        // Shared has the folder `thing/`; the server has the file `thing`.
        std::fs::create_dir_all(dir.join("scripts/shared/thing")).unwrap();
        std::fs::write(dir.join("scripts/shared/thing/old.lua"), "return {}\n").unwrap();
        std::fs::write(dir.join("scripts/server/thing"), "-- the side file\n").unwrap();
        let server = stage_analysis(&dir, &cache, None, Some(Side::Server)).unwrap();
        assert!(server.path().join("scripts/thing").is_file());
        assert!(!server.path().join("scripts/thing/old.lua").exists());
        assert_eq!(server.map["scripts/thing"], "scripts/server/thing");
        assert!(!server.map.contains_key("scripts/thing/old.lua"));
        drop(server);
        // Shared has the file `other`; the server has the folder `other/`.
        std::fs::write(dir.join("scripts/shared/other"), "-- the shared file\n").unwrap();
        std::fs::create_dir_all(dir.join("scripts/server/other")).unwrap();
        std::fs::write(dir.join("scripts/server/other/new.lua"), "return {}\n").unwrap();
        let server = stage_analysis(&dir, &cache, None, Some(Side::Server)).unwrap();
        assert!(server.path().join("scripts/other").is_dir());
        assert!(server.path().join("scripts/other/new.lua").is_file());
        assert_eq!(
            server.map["scripts/other/new.lua"],
            "scripts/server/other/new.lua"
        );
        assert!(!server.map.contains_key("scripts/other"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A flatten that cannot read a folder fails the check, naming the path:
    /// a truncated view must not pass or fail for the wrong reason.
    #[cfg(unix)]
    #[test]
    fn a_failed_flatten_names_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, cache) = per_side_case("flatten-error");
        let locked = dir.join("scripts/shared/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("x.lua"), "return {}\n").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // A root user reads it anyway: nothing to prove there.
        if std::fs::read_dir(&locked).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }
        let result = stage_analysis(&dir, &cache, None, Some(Side::Server));
        // Restore before the guard's cleanup walks the tree.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        match result {
            Err(e) => assert!(e.contains("locked"), "{e}"),
            Ok(_) => panic!("an unreadable folder must fail the flatten"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The staged `.luaurc` of a side points its aliases where the flatten
    /// moved them: this side's and the shared folder lose their folder, the
    /// other side's and everything else stay. The fake analyzer records the
    /// staged `.luaurc` of each run.
    #[cfg(unix)]
    #[test]
    fn a_side_staged_luaurc_rewrites_aliases_into_the_view() {
        let (dir, cache) = per_side_case("alias-rewrite");
        std::fs::write(
            dir.join(".luaurc"),
            r#"{"aliases": {
                "own": "scripts/server/example",
                "dot": "./scripts/server/dot",
                "trail": "scripts/shared/trail/",
                "shared": "scripts/shared/x",
                "other": "scripts/client/other",
                "registry": "scripts/registry"
            }}"#,
        )
        .unwrap();
        let binary = fake_script(
            &dir,
            "tr '\\n' ' ' < .luaurc >> \"$0.luaurc\"\nprintf '\\n--\\n' >> \"$0.luaurc\"\nexit 0\n",
        );
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/server/init.lua".into()],
            &[Side::Server, Side::Client],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Pass), "{outcome:?}");
        let seen = std::fs::read_to_string(binary.with_extension("luaurc")).unwrap();
        let docs: Vec<serde_json::Value> = seen
            .split("\n--\n")
            .filter(|doc| !doc.trim().is_empty())
            .map(|doc| serde_json::from_str(doc.trim()).unwrap())
            .collect();
        assert_eq!(docs.len(), 2, "{seen}");
        let server = &docs[0]["aliases"];
        assert_eq!(server["own"], "scripts/example", "{server}");
        assert_eq!(server["dot"], "scripts/dot", "{server}");
        assert_eq!(server["trail"], "scripts/trail/", "{server}");
        assert_eq!(server["shared"], "scripts/x", "{server}");
        assert_eq!(server["other"], "scripts/client/other", "{server}");
        assert_eq!(server["registry"], "scripts/registry", "{server}");
        let client = &docs[1]["aliases"];
        assert_eq!(client["own"], "scripts/server/example", "{client}");
        assert_eq!(client["other"], "scripts/other", "{client}");
        assert_eq!(client["trail"], "scripts/trail/", "{client}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The analyzer runs once per side, in that side's own staged directory,
    /// on the flattened `scripts/` the runtime's requires resolve in.
    #[cfg(unix)]
    #[test]
    fn the_analyzer_runs_once_per_side_in_its_own_view() {
        let (dir, cache) = per_side_case("runs");
        let binary = fake_script(
            &dir,
            "pwd >> \"$0.pwd\"\nprintf '%s\\n' \"$@\" > \"$0.args\"\n\
             cat scripts/init.lua >> \"$0.content\"\nprintf '\\n--\\n' >> \"$0.content\"\n\
             exit 0\n",
        );
        let scripts = vec![
            "scripts/server/init.lua".to_string(),
            "scripts/client/init.lua".to_string(),
            "scripts/shared/util.lua".to_string(),
        ];
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &scripts,
            &[Side::Server, Side::Client],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Pass), "{outcome:?}");
        let dirs = std::fs::read_to_string(binary.with_extension("pwd")).unwrap();
        let dirs: Vec<&str> = dirs.lines().collect();
        assert_eq!(dirs.len(), 2, "one run per side: {dirs:?}");
        assert_ne!(dirs[0], dirs[1], "both sides share a view");
        assert!(
            dirs.iter().all(|d| d.contains("analysis-")),
            "a run left the staged views: {dirs:?}"
        );
        let seen = std::fs::read_to_string(binary.with_extension("content")).unwrap();
        let seen: Vec<&str> = seen.split("\n--\n").collect();
        assert!(seen[0].contains("server entry"), "{seen:?}");
        assert!(seen[1].contains("client entry"), "{seen:?}");
        let args = std::fs::read_to_string(binary.with_extension("args")).unwrap();
        assert!(args.lines().any(|l| l == "scripts"), "{args}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A require the side's view resolves is not a missing file: luau-lsp
    /// looks for an entry's requires one folder up and takes a dotted name as
    /// a literal file name, the runtime resolves both from the state root.
    /// A require that is really absent still fails.
    #[cfg(unix)]
    #[test]
    fn a_require_the_view_resolves_is_not_a_missing_file() {
        let (dir, cache) = per_side_case("resolved-requires");
        let module = dir.join("scripts/shared/example/command.lua");
        std::fs::create_dir_all(module.parent().unwrap()).unwrap();
        std::fs::write(&module, "return {}\n").unwrap();
        let binary = fake_script(
            &dir,
            "echo \"scripts/init.lua(2,1): TypeError: Unknown require: $PWD/util.lua\"\n\
             echo \"scripts/init.lua(3,1): TypeError: Unknown require: $PWD/example.command.lua\"\n\
             exit 1\n",
        );
        let scripts = vec!["scripts/server/init.lua".to_string()];
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &scripts,
            &[Side::Server],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Pass), "{outcome:?}");
        let missing = fake_script(
            &dir,
            "echo \"scripts/init.lua(4,1): TypeError: Unknown require: $PWD/nope.lua\"\nexit 1\n",
        );
        match analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &scripts,
            &[Side::Server],
            Some(&missing),
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Fail(why) => {
                assert!(why.contains("nope.lua"), "{why}");
                assert!(
                    why.contains("[server] scripts/server/init.lua(4,1)"),
                    "{why}"
                );
            }
            other => panic!("a missing require still fails, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The registry is copied into the view as the separate data stage, not
    /// by the side/shared flatten: a `require("registry")` from a side script
    /// is still unknown, even though the file sits in the view.
    #[cfg(unix)]
    #[test]
    fn a_registry_require_is_not_resolved_by_the_view() {
        let (dir, cache) = per_side_case("registry-require");
        let binary = fake_script(
            &dir,
            "printf '%s\\n' \"scripts/init.lua(1,1): TypeError: Unknown require: $PWD/registry.lua\"\nexit 1\n",
        );
        match analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/server/init.lua".into()],
            &[Side::Server],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Fail(why) => assert!(why.contains("registry.lua"), "{why}"),
            other => panic!("the registry is not a side module, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Diagnostics from a staged view name the mod's real files: the shared
    /// module, and each side's entry, with line and column kept.
    #[cfg(unix)]
    #[test]
    fn a_failing_side_names_its_real_files() {
        let (dir, cache) = per_side_case("paths");
        let binary = fake_script(
            &dir,
            "echo 'scripts/util.lua(1,1): TypeError: shared'\n\
             echo 'scripts/init.lua(2,1): TypeError: entry'\nexit 1\n",
        );
        let scripts = vec!["scripts/shared/util.lua".to_string()];
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &scripts,
            &[Side::Server, Side::Client],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap();
        let Outcome::Fail(why) = outcome else {
            panic!("a type error fails the step, got {outcome:?}");
        };
        for line in [
            "[server] scripts/shared/util.lua(1,1)",
            "[server] scripts/server/init.lua(2,1)",
            "[client] scripts/shared/util.lua(1,1)",
            "[client] scripts/client/init.lua(2,1)",
        ] {
            assert!(why.contains(line), "missing {line:?} in:\n{why}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// luau-lsp on Windows prints backslashes: the path before `(line,col)`
    /// and after `Unknown require: ` are normalised, so the type error is
    /// remapped to the real file and the resolvable require is dropped. A
    /// backslash in the message itself is text and is left alone.
    #[cfg(unix)]
    #[test]
    fn windows_diagnostic_paths_are_normalised() {
        let (dir, cache) = per_side_case("windows-paths");
        std::fs::write(
            dir.join("scripts/server/util.lua"),
            "-- server utility\nreturn {}\n",
        )
        .unwrap();
        let binary = fake_script(
            &dir,
            "root=$(printf '%s' \"$PWD\" | tr '/' '\\\\')\n\
             printf '%s\\n' \"scripts\\\\util.lua(1,1): TypeError: bad escape \\\\q\"\n\
             printf '%s\\n' \"scripts\\\\init.lua(2,1): TypeError: Unknown require: ${root}\\\\util.lua\"\n\
             exit 1\n",
        );
        match analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/server/init.lua".into()],
            &[Side::Server],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap()
        {
            Outcome::Fail(why) => {
                assert!(
                    why.contains(
                        "[server] scripts/server/util.lua(1,1): TypeError: bad escape \\q"
                    ),
                    "{why}"
                );
                assert!(
                    !why.contains("Unknown require"),
                    "the resolvable require was kept:\n{why}"
                );
            }
            other => panic!("the type error fails the step, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A failure only one side has is reported under that side alone.
    #[cfg(unix)]
    #[test]
    fn an_error_only_one_side_has_is_reported_only_for_that_side() {
        let (dir, cache) = per_side_case("one-side");
        let binary = fake_script(
            &dir,
            "if grep -q 'client entry' scripts/init.lua; then\n\
               echo 'scripts/init.lua(1,1): TypeError: client only'\n  exit 1\nfi\n\
             exit 0\n",
        );
        let scripts = vec!["scripts/client/init.lua".to_string()];
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &scripts,
            &[Side::Server, Side::Client],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap();
        let Outcome::Fail(why) = outcome else {
            panic!("the client's error fails the step, got {outcome:?}");
        };
        assert!(
            why.contains("[client] scripts/client/init.lua(1,1)"),
            "{why}"
        );
        assert!(
            !why.contains("[server]"),
            "reported on the server too:\n{why}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A mod with no per-side layout is not given an invented side: it keeps
    /// the single analysis in the mod's own folder.
    #[cfg(unix)]
    #[test]
    fn a_mod_without_sides_is_analysed_once_at_its_root() {
        let (dir, cache) = case("no-sides");
        let binary = fake_script(&dir, "pwd > \"$0.pwd\"\nexit 0\n");
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/init.lua".into()],
            &[],
            Some(&binary),
            &cache,
            false,
            None,
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Pass), "{outcome:?}");
        let ran = std::fs::read_to_string(binary.with_extension("pwd")).unwrap();
        assert_eq!(
            canonical(Path::new(ran.trim())),
            canonical(&dir),
            "the analysis should run in the mod, not a staged view"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The types step with the real luau-lsp: both sides resolve the shared
    /// module as the runtime does, a type error in it is reported for both
    /// sides under its real path, and a client-only error under `[client]`.
    /// `cargo nextest run -p cargo-horizon-mod --run-ignored only` (needs the
    /// network once for the pinned binary, or a cached one).
    #[test]
    #[ignore]
    fn the_types_step_analyses_each_side_as_the_runtime_resolves_requires() {
        let base =
            std::env::temp_dir().join(format!("horizon-mod-real-sides-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("mod");
        per_side_mod(&dir);
        let cache = tools_cache(None).unwrap();
        let scripts = vec![
            "scripts/server/init.lua".to_string(),
            "scripts/client/init.lua".to_string(),
            "scripts/shared/util.lua".to_string(),
        ];
        let run = || {
            analyze(
                &dir,
                &scripts,
                &[Side::Server, Side::Client],
                None,
                &cache,
                false,
                None,
            )
            .unwrap()
        };
        match run() {
            Outcome::Pass => {}
            other => panic!("a shared require resolves on both sides, got {other:?}"),
        }
        std::fs::write(
            dir.join("scripts/shared/util.lua"),
            "local x: number = \"a\"\nreturn {}\n",
        )
        .unwrap();
        match run() {
            Outcome::Fail(why) => {
                assert!(why.contains("[server] scripts/shared/util.lua"), "{why}");
                assert!(why.contains("[client] scripts/shared/util.lua"), "{why}");
            }
            other => panic!("a shared type error fails both sides, got {other:?}"),
        }
        std::fs::write(
            dir.join("scripts/shared/util.lua"),
            "local M = {}\nreturn M\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("scripts/client/init.lua"),
            "local x: number = \"a\"\nreturn x\n",
        )
        .unwrap();
        match run() {
            Outcome::Fail(why) => {
                assert!(why.contains("[client] scripts/client/init.lua"), "{why}");
                assert!(
                    !why.contains("[server]"),
                    "reported for the server too:\n{why}"
                );
            }
            other => panic!("a client type error fails the client alone, got {other:?}"),
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A mod that already has the reserved folder name would be hidden by the
    /// ignore pattern: it is refused, naming the folder.
    #[test]
    fn a_mod_with_the_reserved_folder_name_is_refused() {
        let (dir, cache) = case("reserved-name");
        let own = dir.join("scripts").join(RESERVED_MODKIT);
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("x.lua"), "return {}\n").unwrap();
        let kit = dir.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        let outcome = analyze_with(
            &manifest("unused", &"0".repeat(64)),
            &dir,
            &["scripts/init.lua".into()],
            &[],
            Some(Path::new("/no/such/binary")),
            &cache,
            false,
            Some(&kit),
        );
        match outcome {
            Err(e) => assert!(e.contains(RESERVED_MODKIT), "{e}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A zip entry over the cap is refused, not read whole (the cap is a
    /// parameter, so the test uses a small one); one at the cap is read.
    #[test]
    fn a_zip_entry_over_the_cap_is_refused() {
        let bytes = zip_of(&[("luau-lsp", &[7u8; 100])]);
        let err = unpack_capped(&bytes, 99).unwrap_err();
        assert!(err.contains("larger than 99 bytes"), "{err}");
        assert_eq!(unpack_capped(&bytes, 100).unwrap(), vec![7u8; 100]);
    }

    /// A zip with bytes in front of it (a self-extractor's stub) is read as
    /// the archive it is, not as the binary.
    #[test]
    fn a_zip_with_a_stub_in_front_is_read() {
        let mut bytes = b"#!stub line before the archive\n".to_vec();
        bytes.extend(zip_of(&[("luau-lsp", b"the binary")]));
        assert_eq!(unpack(&bytes).unwrap(), b"the binary");
    }

    /// The unresolvable `@modkit` match follows the OS's case rules: Windows
    /// and macOS fold case, Linux does not.
    #[test]
    fn the_unresolvable_match_folds_case_only_where_files_do() {
        let prefix = "c:/w/horizon-modkit/scripts/modkit";
        let line = "Unknown require: C:\\W\\Horizon-Modkit\\scripts\\modkit.lua";
        assert!(is_unresolvable_require(line, prefix, "windows"));
        assert!(is_unresolvable_require(line, prefix, "macos"));
        assert!(!is_unresolvable_require(line, prefix, "linux"));
    }

    /// Zip bytes with one entry per `(name, contents)`, for the unpack tests.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        let mut archive = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut archive);
            for (name, contents) in entries {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(contents).unwrap();
            }
            zip.finish().unwrap();
        }
        archive.into_inner()
    }

    /// The user cache base per platform, from a fake environment: Windows
    /// takes LOCALAPPDATA, else AppData\Local under USERPROFILE; Linux and
    /// macOS take ~/.cache; XDG_CACHE_HOME wins where it is set.
    #[test]
    fn the_cache_base_follows_each_platforms_convention() {
        let env = |pairs: &[(&str, &str)]| {
            let owned: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            move |name: &str| {
                owned
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        assert_eq!(
            cache_base(
                "windows",
                env(&[("LOCALAPPDATA", "C:\\L"), ("USERPROFILE", "C:\\U")])
            ),
            Some(PathBuf::from("C:\\L"))
        );
        let profile = cache_base("windows", env(&[("USERPROFILE", "C:\\U")])).unwrap();
        assert!(
            profile.starts_with("C:\\U") && profile.ends_with(Path::new("AppData").join("Local")),
            "{profile:?}"
        );
        assert_eq!(
            cache_base("linux", env(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.cache"))
        );
        assert_eq!(
            cache_base(
                "linux",
                env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/home/u")])
            ),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            cache_base("macos", env(&[("HOME", "/Users/u")])),
            Some(PathBuf::from("/Users/u/.cache"))
        );
        for os in ["windows", "linux", "macos"] {
            assert_eq!(cache_base(os, env(&[])), None, "{os} with nothing set");
        }
    }

    /// The `Unknown require` diagnostics of an unresolvable `@modkit` alias
    /// are dropped whatever shape the alias has (a file, a directory, or
    /// `require("@modkit/util")`), with either separator. Any other unknown
    /// require is kept.
    #[test]
    fn only_the_unresolvable_modkit_requires_are_dropped() {
        let prefix = unresolvable_prefix(Path::new("/w/horizon-modkit/scripts/modkit"));
        assert_eq!(prefix, "/w/horizon-modkit/scripts/modkit");
        assert_eq!(
            unresolvable_prefix(Path::new("/w/kit/scripts/modkit.lua")),
            "/w/kit/scripts/modkit"
        );
        let dropped = |line: &str| is_unresolvable_require(line, &prefix, "linux");
        assert!(dropped(
            "Unknown require: /w/horizon-modkit/scripts/modkit.lua"
        ));
        assert!(dropped(
            "Unknown require: /w/horizon-modkit/scripts/modkit/init.lua"
        ));
        assert!(dropped(
            "Unknown require: /w/horizon-modkit/scripts/modkit/util.lua"
        ));
        let win = unresolvable_prefix(Path::new("C:\\w\\horizon-modkit\\scripts\\modkit"));
        assert!(is_unresolvable_require(
            "Unknown require: C:\\w\\horizon-modkit\\scripts\\modkit\\util.lua",
            &win,
            "linux"
        ));
        assert!(is_unresolvable_require(
            "Unknown require: C:\\w\\horizon-modkit\\scripts\\modkit.lua",
            &win,
            "linux"
        ));
        assert!(!dropped("Unknown require: /w/other/thing.lua"));
        assert!(!dropped(
            "Unknown require: /w/horizon-modkit/scripts/modkit2.lua"
        ));
        assert!(!dropped(
            "/w/horizon-modkit/scripts/modkit.lua:1: Type error"
        ));
    }

    /// With no modkit checkout, the dropped `@modkit` requires are the ones
    /// the mod's own `.luaurc` alias names, in the layout it names (the
    /// template's `../horizon-modkit/scripts/server/modkit`): a path the
    /// alias does not name is still a failure.
    #[cfg(unix)]
    #[test]
    fn the_unresolvable_modkit_drop_follows_the_mods_alias() {
        let (dir, cache) = case("alias-drop");
        let write_alias = |path: &str| {
            std::fs::write(
                dir.join(".luaurc"),
                format!("{{\"aliases\": {{\"modkit\": \"{path}\"}}}}"),
            )
            .unwrap();
        };
        write_alias("../horizon-modkit/scripts/server/modkit");
        let alias = dir
            .parent()
            .unwrap()
            .join("horizon-modkit/scripts/server/modkit");
        let binary = fake_script(
            &dir,
            &format!(
                "echo 'scripts/init.lua(1,1): TypeError: Unknown require: {}.lua'\nexit 1\n",
                alias.display()
            ),
        );
        let run = || {
            analyze_with(
                &manifest("unused", &"0".repeat(64)),
                &dir,
                &["scripts/init.lua".into()],
                &[],
                Some(&binary),
                &cache,
                false,
                None,
            )
            .unwrap()
        };
        match run() {
            Outcome::Unchecked(why) => {
                assert!(why.contains("@modkit requires are unchecked"), "{why}");
            }
            other => panic!("the alias's unresolvable require is dropped, got {other:?}"),
        }
        // The same diagnostic under another alias is not dropped: the path
        // comes from the mod's `.luaurc`, not from a fixed layout.
        write_alias("../elsewhere/scripts/server/modkit");
        match run() {
            Outcome::Fail(why) => assert!(why.contains("Unknown require"), "{why}"),
            other => panic!("a path the alias does not name still fails, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Selene over the horizon std: an unused local and an undefined global
    /// are errors; the `horizon` and `registry` globals are known.
    #[test]
    fn lint_flags_unused_locals_and_undefined_globals_but_knows_the_horizon_globals() {
        let dir = std::env::temp_dir().join(format!("horizon-mod-lint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(
            dir.join("scripts/bad.lua"),
            "local unused_thing = 1\nprint(no_such_global_anywhere)\nhorizon.broadcast(\"hi\")\nregistry.item(\"x\", {})\n",
        )
        .unwrap();
        let found = lints(&dir, &["scripts/bad.lua".into()]).unwrap();
        let errors: Vec<_> = found.iter().filter(|l| l.error).map(|l| &l.text).collect();
        // An undefined global is the error that fails the step...
        assert!(
            errors.len() == 1
                && errors[0].contains("bad.lua:2")
                && errors[0].contains("no_such_global_anywhere"),
            "{found:?}"
        );
        // ...an unused local is a warning, and the horizon globals are known.
        assert!(
            found
                .iter()
                .any(|l| !l.error && l.text.contains("bad.lua:1") && l.text.contains("unused")),
            "{found:?}"
        );
        assert!(
            !found.iter().any(|l| l.text.contains("horizon") && l.error),
            "the horizon global must be known: {found:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The format step catches what `fmt` then fixes.
    #[test]
    fn format_check_catches_what_fmt_fixes() {
        let dir = std::env::temp_dir().join(format!("horizon-mod-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/init.lua"), "local   x   =   1\n").unwrap();
        let scripts = vec!["scripts/init.lua".to_string()];
        assert_eq!(unformatted(&dir, &scripts).unwrap(), ["scripts/init.lua"]);
        assert_eq!(format_scripts(&dir, &scripts).unwrap(), 1);
        assert_eq!(unformatted(&dir, &scripts).unwrap().len(), 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("scripts/init.lua")).unwrap(),
            "local x = 1\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
