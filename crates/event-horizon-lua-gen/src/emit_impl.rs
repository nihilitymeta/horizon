//! The methods of `impl mod_api::Hooks for LuaHost`, as Rust source text, for the hooks the Lua
//! host can convert.
//!
//! [`emit`] takes the names of the hooks to emit. A hook is emitted when the `Hooks` trait has its
//! method, the surface has it unscoped with no output, it is a listen or a cancel hook, and every
//! argument has a Lua conversion ([`to_lua_expr`]). Any other hook is skipped with its reason.
//!
//! A hook with no outputs builds its event table from the arguments and takes no answer. A hook
//! with outputs (its `&mut` dispatch parameters, see [`crate::dispatch_sigs`]) also builds the
//! outputs' values into the table, as writable keys, and after `dispatch_turns` each mod's answer
//! is converted and kept by [`crate::dispatch::accept`]. A cancel hook's one output is the bool
//! `cancelled`: its method returns the [`Flow`] that the final value gives. A replace hook (its
//! method returns `Option<X>`, no outputs) takes the first mod's return value that is not nil, through
//! `dispatch_replace` and [`crate::dispatch::answer_of`]; it returns `Some` or `None`. Nothing here
//! compiles the text: the tests check that it parses with `syn`, and the first compile against the
//! mod API is a later step.

use std::collections::{BTreeMap, BTreeSet};

use crate::carried::{self, Reach, Shape};
use crate::dispatch_sigs::{DispatchSigs, OutputSig};
use crate::emit_types::Plain;
use crate::ir::{HookIr, Ir, Kind};
use crate::trait_sigs::HookSig;
use crate::types::{Access, FloatKind, IntKind, Mapped, TypeIr, kind_of, map_type};

/// The emitted file and what became of each hook asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The whole file.
    pub text: String,
    /// The hooks whose methods are in `text`, in name order.
    pub emitted: Vec<String>,
    /// The hooks whose methods answer from the mods' declaration records (`HOST_RECORDED`), in
    /// name order. They are in `text` but not in `emitted`: a Lua handler is never called for them.
    pub host_recorded: Vec<String>,
    /// Each hook asked for that is not emitted, with its reason, in name order.
    pub skipped: Vec<(String, String)>,
    /// How many outputs of the emitted hooks have each type, by the type's spelling.
    pub output_types: BTreeMap<String, usize>,
    /// The answer and output types of the emitted hooks (`mod_api::` removed from an answer), which
    /// the type emitter decides: a unit enum among them gets its `FromLuaValue`.
    pub decisions: Vec<String>,
    /// How many emitted hooks are replace hooks (their method returns an answer).
    pub replace: usize,
    /// How many emitted hooks have a unit enum among their answer or outputs.
    pub decided_enum: usize,
    /// How many emitted hooks have a list or a vector among their outputs.
    pub lists: usize,
}

/// The hooks the host answers from the records the mods declared at load, not from Lua handlers:
/// `(hook, event struct, event field, host method)`. The method takes the event's output and
/// extends it with each loaded mod's records. A hook named here is not in `EMITTED`, so a mod
/// cannot subscribe to it with `horizon.on`.
pub const HOST_RECORDED: &[(&str, &str, &str, &str)] = &[
    ("blocks", "BlocksEvent", "blocks", "declared_blocks"),
    ("items", "ItemsEvent", "items", "declared_items"),
];

/// The method of a host-recorded hook: it passes the event's output to the host method.
fn host_recorded_text(hook: &str, event: &str, field: &str, host_method: &str) -> String {
    format!(
        "
    fn {hook}(&self, context: &mod_api::RegistrationContext, event: &mut mod_api::{event}<'_>) {{
        let _ = context;
        self.{host_method}(event.{field});
    }}
"
    )
}

/// The `impl` header and the two methods every file has. `subscribed_slice` and `on_register` are
/// provided by the host, not generated.
const IMPL_HEAD: &str = "
impl mod_api::Hooks for LuaHost {
    fn subscribed(&self) -> &'static [Option<mod_api::HookId>] {
        self.subscribed_slice()
    }

    fn register(&self, r: &mut dyn mod_api::Registrar, _cx: &mod_api::RegistrationContext) {
        self.on_register(r)
    }

    fn listeners(&self) -> &'static [mod_api::Listener] {
        self.listeners_slice()
    }
";

/// One output of a hook: the Lua key the mod writes (the event field it is copied to), and its
/// type without spaces.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Output {
    key: String,
    ty: String,
    /// The event field is a `&mut` reference (`pub lines: &'a mut Vec<String>`), read and written
    /// through it.
    by_ref: bool,
    /// The carried shape (a stack or a block state, see [`carried`]), which converts through the host.
    shape: Option<Shape>,
}

/// One hook that is emitted, with what its method needs.
struct Method<'a> {
    hook: &'a HookIr,
    sig: &'a HookSig,
    /// `(field, Lua value expression)` for each argument that is not an output, in the surface's
    /// order.
    fields: Vec<(String, String)>,
    /// The outputs, in the dispatch function's parameter order. Empty for a listen hook with none.
    outputs: Vec<Output>,
    cancel: bool,
    /// The answer type `X` of a replace hook (`mod_api::X`, as the method's return type spells it).
    answer: Option<String>,
    /// The host as the Lua crate takes it, when a carried value converts through it. Outside the
    /// dispatch closures, which run while the turn holds the host.
    view: Option<String>,
    /// The `TurnHost` variant the method opens its turns with (see [`turn_expr`]); `None` for a hook
    /// that has no host the turns carry.
    turn: Option<String>,
    /// The host as the closures of the dispatch take it: the turn's own host for a hook whose turn
    /// carries one (the closures run while the turn holds the host), else [`Method::view`].
    turn_view: Option<String>,
    /// The pre-checks of the arguments that carry a stack or a block state: a call that is `None`
    /// when the host has no id for the value, so the hook is not delivered.
    checks: Vec<String>,
}

/// Emits the methods of the hooks named in `hooks`, sorted by name. `plain` is the set of plain
/// surface types, whose `ToLuaValue` impls the convert crate's `generated_types` has. `dispatch`
/// holds each hook's outputs and the event structs' fields. The same inputs always give the same
/// text.
pub fn emit(
    ir: &Ir,
    sigs: &[HookSig],
    dispatch: &DispatchSigs,
    hooks: &[&str],
    plain: &Plain,
) -> Emitted {
    emit_with_handles(ir, sigs, dispatch, hooks, plain, &[])
}

/// [`emit`], with the handle families the generated handle file binds (`handles`): a hook argument
/// of one of those types is carried to the event as its userdata (see [`handle_input`]).
pub fn emit_with_handles(
    ir: &Ir,
    sigs: &[HookSig],
    dispatch: &DispatchSigs,
    hooks: &[&str],
    plain: &Plain,
    handles: &[&str],
) -> Emitted {
    let mut names = hooks.to_vec();
    names.sort_unstable();
    names.dedup();

    let mut methods = Vec::new();
    let mut skipped = Vec::new();
    let mut host_recorded = Vec::new();
    let mut recorded_text = String::new();
    for name in names {
        if let Some((hook, event, field, host_method)) =
            HOST_RECORDED.iter().find(|entry| entry.0 == name)
        {
            if sigs.iter().any(|sig| sig.name == name) && ir.hooks.iter().any(|h| h.name == name) {
                host_recorded.push((*hook).to_owned());
                recorded_text.push_str(&host_recorded_text(hook, event, field, host_method));
            } else {
                skipped.push((
                    name.to_owned(),
                    "not a method of the Hooks trait".to_owned(),
                ));
            }
            continue;
        }
        match plan(ir, sigs, dispatch, name, plain, handles) {
            Ok(method) => methods.push(method),
            Err(reason) => skipped.push((name.to_owned(), reason)),
        }
    }

    let mut text = format!(
        "// @generated by event-horizon-lua-gen from surface {}: do not edit.\n",
        ir.document_hash
    );
    text.push_str(&imports(&methods));
    text.push_str(IMPL_HEAD);
    for method in &methods {
        text.push_str(&method_text(method));
    }
    text.push_str(&recorded_text);
    text.push_str("}\n");

    let mut output_types = BTreeMap::new();
    for output in methods.iter().flat_map(|method| &method.outputs) {
        *output_types.entry(output.ty.clone()).or_default() += 1;
    }
    let answer_of = |method: &Method<'_>| {
        method
            .answer
            .as_deref()
            .map(|answer| answer.replace("mod_api::", ""))
    };
    let decisions: Vec<String> = methods
        .iter()
        .flat_map(|method| {
            method
                .outputs
                .iter()
                .map(|output| output.ty.clone())
                .chain(answer_of(method))
        })
        .collect();
    let decided_enum = methods
        .iter()
        .filter(|method| {
            let outputs = method.outputs.iter().map(|output| output.ty.clone());
            outputs
                .chain(answer_of(method))
                .any(|ty| plain.decides(ty.trim_start_matches("Option<").trim_end_matches('>')))
        })
        .count();
    let lists = methods
        .iter()
        .filter(|method| {
            method
                .outputs
                .iter()
                .any(|output| output.ty.starts_with("Vec<") || output.ty.starts_with("["))
        })
        .count();
    let replace = methods
        .iter()
        .filter(|method| method.answer.is_some())
        .count();

    Emitted {
        text,
        emitted: methods
            .iter()
            .map(|method| method.hook.name.clone())
            .collect(),
        host_recorded,
        skipped,
        output_types,
        decisions,
        replace,
        decided_enum,
        lists,
    }
}

/// Whether a dispatch output of type `ty` (without spaces) has a Lua conversion in this slice: the
/// cancel flag, numbers, strings, `Text`, a unit enum the file decides, a vector or a list of
/// scalars (`[f64; 3]`, `[f32; 3]`, `[i32; 3]`, `Vec<String>`, `Vec<[i32; 3]>`), and an `Option` of
/// a value (only a value of the inner type can be assigned: see [`turn_body`]).
pub fn output_supported(ty: &str, plain: &Plain) -> bool {
    if is_list_kind(ty) || value_supported(ty, plain) {
        return true;
    }
    ty.strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'))
        .is_some_and(|inner| value_supported(inner, plain))
}

