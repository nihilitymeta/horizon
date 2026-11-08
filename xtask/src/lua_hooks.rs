//! `cargo run -p xtask -- lua-hooks`: `crates/event-horizon-lua/src/generated_hooks.rs`,
//! the Lua side of a SPIKE of generated native hooks: for each hook in
//! `GENERATED`, the code that makes `horizon.on("<native hook name>",
//! function(event) ... end)` work with ONE event table holding the struct's
//! fields by name. `--check` fails when the checked-in file is stale, exactly
//! like `hooks-doc --check` and `lua-types --check`.
//!
//! The input is the same hook definitions the other generators read:
//! `native-hooks.json` (plus `tools/native-hook-fragments/`) through
//! `horizon_mod_api_gen::catalog_hooks`, and the native functions' signatures
//! parsed with `syn` the way `hooks_doc` reads their docs. No new source of
//! truth; the one rule mirrored from `horizon-mod-api-gen` (which host type a
//! receiver gets) is marked `MIRRORED` below: if that crate changes it, the
//! generated code stops compiling, loudly, instead of drifting.
//!
//! A hook the converters cannot express fails the generator with a reason
//! starting either `adapter:` (a hand-written converter would unblock it) or
//! `unsupported:` (it cannot be exposed as an event table). The hooks in
//! `SKIPPED` document where generation stops; they emit nothing.

use horizon_mod_api_gen::{catalog_hooks, free_function_source, one_function};
use std::{collections::BTreeMap, fs, path::Path};

/// The hooks the generator emits, in output order: ~4 each of listen, cancel,
/// collect and replace, all unreachable from Lua today, plus none of the
/// hand-written ones.
const GENERATED: &[&str] = &[
    // Listen: only plain fields.
    "level_tick_pre",
    "chunk_load",
    "block_cook",
    "anvil_damaged",
    "player_container_close",
    // Cancel: `horizon.cancel()` in the handler becomes `Flow::Cancel`.
    "block_burn",
    "bell_ring",
    "arrow_loose",
    "block_lock_check",
    // Collect: the handler mutates the out array/table in the event.
    "component_types",
    "datapack_sync",
    "screen_tooltip",
    "is_bonemeal_success",
    // Replace: the handler's value for the out field becomes `Some`.
    "item_fluid_tank",
    "tooltip_component",
    "screen_click",
    "prepare_anvil",
];

/// Hooks the generator deliberately does not emit, with why.
const SKIPPED: &[(&str, &str)] = &[
    (
        "mob_submit",
        "unsupported: a mob's render submission on the render thread \
        (`&NativeMobRender` borrowed views, `&mut BlockEntityDraw` draw handle): \
        calling into the Lua state's thread from the render thread would race it, \
        and neither borrowed views nor the draw handle map to an event table",
    ),
    (
        "state_for_placement",
        "unsupported: opaque composite parameters (`&PlacementContext`, `FluidState`) \
        have no event-table mapping, and the out type nests (`Option<Option<BlockStateId>>`)",
    ),
    (
        "world_drawn",
        "unsupported: a `RenderHooks` hook, not a `Hooks` one (a different trait \
        the Lua runtime does not implement), firing on the render thread with a \
        borrowed view (`&mut WorldDrawn<'_>`) whose inner lifetime cannot leave the frame",
    ),
];

/// What calls the generated method: none, the game host, or a level host.
#[derive(Clone, Copy, PartialEq)]
enum Host {
    None,
    Game { mutable: bool },
    Level { mutable: bool },
}

/// A hook's model, read from the catalog and its native function.
struct Model {
    name: String,
    event: String,
    contract: String,
    out: Option<String>,
    host: Host,
    /// `(name, type)` of every native parameter in order, ticks excluded
    /// (none of the generated hooks has one; one would fail loudly).
    params: Vec<(String, syn::Type)>,
    /// The `&mut` out parameter's inner type, for cancel/replace/collect.
    out_ty: Option<syn::Type>,
}

/// `PascalCase` of a `snake_case` hook name, plus `Event`.
fn event_name(hook: &str) -> String {
    let mut out = String::new();
    for word in hook.split('_') {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out.push_str("Event");
    out
}

/// MIRRORED from `horizon-mod-api-gen`'s private `host_for`: which host type a
/// receiver gets. Game hooks borrow the game host; level hooks (`GameLevel`
/// targets) borrow a level host. A mismatch here is a compile error in the
/// generated code, not silent drift.
fn host_of(target: &str, receiver: Option<bool>) -> Result<Host, String> {
    let is_level = target.contains("::GameLevel<");
    match receiver {
        None => Ok(Host::None),
        Some(mutable) => {
            if is_level {
                Ok(Host::Level { mutable })
            } else {
                Ok(Host::Game { mutable })
            }
        }
    }
}

/// The native function of `hook`: its target's `native_<hook>`, parsed.
fn native_function<'a>(
    root: &Path,
    target: &str,
    hook: &str,
    game: &'a [(String, syn::ItemFn)],
    frees: &'a mut BTreeMap<String, Vec<syn::ItemFn>>,
) -> Result<&'a syn::ItemFn, String> {
    let native = format!("native_{hook}");
    if target.contains("::GameLevel<") || target.contains("::Game<'_>::") {
        let found: Vec<&syn::ItemFn> = game
            .iter()
            .filter(|(module, _)| target.starts_with(module))
            .map(|(_, f)| f)
            .filter(|f| f.sig.ident == native)
            .collect();
        return one_function(found.into_iter(), &native);
    }
    if let Some((krate, path)) = free_function_source(target) {
        let (module, function) = path
            .rsplit_once("::")
            .ok_or_else(|| format!("wrong target for {hook}"))?;
        if function != native {
            return Err(format!("wrong target for {hook}"));
        }
        let key = format!("{krate}/{module}");
        if !frees.contains_key(&key) {
            let file = root
                .join("crates")
                .join(krate)
                .join("src")
                .join(format!("{}.rs", module.replace("::", "/")));
            let source = fs::read_to_string(&file).map_err(|e| format!("{key}: {e}"))?;
            let parsed: syn::File = syn::parse_str(&source).map_err(|e| format!("{key}: {e}"))?;
            let functions = parsed
                .items
                .into_iter()
                .filter_map(|item| match item {
                    syn::Item::Fn(f) => Some(f),
                    _ => None,
                })
                .collect();
            frees.insert(key.clone(), functions);
        }
        return one_function(frees[&key].iter(), &native);
    }
    Err(format!("wrong target for {hook}"))
}

/// All inherent `Game`/`GameLevel` methods under `game/`, by module.
fn game_methods(root: &Path) -> Result<Vec<(String, syn::ItemFn)>, String> {
    fn visit(
        path: &Path,
        module: &str,
        methods: &mut Vec<(String, syn::ItemFn)>,
    ) -> Result<(), String> {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned();
            if path.is_dir() {
                visit(&path, &format!("{module}::{stem}"), methods)?;
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let file: syn::File = syn::parse_str(&source).map_err(|e| e.to_string())?;
                for item in file.items {
                    if let syn::Item::Impl(item) = item
                        && item.trait_.is_none()
                    {
                        methods.extend(
                            horizon_mod_api_gen::impl_functions(item)
                                .map(|f| (module.to_owned(), f)),
                        );
                    }
                }
            }
        }
        Ok(())
    }
    let mut methods = Vec::new();
    visit(
        &root.join("crates/horizon-client/src/game"),
        "horizon_client::game",
        &mut methods,
    )?;
    Ok(methods)
}

