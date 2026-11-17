//! The handle methods the Lua host binds (issue 74, H1b): for each handle family, its members that
//! only read the world, with their arguments and results converted.
//!
//! [`emit`] takes the families as a parameter ([`FAMILIES`] is the default). A member is bound when
//! its receiver is `self` or `&self`, its first parameter is `&dyn Host` or there is none, and every
//! other parameter and the return type convert. A `BlockState` argument or result needs the host
//! (it converts through it). Anything else is skipped with a reason, and the coverage text lists
//! each member. Writes take `&mut` hosts, so none is bound here.
//!
//! The text is one `impl` of the hand-written `Family` trait per family, one function per bound
//! member, and the table `cost`/`run` that the host's scoped function calls.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::emit_impl::scalar_name;
use crate::emit_types::{Plain, snake_case};
use crate::handles::{MemberIr, MemberKind, ParamIr, Receiver};
use crate::types::{Access, FloatKind, TypeIr, map_type};

/// The families bound when the generator is not told others.
pub const FAMILIES: &[&str] = &["EntityRef", "LevelRef"];

/// The cost of a read, in budget units.
const READ: u32 = 1;
/// The cost of a heavy read: one that returns a list, or one of the names below.
const HEAVY: u32 = 100;
/// Methods heavy by name, beside every method that returns a list.
const HEAVY_NAMES: &[&str] = &[
    "all_entities",
    "clip",
    "collision_boxes",
    "entities_in",
    "loaded_chunks",
    "online_players",
];
/// Method name prefixes that are heavy.
const HEAVY_PREFIXES: &[&str] = &["datapack_", "find_", "ray_trace"];
/// The arrays a Lua value converts into (`vector.rs`), by element type and length.
const FROM_LUA_ARRAYS: &[(&str, usize)] = &[
    ("i32", 2),
    ("i32", 3),
    ("f64", 2),
    ("f64", 3),
    ("f32", 2),
    ("f32", 3),
];
/// The arrays a Rust value converts to Lua (`vector.rs` and `generated_types.rs`).
const TO_LUA_ARRAYS: &[(&str, usize)] = &[
    ("bool", 3),
    ("bool", 6),
    ("f32", 2),
    ("f32", 3),
    ("f32", 4),
    ("f32", 6),
    ("f32", 12),
    ("f32", 16),
    ("f64", 2),
    ("f64", 3),
    ("f64", 4),
    ("f64", 6),
    ("i32", 2),
    ("i32", 3),
    ("i32", 4),
    ("u8", 2),
    ("u8", 3),
    ("u16", 2),
    ("u32", 2),
    ("u64", 2),
    ("usize", 2),
];
/// Named types that convert both ways by hand in `event-horizon-lua-convert`.
const HAND_NAMED: &[&str] = &[
    "BlockPos",
    "Compound",
    "Duration",
    "Identifier",
    "Rotation",
    "Text",
    "text::Text",
];

/// How one argument reaches the game's method.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Arg {
    /// A value read with `Args::get`, passed as it is or by reference (`&str`, `&Identifier`).
    Get { ty: String, by_ref: bool },
    /// A handle of the family, read with `Args::handle`.
    Handle { ty: String },
    /// An optional handle of the family, read with `Args::opt_handle`.
    OptHandle { ty: String },
    /// A block state, read with `Args::state` through the host.
    State,
}

/// How the result becomes a Lua value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ret {
    /// A value with a `ToLuaValue` impl.
    Value,
    /// A string (`String`, `&str`).
    Text,
    /// An optional string.
    OptText,
    /// A handle of the family, as its userdata.
    Family,
    OptFamily,
    ListFamily,
    OptListFamily,
    /// An optional block state, as its `{id, props}` table through the host.
    State,
    /// A world epoch, as its number.
    Epoch,
}

/// A member the generator binds.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bound {
    id: String,
    op: String,
    name: String,
    host: bool,
    args: Vec<Arg>,
    ret: Ret,
    cost: u32,
}

