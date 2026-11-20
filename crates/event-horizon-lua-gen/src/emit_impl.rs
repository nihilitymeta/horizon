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
use crate::ir::{ArgIr, HookIr, Ir, Kind, Side};
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
    (
        "register_client_commands",
        "RegisterClientCommandsEvent",
        "commands",
        "declared_client_commands",
    ),
    (
        "register_configs",
        "RegisterConfigsEvent",
        "configs",
        "declared_configs",
    ),
    (
        "register_commands",
        "RegisterCommandsEvent",
        "commands",
        "declared_commands",
    ),
    (
        "data_map_types",
        "DataMapTypesEvent",
        "types",
        "declared_data_map_types",
    ),
    (
        "data_pack_files",
        "DataPackFilesEvent",
        "files",
        "declared_data_pack_files",
    ),
];

/// Fields a hook's event carries that no argument of the hook supplies: `(hook, field, expr)`. The
/// expression is Rust, built in the hook's `build` closure, where `lua` is in scope. `feedback` is
/// the function a `run_mod_command` handler calls to answer its sender (`crate::commands`).
pub const EXTRA_EVENT_FIELDS: &[(&str, &str, &str)] = &[(
    "run_mod_command",
    "feedback",
    "crate::commands::feedback_lua(lua)?",
)];

/// The hooks whose event reaches only the mods a predicate names: `(hook, event field, predicate)`.
/// The turn of a mod the predicate refuses does not open, so it gets no event and logs nothing
/// (`LuaHost::dispatch_turns_scoped`). The predicate is `fn(&LuaMod, &field) -> bool` in the host crate:
/// `run_mod_command` goes only to the mod that declared the command's name (`commands::delivers_to`).
pub const SCOPED_HOOKS: &[(&str, &str, &str)] =
    &[("run_mod_command", "command", "crate::commands::delivers_to")];

/// The hooks whose event carries one mod's own file: `(hook, event field)`, the field a `ConfigFile`.
/// Their handlers run only in the mod named by that file's `mod_id`, so no mod reads another's config.
pub const OWNED_HOOKS: &[(&str, &str)] = &[
    ("config_loading", "config"),
    ("config_reloading", "config"),
    ("config_unloading", "config"),
];

/// The method of a host-recorded hook: it passes the event's output to the host method. The
/// method takes the hook's host parameter as the trait declares it (`context`, or `host`), if any.
fn host_recorded_text(
    hook: &str,
    event: &str,
    field: &str,
    host_method: &str,
    sig: &HookSig,
) -> String {
    let (host, unused) = match (&sig.host_name, &sig.host_ty) {
        (Some(name), Some(ty)) => (
            format!("{name}: {ty}, "),
            format!("let _ = {name};\n        "),
        ),
        _ => (String::new(), String::new()),
    };
    format!(
        "
    fn {hook}(&self, {host}event: &mut mod_api::{event}<'_>) {{
        {unused}self.{host_method}(event.{field});
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
    /// The event struct's field the output is read from and written to. It is `key` except for a
    /// cancel flag, whose field is always `cancelled` (see [`cancel_key`]).
    field: String,
    ty: String,
    /// The event field is a `&mut` reference (`pub lines: &'a mut Vec<String>`), read and written
    /// through it.
    by_ref: bool,
    /// The carried shape (a stack or a block state, see [`carried`]), which converts through the host.
    shape: Option<Shape>,
}

impl Method<'_> {
    /// The shape's view: a stack's own view, else the state view (the host's, or the level's), inside
    /// the dispatch closures when `in_turn`, else outside them. `None` when the shape has no view.
    fn view_for(&self, shape: Shape, in_turn: bool) -> Option<(Shape, &str)> {
        let view = match (carried::is_stack(shape), in_turn) {
            (true, true) => self.stack_turn_view.as_deref(),
            (true, false) => self.stack_view.as_deref(),
            (false, true) => self.turn_view.as_deref(),
            (false, false) => self.view.as_deref(),
        };
        view.map(|view| (shape, view))
    }
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
    /// The view a stack converts through outside the closures: the host for a `Host` hook, else the
    /// global tables (see [`carried::stack_reach`]). Stacks use it where the state views would not do.
    stack_view: Option<String>,
    /// [`Method::stack_view`] as the closures of the dispatch take it (the turn's host for a `Host`).
    stack_turn_view: Option<String>,
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
            match sigs.iter().find(|sig| sig.name == name) {
                Some(sig) if ir.hooks.iter().any(|h| h.name == name) => {
                    host_recorded.push((*hook).to_owned());
                    recorded_text.push_str(&host_recorded_text(
                        hook,
                        event,
                        field,
                        host_method,
                        sig,
                    ));
                }
                _ => skipped.push((
                    name.to_owned(),
                    "not a method of the Hooks trait".to_owned(),
                )),
            }
            continue;
        }
        if HOT_HOOKS.contains(&name) {
            skipped.push((name.to_owned(), HOT_REASON.to_owned()));
            continue;
        }
        match plan(ir, sigs, dispatch, name, plain, handles, false) {
            Ok(method) => methods.push(method),
            Err(reason) => skipped.push((name.to_owned(), reason)),
        }
    }

    let mut text = format!(
        "// @generated by event-horizon-lua-gen from surface {}: do not edit.\n",
        ir.document_hash
    );
    text.push_str(&imports(&methods, "use crate::host::LuaHost;\n"));
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