/// Whether a value of type `ty` (without spaces, not `mod_api::`) converts both ways in this slice:
/// a bool, a number, a string, `Text`, or a unit enum with its `FromLuaValue` in the file.
fn value_supported(ty: &str, plain: &Plain) -> bool {
    const SCALARS: [&str; 7] = ["bool", "i32", "u32", "f32", "f64", "String", "Text"];
    SCALARS.contains(&ty) || plain.decides(ty)
}

/// The answer type of a replace hook: its method returns `Option<X>` and `X` converts from Lua.
/// Returns `mod_api::X` (as the method spells it) for the generated code.
fn replace_answer(sig: &HookSig, plain: &Plain, reach: Reach) -> Result<String, String> {
    let ret = sig
        .ret
        .as_deref()
        .unwrap_or_default()
        .replace("mod_api::", "");
    let Some(inner) = ret
        .strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'))
    else {
        return Err(format!("replace hook does not return an Option: `{ret}`"));
    };
    if mentions_handle(inner) {
        return Err(HANDLE_OUTPUT.to_owned());
    }
    if let Some(shape) = carried::shape_of(inner) {
        // A stack or a block state answer converts through the host; a list or an option of one
        // does not answer a replace hook.
        return match shape {
            Shape::Stack | Shape::State if carried::allowed(shape, reach) => {
                Ok(format!("mod_api::{inner}"))
            }
            Shape::Stack | Shape::State => Err(carried::refusal(shape)),
            _ => Err(format!("answer: {inner}")),
        };
    }
    if !value_supported(inner, plain) {
        return Err(format!("answer: {inner}"));
    }
    Ok(format!("mod_api::{inner}"))
}

/// The type of a collect hook's output argument (`&'a mut Vec<String>`), without spaces: `Vec<String>`.
fn collect_type(rust_type: &str) -> String {
    rust_type
        .split_once("mut")
        .map(|(_, rest)| rest.chars().filter(|ch| !ch.is_whitespace()).collect())
        .unwrap_or_default()
}

/// Whether `ty` (without spaces) is one of the list kinds a hook may write back in this slice.
fn is_list_kind(ty: &str) -> bool {
    [
        "[f64;3]",
        "[f32;3]",
        "[i32;3]",
        "Vec<String>",
        "Vec<[i32;3]>",
    ]
    .contains(&ty)
}

/// Whether a dispatch parameter is a cancel hook's flag: the bool its body copies the event's
/// `cancelled` into. A cancel hook's other bools (`is_drowning`) are ordinary outputs.
fn is_cancel_flag(param: &OutputSig) -> bool {
    param.ty == "bool" && param.copies_cancel
}

/// The outputs of `hook`, from its dispatch function, in parameter order, with the cancel flag
/// keyed `cancelled`. A cancel hook always has the flag as an output: a dispatch that names no
/// flag gets it added last, and its answer reaches the game through the `Flow` the method returns.
/// A reason when an output cannot be converted, when the flag is ambiguous, or when an output does
/// not match the event struct the method writes back to.
fn outputs_of(
    hook: &HookIr,
    sig: &HookSig,
    cancel: bool,
    dispatch: &DispatchSigs,
    plain: &Plain,
    reach: Reach,
) -> Result<Vec<Output>, String> {
    let params = match dispatch.hooks.get(&hook.name) {
        None => return Err("no dispatch function for it".to_owned()),
        Some(None) => return Err("two dispatch functions name it".to_owned()),
        Some(Some(params)) => params,
    };
    // The spelling a dispatch function gives a type may carry the mod API's path (`mod_api::X`).
    let shown = |ty: &str| ty.replace("mod_api::", "");
    for param in params {
        if mentions_handle(&param.ty) {
            return Err(HANDLE_OUTPUT.to_owned());
        }
        match carried::shape_of(&param.ty) {
            Some(shape) if !carried::allowed(shape, reach) => {
                return Err(carried::refusal(shape));
            }
            Some(_) => {}
            None if !output_supported(&shown(&param.ty), plain) => {
                return Err(format!("output: {}", shown(&param.ty)));
            }
            None => {}
        }
    }
    if cancel && params.iter().filter(|param| is_cancel_flag(param)).count() > 1 {
        return Err("cancel hook: two `&mut bool` cancel flags".to_owned());
    }
    // The event's type name is the sig's event type up to its generic arguments.
    let event = sig.event_ty.split('<').next().unwrap_or_default().trim();
    let fields = dispatch.structs.get(event);
    let mut outputs = Vec::new();
    for param in params {
        let key = if cancel && is_cancel_flag(param) {
            "cancelled".to_owned()
        } else {
            param.param.clone()
        };
        let field = fields
            .and_then(|fields| fields.get(&key))
            .map(String::as_str);
        // An output field is a `&mut` reference in the event struct (`&'a mut Vec<String>`), or a
        // plain value; the reference's referent is the output's type.
        let by_ref = field.is_some_and(|text| text.starts_with('&'));
        let field_ty = field.map(|text| {
            if by_ref {
                collect_type(text)
            } else {
                text.to_owned()
            }
        });
        if field_ty.as_deref() != Some(param.ty.as_str()) {
            return Err(format!(
                "output `{key}`: `{event}` has no `{}` field of that name",
                param.ty
            ));
        }
        outputs.push(Output {
            key,
            ty: shown(&param.ty),
            by_ref,
            shape: carried::shape_of(&param.ty),
        });
    }
    if cancel && !outputs.iter().any(|output| output.key == "cancelled") {
        if fields
            .and_then(|fields| fields.get("cancelled"))
            .map(String::as_str)
            != Some("bool")
        {
            return Err(format!("`{event}` has no `cancelled: bool` field"));
        }
        outputs.push(Output {
            key: "cancelled".to_owned(),
            ty: "bool".to_owned(),
            by_ref: false,
            shape: None,
        });
    }
    Ok(outputs)
}

/// Decides whether the hook `name` can be emitted, and what its method contains.
fn plan<'a>(
    ir: &'a Ir,
    sigs: &'a [HookSig],
    dispatch: &DispatchSigs,
    name: &str,
    plain: &Plain,
    handles: &[&str],
) -> Result<Method<'a>, String> {
    let Some(sig) = sigs.iter().find(|sig| sig.name == name) else {
        return Err("not a method of the Hooks trait".to_owned());
    };
    let Some(hook) = ir.hooks.iter().find(|hook| hook.name == name) else {
        return Err("not in the surface".to_owned());
    };
    if hook.scope.is_some() {
        return Err("scoped: fired through a Part trait, not Hooks".to_owned());
    }
    // A collect hook's `&mut` output is one of its dispatch outputs. A list kind is taken here: the
    // mod assigns the list, which replaces the current value. Any other output stays out.
    if let Some(output) = &hook.output {
        if mentions_handle(&output.rust_type) {
            return Err(HANDLE_OUTPUT.to_owned());
        }
        let list = hook.kind == Kind::Collect
            && (is_list_kind(&collect_type(&output.rust_type))
                || carried::shape_of(&output.rust_type).is_some());
        if !list {
            return Err("has an output: its listeners write back, not in this slice".to_owned());
        }
    }
    let replace = matches!(hook.kind, Kind::Replace);
    let reach = carried::reach_of(sig.host_ty.as_deref());
    let view = carried::view(sig.host_name.as_deref(), sig.host_ty.as_deref(), reach);
    let turn = turn_expr(sig.host_name.as_deref(), sig.host_ty.as_deref());
    let turn_view = if turn.is_some() {
        Some("turn.read_or_unavailable()".to_owned())
    } else {
        view.clone()
    };
    let cancel = match hook.kind {
        Kind::Listen | Kind::Replace | Kind::Collect => false,
        Kind::Cancel => true,
    };
    let answer = if replace {
        Some(replace_answer(sig, plain, reach)?)
    } else {
        None
    };
    if cancel {
        if sig.ret.as_deref() != Some("Flow") {
            return Err("cancel hook does not return Flow".to_owned());
        }
        let answers = hook
            .arguments
            .iter()
            .any(|arg| arg.name == "cancelled" && arg.rust_type == "bool");
        if !answers {
            return Err("cancel hook has no `cancelled: bool` argument".to_owned());
        }
    } else if let (false, Some(ret)) = (replace, &sig.ret) {
        return Err(format!(
            "listen hook returns `{ret}`: its listeners answer nothing"
        ));
    }

    let outputs = outputs_of(hook, sig, cancel, dispatch, plain, reach)?;
    if replace && !outputs.is_empty() {
        return Err("replace hook with a mutable output: not in this slice".to_owned());
    }
    let mut fields = Vec::new();
    let mut checks = Vec::new();
    for arg in &hook.arguments {
        if cancel && arg.name == "cancelled" {
            continue;
        }
        if outputs.iter().any(|output| output.key == arg.name) {
            // An output is read from its state (see `turn_body`), not from the argument's value.
            continue;
        }
        // A stack or a block state (not a `&mut`) converts through the host, when it has one.
        let carried_shape = carried::shape_of(&arg.rust_type);
        if let Some(shape) = carried_shape.filter(|shape| *shape != Shape::StackList) {
            if arg.rust_type.contains("mut") {
                return Err(format!(
                    "argument `{}`: a mutable argument writes back, not in this slice",
                    arg.name
                ));
            }
            let (Some(view), true) = (view.as_deref(), carried::allowed(shape, reach)) else {
                return Err(carried::refusal(shape));
            };
            let field = format!("{}.{}", sig.event_name, arg.name);
            checks.push(carried::input_check(shape, &arg.rust_type, &field, view));
            let built = turn_view.as_deref().unwrap_or(view);
            fields.push((
                arg.name.clone(),
                carried::input_lua(shape, &arg.rust_type, &field, built),
            ));
            continue;
        }
        if is_generated_handle(&arg.rust_type, handles) {
            let field = format!("{}.{}", sig.event_name, arg.name);
            let expr = handle_input(&hook.name, &arg.name, &arg.rust_type, &field)?;
            if turn.is_none() {
                return Err(match sig.host_ty.as_deref() {
                    Some(host) => format!("handle input: host `{host}` is not carried by a turn"),
                    None => "handle input: the hook has no host".to_owned(),
                });
            }
            fields.push((arg.name.clone(), expr));
            continue;
        }
        let mapped = map_type(&arg.rust_type)
            .map_err(|error| format!("argument `{}`: {error}", arg.name))?;
        let shown = format!(
            "argument `{}` has type `{}` (kind {:?})",
            arg.name,
            arg.rust_type,
            kind_of(&mapped)
        );
        if mapped.access == Access::RefMut {
            return Err(format!(
                "{shown}: a mutable argument writes back, not in this slice"
            ));
        }
        let reference_in_option = arg.rust_type.contains("Option<&")
            && !matches!(&mapped.ty, TypeIr::Option(inner) if matches!(**inner, TypeIr::Str));
        if reference_in_option {
            return Err(format!(
                "{shown}: a reference inside Option has no Lua conversion"
            ));
        }
        match to_lua_expr(&format!("{}.{}", sig.event_name, arg.name), &mapped, plain) {
            Some(expr) => fields.push((arg.name.clone(), expr)),
            None => return Err(format!("{shown}: no Lua conversion")),
        }
    }

    Ok(Method {
        hook,
        sig,
        fields,
        outputs,
        cancel,
        answer,
        view,
        turn,
        turn_view,
        checks,
    })
}

