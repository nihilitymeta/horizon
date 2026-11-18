//! `event-horizon-lua-gen --surface PATH --hooks-trait PATH --out DIR --types PATH [--check]`: writes
//! the Lua host's generated hook table (`DIR/hooks.rs`), its `Hooks` methods (`DIR/hooks_impl.rs`)
//! and the `ToLuaValue` impls of the surface's plain types (`PATH`, the convert crate's
//! `generated_types.rs`), and the handle families' methods (`DIR/handles.rs`) with their coverage
//! (`DIR/handles_coverage.txt`). `--families A,B` picks the handle families (default: every `*Ref` owner of
//! the surface; a family that binds no member is not generated and is listed in the coverage).
//!
//! Exit codes: 0 on success (with `--check`, when every file matches what would be written), 1 when
//! `--check` finds a file that differs, 2 on bad input or arguments.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use event_horizon_lua_gen::{
    dispatch_sigs, emit_handles, emit_hooks, emit_impl, emit_types, handles, ir, trait_sigs,
};
use mod_surface::Surface;

/// The parsed command line.
struct Args {
    surface: PathBuf,
    hooks_trait: PathBuf,
    out: PathBuf,
    types: PathBuf,
    families: Option<Vec<String>>,
    check: bool,
}

const USAGE: &str = "usage: event-horizon-lua-gen --surface PATH --hooks-trait PATH --out DIR --types PATH [--families A,B] [--check]";

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut surface = None;
    let mut hooks_trait = None;
    let mut out = None;
    let mut types = None;
    let mut families = None;
    let mut check = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--surface" => surface = Some(PathBuf::from(value(&mut args, "--surface")?)),
            "--hooks-trait" => {
                hooks_trait = Some(PathBuf::from(value(&mut args, "--hooks-trait")?))
            }
            "--out" => out = Some(PathBuf::from(value(&mut args, "--out")?)),
            "--types" => types = Some(PathBuf::from(value(&mut args, "--types")?)),
            "--families" => {
                families = Some(
                    value(&mut args, "--families")?
                        .split(',')
                        .map(str::to_owned)
                        .collect(),
                )
            }
            "--check" => check = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        surface: surface.ok_or("--surface is required")?,
        hooks_trait: hooks_trait.ok_or("--hooks-trait is required")?,
        out: out.ok_or("--out is required")?,
        types: types.ok_or("--types is required")?,
        families,
        check,
    })
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// What one pass of the generator makes: the hook table and methods, the plain types and the
/// handle methods, for the handle families `families` (`dropped` are listed in the coverage).
struct Plan {
    families: Vec<String>,
    handles: emit_handles::Emitted,
    table: String,
    methods: emit_impl::Emitted,
    first_methods: emit_impl::Emitted,
    /// `parts.rs`: the BlockPart methods and their handler table.
    parts: emit_impl::PartsEmitted,
    types: emit_types::TypesPlan,
    /// `hooks_coverage.txt`: every hook of the surface, emitted or with its skip reason.
    hook_coverage: String,
}

