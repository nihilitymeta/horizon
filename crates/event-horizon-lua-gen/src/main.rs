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
    dispatch_sigs, emit_handles, emit_hooks, emit_impl, emit_luau, emit_types, handles, ir,
    trait_sigs,
};
use mod_surface::Surface;

/// The parsed command line.
struct Args {
    surface: PathBuf,
    hooks_trait: PathBuf,
    out: PathBuf,
    types: PathBuf,
    /// The directory of `horizon.d.luau` and `.luaurc.example`; by default `OUT/../../lua-types`.
    lua_types: Option<PathBuf>,
    families: Option<Vec<String>>,
    check: bool,
}

const USAGE: &str = "usage: event-horizon-lua-gen --surface PATH --hooks-trait PATH --out DIR --types PATH [--lua-types DIR] [--families A,B] [--check]";

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut surface = None;
    let mut hooks_trait = None;
    let mut out = None;
    let mut types = None;
    let mut families = None;
    let mut lua_types = None;
    let mut check = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--surface" => surface = Some(PathBuf::from(value(&mut args, "--surface")?)),
            "--hooks-trait" => {
                hooks_trait = Some(PathBuf::from(value(&mut args, "--hooks-trait")?))
            }
            "--out" => out = Some(PathBuf::from(value(&mut args, "--out")?)),
            "--types" => types = Some(PathBuf::from(value(&mut args, "--types")?)),
            "--lua-types" => lua_types = Some(PathBuf::from(value(&mut args, "--lua-types")?)),
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
        lua_types,
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
    /// `item_parts.rs`: the ItemPart methods and their handler table.
    item_parts: emit_impl::PartsEmitted,
    /// `block_entity_parts.rs`: the BlockEntityPart methods and their handler table.
    block_entity_parts: emit_impl::PartsEmitted,
    /// `fluid_parts.rs`: the FluidPart methods and their handler table.
    fluid_parts: emit_impl::PartsEmitted,
    /// `entity_parts.rs`: the EntityPart methods and their handler table.
    entity_parts: emit_impl::PartsEmitted,
    /// `menu_parts.rs`: the MenuPart methods and their handler table.
    menu_parts: emit_impl::PartsEmitted,
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
    item_part_sigs: &[trait_sigs::HookSig],
    block_entity_part_sigs: &[trait_sigs::HookSig],
    fluid_part_sigs: &[trait_sigs::HookSig],
    entity_part_sigs: &[trait_sigs::HookSig],
    menu_part_sigs: &[trait_sigs::HookSig],
    dispatch: &dispatch_sigs::DispatchSigs,
    members: &[handles::MemberIr],
    families: &[&str],
    dropped: &[&str],
) -> Result<Plan, String> {
    // The EntityPart's bound plain methods are planned as replace hooks of their own surface.
    let (entity_ir, entity_dispatch) = emit_impl::with_plain(
        surface,
        dispatch,
        entity_part_sigs,
        emit_impl::PartTrait::Entity,
    );
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
                .chain(item_part_sigs)
                .chain(block_entity_part_sigs)
                .chain(fluid_part_sigs)
                .chain(entity_part_sigs)
                .chain(menu_part_sigs)
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
    let first_parts = emit_impl::emit_parts(
        surface,
        part_sigs,
        dispatch,
        &first.plain,
        families,
        emit_impl::PartTrait::Block,
    );
    let first_item_parts = emit_impl::emit_parts(
        surface,
        item_part_sigs,
        dispatch,
        &first.plain,
        families,
        emit_impl::PartTrait::Item,
    );
    let first_block_entity_parts = emit_impl::emit_parts(
        surface,
        block_entity_part_sigs,
        dispatch,
        &first.plain,
        families,
        emit_impl::PartTrait::BlockEntity,
    );
    let first_fluid_parts = emit_impl::emit_parts(
        surface,
        fluid_part_sigs,
        dispatch,
        &first.plain,
        families,
        emit_impl::PartTrait::Fluid,
    );
    let first_entity_parts = emit_impl::emit_parts(
        &entity_ir,
        entity_part_sigs,
        &entity_dispatch,
        &first.plain,
        families,
        emit_impl::PartTrait::Entity,
    );
    let first_menu_parts = emit_impl::emit_parts(
        surface,
        menu_part_sigs,
        dispatch,
        &first.plain,
        families,
        emit_impl::PartTrait::Menu,
    );
    let mut decided = first_methods.decisions.clone();
    decided.extend(first_parts.decisions.iter().cloned());
    decided.extend(first_item_parts.decisions.iter().cloned());
    decided.extend(first_block_entity_parts.decisions.iter().cloned());
    decided.extend(first_fluid_parts.decisions.iter().cloned());
    decided.extend(first_entity_parts.decisions.iter().cloned());
    decided.extend(first_menu_parts.decisions.iter().cloned());
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
    decided.extend(first_item_parts.decisions.iter().cloned());
    decided.extend(first_block_entity_parts.decisions.iter().cloned());
    decided.extend(first_fluid_parts.decisions.iter().cloned());
    decided.extend(first_entity_parts.decisions.iter().cloned());
    decided.extend(first_menu_parts.decisions.iter().cloned());
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
    let parts = emit_impl::emit_parts(
        surface,
        part_sigs,
        dispatch,
        &types.plain,
        families,
        emit_impl::PartTrait::Block,
    );
    let item_parts = emit_impl::emit_parts(
        surface,
        item_part_sigs,
        dispatch,
        &types.plain,
        families,
        emit_impl::PartTrait::Item,
    );
    if methods.emitted != first_methods.emitted {
        return Err("the two passes emit different hooks".to_owned());
    }
    if parts.emitted != first_parts.emitted {
        return Err("the two passes emit different block part methods".to_owned());
    }
    if item_parts.emitted != first_item_parts.emitted {
        return Err("the two passes emit different item part methods".to_owned());
    }
    let block_entity_parts = emit_impl::emit_parts(
        surface,
        block_entity_part_sigs,
        dispatch,
        &types.plain,
        families,
        emit_impl::PartTrait::BlockEntity,
    );
    if block_entity_parts.emitted != first_block_entity_parts.emitted {
        return Err("the two passes emit different block entity part methods".to_owned());
    }
    let fluid_parts = emit_impl::emit_parts(
        surface,
        fluid_part_sigs,
        dispatch,
        &types.plain,
        families,
        emit_impl::PartTrait::Fluid,
    );
    if fluid_parts.emitted != first_fluid_parts.emitted {
        return Err("the two passes emit different fluid part methods".to_owned());
    }
    let entity_parts = emit_impl::emit_parts(
        &entity_ir,
        entity_part_sigs,
        &entity_dispatch,
        &types.plain,
        families,
        emit_impl::PartTrait::Entity,
    );
    if entity_parts.emitted != first_entity_parts.emitted {
        return Err("the two passes emit different entity part methods".to_owned());
    }
    let menu_parts = emit_impl::emit_parts(
        surface,
        menu_part_sigs,
        dispatch,
        &types.plain,
        families,
        emit_impl::PartTrait::Menu,
    );
    if menu_parts.emitted != first_menu_parts.emitted {
        return Err("the two passes emit different menu part methods".to_owned());
    }
    let emitted: Vec<&str> = methods.emitted.iter().map(String::as_str).collect();
    let table = emit_hooks::emit(surface, &emitted);
    let mut hook_coverage = emit_hooks::coverage(
        surface,
        &methods.emitted,
        &methods.host_recorded,
        &first_methods.skipped,
    );
    hook_coverage.push_str(&part_coverage(&parts, emit_impl::PartTrait::Block));
    hook_coverage.push_str(&part_coverage(&item_parts, emit_impl::PartTrait::Item));
    hook_coverage.push_str(&part_coverage(
        &block_entity_parts,
        emit_impl::PartTrait::BlockEntity,
    ));
    hook_coverage.push_str(&part_coverage(&fluid_parts, emit_impl::PartTrait::Fluid));
    hook_coverage.push_str(&part_coverage(&entity_parts, emit_impl::PartTrait::Entity));
    hook_coverage.push_str(&part_coverage(&menu_parts, emit_impl::PartTrait::Menu));

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
        item_parts,
        block_entity_parts,
        fluid_parts,
        entity_parts,
        menu_parts,
        types,
        hook_coverage,
    })
}

