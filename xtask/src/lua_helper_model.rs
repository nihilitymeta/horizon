//! `cargo run -p xtask -- lua-helpers`: the model under the helper
//! generator. It resolves every native helper's signature with syn (from
//! `native-hooks.json` and `tools/native-hook-fragments/`, the same sources
//! `hooks_doc` reads), and classifies each into exactly one class:
//! [`HelperClass::Generated`] (plain args and a supported return; the
//! generator can emit the `horizon.*` bridge), [`HelperClass::AlreadyLua`]
//! (already a hand-written Lua helper), [`HelperClass::Adapter`] (a
//! missing Lua conversion, with the blocking types), [`HelperClass::Handle`]
//! (an opaque draw/mod handle), or [`HelperClass::Cannot`] (no bridge, with
//! the reason). The generated `Host` trait in `horizon-client` carries the
//! same `native_*` signatures without the prefix; the Lua bridge emits
//! whatever the trait declares.

use horizon_mod_api_gen::{free_function_source, impl_functions, one_function};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::{FnArg, Item, ReturnType, Type};

/// The helpers `lua-helpers` can emit today: plain arguments, a plain
/// return, the name not already a hand-written Lua helper. `Read` for
/// `&self` (a value travels back), `Action` for `&mut self` (the return is
/// dropped, as every hand-written queued action drops it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenKind {
    Read,
    Action,
}

/// Every helper lands in exactly one class.
#[derive(Clone, Debug)]
pub enum HelperClass {
    Generated(GenKind),
    AlreadyLua,
    /// A hand-written converter would unblock it: the blocking types.
    Adapter(Vec<String>),
    /// An opaque draw/mod handle: the types that block.
    Handle(Vec<String>),
    /// No bridge exists: the reason.
    Cannot(String),
}

/// One argument of a helper, in the model's terms (the Lua type the
/// emitted bridge reads it as, and the normalized Rust scalar it converts
/// to). `player` marks a `u128` argument, read through `uuid_of` (a player
/// table, uuid text, or a player name, like every hand-written action's
/// first argument); `entity` marks an [`EntityRef`] argument, read as an
/// entity id number and resolved through the `Host::entity` lookup.
#[derive(Clone, Debug)]
pub struct Arg {
    /// The native argument's own name, for the emitted local.
    pub name: String,
    /// The normalized Rust type: `u128` for a player, `EntityRef` for an
    /// entity, `[f64; 3]`/`[i32; 3]` for a position table, `&str`, an int,
    /// `f32`, `f64`, `bool`, or `String` for a by-value string.
    pub rust: String,
    /// True when the argument is a player (a `u128`), read via `uuid_of`.
    pub player: bool,
    /// True when the argument is an entity (an `EntityRef`).
    pub entity: bool,
}

/// One resolved helper: its signature, class, and doc.
pub struct HelperRow {
    pub name: String,
    /// The native target (`horizon_client::game::...::native_<name>`).
    pub target: String,
    pub class: HelperClass,
    /// The normalized arguments (empty unless Generated).
    pub args: Vec<Arg>,
    /// The normalized return: `unit`, a scalar (`bool`, `String`, an int,
    /// `f32`, `f64`), a position (`[f64; 3]`/`[i32; 3]`), or
    /// `Option<scalar>`/`Option<[f64; 3]>`/`Option<[i32; 3]>`
    /// (empty unless Generated).
    pub ret: String,
    /// The `native_*` function's prose (its doc, first paragraph).
    pub doc: String,
    /// The `API n.` its doc names, when it names one.
    pub api: Option<u32>,
    /// The `receiver` the catalog names (`entity`, `level`), when it does.
    pub receiver: Option<String>,
    /// Whether the native receiver is `&mut self` (the `Host` trait's
    /// method takes `&mut self` for it, `&self` otherwise).
    pub receiver_mut: bool,
    /// Whether the native return is a string borrow (`&str` or
    /// `Option<&str>`, normalized to `String`): the `Host` trait's
    /// method returns the borrow, not an owned `String`.
    pub ret_borrowed: bool,
    /// Whether the borrowed return is a `'static` str (an explicit
    /// `&'static str` in the native signature, like `facing`'s
    /// `Option<&'static str>`): the double must spell the same
    /// lifetime, or the `impl` signature does not match the trait.
    pub ret_static: bool,
}