/// The generator's output for the families asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The file `handles.rs`.
    pub text: String,
    /// The coverage listing, `handles_coverage.txt`.
    pub coverage: String,
    /// The bound members' ids, in family then name order.
    pub bound: Vec<String>,
    /// The heavy members' ids (a subset of `bound`).
    pub heavy: Vec<String>,
    /// Per family: `(members, bound)`.
    pub per_family: Vec<(String, usize, usize)>,
    /// Each member that is not bound, with its reason, in family then name order.
    pub skipped: Vec<(String, String)>,
}

/// The category of a skip reason: the text before its first colon.
pub fn reason_category(reason: &str) -> &str {
    reason.split(':').next().unwrap_or(reason).trim()
}

/// The names of the types the families' read members take as owned arguments, which the type
/// emitter must decide (a unit enum among them converts from Lua). A family's own handles are left
/// out, and so are the writes' types: a write is not bound in this slice.
pub fn argument_types(members: &[MemberIr], families: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    for member in members
        .iter()
        .filter(|m| is_family_member(m, families) && reads_only(m))
    {
        for name in named_arguments(member, families) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The types of the arguments of the read members that [`classify`] binds under `plain`: the
/// converters the type emitter must decide, and no more. A bound member's enum argument is always
/// among them, so deciding exactly these leaves every bound member bound.
pub fn bound_argument_types(members: &[MemberIr], families: &[&str], plain: &Plain) -> Vec<String> {
    let mut names = Vec::new();
    for member in members
        .iter()
        .filter(|m| is_family_member(m, families) && reads_only(m))
    {
        let family = member.owner.as_str();
        if classify(member, family, families, plain).is_err() {
            continue;
        }
        for name in named_arguments(member, families) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The named types among a member's owned arguments (`T` or `Option<T>`), other than the families'.
fn named_arguments(member: &MemberIr, families: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    for param in &member.params {
        let Ok(mapped) = map_type(&param.rust_type) else {
            continue;
        };
        let inner = match &mapped.ty {
            TypeIr::Option(inner) => inner.as_ref(),
            other => other,
        };
        if let (Access::Owned, TypeIr::Named(name)) = (mapped.access, inner)
            && !families.contains(&name.as_str())
        {
            names.push(name.clone());
        }
    }
    names
}

/// Whether a member takes no host or the read host (`&dyn Host`) as its first parameter.
fn reads_only(member: &MemberIr) -> bool {
    member.params.first().is_none_or(|first| {
        first
            .rust_type
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            == "&dyn Host"
    })
}

fn is_family_member(member: &MemberIr, families: &[&str]) -> bool {
    !member.from_trait && families.contains(&member.owner.as_str())
}

/// Emits the handle file and its coverage for the members of `families`, in the order given.
/// `plain` is the set of plain surface types the type emitter decided; `hash` is the surface's.
pub fn emit(members: &[MemberIr], families: &[&str], plain: &Plain, hash: &str) -> Emitted {
    let mut text = format!(
        "// @generated by event-horizon-lua-gen from surface {hash}: do not edit.\n\
         // Families: {}. Coverage: handles_coverage.txt.\n",
        families.join(", ")
    );
    let mut coverage = format!(
        "# Handle coverage (issue 74, H1b) from surface {hash}, by event-horizon-lua-gen: do not edit.\n\
         # families: {}\n",
        families.join(", ")
    );
    let mut bound_all: Vec<Bound> = Vec::new();
    let mut skipped = Vec::new();
    let mut per_family = Vec::new();
    let mut impls = String::new();
    let mut functions = String::new();

    for family in families {
        let mut family_members: Vec<&MemberIr> = members
            .iter()
            .filter(|m| !m.from_trait && m.owner == *family)
            .collect();
        family_members.sort_by(|a, b| a.name.cmp(&b.name));
        let mut bound_here = Vec::new();
        let _ = writeln!(coverage, "\n{family}: {} members", family_members.len());
        for member in &family_members {
            match classify(member, family, families, plain) {
                Ok(bound) => {
                    let _ = writeln!(coverage, "  {} bound, cost {}", bound.id, bound.cost);
                    bound_here.push(bound);
                }
                Err(reason) => {
                    let _ = writeln!(coverage, "  {} skipped: {reason}", member.id);
                    skipped.push((member.id.clone(), reason));
                }
            }
        }
        per_family.push((family.to_string(), family_members.len(), bound_here.len()));
        impls.push_str(&family_impl(family, &bound_here));
        for bound in &bound_here {
            functions.push_str(&operation(family, bound));
        }
        bound_all.extend(bound_here);
    }

    text.push_str(&format!(
        "\n// The userdata methods of each family: one call of the scoped function per method.\n{impls}"
    ));
    text.push_str(&functions);
    text.push_str(&table(&bound_all));

    let bound: Vec<String> = bound_all.iter().map(|b| b.id.clone()).collect();
    let heavy: Vec<String> = bound_all
        .iter()
        .filter(|b| b.cost == HEAVY)
        .map(|b| b.id.clone())
        .collect();
    let _ = writeln!(
        coverage,
        "\nsummary: bound {} of {}; heavy {}",
        bound.len(),
        per_family.iter().map(|(_, n, _)| n).sum::<usize>(),
        heavy.len()
    );
    let mut by_category: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, reason) in &skipped {
        *by_category.entry(reason_category(reason)).or_default() += 1;
    }
    for (category, count) in &by_category {
        let _ = writeln!(coverage, "  skipped, {category}: {count}");
    }
    let _ = writeln!(coverage, "heavy (cost {HEAVY}): {}", heavy.join(", "));
    Emitted {
        text,
        coverage,
        bound,
        heavy,
        per_family,
        skipped,
    }
}

/// Classifies one member: bound with its conversions, or the reason it is skipped.
fn classify(
    member: &MemberIr,
    family: &str,
    families: &[&str],
    plain: &Plain,
) -> Result<Bound, String> {
    if member.kind != MemberKind::Function {
        return Err("other: a const item".to_owned());
    }
    if !member.generics.is_empty() {
        return Err(format!("generics: {}", member.generics));
    }
    if matches!(member.receiver, Receiver::None | Receiver::RefMut) {
        return Err("other: no receiver, or &mut self".to_owned());
    }
    let host = match member.params.first() {
        Some(first) => is_host_param(first)?,
        None => false,
    };
    let params = if host {
        &member.params[1..]
    } else {
        member.params.as_slice()
    };
    let args = params
        .iter()
        .map(|param| arg_of(&param.rust_type, families, plain))
        .collect::<Result<Vec<Arg>, String>>()?;
    if !host && args.contains(&Arg::State) {
        return Err("unconvertible arg: BlockState needs a host".to_owned());
    }
    let ret_text = member
        .ret
        .as_deref()
        .ok_or_else(|| "unconvertible return: no return type".to_owned())?;
    if ret_text.starts_with("Result<") {
        return Err(format!("Result: {ret_text}"));
    }
    let (ret, ret_ty) = match ret_text {
        "WorldEpoch" => (Ret::Epoch, None),
        _ => {
            let mapped =
                map_type(ret_text).map_err(|_| format!("unconvertible return: {ret_text}"))?;
            let ret = ret_of(&mapped.ty, families, plain)
                .ok_or_else(|| format!("unconvertible return: {ret_text}"))?;
            (ret, Some(mapped.ty))
        }
    };
    if ret == Ret::State && !host {
        return Err("unconvertible return: BlockState needs a host".to_owned());
    }
    let heavy = HEAVY_NAMES.contains(&member.name.as_str())
        || HEAVY_PREFIXES
            .iter()
            .any(|prefix| member.name.starts_with(prefix))
        || ret_ty.as_ref().is_some_and(returns_list)
        || matches!(ret, Ret::ListFamily | Ret::OptListFamily);
    Ok(Bound {
        id: member.id.clone(),
        op: format!("{family}::{}", member.name),
        name: member.name.clone(),
        host,
        args,
        ret,
        cost: if heavy { HEAVY } else { READ },
    })
}

/// Whether the first parameter is the read host (`&dyn Host`). Any other host is an error that
/// names it, so a write's host is never mistaken for a read.
fn is_host_param(param: &ParamIr) -> Result<bool, String> {
    let text = param.rust_type.as_str();
    let Ok(mapped) = map_type(text) else {
        return Ok(false);
    };
    match (mapped.access, &mapped.ty) {
        (Access::Ref, TypeIr::Dyn(name)) if name == "Host" => Ok(true),
        (Access::RefMut, TypeIr::Dyn(_)) => Err(format!("&mut host: {text}")),
        (_, TypeIr::Dyn(_)) => Err(format!("other: host kind {text}")),
        _ => Ok(false),
    }
}

fn returns_list(ty: &TypeIr) -> bool {
    match ty {
        TypeIr::List(_) => true,
        TypeIr::Option(inner) => returns_list(inner),
        _ => false,
    }
}

/// How an argument of type `text` converts, or why it does not.
fn arg_of(text: &str, families: &[&str], plain: &Plain) -> Result<Arg, String> {
    let unconvertible = || format!("unconvertible arg: {text}");
    let mapped = map_type(text).map_err(|_| {
        if text.contains("impl") {
            format!("other: closure or impl Trait: {text}")
        } else {
            format!("other: unparsed type: {text}")
        }
    })?;
    match mapped.access {
        Access::RefMut => return Err(format!("other: &mut argument: {text}")),
        Access::Ref => {
            return match &mapped.ty {
                TypeIr::Str => Ok(Arg::Get {
                    ty: "String".to_owned(),
                    by_ref: true,
                }),
                TypeIr::Named(name) if name == "Identifier" => Ok(Arg::Get {
                    ty: path_of(name),
                    by_ref: true,
                }),
                _ => Err(unconvertible()),
            };
        }
        Access::Owned => {}
    }
    match &mapped.ty {
        TypeIr::Named(name) if families.contains(&name.as_str()) => {
            Ok(Arg::Handle { ty: path_of(name) })
        }
        TypeIr::Option(inner) if is_family(inner, families) => match &**inner {
            TypeIr::Named(name) => Ok(Arg::OptHandle { ty: path_of(name) }),
            _ => Err(unconvertible()),
        },
        TypeIr::Named(name) if name == "BlockState" => Ok(Arg::State),
        ty => spelling(ty, plain)
            .map(|ty| Arg::Get { ty, by_ref: false })
            .ok_or_else(unconvertible),
    }
}

fn is_named(ty: &TypeIr, name: &str) -> bool {
    matches!(ty, TypeIr::Named(found) if found == name)
}

/// Whether a type is one of the generated handle families: a handle of any of them converts.
fn is_family(ty: &TypeIr, families: &[&str]) -> bool {
    matches!(ty, TypeIr::Named(found) if families.contains(&found.as_str()))
}

/// The Rust path of a named type in the generated code.
fn path_of(name: &str) -> String {
    match name {
        "text::Text" | "Text" => "mod_api::Text".to_owned(),
        "Duration" => "std::time::Duration".to_owned(),
        other => format!("mod_api::{other}"),
    }
}

/// The Rust spelling of a type that `FromLuaValue` converts, or `None`.
fn spelling(ty: &TypeIr, plain: &Plain) -> Option<String> {
    Some(match ty {
        TypeIr::Bool => "bool".to_owned(),
        TypeIr::Float(FloatKind::F32) => "f32".to_owned(),
        TypeIr::Float(FloatKind::F64) => "f64".to_owned(),
        TypeIr::Int(_) => scalar_name(ty)?.to_owned(),
        TypeIr::Str => "String".to_owned(),
        TypeIr::Option(inner) => format!("Option<{}>", spelling(inner, plain)?),
        TypeIr::List(elem) => format!("Vec<{}>", spelling(elem, plain)?),
        TypeIr::Array(elem, len) => {
            let name = scalar_name(elem)?;
            if !FROM_LUA_ARRAYS.contains(&(name, *len)) {
                return None;
            }
            format!("[{name}; {len}]")
        }
        TypeIr::Named(name) if HAND_NAMED.contains(&name.as_str()) => path_of(name),
        TypeIr::Named(name) if plain.decides(name) => format!("mod_api::{name}"),
        _ => return None,
    })
}

/// How a result becomes a Lua value, or `None` when it has no conversion.
fn ret_of(ty: &TypeIr, families: &[&str], plain: &Plain) -> Option<Ret> {
    Some(match ty {
        TypeIr::Str => Ret::Text,
        TypeIr::Option(inner) if matches!(**inner, TypeIr::Str) => Ret::OptText,
        ty if is_family(ty, families) => Ret::Family,
        TypeIr::Option(inner) if is_family(inner, families) => Ret::OptFamily,
        TypeIr::List(inner) if is_family(inner, families) => Ret::ListFamily,
        TypeIr::Option(inner) if matches!(&**inner, TypeIr::List(elem) if is_family(elem, families)) => {
            Ret::OptListFamily
        }
        TypeIr::Option(inner) if is_named(inner, "BlockState") => Ret::State,
        ty if value_ret(ty, plain) => Ret::Value,
        _ => return None,
    })
}

/// Whether a value of this type has a `ToLuaValue` impl in the convert crate.
fn value_ret(ty: &TypeIr, plain: &Plain) -> bool {
    match ty {
        TypeIr::Bool | TypeIr::Float(_) | TypeIr::Str => true,
        TypeIr::Int(_) => scalar_name(ty).is_some(),
        TypeIr::Option(inner) | TypeIr::List(inner) => value_ret(inner, plain),
        TypeIr::Array(elem, len) => {
            scalar_name(elem).is_some_and(|name| TO_LUA_ARRAYS.contains(&(name, *len)))
        }
        TypeIr::Named(name) => HAND_NAMED.contains(&name.as_str()) || plain.contains(name),
        _ => false,
    }
}

/// The `impl Family` of one family: its name, its `tostring` and its methods.
fn family_impl(family: &str, bound: &[Bound]) -> String {
    let has_id = bound.iter().any(|b| b.name == "id");
    let describe = if has_id {
        format!("format!(\"{family}({{}})\", self.id())")
    } else {
        format!("String::from(\"{family}\")")
    };
    let mut methods = String::new();
    for method in bound {
        let _ = writeln!(
            methods,
            "        methods.add_function(\"{name}\", |lua: &mlua::Lua, (this, args): (mlua::AnyUserData, mlua::MultiValue)| {{\n            \
             crate::hostcall::host_call(lua, \"{op}\", this, args)\n        }});",
            name = method.name,
            op = method.op
        );
    }
    let param = if bound.is_empty() {
        "_methods"
    } else {
        "methods"
    };
    format!(
        "impl crate::handles::Family for mod_api::{family} {{\n    \
         const NAME: &'static str = \"{family}\";\n    \
         fn describe(self) -> String {{\n        {describe}\n    }}\n    \
         fn add_methods<M: mlua::UserDataMethods<crate::handles::Token<Self>>>({param}: &mut M) {{\n{methods}    }}\n}}\n\n"
    )
}

/// The function of one bound member: it converts the receiver and the arguments (before the host
/// is touched), calls the game's method, and converts the result.
fn operation(family: &str, bound: &Bound) -> String {
    let fn_name = format!("{}_{}", snake_case(family), bound.name);
    let op = &bound.op;
    let mut body = String::new();
    if bound.args.is_empty() {
        let _ = writeln!(body, "    crate::handles::Args::new(\"{op}\", args, 0)?;");
    } else {
        let _ = writeln!(
            body,
            "    let args = crate::handles::Args::new(\"{op}\", args, {})?;",
            bound.args.len()
        );
    }
    let _ = writeln!(
        body,
        "    let this: mod_api::{family} = crate::handles::receiver(this, \"{op}\")?;"
    );
    if bound.host {
        body.push_str("    let read = crate::handles::read_of(host)?;\n");
    }
    let mut call_args = Vec::new();
    if bound.host {
        call_args.push("read".to_owned());
    }
    for (index, arg) in bound.args.iter().enumerate() {
        let name = format!("a{index}");
        let (line, pass) = match arg {
            Arg::Get { by_ref, .. } => (
                format!("args.get({index})?"),
                if *by_ref {
                    format!("&{name}")
                } else {
                    name.clone()
                },
            ),
            Arg::Handle { .. } => (format!("args.handle({index})?"), name.clone()),
            Arg::OptHandle { .. } => (format!("args.opt_handle({index})?"), name.clone()),
            Arg::State => (format!("args.state({index}, read)?"), name.clone()),
        };
        let ty = match arg {
            Arg::Get { ty, .. } | Arg::Handle { ty } => ty.clone(),
            Arg::OptHandle { ty } => format!("Option<{ty}>"),
            Arg::State => "mod_api::BlockState".to_owned(),
        };
        let _ = writeln!(body, "    let {name}: {ty} = {line};");
        call_args.push(pass);
    }
    let _ = writeln!(
        body,
        "    let out = this.{}({});",
        bound.name,
        call_args.join(", ")
    );
    let result = match bound.ret {
        Ret::Value => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)".to_owned(),
        Ret::Text => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out.to_string(), lua)".to_owned(),
        Ret::OptText => {
            "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out.map(|value| value.to_string()), lua)".to_owned()
        }
        Ret::Family => "crate::handles::family_out(lua, out)".to_owned(),
        Ret::OptFamily => "crate::handles::family_opt_out(lua, out)".to_owned(),
        Ret::ListFamily => "crate::handles::family_list_out(lua, out)".to_owned(),
        Ret::OptListFamily => "crate::handles::family_opt_list_out(lua, out)".to_owned(),
        Ret::State => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&crate::stacks::state_in(read, out.as_ref()).flatten(), lua)"
            .to_owned(),
        Ret::Epoch => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out.get(), lua)".to_owned(),
    };
    let host_param = if bound.host { "host" } else { "_host" };
    format!(
        "fn {fn_name}(lua: &mlua::Lua, {host_param}: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {{\n{body}    {result}\n}}\n\n"
    )
}