/// Read `hook`'s model: its contract and out parameter from the catalog, its
/// receiver and parameters from the native function.
fn model_of(
    root: &Path,
    hook: &serde_json::Value,
    game: &[(String, syn::ItemFn)],
    frees: &mut BTreeMap<String, Vec<syn::ItemFn>>,
) -> Result<Model, String> {
    let name = hook["name"].as_str().ok_or("hook name missing")?;
    let api = hook
        .get("mod_api")
        .ok_or_else(|| format!("hook {name} has no mod_api entry"))?;
    let contract = api["contract"].as_str().ok_or("contract missing")?;
    if !matches!(contract, "listen" | "cancel" | "collect" | "replace") {
        return Err(format!("{name}: contract {contract} is not generated"));
    }
    let out = api.get("out").and_then(|v| v.as_str()).map(str::to_owned);
    let target = hook["target"].as_str().ok_or("target missing")?;
    let native = native_function(root, target, name, game, frees)?.clone();
    let mut receiver = None;
    let mut params = Vec::new();
    for arg in &native.sig.inputs {
        match arg {
            syn::FnArg::Receiver(r) => {
                receiver = Some(r.mutability.is_some());
            }
            syn::FnArg::Typed(arg) => {
                let syn::Pat::Ident(pat) = &*arg.pat else {
                    return Err(format!("{name}: unsupported argument"));
                };
                if pat.ident == "ticks" {
                    return Err(format!(
                        "{name}: adapter: tick-scheduling hooks need the level tick host"
                    ));
                }
                params.push((pat.ident.to_string(), (*arg.ty).clone()));
            }
        }
    }
    // A collect out stays a parameter: the event carries it as `&mut` for every
    // mod to edit in turn. Every other out leaves the parameters: the method
    // returns it instead.
    let collect = contract == "collect";
    let out_ty = match &out {
        None => None,
        Some(out) => {
            let index = params
                .iter()
                .position(|(n, _)| n == out)
                .ok_or_else(|| format!("{name}: missing output {out}"))?;
            let ty = if collect {
                params[index].1.clone()
            } else {
                params.remove(index).1
            };
            let syn::Type::Reference(reference) = &ty else {
                return Err(format!(
                    "{name}: unsupported: output {out} is not a reference"
                ));
            };
            if reference.mutability.is_none() {
                return Err(format!(
                    "{name}: unsupported: output {out} is not written back"
                ));
            }
            Some((*reference.elem).clone())
        }
    };
    Ok(Model {
        event: event_name(name),
        contract: contract.to_owned(),
        out,
        host: host_of(target, receiver)?,
        params,
        out_ty,
        name: name.to_owned(),
    })
}

/// Whether a type is the path `name` (any lifetimes, no other arguments).
fn is_path(ty: &syn::Type, name: &str) -> bool {
    matches!(ty, syn::Type::Path(p) if p.qself.is_none() && p.path.is_ident(name))
}

/// `&T` (any lifetime) or `T` itself: the referent and whether it was borrowed.
fn deref(ty: &syn::Type) -> (&syn::Type, bool) {
    match ty {
        syn::Type::Reference(r) => (&r.elem, true),
        other => (other, false),
    }
}

/// A seeded Lua value: the Rust expression building it from `event`.
/// `writeback` is the statement copying an edited value back, if the field is
/// written after the handlers run.
struct Seed {
    expr: String,
    writeback: Option<String>,
}