/// What a Lua script sees of one emitted hook or Part method: the event's fields (read-only) and its
/// outputs (writable keys, the answers), each with its Rust type, and a replace hook's answer type.
/// The Luau type definitions are written from these ([`crate::emit_luau`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventShape {
    pub hook: String,
    /// `(key, Rust type)` of each event field, in the order the method writes them.
    pub fields: Vec<(String, String)>,
    /// `(key, Rust type)` of each writable output. A cancel hook's flag is keyed `cancelled`.
    pub outputs: Vec<(String, String)>,
    /// The answer type `X` of a replace hook (its method returns `Option<X>`).
    pub answer: Option<String>,
}

/// The event shapes of the hooks in `names` that the same plan emits, in name order: the Hooks
/// trait's methods when `part` is false, a Part trait's methods when it is true (`sigs` is that
/// trait's). A name the plan refuses is left out; [`emit_with_handles`] and [`emit_parts`] say why.
pub fn shapes(
    ir: &Ir,
    sigs: &[HookSig],
    dispatch: &DispatchSigs,
    names: &[&str],
    plain: &Plain,
    handles: &[&str],
    part: bool,
) -> Vec<EventShape> {
    let mut names = names.to_vec();
    names.sort_unstable();
    names.dedup();
    let mut shapes = Vec::new();
    for name in names {
        let Ok(method) = plan(ir, sigs, dispatch, name, plain, handles, part) else {
            continue;
        };
        let rust_type = |key: &str| -> String {
            if key == "feedback" {
                return "@feedback".to_owned();
            }
            method
                .hook
                .arguments
                .iter()
                .find(|arg| arg.name == key)
                .map(|arg| arg.rust_type.clone())
                .unwrap_or_default()
        };
        let fields = method
            .fields
            .iter()
            .map(|(key, _)| (key.clone(), rust_type(key)))
            .collect();
        let outputs = method
            .outputs
            .iter()
            .map(|output| (output.key.clone(), output.ty.clone()))
            .collect();
        shapes.push(EventShape {
            hook: name.to_owned(),
            fields,
            outputs,
            answer: method
                .answer
                .as_deref()
                .map(|answer| answer.replace("mod_api::", "")),
        });
    }
    shapes
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
    SCALAR_NAMES.contains(&ty) || ty == "Text" || plain.decides(ty)
}

/// The scalar types a value converts as: primitives and `String`, spelled without a path.
const SCALAR_NAMES: [&str; 6] = ["bool", "i32", "u32", "f32", "f64", "String"];

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
    // A scalar is the std or primitive type, not a mod API one.
    if SCALAR_NAMES.contains(&inner) {
        return Ok(inner.to_owned());
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

/// The Lua key a cancel hook's flag is read and written under: `ran` for `run_mod_command` (its
/// dispatch names the flag `ran`), `cancelled` for the other cancel hooks. The event struct's field
/// is `cancelled` for all of them.
pub fn cancel_key(hook: &str) -> &'static str {
    match hook {
        "run_mod_command" => "ran",
        _ => "cancelled",
    }
}

/// The `Vec<Text>` outputs a cancel hook appends to rather than replaces: `(hook, key, type)`. Each
/// mod's turn starts with an empty list and answers the lines it adds; the game appends them to the
/// hook's list in dispatch order (`accept_lines` in `dispatch`), so a mod never sees another's lines.
pub const APPENDED_OUTPUTS: &[(&str, &str, &str)] = &[("player_command", "replies", "Vec<Text>")];

/// Whether the output `key` of `hook`, of type `ty` (without spaces), is one of [`APPENDED_OUTPUTS`].
fn is_appended(hook: &str, key: &str, ty: &str) -> bool {
    APPENDED_OUTPUTS
        .iter()
        .any(|(h, k, t)| *h == hook && *k == key && *t == ty)
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
    part: bool,
) -> Result<Vec<Output>, String> {
    let all = match dispatch.hooks.get(&hook.name) {
        None => return Err("no dispatch function for it".to_owned()),
        Some(None) => return Err("two dispatch functions name it".to_owned()),
        Some(Some(params)) => params,
    };
    // A Part replace hook's dispatch function ends with its answer slot (`survives: &mut Option<bool>`,
    // which `*survives = Some(..)` fills): that slot is the answer of the method, not an output. The
    // Hooks trait keeps its replace hooks out (their answer slot is not read), as before.
    let params = if part && hook.kind == Kind::Replace {
        &all[..all.len().saturating_sub(1)]
    } else {
        &all[..]
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
            None if !output_supported(&shown(&param.ty), plain)
                && !is_appended(&hook.name, &param.param, &shown(&param.ty)) =>
            {
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
        let flag = cancel && is_cancel_flag(param);
        let key = if flag {
            cancel_key(&hook.name).to_owned()
        } else {
            param.param.clone()
        };
        let field_name = if flag {
            "cancelled".to_owned()
        } else {
            key.clone()
        };
        let field = fields
            .and_then(|fields| fields.get(&field_name))
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
            field: field_name,
            ty: shown(&param.ty),
            by_ref,
            shape: carried::shape_of(&param.ty),
        });
    }
    if cancel && !outputs.iter().any(|output| output.field == "cancelled") {
        if fields
            .and_then(|fields| fields.get("cancelled"))
            .map(String::as_str)
            != Some("bool")
        {
            return Err(format!("`{event}` has no `cancelled: bool` field"));
        }
        outputs.push(Output {
            key: cancel_key(&hook.name).to_owned(),
            field: "cancelled".to_owned(),
            ty: "bool".to_owned(),
            by_ref: false,
            shape: None,
        });
    }
    Ok(outputs)
}

