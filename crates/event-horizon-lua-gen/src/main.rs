//! `event-horizon-lua-gen --surface PATH --hooks-trait PATH --out DIR --types PATH [--check]`: writes
//! the Lua host's generated hook table (`DIR/hooks.rs`), its `Hooks` methods (`DIR/hooks_impl.rs`)
//! and the `ToLuaValue` impls of the surface's plain types (`PATH`, the convert crate's
//! `generated_types.rs`).
//!
//! Exit codes: 0 on success (with `--check`, when every file matches what would be written), 1 when
//! `--check` finds a file that differs, 2 on bad input or arguments.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use event_horizon_lua_gen::{emit_hooks, emit_impl, emit_types, ir, trait_sigs};
use mod_surface::Surface;

/// The parsed command line.
struct Args {
    surface: PathBuf,
    hooks_trait: PathBuf,
    out: PathBuf,
    types: PathBuf,
    check: bool,
}

const USAGE: &str = "usage: event-horizon-lua-gen --surface PATH --hooks-trait PATH --out DIR --types PATH [--check]";

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut surface = None;
    let mut hooks_trait = None;
    let mut out = None;
    let mut types = None;
    let mut check = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--surface" => surface = Some(PathBuf::from(value(&mut args, "--surface")?)),
            "--hooks-trait" => {
                hooks_trait = Some(PathBuf::from(value(&mut args, "--hooks-trait")?))
            }
            "--out" => out = Some(PathBuf::from(value(&mut args, "--out")?)),
            "--types" => types = Some(PathBuf::from(value(&mut args, "--types")?)),
            "--check" => check = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        surface: surface.ok_or("--surface is required")?,
        hooks_trait: hooks_trait.ok_or("--hooks-trait is required")?,
        out: out.ok_or("--out is required")?,
        types: types.ok_or("--types is required")?,
        check,
    })
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn run(args: Args) -> Result<ExitCode, String> {
    let bytes = read(&args.surface)?;
    let surface =
        ir::load(&bytes).map_err(|error| format!("{}: {error}", args.surface.display()))?;
    let document =
        Surface::read(&bytes).map_err(|error| format!("{}: {error}", args.surface.display()))?;
    let trait_text = String::from_utf8(read(&args.hooks_trait)?)
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;
    let sigs = trait_sigs::parse(&trait_text)
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;

    let names: Vec<&str> = surface
        .hooks
        .iter()
        .map(|hook| hook.name.as_str())
        .collect();
    let types = emit_types::plan(&document.types, &document.handles, &document.hash);
    let methods = emit_impl::emit(&surface, &sigs, &names, &types.plain);
    let emitted: Vec<&str> = methods.emitted.iter().map(String::as_str).collect();
    let table = emit_hooks::emit(&surface, &emitted);

    let table_path = args.out.join("hooks.rs");
    let methods_path = args.out.join("hooks_impl.rs");
    if args.check {
        let mut differ = Vec::new();
        for (path, text) in [
            (&table_path, &table),
            (&methods_path, &methods.text),
            (&args.types, &types.text),
        ] {
            let current = std::fs::read_to_string(path).ok();
            if current.as_deref() != Some(text.as_str()) {
                differ.push(path.display().to_string());
            }
        }
        if differ.is_empty() {
            println!("generated files match the surface");
            return Ok(ExitCode::SUCCESS);
        }
        for path in differ {
            println!("generated file is stale: {path}");
        }
        return Ok(ExitCode::from(1));
    }

    std::fs::create_dir_all(&args.out)
        .map_err(|error| format!("cannot create {}: {error}", args.out.display()))?;
    std::fs::write(&table_path, &table)
        .map_err(|error| format!("cannot write {}: {error}", table_path.display()))?;
    std::fs::write(&methods_path, &methods.text)
        .map_err(|error| format!("cannot write {}: {error}", methods_path.display()))?;
    if let Some(parent) = args.types.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&args.types, &types.text)
        .map_err(|error| format!("cannot write {}: {error}", args.types.display()))?;

    println!("hooks in the surface: {}", surface.hooks.len());
    println!("methods emitted: {}", methods.emitted.len());
    println!("hooks skipped: {}", methods.skipped.len());
    let mut by_reason = BTreeMap::<String, usize>::new();
    for (_, reason) in &methods.skipped {
        *by_reason.entry(emit_impl::group_key(reason)).or_default() += 1;
    }
    for (reason, count) in &by_reason {
        println!("{count:5}  {reason}");
    }
    println!("plain types emitted: {}", types.emitted.len());
    println!("types refused: {}", types.refused.len());
    let mut refused_by_reason = BTreeMap::<String, usize>::new();
    for (_, reason) in &types.refused {
        *refused_by_reason
            .entry(emit_impl::group_key(reason))
            .or_default() += 1;
    }
    for (reason, count) in &refused_by_reason {
        println!("{count:5}  {reason}");
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("event-horizon-lua-gen: {message}");
            ExitCode::from(2)
        }
    }
}