/// One pass over the surface for the handle families `families`.
fn plan_pass(
    surface: &ir::Ir,
    document: &Surface,
    sigs: &[trait_sigs::HookSig],
    part_sigs: &[trait_sigs::HookSig],
    dispatch: &dispatch_sigs::DispatchSigs,
    members: &[handles::MemberIr],
    families: &[&str],
    dropped: &[&str],
) -> Result<Plan, String> {
    let names: Vec<&str> = surface
        .hooks
        .iter()
        .map(|hook| hook.name.as_str())
        .collect();
    let hook_types: Vec<String> = surface
        .hooks
        .iter()
        .flat_map(|hook| hook.arguments.iter().map(|arg| arg.rust_type.clone()))
        .collect();
    // The handle methods' argument types the type emitter decides too: a unit enum among them
    // converts from Lua, as a hook's answer does.
    let handle_types = emit_handles::argument_types(members, families);
    // Two passes: the first decides every unit enum any hook could answer or output with, and its
    // skip reasons are the ones reported (they are not skewed by the second pass); the second
    // decides only the enums of the hooks the first pass emits, and must emit the same hooks.
    let candidates: Vec<String> = dispatch
        .hooks
        .values()
        .flatten()
        .flatten()
        .map(|output| output.ty.clone())
        .chain(
            sigs.iter()
                .chain(part_sigs)
                .filter_map(|sig| sig.ret.clone()),
        )
        .chain(handle_types.iter().cloned())
        .collect();
    let first = emit_types::plan_decided(
        &document.types,
        &document.handles,
        &document.hash,
        &hook_types,
        &candidates,
    );
    let first_methods =
        emit_impl::emit_with_handles(surface, sigs, dispatch, &names, &first.plain, families);
    let first_parts =
        emit_impl::emit_block_parts(surface, part_sigs, dispatch, &first.plain, families);
    let mut decided = first_methods.decisions.clone();
    decided.extend(first_parts.decisions.iter().cloned());
    decided.extend(handle_types.iter().cloned());
    let superset = emit_types::plan_decided(
        &document.types,
        &document.handles,
        &document.hash,
        &hook_types,
        &decided,
    );
    // The superset of the handle argument types decided above is narrowed to the bound members'
    // argument types, and the plan is made again: no converter is emitted for an unbound member.
    let bound_types = emit_handles::bound_argument_types(members, families, &superset.plain);
    let mut decided = first_methods.decisions.clone();
    decided.extend(first_parts.decisions.iter().cloned());
    decided.extend(bound_types);
    let types = emit_types::plan_decided(
        &document.types,
        &document.handles,
        &document.hash,
        &hook_types,
        &decided,
    );
    let methods =
        emit_impl::emit_with_handles(surface, sigs, dispatch, &names, &types.plain, families);
    let parts = emit_impl::emit_block_parts(surface, part_sigs, dispatch, &types.plain, families);
    if methods.emitted != first_methods.emitted {
        return Err("the two passes emit different hooks".to_owned());
    }
    if parts.emitted != first_parts.emitted {
        return Err("the two passes emit different block part methods".to_owned());
    }
    let emitted: Vec<&str> = methods.emitted.iter().map(String::as_str).collect();
    let table = emit_hooks::emit(surface, &emitted);
    let mut hook_coverage = emit_hooks::coverage(
        surface,
        &methods.emitted,
        &methods.host_recorded,
        &first_methods.skipped,
    );
    hook_coverage.push_str(&block_part_coverage(&parts));

    let handles =
        emit_handles::emit_listing(members, families, dropped, &types.plain, &document.hash);
    let superset_bound =
        emit_handles::emit(members, families, &superset.plain, &document.hash).bound;
    if superset_bound != handles.bound {
        return Err("narrowing the converters changed the bound handle members".to_owned());
    }
    Ok(Plan {
        families: families.iter().map(|family| (*family).to_owned()).collect(),
        handles,
        table,
        methods,
        first_methods,
        parts,
        types,
        hook_coverage,
    })
}