/// The event-table seed and write-back for one native parameter. Errors start
/// with `adapter:` (a hand-written converter would unblock it) or
/// `unsupported:` (no event-table mapping exists).
fn seed_of(name: &str, field: &str, ty: &syn::Type, event: &str) -> Result<Seed, String> {
    let plain = || Seed {
        expr: format!("{event}.{field}"),
        writeback: None,
    };
    let adapter = |what: &str| format!("{name}: adapter: {what}");
    let unsupported = |what: &str| format!("{name}: unsupported: {what}");
    let (inner, borrowed) = deref(ty);
    // `&mut` scalars are copied into the event as plain values and written
    // back after the call, exactly like the generated dispatch does.
    if let syn::Type::Reference(r) = ty
        && r.mutability.is_some()
        && let syn::Type::Path(p) = &*r.elem
        && p.qself.is_none()
        && let Some(id) = p.path.get_ident()
        && [
            "bool", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
        ]
        .iter()
        .any(|s| id == s)
    {
        let ty = id.to_string();
        return Ok(Seed {
            expr: format!("{event}.{field}"),
            writeback: Some(format!(
                "{event}.{field} = table.get::<{ty}>(\"{field}\")?;"
            )),
        });
    }
    match inner {
        syn::Type::Path(p) if p.qself.is_none() => {
            let id = p
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            match id.as_str() {
                "str" | "String" => Ok(plain()),
                "bool" | "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32"
                | "f64" => Ok(plain()),
                "char" => Err(adapter("a `char` needs its string form")),
                "u128" => Ok(Seed {
                    expr: format!("uuid_text({event}.{field})"),
                    writeback: None,
                }),
                "BlockPos" => Ok(Seed {
                    expr: format!(
                        "crate::generated_hooks::pos_table(lua, [{event}.{field}.x, {event}.{field}.y, {event}.{field}.z])?"
                    ),
                    writeback: None,
                }),
                "LevelDimension" => Ok(Seed {
                    expr: format!("dimension_id({event}.{field})"),
                    writeback: None,
                }),
                "BlockStateId" | "BlockId" => Err(adapter(
                    "a block state needs its id string from a host or level",
                )),
                "Direction" => Err(adapter("a direction needs its name")),
                "Text" => Err(adapter("rich text needs its plain form")),
                "Compound" => Err(adapter("a compound needs the nbt table")),
                "ItemStack" if borrowed => Ok(Seed {
                    expr: format!("item_table_of(lua, {event}.{field})?"),
                    writeback: None,
                }),
                _ => Err(unsupported(&format!("type {id} has no mapping"))),
            }
        }
        syn::Type::Array(array) => {
            let len = match &array.len {
                syn::Expr::Lit(lit) => match &lit.lit {
                    syn::Lit::Int(n) => n.base10_parse::<usize>().unwrap_or(0),
                    _ => 0,
                },
                _ => 0,
            };
            let elem = match &*array.elem {
                syn::Type::Path(p) => p
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            match (len, elem.as_str()) {
                (3, "i32") => Ok(Seed {
                    expr: format!("crate::generated_hooks::pos_table(lua, {event}.{field})?"),
                    writeback: None,
                }),
                (2, "f32") => Ok(Seed {
                    expr: format!("crate::generated_hooks::pair_table(lua, {event}.{field})?"),
                    writeback: None,
                }),
                (2, "i32") => Err(adapter("a 2D int array needs its table form")),
                _ => Err(adapter("a short array needs its table form")),
            }
        }
        syn::Type::Slice(_) => Ok(Seed {
            expr: format!("crate::generated_hooks::int_seq(lua, {event}.{field})?"),
            writeback: None,
        }),
        syn::Type::Tuple(_) => Err(adapter("a tuple needs its table form")),
        _ => Err(unsupported("this type has no mapping")),
    }
}

/// One table field: its setter statement and its write-back statement, if any.
struct Field {
    set: String,
    writeback: Option<String>,
}

/// The chunk a `chunk_load`-style hook names, as `{x, z}` rather than the
/// generic `{x, y}` pair: chunk coordinates run along x and z.
fn chunk_seed(field: &str, event: &str) -> String {
    format!(
        "{{\nlet chunk = lua.create_table()?;\nchunk.set(\"x\", {event}.{field}[0])?;\nchunk.set(\"z\", {event}.{field}[1])?;\nchunk\n}}"
    )
}

/// The player a hook names by uuid (and name), as the hand-written player
/// hooks build it: the world's record while they are on, else uuid and name.
fn player_seed(event: &str) -> String {
    format!("self.player_by_uuid(host, {event}.player, Some({event}.name))?")
}

/// A block id through a level host: `state_block`, air when unknown, as the
/// block hooks resolve it.
fn block_seed(state: &str) -> String {
    format!("level.state_block({state}).unwrap_or_else(|| \"minecraft:air\".to_owned())")
}

/// The table fields of `model`: every parameter by its converter, except the
/// cancel out flag (handlers use `horizon.cancel()` instead) and the fields
/// the per-hook notes below replace. `event` is the native event's binding.
fn fields_of(model: &Model, event: &str) -> Result<Vec<Field>, String> {
    let name = model.name.as_str();
    // Parameters the generic converter does not see: `skip` names them, and
    // `extra` adds their replacement (a player table, a block id, an out
    // value) in table order.
    let (skip, extra): (Vec<&str>, Vec<(String, Option<String>)>) = match name {
        "chunk_load" => (
            vec!["chunk"],
            vec![(
                format!("table.set(\"chunk\", {})?;", chunk_seed("chunk", event)),
                None,
            )],
        ),
        "player_container_close" => (
            vec!["player", "name", "pos"],
            vec![
                (
                    format!("table.set(\"player\", {})?;", player_seed(event)),
                    None,
                ),
                (
                    format!(
                        "table.set(\"pos\", match {event}.pos {{ Some(p) => mlua::Value::Table(crate::generated_hooks::pos_table(lua, p)?), None => mlua::Value::Nil }})?;"
                    ),
                    None,
                ),
            ],
        ),
        "block_lock_check" | "prepare_anvil" => (
            vec!["player", "name"],
            vec![(
                format!("table.set(\"player\", {})?;", player_seed(event)),
                None,
            )],
        ),
        "block_burn" => (
            vec![],
            vec![(
                format!(
                    "table.set(\"block\", {})?;",
                    block_seed(&format!(
                        "level.block_state([{event}.pos.x, {event}.pos.y, {event}.pos.z])"
                    ))
                ),
                None,
            )],
        ),
        // A level hook has no game host, so no player table: the uuid text,
        // or nil when no player rang it.
        "bell_ring" => (
            vec!["player"],
            vec![
                (
                    format!(
                        "table.set(\"block\", {})?;",
                        block_seed(&format!(
                            "level.block_state([{event}.pos.x, {event}.pos.y, {event}.pos.z])"
                        ))
                    ),
                    None,
                ),
                (
                    format!(
                        "table.set(\"player\", match {event}.player {{ Some(p) => mlua::Value::String(lua.create_string(uuid_text(p))?), None => mlua::Value::Nil }})?;"
                    ),
                    None,
                ),
            ],
        ),
        "is_bonemeal_success" => (
            vec!["state"],
            vec![(
                format!("table.set(\"block\", {})?;", block_seed("event.state")),
                None,
            )],
        ),
        _ => (Vec::new(), Vec::new()),
    };
    // Seed the replace out value as nil: the method is not told the current
    // value, so the handler only provides one; nil reads back as None.
    let mut outs = Vec::new();
    if model.contract == "replace"
        && let Some(out) = &model.out
        && let Some(ty) = &model.out_ty
    {
        outs.push(out_seed(model, out, ty, event)?);
    }
    let mut fields = Vec::new();
    for (param, ty) in &model.params {
        if skip.contains(&param.as_str()) {
            continue;
        }
        // A collect out is seeded from the shared `&mut` for every handler to
        // edit in turn; `&mut bool` copies like any scalar.
        if model.contract == "collect" && model.out.as_deref() == Some(param.as_str()) {
            let (inner, _) = deref(ty);
            if is_vec_of(inner, "String") {
                fields.push(Field {
                    set: format!(
                        "table.set(\"{param}\", crate::generated_hooks::string_seq(lua, {event}.{param})?)?;"
                    ),
                    writeback: Some(out_write(model)?),
                });
                continue;
            }
        }
        let seed = seed_of(name, param, ty, event)?;
        fields.push(Field {
            set: format!("table.set(\"{param}\", {})?;", seed.expr),
            writeback: seed.writeback,
        });
    }
    for (set, writeback) in extra.into_iter().chain(outs) {
        fields.push(Field { set, writeback });
    }
    Ok(fields)
}

/// `Some` inner type of an `Option<T>` out: replace outs are seeded from the
/// current value and read back the same way, nil for None.
fn option_inner(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(p) = ty
        && p.path.segments.last().is_some_and(|s| s.ident == "Option")
        && let Some(seg) = p.path.segments.last()
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return Some(inner);
    }
    None
}

/// Whether `ty` is a `&mut` scalar the event copies as a plain value.
fn is_scalar_mut(ty: &syn::Type) -> bool {
    if let syn::Type::Reference(r) = ty
        && r.mutability.is_some()
        && let syn::Type::Path(p) = &*r.elem
        && p.qself.is_none()
        && let Some(id) = p.path.get_ident()
    {
        return [
            "bool", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64",
        ]
        .iter()
        .any(|s| id == s);
    }
    false
}

/// Whether `ty` carries a borrow: the event then needs a lifetime.
fn contains_ref(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Reference(_) => true,
        syn::Type::Path(p) => p.path.segments.iter().any(|s| match &s.arguments {
            syn::PathArguments::AngleBracketed(args) => args
                .args
                .iter()
                .any(|g| matches!(g, syn::GenericArgument::Type(t) if contains_ref(t))),
            _ => false,
        }),
        syn::Type::Array(a) => contains_ref(&a.elem),
        syn::Type::Slice(s) => contains_ref(&s.elem),
        syn::Type::Tuple(t) => t.elems.iter().any(contains_ref),
        _ => false,
    }
}

/// The seed of a replace out value: always nil. The trait method is not told
/// the current value (the event carries no out field), so the handler only
/// provides one; nil reads back as None.
fn out_seed(
    model: &Model,
    out: &str,
    ty: &syn::Type,
    _event: &str,
) -> Result<(String, Option<String>), String> {
    let name = model.name.as_str();
    let inner = option_inner(ty).unwrap_or(ty);
    let supported = match inner {
        syn::Type::Path(_) if is_path(inner, "i32") => true,
        syn::Type::Array(_) => true,
        syn::Type::Path(_) if is_path(inner, "ItemStack") => true,
        _ => false,
    };
    if !supported {
        return Err(format!(
            "{name}: adapter: a replace out needs its table form"
        ));
    }
    Ok((format!("table.set(\"{out}\", mlua::Value::Nil)?;"), None))
}