fn prose(attrs: &[syn::Attribute], name: &str) -> Result<String, String> {
    let docs = horizon_mod_api_gen::doc_comments(attrs, name, true)?;
    let mut out = String::new();
    for line in docs.lines() {
        let line = line.trim_start_matches("///").trim();
        if line.is_empty() {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    Ok(out)
}

fn api_of(prose: &str) -> Option<u32> {
    let mut last = None;
    let mut rest = prose;
    while let Some(i) = rest.find("API ") {
        let after = &rest[i + 4..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u32>() {
            last = Some(n);
        }
        rest = &after[digits.len()..];
    }
    last
}

/// The plain scalar a normalized type names: an int, a float, `bool`,
/// `&str`, or a by-value `String`. Everything else is a blocking type.
fn scalar_of(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 => {
            match p.path.segments[0].ident.to_string().as_str() {
                "i8" => Some("i8"),
                "i16" => Some("i16"),
                "i32" => Some("i32"),
                "i64" => Some("i64"),
                "i128" => Some("i128"),
                "isize" => Some("isize"),
                "u8" => Some("u8"),
                "u16" => Some("u16"),
                "u32" => Some("u32"),
                "u64" => Some("u64"),
                "usize" => Some("usize"),
                "u128" => Some("u128"),
                "f32" => Some("f32"),
                "f64" => Some("f64"),
                "bool" => Some("bool"),
                "String" => Some("String"),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether the type is an opaque `&mut` handle (anything mutable except
/// the receiver, which the caller handles): a borrow Lua cannot name.
fn is_mut_handle(ty: &Type) -> bool {
    matches!(ty, Type::Reference(r) if r.mutability.is_some() && scalar_of(&r.elem).is_none())
}

/// The head type token of a non-plain type, for the blocking list:
/// `Vec<X>` names `Vec<X>`, `&ItemStack` names `ItemStack`, `[f64; 3]`
/// names `[f64; 3]`, a by-value struct names itself. (A small renderer
/// of its own: xtask has no `quote`.)
fn blocking_head(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => blocking_head(&r.elem),
        Type::Array(a) => {
            let len = match &a.len {
                syn::Expr::Lit(lit) => match &lit.lit {
                    syn::Lit::Int(n) => n.base10_digits().to_owned(),
                    _ => "N".to_owned(),
                },
                _ => "N".to_owned(),
            };
            format!("[{}; {len}]", blocking_head(&a.elem))
        }
        Type::Slice(s) => format!("[{}]", blocking_head(&s.elem)),
        Type::Path(p) => {
            let Some(seg) = p.path.segments.last() else {
                return String::new();
            };
            let name = seg.ident.to_string();
            match &seg.arguments {
                syn::PathArguments::AngleBracketed(args) if !args.args.is_empty() => {
                    let generics: Vec<String> = args
                        .args
                        .iter()
                        .filter_map(|arg| match arg {
                            syn::GenericArgument::Type(inner) => Some(blocking_head(inner)),
                            _ => None,
                        })
                        .collect();
                    format!("{name}<{}>", generics.join(", "))
                }
                _ => name,
            }
        }
        Type::Tuple(t) => format!("({} elements)", t.elems.len()),
        Type::TraitObject(t) => {
            let name = t
                .bounds
                .first()
                .and_then(|bound| match bound {
                    syn::TypeParamBound::Trait(bound) => {
                        bound.path.segments.last().map(|seg| seg.ident.to_string())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            format!("dyn {name} (callback)")
        }
        Type::BareFn(_) => "fn pointer (callback)".to_owned(),
        _ => "<type>".to_owned(),
    }
}

/// Whether a type is a plain scalar-or-string `Option`: the return side
/// of a generated read (a value travels back, or nil).
fn is_plain_option(ty: &Type) -> bool {
    let Type::Path(p) = ty else {
        return false;
    };
    let Some(seg) = p.path.segments.last() else {
        return false;
    };
    if seg.ident != "Option" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return false;
    };
    let Some(syn::GenericArgument::Type(inner)) = args.args.first() else {
        return false;
    };
    if scalar_of(inner).is_some() {
        return true;
    }
    let Type::Reference(r) = inner else {
        return false;
    };
    let Type::Path(ip) = &*r.elem else {
        return false;
    };
    ip.path
        .segments
        .last()
        .is_some_and(|s| s.ident == "str" || s.ident == "String")
}

/// Whether the type is the by-value [`EntityRef`] handle: a Lua number
/// (the entity's network id) resolved through `Host::entity`.
fn is_entity_ref(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.qself.is_none()
        && p.path.segments.len() == 1
        && p.path.segments[0].ident == "EntityRef")
}

/// The position layout a type carries: `[f64; 3]` or `[i32; 3]` (a
/// `{x, y, z}` table), or `None` for anything else.
fn vec3_layout(ty: &Type) -> Option<&'static str> {
    let Type::Array(a) = ty else {
        return None;
    };
    let syn::Expr::Lit(lit) = &a.len else {
        return None;
    };
    let syn::Lit::Int(n) = &lit.lit else {
        return None;
    };
    if n.base10_digits() != "3" {
        return None;
    }
    let Type::Path(p) = &*a.elem else {
        return None;
    };
    let name = p.path.segments.last()?.ident.to_string();
    match name.as_str() {
        "f64" => Some("[f64; 3]"),
        "i32" => Some("[i32; 3]"),
        _ => None,
    }
}

/// The position layout of an `Option<[f64; 3]>`/`Option<[i32; 3]>` return,
/// or `None` (its `nil`-for-`None` is the value-or-nil a read answers).
fn opt_vec3_layout(ty: &Type) -> Option<&'static str> {
    let Type::Path(p) = ty else {
        return None;
    };
    let seg = p.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let Some(syn::GenericArgument::Type(inner)) = args.args.first() else {
        return None;
    };
    vec3_layout(inner)
}

/// Whether the type is a plain string borrow (`&str`, `&'static str`,
/// `&String`): the bridge answers it as its owned `String`.
fn str_borrow(ty: &Type) -> bool {
    let Type::Reference(r) = ty else {
        return false;
    };
    let Type::Path(p) = &*r.elem else {
        return false;
    };
    p.path
        .segments
        .last()
        .is_some_and(|s| s.ident == "str" || s.ident == "String")
}

/// The specific reason of a callback construct, naming the trait or the
/// pointer the signature carries (e.g. `(&dyn Fn)`).
fn callback_reason(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => callback_reason(&r.elem),
        Type::TraitObject(t) => {
            let name = t
                .bounds
                .first()
                .and_then(|bound| match bound {
                    syn::TypeParamBound::Trait(bound) => {
                        bound.path.segments.last().map(|seg| seg.ident.to_string())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "Fn".to_owned());
            format!("a callback parameter (&dyn {name})")
        }
        Type::BareFn(_) => "a callback parameter (fn pointer)".to_owned(),
        _ => format!("a callback parameter ({})", blocking_head(ty)),
    }
}

/// One argument or return type's place in the classes: a plain string
/// borrow or scalar is nothing (the caller handles the receiver); an
/// entity id (`EntityRef`) or a position table (`[f64; 3]`/`[i32; 3]`)
/// is nothing either (the bridge converts it); an opaque `&mut` is a
/// handle; a callback, a trait object, or a tuple is beyond the bridge
/// (a design of its own); any other type blocks on the type it names.
/// An `Option<scalar>` blocks only in an argument: Lua has no absent
/// argument (a `None`-for-nil write needs the optional-parameter
/// design); in a return it is exactly the value-or-nil a read answers,
/// as an optional position (`Option<[f64; 3]>`/`Option<[i32; 3]>`) is.
fn collect_blocks(
    ty: &Type,
    in_return: bool,
    blocking: &mut Vec<String>,
    handles: &mut Vec<String>,
    cannot: &mut Vec<String>,
) {
    if is_mut_handle(ty) {
        handles.push(blocking_head(ty));
        return;
    }
    // The converted types: an entity id number (`EntityRef`), a position
    // table (`[f64; 3]`/`[i32; 3]`), and the value-or-nil of an optional
    // position in a return. An optional position as an argument still
    // blocks: Lua has no absent argument.
    if is_entity_ref(ty)
        || vec3_layout(ty).is_some()
        || (in_return && opt_vec3_layout(ty).is_some())
    {
        return;
    }
    match ty {
        Type::Reference(r) => {
            // `&str` and `&String` are the plain string argument;
            // any other borrow is a struct borrow, blocking on the
            // struct itself. A borrowed trait object (`&dyn Fn`) is a
            // callback: it needs its own design.
            if let Type::Path(p) = &*r.elem
                && let Some(seg) = p.path.segments.last()
                && (seg.ident == "str" || seg.ident == "String")
            {
                return;
            }
            if matches!(r.elem.as_ref(), Type::TraitObject(_)) {
                cannot.push(callback_reason(ty));
                return;
            }
            blocking.push(blocking_head(ty));
        }
        Type::Path(_) if scalar_of(ty).is_some() || (in_return && is_plain_option(ty)) => {}
        Type::BareFn(_) | Type::ImplTrait(_) | Type::TraitObject(_) => {
            cannot.push(callback_reason(ty))
        }
        Type::Tuple(_) => cannot.push(format!(
            "a multi-value return {}: the bridge is one value",
            blocking_head(ty)
        )),
        _ => blocking.push(blocking_head(ty)),
    }
}

/// The blocking-type list of a signature: every non-plain type in its
/// arguments and return, plus the `&mut`-handle and callback markers the
/// classes split on.
fn blocks_of(f: &syn::ItemFn) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut blocking = Vec::new();
    let mut handles = Vec::new();
    let mut cannot = Vec::new();
    for arg in &f.sig.inputs {
        if let FnArg::Typed(arg) = arg {
            collect_blocks(&arg.ty, false, &mut blocking, &mut handles, &mut cannot);
        }
    }
    if let ReturnType::Type(_, ty) = &f.sig.output {
        collect_blocks(ty, true, &mut blocking, &mut handles, &mut cannot);
    }
    blocking.sort();
    blocking.dedup();
    handles.sort();
    handles.dedup();
    cannot.sort();
    cannot.dedup();
    (blocking, handles, cannot)
}

/// The supported plain return, in normalized form: `unit`, a scalar
/// (`bool`, `String`, an int, `f32`, `f64`), a position (`[f64; 3]` or
/// `[i32; 3]`), a string borrow (`&str` as `String`), or
/// `Option<scalar>` (with `Option<&str>` as `Option<String>`), or an
/// optional position (`Option<[f64; 3]>`/`Option<[i32; 3]>`). `None` when
/// the return is not plain and supported (a wide integer a Lua number
/// cannot hold, `Option<EntityRef>` (the lookup the bridge uses, never
/// a Lua function), a `Result`, a nested `Option<other>`, a struct, ...).
fn supported_return(f: &syn::ItemFn) -> Option<String> {
    let ty = match &f.sig.output {
        ReturnType::Default => return Some("unit".to_owned()),
        ReturnType::Type(_, ty) => ty,
    };
    if str_borrow(ty) {
        return Some("String".to_owned());
    }
    if let Some(scalar) = scalar_of(ty) {
        return match scalar {
            "bool" | "String" | "f32" | "f64" => Some(scalar.to_owned()),
            "u128" | "i128" | "u64" => None,
            _ => Some(scalar.to_owned()),
        };
    }
    // `Option<T>`: only the scalars Lua answers (a bool, a number,
    // a string, a string borrow) map to a value-or-nil.
    if let Type::Path(p) = &**ty
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        if str_borrow(inner) {
            return Some("Option<String>".to_owned());
        }
        if let Some(inner_scalar) = scalar_of(inner) {
            return match inner_scalar {
                "u128" | "i128" | "u64" => None,
                inner_scalar => Some(format!("Option<{inner_scalar}>")),
            };
        }
        // An optional position: a table or nil, like the
        // `Option<[f32; 2]>` precedent; `Option<EntityRef>` stays
        // unsupported below (it is the `entity` lookup itself).
        if let Some(v) = opt_vec3_layout(ty) {
            return Some(format!("Option<{v}>"));
        }
    }
    // A plain position: a `{x, y, z}` table.
    if let Some(v) = vec3_layout(ty) {
        return Some(v.to_owned());
    }
    None
}

/// The plain arguments of a generated helper, in order (the receiver is
/// not an argument). Every `u128` names a player: the hand-written
/// pattern reads it through `uuid_of`.
fn plain_args(f: &syn::ItemFn) -> Vec<Arg> {
    let mut out = Vec::new();
    for arg in &f.sig.inputs {
        let FnArg::Typed(arg) = arg else { continue };
        let name = match &*arg.pat {
            syn::Pat::Ident(pat) => pat.ident.to_string(),
            _ => "value".to_owned(),
        };
        out.push(normalize_arg(&arg.ty, &name));
    }
    out
}

/// One argument in the model's terms, or the adapter block it names.
fn normalize_arg(ty: &Type, name: &str) -> Arg {
    match ty {
        Type::Reference(r) => {
            // `&str` / `&String`: the plain string argument.
            if let Type::Path(p) = &*r.elem
                && let Some(seg) = p.path.segments.last()
                && (seg.ident == "str" || seg.ident == "String")
            {
                return Arg {
                    name: name.to_owned(),
                    rust: "&str".to_owned(),
                    player: false,
                    entity: false,
                };
            }
            // Any other borrow blocks (the struct, not the borrow).
            Arg {
                name: name.to_owned(),
                rust: blocking_head(ty),
                player: false,
                entity: false,
            }
        }
        // An entity id number (`EntityRef`): the `Host::entity` lookup.
        Type::Path(_) if is_entity_ref(ty) => Arg {
            name: name.to_owned(),
            rust: "EntityRef".to_owned(),
            player: false,
            entity: true,
        },
        Type::Path(_) if scalar_of(ty).is_some() => {
            let scalar = scalar_of(ty).unwrap();
            match scalar {
                "u128" => Arg {
                    name: name.to_owned(),
                    rust: "u128".to_owned(),
                    player: true,
                    entity: false,
                },
                "String" => Arg {
                    name: name.to_owned(),
                    rust: "String".to_owned(),
                    player: false,
                    entity: false,
                },
                "bool" => Arg {
                    name: name.to_owned(),
                    rust: "bool".to_owned(),
                    player: false,
                    entity: false,
                },
                "f32" | "f64" => Arg {
                    name: name.to_owned(),
                    rust: scalar.to_owned(),
                    player: false,
                    entity: false,
                },
                _ => Arg {
                    name: name.to_owned(),
                    rust: scalar.to_owned(),
                    player: false,
                    entity: false,
                },
            }
        }
        // A position table (`[f64; 3]`/`[i32; 3]`).
        Type::Array(_) if vec3_layout(ty).is_some() => Arg {
            name: name.to_owned(),
            rust: vec3_layout(ty).unwrap().to_owned(),
            player: false,
            entity: false,
        },
        _ => Arg {
            name: name.to_owned(),
            rust: blocking_head(ty),
            player: false,
            entity: false,
        },
    }
}

/// All the helpers (`native-hooks.json` plus the pending fragments), with
/// their resolved signatures, in name order.
pub fn classify_helpers(root: &Path) -> Result<Vec<HelperRow>, String> {
    let catalog: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("native-hooks.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut helpers = catalog["helpers"]
        .as_array()
        .ok_or("catalog helpers must be an array")?
        .clone();
    helpers.extend(super::hooks_doc::fragment_helpers(root)?);
    helpers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let lua_names: BTreeSet<&str> = super::hooks_doc::lua_helper_names().into_iter().collect();
    let mut parsed: BTreeMap<String, Vec<(String, syn::ItemFn)>> = BTreeMap::new();
    let mut rows = Vec::new();
    for helper in &helpers {
        let name = helper["name"]
            .as_str()
            .ok_or("helper name missing")?
            .to_owned();
        let target = helper["target"]
            .as_str()
            .ok_or("helper target missing")?
            .to_owned();
        let native = target.rsplit("::").next().unwrap_or(&target).to_owned();
        let (file, free) = resolve_source(root, &target)?;
        let key = file.display().to_string();
        if !parsed.contains_key(&key) {
            let source = fs::read_to_string(&file).map_err(|e| format!("{key}: {e}"))?;
            let file_ast = syn::parse_file(&source).map_err(|e| format!("{key}: {e}"))?;
            let mut functions = Vec::new();
            for item in file_ast.items {
                match item {
                    Item::Fn(f) if free => functions.push((String::new(), f)),
                    Item::Impl(item) if !free => {
                        let self_ty = match &*item.self_ty {
                            Type::Path(p) => p
                                .path
                                .segments
                                .last()
                                .map(|s| s.ident.to_string())
                                .unwrap_or_default(),
                            _ => String::new(),
                        };
                        functions.extend(impl_functions(item).map(|f| (self_ty.clone(), f)));
                    }
                    _ => {}
                }
            }
            parsed.insert(key.clone(), functions);
        }
        let wanted_ty = target
            .split("::")
            .find(|seg| seg.contains('<'))
            .map(|seg| seg.split('<').next().unwrap_or_default().to_owned())
            .unwrap_or_default();
        let f = one_function(
            parsed[&key]
                .iter()
                .filter(|(ty, _)| free || *ty == wanted_ty)
                .map(|(_, f)| f),
            &native,
        )?;
        let doc = prose(&f.attrs, &native)?;
        let api = api_of(&doc);
        // The receiver: `&self`, `&mut self`, or none (a free function,
        // kept callable on `&self` in the trait). Anything else (an
        // owned `self`) is not object-safe.
        let mut receiver_mut = false;
        let mut receiver_owned = false;
        let mut has_receiver = false;
        for arg in &f.sig.inputs {
            if let FnArg::Receiver(r) = arg {
                has_receiver = true;
                receiver_mut = r.mutability.is_some();
                receiver_owned = r.reference.is_none();
                break;
            }
        }
        let receiver = helper["receiver"].as_str().map(str::to_owned);
        let ret_borrowed = return_is_borrowed(f);
        let ret_static = return_is_static_borrowed(f);
        if f.sig.asyncness.is_some() {
            rows.push(row(
                name,
                target,
                HelperClass::Cannot("async: the `Host` trait is object-safe".to_owned()),
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        if f.sig
            .generics
            .params
            .iter()
            .any(|p| !matches!(p, syn::GenericParam::Lifetime(_)))
        {
            rows.push(row(
                name,
                target,
                HelperClass::Cannot("generic: the `Host` trait is object-safe".to_owned()),
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        if receiver_owned {
            rows.push(row(
                name,
                target,
                HelperClass::Cannot("an owned `self` receiver".to_owned()),
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        if lua_names.contains(name.as_str()) {
            rows.push(row(
                name,
                target,
                HelperClass::AlreadyLua,
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        // A helper whose name is already a Lua `horizon.*` function
        // (hook registration, a hand-written read or action) cannot be
        // emitted under the same name: it would overwrite that
        // function. A hand-written helper keeps its `AlreadyLua` above;
        // anything else named reserved is a collision.
        if super::lua_types::reserved_horizon_name(&name) {
            rows.push(row(
                name,
                target,
                HelperClass::Cannot("name collides with an existing horizon.* function".to_owned()),
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        let (blocking, handles, cannot) = blocks_of(f);
        if !handles.is_empty() {
            rows.push(row(
                name,
                target,
                HelperClass::Handle(handles),
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        if !cannot.is_empty() {
            rows.push(row(
                name,
                target,
                HelperClass::Cannot(cannot.join("; ")),
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        let ret = supported_return(f);
        if is_result_type(&f.sig.output) {
            let head = match &f.sig.output {
                ReturnType::Type(_, ty) => blocking_head(ty),
                _ => "Result".to_owned(),
            };
            rows.push(cannot_row(
                name,
                target,
                &format!("returns {head}: no error design"),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        // Step 2: any non-plain argument or return is an Adapter, with
        // the return named too. `blocks_of` already names the return,
        // except a supported-looking scalar the bridge rejects (a wide
        // `u64`/`u128`/`i128` a Lua number cannot hold): name it as well.
        // A `level`/`living` `receiver` tag is a ref type the bridge
        // cannot name, so a Generated helper never carries one of those.
        // An `entity` tag names the helper's first argument, an
        // `EntityRef` (an id number the bridge converts), so it adds no
        // blocker of its own.
        let mut eff_blocking = blocking;
        if ret.is_none()
            && let ReturnType::Type(_, ty) = &f.sig.output
        {
            let head = blocking_head(ty);
            if !head.is_empty() && !eff_blocking.contains(&head) {
                eff_blocking.push(head);
            }
        }
        if let Some(tag) = receiver.as_deref()
            && tag != "entity"
        {
            let ref_ty = match tag {
                "level" => "LevelRef",
                "living" => "LivingRef",
                _ => tag,
            };
            let ref_ty = ref_ty.to_owned();
            if !eff_blocking.contains(&ref_ty) {
                eff_blocking.push(ref_ty);
            }
        }
        eff_blocking.sort();
        eff_blocking.dedup();
        if !eff_blocking.is_empty() {
            rows.push(row(
                name,
                target,
                HelperClass::Adapter(eff_blocking),
                Vec::new(),
                String::new(),
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        // Step 3/4: with nothing blocking, the return is supported.
        // `&self` + `()` is the same shape as `&mut self` + `()`: an
        // Action. `&mut self` + a data return cannot ride the queue.
        // A `&mut self` helper whose `bool` answers a query (its doc starts
        // "Whether" and it takes no `bool` argument to set) is not a queued
        // action: the queue drops the answer. It needs a read design; a
        // setter's `bool` is its success flag and stays an action below.
        if receiver_mut
            && ret.as_deref() == Some("bool")
            && doc.starts_with("Whether")
            && !plain_args(f).iter().any(|a| a.rust == "bool")
        {
            rows.push(cannot_row(
                name,
                target,
                "a `&mut self` helper that answers a query: a queued \
                 action would drop the answer (needs a read design)",
                doc,
                api,
                receiver,
                receiver_mut,
                ret_borrowed,
                ret_static,
            ));
            continue;
        }
        let kind = if !has_receiver {
            // A receiverless helper is `&self` in the trait: a read of
            // a plain return, a queued action of unit/bool.
            match ret.as_deref() {
                Some("unit") | Some("bool") => GenKind::Action,
                Some(_) => GenKind::Read,
                None => unreachable!("an unsupported return blocks as Adapter above"),
            }
        } else if receiver_mut {
            match ret.as_deref() {
                Some("unit") | Some("bool") => GenKind::Action,
                Some(_) => {
                    rows.push(row(
                        name,
                        target,
                        HelperClass::Cannot(
                            "a `&mut self` helper returning data is not a queued action: it needs the run_command_as callback design".to_owned(),
                        ),
                        Vec::new(),
                        String::new(),
                        doc,
                        api,
                        receiver,
                        receiver_mut,
                        ret_borrowed,
                        ret_static,
                    ));
                    continue;
                }
                None => unreachable!("an unsupported return blocks as Adapter above"),
            }
        } else {
            match ret.as_deref() {
                Some("unit") => GenKind::Action,
                Some(_) => GenKind::Read,
                None => unreachable!("an unsupported return blocks as Adapter above"),
            }
        };
        rows.push(row(
            name,
            target,
            HelperClass::Generated(kind),
            plain_args(f),
            ret.unwrap(),
            doc,
            api,
            receiver,
            receiver_mut,
            ret_borrowed,
            ret_static,
        ));
    }
    Ok(rows)
}

fn is_result_type(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };
    matches!(&**ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Result"))
}

fn resolve_source(root: &Path, target: &str) -> Result<(PathBuf, bool), String> {
    if let Some((krate, path)) = free_function_source(target) {
        let (module, _) = path
            .rsplit_once("::")
            .ok_or_else(|| format!("wrong target {target}"))?;
        Ok((
            root.join("crates")
                .join(krate)
                .join("src")
                .join(format!("{}.rs", module.replace("::", "/"))),
            true,
        ))
    } else {
        let rest = target
            .strip_prefix("horizon_client::game::")
            .ok_or_else(|| format!("wrong target {target}"))?;
        let module = rest
            .split("::")
            .take_while(|seg| !seg.contains('<') && !seg.starts_with("Game"))
            .collect::<Vec<_>>()
            .join("/");
        Ok((
            root.join("crates/horizon-client/src/game")
                .join(format!("{module}.rs")),
            false,
        ))
    }
}

fn return_is_borrowed(f: &syn::ItemFn) -> bool {
    let ReturnType::Type(_, ty) = &f.sig.output else {
        return false;
    };
    if str_borrow(ty) {
        return true;
    }
    if let Type::Path(p) = &**ty
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return str_borrow(inner);
    }
    false
}

/// Whether a borrowed return's lifetime is explicitly `'static`.
fn return_is_static_borrowed(f: &syn::ItemFn) -> bool {
    let ReturnType::Type(_, ty) = &f.sig.output else {
        return false;
    };
    if is_static_str_borrow(ty) {
        return true;
    }
    if let Type::Path(p) = &**ty
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return is_static_str_borrow(inner);
    }
    false
}

/// Whether the type is a string borrow with an explicit `'static`.
fn is_static_str_borrow(ty: &Type) -> bool {
    let Type::Reference(r) = ty else {
        return false;
    };
    if !r.lifetime.as_ref().is_some_and(|l| l.ident == "static") {
        return false;
    }
    let Type::Path(p) = &*r.elem else {
        return false;
    };
    p.path
        .segments
        .last()
        .is_some_and(|s| s.ident == "str" || s.ident == "String")
}

#[allow(clippy::too_many_arguments)]
fn row(
    name: String,
    target: String,
    class: HelperClass,
    args: Vec<Arg>,
    ret: String,
    doc: String,
    api: Option<u32>,
    receiver: Option<String>,
    receiver_mut: bool,
    ret_borrowed: bool,
    ret_static: bool,
) -> HelperRow {
    HelperRow {
        name,
        target,
        class,
        args,
        ret,
        doc,
        api,
        receiver,
        receiver_mut,
        ret_borrowed,
        ret_static,
    }
}

fn cannot_row(
    name: String,
    target: String,
    why: &str,
    doc: String,
    api: Option<u32>,
    receiver: Option<String>,
    receiver_mut: bool,
    ret_borrowed: bool,
    ret_static: bool,
) -> HelperRow {
    row(
        name,
        target,
        HelperClass::Cannot(why.to_owned()),
        Vec::new(),
        String::new(),
        doc,
        api,
        receiver,
        receiver_mut,
        ret_borrowed,
        ret_static,
    )
}

/// The Lua declaration of one generated helper function: its parameters
/// renamed (`who`, then numbered), matching the emitted Rust reads. A
/// player argument takes a player table, uuid text, or a player name
/// (`string | ReadPlayer`, as the hand-written `who` parameters spell
/// it); an entity argument is its network id (a number); a position is
/// a `Vec3`; reads return their value type (a `number?` for an optional
/// number, a `Vec3?` for an optional position, and so on); actions
/// return `()`.
pub fn luau_function(row: &HelperRow) -> (String, String) {
    let mut strings = 0;
    let mut other = 0;
    let mut params: Vec<String> = Vec::new();
    for a in &row.args {
        match a.rust.as_str() {
            "u128" => params.push("who: string | ReadPlayer".to_owned()),
            "&str" | "String" => {
                strings += 1;
                let name = if strings == 1 {
                    "name".to_owned()
                } else {
                    format!("name{strings}")
                };
                params.push(format!("{name}: string"));
            }
            "[f64; 3]" | "[i32; 3]" | "Option<[f64; 3]>" | "Option<[i32; 3]>" => {
                other += 1;
                let name = if other == 1 {
                    "value".to_owned()
                } else {
                    format!("value{other}")
                };
                params.push(format!("{name}: Vec3"));
            }
            // Numbers, booleans and entity ids share one counter, so no
            // two parameters ever carry the same name.
            _ => {
                other += 1;
                let ty = if a.rust == "bool" {
                    "boolean"
                } else {
                    "number"
                };
                let name = if other == 1 {
                    "value".to_owned()
                } else {
                    format!("value{other}")
                };
                params.push(format!("{name}: {ty}"));
            }
        }
    }
    let ret = match row.ret.as_str() {
        "unit" => "()",
        // A read's `bool` is its answer; an action's `bool` is its queued
        // success flag, which the queue drops.
        "bool" => {
            if matches!(&row.class, HelperClass::Generated(GenKind::Read)) {
                "boolean"
            } else {
                "()"
            }
        }
        "Option<bool>" => "boolean?",
        "Option<String>" => "string?",
        "String" => "string",
        "[f64; 3]" | "[i32; 3]" => "Vec3",
        "Option<[f64; 3]>" | "Option<[i32; 3]>" => "Vec3?",
        _ if row.ret.starts_with("Option<") => "number?",
        _ => "number",
    };
    let ret = if matches!(&row.class, HelperClass::Generated(GenKind::Action)) {
        "()"
    } else {
        ret
    };
    (
        row.name.clone(),
        format!("({}) -> {ret}", params.join(", ")),
    )
}

/// The counts of every helper, by class, for the exposure doc header.
pub fn class_counts(rows: &[HelperRow]) -> (usize, usize, usize, usize, usize) {
    let mut generated = 0;
    let mut already = 0;
    let mut adapter = 0;
    let mut handle = 0;
    let mut cannot = 0;
    for row in rows {
        match &row.class {
            HelperClass::Generated(_) => generated += 1,
            HelperClass::AlreadyLua => already += 1,
            HelperClass::Adapter(_) => adapter += 1,
            HelperClass::Handle(_) => handle += 1,
            HelperClass::Cannot(_) => cannot += 1,
        }
    }
    (generated, already, adapter, handle, cannot)
}

/// The adapter blocking types, ranked by how many helpers they block.
pub fn adapter_ranking(rows: &[HelperRow]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        if let HelperClass::Adapter(types) = &row.class {
            for ty in types {
                *counts.entry(ty.clone()).or_default() += 1;
            }
        }
    }
    let mut ranked: Vec<_> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked
}

/// How many helpers would become Generated if the `n` top blocking types
/// were unblocked (each counted once per helper; a helper blocked by two
/// of the top `n` is counted once). Only the adapter class counts: a
/// helper also blocked by a handle, an unanswerable return, or an
/// already hand-written name stays out.
pub fn unblocked_by_top(rows: &[HelperRow], n: usize) -> usize {
    let ranking = adapter_ranking(rows);
    let top: BTreeSet<String> = ranking.into_iter().take(n).map(|(ty, _)| ty).collect();
    rows.iter()
        .filter(|row| match &row.class {
            HelperClass::Adapter(types) => types.iter().all(|t| top.contains(t)),
            _ => false,
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    /// Every helper in the catalog lands in exactly one class, and the
    /// 403 count holds (182 catalog helpers, 221 in the pending
    /// fragments).
    #[test]
    fn every_helper_lands_in_one_class() {
        let rows = classify_helpers(&root()).unwrap();
        assert_eq!(rows.len(), 403, "every helper is classified");
        let (generated, already, adapter, handle, cannot) = class_counts(&rows);
        assert_eq!(
            generated + already + adapter + handle + cannot,
            rows.len(),
            "the class counts add up to every helper"
        );
    }

    /// The smoke list's five reads and nine enabled actions (the helpers
    /// the spike enabled first, less `mob_navigating`, which answers a
    /// query) classify as Generated with the kind the receiver and return
    /// promise.
    #[test]
    fn the_spike_set_classifies_as_promised() {
        let rows = classify_helpers(&root()).unwrap();
        let reads = [
            "profile_by_uuid",
            "player_view_distance",
            "player_sneaking",
            "player_stat",
            "max_stack_size",
        ];
        let actions = [
            "remove_effect",
            "set_attribute_base",
            "set_flight",
            "respawn_player",
            "resend_commands",
            "hide_entity_from",
            "sleep_ignored",
            "mob_stop",
            "refresh_player",
        ];
        for name in reads {
            let row = rows.iter().find(|r| r.name == name).unwrap();
            match &row.class {
                HelperClass::Generated(GenKind::Read) => {}
                class => panic!("{name} should be a generated read, got {class:?}"),
            }
        }
        for name in actions {
            let row = rows.iter().find(|r| r.name == name).unwrap();
            match &row.class {
                HelperClass::Generated(GenKind::Action) => {}
                class => panic!("{name} should be a generated action, got {class:?}"),
            }
        }
        // No smoke name is already a Lua helper.
        for name in reads.iter().copied().chain(actions) {
            let row = rows.iter().find(|r| r.name == *name).unwrap();
            assert!(
                !matches!(&row.class, HelperClass::AlreadyLua),
                "{name} must not already be a Lua helper"
            );
        }
    }

    /// A `&mut self` helper whose `bool` answers a query (its doc starts
    /// "Whether" and it takes no `bool` argument) is not a queued action:
    /// the queue would drop the answer. `entity_is_mob` (pinned here) and
    /// `mob_navigating` are those; `sleep_ignored` takes the bool it sets,
    /// so its `bool` stays a success flag and it stays an action.
    #[test]
    fn answer_queries_are_cannot_not_queued_actions() {
        let rows = classify_helpers(&root()).unwrap();
        for name in ["entity_is_mob", "mob_navigating"] {
            let row = rows.iter().find(|r| r.name == name).unwrap();
            match &row.class {
                HelperClass::Cannot(why) => assert_eq!(
                    why,
                    "a `&mut self` helper that answers a query: a queued \
                     action would drop the answer (needs a read design)"
                ),
                class => panic!("{name} should be cannot, got {class:?}"),
            }
        }
        let row = rows.iter().find(|r| r.name == "sleep_ignored").unwrap();
        assert!(
            matches!(&row.class, HelperClass::Generated(GenKind::Action)),
            "sleep_ignored should stay an action, got {:?}",
            row.class
        );
    }

    /// No `Cannot` row carries the generic fallback (or nothing at all):
    /// every reason names its type or construct, so the check fails when
    /// the classifier throws a non-plain return into the catch-all.
    #[test]
    fn cannot_reasons_are_specific() {
        let rows = classify_helpers(&root()).unwrap();
        for row in &rows {
            if let HelperClass::Cannot(why) = &row.class {
                assert!(!why.is_empty(), "{} has an empty reason", row.name);
                assert!(
                    !why.contains("a return the bridge cannot answer"),
                    "{} uses the generic fallback: {why}",
                    row.name
                );
            }
        }
    }

    /// A `Generated` helper carries no `receiver` tag but `entity` (its
    /// `EntityRef` is its first argument, an id number the bridge
    /// converts); a `level`/`living` tag still blocks. The string-borrow
    /// reads and `&self` unit actions classify as Generated after the
    /// order fix, and the entity/position helpers classify once their
    /// types convert.
    #[test]
    fn generated_helpers_carry_no_receiver() {
        let rows = classify_helpers(&root()).unwrap();
        for row in &rows {
            if matches!(row.class, HelperClass::Generated(_)) {
                assert!(
                    matches!(row.receiver.as_deref(), None | Some("entity")),
                    "{} is Generated but carries receiver {:?}",
                    row.name,
                    row.receiver
                );
            }
        }
        for name in ["biome_name", "mod_screen"] {
            let row = rows.iter().find(|r| r.name == name).unwrap();
            assert!(
                matches!(row.class, HelperClass::Generated(GenKind::Read)),
                "{name} should be a generated read, got {:?}",
                row.class
            );
        }
        for name in [
            "set_setting",
            "count_packets",
            "spans",
            "mob_cost_sampling",
            "watch_server_packets",
            "watch_entity_data",
        ] {
            let row = rows.iter().find(|r| r.name == name).unwrap();
            assert!(
                matches!(row.class, HelperClass::Generated(GenKind::Action)),
                "{name} should be a generated action, got {:?}",
                row.class
            );
        }
        // Entity and position helpers are Generated once their types
        // convert (`entity_position` is a read of `[f64; 3]`,
        // `block_entity_sync` an action on `[i32; 3]`).
        for (name, kind) in [
            ("entity_position", GenKind::Read),
            ("block_entity_sync", GenKind::Action),
            ("entity_set_velocity", GenKind::Action),
        ] {
            let row = rows.iter().find(|r| r.name == name).unwrap();
            assert!(
                matches!(&row.class, HelperClass::Generated(k) if *k == kind),
                "{name} should be Generated({kind:?}), got {:?}",
                row.class
            );
        }
        for name in ["entity_nbt", "level_height", "living_equipment"] {
            let row = rows.iter().find(|r| r.name == name).unwrap();
            assert!(
                matches!(row.class, HelperClass::Adapter(_)),
                "{name} should be Adapter, got {:?}",
                row.class
            );
        }
    }

    /// A helper named for an existing `horizon.*` function (`on` is
    /// the hook registration) is `Cannot` with the collision reason:
    /// emitting it would overwrite that function. A hand-written
    /// helper (an `AlreadyLua` name) keeps its class instead.
    #[test]
    fn a_reserved_name_is_cannot() {
        let rows = classify_helpers(&root()).unwrap();
        let row = rows.iter().find(|r| r.name == "on").unwrap();
        match &row.class {
            HelperClass::Cannot(why) => assert!(
                why.contains("collides"),
                "on should name the collision, got {why}"
            ),
            class => panic!("on should be Cannot, got {class:?}"),
        }
    }
}