/// The skip reason of a hook that carries or answers a handle as an output (a `&mut` parameter, a
/// collect output or a replace answer): a mod cannot make the game store a handle.
pub const HANDLE_OUTPUT: &str = "handle output";

/// Whether a type's text names a handle (an identifier ending in `Ref`, such as `Option<EntityRef>`).
pub fn mentions_handle(ty: &str) -> bool {
    ty.split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .any(|word| word.len() > 3 && word.ends_with("Ref"))
}

/// The shape of a handle-typed hook argument, without its family name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleIn {
    Plain,
    Ref,
    Opt,
    OptRef,
    RefOpt,
    List,
}

/// The shape and family name of a handle-typed argument (`&'a Option<EntityRef>`, `Vec<EntityRef>`),
/// or `None` for a spelling the conversion does not take.
fn handle_parts(ty: &str) -> Option<(HandleIn, &str)> {
    let wrapped = |prefix: &str| {
        ty.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix('>'))
    };
    let (shape, inner) = if let Some(inner) = wrapped("&Option<") {
        (HandleIn::RefOpt, inner)
    } else if let Some(inner) = wrapped("Option<&") {
        (HandleIn::OptRef, inner)
    } else if let Some(inner) = wrapped("Option<") {
        (HandleIn::Opt, inner)
    } else if let Some(inner) = wrapped("Vec<") {
        (HandleIn::List, inner)
    } else if let Some(inner) = ty.strip_prefix('&') {
        (HandleIn::Ref, inner)
    } else {
        (HandleIn::Plain, ty)
    };
    let plain_name = !inner.is_empty() && inner.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
    plain_name.then_some((shape, inner))
}

/// The argument type's text without its lifetimes and spaces.
fn compact(ty: &str) -> String {
    ty.replace("'a", "")
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

/// Whether a hook argument of type `ty` is a handle of a generated family (a `&mut` one too, which
/// is then a `handle output`), in a shape [`handle_input`] converts. Any other handle-named type keeps its old conversion (a `ChannelRef`
/// is a plain record, not a handle), so its hook's emission does not change.
fn is_generated_handle(ty: &str, handles: &[&str]) -> bool {
    let text = compact(ty);
    let family = handle_parts(text.trim_start_matches("&mut")).map(|(_, family)| family);
    family.is_some_and(|family| handles.contains(&family))
}

/// The Lua value of a handle-typed hook argument of a generated family: its userdata, nil for none,
/// or the list of userdata (capped by the handle module). `field` is the event field the value is
/// read from. A `&mut` handle writes back, so it is an output (`handle output`).
fn handle_input(hook: &str, name: &str, ty: &str, field: &str) -> Result<String, String> {
    let text = compact(ty);
    if text.starts_with("&mut") {
        return Err(HANDLE_OUTPUT.to_owned());
    }
    let Some((shape, _)) = handle_parts(&text) else {
        return Err(format!(
            "handle input `{name}`: `{ty}` is not a handle shape"
        ));
    };
    Ok(match shape {
        HandleIn::Plain => format!("crate::handles::family_out(lua, {field})?"),
        HandleIn::Ref => format!("crate::handles::family_out(lua, *{field})?"),
        HandleIn::Opt => format!("crate::handles::family_opt_out(lua, {field})?"),
        HandleIn::OptRef => format!("crate::handles::family_opt_out(lua, {field}.copied())?"),
        HandleIn::RefOpt => format!("crate::handles::family_opt_out(lua, *{field})?"),
        HandleIn::List => format!("crate::handles::family_capped_out(lua, {hook:?}, &{field})?"),
    })
}

/// The Lua value of the field `field_expr` (such as `event.pos`) as an `mlua::Value`, or `None`
/// when the type has no conversion. Every conversion used here is an impl of `ToLuaValue`.
fn to_lua_expr(field_expr: &str, mapped: &Mapped, plain: &Plain) -> Option<String> {
    match &mapped.ty {
        TypeIr::Str => Some(format!("{field_expr}.to_string().to_lua_value(lua)?")),
        TypeIr::Option(inner) if matches!(**inner, TypeIr::Str) => Some(format!(
            "{field_expr}.as_ref().map(|value| value.to_string()).to_lua_value(lua)?"
        )),
        ty => convertible(ty, plain).then(|| format!("{field_expr}.to_lua_value(lua)?")),
    }
}

/// The longest array a Lua value converts: `[T; N]` with `1 <= N <= MAX_ARRAY_LEN`, `T` a scalar.
pub const MAX_ARRAY_LEN: usize = 16;

/// The Rust name of a bool, float or integer type that converts, or `None` (`i128` and `isize`
/// have no impl). The same name spells the array impls in `generated_types`.
pub fn scalar_name(ty: &TypeIr) -> Option<&'static str> {
    Some(match ty {
        TypeIr::Bool => "bool",
        TypeIr::Float(FloatKind::F32) => "f32",
        TypeIr::Float(FloatKind::F64) => "f64",
        TypeIr::Int(kind) => match kind {
            IntKind::I8 => "i8",
            IntKind::U8 => "u8",
            IntKind::I16 => "i16",
            IntKind::U16 => "u16",
            IntKind::I32 => "i32",
            IntKind::U32 => "u32",
            IntKind::I64 => "i64",
            IntKind::U64 => "u64",
            IntKind::U128 => "u128",
            IntKind::Usize => "usize",
            IntKind::I128 | IntKind::Isize => return None,
        },
        _ => return None,
    })
}

/// Whether `ty` has a `ToLuaValue` impl in `event-horizon-lua-convert`, or is a plain surface type
/// whose impl `generated_types` has (for a reference, the referent). `u128` (a UUID, as text),
/// `usize` and `Identifier` have impls in its `ids` module; `i128` and `isize` have none. `Duration`
/// has one in `scalar.rs`; `char`, `Path` and `PathBuf` have one in `generated_types` (the std impls).
pub fn convertible(ty: &TypeIr, plain: &Plain) -> bool {
    match ty {
        TypeIr::Str => true,
        TypeIr::Bool | TypeIr::Float(_) | TypeIr::Int(_) => scalar_name(ty).is_some(),
        TypeIr::Option(inner) => !matches!(**inner, TypeIr::Option(_)) && convertible(inner, plain),
        TypeIr::Array(elem, len) => {
            (1..=MAX_ARRAY_LEN).contains(len) && scalar_name(elem).is_some()
        }
        TypeIr::Named(name) => {
            matches!(
                name.as_str(),
                "BlockPos"
                    | "Compound"
                    | "Text"
                    | "text::Text"
                    | "Identifier"
                    | "Rotation"
                    | "Duration"
                    | "char"
                    | "Path"
                    | "PathBuf"
            ) || plain.contains(name)
        }
        _ => false,
    }
}

/// The `use` lines the file needs: the host's, the convert trait if a value is converted, the mod
/// API, and every type the signatures name from the mod API.
fn imports(methods: &[Method<'_>]) -> String {
    let mut out = String::from("use crate::host::LuaHost;\n");
    if methods
        .iter()
        .any(|method| !method.fields.is_empty() || !method.outputs.is_empty())
    {
        out.push_str("use event_horizon_lua_convert::scalar::ToLuaValue;\n");
    }

    let mut names = BTreeSet::new();
    for method in methods {
        let texts = [
            method.sig.host_ty.as_deref(),
            Some(method.sig.event_ty.as_str()),
            method.sig.ret.as_deref(),
        ];
        for text in texts.into_iter().flatten() {
            names.extend(type_names(text));
        }
    }
    if !names.is_empty() {
        let list: Vec<String> = names.into_iter().collect();
        if let [only] = list.as_slice() {
            out.push_str(&format!("use mod_api::{only};\n"));
        } else {
            out.push_str(&format!("use mod_api::{{{}}};\n", list.join(", ")));
        }
    }
    out
}

/// The plain type names in a signature's text: not lifetimes, not path prefixes, not the
/// primitives or std names the file does not import.
fn type_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut start = None;
    for (index, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        let is_ident = ch.is_alphanumeric() || ch == '_';
        match (start, is_ident) {
            (None, true) => start = Some(index),
            (Some(from), false) => {
                start = None;
                let word = &text[from..index];
                if is_plain_name(text, from, index, word) {
                    names.push(word.to_owned());
                }
            }
            _ => {}
        }
    }
    names
}