/// Decides whether the hook `name` can be emitted, and what its method contains. `part` is true for
/// a method of a Part trait (`sigs` is that trait's): such a hook is fired through its Part, so a
/// scoped hook is expected; for the `Hooks` trait it is refused.
fn plan<'a>(
    ir: &'a Ir,
    sigs: &'a [HookSig],
    dispatch: &DispatchSigs,
    name: &str,
    plain: &Plain,
    handles: &[&str],
    part: bool,
) -> Result<Method<'a>, String> {
    let Some(sig) = sigs.iter().find(|sig| sig.name == name) else {
        return Err("not a method of the Hooks trait".to_owned());
    };
    let Some(hook) = ir.hooks.iter().find(|hook| hook.name == name) else {
        return Err("not in the surface".to_owned());
    };
    if hook.scope.is_some() && !part {
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
    let stack_view = carried::view(
        sig.host_name.as_deref(),
        sig.host_ty.as_deref(),
        carried::stack_reach(reach),
    );
    let stack_turn_view = if reach == Reach::Host {
        turn_view.clone()
    } else {
        stack_view.clone()
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

    let outputs = outputs_of(hook, sig, cancel, dispatch, plain, reach, part)?;
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
            let (shape_view, shape_turn) = if carried::is_stack(shape) {
                (stack_view.as_deref(), stack_turn_view.as_deref())
            } else {
                (view.as_deref(), turn_view.as_deref())
            };
            let (Some(view), true) = (shape_view, carried::allowed(shape, reach)) else {
                return Err(carried::refusal(shape));
            };
            let field = field_of(sig, &arg.name);
            checks.push(carried::input_check(shape, &arg.rust_type, &field, view));
            let built = shape_turn.unwrap_or(view);
            fields.push((
                arg.name.clone(),
                carried::input_lua(shape, &arg.rust_type, &field, built),
            ));
            continue;
        }
        if let Some(kind) = command_input(&arg.rust_type) {
            // A command's source or invocation converts read-only (`crate::commands`); a `&mut` one
            // would write back, which this slice does not do.
            if arg.rust_type.contains("mut") {
                return Err(format!(
                    "argument `{}`: a mutable argument writes back, not in this slice",
                    arg.name
                ));
            }
            let field = field_of(sig, &arg.name);
            let built = match (turn.is_some(), turn_view.as_deref()) {
                (true, Some(built)) => built,
                _ => {
                    return Err(format!(
                        "argument `{}`: the hook has no turn host",
                        arg.name
                    ));
                }
            };
            let expr = format!("crate::commands::{kind}_lua(lua, {built}, {field})?");
            fields.push((arg.name.clone(), expr));
            continue;
        }
        if is_generated_handle(&arg.rust_type, handles) {
            let field = field_of(sig, &arg.name);
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
        if is_block_click(&arg.rust_type) {
            // A right click is a borrowed view whose held stack is carried through the host.
            let (Some(view), true) = (
                stack_view.as_deref(),
                carried::allowed(Shape::OptStack, reach),
            ) else {
                return Err(carried::refusal(Shape::OptStack));
            };
            let field = field_of(sig, &arg.name);
            let stack_field = format!("{field}.stack");
            checks.push(carried::input_check(
                Shape::OptStack,
                "Option<&ItemStack>",
                &stack_field,
                view,
            ));
            let built = stack_turn_view.as_deref().unwrap_or(view);
            let stack =
                carried::input_lua(Shape::OptStack, "Option<&ItemStack>", &stack_field, built);
            // The event field is the click itself when its type is a reference, else a value.
            let click = if arg.rust_type.trim_start().starts_with('&') {
                field.clone()
            } else {
                format!("&{field}")
            };
            fields.push((
                arg.name.clone(),
                format!("crate::hostcall::block_click_lua(lua, {click}, {stack})?"),
            ));
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
        match to_lua_expr(&field_of(sig, &arg.name), &mapped, plain) {
            Some(expr) => fields.push((arg.name.clone(), expr)),
            None => return Err(format!("{shown}: no Lua conversion")),
        }
    }

    for (hook_name, field, expr) in EXTRA_EVENT_FIELDS {
        if *hook_name == hook.name {
            fields.push(((*field).to_owned(), (*expr).to_owned()));
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
        stack_view,
        stack_turn_view,
        checks,
    })
}

/// The kind of a command argument (`source` or `invocation`), when its type is one of the command
/// types `crate::commands` converts: `&'a command::CommandSource`, `&'a CommandInvocation`.
pub(crate) fn command_input(ty: &str) -> Option<&'static str> {
    match compact(ty).trim_start_matches('&') {
        "command::CommandSource" | "CommandSource" => Some("source"),
        "command::CommandInvocation" | "CommandInvocation" => Some("invocation"),
        _ => None,
    }
}