/// The Part lines of `hooks_coverage.txt`: `block_part.<method> emitted` or `... skipped: <reason>`
/// (`item_part.` for the ItemPart trait).
fn part_coverage(parts: &emit_impl::PartsEmitted, part: emit_impl::PartTrait) -> String {
    let (label, prefix) = match part {
        emit_impl::PartTrait::Block => ("block parts (issue 74, P1)", "block_part"),
        emit_impl::PartTrait::Item => ("item parts (issue 74, P2)", "item_part"),
        emit_impl::PartTrait::BlockEntity => {
            ("block entity parts (issue 74, P3)", "block_entity_part")
        }
        emit_impl::PartTrait::Fluid => ("fluid parts (issue 74, P4)", "fluid_part"),
        emit_impl::PartTrait::Entity => ("entity parts (issue 74, P5)", "entity_part"),
        emit_impl::PartTrait::Menu => ("menu parts (issue 74, B)", "menu_part"),
    };
    let mut out = format!("\n# {label}: the {} trait's methods\n", part.name());
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
        out.push_str(&format!("{prefix}.{name} {status}\n"));
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
    let item_part_sigs = trait_sigs::parse_trait(&trait_text, "ItemPart")
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;
    let block_entity_part_sigs = trait_sigs::parse_trait(&trait_text, "BlockEntityPart")
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;
    let fluid_part_sigs = trait_sigs::parse_trait(&trait_text, "FluidPart")
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;
    // The EntityPart's hook-shaped methods and its plain ones (the game calls these directly).
    let menu_part_sigs = trait_sigs::parse_trait(&trait_text, "MenuPart")
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;
    let mut entity_part_sigs = trait_sigs::parse_trait(&trait_text, "EntityPart")
        .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?;
    entity_part_sigs.extend(
        trait_sigs::parse_plain(&trait_text, "EntityPart")
            .map_err(|error| format!("{}: {error}", args.hooks_trait.display()))?,
    );
    entity_part_sigs.sort_by(|a, b| a.name.cmp(&b.name));
    let plan = loop {
        let refs: Vec<&str> = families.iter().map(String::as_str).collect();
        let dropped: Vec<&str> = requested
            .iter()
            .map(String::as_str)
            .filter(|family| !refs.contains(family))
            .collect();
        let plan = plan_pass(
            &surface,
            &document,
            &sigs,
            &part_sigs,
            &item_part_sigs,
            &block_entity_part_sigs,
            &fluid_part_sigs,
            &entity_part_sigs,
            &menu_part_sigs,
            &dispatch,
            &members,
            &refs,
            &dropped,
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
        item_parts,
        block_entity_parts,
        fluid_parts,
        entity_parts,
        menu_parts,
        types,
        hook_coverage,
    } = plan;

    // The Luau type definitions, from the same plan: the emitted hooks and Part methods with their
    // shapes, and the bound handle members of the families.
    let family_refs: Vec<&str> = families.iter().map(String::as_str).collect();
    let emitted_hooks: Vec<&str> = methods.emitted.iter().map(String::as_str).collect();
    let hook_shapes = emit_impl::shapes(
        &surface,
        &sigs,
        &dispatch,
        &emitted_hooks,
        &types.plain,
        &family_refs,
        false,
    );
    let part_shapes = |sigs: &[trait_sigs::HookSig], emitted: &[String]| {
        let names: Vec<&str> = emitted.iter().map(String::as_str).collect();
        emit_impl::shapes(
            &surface,
            sigs,
            &dispatch,
            &names,
            &types.plain,
            &family_refs,
            true,
        )
    };
    let part_lists = [
        part_shapes(&part_sigs, &parts.emitted),
        part_shapes(&item_part_sigs, &item_parts.emitted),
        part_shapes(&block_entity_part_sigs, &block_entity_parts.emitted),
        part_shapes(&fluid_part_sigs, &fluid_parts.emitted),
        {
            let (entity_ir, entity_dispatch) = emit_impl::with_plain(
                &surface,
                &dispatch,
                &entity_part_sigs,
                emit_impl::PartTrait::Entity,
            );
            let names: Vec<&str> = entity_parts.emitted.iter().map(String::as_str).collect();
            emit_impl::shapes(
                &entity_ir,
                &entity_part_sigs,
                &entity_dispatch,
                &names,
                &types.plain,
                &family_refs,
                true,
            )
        },
        part_shapes(&menu_part_sigs, &menu_parts.emitted),
    ];
    let bound_members: Vec<&handles::MemberIr> = members
        .iter()
        .filter(|member| handles_out.bound.contains(&member.id))
        .collect();
    let luau = emit_luau::emit(&emit_luau::Input {
        hash: &surface.document_hash,
        hooks: &hook_shapes,
        parts: [
            part_lists[0].as_slice(),
            part_lists[1].as_slice(),
            part_lists[2].as_slice(),
            part_lists[3].as_slice(),
            part_lists[4].as_slice(),
            part_lists[5].as_slice(),
        ],
        families: &families,
        members: &bound_members,
        plain: &types.plain,
    })
    .map_err(|error| format!("horizon.d.luau: {error}"))?;
    let lua_types_dir = args
        .lua_types
        .clone()
        .unwrap_or_else(|| args.out.join("../../lua-types"));
    let luau_path = lua_types_dir.join("horizon.d.luau");
    let luaurc_path = lua_types_dir.join(".luaurc.example");
    let luaurc = emit_luau::LUAURC_EXAMPLE.to_owned();

    let table_path = args.out.join("hooks.rs");
    let methods_path = args.out.join("hooks_impl.rs");
    let handles_path = args.out.join("handles.rs");
    let coverage_path = args.out.join("handles_coverage.txt");
    let hook_coverage_path = args.out.join("hooks_coverage.txt");
    let parts_path = args.out.join("parts.rs");
    let item_parts_path = args.out.join("item_parts.rs");
    let block_entity_parts_path = args.out.join("block_entity_parts.rs");
    let fluid_parts_path = args.out.join("fluid_parts.rs");
    let entity_parts_path = args.out.join("entity_parts.rs");
    let menu_parts_path = args.out.join("menu_parts.rs");
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
            (&item_parts_path, &item_parts.text),
            (&block_entity_parts_path, &block_entity_parts.text),
            (&fluid_parts_path, &fluid_parts.text),
            (&entity_parts_path, &entity_parts.text),
            (&menu_parts_path, &menu_parts.text),
            (&luau_path, &luau.text),
            (&luaurc_path, &luaurc),
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
    std::fs::create_dir_all(&lua_types_dir)
        .map_err(|error| format!("cannot create {}: {error}", lua_types_dir.display()))?;
    std::fs::write(&luau_path, &luau.text)
        .map_err(|error| format!("cannot write {}: {error}", luau_path.display()))?;
    std::fs::write(&luaurc_path, &luaurc)
        .map_err(|error| format!("cannot write {}: {error}", luaurc_path.display()))?;
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
    std::fs::write(&item_parts_path, &item_parts.text)
        .map_err(|error| format!("cannot write {}: {error}", item_parts_path.display()))?;
    std::fs::write(&block_entity_parts_path, &block_entity_parts.text).map_err(|error| {
        format!(
            "cannot write {}: {error}",
            block_entity_parts_path.display()
        )
    })?;
    std::fs::write(&fluid_parts_path, &fluid_parts.text)
        .map_err(|error| format!("cannot write {}: {error}", fluid_parts_path.display()))?;
    std::fs::write(&entity_parts_path, &entity_parts.text)
        .map_err(|error| format!("cannot write {}: {error}", entity_parts_path.display()))?;
    std::fs::write(&menu_parts_path, &menu_parts.text)
        .map_err(|error| format!("cannot write {}: {error}", menu_parts_path.display()))?;
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
    println!(
        "horizon.d.luau: {} hook events, {} part methods, {} handle members typed",
        luau.hooks, luau.parts, luau.members
    );
    let mut any_by_type = BTreeMap::<&str, usize>::new();
    for ty in &luau.any {
        *any_by_type.entry(ty.as_str()).or_default() += 1;
    }
    println!("horizon.d.luau: {} types written any:", luau.any.len());
    for (ty, count) in &any_by_type {
        println!("{count:5}  {ty}");
    }
    println!("horizon.d.luau: {} named plain types emitted", luau.named);
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