fn is_plain_name(text: &str, from: usize, to: usize, word: &str) -> bool {
    const NOT_IMPORTED: [&str; 26] = [
        "Self", "Option", "Vec", "String", "Box", "Arc", "dyn", "mut", "bool", "char", "str", "i8",
        "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32",
        "f64", "_",
    ];
    let before = &text[..from];
    let after = &text[to..];
    !(before.ends_with('\'')
        || before.ends_with("::")
        || after.starts_with("::")
        || NOT_IMPORTED.contains(&word))
}

/// Replaces the text between each pair of backticks with `<..>`, so reasons that differ only
/// in the names they quote group together.
pub fn group_key(reason: &str) -> String {
    let mut key = String::new();
    let mut inside = false;
    for ch in reason.chars() {
        if ch == '`' {
            if !inside {
                key.push_str("<..>");
            }
            inside = !inside;
        } else if !inside {
            key.push(ch);
        }
    }
    key
}

/// One method, with its leading blank line, as the file spells it.
fn method_text(method: &Method<'_>) -> String {
    let sig = method.sig;
    // A reference field is cloned out (its type may be `Copy`, which clippy reports): one allow.
    let allow = if method.outputs.iter().any(|output| output.by_ref) {
        "    #[allow(clippy::clone_on_copy)]\n"
    } else {
        ""
    };
    let mut out = format!("\n{allow}    {} {{\n", sig.signature());
    if let Some(host) = &sig.host_name {
        out.push_str(&format!("        let _ = &{host};\n"));
    }
    if method.fields.is_empty() && method.outputs.is_empty() {
        out.push_str(&format!("        let _ = &{};\n", sig.event_name));
    }
    if let Some(answer) = &method.answer {
        out.push_str(&checks_text(method, false));
        out.push_str(&replace_body(method, answer));
    } else if method.outputs.is_empty() {
        out.push_str(&checks_text(method, false));
        out.push_str(&plain_body(method));
    } else {
        out.push_str(&turn_body(method));
    }
    out.push_str("    }\n");
    out
}

/// The `TurnHost` a hook's method opens its turns with: the variant its host parameter is, or
/// `TurnHost::None` for a hook that has no host the turns carry (its handles raise `no host call
/// outside a hook`). `host` is moved into the turn, so the body does not use it after the call.
fn turn_expr(host_name: Option<&str>, host_ty: Option<&str>) -> Option<String> {
    let (name, ty) = (host_name?, host_ty?);
    Some(match ty {
        "&mut dyn ServerHost" => format!("Server({name})"),
        "&mut dyn ClientHost" => format!("Client({name})"),
        "&dyn ClientHost" => format!("ClientRead({name})"),
        "&dyn Host" => format!("Read({name})"),
        "HostSide" => format!("of_side({name})"),
        _ => return None,
    })
}

/// The `TurnHost` argument of a method's dispatch call.
fn turn_text(method: &Method<'_>) -> String {
    match &method.turn {
        Some(variant) => format!("crate::hostcall::TurnHost::{variant}"),
        None => "crate::hostcall::TurnHost::None".to_owned(),
    }
}

/// The closure's parameter for the turn: `turn` when its body reads the turn's host, else `_turn`.
/// Only the turn's own host view has that spelling, so no field name can match it.
fn turn_param(body: &str) -> &'static str {
    if body.contains("turn.read_or_unavailable()") {
        "turn"
    } else {
        "_turn"
    }
}

/// The `build` closure of a hook: the event table with its fields (and, for a hook with outputs,
/// their current values), then the event. It reads the host through the turn, not the method's
/// parameter, which the open turn holds.
fn build_text(method: &Method<'_>, outputs: bool) -> String {
    let mut body = String::from("                let data = lua.create_table()?;\n");
    for (field, expr) in &method.fields {
        body.push_str(&format!(
            "                data.raw_set({field:?}, {expr})?;\n"
        ));
    }
    let mut keys = Vec::new();
    if outputs {
        for output in &method.outputs {
            let value = match (output.shape, method.turn_view.as_deref()) {
                (Some(shape), Some(view)) => carried::output_lua(shape, &output.key, view),
                _ => format!("out_{}.borrow().to_lua_value(lua)?", output.key),
            };
            body.push_str(&format!(
                "                data.raw_set({key:?}, {value})?;\n",
                key = output.key
            ));
            keys.push(format!("{:?}", output.key));
        }
    }
    body.push_str(&format!(
        "                crate::dispatch::new_event(lua, data, &[{}])\n",
        keys.join(", ")
    ));
    format!(
        "            &|lua, {}| {{\n{body}            }},\n",
        turn_param(&body)
    )
}

/// The body of a hook with no outputs: its handlers run in the turn, and nothing is committed.
fn plain_body(method: &Method<'_>) -> String {
    let mut out = String::from("        crate::hostcall::refused(self.dispatch_turns(\n");
    out.push_str(&format!("            {:?},\n", method.hook.name));
    out.push_str(&format!("            {},\n", turn_text(method)));
    out.push_str("            &|| false,\n");
    out.push_str(&build_text(method, false));
    out.push_str("            &mut |_, _, _| true,\n");
    out.push_str("        ));\n");
    out
}

/// The pre-checks of a hook: each stack or block state in it must have an id, or the hook is not
/// delivered (one log line, and the hook returns as if no mod listened). `outputs` adds the outputs'
/// current values; they are read from their cells, so it runs after the cells are made.
fn checks_text(method: &Method<'_>, outputs: bool) -> String {
    let mut calls = method.checks.clone();
    if let (true, Some(view)) = (outputs, method.view.as_deref()) {
        for output in &method.outputs {
            if let Some(shape) = output.shape {
                calls.push(carried::output_check(shape, &output.key, view));
            }
        }
    }
    let event = &method.sig.event_name;
    let ret = if method.cancel {
        format!(" if {event}.cancelled {{ Flow::Cancel }} else {{ Flow::Continue }}")
    } else if method.answer.is_some() {
        " None".to_owned()
    } else {
        String::new()
    };
    let hook = &method.hook.name;
    if calls.is_empty() {
        return String::new();
    }
    // No mod handles the hook: nothing is converted, and the hook returns as if nothing listened.
    let mut out = format!(
        "        if !self.has_handlers({hook:?}) {{\n            return{ret};\n        }}\n"
    );
    for call in calls {
        out.push_str(&format!(
            "        if {call}.is_none() {{\n            crate::stacks::undelivered({hook:?});\n            return{ret};\n        }}\n"
        ));
    }
    out
}

/// The body of a replace hook: `dispatch_replace` gives the first mod's answer that is not nil, and
/// `answer_of` converts it. A refused answer is logged there and the chain goes on.
fn replace_body(method: &Method<'_>, answer: &str) -> String {
    let hook = &method.hook.name;
    let mut out = String::from("        let mut answer = None;\n");
    out.push_str("        crate::hostcall::refused(self.dispatch_replace(\n");
    out.push_str(&format!("            {hook:?},\n"));
    out.push_str(&format!("            {},\n", turn_text(method)));
    out.push_str(&build_text(method, false));
    let mut body = String::new();
    match carried::shape_of(answer).zip(method.turn_view.as_deref()) {
        Some((shape, view)) => body.push_str(&format!(
            "                match crate::dispatch::answer_by(id, {hook:?}, &value, {}) {{\n",
            carried::converter(shape, view)
        )),
        None => body.push_str(&format!(
            "                match crate::dispatch::answer_of::<{answer}>(id, {hook:?}, &value) {{\n"
        )),
    }
    body.push_str("                    Some(found) => {\n");
    body.push_str("                        answer = Some(found);\n");
    body.push_str("                        true\n");
    body.push_str("                    }\n");
    body.push_str("                    None => false,\n");
    body.push_str("                }\n");
    out.push_str(&format!(
        "            &mut |id, value, {}| {{\n{body}            }},\n",
        turn_param(&body)
    ));
    out.push_str("        ));\n");
    out.push_str("        answer\n");
    out
}

/// The body of a hook with outputs. Each output's current value is a `RefCell` that starts as the
/// event's field. The event table gets the current values as data and the output keys as writable
/// keys. After each mod's turn `accept` takes the value the mod wrote, if it converts. A cancel
/// hook's flag is also the value its `is_cancelled` test reads. At the end the final values are
/// written back, and a cancel hook returns the [`Flow`] of its final flag.
fn turn_body(method: &Method<'_>) -> String {
    let event = &method.sig.event_name;
    let hook = &method.hook.name;
    let mut out = String::new();
    for output in &method.outputs {
        // A list, a carried value or a value behind a reference is copied out of the event: the event
        // keeps its own value until the write back.
        let copy = if output.by_ref || output.shape.is_some() || output.ty.starts_with("Vec<") {
            ".clone()"
        } else {
            ""
        };
        out.push_str(&format!(
            "        let out_{key} = std::cell::RefCell::new({event}.{key}{copy});\n",
            key = output.key
        ));
    }
    out.push_str(&checks_text(method, true));
    let is_cancelled = if method.cancel {
        "&|| *out_cancelled.borrow()"
    } else {
        "&|| false"
    };
    out.push_str("        crate::hostcall::refused(self.dispatch_turns(\n");
    out.push_str(&format!("            {hook:?},\n"));
    out.push_str(&format!("            {},\n", turn_text(method)));
    out.push_str(&format!("            {is_cancelled},\n"));
    out.push_str(&build_text(method, true));
    out.push_str(&commit_text(method));
    out.push_str("        ));\n");
    for output in &method.outputs {
        let deref = if output.by_ref { "*" } else { "" };
        out.push_str(&format!(
            "        {deref}{event}.{key} = out_{key}.into_inner();\n",
            key = output.key
        ));
    }
    if method.cancel {
        out.push_str(&format!(
            "        if {event}.cancelled {{ Flow::Cancel }} else {{ Flow::Continue }}\n"
        ));
    }
    out
}