/// The Rust expression reading a replace out value back, and its type.
fn out_read(model: &Model) -> Result<(String, String), String> {
    let name = model.name.as_str();
    let out = model.out.as_deref().ok_or("replace without out")?;
    let ty = model.out_ty.as_ref().ok_or("replace without out type")?;
    let inner = option_inner(ty).unwrap_or(ty);
    let read = match inner {
        syn::Type::Path(_) if is_path(inner, "i32") => format!(
            "Ok(match table.get::<mlua::Value>(\"{out}\")? {{ mlua::Value::Nil => None, v => Some(mlua::FromLua::from_lua(v, lua)?) }})"
        ),
        syn::Type::Array(_) => format!(
            "Ok(match table.get::<mlua::Value>(\"{out}\")? {{ mlua::Value::Nil => None, mlua::Value::Table(t) => Some(crate::generated_hooks::read_pair(&t)?), _ => return Err(mlua::Error::runtime(\"{out} is a table or nil\")) }})"
        ),
        syn::Type::Path(_) if is_path(inner, "ItemStack") => format!(
            "Ok(match table.get::<mlua::Value>(\"{out}\")? {{ mlua::Value::Nil => None, mlua::Value::Table(t) => Some(crate::generated_hooks::item_from_table(&t)?), _ => return Err(mlua::Error::runtime(\"{out} is a table or nil\")) }})"
        ),
        _ => {
            return Err(format!(
                "{name}: adapter: a replace out needs its table form"
            ));
        }
    };
    // The trait returns the out type itself (`Option<i32>`), filled by the
    // first `Some` a handler provides.
    let ty_name = match inner {
        syn::Type::Path(_) if is_path(inner, "i32") => "Option<i32>",
        syn::Type::Array(_) => "Option<[f32; 2]>",
        _ => "Option<horizon_client::mod_api::ItemStack>",
    };
    Ok((ty_name.to_owned(), read))
}

/// Whether `ty` is `Vec<elem>`.
fn is_vec_of(ty: &syn::Type, elem: &str) -> bool {
    if let syn::Type::Path(p) = ty
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Vec"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return is_path(inner, elem);
    }
    false
}

/// The write-back of a collect out value through the event's `&mut`.
fn out_write(model: &Model) -> Result<String, String> {
    let out = model.out.as_deref().ok_or("collect without out")?;
    let ty = model.out_ty.as_ref().ok_or("collect without out type")?;
    let (inner, _) = deref(ty);
    if is_vec_of(inner, "String") {
        return Ok(format!(
            "*event.{out} = crate::generated_hooks::read_string_seq(&table.get::<mlua::Table>(\"{out}\")?)?;"
        ));
    }
    // Scalar collect outs (`&mut bool`) copy like any scalar.
    if let syn::Type::Path(_) = inner {
        return Ok(format!("event.{out} = table.get(\"{out}\")?;"));
    }
    Err(format!(
        "{}: adapter: a collect out needs its table form",
        model.name
    ))
}

/// How the method reaches the handlers: through the game host (which may run
/// actions), through a level host (reads only), or with no host at all.
enum Call {
    /// `call_kind` with the host: every action runs, `cancel` takes the hook.
    Kind,
    /// `call_bare`: no host, actions are dropped with a warning, `cancel`
    /// still takes a cancel hook.
    Bare,
    /// `run_handlers` with a level reader: level reads work, actions drop.
    Level,
}

/// One generated `Hooks` method, as the hand-written ones are shaped: skip at
/// once with no handler, build the event table, run the handlers, read back
/// what they changed, return the contract's answer.
fn method_of(model: &Model) -> Result<String, String> {
    let name = model.name.as_str();
    let event = model.event.as_str();
    let (sig_host, call) = match model.host {
        Host::None => (String::new(), Call::Bare),
        Host::Game { mutable: true } => (
            ", host: &mut dyn horizon_client::mod_api::Host".to_owned(),
            Call::Kind,
        ),
        // A read-only host cannot run actions: reads build the table, queued
        // actions drop, `cancel` still takes a cancel hook. None of the
        // generated hooks reads through it, so it stays unnamed.
        Host::Game { mutable: false } => (
            ", _host: &dyn horizon_client::mod_api::Host".to_owned(),
            Call::Bare,
        ),
        Host::Level { mutable: false } => (
            ", level: &dyn horizon_client::mod_api::LevelHost".to_owned(),
            Call::Level,
        ),
        Host::Level { mutable: true } => (
            ", level: &mut dyn horizon_client::mod_api::LevelRandomHost".to_owned(),
            Call::Level,
        ),
    };
    let fields = fields_of(model, "event")?;
    let mut sets = String::new();
    for field in &fields {
        sets.push_str(&format!("{}\n", field.set));
    }
    let mut backs = String::new();
    for field in &fields {
        if let Some(writeback) = &field.writeback {
            backs.push_str(writeback);
            backs.push('\n');
        }
    }
    // The call, per host: `cancel` takes a cancel hook everywhere, other
    // actions run on a read-write game host and drop elsewhere.
    let cancel_arm = matches!(model.contract.as_str(), "cancel")
        .then_some("\"cancel\" => cancel = true,")
        .unwrap_or("");
    let run = match call {
        Call::Kind => format!(
            "self.call_kind(hook, None, host, table.clone(), &mut |host, action, args| {{ match action {{ {cancel_arm} _ => self.run_action(host, action, args)?, }} Ok(()) }})?;"
        ),
        Call::Bare => format!(
            "self.call_bare(hook, None, table.clone(), &mut |action, _args| {{ match action {{ {cancel_arm} _ => self.warn(format!(\"{{hook}}: {{action}} needs a host; dropped\")), }} Ok(()) }})?;"
        ),
        Call::Level => {
            let reader = match model.host {
                Host::Level { mutable: false } => "Reader::Level(level)",
                _ => "Reader::Level(&*level)",
            };
            format!(
                "for (action, _args) in self.run_handlers(hook, None, {reader}, table.clone())? {{ match action.as_str() {{ {cancel_arm} _ => self.warn(format!(\"{{hook}}: {{action}} needs a host; dropped\")), }} }}"
            )
        }
    };
    let cancel_let = matches!(model.contract.as_str(), "cancel")
        .then_some("let mut cancel = false;\n")
        .unwrap_or("");
    // The contract's answer: `Flow` for cancel, `Some` for replace, nothing
    // otherwise; a handler error keeps the native side as it was.
    let replace_read = if model.contract == "replace" {
        Some(out_read(model)?)
    } else {
        None
    };
    let (ret, ok, close) = match model.contract.as_str() {
        "cancel" => (
            " -> horizon_client::mod_api::Flow".to_owned(),
            "mlua::Result<bool>".to_owned(),
            "match outcome { Ok(true) => horizon_client::mod_api::Flow::Cancel, Ok(false) => horizon_client::mod_api::Flow::Continue, Err(e) => self.hook_error(hook, e), }".to_owned(),
        ),
        "replace" => {
            let (ty, _) = replace_read.clone().ok_or("replace without out")?;
            (
                format!(" -> {ty}"),
                format!("mlua::Result<{ty}>"),
                "match outcome { Ok(v) => v, Err(e) => { self.error(format!(\"{hook}: {e}\")); None } }".to_owned(),
            )
        }
        _ => (
            String::new(),
            "mlua::Result<()>".to_owned(),
            "if let Err(e) = outcome { self.error(format!(\"{hook}: {e}\")); }".to_owned(),
        ),
    };
    let tail = match model.contract.as_str() {
        "cancel" => format!("{{ {backs}Ok(cancel) }}"),
        "replace" => {
            let (_, read) = replace_read.ok_or("replace without out")?;
            format!("{{ {backs}{read} }}")
        }
        _ => format!("{{ {backs}Ok(()) }}"),
    };
    // The event's lifetime: none when every field is owned or copied in.
    let lifetime = if model
        .params
        .iter()
        .any(|(_, ty)| contains_ref(ty) && !is_scalar_mut(ty))
    {
        "<'_>"
    } else {
        ""
    };
    let doc = match model.contract.as_str() {
        "cancel" => format!(
            "/// A generated hook: `horizon.on(\"{name}\", function(event) ... end)` hears it with\n\
             /// one event table; `horizon.cancel()` in the handler takes it (`Flow::Cancel`)."
        ),
        "replace" => format!(
            "/// A generated hook: `horizon.on(\"{name}\", function(event) ... end)` hears it with\n\
             /// one event table; the handler's value for the out field becomes `Some`."
        ),
        "collect" => format!(
            "/// A generated hook: `horizon.on(\"{name}\", function(event) ... end)` hears it with\n\
             /// one event table; what the handler leaves in the out field is kept."
        ),
        _ => format!(
            "/// A generated hook: `horizon.on(\"{name}\", function(event) ... end)` hears it with\n\
             /// one event table."
        ),
    };
    Ok(format!(
        "{doc}\nfn {name}(&self{sig_host}, event: &mut horizon_client::mod_api::{event}{lifetime}){ret} {{\n\
         let hook = \"{name}\";\n\
         if !self.has_handlers(hook) {{ {dflt} }}\n\
         let outcome = (|| -> {ok} {{\nlet lua = &self.lua;\nlet table = lua.create_table()?;\n{sets}{cancel_let}{run}\n{tail}\n}})();\n{close}\n}}\n",
        dflt = match model.contract.as_str() {
            "cancel" => "return horizon_client::mod_api::Flow::Continue;",
            "replace" => "return None;",
            _ => "return;",
        },
    ))
}