/// The op table: each operation's cost, and the dispatch to its function.
fn table(bound: &[Bound]) -> String {
    let mut costs = String::new();
    let mut runs = String::new();
    for op in bound {
        let _ = writeln!(costs, "        \"{}\" => {},", op.op, op.cost);
        let fn_name = format!(
            "{}_{}",
            snake_case(op.op.split("::").next().unwrap_or_default()),
            op.name
        );
        let _ = writeln!(
            runs,
            "        \"{}\" => {fn_name}(lua, host, this, args),",
            op.op
        );
    }
    let mut out = String::from(
        "/// What an operation costs in budget units, by its `Family::method` name, or `None` for a name\n\
         /// that is not bound.\n\
         pub fn cost(op: &str) -> Option<u32> {\n    Some(match op {\n",
    );
    out.push_str(&costs);
    out.push_str("        _ => return None,\n    })\n}\n\n");
    out.push_str(
        "/// Runs one bound operation with the turn's host and the handle's userdata, or `None` for a name\n\
         /// that is not bound.\n\
         pub fn run(\n    lua: &mlua::Lua,\n    host: &mut crate::hostcall::TurnHost<'_>,\n    op: &str,\n    this: &mlua::AnyUserData,\n    args: mlua::MultiValue,\n) -> Option<mlua::Result<mlua::Value>> {\n    Some(match op {\n",
    );
    out.push_str(&runs);
    out.push_str("        _ => return None,\n    })\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handles;

    /// A surface with the members the tests name: two families, and handles and methods of each.
    fn members(signatures: &[(&str, &str, &str)]) -> Vec<MemberIr> {
        let handles: Vec<serde_json::Value> = signatures
            .iter()
            .map(|(owner, name, signature)| {
                serde_json::json!({
                    "id": format!("handle:{owner}::{name}"),
                    "hash": "00",
                    "signature": signature,
                    "wasm": true,
                    "refs": []
                })
            })
            .collect();
        let document = serde_json::json!({
            "schema": 2,
            "hash": "ab",
            "handles": handles,
            "traits": []
        });
        handles::load(document.to_string().as_bytes()).expect("the document loads")
    }

    fn plain() -> Plain {
        Plain::deciding(["LeashHolder", "Direction"], ["Direction"])
    }

    /// Classifies the member a signature names (`fn name(...)`) in the `EntityRef` family.
    fn one(signature: &str) -> Result<Bound, String> {
        let name = signature
            .trim_start_matches("fn ")
            .split(['(', '<'])
            .next()
            .unwrap_or_default();
        let member = members(&[("EntityRef", name, signature)]).remove(0);
        classify(&member, "EntityRef", &["EntityRef"], &plain())
    }

    #[test]
    fn a_plain_read_is_bound_with_the_host_and_no_arguments() {
        let bound = one("fn position(self, _: &dyn Host) -> Option<[f64; 3]>").unwrap();
        assert!(bound.host);
        assert!(bound.args.is_empty());
        assert_eq!(bound.ret, Ret::Value);
        assert_eq!(bound.cost, READ);
        assert_eq!(bound.op, "EntityRef::position");
    }

    #[test]
    fn an_option_return_and_a_handle_argument_are_bound() {
        let bound = one("fn f(self, _: &dyn Host, _: EntityRef) -> Option<bool>").unwrap();
        assert_eq!(
            bound.args,
            [Arg::Handle {
                ty: "mod_api::EntityRef".to_owned()
            }]
        );
        assert_eq!(bound.ret, Ret::Value);
    }

    #[test]
    fn a_list_of_the_family_is_bound_and_heavy() {
        let bound = one("fn passengers(self, _: &dyn Host) -> Option<Vec<EntityRef>>").unwrap();
        assert_eq!(bound.ret, Ret::OptListFamily);
        assert_eq!(bound.cost, HEAVY);
        let found = one("fn find_thing(self, _: &dyn Host) -> Option<bool>").unwrap();
        assert_eq!(found.cost, HEAVY);
    }

    #[test]
    fn an_optional_handle_and_a_block_state_result_convert_through_the_host() {
        let level = one("fn level(self, _: &dyn Host) -> Option<LevelRef>");
        assert!(level.is_err(), "LevelRef is not in the family");
        let state =
            one("fn block_state(self, _: &dyn Host, _: [i32; 3]) -> Option<BlockState>").unwrap();
        assert_eq!(state.ret, Ret::State);
        assert_eq!(
            state.args,
            [Arg::Get {
                ty: "[i32; 3]".to_owned(),
                by_ref: false
            }]
        );
        let direction = one("fn signal(self, _: &dyn Host, _: Direction) -> Option<i32>").unwrap();
        assert_eq!(
            direction.args,
            [Arg::Get {
                ty: "mod_api::Direction".to_owned(),
                by_ref: false
            }]
        );
    }

    #[test]
    fn a_str_argument_is_read_as_a_string_and_passed_by_reference() {
        let bound = one("fn tag(self, _: &dyn Host, _: &str) -> Option<bool>").unwrap();
        assert_eq!(
            bound.args,
            [Arg::Get {
                ty: "String".to_owned(),
                by_ref: true
            }]
        );
    }

    #[test]
    fn the_skip_reasons_are_the_named_categories() {
        let cases = [
            ("fn set(self, _: &mut dyn ServerHost) -> bool", "&mut host"),
            ("fn f(self, _: &dyn Host, _: &mut Vec<u8>) -> bool", "other"),
            (
                "fn f<T: Copy>(self, _: &dyn Host, _: T) -> Option<bool>",
                "generics",
            ),
            ("fn f(self, _: &dyn Host) -> Result<bool, Error>", "Result"),
            (
                "fn f(self, _: &dyn Host) -> Option<LivingRef>",
                "unconvertible return",
            ),
            (
                "fn f(self, _: &dyn Host, _: PlayerRef) -> bool",
                "unconvertible arg",
            ),
            (
                "fn f(self, _: &dyn Host, _: impl FnOnce(u8)) -> bool",
                "other",
            ),
        ];
        for (signature, category) in cases {
            let reason = one(signature).expect_err(signature);
            assert_eq!(reason_category(&reason), category, "{signature}: {reason}");
        }
    }

    #[test]
    fn an_id_accessor_shows_in_tostring_and_a_family_without_one_does_not() {
        let with_id = members(&[("EntityRef", "id", "fn id(self) -> u32")]);
        let text = emit(&with_id, &["EntityRef"], &plain(), "ab").text;
        assert!(
            text.contains("format!(\"EntityRef({})\", self.id())"),
            "{text}"
        );
        let without = emit(
            &members(&[("LevelRef", "epoch", "fn epoch(self) -> WorldEpoch")]),
            &["LevelRef"],
            &plain(),
            "ab",
        )
        .text;
        assert!(without.contains("String::from(\"LevelRef\")"), "{without}");
    }

    #[test]
    fn one_bound_member_emits_its_function_its_registration_and_its_table_row() {
        let all = members(&[(
            "EntityRef",
            "is_in_water",
            "fn is_in_water(self, _: &dyn Host) -> Option<bool>",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        let golden = "fn entity_ref_is_in_water(lua: &mlua::Lua, host: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {\n    crate::handles::Args::new(\"EntityRef::is_in_water\", args, 0)?;\n    let this: mod_api::EntityRef = crate::handles::receiver(this, \"EntityRef::is_in_water\")?;\n    let read = crate::handles::read_of(host)?;\n    let out = this.is_in_water(read);\n    event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)\n}\n";
        assert!(out.text.contains(golden), "{}", out.text);
        assert!(out.text.contains("methods.add_function(\"is_in_water\""));
        assert!(out.text.contains("\"EntityRef::is_in_water\" => 1,"));
        assert_eq!(out.bound, ["handle:EntityRef::is_in_water"]);
        assert!(out.skipped.is_empty());
    }

    #[test]
    fn a_skipped_member_is_listed_with_its_reason_in_the_coverage() {
        let all = members(&[(
            "EntityRef",
            "set",
            "fn set(self, _: &mut dyn ServerHost) -> bool",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        assert!(
            out.coverage
                .contains("handle:EntityRef::set skipped: &mut host: &mut dyn ServerHost"),
            "{}",
            out.coverage
        );
        assert_eq!(out.per_family, [("EntityRef".to_owned(), 1, 0)]);
    }

    #[test]
    fn the_family_list_is_a_parameter() {
        let all = members(&[
            ("EntityRef", "id", "fn id(self) -> u32"),
            ("LevelRef", "epoch", "fn epoch(self) -> WorldEpoch"),
        ]);
        let only_level = emit(&all, &["LevelRef"], &plain(), "ab");
        assert_eq!(only_level.per_family, [("LevelRef".to_owned(), 1, 1)]);
        assert!(
            !only_level
                .text
                .contains("impl crate::handles::Family for mod_api::EntityRef")
        );
    }

    #[test]
    fn a_vec_of_handles_result_golden() {
        let all = members(&[(
            "EntityRef",
            "passengers",
            "fn passengers(self, _: &dyn Host) -> Option<Vec<EntityRef>>",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        let golden = "fn entity_ref_passengers(lua: &mlua::Lua, host: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {\n    crate::handles::Args::new(\"EntityRef::passengers\", args, 0)?;\n    let this: mod_api::EntityRef = crate::handles::receiver(this, \"EntityRef::passengers\")?;\n    let read = crate::handles::read_of(host)?;\n    let out = this.passengers(read);\n    crate::handles::family_opt_list_out(lua, out)\n}\n";
        assert!(out.text.contains(golden), "{}", out.text);
        assert_eq!(out.heavy, ["handle:EntityRef::passengers"]);
    }

    #[test]
    fn a_handle_argument_golden() {
        let all = members(&[(
            "EntityRef",
            "is_passenger_of_same_vehicle",
            "fn is_passenger_of_same_vehicle(self, _: &dyn Host, _: EntityRef) -> Option<bool>",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        let golden = "fn entity_ref_is_passenger_of_same_vehicle(lua: &mlua::Lua, host: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {\n    let args = crate::handles::Args::new(\"EntityRef::is_passenger_of_same_vehicle\", args, 1)?;\n    let this: mod_api::EntityRef = crate::handles::receiver(this, \"EntityRef::is_passenger_of_same_vehicle\")?;\n    let read = crate::handles::read_of(host)?;\n    let a0: mod_api::EntityRef = args.handle(0)?;\n    let out = this.is_passenger_of_same_vehicle(read, a0);\n    event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)\n}\n";
        assert!(out.text.contains(golden), "{}", out.text);
    }

    #[test]
    fn only_the_bound_members_argument_types_are_decided() {
        let all = members(&[
            (
                "EntityRef",
                "signal",
                "fn signal(self, _: &dyn Host, _: Direction) -> Option<i32>",
            ),
            (
                "EntityRef",
                "hand",
                "fn hand(self, _: &dyn Host, _: PlayerRef, _: Hand) -> Option<bool>",
            ),
        ]);
        let names = bound_argument_types(
            &all,
            &["EntityRef"],
            &Plain::deciding(["Direction", "Hand"], ["Direction", "Hand"]),
        );
        assert_eq!(names, ["Direction"]);
    }
}