/// Whether an argument type (`&'a BlockClick<'a>`) is the right click view, read by value.
pub(crate) fn is_block_click(ty: &str) -> bool {
    let text: String = ty.chars().filter(|ch| !ch.is_whitespace()).collect();
    let text = text.replace("'a", "");
    !text.contains("mut") && text.trim_start_matches('&').starts_with("BlockClick<")
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

/// The expression that reads argument `name` of `sig`: its event's field, or the parameter itself for a
/// plain method (the game passes the parameters, there is no event struct).
fn field_of(sig: &HookSig, name: &str) -> String {
    match sig.plain {
        Some(_) => name.to_owned(),
        None => format!("{}.{name}", sig.event_name),
    }
}

/// The Lua value of the field `field_expr` (such as `event.pos`) as an `mlua::Value`, or `None`
/// when the type has no conversion. Every conversion used here is an impl of `ToLuaValue`.
fn to_lua_expr(field_expr: &str, mapped: &Mapped, plain: &Plain) -> Option<String> {
    match &mapped.ty {
        TypeIr::Str => Some(format!("{field_expr}.to_string().to_lua_value(lua)?")),
        TypeIr::Option(inner) if matches!(**inner, TypeIr::Str) => Some(format!(
            "{field_expr}.as_ref().map(|value| value.to_string()).to_lua_value(lua)?"
        )),
        // A list of numbers or booleans is a Lua array table: `[T]` converts (a borrowed slice too).
        TypeIr::List(elem) if scalar_name(elem).is_some() => {
            Some(format!("{field_expr}.to_lua_value(lua)?"))
        }
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
/// has one in `scalar.rs`; a block entity type key has one in `ids.rs` (its id string);
/// `char`, `Path` and `PathBuf` have one in `generated_types` (the std impls).
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
                    | "BlockEntityTypeKey"
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

/// The `use` lines the file needs: the host's (`host_use`), the convert trait if a value is converted,
/// the mod API, and every type the signatures name from the mod API.
fn imports(methods: &[Method<'_>], host_use: &str) -> String {
    let mut out = String::from(host_use);
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
        // A plain method's arguments are its parameters: their types are named too.
        for (_, ty) in method.sig.plain.iter().flatten() {
            names.extend(type_names(ty));
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
            let value = match output.shape.and_then(|shape| method.view_for(shape, true)) {
                _ if is_appended(&method.hook.name, &output.key, &output.ty) => {
                    "lua.create_table()?".to_owned()
                }
                Some((shape, view)) => carried::output_lua(shape, &output.key, view),
                None => format!("out_{}.borrow().to_lua_value(lua)?", output.key),
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

/// The dispatch method of a hook that has no owner, and the argument that scopes it. A hook in
/// `SCOPED_HOOKS` goes to `dispatch_turns_scoped` with its predicate on the event's field; any other
/// hook goes to `dispatch_turns` with no argument.
fn dispatch_scope(method: &Method<'_>) -> (&'static str, String) {
    match SCOPED_HOOKS
        .iter()
        .find(|(hook, _, _)| *hook == method.hook.name)
    {
        Some((_, field, predicate)) => (
            "dispatch_turns_scoped",
            format!(
                "            &|m| {predicate}(m, {}.{field}),\n",
                method.sig.event_name
            ),
        ),
        None => ("dispatch_turns", String::new()),
    }
}

/// The body of a hook with no outputs: its handlers run in the turn, and nothing is committed.
fn plain_body(method: &Method<'_>) -> String {
    // A hook whose event names one mod's own data goes to that mod only (`OWNED_HOOKS`).
    let owned = OWNED_HOOKS
        .iter()
        .find(|(hook, _)| *hook == method.hook.name);
    let (dispatch, owner) = match owned {
        Some((_, field)) => (
            "dispatch_turns_owned",
            format!("            &event.{field}.mod_id,\n"),
        ),
        None => dispatch_scope(method),
    };
    let mut out = format!("        crate::hostcall::refused(self.{dispatch}(\n");
    out.push_str(&format!("            {:?},\n", method.hook.name));
    out.push_str(&owner);
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
    let appended = method
        .outputs
        .iter()
        .any(|output| is_appended(&method.hook.name, &output.key, &output.ty));
    if outputs && appended && calls.is_empty() {
        // Nothing to convert, but a cancellable hook with no handler should cost no turn.
        let ret = if method.cancel {
            format!(
                " if {}.cancelled {{ Flow::Cancel }} else {{ Flow::Continue }}",
                method.sig.event_name
            )
        } else {
            String::new()
        };
        return format!(
            "        if !self.has_handlers({:?}) {{\n            return{ret};\n        }}\n",
            method.hook.name
        );
    }
    if outputs {
        for output in &method.outputs {
            if let Some((shape, view)) =
                output.shape.and_then(|shape| method.view_for(shape, false))
            {
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
    match carried::shape_of(answer).and_then(|shape| method.view_for(shape, true)) {
        Some((shape, view)) => body.push_str(&format!(
            "                match crate::dispatch::answer_by(id, {hook:?}, &value, {}) {{\n",
            carried::converter(shape, view)
        )),
        None => body.push_str(&format!(
            "                match crate::dispatch::answer_of::<{answer}>(id, {hook:?}, &value) {{\n"
        )),
    }
    // A ranged answer outside its range is refused, as an answer of no type: the next part answers.
    let (guard, other) = match answer_range(hook) {
        Some(range) => (format!(" if ({range}).contains(&found)"), "_"),
        None => (String::new(), "None"),
    };
    body.push_str(&format!("                    Some(found){guard} => {{\n"));
    body.push_str("                        answer = Some(found);\n");
    body.push_str("                        true\n");
    body.push_str("                    }\n");
    body.push_str(&format!("                    {other} => false,\n"));
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
            "        let out_{key} = std::cell::RefCell::new({event}.{field}{copy});\n",
            key = output.key,
            field = output.field
        ));
    }
    out.push_str(&checks_text(method, true));
    let is_cancelled = if method.cancel {
        let key = method
            .outputs
            .iter()
            .find(|output| output.field == "cancelled")
            .map_or("cancelled", |output| output.key.as_str());
        format!("&|| *out_{key}.borrow()")
    } else {
        "&|| false".to_owned()
    };
    let (dispatch, scope) = dispatch_scope(method);
    out.push_str(&format!(
        "        crate::hostcall::refused(self.{dispatch}(\n"
    ));
    out.push_str(&format!("            {hook:?},\n"));
    out.push_str(&scope);
    out.push_str(&format!("            {},\n", turn_text(method)));
    out.push_str(&format!("            {is_cancelled},\n"));
    out.push_str(&build_text(method, true));
    out.push_str(&commit_text(method));
    out.push_str("        ));\n");
    for output in &method.outputs {
        let deref = if output.by_ref { "*" } else { "" };
        out.push_str(&format!(
            "        {deref}{event}.{field} = out_{key}.into_inner();\n",
            key = output.key,
            field = output.field
        ));
    }
    if method.cancel {
        out.push_str(&format!(
            "        if {event}.cancelled {{ Flow::Cancel }} else {{ Flow::Continue }}\n"
        ));
    }
    out
}

/// The integer outputs a mod's answer must keep in a range, as `(hook, output, range)`: the range is
/// the Rust text of an inclusive range, the one the game holds the value to (`WorldFluid::check` for
/// the world fluid's numbers, and the game's clamp of `fluid_rules`' answers, which the Lua side
/// refuses instead: an answer outside it keeps the previous value and logs one warning).
pub const OUTPUT_RANGES: &[(&str, &str, &str)] = &[
    ("fluid_rules", "tick_delay", "1..=i32::MAX"),
    ("fluid_rules", "drop_off", "1..=7"),
    ("fluid_rules", "slope_find_distance", "1..=4"),
];

/// The range an output of `hook` must stay in, if [`OUTPUT_RANGES`] names one.
fn output_range(hook: &str, key: &str) -> Option<&'static str> {
    OUTPUT_RANGES
        .iter()
        .find(|(name, output, _)| *name == hook && *output == key)
        .map(|(_, _, range)| *range)
}

/// The `commit` closure of a hook with outputs: each output the mod wrote is accepted if it
/// converts, and the chain goes on.
fn commit_text(method: &Method<'_>) -> String {
    let hook = &method.hook.name;
    let mut body = String::new();
    // A refused line list refuses the whole answer: the cancel flag is then not taken either.
    let appended = method
        .outputs
        .iter()
        .any(|output| is_appended(hook, &output.key, &output.ty));
    if appended {
        body.push_str("                let mut refused = false;\n");
    }
    for output in &method.outputs {
        if is_appended(hook, &output.key, &output.ty) {
            body.push_str(&format!(
                "                refused |= crate::dispatch::accept_lines(&out_{key}, id, {hook:?}, {key:?}, &answered.answer({key:?}));\n",
                key = output.key
            ));
            continue;
        }
        let guard = appended && output.field == "cancelled";
        if guard {
            body.push_str("                if !refused {\n    ");
        }
        // An `Option` output takes `horizon.none` as None; any other output refuses it.
        let optional = output.ty.starts_with("Option<");
        match output.shape.and_then(|shape| method.view_for(shape, true)) {
            Some((shape, view)) => body.push_str(&format!(
                "                crate::dispatch::{accept}(&out_{key}, id, {hook:?}, {key:?}, &answered.answer({key:?}), {convert});\n",
                accept = if optional { "accept_opt_by" } else { "accept_by" },
                key = output.key,
                convert = carried::converter(shape, view)
            )),
            _ => match output_range(hook, &output.key) {
                Some(range) => body.push_str(&format!(
                    "                crate::dispatch::accept_in(&out_{key}, id, {hook:?}, {key:?}, &answered.answer({key:?}), {range});\n",
                    key = output.key
                )),
                None => body.push_str(&format!(
                    "                crate::dispatch::{accept}(&out_{key}, id, {hook:?}, {key:?}, &answered.answer({key:?}));\n",
                    accept = if optional { "accept_opt" } else { "accept" },
                    key = output.key
                )),
            },
        }
        if guard {
            body.push_str("                }\n");
        }
    }
    body.push_str("                true\n");
    format!(
        "            &mut |id, answered, {}| {{\n{body}            }},\n",
        turn_param(&body)
    )
}

/// The Part traits the Lua host binds one adapter for: the BlockPart, the ItemPart, the
/// BlockEntityPart, the FluidPart and the EntityPart methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartTrait {
    Block,
    Item,
    BlockEntity,
    Fluid,
    Entity,
}

impl PartTrait {
    /// The trait's name, as the Rust source spells it.
    pub fn name(self) -> &'static str {
        match self {
            PartTrait::Block => "BlockPart",
            PartTrait::Item => "ItemPart",
            PartTrait::BlockEntity => "BlockEntityPart",
            PartTrait::Fluid => "FluidPart",
            PartTrait::Entity => "EntityPart",
        }
    }

    /// The adapter struct the generated methods are written on (in `host.rs`).
    fn adapter(self) -> &'static str {
        match self {
            PartTrait::Block => "LuaBlockPart",
            PartTrait::Item => "LuaItemPart",
            PartTrait::BlockEntity => "LuaBlockEntityPart",
            PartTrait::Fluid => "LuaFluidPart",
            PartTrait::Entity => "LuaEntityPart",
        }
    }

    /// The trait method that returns the part's subscription.
    fn subscribed(self) -> &'static str {
        match self {
            PartTrait::Block => "block_subscribed",
            PartTrait::Item => "item_subscribed",
            PartTrait::BlockEntity => "block_entity_subscribed",
            PartTrait::Fluid => "fluid_subscribed",
            PartTrait::Entity => "entity_subscribed",
        }
    }

    /// The generated table of the trait's handler names.
    fn table(self) -> &'static str {
        match self {
            PartTrait::Block => "BLOCK_PARTS",
            PartTrait::Item => "ITEM_PARTS",
            PartTrait::BlockEntity => "BLOCK_ENTITY_PARTS",
            PartTrait::Fluid => "FLUID_PARTS",
            PartTrait::Entity => "ENTITY_PARTS",
        }
    }
}

/// The Part methods the Lua host does not bind, as `(trait, method)`, with the reason. A hot method
/// runs per tick or per frame and is not bound until it is measured; the worldgen context is not
/// bound at all (see [`emit_parts`]).
pub const PART_HOT: &[(&str, &str)] = &[
    ("BlockPart", "block_render_visibility"),
    ("BlockPart", "can_entity_destroy"),
    ("BlockPart", "enchant_power_bonus"),
    ("BlockPart", "falling_tick"),
    ("BlockPart", "is_fire_source"),
    ("BlockPart", "minecart_pass"),
    ("BlockPart", "state_at_viewpoint"),
    ("ItemPart", "inventory_tick"),
    ("ItemPart", "item_bar"),
    ("ItemPart", "on_use_tick"),
    ("ItemPart", "on_entity_item_update"),
    ("ItemPart", "should_cause_block_break_reset"),
    ("ItemPart", "tooltip"),
    ("ItemPart", "highlight_tip"),
    ("ItemPart", "use_animation"),
    ("ItemPart", "use_duration"),
    ("BlockEntityPart", "block_entity_tick"),
    ("BlockEntityPart", "block_entity_ticks"),
    ("BlockEntityPart", "block_entity_client_tick"),
    ("BlockEntityPart", "block_entity_client_ticks"),
    ("BlockEntityPart", "block_entity_view_distance"),
    ("BlockEntityPart", "block_entity_render_bounding_box"),
    ("BlockEntityPart", "block_entity_submit"),
    ("BlockEntityPart", "screen_draw"),
    ("BlockEntityPart", "screen_tooltip"),
];
/// The Part methods skipped for a reason of their own, as `(trait, method, reason)`: a block entity's
/// saved data is NBT output, which the Lua host does not hand out; attachment state is not a Part
/// state in v1.
pub const PART_EXCLUDED: &[(&str, &str, &str)] = &[
    ("BlockEntityPart", "block_entity", NBT_REASON),
    ("BlockEntityPart", "block_entity_update_tag", NBT_REASON),
    (
        "BlockEntityPart",
        "stateful_block_entity",
        ATTACHMENT_REASON,
    ),
    ("EntityPart", "general_entity_tick", MOB_REASON),
];
const NBT_REASON: &str = "NBT output (Compound is denied)";
/// The reason of a mob's goals and brains and of a general entity row's tick: each runs per mob or per
/// tick, and needs a measurement before it is bound.
pub const MOB_REASON: &str = "per-mob object, per-tick turn: needs measurement";
/// The reason of a plain method the route table does not bind yet (see [`PLAIN_PART_METHODS`]).
pub const NEXT_CHUNK_REASON: &str = "plain method: next chunk";

/// How a plain Part method (one the game calls directly, with no event struct: see
/// `trait_sigs::parse_plain`) enters the generator. A bound method is planned like a replace hook,
/// from the parameters of its trait signature; a skipped one is listed with its reason.
pub struct PlainPart {
    /// The Part trait the method is on.
    pub trait_name: &'static str,
    /// The method.
    pub method: &'static str,
    /// `Ok(())` to bind it, or the reason it is skipped.
    pub route: Result<(), &'static str>,
    /// The inclusive range a bound answer must stay in (an out-of-range answer is refused, and the
    /// next part or the game's own answer applies). `None`: the answer's type is the whole range.
    pub answer_range: Option<&'static str>,
}

/// The route of every plain Part method the generator knows. Anything not listed here is not bound.
pub const PLAIN_PART_METHODS: &[PlainPart] = &[
    PlainPart {
        trait_name: "EntityPart",
        method: "is_shearable",
        route: Ok(()),
        answer_range: None,
    },
    PlainPart {
        trait_name: "EntityPart",
        method: "on_sheared",
        route: Err(NEXT_CHUNK_REASON),
        answer_range: None,
    },
    PlainPart {
        trait_name: "EntityPart",
        method: "spawn_cluster_size",
        route: Err(NEXT_CHUNK_REASON),
        answer_range: None,
    },
    PlainPart {
        trait_name: "EntityPart",
        method: "register_goals",
        route: Err(MOB_REASON),
        answer_range: None,
    },
    PlainPart {
        trait_name: "EntityPart",
        method: "register_behaviors",
        route: Err(MOB_REASON),
        answer_range: None,
    },
];

/// The route of `method` on `part`, if the table names it.
fn plain_route(part: PartTrait, method: &str) -> Option<&'static PlainPart> {
    PLAIN_PART_METHODS
        .iter()
        .find(|route| route.trait_name == part.name() && route.method == method)
}