/// A test of one generated hook: the Lua handler (asserting every field it is
/// told) and the Rust side (firing the native dispatch with known values,
/// asserting what arrived back).
struct HookTest {
    hook: &'static str,
    lua: &'static str,
    rust: &'static str,
}

const HOOK_TESTS: &[HookTest] = &[
    HookTest {
        hook: "level_tick_pre",
        lua: r#"horizon.on("level_tick_pre", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_level_tick_pre", LUA);
let mods = boxed(m);
dispatch_level_tick_pre(&mut FakeHost::empty(), &mods, "minecraft:overworld");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "chunk_load",
        lua: r#"horizon.on("chunk_load", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.chunk.x == 3 and event.chunk.z == -5, "chunk")
  assert(event.new == true, "new")
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_chunk_load", LUA);
let mods = boxed(m);
dispatch_chunk_load(&mut FakeHost::empty(), &mods, "minecraft:overworld", [3, -5], true);
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "block_cook",
        lua: r#"horizon.on("block_cook", function(event)
  assert(event.pos.x == 1 and event.pos.y == 2 and event.pos.z == 3, "pos")
  assert(event.item == "minecraft:wheat", "item")
  assert(event.result == "minecraft:bread", "result")
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_block_cook", LUA);
let mods = boxed(m);
dispatch_block_cook(&FakeHost::empty(), &mods, [1, 2, 3], "minecraft:wheat", "minecraft:bread");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "anvil_damaged",
        lua: r#"horizon.on("anvil_damaged", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.pos.x == 4 and event.pos.y == 64 and event.pos.z == -2, "pos")
  assert(event.from == "minecraft:damaged_anvil", "from")
  assert(event.to == "minecraft:chipped_anvil", "to")
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_anvil_damaged", LUA);
let mods = boxed(m);
dispatch_anvil_damaged(&mut FakeHost::empty(), &mods, "minecraft:overworld", [4, 64, -2], "minecraft:damaged_anvil", "minecraft:chipped_anvil");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "player_container_close",
        lua: r#"horizon.on("player_container_close", function(event)
  assert(event.player.uuid == "00000000-0000-0000-0000-000000000007", "uuid")
  assert(event.player.name == "Alex", "name")
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.menu == "minecraft:chest", "menu")
  assert(event.pos == nil, "pos")
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_player_container_close", LUA);
let mods = boxed(m);
dispatch_player_container_close(&mut FakeHost::empty(), &mods, 7, "Alex", "minecraft:overworld", "minecraft:chest", None);
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "block_burn",
        lua: r#"horizon.on("block_burn", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.pos.x == 1 and event.pos.y == 2 and event.pos.z == 3, "pos")
  assert(event.block == "minecraft:stone", "block")
  horizon.cancel()
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_block_burn", LUA);
let mods = boxed(m);
let mut cancel = false;
dispatch_block_burn(&FakeLevel, &mods, LevelDimension::Overworld, BlockPos::new(1, 2, 3), &mut cancel);
assert!(cancel, "a cancel takes the burn");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "bell_ring",
        lua: r#"horizon.on("bell_ring", function(event)
  assert(event.player == "00000000-0000-0000-0000-000000000007", "player")
  assert(event.block == "minecraft:stone", "block")
  assert(event.pos.x == 0 and event.pos.y == 64 and event.pos.z == 0, "pos")
  horizon.cancel()
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_bell_ring", LUA);
let mods = boxed(m);
let mut cancel = false;
dispatch_bell_ring(&FakeLevel, &mods, LevelDimension::Overworld, Some(7), BlockPos::new(0, 64, 0), &mut cancel);
assert!(cancel, "a cancel stills the bell");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "arrow_loose",
        lua: r#"horizon.on("arrow_loose", function(event)
  assert(event.stack.id == "minecraft:bow", "item")
  assert(event.has_ammo == true, "ammo")
  assert(event.charge == 0, "charge")
  event.charge = 3
  horizon.cancel()
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_arrow_loose", LUA);
let mods = boxed(m);
let stack = ItemStack { item: "minecraft:bow".into(), count: 1, damage: 0, extra: None };
let mut charge = 0;
let mut canceled = false;
dispatch_arrow_loose(&mut FakeHost::empty(), &mods, &stack, true, &mut charge, &mut canceled);
assert!(canceled, "a cancel holds the arrow");
assert_eq!(charge, 3, "the edited charge is written back");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "block_lock_check",
        lua: r#"horizon.on("block_lock_check", function(event)
  assert(event.player.name == "Alex", "player")
  assert(event.pos.x == 5 and event.pos.y == 6 and event.pos.z == 7, "pos")
  horizon.cancel()
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_block_lock_check", LUA);
let mods = boxed(m);
let mut locked = false;
dispatch_block_lock_check(&mut FakeHost::empty(), &mods, 7, "Alex", [5, 6, 7], &mut locked);
assert!(locked, "a cancel keeps the lock");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "component_types",
        lua: r#"horizon.on("component_types", function(event)
  assert(event.types[1] == "minecraft:existing", "seed")
  table.insert(event.types, "ruby:part")
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_component_types", LUA);
let mods = boxed(m);
let mut types = vec!["minecraft:existing".to_owned()];
dispatch_component_types(&mods, &mut types);
assert_eq!(types, ["minecraft:existing", "ruby:part"]);
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "datapack_sync",
        lua: r#"horizon.on("datapack_sync", function(event)
  assert(event.recipe_types[1] == "minecraft:crafting", "seed")
  event.recipe_types[1] = "ruby:pressing"
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_datapack_sync", LUA);
let mods = boxed(m);
let mut types = vec!["minecraft:crafting".to_owned()];
dispatch_datapack_sync(&mut FakeHost::empty(), &mods, &mut types);
assert_eq!(types, ["ruby:pressing"]);
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "screen_tooltip",
        lua: r#"horizon.on("screen_tooltip", function(event)
  assert(event.block == "ruby:gauge", "block")
  assert(event.data[1] == 4 and event.data[2] == 9, "data")
  assert(event.at.x == 1.5 and event.at.y == 2.5, "at")
  assert(event.lines[1] == "old", "lines")
  event.lines[1] = "new"
  table.insert(event.lines, "added")
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_screen_tooltip", LUA);
let mods = boxed(m);
let mut lines = vec!["old".to_owned()];
dispatch_screen_tooltip(&FakeHost::empty(), &mods, "ruby:gauge", &[4, 9], [1.5, 2.5], &mut lines);
assert_eq!(lines, ["new", "added"]);
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "is_bonemeal_success",
        lua: r#"horizon.on("is_bonemeal_success", function(event)
  assert(event.dimension == "minecraft:overworld", "dimension")
  assert(event.block == "minecraft:stone", "block")
  assert(event.success == false, "seed")
  event.success = true
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_is_bonemeal_success", LUA);
let mods = boxed(m);
let mut success = false;
dispatch_is_bonemeal_success(&mut FakeLevel, &mods, LevelDimension::Overworld, BlockPos::new(1, 2, 3), BlockStateId(7), &mut success);
assert!(success, "the grown flag is written back");
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "item_fluid_tank",
        lua: r#"horizon.on("item_fluid_tank", function(event)
  assert(event.item == "ruby:drum", "item")
  assert(event.capacity == nil, "seed")
  event.capacity = 81000
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_item_fluid_tank", LUA);
let mods = boxed(m);
let mut capacity = None;
dispatch_item_fluid_tank(&FakeHost::empty(), &mods, "ruby:drum", &mut capacity);
assert_eq!(capacity, Some(81000));
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "tooltip_component",
        lua: r#"horizon.on("tooltip_component", function(event)
  assert(event.stack.id == "ruby:gauge", "item")
  assert(event.size == nil, "seed")
  event.size = { x = 16, y = 8 }
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_tooltip_component", LUA);
let mods = boxed(m);
let stack = ItemStack { item: "ruby:gauge".into(), count: 1, damage: 0, extra: None };
let mut size = None;
dispatch_tooltip_component(&FakeHost::empty(), &mods, &stack, &mut size);
assert_eq!(size, Some([16.0, 8.0]));
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "screen_click",
        lua: r#"horizon.on("screen_click", function(event)
  assert(event.block == "ruby:gauge", "block")
  assert(event.data[1] == 4, "data")
  assert(event.right == true, "button kind")
  assert(event.button == nil, "seed")
  event.button = 2
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_screen_click", LUA);
let mods = boxed(m);
let mut button = None;
dispatch_screen_click(&FakeHost::empty(), &mods, "ruby:gauge", &[4], [1.5, 2.5], true, &mut button);
assert_eq!(button, Some(2));
no_error(&id); drop_dir(dir);"#,
    },
    HookTest {
        hook: "prepare_anvil",
        lua: r#"horizon.on("prepare_anvil", function(event)
  assert(event.player.name == "Alex", "player")
  assert(event.cost == 5, "cost")
  assert(event.result == nil, "seed")
  event.result = { id = "minecraft:diamond_sword", count = 1 }
  event.cost = 9
end)"#,
        rust: r#"let (m, id, dir) = loaded("gen_prepare_anvil", LUA);
