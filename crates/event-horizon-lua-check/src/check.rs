//! `check <path>`: one mod, three passes. (1) The load: the mod goes through the Lua host headless
//! in a scratch game folder. (2) The lint: selene over each script the manifest lists. (3) The
//! format: StyLua over each script. A warning fails the check only under `--strict`.

use std::path::Path;
use std::process::ExitCode;

use event_horizon_lua::discover;
use event_horizon_lua::hlua::Package;

use crate::finding::{Finding, Level};
use crate::format;
use crate::lint::{self, Linter};
use crate::load::{self, ScratchGame};

pub fn run(path: &Path, strict: bool) -> ExitCode {
    let path = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "event-horizon-lua-check: cannot read {}: {error}",
                path.display()
            );
            return ExitCode::from(2);
        }
    };
    let folder = path.is_dir();
    let archive = path
        .extension()
        .is_some_and(|ext| ext == discover::ARCHIVE_EXT);
    if !folder && !archive {
        eprintln!(
            "event-horizon-lua-check: {} is neither a mod folder nor a .{} archive",
            path.display(),
            discover::ARCHIVE_EXT
        );
        return ExitCode::from(2);
    }
    let scratch = match ScratchGame::new() {
        Ok(scratch) => scratch,
        Err(error) => {
            eprintln!("event-horizon-lua-check: cannot make a scratch game folder: {error}");
            return ExitCode::from(2);
        }
    };

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The load's findings are about the mod as a whole, so they are filed under `load`.
    let mut findings: Vec<Finding> = Vec::new();
    let outcome = load::load(scratch.path(), std::slice::from_ref(&path));
    let loaded = outcome.loaded == 1 && outcome.refused.is_empty();
    if loaded {
        println!("load: loaded");
    } else {
        if outcome.refused.is_empty() {
            findings.push(Finding::whole(
                "load",
                Level::Error,
                "refused: no Lua mod loaded".to_owned(),
            ));
        }
        for line in &outcome.refused {
            findings.push(Finding::whole(
                "load",
                Level::Error,
                format!("refused: {line}"),
            ));
        }
    }

    // The scripts are read from the package the scan opens, the same reader the load used.
    let scan = discover::scan_on(scratch.path(), std::slice::from_ref(&path), false);
    match scan.loaded.into_iter().find(|scanned| scanned.dev) {
        Some(scanned) => {
            let config_folder = folder.then_some(path.as_path());
            match check_scripts(&scanned.package, config_folder) {
                Ok(more) => findings.extend(more),
                Err(message) => {
                    eprintln!("event-horizon-lua-check: {message}");
                    return ExitCode::from(1);
                }
            }
        }
        None => {
            // A refused package has no scripts to lint: the load finding above says why.
            if loaded {
                findings.push(Finding::whole(
                    "load",
                    Level::Error,
                    format!("the scan did not return {name}"),
                ));
            }
        }
    }

    let errors = findings.iter().filter(|f| f.level == Level::Error).count();
    let warnings = findings.len() - errors;
    for finding in &findings {
        println!("{}", finding.render());
    }
    let failed = errors > 0 || (strict && warnings > 0);
    if failed {
        println!("result: failed, {errors} error(s), {warnings} warning(s)");
        ExitCode::from(1)
    } else {
        println!("result: ok, {warnings} warning(s)");
        ExitCode::SUCCESS
    }
}

/// Lints and format-checks every script the manifest lists. The `Err` is a problem with the checker
/// itself (its lint setup, or a config file that does not parse), not a finding in the mod.
fn check_scripts(package: &Package, folder: Option<&Path>) -> Result<Vec<Finding>, String> {
    let config = format::config_for(folder)?;
    let linter = Linter::new()?;
    let scripts = match package.entry_sources() {
        Ok(scripts) => scripts,
        Err(refusal) => {
            return Ok(vec![Finding::whole(
                "scripts",
                Level::Error,
                format!("{} ({})", refusal.reason(), refusal.key()),
            )]);
        }
    };
    let mut findings = Vec::new();
    for (name, text) in &scripts {
        match lint::parse(name, text) {
            Err(errors) => findings.extend(errors),
            Ok(ast) => {
                findings.extend(linter.lint(name, text, &ast));
                match format::would_reformat(&config, text) {
                    Ok(false) => {}
                    Ok(true) => findings.push(Finding::whole(
                        name,
                        Level::Warning,
                        "would reformat".to_owned(),
                    )),
                    Err(error) => findings.push(Finding::whole(
                        name,
                        Level::Error,
                        format!("format: {error}"),
                    )),
                }
            }
        }
    }
    Ok(findings)
}