/// The surface and dispatch signatures of a Part trait with its bound plain methods added: each bound
/// plain method becomes a replace hook of the surface, with its parameters as arguments and no
/// outputs, so [`plan`] plans it like one. The surface itself is not changed (the plain methods are
/// not in it: the game does not fire them as hooks).
pub fn with_plain(
    ir: &Ir,
    dispatch: &DispatchSigs,
    sigs: &[HookSig],
    part: PartTrait,
) -> (Ir, DispatchSigs) {
    let mut ir = ir.clone();
    let mut dispatch = DispatchSigs {
        hooks: dispatch.hooks.clone(),
        structs: dispatch.structs.clone(),
    };
    for sig in sigs {
        let (Some(args), Some(route)) = (&sig.plain, plain_route(part, &sig.name)) else {
            continue;
        };
        if route.route.is_err() {
            continue;
        }
        ir.hooks.push(HookIr {
            id: format!("hook:{}", sig.name),
            name: sig.name.clone(),
            kind: Kind::Replace,
            side: Side::Server,
            cancellable: None,
            scope: None,
            engine_target: String::new(),
            arguments: args
                .iter()
                .map(|(name, ty)| ArgIr {
                    name: name.clone(),
                    rust_type: ty.clone(),
                })
                .collect(),
            output: None,
            hash: "0".to_owned(),
            version: 1,
        });
        dispatch.hooks.insert(sig.name.clone(), Some(Vec::new()));
    }
    ir.hooks.sort_by(|a, b| a.name.cmp(&b.name));
    (ir, dispatch)
}