let mods = boxed(m);
let mut result = None;
let mut cost = 5;
dispatch_prepare_anvil(&mut FakeHost::empty(), &mods, 7, "Alex", [5, 6, 7], &mut cost, &mut result);
assert_eq!(cost, 9, "the edited cost is written back");
let result = result.expect("a replaced result");
assert_eq!(&*result.item, "minecraft:diamond_sword");
assert_eq!(result.count, 1);
no_error(&id); drop_dir(dir);"#,
    },
];

/// The fixed prologue of the generated file: the name list, the table helpers
/// the methods share, and the macro that holds them.
fn prologue() -> String {
    let mut names = String::from("pub const GENERATED_HOOK_NAMES: &[&str] = &[\n");
    for hook in GENERATED {
        names.push_str(&format!("    \"{hook}\",\n"));
    }
    names.push_str("];\n");
    format!(
        "//! Generated by `cargo run -p xtask -- lua-hooks` from native-hooks.json and the\n\
         //! native functions' signatures: edit xtask/src/lua_hooks.rs, then regenerate.\n\n\
         /// The hooks the generator wires into `horizon.on`: the native names, one\n\
         /// event table per hook. `horizon.on` rejects a kind for these: the spike\n\
         /// answers every event to every handler.\n\
         {names}\n\
         /// `{{x, y, z}}` of a block position, as the hand-written hooks name it.\n\
         pub(crate) fn pos_table(lua: &mlua::Lua, pos: [i32; 3]) -> mlua::Result<mlua::Table> {{\n\
         let t = lua.create_table()?;\n\
         t.set(\"x\", pos[0])?;\n\
         t.set(\"y\", pos[1])?;\n\
         t.set(\"z\", pos[2])?;\n\
         Ok(t)\n\
         }}\n\n\
         /// `{{x, y}}` of a 2D float pair (`screen_click`'s `at`).\n\
         pub(crate) fn pair_table(lua: &mlua::Lua, pos: [f32; 2]) -> mlua::Result<mlua::Table> {{\n\
         let t = lua.create_table()?;\n\
         t.set(\"x\", pos[0])?;\n\
         t.set(\"y\", pos[1])?;\n\
         Ok(t)\n\
         }}\n\n\
         /// A number field back as a float: scripts write `16` or `16.0` alike.\n\
         pub(crate) fn read_number(t: &mlua::Table, key: &str) -> mlua::Result<f32> {{\n\
         match t.get::<mlua::Value>(key)? {{\n\
         mlua::Value::Integer(n) => Ok(n as f32),\n\
         mlua::Value::Number(n) => Ok(n as f32),\n\
         v => Err(mlua::Error::runtime(format!(\"{{key}}: expected a number, got {{}}\", v.type_name()))),\n\
         }}\n\
         }}\n\
         /// A `{{x, y}}` table back into a float pair.\n\
         pub(crate) fn read_pair(t: &mlua::Table) -> mlua::Result<[f32; 2]> {{\n\
         Ok([read_number(t, \"x\")?, read_number(t, \"y\")?])\n\
         }}\n\n\
         /// An item stack from its table: `id`, `count`, `damage`. Extra\n\
         /// components do not survive: a replaced stack is plain.\n\
         pub(crate) fn item_from_table(table: &mlua::Table) -> mlua::Result<horizon_client::mod_api::ItemStack> {{\n\
         Ok(horizon_client::mod_api::ItemStack {{\n\
         item: std::sync::Arc::from(table.get::<String>(\"id\")?.as_str()),\n\
         count: table.get::<Option<u32>>(\"count\")?.unwrap_or(1),\n\
         damage: table.get::<Option<u32>>(\"damage\")?.unwrap_or(0),\n\
         extra: None,\n\
         }})\n\
         }}\n\n\
         /// A string slice as a sequence table.\n\
         pub(crate) fn string_seq(lua: &mlua::Lua, items: &[String]) -> mlua::Result<mlua::Table> {{\n\
         lua.create_sequence_from(items.iter().map(String::as_str))\n\
         }}\n\n\
         /// An int slice as a sequence table.\n\
         pub(crate) fn int_seq(lua: &mlua::Lua, items: &[i32]) -> mlua::Result<mlua::Table> {{\n\
         lua.create_sequence_from(items.iter().copied())\n\
         }}\n\n\
         /// A string sequence table back into a vector.\n\
         pub(crate) fn read_string_seq(t: &mlua::Table) -> mlua::Result<Vec<String>> {{\n\
         t.sequence_values::<String>().collect()\n\
         }}\n\n\
         /// The generated `Hooks` methods, invoked once inside\n\
         /// `impl Hooks for LuaMod`: one impl block keeps every method, as a\n\
         /// trait cannot be implemented twice.\n\
         macro_rules! impl_generated_hooks {{\n() => {{\n"
    )
}