/// The `commit` closure of a hook with outputs: each output the mod wrote is accepted if it
/// converts, and the chain goes on.
fn commit_text(method: &Method<'_>) -> String {
    let hook = &method.hook.name;
    let mut body = String::new();
    for output in &method.outputs {
        // An `Option` output takes `horizon.none` as None; any other output refuses it.
        let optional = output.ty.starts_with("Option<");
        match (output.shape, method.turn_view.as_deref()) {
            (Some(shape), Some(view)) => body.push_str(&format!(
                "                crate::dispatch::{accept}(&out_{key}, id, {hook:?}, {key:?}, &answered.answer({key:?}), {convert});\n",
                accept = if optional { "accept_opt_by" } else { "accept_by" },
                key = output.key,
                convert = carried::converter(shape, view)
            )),
            _ => body.push_str(&format!(
                "                crate::dispatch::{accept}(&out_{key}, id, {hook:?}, {key:?}, &answered.answer({key:?}));\n",
                accept = if optional { "accept_opt" } else { "accept" },
                key = output.key
            )),
        }
    }
    body.push_str("                true\n");
    format!(
        "            &mut |id, answered, {}| {{\n{body}            }},\n",
        turn_param(&body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch_sigs::OutputSig;
    use crate::emit_types::Plain;
    use crate::ir::{ArgIr, Side};
    use std::collections::BTreeMap;

    fn arg(name: &str, ty: &str) -> ArgIr {
        ArgIr {
            name: name.to_owned(),
            rust_type: ty.to_owned(),
        }
    }

    fn hook(name: &str, kind: Kind, arguments: Vec<ArgIr>) -> HookIr {
        HookIr {
            id: format!("hook:{name}"),
            name: name.to_owned(),
            kind,
            side: Side::Server,
            cancellable: None,
            scope: None,
            engine_target: format!("horizon_client::game::{name}"),
            arguments,
            output: None,
            hash: "0".to_owned(),
            version: 1,
        }
    }

    fn sig(
        name: &str,
        host: Option<(&str, &str)>,
        event_name: &str,
        event_ty: &str,
        ret: Option<&str>,
    ) -> HookSig {
        HookSig {
            name: name.to_owned(),
            host_name: host.map(|(name, _)| name.to_owned()),
            host_ty: host.map(|(_, ty)| ty.to_owned()),
            event_name: event_name.to_owned(),
            event_ty: event_ty.to_owned(),
            ret: ret.map(str::to_owned),
        }
    }

    /// `emit` for hooks that have no outputs: each hook of `ir` is given an empty output list.
    fn emit_plain(ir: &Ir, sigs: &[HookSig], hooks: &[&str], plain: &Plain) -> Emitted {
        let dispatch = DispatchSigs {
            hooks: ir
                .hooks
                .iter()
                .map(|hook| (hook.name.clone(), Some(Vec::new())))
                .collect(),
            structs: BTreeMap::new(),
        };
        emit(ir, sigs, &dispatch, hooks, plain)
    }

    fn ir(hooks: Vec<HookIr>) -> Ir {
        Ir {
            hooks,
            document_hash: "abc123".to_owned(),
        }
    }

    /// The shape of `block_step_on` without its `state` argument (which has no conversion).
    fn step_ir() -> Ir {
        ir(vec![hook(
            "block_step_on",
            Kind::Listen,
            vec![
                arg("dimension", "&'a str"),
                arg("entity", "i32"),
                arg("position", "[i32; 3]"),
            ],
        )])
    }

    fn step_sig() -> HookSig {
        sig("block_step_on", None, "event", "BlockStepOnEvent<'_>", None)
    }

    /// The shape of `block_receive_game`: a cancel hook with a `cancelled` argument.
    fn receive_ir() -> Ir {
        ir(vec![hook(
            "block_receive_game",
            Kind::Cancel,
            vec![
                arg("dimension", "&'a str"),
                arg("pos", "[i32; 3]"),
                arg("block", "&'a str"),
                arg("event", "&'a str"),
                arg("cancelled", "bool"),
            ],
        )])
    }

    /// The dispatch of `block_receive_game`: its `cancel` output and the `cancelled` field.
    fn receive_dispatch() -> DispatchSigs {
        DispatchSigs {
            hooks: [(
                "block_receive_game".to_owned(),
                Some(vec![OutputSig {
                    copies_cancel: true,
                    param: "cancel".to_owned(),
                    ty: "bool".to_owned(),
                }]),
            )]
            .into(),
            structs: [(
                "BlockReceiveGameEvent".to_owned(),
                [("cancelled".to_owned(), "bool".to_owned())].into(),
            )]
            .into(),
        }
    }

    /// The dispatch of `compute_fov`: its `degrees` output and the event's field.
    fn compute_fov_dispatch() -> DispatchSigs {
        DispatchSigs {
            hooks: [(
                "compute_fov".to_owned(),
                Some(vec![OutputSig {
                    copies_cancel: false,
                    param: "degrees".to_owned(),
                    ty: "f32".to_owned(),
                }]),
            )]
            .into(),
            structs: [(
                "ComputeFovEvent".to_owned(),
                [("degrees".to_owned(), "f32".to_owned())].into(),
            )]
            .into(),
        }
    }

    fn receive_sig() -> HookSig {
        sig(
            "block_receive_game",
            Some(("host", "&mut dyn ServerHost")),
            "event_",
            "BlockReceiveGameEvent<'_>",
            Some("Flow"),
        )
    }

    #[test]
    fn a_listen_hook_gets_a_golden_method() {
        let out = emit_plain(
            &step_ir(),
            &[step_sig()],
            &["block_step_on"],
            &Plain::default(),
        );
        let expected = r#"// @generated by event-horizon-lua-gen from surface abc123: do not edit.
use crate::host::LuaHost;
use event_horizon_lua_convert::scalar::ToLuaValue;
use mod_api::BlockStepOnEvent;

impl mod_api::Hooks for LuaHost {
    fn subscribed(&self) -> &'static [Option<mod_api::HookId>] {
        self.subscribed_slice()
    }

    fn register(&self, r: &mut dyn mod_api::Registrar, _cx: &mod_api::RegistrationContext) {
        self.on_register(r)
    }

    fn listeners(&self) -> &'static [mod_api::Listener] {
        self.listeners_slice()
    }

    fn block_step_on(&self, event: &mut BlockStepOnEvent<'_>) {
        crate::hostcall::refused(self.dispatch_turns(
            "block_step_on",
            crate::hostcall::TurnHost::None,
            &|| false,
            &|lua, _turn| {
                let data = lua.create_table()?;
                data.raw_set("dimension", event.dimension.to_string().to_lua_value(lua)?)?;
                data.raw_set("entity", event.entity.to_lua_value(lua)?)?;
                data.raw_set("position", event.position.to_lua_value(lua)?)?;
                crate::dispatch::new_event(lua, data, &[])
            },
            &mut |_, _, _| true,
        ));
    }
}
"#;
        assert_eq!(out.text, expected);
        assert_eq!(out.emitted, ["block_step_on"]);
        assert!(out.skipped.is_empty());
        syn::parse_file(&out.text).expect("the golden text parses");
    }

    #[test]
    fn a_stack_in_hook_converts_its_stack_through_the_host_and_is_pre_checked() {
        let ir = ir(vec![hook(
            "stack_seen",
            Kind::Listen,
            vec![arg("stack", "&'a item::item_stack::ItemStack")],
        )]);
        let sigs = [sig(
            "stack_seen",
            Some(("host", "&mut dyn ServerHost")),
            "event",
            "StackSeenEvent<'_>",
            None,
        )];
        let out = emit_plain(&ir, &sigs, &["stack_seen"], &Plain::default());
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        let expected = r#"    fn stack_seen(&self, host: &mut dyn ServerHost, event: &mut StackSeenEvent<'_>) {
        let _ = &host;
        if !self.has_handlers("stack_seen") {
            return;
        }
        if crate::stacks::stack_in(&*host as &dyn mod_api::Host, Some(event.stack)).is_none() {
            crate::stacks::undelivered("stack_seen");
            return;
        }
        crate::hostcall::refused(self.dispatch_turns(
            "stack_seen",
            crate::hostcall::TurnHost::Server(host),
            &|| false,
            &|lua, turn| {
                let data = lua.create_table()?;
                data.raw_set("stack", crate::stacks::stack_in(turn.read_or_unavailable(), Some(event.stack)).ok_or_else(|| mlua::Error::runtime("a carried value has no id"))?.to_lua_value(lua)?)?;
                crate::dispatch::new_event(lua, data, &[])
            },
            &mut |_, _, _| true,
        ));
    }
"#;
        assert!(out.text.contains(expected), "{}", out.text);
        // The gate precedes every conversion of the hook: a hook no mod handles converts nothing.
        let gate = out
            .text
            .find("has_handlers(\"stack_seen\")")
            .expect("gated");
        let first = out.text.find("stack_in(").expect("converted");
        assert!(
            gate < first,
            "the gate must come before the first conversion"
        );
    }

    #[test]
    fn a_state_out_cancel_hook_reads_its_state_and_writes_it_back_through_the_level() {
        let place = hook(
            "place_state",
            Kind::Cancel,
            vec![arg("cancelled", "bool"), arg("state", "&'a mut BlockState")],
        );
        let ir = ir(vec![place]);
        let sigs = [sig(
            "place_state",
            Some(("level", "&dyn LevelHost")),
            "event_",
            "PlaceStateEvent<'_>",
            Some("Flow"),
        )];
        let dispatch = DispatchSigs {
            hooks: [(
                "place_state".to_owned(),
                Some(vec![
                    OutputSig {
                        param: "state".to_owned(),
                        ty: "BlockState".to_owned(),
                        copies_cancel: false,
                    },
                    OutputSig {
                        param: "cancel".to_owned(),
                        ty: "bool".to_owned(),
                        copies_cancel: true,
                    },
                ]),
            )]
            .into(),
            structs: [(
                "PlaceStateEvent".to_owned(),
                [
                    ("state".to_owned(), "&'a mut BlockState".to_owned()),
                    ("cancelled".to_owned(), "bool".to_owned()),
                ]
                .into(),
            )]
            .into(),
        };
        let out = emit(&ir, &sigs, &dispatch, &["place_state"], &Plain::default());
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        for piece in [
            "    fn place_state(&self, level: &dyn LevelHost, event_: &mut PlaceStateEvent<'_>) -> Flow {\n",
            "        if crate::stacks::state_in(level, Some(&*out_state.borrow())).is_none() {\n            crate::stacks::undelivered(\"place_state\");\n            return if event_.cancelled { Flow::Cancel } else { Flow::Continue };\n        }\n",
            "                data.raw_set(\"state\", crate::stacks::state_in(level, Some(&*out_state.borrow())).ok_or_else(|| mlua::Error::runtime(\"a carried value has no id\"))?.to_lua_value(lua)?)?;\n",
            "                crate::dispatch::accept_by(&out_state, id, \"place_state\", \"state\", &answered.answer(\"state\"), |value| crate::stacks::state_value(level, value));\n",
            "        *event_.state = out_state.into_inner();\n",
        ] {
            assert!(
                out.text.contains(piece),
                "missing:\n{piece}\nin:\n{}",
                out.text
            );
        }
    }

    #[test]
    fn a_cancel_hook_answers_with_cancelled_and_returns_flow() {
        let out = emit(
            &receive_ir(),
            &[receive_sig()],
            &receive_dispatch(),
            &["block_receive_game"],
            &Plain::default(),
        );
        let text = &out.text;
        for piece in [
            "use mod_api::{BlockReceiveGameEvent, Flow, ServerHost};\n",
            "    fn block_receive_game(&self, host: &mut dyn ServerHost, event_: &mut BlockReceiveGameEvent<'_>) -> Flow {\n",
            "        let _ = &host;\n",
            "        let out_cancelled = std::cell::RefCell::new(event_.cancelled);\n",
            "        crate::hostcall::refused(self.dispatch_turns(\n            \"block_receive_game\",\n            crate::hostcall::TurnHost::Server(host),\n            &|| *out_cancelled.borrow(),\n",
            "                data.raw_set(\"cancelled\", out_cancelled.borrow().to_lua_value(lua)?)?;\n",
            "                crate::dispatch::new_event(lua, data, &[\"cancelled\"])\n",
            "                crate::dispatch::accept(&out_cancelled, id, \"block_receive_game\", \"cancelled\", &answered.answer(\"cancelled\"));\n",
            "        event_.cancelled = out_cancelled.into_inner();\n",
            "        if event_.cancelled { Flow::Cancel } else { Flow::Continue }\n",
        ] {
            assert!(text.contains(piece), "missing {piece:?} in:\n{text}");
        }
        assert!(
            !text.contains("\"cancel\""),
            "the answer key is `cancelled`:\n{text}"
        );
        assert!(text.contains("data.raw_set(\"pos\", event_.pos.to_lua_value(lua)?)?;"));
        syn::parse_file(text).expect("the cancel text parses");
    }

    #[test]
    fn a_scalar_output_hook_gets_a_golden_method() {
        let ir = ir(vec![hook(
            "compute_fov",
            Kind::Listen,
            vec![
                arg("partial", "f32"),
                arg("camera", "[f64; 3]"),
                arg("degrees", "f32"),
            ],
        )]);
        let sigs = [sig(
            "compute_fov",
            Some(("host", "&dyn ClientHost")),
            "event",
            "ComputeFovEvent",
            None,
        )];
        let out = emit(
            &ir,
            &sigs,
            &compute_fov_dispatch(),
            &["compute_fov"],
            &Plain::default(),
        );
        let expected = r#"// @generated by event-horizon-lua-gen from surface abc123: do not edit.
use crate::host::LuaHost;
use event_horizon_lua_convert::scalar::ToLuaValue;
use mod_api::{ClientHost, ComputeFovEvent};

impl mod_api::Hooks for LuaHost {
    fn subscribed(&self) -> &'static [Option<mod_api::HookId>] {
        self.subscribed_slice()
    }

    fn register(&self, r: &mut dyn mod_api::Registrar, _cx: &mod_api::RegistrationContext) {
        self.on_register(r)
    }

    fn listeners(&self) -> &'static [mod_api::Listener] {
        self.listeners_slice()
    }

    fn compute_fov(&self, host: &dyn ClientHost, event: &mut ComputeFovEvent) {
        let _ = &host;
        let out_degrees = std::cell::RefCell::new(event.degrees);
        crate::hostcall::refused(self.dispatch_turns(
            "compute_fov",
            crate::hostcall::TurnHost::ClientRead(host),
            &|| false,
            &|lua, _turn| {
                let data = lua.create_table()?;
                data.raw_set("partial", event.partial.to_lua_value(lua)?)?;
                data.raw_set("camera", event.camera.to_lua_value(lua)?)?;
                data.raw_set("degrees", out_degrees.borrow().to_lua_value(lua)?)?;
                crate::dispatch::new_event(lua, data, &["degrees"])
            },
            &mut |id, answered, _turn| {
                crate::dispatch::accept(&out_degrees, id, "compute_fov", "degrees", &answered.answer("degrees"));
                true
            },
        ));
        event.degrees = out_degrees.into_inner();
    }
}
"#;
        assert_eq!(out.text, expected);
        assert_eq!(out.emitted, ["compute_fov"]);
        assert_eq!(out.output_types.get("f32"), Some(&1));
        syn::parse_file(&out.text).expect("the golden text parses");
    }

    #[test]
    fn an_option_output_takes_horizon_none_as_none() {
        let ir = ir(vec![hook(
            "pick_thing",
            Kind::Listen,
            vec![arg("value", "Option<u32>")],
        )]);
        let sigs = [sig("pick_thing", None, "event", "PickThingEvent<'_>", None)];
        let dispatch = DispatchSigs {
            hooks: [(
                "pick_thing".to_owned(),
                Some(vec![OutputSig {
                    copies_cancel: false,
                    param: "value".to_owned(),
                    ty: "Option<u32>".to_owned(),
                }]),
            )]
            .into(),
            structs: [(
                "PickThingEvent".to_owned(),
                [("value".to_owned(), "Option<u32>".to_owned())].into(),
            )]
            .into(),
        };
        let out = emit(&ir, &sigs, &dispatch, &["pick_thing"], &Plain::default());
        assert!(
            out.text
                .contains("crate::dispatch::accept_opt(&out_value, id, \"pick_thing\", \"value\""),
            "{}",
            out.text
        );
        assert!(out.text.contains("event.value = out_value.into_inner();"));
        syn::parse_file(&out.text).expect("the text parses");
    }

    #[test]
    fn an_output_the_conversion_layer_lacks_skips_its_hook_with_the_type() {
        let ir = ir(vec![hook("vec_out", Kind::Listen, vec![])]);
        let sigs = [sig("vec_out", None, "event", "VecOutEvent", None)];
        let dispatch = DispatchSigs {
            hooks: [(
                "vec_out".to_owned(),
                Some(vec![OutputSig {
                    copies_cancel: false,
                    param: "lines".to_owned(),
                    ty: "Vec<(Text,u32)>".to_owned(),
                }]),
            )]
            .into(),
            structs: BTreeMap::new(),
        };
        let out = emit(&ir, &sigs, &dispatch, &["vec_out"], &Plain::default());
        assert!(out.emitted.is_empty());
        assert_eq!(out.skipped[0].1, "output: Vec<(Text,u32)>");
    }

    #[test]
    fn an_output_without_a_matching_event_field_skips_its_hook() {
        let ir = ir(vec![hook("mismatch", Kind::Listen, vec![])]);
        let sigs = [sig("mismatch", None, "event", "MismatchEvent", None)];
        let dispatch = DispatchSigs {
            hooks: [(
                "mismatch".to_owned(),
                Some(vec![OutputSig {
                    copies_cancel: false,
                    param: "ratio".to_owned(),
                    ty: "f32".to_owned(),
                }]),
            )]
            .into(),
            structs: [(
                "MismatchEvent".to_owned(),
                [("ratio".to_owned(), "f64".to_owned())].into(),
            )]
            .into(),
        };
        let out = emit(&ir, &sigs, &dispatch, &["mismatch"], &Plain::default());
        assert!(out.emitted.is_empty());
        assert!(out.skipped[0].1.contains("`ratio`"), "{:?}", out.skipped);
    }

    #[test]
    fn output_types_are_the_supported_set() {
        for ty in [
            "bool",
            "i32",
            "u32",
            "f32",
            "f64",
            "String",
            "Text",
            "[f64;3]",
            "Option<u32>",
            "Option<Text>",
            "Option<bool>",
            "Vec<String>",
            "Vec<[i32;3]>",
            "[i32;3]",
            "[f32;3]",
        ] {
            assert!(output_supported(ty, &Plain::default()), "{ty}");
        }
        for ty in [
            "Vec<(Text,u32)>",
            "Option<Option<u32>>",
            "TriState",
            "&mut dyn Host",
            "Option<ItemStack>",
            "u64",
        ] {
            assert!(!output_supported(ty, &Plain::default()), "{ty}");
        }
        let decided = Plain::deciding(["TriState"], ["TriState"]);
        assert!(output_supported("TriState", &decided));
        assert!(output_supported("Option<TriState>", &decided));
        assert!(!output_supported(
            "TriState",
            &Plain::from_paths(["TriState"])
        ));
    }

    #[test]
    fn an_unconvertible_field_is_skipped_with_its_name_and_type() {
        let ir = ir(vec![hook(
            "level_thing",
            Kind::Listen,
            vec![arg("level", "&'a LevelRef"), arg("kind_name", "&'a str")],
        )]);
        let out = emit_plain(
            &ir,
            &[sig(
                "level_thing",
                None,
                "event",
                "LevelThingEvent<'_>",
                None,
            )],
            &["level_thing"],
            &Plain::default(),
        );
        assert!(out.emitted.is_empty());
        assert_eq!(out.skipped.len(), 1);
        let (name, reason) = &out.skipped[0];
        assert_eq!(name, "level_thing");
        assert!(reason.contains("`level`"), "{reason}");
        assert!(reason.contains("`&'a LevelRef`"), "{reason}");
        assert!(reason.contains("Handle"), "{reason}");
    }

    #[test]
    fn a_named_plain_type_converts_only_when_the_plain_set_has_it() {
        let ir = ir(vec![hook(
            "server_thing",
            Kind::Listen,
            vec![arg("context", "&'a LifecycleContext")],
        )]);
        let sigs = [sig(
            "server_thing",
            None,
            "event",
            "ServerThingEvent<'_>",
            None,
        )];
        let without = emit_plain(&ir, &sigs, &["server_thing"], &Plain::default());
        assert!(without.emitted.is_empty());
        assert!(
            without.skipped[0].1.contains("no Lua conversion"),
            "{:?}",
            without.skipped
        );

        let plain = Plain::from_paths(["LifecycleContext"]);
        let with = emit_plain(&ir, &sigs, &["server_thing"], &plain);
        assert_eq!(with.emitted, ["server_thing"]);
        assert!(
            with.text
                .contains("data.raw_set(\"context\", event.context.to_lua_value(lua)?)?;"),
            "{}",
            with.text
        );
        syn::parse_file(&with.text).expect("the text parses");
    }

    #[test]
    fn a_hook_outside_the_trait_or_the_surface_is_skipped_each_with_its_reason() {
        let mut collects = hook("collects", Kind::Collect, vec![]);
        collects.output = Some(arg("lines", "&'a mut Vec<(Text,u32)>"));
        let ir = ir(vec![hook("only_surface", Kind::Listen, vec![]), collects]);
        let sigs = [
            sig("only_trait", None, "event", "OnlyTraitEvent<'_>", None),
            sig("collects", None, "event", "CollectsEvent<'_>", None),
        ];
        let out = emit_plain(
            &ir,
            &sigs,
            &["only_surface", "only_trait", "collects"],
            &Plain::default(),
        );
        assert!(out.emitted.is_empty());
        let reasons: Vec<&str> = out.skipped.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(
            out.skipped
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            ["collects", "only_surface", "only_trait"]
        );
        assert_eq!(
            reasons[0],
            "has an output: its listeners write back, not in this slice"
        );
        assert_eq!(reasons[1], "not a method of the Hooks trait");
        assert_eq!(reasons[2], "not in the surface");
        assert_eq!(
            group_key("argument `a` has type `b`"),
            group_key("argument `c` has type `d`")
        );
    }

    #[test]
    fn a_cancel_hook_that_returns_nothing_is_skipped() {
        let out = emit_plain(
            &receive_ir(),
            &[sig(
                "block_receive_game",
                Some(("host", "&mut dyn ServerHost")),
                "event_",
                "BlockReceiveGameEvent<'_>",
                None,
            )],
            &["block_receive_game"],
            &Plain::default(),
        );
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].1, "cancel hook does not return Flow");
    }

    #[test]
    fn the_output_is_deterministic_and_in_name_order() {
        let ir = ir(vec![
            hook("a_step", Kind::Listen, vec![arg("entity", "i32")]),
            hook("b_step", Kind::Listen, vec![arg("entity", "i32")]),
        ]);
        let sigs = [
            sig("a_step", None, "event", "AStepEvent<'_>", None),
            sig("b_step", None, "event", "BStepEvent<'_>", None),
        ];
        let forward = emit_plain(
            &ir,
            &sigs,
            &["a_step", "b_step", "z_missing", "y_missing"],
            &Plain::default(),
        );
        let backward = emit_plain(
            &ir,
            &sigs,
            &["y_missing", "z_missing", "b_step", "a_step"],
            &Plain::default(),
        );
        assert_eq!(forward, backward);
        assert_eq!(
            forward,
            emit_plain(
                &ir,
                &sigs,
                &["a_step", "b_step", "z_missing", "y_missing"],
                &Plain::default()
            )
        );
        assert_eq!(forward.emitted, ["a_step", "b_step"]);
        let skipped: Vec<&str> = forward.skipped.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(skipped, ["y_missing", "z_missing"]);
        let first = forward.text.find("fn a_step").expect("a_step is emitted");
        let second = forward.text.find("fn b_step").expect("b_step is emitted");
        assert!(first < second);
    }

    #[test]
    fn a_hook_with_no_arguments_keeps_its_event_in_use() {
        let out = emit_plain(
            &ir(vec![hook("bare", Kind::Listen, vec![])]),
            &[sig("bare", None, "event", "BareEvent<'_>", None)],
            &["bare"],
            &Plain::default(),
        );
        assert!(out.text.contains("        let _ = &event;\n"));
        assert!(!out.text.contains("ToLuaValue"), "no value is converted");
        syn::parse_file(&out.text).expect("the text parses");
    }

    #[test]
    #[ignore = "reads $EH_SURFACE and $EH_HOOKS_TRAIT; run by hand"]
    fn the_real_surface_slice() {
        let (Some(surface), Some(trait_path)) = (
            std::env::var_os("EH_SURFACE"),
            std::env::var_os("EH_HOOKS_TRAIT"),
        ) else {
            eprintln!("EH_SURFACE and EH_HOOKS_TRAIT must both be set: skipping");
            return;
        };
        let json = std::fs::read(&surface).expect("EH_SURFACE is readable");
        let ir = crate::ir::load(&json).expect("the surface loads");
        let source = std::fs::read_to_string(&trait_path).expect("EH_HOOKS_TRAIT is readable");
        let sigs = crate::trait_sigs::parse(&source).expect("the trait parses");
        let names: Vec<&str> = ir.hooks.iter().map(|hook| hook.name.as_str()).collect();

        let dispatch = crate::dispatch_sigs::parse(&source).expect("the dispatch functions parse");
        let out = emit(&ir, &sigs, &dispatch, &names, &Plain::default());
        syn::parse_file(&out.text).expect("the emitted file parses");

        let mut table = std::collections::BTreeMap::<String, usize>::new();
        for (_, reason) in &out.skipped {
            *table.entry(group_key(reason)).or_default() += 1;
        }
        for (name, reason) in &out.skipped {
            if reason.contains("field of that name") {
                println!("no matching event field: {name}: {reason}");
            }
        }
        println!("hooks asked: {}", names.len());
        println!("emitted: {}", out.emitted.len());
        println!("skipped: {}", out.skipped.len());
        for (reason, count) in &table {
            println!("{count:5}  {reason}");
        }
    }

    /// The shape of a replace hook: `pick` answers with a `TriState` from its handlers' returns.
    fn replace_sig() -> HookSig {
        sig(
            "pick",
            Some(("host", "&dyn ClientHost")),
            "event",
            "PickEvent<'_>",
            Some("Option<mod_api::TriState>"),
        )
    }

    #[test]
    fn a_replace_hook_gets_a_golden_method_that_returns_its_first_answer() {
        let ir = ir(vec![hook(
            "pick",
            Kind::Replace,
            vec![arg("entity", "i32")],
        )]);
        let plain = Plain::deciding(["TriState"], ["TriState"]);
        let out = emit(
            &ir,
            &[replace_sig()],
            &DispatchSigs {
                hooks: [("pick".to_owned(), Some(Vec::new()))].into(),
                structs: BTreeMap::new(),
            },
            &["pick"],
            &plain,
        );
        let expected = r#"    fn pick(&self, host: &dyn ClientHost, event: &mut PickEvent<'_>) -> Option<mod_api::TriState> {
        let _ = &host;
        let mut answer = None;
        crate::hostcall::refused(self.dispatch_replace(
            "pick",
            crate::hostcall::TurnHost::ClientRead(host),
            &|lua, _turn| {
                let data = lua.create_table()?;
                data.raw_set("entity", event.entity.to_lua_value(lua)?)?;
                crate::dispatch::new_event(lua, data, &[])
            },
            &mut |id, value, _turn| {
                match crate::dispatch::answer_of::<mod_api::TriState>(id, "pick", &value) {
                    Some(found) => {
                        answer = Some(found);
                        true
                    }
                    None => false,
                }
            },
        ));
        answer
    }