/// The answer range of the plain method `method`, if the route table names one (see
/// [`PlainPart::answer_range`]). Method names are unique across the Part traits the table names.
fn answer_range(method: &str) -> Option<&'static str> {
    PLAIN_PART_METHODS
        .iter()
        .find(|route| route.method == method)
        .and_then(|route| route.answer_range)
}

const ATTACHMENT_REASON: &str = "attachment state: not a Part state in v1";
const HOT_REASON: &str = "hot path (needs measurement)";
/// The hooks, on either a Hooks or a Part trait, that stay unbound with [`HOT_REASON`]: the ones
/// whose call sites run per shape update, per neighbour notify, per entity movement step or per hit.
/// Issue 74 removed the rest of the old list after measuring it: a subscribed delivery costs about
/// 4-5 us (measured on `can_survive` by `hot_path_bench` in `event-horizon-lua`'s `builtin.rs`,
/// release, 8 cores), and an unsubscribed hook about 50 ns, since the listen gate runs before any
/// conversion. `living_knockback` fires once per hit, and its gate-only cost with no subscriber is
/// 48 ns, so it is bound. The hooks kept here are kept for their call frequency, not for a missing
/// conversion: `update_shape` and `neighbor_changed` run per block update; `block_jump_factor`,
/// `block_speed_factor`, `friction`, `is_ladder`, `makes_open_trapdoor_above_climbable` and
/// `block_step_on` run per entity movement step, under the block a mover stands on or touches;
/// `living_incoming_damage` takes a mutable `DamageContainer`; and `ingredient_test` may run many
/// times per tick during recipe matching. None of them has a call-frequency measurement, and the
/// gate-only and subscribed costs above are the only numbers there are, so enabling any of them
/// needs that measurement first.
pub const HOT_HOOKS: &[&str] = &[
    "update_shape",
    "neighbor_changed",
    "block_jump_factor",
    "block_speed_factor",
    "friction",
    "is_ladder",
    "makes_open_trapdoor_above_climbable",
    "block_step_on",
    "living_incoming_damage",
    "ingredient_test",
];
const WORKER_REASON: &str = "worldgen worker";
/// Part methods that take or answer a container, a capability or a payload handler: the Lua host
/// does not carry those yet (see the manifest `powers`).
const CONTAINER_REASON: &str = "item container, capability or payload handler: not in v1";