/// The BlockPart lines of `hooks_coverage.txt`: `block_part.<method> emitted` or `... skipped: <reason>`.
fn block_part_coverage(parts: &emit_impl::PartsEmitted) -> String {
    let mut out = String::from("\n# block parts (issue 74, P1): the BlockPart trait's methods\n");
    let mut lines: Vec<(String, String)> = parts
        .emitted
        .iter()
        .map(|name| (name.clone(), "emitted".to_owned()))
        .chain(
            parts
                .skipped
                .iter()
                .map(|(name, reason)| (name.clone(), format!("skipped: {reason}"))),
        )
        .collect();
    lines.sort();
    for (name, status) in lines {
        out.push_str(&format!("block_part.{name} {status}\n"));
    }
    out
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
    let dispatch = dispatch_sigs::parse(&trait_text)
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;

    let members =
        handles::load(&bytes).map_err(|error| format!("{}: {error}", args.surface.display()))?;
    let requested: Vec<String> = args
        .families
        .clone()
        .unwrap_or_else(|| emit_handles::candidate_families(&members));
    // A family that binds no member is dropped and the pass made again: dropping one can only
    // narrow the others' conversions, so the loop ends with every generated family bound.
    let mut families = requested.clone();
    let part_sigs = trait_sigs::parse_trait(&trait_text, "BlockPart")
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;
    let plan = loop {
        let refs: Vec<&str> = families.iter().map(String::as_str).collect();
        let dropped: Vec<&str> = requested
            .iter()
            .map(String::as_str)
            .filter(|family| !refs.contains(family))
            .collect();
        let plan = plan_pass(
            &surface, &document, &sigs, &part_sigs, &dispatch, &members, &refs, &dropped,
        )?;
        let empty: Vec<String> = plan
            .handles
            .per_family
            .iter()
            .filter(|(family, _, bound)| *bound == 0 && refs.contains(&family.as_str()))
            .map(|(family, _, _)| family.clone())
            .collect();
        if empty.is_empty() {
            break plan;
        }
        families.retain(|family| !empty.contains(family));
        if families.is_empty() {
            return Err("no handle family binds a member".to_owned());
        }
    };
    let Plan {
        families,
        handles: handles_out,
        table,
        methods,
        first_methods,
        parts,
        types,
        hook_coverage,
    } = plan;

    let table_path = args.out.join("hooks.rs");
    let methods_path = args.out.join("hooks_impl.rs");
    let handles_path = args.out.join("handles.rs");
    let coverage_path = args.out.join("handles_coverage.txt");
    let hook_coverage_path = args.out.join("hooks_coverage.txt");
    let parts_path = args.out.join("parts.rs");
    if args.check {
        let mut differ = Vec::new();
        for (path, text) in [
            (&table_path, &table),
            (&methods_path, &methods.text),
            (&args.types, &types.text),
            (&handles_path, &handles_out.text),
            (&coverage_path, &handles_out.coverage),
            (&hook_coverage_path, &hook_coverage),
            (&parts_path, &parts.text),
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
    std::fs::write(&handles_path, &handles_out.text)
        .map_err(|error| format!("cannot write {}: {error}", handles_path.display()))?;
    std::fs::write(&coverage_path, &handles_out.coverage)
        .map_err(|error| format!("cannot write {}: {error}", coverage_path.display()))?;
    std::fs::write(&hook_coverage_path, &hook_coverage)
        .map_err(|error| format!("cannot write {}: {error}", hook_coverage_path.display()))?;
    std::fs::write(&parts_path, &parts.text)
        .map_err(|error| format!("cannot write {}: {error}", parts_path.display()))?;
    println!("handle families: {}", families.join(", "));
    for (family, members, bound) in &handles_out.per_family {
        println!("  {family}: {members} members, {bound} bound");
    }
    println!("handle members bound: {}", handles_out.bound.len());
    println!("  heavy (cost 100): {}", handles_out.heavy.len());
    let mut skipped_by = BTreeMap::<String, usize>::new();
    for (_, reason) in &handles_out.skipped {
        *skipped_by
            .entry(emit_handles::reason_category(reason).to_owned())
            .or_default() += 1;
    }
    for (reason, count) in &skipped_by {
        println!("  skipped, {reason}: {count}");
    }

    println!("hooks in the surface: {}", surface.hooks.len());
    println!("methods emitted: {}", methods.emitted.len());
    println!(
        "  replace hooks (answer from a return value): {}",
        methods.replace
    );
    println!(
        "  hooks with a unit enum answer or output: {}",
        methods.decided_enum
    );
    println!("  hooks with a list or vector output: {}", methods.lists);
    println!("hooks skipped: {}", first_methods.skipped.len());
    let mut by_reason = BTreeMap::<String, usize>::new();
    for (_, reason) in &first_methods.skipped {
        *by_reason.entry(emit_impl::group_key(reason)).or_default() += 1;
    }
    for (reason, count) in &by_reason {
        println!("{count:5}  {reason}");
    }
    println!("output types of the emitted hooks:");
    for (ty, count) in &methods.output_types {
        println!("{count:5}  {ty}");
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