"#;
        assert!(out.text.contains(expected), "{}", out.text);
        assert_eq!(out.emitted, ["pick"]);
        assert_eq!(out.replace, 1);
        assert_eq!(out.decisions, ["TriState"]);
        syn::parse_file(&out.text).expect("the golden text parses");
    }

    #[test]
    fn a_replace_answer_without_a_from_impl_skips_its_hook() {
        let ir = ir(vec![hook("pick", Kind::Replace, vec![])]);
        let dispatch = DispatchSigs {
            hooks: [("pick".to_owned(), Some(Vec::new()))].into(),
            structs: BTreeMap::new(),
        };
        let out = emit(
            &ir,
            &[replace_sig()],
            &dispatch,
            &["pick"],
            &Plain::from_paths(["TriState"]),
        );
        assert!(out.emitted.is_empty());
        assert_eq!(out.skipped[0].1, "answer: TriState");
        let not_an_option = sig("pick", None, "event", "PickEvent<'_>", Some("TriState"));
        let out = emit(
            &ir,
            &[not_an_option],
            &dispatch,
            &["pick"],
            &Plain::deciding(["TriState"], ["TriState"]),
        );
        assert_eq!(
            out.skipped[0].1,
            "replace hook does not return an Option: `TriState`"
        );
    }

    #[test]
    fn a_collect_hook_with_a_list_output_assigns_the_list_and_reads_it_back() {
        let mut collects = hook("lines_hook", Kind::Collect, vec![arg("x", "i32")]);
        collects.output = Some(arg("lines", "&'a mut Vec<String>"));
        let lines_ir = ir(vec![collects]);
        let dispatch = DispatchSigs {
            hooks: [(
                "lines_hook".to_owned(),
                Some(vec![OutputSig {
                    copies_cancel: false,
                    param: "lines".to_owned(),
                    ty: "Vec<String>".to_owned(),
                }]),
            )]
            .into(),
            structs: [(
                "LinesHookEvent".to_owned(),
                [("lines".to_owned(), "Vec<String>".to_owned())].into(),
            )]
            .into(),
        };
        let sigs = [sig("lines_hook", None, "event", "LinesHookEvent<'_>", None)];
        let out = emit(
            &lines_ir,
            &sigs,
            &dispatch,
            &["lines_hook"],
            &Plain::default(),
        );
        for piece in [
            "let out_lines = std::cell::RefCell::new(event.lines.clone());\n",
            "data.raw_set(\"lines\", out_lines.borrow().to_lua_value(lua)?)?;\n",
            "crate::dispatch::accept(&out_lines, id, \"lines_hook\", \"lines\", &answered.answer(\"lines\"));\n",
            "event.lines = out_lines.into_inner();\n",
        ] {
            assert!(
                out.text.contains(piece),
                "missing {piece:?} in:\n{}",
                out.text
            );
        }
        assert_eq!(out.emitted, ["lines_hook"]);
        assert_eq!(out.lists, 1);
        syn::parse_file(&out.text).expect("the text parses");

        let mut other = hook("tuples_hook", Kind::Collect, vec![]);
        other.output = Some(arg("pairs", "&'a mut Vec<(Text,u32)>"));
        let out = emit(
            &ir(vec![other]),
            &[sig(
                "tuples_hook",
                None,
                "event",
                "TuplesHookEvent<'_>",
                None,
            )],
            &DispatchSigs {
                hooks: [("tuples_hook".to_owned(), Some(Vec::new()))].into(),
                structs: BTreeMap::new(),
            },
            &["tuples_hook"],
            &Plain::default(),
        );
        assert_eq!(
            out.skipped[0].1,
            "has an output: its listeners write back, not in this slice"
        );
    }

    #[test]
    fn each_host_kind_opens_its_turn_with_its_own_host_and_a_hook_without_one_with_none() {
        // One listen hook per host parameter kind the turn carries, and one for a level host, which
        // the turn does not carry (its handles raise `no host call outside a hook`).
        let kinds = [
            (
                "server_hook",
                "&mut dyn ServerHost",
                "crate::hostcall::TurnHost::Server(host)",
            ),
            (
                "client_hook",
                "&mut dyn ClientHost",
                "crate::hostcall::TurnHost::Client(host)",
            ),
            (
                "client_read_hook",
                "&dyn ClientHost",
                "crate::hostcall::TurnHost::ClientRead(host)",
            ),
            (
                "read_hook",
                "&dyn Host",
                "crate::hostcall::TurnHost::Read(host)",
            ),
            (
                "side_hook",
                "HostSide",
                "crate::hostcall::TurnHost::of_side(host)",
            ),
            (
                "level_hook",
                "&dyn LevelHost",
                "crate::hostcall::TurnHost::None",
            ),
        ];
        let ir = ir(kinds
            .iter()
            .map(|(name, _, _)| hook(name, Kind::Listen, vec![arg("x", "i32")]))
            .collect());
        let sigs: Vec<HookSig> = kinds
            .iter()
            .map(|(name, ty, _)| sig(name, Some(("host", ty)), "event_", "Ev<'_>", None))
            .collect();
        let names: Vec<&str> = kinds.iter().map(|(name, _, _)| *name).collect();
        let out = emit_plain(&ir, &sigs, &names, &Plain::default());
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert_eq!(out.emitted.len(), kinds.len());
        for (name, _, variant) in kinds {
            let start = out
                .text
                .find(&format!("    fn {name}("))
                .unwrap_or_else(|| panic!("{name} is emitted:\n{}", out.text));
            let body = &out.text[start..];
            let body = &body[..body.find("\n    }\n").expect("the method ends")];
            assert!(
                body.contains(&format!("            {variant},\n")),
                "{name} opens its turn with {variant}:\n{body}"
            );
            assert_eq!(
                body.matches("crate::hostcall::TurnHost::").count(),
                1,
                "{name} names one turn host:\n{body}"
            );
            assert!(
                body.contains("crate::hostcall::refused(self.dispatch_turns("),
                "{name}"
            );
        }
    }
    /// The handle families a test generates, for the handle inputs.
    const HANDLE_FAMILIES: &[&str] = &["EntityRef", "PlayerRef", "LivingRef"];

    /// A server hook `strike` that carries an `EntityRef` and an `Option<PlayerRef>` to its event.
    fn strike_ir() -> Ir {
        ir(vec![hook(
            "strike",
            Kind::Listen,
            vec![
                arg("entity", "EntityRef"),
                arg("placer", "Option<PlayerRef>"),
            ],
        )])
    }

    fn strike_sig() -> HookSig {
        sig(
            "strike",
            Some(("host", "&mut dyn ServerHost")),
            "event",
            "StrikeEvent",
            None,
        )
    }

    fn emit_handles(ir: &Ir, sigs: &[HookSig], hooks: &[&str]) -> Emitted {
        let dispatch = DispatchSigs {
            hooks: ir
                .hooks
                .iter()
                .map(|hook| (hook.name.clone(), Some(Vec::new())))
                .collect(),
            structs: BTreeMap::new(),
        };
        emit_with_handles(
            ir,
            sigs,
            &dispatch,
            hooks,
            &Plain::default(),
            HANDLE_FAMILIES,
        )
    }

    #[test]
    fn a_hook_with_handle_inputs_carries_their_userdata_to_the_event() {
        let out = emit_handles(&strike_ir(), &[strike_sig()], &["strike"]);
        assert_eq!(out.emitted, ["strike"], "{:?}", out.skipped);
        let expected = r#"    fn strike(&self, host: &mut dyn ServerHost, event: &mut StrikeEvent) {
        let _ = &host;
        crate::hostcall::refused(self.dispatch_turns(
            "strike",
            crate::hostcall::TurnHost::Server(host),
            &|| false,
            &|lua, _turn| {
                let data = lua.create_table()?;
                data.raw_set("entity", crate::handles::family_out(lua, event.entity)?)?;
                data.raw_set("placer", crate::handles::family_opt_out(lua, event.placer)?)?;
                crate::dispatch::new_event(lua, data, &[])
            },
            &mut |_, _, _| true,
        ));
    }