/// The generated file of one Part trait: the table of its handler names with the reason each is not
/// bound, and the impl of its adapter, with the emitted methods.
pub struct PartsEmitted {
    /// The whole file.
    pub text: String,
    /// The Part methods emitted, in name order.
    pub emitted: Vec<String>,
    /// Each Part method not emitted, with its reason, in name order.
    pub skipped: Vec<(String, String)>,
    /// The answer and output types of the emitted methods (see [`Emitted::decisions`]).
    pub decisions: Vec<String>,
}

/// Emits the methods of the Part trait `part` (`sigs` is that trait's hook-shaped methods): each one
/// the hook pipeline can convert is emitted as a method of its adapter with the same name. The same
/// inputs always give the same text.
pub fn emit_parts(
    ir: &Ir,
    sigs: &[HookSig],
    dispatch: &DispatchSigs,
    plain: &Plain,
    handles: &[&str],
    part: PartTrait,
) -> PartsEmitted {
    let mut methods = Vec::new();
    let mut skipped = Vec::new();
    let mut table = String::new();
    let mut names: Vec<&HookSig> = sigs.iter().collect();
    names.sort_by(|a, b| a.name.cmp(&b.name));
    for sig in names {
        let name = sig.name.as_str();
        let worldgen = sig
            .host_ty
            .iter()
            .chain(&sig.ret)
            .any(|ty| ty.contains("WorldgenHost"))
            || sig.event_ty.contains("WorldgenHost");
        let container = sig
            .host_ty
            .iter()
            .chain(&sig.ret)
            .any(|ty| CONTAINER_TYPES.iter().any(|name| ty.contains(name)))
            || CONTAINER_TYPES
                .iter()
                .any(|name| sig.event_ty.contains(name));
        let hot = PART_HOT.contains(&(part.name(), name)) || HOT_HOOKS.contains(&name);
        let excluded = PART_EXCLUDED
            .iter()
            .find(|(trait_name, method, _)| *trait_name == part.name() && *method == name)
            .map(|(_, _, reason)| (*reason).to_owned());
        let skipped_route = match plain_route(part, name).map(|route| route.route) {
            Some(Err(reason)) => Some(reason.to_owned()),
            _ => None,
        };
        let result = if let Some(reason) = skipped_route {
            Err(reason)
        } else if hot {
            Err(HOT_REASON.to_owned())
        } else if let Some(reason) = excluded {
            Err(reason)
        } else if worldgen {
            Err(WORKER_REASON.to_owned())
        } else if container {
            Err(CONTAINER_REASON.to_owned())
        } else {
            plan(ir, sigs, dispatch, name, plain, handles, true)
        };
        match result {
            Ok(method) => {
                table.push_str(&format!("    ({name:?}, None),\n"));
                methods.push(method);
            }
            Err(reason) => {
                table.push_str(&format!("    ({name:?}, Some({reason:?})),\n"));
                skipped.push((name.to_owned(), reason));
            }
        }
    }
    let emitted: Vec<String> = methods
        .iter()
        .map(|method| method.hook.name.clone())
        .collect();
    let decisions = methods
        .iter()
        .flat_map(|method| {
            method.outputs.iter().map(|output| output.ty.clone()).chain(
                method
                    .answer
                    .as_deref()
                    .map(|answer| answer.replace("mod_api::", "")),
            )
        })
        .collect();

    let (name, adapter) = (part.name(), part.adapter());
    let mut text = format!(
        "// @generated by event-horizon-lua-gen from surface {}: do not edit.\n",
        ir.document_hash
    );
    text.push_str(&imports(
        &methods,
        &format!("use crate::host::{adapter};\n"),
    ));
    text.push_str(&format!(
        "\n/// Every {name} handler name with the reason it is not bound (`None`: it is bound), sorted.\n\
         pub const {table}: &[(&str, Option<&str>)] = &[\n",
        table = part.table()
    ));
    text.push_str(&table);
    text.push_str("];\n");
    text.push_str(&format!(
        "\nimpl mod_api::{name} for {adapter} {{\n    \
         fn {subscribed}(&self) -> &'static [Option<mod_api::HookId>] {{\n        \
         self.subscribed()\n    }}\n",
        subscribed = part.subscribed()
    ));
    for method in &methods {
        text.push_str(&method_text(method));
    }
    text.push_str("}\n");
    if part == PartTrait::BlockEntity {
        // A block entity part is a BlockPart too (the supertrait), and it subscribes to no block hook:
        // the game's block dispatch never calls it, so its block-side answers stay the game's.
        text.push_str(&format!(
            "\nimpl mod_api::BlockPart for {adapter} {{\n    \
             fn block_subscribed(&self) -> &'static [Option<mod_api::HookId>] {{\n        &[]\n    }}\n}}\n"
        ));
    }
    PartsEmitted {
        text,
        emitted,
        skipped,
        decisions,
    }
}