/// The fixed test harness of the generated file: the mod loader, the fake
/// hosts, and the no-handler test. Per-hook tests follow.
fn test_prologue() -> String {
    String::from(
        "#[cfg(test)]\n\
         mod generated_tests {\n\
         use crate::lua_mods::LuaMod;\n\
         use horizon_client::dev_input::mods::{ModStatus, set_status, status};\n\
         use horizon_client::mod_api::{\n\
         AttachmentError, Attachments, BlockPos, BlockStateId, Holder, Hooks, Host, ItemStack,\n\
         LevelDimension, LevelHost, LevelRandomHost, NativeServerPlayer,\n\
         dispatch_anvil_damaged, dispatch_arrow_loose, dispatch_bell_ring, dispatch_block_burn,\n\
         dispatch_block_cook, dispatch_block_lock_check, dispatch_chunk_load, dispatch_component_types,\n\
         dispatch_datapack_sync, dispatch_is_bonemeal_success, dispatch_item_fluid_tank,\n\
         dispatch_level_tick_pre, dispatch_player_container_close, dispatch_prepare_anvil,\n\
         dispatch_screen_click, dispatch_screen_tooltip, dispatch_tooltip_component,\n\
         };\n\
         use horizon_mods::Package;\n\
         use std::path::PathBuf;\n\n\
         const MANIFEST_TEXT: &str = \"id = \\\"ruby\\\"\\nname = \\\"Ruby\\\"\\nversion = \\\"0.1.0\\\"\\napi = \\\"0.2\\\"\\nside = \\\"both\\\"\\ncapabilities = [\\\"scripts\\\"]\\nexperimental = [\\\"scripts\\\"]\\n\";\n\n\
         fn hmod(name: &str, lua: &str) -> (PathBuf, PathBuf) {\n\
         let dir = std::env::temp_dir().join(format!(\"horizon-lua-gen-{name}-{}\", std::process::id()));\n\
         let _ = std::fs::remove_dir_all(&dir);\n\
         std::fs::create_dir_all(&dir).unwrap();\n\
         let path = dir.join(\"ruby.hmod\");\n\
         std::fs::write(&path, Package::write_files(MANIFEST_TEXT, None, &[(\"scripts/init.lua\", lua.as_bytes())]).unwrap()).unwrap();\n\
         (dir, path)\n\
         }\n\n\
         /// Load a mod running `lua`, with a clean status row: `no_error` then\n\
         /// tells a handler's `assert` from silence.\n\
         fn loaded(name: &str, lua: &str) -> (LuaMod, String, PathBuf) {\n\
         let (dir, path) = hmod(name, lua);\n\
         let m = LuaMod::load(&path, &[]).unwrap();\n\
         let id = m.id().to_owned();\n\
         set_status(ModStatus { id: id.clone(), ..Default::default() });\n\
         (m, id, dir)\n\
         }\n\n\
         fn boxed(m: LuaMod) -> Vec<Box<dyn Hooks>> {\n\
         vec![Box::new(m) as Box<dyn Hooks>]\n\
         }\n\n\
         fn drop_dir(dir: PathBuf) {\n\
         std::fs::remove_dir_all(&dir).unwrap();\n\
         }\n\n\
         /// No `error` row for `id`: every `assert` in its handlers held.\n\
         fn no_error(id: &str) {\n\
         assert!(status().into_iter().find(|s| s.id == id).and_then(|s| s.error).is_none(), \"a handler failed: {:?}\", status().into_iter().find(|s| s.id == id).and_then(|s| s.error));\n\
         }\n\n\
         // A game host with no players on it: `player_by_uuid` answers the\n\
// uuid and the name the hook told, as for a player out of reach (the\n\
// engine's `NativeServerPlayer` is `#[non_exhaustive]`, so a test\n\
// outside its crate cannot build the full table).\n\
struct FakeHost;\n\
impl FakeHost {\n\
fn empty() -> Self {\n\
Self\n\
}\n\
}\n\
impl Host for FakeHost {\n\
         fn level_players(&self) -> Vec<NativeServerPlayer> {\n\
         Vec::new()\n\
         }\n\
         }\n\
         /// A level of one stone block.\n\
         struct FakeLevel;\n\
         impl LevelHost for FakeLevel {\n\
         fn block_state(&self, _: [i32; 3]) -> BlockStateId {\n\
         BlockStateId(7)\n\
         }\n\
         fn is_client_side(&self) -> bool {\n\
         false\n\
         }\n\
         fn state_with_property(&self, _: BlockStateId, _: &str, _: &str) -> Option<BlockStateId> {\n\
         None\n\
         }\n\
         fn mod_block_entity_data(&self, _: [i32; 3]) -> Option<horizon_client::mod_api::Compound> {\n\
         None\n\
         }\n\
         fn state_block(&self, state: BlockStateId) -> Option<String> {\n\
         (state == BlockStateId(7)).then(|| \"minecraft:stone\".to_owned())\n\
         }\n\
         fn state_properties(&self, _: BlockStateId) -> Vec<(String, String)> {\n\
         Vec::new()\n\
         }\n\
         }\n\
         impl LevelRandomHost for FakeLevel {\n\
         fn attachments(&mut self, _: Holder) -> Result<&mut Attachments, AttachmentError> {\n\
         Err(AttachmentError::MissingHolder)\n\
         }\n\
         fn random(&mut self) -> Option<&mut horizon_client::mod_api::LegacyRandom> {\n\
         None\n\
         }\n\
         }\n",
    )
}