"#;
        assert!(out.text.contains(expected), "{}", out.text);
    }

    #[test]
    fn handle_shapes_are_read_by_their_spelling() {
        assert_eq!(
            handle_parts("EntityRef"),
            Some((HandleIn::Plain, "EntityRef"))
        );
        assert_eq!(
            handle_parts("&EntityRef"),
            Some((HandleIn::Ref, "EntityRef"))
        );
        assert_eq!(
            handle_parts("Option<EntityRef>"),
            Some((HandleIn::Opt, "EntityRef"))
        );
        assert_eq!(
            handle_parts("Option<&EntityRef>"),
            Some((HandleIn::OptRef, "EntityRef"))
        );
        assert_eq!(
            handle_parts("&Option<EntityRef>"),
            Some((HandleIn::RefOpt, "EntityRef"))
        );
        assert_eq!(
            handle_parts("Vec<EntityRef>"),
            Some((HandleIn::List, "EntityRef"))
        );
        assert_eq!(handle_parts("Option<Option<EntityRef>>"), None);
        assert_eq!(handle_parts("&Vec<EntityRef>"), None);
        assert!(is_generated_handle(
            "&'a mut Option<LivingRef>",
            HANDLE_FAMILIES
        ));
        assert!(!is_generated_handle("&'a ChannelRef", HANDLE_FAMILIES));
        assert!(mentions_handle("Option<EntityRef>") && !mentions_handle("EntityRefs"));
    }

    #[test]
    fn a_mutable_handle_is_a_handle_output_and_a_handle_answer_too() {
        let hook_with_output = HookIr {
            output: Some(ArgIr {
                name: "entities".to_owned(),
                rust_type: "&'a mut Vec<EntityRef>".to_owned(),
            }),
            ..hook("explode", Kind::Collect, vec![arg("level", "u32")])
        };
        let explode = ir(vec![hook_with_output]);
        let explode_sig = sig("explode", None, "event", "ExplodeEvent", None);
        let out = emit_handles(&explode, &[explode_sig], &["explode"]);
        assert_eq!(
            out.skipped,
            [("explode".to_owned(), HANDLE_OUTPUT.to_owned())]
        );

        let replace = hook(
            "pick",
            Kind::Replace,
            vec![arg("player", "Option<PlayerRef>")],
        );
        let pick_sig = sig(
            "pick",
            Some(("host", "&dyn Host")),
            "event",
            "PickEvent",
            Some("Option<PlayerRef>"),
        );
        let out = emit_handles(&ir(vec![replace]), &[pick_sig], &["pick"]);
        assert_eq!(out.skipped, [("pick".to_owned(), HANDLE_OUTPUT.to_owned())]);
    }

    #[test]
    fn a_handle_input_with_no_host_to_read_it_is_skipped_with_its_reason() {
        let sig = sig("strike", None, "event", "StrikeEvent", None);
        let out = emit_handles(&strike_ir(), &[sig], &["strike"]);
        assert_eq!(
            out.skipped,
            [(
                "strike".to_owned(),
                "handle input: the hook has no host".to_owned()
            )]
        );
    }
    #[test]
    fn each_handle_shape_reads_its_event_field_by_the_shape() {
        let expr = |ty: &str| handle_input("hook_x", "h", ty, "event.h");
        assert_eq!(
            expr("EntityRef"),
            Ok("crate::handles::family_out(lua, event.h)?".to_owned())
        );
        assert_eq!(
            expr("&'a EntityRef"),
            Ok("crate::handles::family_out(lua, *event.h)?".to_owned())
        );
        assert_eq!(
            expr("Option<&'a EntityRef>"),
            Ok("crate::handles::family_opt_out(lua, event.h.copied())?".to_owned())
        );
        assert_eq!(
            expr("&'a Option<LivingRef>"),
            Ok("crate::handles::family_opt_out(lua, *event.h)?".to_owned())
        );
        assert_eq!(
            expr("Vec<PlayerRef>"),
            Ok("crate::handles::family_capped_out(lua, \"hook_x\", &event.h)?".to_owned())
        );
        assert_eq!(
            expr("&'a mut Option<LivingRef>"),
            Err(HANDLE_OUTPUT.to_owned())
        );
        assert!(handle_parts("Option<Option<EntityRef>>").is_none());
    }
}
