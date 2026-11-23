//! `--list`: the Lua mods a game folder and the dev paths give, one line each, with the status the
//! host gives: loaded, refused (its translation key and reason) or disabled by `enabled.toml`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use event_horizon_lua::discover::{self, Refused};

use crate::load;

pub fn run(game_dir: &Path, dev: &[PathBuf]) -> ExitCode {
    let scan = discover::scan_on(game_dir, dev, false);
    let host = load::load(game_dir, dev);
    // The host's refusal lines for folders the scan refused are already listed below; what is left
    // is a mod that scanned as loaded and was refused when the host declared it.
    let scan_lines: Vec<String> = scan.refused.iter().map(refusal_line).collect();
    let mut declared: Vec<Option<String>> = host
        .refused
        .iter()
        .filter(|line| !scan_lines.contains(line))
        .map(|line| Some(line.clone()))
        .collect();
    let mut lines = Vec::new();

    for scanned in &scan.loaded {
        let found = &scanned.found;
        let prefix = format!("{}: ", found.id);
        let status = match declared.iter_mut().find(|slot| {
            slot.as_deref()
                .is_some_and(|line| line.starts_with(&prefix))
        }) {
            Some(slot) => format!("refused: {}", slot.take().unwrap_or_default()),
            None => "loaded".to_owned(),
        };
        lines.push(format!(
            "{} {} {} {status}",
            found.id,
            found.version,
            side_name(found.side)
        ));
    }
    for refused in &scan.refused {
        lines.push(format!(
            "{} refused: {}",
            refused_name(refused),
            refusal_words(refused)
        ));
    }
    for disabled in &scan.disabled {
        lines.push(format!(
            "{} {} {} disabled",
            disabled.id,
            disabled.version,
            side_name(disabled.side)
        ));
    }
    // A host refusal that no loaded mod accounts for is still shown.
    for line in declared.into_iter().flatten() {
        lines.push(format!("(host) refused: {line}"));
    }

    for line in &lines {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

/// The side as the manifest names it (`client`, `server` or `both`); its type is the crate's
/// private manifest module, so it is read through its `Debug` text.
fn side_name(side: impl std::fmt::Debug) -> String {
    format!("{side:?}").to_lowercase()
}

/// The folder or archive a scan refused, by its file name.
fn refused_name(refused: &Refused) -> String {
    refused
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `<reason> (<key>)`, the words the host gives in its refusal line.
fn refusal_words(refused: &Refused) -> String {
    format!("{} ({})", refused.fault.reason(), refused.fault.key())
}

/// The host's refusal line for a folder the scan refused: `<name>: <reason> (<key>)`.
fn refusal_line(refused: &Refused) -> String {
    format!("{}: {}", refused_name(refused), refusal_words(refused))
}