/// The types a Part method may not take or answer yet: a container, a capability or a payload
/// handler (their names as the trait spells them).
const CONTAINER_TYPES: &[&str] = &["ItemContainer", "Capability", "PayloadHandler"];

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
            plain: None,
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

    /// The shape of `block_step_probe` without its `state` argument (which has no conversion).
    fn step_ir() -> Ir {
        ir(vec![hook(
            "block_step_probe",
            Kind::Listen,
            vec![
                arg("dimension", "&'a str"),
                arg("entity", "i32"),
                arg("position", "[i32; 3]"),
            ],
        )])
    }

    fn step_sig() -> HookSig {
        sig(
            "block_step_probe",
            None,
            "event",
            "BlockStepOnEvent<'_>",
            None,
        )
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
            &["block_step_probe"],
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

    fn block_step_probe(&self, event: &mut BlockStepOnEvent<'_>) {
        crate::hostcall::refused(self.dispatch_turns(
            "block_step_probe",
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
        assert_eq!(out.emitted, ["block_step_probe"]);
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
    fn a_stack_in_hook_with_no_host_converts_through_the_global_table_and_refuses_before_dispatch()
    {
        let ir = ir(vec![hook(
            "stack_seen_global",
            Kind::Listen,
            vec![arg("stack", "&'a item::item_stack::ItemStack")],
        )]);
        let sigs = [sig(
            "stack_seen_global",
            None,
            "event",
            "StackSeenEvent<'_>",
            None,
        )];
        let out = emit_plain(&ir, &sigs, &["stack_seen_global"], &Plain::default());
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        // Tables missing: stack_in is None, so the hook is not delivered (undelivered, then return).
        assert!(
            out.text.contains(
                "        if crate::stacks::stack_in(&crate::stacks::GlobalHost, Some(event.stack)).is_none() {\n            crate::stacks::undelivered(\"stack_seen_global\");\n            return;\n        }\n"
            ),
            "{}",
            out.text
        );
        assert!(out.text.contains("TurnHost::None"), "{}", out.text);
        let gate = out
            .text
            .find("has_handlers(\"stack_seen_global\")")
            .expect("gated");
        let check = out.text.find("stack_in(").expect("checked");
        let dispatch = out.text.find("dispatch_turns(").expect("dispatched");
        assert!(
            gate < check && check < dispatch,
            "gate, then check, then dispatch"
        );
        syn::parse_file(&out.text).expect("the text parses");
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
    fn a_block_entity_key_and_a_slice_of_ints_convert_and_a_slice_of_strings_does_not() {
        let ir = ir(vec![
            hook("keyed", Kind::Listen, vec![arg("id", "BlockEntityTypeKey")]),
            hook("ints", Kind::Listen, vec![arg("data", "&'a[i32]")]),
            hook("strings", Kind::Listen, vec![arg("data", "&'a[String]")]),
        ]);
        let sigs = [
            sig("keyed", None, "event", "KeyedEvent<'_>", None),
            sig("ints", None, "event", "IntsEvent<'_>", None),
            sig("strings", None, "event", "StringsEvent<'_>", None),
        ];
        let emitted = emit_plain(&ir, &sigs, &["keyed", "ints", "strings"], &Plain::default());
        assert_eq!(emitted.emitted, ["ints", "keyed"]);
        assert!(
            emitted
                .text
                .contains("data.raw_set(\"id\", event.id.to_lua_value(lua)?)?;"),
            "{}",
            emitted.text
        );
        assert!(
            emitted
                .text
                .contains("data.raw_set(\"data\", event.data.to_lua_value(lua)?)?;"),
            "{}",
            emitted.text
        );
        assert!(
            emitted.skipped[0].1.contains("no Lua conversion"),
            "{:?}",
            emitted.skipped
        );
        syn::parse_file(&emitted.text).expect("the text parses");
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

    #[test]
    fn a_hot_hook_is_skipped_with_its_reason() {
        let out = emit_plain(
            &step_ir(),
            &[step_sig()],
            &["update_shape"],
            &Plain::default(),
        );
        assert!(out.emitted.is_empty());
        assert_eq!(
            out.skipped,
            [("update_shape".to_owned(), HOT_REASON.to_owned())]
        );
    }
}