/// Emit the generated file: the prologue, one method per hook in a macro, the
/// test harness, one test per hook, and the no-handler test.
fn generate(root: &Path) -> Result<String, String> {
    let catalog: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("native-hooks.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let hooks = catalog_hooks(root, &catalog)?;
    let by_name: BTreeMap<&str, &serde_json::Value> = hooks
        .iter()
        .map(|h| (h["name"].as_str().unwrap_or_default(), h))
        .collect();
    for skipped in SKIPPED {
        if !by_name.contains_key(skipped.0) {
            return Err(format!("{}: no such hook to skip", skipped.0));
        }
        if GENERATED.contains(&skipped.0) {
            return Err(format!("{}: both generated and skipped", skipped.0));
        }
    }
    let game = game_methods(root)?;
    let mut frees: BTreeMap<String, Vec<syn::ItemFn>> = BTreeMap::new();
    let mut methods = String::new();
    for name in GENERATED {
        let hook = by_name
            .get(name)
            .ok_or_else(|| format!("{name}: no such hook"))?;
        let model = model_of(root, hook, &game, &mut frees)?;
        methods.push_str(&method_of(&model)?);
    }
    let mut tests = String::new();
    for test in HOOK_TESTS {
        if !GENERATED.contains(&test.hook) {
            return Err(format!("{}: tested but not generated", test.hook));
        }
        tests.push_str(&format!(
            "#[test]\nfn generated_{}() {{\nconst LUA: &str = r#\"{}\"#;\n{}\n}}\n",
            test.hook, test.lua, test.rust
        ));
    }
    let mut out = prologue();
    out.push_str(&methods);
    out.push_str("} }\n");
    out.push_str(&test_prologue());
    out.push_str(&tests);
    out.push_str(&no_handler_test());
    out.push_str("}\n");
    rustfmt(&out)
}

/// A mod registering nothing, fired through every generated dispatch: cancel
/// stays quiet, collected values keep their seed, replaced values stay None.
fn no_handler_test() -> String {
    String::from(
        "#[test]\n\
         fn generated_no_handler_does_no_lua_work() {\n\
         const LUA: &str = \"-- nothing registered\";\n\
         let (m, id, dir) = loaded(\"gen_no_handler\", LUA);\n\
         let mods = boxed(m);\n\
         dispatch_level_tick_pre(&mut FakeHost::empty(), &mods, \"minecraft:overworld\");\n\
         dispatch_chunk_load(&mut FakeHost::empty(), &mods, \"minecraft:overworld\", [0, 0], false);\n\
         dispatch_block_cook(&FakeHost::empty(), &mods, [0, 0, 0], \"minecraft:wheat\", \"minecraft:bread\");\n\
         dispatch_anvil_damaged(&mut FakeHost::empty(), &mods, \"minecraft:overworld\", [0, 0, 0], \"minecraft:anvil\", \"minecraft:chipped_anvil\");\n\
         dispatch_player_container_close(&mut FakeHost::empty(), &mods, 7, \"Alex\", \"minecraft:overworld\", \"minecraft:chest\", None);\n\
         let mut cancel = false;\n\
         dispatch_block_burn(&FakeLevel, &mods, LevelDimension::Overworld, BlockPos::new(0, 0, 0), &mut cancel);\n\
         assert!(!cancel);\n\
         dispatch_bell_ring(&FakeLevel, &mods, LevelDimension::Overworld, None, BlockPos::new(0, 0, 0), &mut cancel);\n\
         assert!(!cancel);\n\
         let stack = ItemStack { item: \"minecraft:bow\".into(), count: 1, damage: 0, extra: None };\n\
         let mut charge = 0;\n\
         let mut canceled = false;\n\
         dispatch_arrow_loose(&mut FakeHost::empty(), &mods, &stack, true, &mut charge, &mut canceled);\n\
         assert!(!canceled && charge == 0);\n\
         let mut locked = false;\n\
         dispatch_block_lock_check(&mut FakeHost::empty(), &mods, 7, \"Alex\", [0, 0, 0], &mut locked);\n\
         assert!(!locked);\n\
         let mut types = vec![\"minecraft:existing\".to_owned()];\n\
         dispatch_component_types(&mods, &mut types);\n\
         assert_eq!(types, [\"minecraft:existing\"]);\n\
         let mut recipes = vec![\"minecraft:crafting\".to_owned()];\n\
         dispatch_datapack_sync(&mut FakeHost::empty(), &mods, &mut recipes);\n\
         assert_eq!(recipes, [\"minecraft:crafting\"]);\n\
         let mut lines = vec![\"old\".to_owned()];\n\
         dispatch_screen_tooltip(&FakeHost::empty(), &mods, \"ruby:gauge\", &[4], [0.0, 0.0], &mut lines);\n\
         assert_eq!(lines, [\"old\"]);\n\
         let mut success = false;\n\
         dispatch_is_bonemeal_success(&mut FakeLevel, &mods, LevelDimension::Overworld, BlockPos::new(0, 0, 0), BlockStateId(7), &mut success);\n\
         assert!(!success);\n\
         let mut capacity = None;\n\
         dispatch_item_fluid_tank(&FakeHost::empty(), &mods, \"ruby:drum\", &mut capacity);\n\
         assert_eq!(capacity, None);\n\
         let mut size = None;\n\
         dispatch_tooltip_component(&FakeHost::empty(), &mods, &stack, &mut size);\n\
         assert_eq!(size, None);\n\
         let mut button = None;\n\
         dispatch_screen_click(&FakeHost::empty(), &mods, \"ruby:gauge\", &[4], [0.0, 0.0], true, &mut button);\n\
         assert_eq!(button, None);\n\
         let mut result = None;\n\
         let mut cost = 5;\n\
         dispatch_prepare_anvil(&mut FakeHost::empty(), &mods, 7, \"Alex\", [0, 0, 0], &mut cost, &mut result);\n\
         assert!(result.is_none() && cost == 5);\n\
         no_error(&id); drop_dir(dir);\n\
         }\n",
    )
}

/// `code` as `cargo fmt` formats it.
fn rustfmt(code: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = std::process::Command::new("rustfmt")
        .args(["--emit", "stdout", "--edition", "2024"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("rustfmt stdin")?
        .write_all(code.as_bytes())
        .map_err(|e| e.to_string())?;
    let formatted = child.wait_with_output().map_err(|e| e.to_string())?;
    if !formatted.status.success() {
        std::fs::write("/tmp/lua_hooks_raw.rs", code).map_err(|e| e.to_string())?;
        if let Err(e) = syn::parse_file(code) {
            return Err(format!("generated code does not parse: {e}"));
        }
        return Err(format!(
            "rustfmt failed: {}",
            String::from_utf8_lossy(&formatted.stderr)
        ));
    }
    String::from_utf8(formatted.stdout).map_err(|e| e.to_string())
}

pub fn run(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let out = generate(&root)?;
    let path = root.join("crates/event-horizon-lua/src/generated_hooks.rs");
    if check {
        let current = fs::read_to_string(&path).unwrap_or_default();
        if current != out {
            return Err(format!(
                "{} is out of date: run cargo run -p xtask -- lua-hooks",
                path.display()
            ));
        }
    } else {
        fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The generated file is what the generator says: the spike cannot drift
    /// from its checked-in output unnoticed.
    #[test]
    fn the_generated_file_is_what_the_generator_says() {
        super::run(true).unwrap();
    }

    /// Every hook the generator names exists, is unclaimed by the hand-written
    /// runtime, and takes no kind-filtered Lua name already.
    #[test]
    fn generated_hooks_are_new_and_known() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let catalog: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("native-hooks.json")).unwrap())
                .unwrap();
        let hooks = super::catalog_hooks(&root, &catalog).unwrap();
        let names: std::collections::BTreeSet<&str> = hooks
            .iter()
            .map(|h| h["name"].as_str().unwrap_or_default())
            .collect();
        let runtime =
            std::fs::read_to_string(root.join("crates/event-horizon-lua/src/lua_mods.rs")).unwrap();
        let start = runtime.find("pub const LUA_HOOK_NAMES").unwrap();
        let end = start + runtime[start..].find("];").unwrap();
        for name in super::GENERATED {
            assert!(names.contains(name), "{name}: no such hook");
            assert!(
                !runtime[start..end].contains(&format!("\"{name}\"")),
                "{name}: the hand-written runtime already reaches it"
            );
        }
        for test in super::HOOK_TESTS {
            assert!(
                super::GENERATED.contains(&test.hook),
                "{}: tested but not generated",
                test.hook
            );
        }
    }
}
