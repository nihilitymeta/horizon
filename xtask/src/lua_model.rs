//! The shared model of issue #42 round 2: every native hook in exactly one
//! class, and every generated hook's fields as converter tags.
//!
//! `lua_hooks.rs` emits the runtime table from this; `lua_types.rs` emits the
//! `.d.lua` event types and overloads from it. Both `--check`s fail when the
//! model says something the checked-in output does not.
//!
//! Classes: `generated` (every field has a converter below and the
//! host/contract is one the runtime handles), `adapter` (a hand-written
//! converter would unblock it: the missing converter is named), `tick`
//! (a `ticks` parameter: the level-tick host the Lua runtime does not
//! implement), `cannot` (no event-table mapping exists: the reason is named).
//!
//! The one rule mirrored from `horizon-mod-api-gen` (which host type a
//! receiver gets, and which output shapes the trait takes) is marked
//! `MIRRORED` below: if that crate changes it, the classification fails
//! loudly instead of drifting.

use horizon_mod_api_gen::{catalog_hooks, free_function_source, one_function};
use std::{collections::BTreeMap, fs, path::Path};

/// Mirrors `horizon-mod-api-gen`'s private `host_for` and its `receiver`
/// reading: which host type a receiver gets. Game hooks borrow the game
/// host; level hooks (`GameLevel` targets) borrow a level host. A mismatch
/// here is a compile error in the generated code, not silent drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Host {
    /// No receiver: the method takes no host.
    None,
    /// `host: &mut dyn Host`: actions run, kinds resolve.
    GameMut,
    /// `host: &dyn Host` (named `_host` unless a kind resolves through it):
    /// reads build the table, queued actions drop.
    GameRef,
    /// `level: &dyn LevelHost`: level reads build the table, actions drop.
    Level,
    /// `level: &mut dyn LevelRandomHost`: level reads build the table.
    LevelMut,
}

/// How a generated hook's handlers filter by kind (`horizon.on(name, fn,
/// kind)`): none, a `kind` string field of the event, or an entity id the
/// host resolves (`host.entity` then `host.entity_kind`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    None,
    /// `qualify(event.kind)`: the field names an entity type.
    Field,
    /// The entity id field names the entity whose kind filters.
    Entity {
        field: String,
        opt: bool,
    },
}

/// One converter tag per event-table field: the seed expression, the
/// write-back (if the handler may change it), the `.d.lua` type, and the
/// sample both sides of the smoke test use.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Conv {
    /// `&str` and owned numbers and bools: copied into the table as they are.
    Plain,
    /// `String`, `&String`: cloned into the table.
    PlainString,
    /// `Option` of a plain scalar or `&str`: nil or the value.
    OptPlain,
    /// `u128`: `uuid_text`.
    Uuid,
    /// `Option<u128>`: nil or `uuid_text`.
    OptUuid,
    /// `(player: u128, name)` on a game host: `player_by_uuid`.
    Player,
    /// `(player: Option<u128>, name)` on a game host.
    PlayerOpt,
    /// `LevelDimension`: `dimension_id`.
    Dim,
    /// `[i32; 3]`: `{x, y, z}`.
    Pos,
    /// `BlockPos`: `{x, y, z}`.
    PosStruct,
    /// `Option<[i32; 3]>`: nil or `{x, y, z}`.
    OptPos,
    /// `[i32; 2]` named `chunk` or `column`: `{x, z}`.
    Chunk,
    /// `[f32; 2]` or `[f64; 2]`: `{x, y}`.
    Pair,
    /// `[f64; 3]`: `{x, y, z}` floats.
    Vec3f,
    /// `&[i32]`: a sequence table.
    IntSeq,
    /// `&[String]`: a sequence table.
    StrSeq,
    /// `&ItemStack`: `item_table_of`.
    Item,
    /// `Option<&ItemStack>`: nil or `item_table_of`.
    OptItem,
    /// `BlockStateId` with a level host: `level.state_block`.
    StateLevel,
    /// `BlockStateId` with a game host: `host.state_name`.
    StateGame,
    /// The spike's hand note, kept for its tests: the block at the event's
    /// `pos`, for level hooks with a `pos` but no state of their own.
    BlockAtPos,
    /// The spike's hand note, kept for its tests: the block of the event's
    /// `state`, exposed as `block`.
    BlockFromState,
    /// `&mut` scalar, `&mut Option` scalar, `&mut bool`: copied in and
    /// written back.
    MutScalar,
    /// `&mut String`: cloned in and written back.
    MutString,
    /// `&mut Option<String>`: nil or the string, written back.
    MutOptString,
    /// `&mut [i32; 3]`: `{x, y, z}`, written back.
    MutPos,
    /// `&mut [f64; 3]`: `{x, y, z}` floats, written back.
    MutVec3,
    /// `&mut [f32; 2]`: `{x, y}`, written back.
    MutPair,
    /// `&mut Vec<String>`: a sequence table, written back.
    MutStrVec,
}

/// How a `replace` out value reads back: nil stays `None`, a table or value
/// becomes `Some`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplaceRead {
    Int,
    Bool,
    Pair,
    Item,
}

/// One event-table field of a generated hook.
#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub conv: Conv,
    /// The `.d.lua` type of the field (`?` already applied).
    pub ts: String,
}

/// An extra event-table field with no native parameter: the spike's hand
/// notes, kept so its tests assert what they always did.
#[derive(Clone, Debug)]
pub struct Extra {
    pub name: String,
    pub conv: Conv,
}

impl Model {
    /// The parameter the kind filter reads, if any: the `kind` field or the
    /// resolved entity id.
    pub fn kind_field(&self) -> Option<&str> {
        match &self.kind {
            Kind::Field => Some("kind"),
            Kind::Entity { field, .. } => Some(field.as_str()),
            Kind::None => None,
        }
    }
}

/// A generated hook's model: everything the emitters need.
#[derive(Clone, Debug)]
pub struct Model {
    pub name: String,
    pub event: String,
    pub contract: String,
    pub host: Host,
    /// Every native parameter in order, except a non-collect out: the
    /// event-table fields (a `player` table stands in for `player`+`name`).
    pub fields: Vec<Field>,
    /// Extra table fields with no parameter (the spike's hand notes).
    pub extras: Vec<Extra>,
    /// The raw parameter names the `player` table (if any) stands in for.
    pub skipped: Vec<String>,
    /// The native parameter names in dispatch order (ins, then outs).
    pub dispatch_params: Vec<Param>,
    pub out: Option<Out>,
    /// A cancel hook's flag parameter: removed from the table, set through
    /// `horizon.cancel()`, asserted on the native side.
    pub cancel_flag: Option<String>,
    pub kind: Kind,
    /// The event's elided lifetimes (`""`, `"<'_>"`, `"<'_, '_>"`).
    pub lifetimes: String,
    /// Whether any field is a borrow the event carries (informational).
    pub _borrows: bool,
}

/// One native parameter, for dispatch calls and sample values.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    /// The parameter's type, canonically rendered.
    pub ty: String,
}

/// A non-collect out parameter: the replace reader and the exact return type.
#[derive(Clone, Debug)]
pub struct Out {
    pub name: String,
    pub read: ReplaceRead,
    /// The trait method's exact return type (`Option<...>`).
    pub ret: String,
}

/// Every hook in exactly one class.
#[derive(Clone, Debug)]
pub enum Class {
    Generated(Model),
    /// A hand-written converter would unblock it: the missing converters.
    Adapter(Vec<String>),
    /// A `ticks` parameter: the level-tick host the Lua runtime does not
    /// implement.
    Tick,
    /// No event-table mapping exists: the reason.
    Cannot(String),
}

pub struct HookClass {
    pub name: String,
    pub class: Class,
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

/// The last segment's identifier of a path type.
fn last_ident(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) if p.qself.is_none() => {
            p.path.segments.last().map(|s| s.ident.to_string())
        }
        _ => None,
    }
}

/// `Some` inner type of an `Option<T>`.
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

/// A canonical rendering of a type, for the return types the generated code
/// repeats and the samples the tests build. Spacing follows `prettyplease`;
/// only the shape matters.
pub fn ty_str(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => {
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| match &s.arguments {
                    syn::PathArguments::None => s.ident.to_string(),
                    syn::PathArguments::AngleBracketed(a) => {
                        let args: Vec<String> = a
                            .args
                            .iter()
                            .map(|g| match g {
                                syn::GenericArgument::Type(t) => ty_str(t),
                                _ => "?".to_owned(),
                            })
                            .collect();
                        format!("{}<{}>", s.ident, args.join(", "))
                    }
                    syn::PathArguments::Parenthesized(a) => {
                        let ins: Vec<String> = a.inputs.iter().map(ty_str).collect();
                        let out = match &a.output {
                            syn::ReturnType::Default => String::new(),
                            syn::ReturnType::Type(_, t) => format!(" -> {}", ty_str(t)),
                        };
                        format!("{}({}){}", s.ident, ins.join(", "), out)
                    }
                })
                .collect();
            let mut out = String::new();
            if p.qself.is_some() {
                out.push_str("<qself>::");
            }
            if p.path.leading_colon.is_some() {
                out.push_str("::");
            }
            out + &segs.join("::")
        }
        syn::Type::Reference(r) => format!(
            "&{}{}",
            if r.mutability.is_some() { "mut " } else { "" },
            ty_str(&r.elem)
        ),
        syn::Type::Array(a) => {
            let len = match &a.len {
                syn::Expr::Lit(lit) => match &lit.lit {
                    syn::Lit::Int(n) => n.base10_digits().to_owned(),
                    _ => "?".into(),
                },
                _ => "?".into(),
            };
            format!("[{}; {}]", ty_str(&a.elem), len)
        }
        syn::Type::Slice(s) => format!("[{}]", ty_str(&s.elem)),
        syn::Type::Tuple(t) => {
            let es: Vec<String> = t.elems.iter().map(ty_str).collect();
            format!("({})", es.join(", "))
        }
        syn::Type::Paren(p) => ty_str(&p.elem),
        syn::Type::Group(g) => ty_str(&g.elem),
        _ => "?".to_owned(),
    }
}

/// Whether `ty` is a plain scalar the table copies: numbers and bools.
fn is_plain_scalar(ty: &syn::Type) -> bool {
    matches!(
        last_ident(ty).as_deref(),
        Some(
            "bool"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "f32"
                | "f64"
                | "usize"
                | "isize"
                | "i128"
                | "u128"
        )
    )
}

/// Whether `ty` is a `&mut` scalar the trait copies into the event and the
/// dispatch writes back (mirrors `EventLifetimes::field`'s scalar list).
fn is_copied_mut(ty: &syn::Type) -> bool {
    if let syn::Type::Reference(r) = ty
        && r.mutability.is_some()
    {
        return is_plain_scalar(&r.elem) || last_ident(&r.elem).as_deref() == Some("char");
    }
    false
}

/// Mirrors `horizon-mod-api-gen`'s private `EventLifetimes`: which elided
/// lifetimes the event struct carries over its fields. Every elided
/// reference lifetime becomes `'a`, or `'b` inside a `&mut`. A mismatch here
/// is a compile error in the generated methods, not silent drift.
struct Lifetimes {
    a: bool,
    b: bool,
    in_mut: bool,
}

impl Lifetimes {
    fn visit(&mut self, ty: &syn::Type) {
        match ty {
            syn::Type::Reference(r) => {
                if self.in_mut {
                    self.b = true;
                } else {
                    self.a = true;
                }
                let outer = self.in_mut;
                self.in_mut |= r.mutability.is_some();
                self.visit(&r.elem);
                self.in_mut = outer;
            }
            syn::Type::Path(p) => {
                for seg in &p.path.segments {
                    if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                        for arg in &args.args {
                            if let syn::GenericArgument::Type(t) = arg {
                                self.visit(t);
                            }
                        }
                    }
                }
            }
            syn::Type::Array(a) => self.visit(&a.elem),
            syn::Type::Slice(s) => self.visit(&s.elem),
            syn::Type::Tuple(t) => {
                for e in &t.elems {
                    self.visit(e);
                }
            }
            syn::Type::Paren(p) => self.visit(&p.elem),
            syn::Type::Group(g) => self.visit(&g.elem),
            _ => {}
        }
    }
}

/// The elided lifetimes of an event over its struct fields: `""`, `"<'_>"`
/// or `"<'_, '_>"`, exactly as the trait's event type carries them.
fn event_lifetimes(fields: &[syn::Type]) -> String {
    let mut lt = Lifetimes {
        a: false,
        b: false,
        in_mut: false,
    };
    for ty in fields {
        if is_copied_mut(ty) {
            continue;
        }
        let mut field = ty.clone();
        strip_lifetimes(&mut field);
        lt.visit(&field);
    }
    match (lt.a, lt.b) {
        (_, true) => "<'_, '_>".to_owned(),
        (true, false) => "<'_>".to_owned(),
        (false, false) => String::new(),
    }
}

/// The elided lifetime, parsed rather than built (xtask has no direct
/// `proc-macro2` dependency to make a span with).
fn parse_tick_lifetime() -> syn::Lifetime {
    syn::parse_str("'_").unwrap()
}

/// Replace every lifetime with `'_`, as the trait's event structs do.
fn strip_lifetimes(ty: &mut syn::Type) {
    match ty {
        syn::Type::Reference(r) => {
            r.lifetime = Some(parse_tick_lifetime());
            strip_lifetimes(&mut r.elem);
        }
        syn::Type::Path(p) => {
            for seg in &mut p.path.segments {
                if let syn::PathArguments::AngleBracketed(args) = &mut seg.arguments {
                    for arg in &mut args.args {
                        match arg {
                            syn::GenericArgument::Type(t) => strip_lifetimes(t),
                            syn::GenericArgument::Lifetime(l) => {
                                *l = parse_tick_lifetime();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        syn::Type::Array(a) => strip_lifetimes(&mut a.elem),
        syn::Type::Slice(s) => strip_lifetimes(&mut s.elem),
        syn::Type::Tuple(t) => {
            for e in &mut t.elems {
                strip_lifetimes(e);
            }
        }
        syn::Type::Paren(p) => strip_lifetimes(&mut p.elem),
        syn::Type::Group(g) => strip_lifetimes(&mut g.elem),
        _ => {}
    }
}

/// `kind` string fields that do NOT name an entity type: no kind filter,
/// even when the hook is otherwise generated. Everything else named `kind`
/// is an entity/mob/vehicle/hanging type the filter matches.
const NON_ENTITY_KINDS: &[(&str, &str)] = &[
    ("toast_add", "a toast category, not an entity type"),
    ("container_opened", "a menu type, not an entity type"),
    ("block_spread", "`fluid` or `fire`, not an entity type"),
    ("consume_effect", "an effect namespace, not an entity type"),
    ("enchant_effect", "an effect namespace, not an entity type"),
    ("loot_function", "a loot function id, not an entity type"),
    ("loot_condition", "a loot condition id, not an entity type"),
    ("ingredient", "an ingredient kind id, not an entity type"),
    (
        "ingredient_test",
        "an ingredient kind id, not an entity type",
    ),
];

/// Entity-id parameter names that resolve to a kind through the host, in
/// the order the subject is picked when several are present. `causing` and
/// `direct` (damage sources) and `merchant`/`parent`/`pearl`/slots are
/// exposed as plain numbers: only the hook's subject filters.
const ENTITY_ID_NAMES: &[&str] = &[
    "entity",
    "entity_id",
    "projectile",
    "target",
    "vehicle",
    "owner",
];

/// The outcome of classifying one field.
enum FieldOutcome {
    Conv(Conv, String),
    Adapter(String),
    Cannot(String),
}

/// Classify one parameter's converter. `host` decides the `BlockStateId`
/// and player-table rules; the error strings name the missing converter or
/// the reason (they end up in the checked-in classification). `is_out` is
/// the hook's collect out.
fn field_of(hook: &str, param: &str, ty: &syn::Type, host: Host, is_out: bool) -> FieldOutcome {
    let adapter = |what: &str| FieldOutcome::Adapter(format!("{hook}: adapter: {what}"));
    let cannot = |what: &str| FieldOutcome::Cannot(format!("{hook}: unsupported: {what}"));
    let (inner, borrowed) = deref(ty);
    let ident = last_ident(inner);
    // A `char` has no Lua form (a `&mut char` the trait copies is not
    // worth a converter either).
    if ident.as_deref() == Some("char") {
        return adapter("a `char` needs its string form");
    }
    // Rich text never crosses as text: only its plain form would, and a
    // hand-written converter would have to pick it.
    if matches!(ident.as_deref(), Some("Text")) {
        return adapter("rich text needs its plain form");
    }
    if matches!(ident.as_deref(), Some("Compound" | "Value" | "Map")) {
        return adapter("a compound needs the nbt table");
    }
    if let Some(inner_t) = option_inner(inner) {
        let inner_id = last_ident(inner_t);
        if matches!(inner_id.as_deref(), Some("Text")) {
            return adapter("rich text needs its plain form");
        }
        if matches!(inner_id.as_deref(), Some("Compound" | "Value")) {
            return adapter("a compound needs the nbt table");
        }
    }
    // Tuples never map to an event-table field.
    if matches!(inner, syn::Type::Tuple(_)) {
        return adapter("a tuple needs its table form");
    }
    // `&mut` seeds copy in and write back; everything else borrows or
    // owns. Shared borrows are only `&str`, item stacks, slices and the
    // opaque types, each handled on its own arm below.
    if let syn::Type::Reference(r) = ty
        && r.mutability.is_some()
    {
        return mut_of(hook, param, &r.elem, host, is_out, adapter, cannot);
    }
    // `Option<T>`: nil or the inner seed, for the inners the table handles.
    if let Some(inner_t) = option_inner(inner) {
        let inner_id = last_ident(inner_t);
        match inner_id.as_deref() {
            Some("ItemStack") if matches!(inner_t, syn::Type::Path(_)) => {
                return FieldOutcome::Conv(Conv::OptItem, "Item?".into());
            }
            Some("u128") => return FieldOutcome::Conv(Conv::OptUuid, "string?".into()),
            _ if is_plain_scalar(inner_t) => {
                let ts = ts_of_scalar(inner_id.as_deref().unwrap_or("?"));
                return FieldOutcome::Conv(Conv::OptPlain, format!("{ts}?"));
            }
            Some("Direction") => {
                return adapter("an optional direction needs its name");
            }
            Some("Hand") => {
                return adapter("an optional hand needs its name");
            }
            Some("BlockPos") => {
                return adapter("an optional pos needs its table form");
            }
            _ => {}
        }
        if let syn::Type::Array(array) = inner_t {
            let is_i32x3 = is_path(&array.elem, "i32")
                && matches!(&array.len, syn::Expr::Lit(lit) if matches!(&lit.lit, syn::Lit::Int(n) if n.base10_digits() == "3"));
            if is_i32x3 {
                return FieldOutcome::Conv(Conv::OptPos, "Vec3?".into());
            }
            return adapter("an optional array needs its table form");
        }
        if matches!(inner_t, syn::Type::Reference(_)) {
            let (ri, _) = deref(inner_t);
            if is_path(ri, "ItemStack") {
                return FieldOutcome::Conv(Conv::OptItem, "Item?".into());
            }
            if is_path(ri, "str") {
                return FieldOutcome::Conv(Conv::OptPlain, "string?".into());
            }
            return adapter("an optional borrow needs its table form");
        }
        return adapter("an optional value needs its table form");
    }
    // Scalars and strings copy straight in.
    if is_path(inner, "str") {
        return FieldOutcome::Conv(Conv::Plain, "string".into());
    }
    if matches!(ident.as_deref(), Some("String")) && !borrowed {
        return FieldOutcome::Conv(Conv::PlainString, "string".into());
    }
    if matches!(ident.as_deref(), Some("String")) && borrowed {
        return FieldOutcome::Conv(Conv::PlainString, "string".into());
    }
    if is_plain_scalar(inner) {
        let ts = ts_of_scalar(ident.as_deref().unwrap_or("?"));
        return FieldOutcome::Conv(Conv::Plain, ts.into());
    }
    if last_ident(inner).as_deref() == Some("u128") {
        return FieldOutcome::Conv(Conv::Uuid, "string".into());
    }
    match ident.as_deref() {
        Some("LevelDimension") => FieldOutcome::Conv(Conv::Dim, "string".into()),
        Some("BlockPos") => FieldOutcome::Conv(Conv::PosStruct, "Vec3".into()),
        Some("BlockStateId") => match host {
            Host::Level | Host::LevelMut => FieldOutcome::Conv(Conv::StateLevel, "string".into()),
            Host::GameMut => FieldOutcome::Conv(Conv::StateGame, "string".into()),
            _ => adapter("a block state needs a host to name it"),
        },
        Some("BlockId") => adapter("a block id needs its name"),
        Some("Direction") => adapter("a direction needs its name"),
        Some("Hand") => adapter("a hand needs its name"),
        Some("ItemStack") if borrowed => FieldOutcome::Conv(Conv::Item, "Item".into()),
        Some("ItemStack") => FieldOutcome::Conv(Conv::Item, "Item".into()),
        _ => array_or_other_of(hook, param, ty, inner, borrowed, host, adapter, cannot),
    }
}

/// Whether `ty` is a borrowed `&str`.
fn is_str_ref(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Reference(r) if r.mutability.is_none() && is_path(&r.elem, "str"))
}

/// The event struct's field type for a native parameter: what the trait
/// holds (mirrors `EventLifetimes::field`'s scalar copy).
fn struct_field_ty(ty: &syn::Type) -> syn::Type {
    if is_copied_mut(ty)
        && let syn::Type::Reference(r) = ty
    {
        return (*r.elem).clone();
    }
    ty.clone()
}

/// The `.d.lua` type of a plain scalar.
fn ts_of_scalar(id: &str) -> &'static str {
    match id {
        "bool" => "boolean",
        "f32" | "f64" => "number",
        _ => "number",
    }
}

/// Whether `ty` mentions rich text, compounds or stacks: the adapter
/// reasons `field_of` gives, shared with `mut_of` so a `&mut`-wrapped one
/// does not fall through to "no mapping".
fn wrapped_reason(ty: &syn::Type) -> Option<&'static str> {
    match ty {
        syn::Type::Path(p) if p.qself.is_none() => {
            let id = p.path.segments.last().map(|s| s.ident.to_string());
            match id.as_deref() {
                Some("Text") => Some("rich text needs its plain form"),
                Some("Compound" | "Value" | "Map") => Some("a compound needs the nbt table"),
                Some("ItemStack") => Some("a stack out needs its table form"),
                _ => {
                    if let Some(seg) = p.path.segments.last()
                        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
                    {
                        args.args.iter().find_map(|g| match g {
                            syn::GenericArgument::Type(t) => wrapped_reason(t),
                            _ => None,
                        })
                    } else {
                        None
                    }
                }
            }
        }
        syn::Type::Reference(r) => wrapped_reason(&r.elem),
        syn::Type::Array(a) => wrapped_reason(&a.elem),
        syn::Type::Slice(s) => wrapped_reason(&s.elem),
        syn::Type::Tuple(_) => Some("a tuple needs its table form"),
        syn::Type::Paren(p) => wrapped_reason(&p.elem),
        syn::Type::Group(g) => wrapped_reason(&g.elem),
        _ => None,
    }
}

/// Whether `ty` mentions an item stack: a `&mut` one cannot write back
/// without losing components, and a collect out needs its table form.
fn mentions_stack(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(p) if p.qself.is_none() => {
            if last_ident(ty).as_deref() == Some("ItemStack") {
                return true;
            }
            p.path.segments.iter().any(|seg| {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    args.args.iter().any(|g| match g {
                        syn::GenericArgument::Type(t) => mentions_stack(t),
                        _ => false,
                    })
                } else {
                    false
                }
            })
        }
        syn::Type::Reference(r) => mentions_stack(&r.elem),
        syn::Type::Array(a) => mentions_stack(&a.elem),
        syn::Type::Slice(s) => mentions_stack(&s.elem),
        syn::Type::Tuple(t) => t.elems.iter().any(mentions_stack),
        syn::Type::Paren(p) => mentions_stack(&p.elem),
        syn::Type::Group(g) => mentions_stack(&g.elem),
        _ => false,
    }
}

/// `&mut` fields: copied into the table and written back after the call.
/// `is_out` is the hook's collect out (the only `&mut` whose write-back the
/// contract owns).
fn mut_of(
    hook: &str,
    _param: &str,
    elem: &syn::Type,
    _host: Host,
    is_out: bool,
    adapter: impl Fn(&str) -> FieldOutcome,
    cannot: impl Fn(&str) -> FieldOutcome,
) -> FieldOutcome {
    if mentions_stack(elem) {
        return adapter(if is_out {
            "a stack out needs its table form"
        } else {
            "a mutable stack needs its table form"
        });
    }
    if let Some(why) = wrapped_reason(elem) {
        return adapter(why);
    }
    let elem_id = last_ident(elem);
    match elem_id.as_deref() {
        Some("String") => return FieldOutcome::Conv(Conv::MutString, "string".into()),
        _ if is_plain_scalar(elem) => {
            let ts = ts_of_scalar(elem_id.as_deref().unwrap_or("?"));
            return FieldOutcome::Conv(Conv::MutScalar, ts.into());
        }
        _ => {}
    }
    if let Some(opt) = option_inner(elem) {
        let opt_id = last_ident(opt);
        match opt_id.as_deref() {
            Some("String") => {
                return FieldOutcome::Conv(Conv::MutOptString, "string?".into());
            }
            _ if is_plain_scalar(opt) => {
                let ts = ts_of_scalar(opt_id.as_deref().unwrap_or("?"));
                return FieldOutcome::Conv(Conv::MutScalar, format!("{ts}?"));
            }
            _ => {}
        }
    }
    if is_vec_of(elem, "String") {
        return FieldOutcome::Conv(Conv::MutStrVec, "{ string }".into());
    }
    if let syn::Type::Array(array) = elem {
        let len = match &array.len {
            syn::Expr::Lit(lit) => match &lit.lit {
                syn::Lit::Int(n) => n.base10_parse::<usize>().unwrap_or(0),
                _ => 0,
            },
            _ => 0,
        };
        let arr_elem = last_ident(&array.elem)
            .as_deref()
            .unwrap_or_default()
            .to_owned();
        match (len, arr_elem.as_str()) {
            (3, "i32") => return FieldOutcome::Conv(Conv::MutPos, "Vec3".into()),
            (3, "f64") => return FieldOutcome::Conv(Conv::MutVec3, "Vec3".into()),
            (2, "f32") => return FieldOutcome::Conv(Conv::MutPair, "Vec2".into()),
            _ => return adapter("a mutable array needs its table form"),
        }
    }
    if is_path(elem, "ItemStack") {
        return adapter("a mutable stack needs its table form");
    }
    opaque_of(hook, elem, adapter, cannot)
}

/// Arrays, slices and the opaque types: the long tail of the classifier.
#[allow(clippy::too_many_arguments)]
fn array_or_other_of(
    _hook: &str,
    param: &str,
    _ty: &syn::Type,
    inner: &syn::Type,
    _borrowed: bool,
    _host: Host,
    adapter: impl Fn(&str) -> FieldOutcome,
    cannot: impl Fn(&str) -> FieldOutcome,
) -> FieldOutcome {
    // Fixed arrays with a table form the runtime already builds. Mutable
    // arrays never reach this arm (they write back through `mut_of`).
    if let syn::Type::Array(array) = inner {
        let len = match &array.len {
            syn::Expr::Lit(lit) => match &lit.lit {
                syn::Lit::Int(n) => n.base10_parse::<usize>().unwrap_or(0),
                _ => 0,
            },
            _ => 0,
        };
        let elem = last_ident(&array.elem)
            .as_deref()
            .unwrap_or_default()
            .to_owned();
        match (len, elem.as_str()) {
            (2, "i32") if param == "chunk" || param == "column" => {
                return FieldOutcome::Conv(Conv::Chunk, "Chunk".into());
            }
            (3, "i32") => return FieldOutcome::Conv(Conv::Pos, "Vec3".into()),
            (3, "f64") => return FieldOutcome::Conv(Conv::Vec3f, "Vec3".into()),
            (2, "f32") | (2, "f64") => {
                return FieldOutcome::Conv(Conv::Pair, "Vec2".into());
            }
            (2, "i32") => {
                return adapter("a 2D int array needs its table form");
            }
            _ => return adapter("an array needs its table form"),
        }
    }
    // Slices: only the scalar sequence tables exist.
    if let syn::Type::Slice(slice) = inner {
        if is_path(&slice.elem, "i32") {
            return FieldOutcome::Conv(Conv::IntSeq, "{ number }".into());
        }
        if is_path(&slice.elem, "String") {
            return FieldOutcome::Conv(Conv::StrSeq, "{ string }".into());
        }
        if is_path(&slice.elem, "u8") {
            return adapter("bytes need their table form");
        }
        if is_path(&slice.elem, "BlockPos") {
            return adapter("a block list needs its table form");
        }
        return adapter("a slice needs its table form");
    }
    // The opaque types, each with its missing converter or reason.
    opaque_of(_hook, inner, adapter, cannot)
}

/// One rule per opaque type: `adapter` names the hand-written converter
/// that would unblock it, `cannot` the reason no table mapping exists.
/// The single generic argument of a `Vec<X>`/`Option<X>`, or `None`.
fn single_generic_arg<'t>(ty: &'t syn::Type, names: &[&str]) -> Option<&'t syn::Type> {
    if let syn::Type::Path(p) = ty
        && p.qself.is_none()
        && let Some(seg) = p.path.segments.last()
        && names.contains(&seg.ident.to_string().as_str())
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(t)) = args.args.first()
    {
        return Some(t);
    }
    None
}

/// The cannot reason for a borrow that can never enter the Lua state, if
/// the type is one: render-thread borrows and draw handles (Lua runs on the
/// game thread and has no drawing API), worldgen borrows, borrowed loot
/// state, and client input.
fn cannot_id(id: &str) -> Option<String> {
    if matches!(
        id,
        "NativeMobRender"
            | "NativePlayerRender"
            | "BlockEntityDraw"
            | "NativeWorldDraw"
            | "WorldPrimitives"
            | "WorldHold"
            | "RenderNeeds"
            | "Corners"
            | "HandDrawn"
            | "HudBatch"
            | "NativeScreenView"
            | "NativeScreen"
            | "NativePanelEvent"
            | "NativeTextField"
            | "GuiDraw"
            | "NativeBlockRim"
            | "NativeColoredLight"
            | "FogData"
            | "FogType"
            | "CameraEntity"
            | "FovPlayer"
            | "BiomeColors"
            | "HealthInputs"
            | "NativeMinimap"
            | "NativeZoom"
            | "StackView"
            | "NativeShake"
            | "WorldDrawn"
    ) {
        return Some(format!("{id} borrows render state (render thread)"));
    }
    if matches!(
        id,
        "NativeGenerate" | "NativeProtoChunk" | "NativeLodColumn"
    ) {
        return Some(format!("{id} borrows the generating chunk (worldgen)"));
    }
    if matches!(
        id,
        "NativeLootContext" | "LootStack" | "LootEntity" | "LootDamage"
    ) {
        return Some(format!("{id} is borrowed loot state"));
    }
    if id == "KeyEvent" {
        return Some("a borrowed client key event cannot enter the Lua state".into());
    }
    if id == "Receiver" {
        return Some("a login gate cannot wait on the Lua thread".into());
    }
    None
}

fn opaque_of(
    _hook: &str,
    ty: &syn::Type,
    adapter: impl Fn(&str) -> FieldOutcome,
    cannot: impl Fn(&str) -> FieldOutcome,
) -> FieldOutcome {
    // A list around one opaque thing: a borrow inside stays lost,
    // anything else names the element's converter.
    let listed: Option<&syn::Type> = single_generic_arg(ty, &["Vec"]).or(match ty {
        syn::Type::Slice(s) => Some(&*s.elem),
        _ => None,
    });
    if let Some(elem) = listed {
        if let Some(why) = cannot_id(last_ident(elem).as_deref().unwrap_or_default()) {
            return cannot(&why);
        }
        if is_plain_scalar(elem) {
            return adapter("a scalar list needs its table form");
        }
        if let Some(why) = wrapped_reason(elem) {
            return adapter(why);
        }
        if let syn::Type::Array(a) = elem
            && is_path(&a.elem, "i32")
        {
            return adapter("a block list needs its table form");
        }
        let (inner, _) = deref(elem);
        if is_path(inner, "str") {
            return adapter("needs owned strings");
        }
        let elem_id = last_ident(elem).as_deref().unwrap_or_default().to_owned();
        return adapter(&format!("a {} list needs its table form", snake(&elem_id)));
    }
    // An optional around one opaque thing: same, with optional wording.
    if let Some(inner) = single_generic_arg(ty, &["Option"]) {
        if let Some(why) = cannot_id(last_ident(inner).as_deref().unwrap_or_default()) {
            return cannot(&why);
        }
        if let Some(why) = wrapped_reason(inner) {
            return adapter(why);
        }
        let inner_id = last_ident(inner).as_deref().unwrap_or_default().to_owned();
        if is_plain_scalar(inner) || inner_id.is_empty() {
            return adapter("an optional value needs its table form");
        }
        return adapter(&format!(
            "an optional {} needs its table form",
            snake(&inner_id)
        ));
    }
    let id = last_ident(ty).as_deref().unwrap_or_default().to_owned();
    if let Some(why) = cannot_id(&id) {
        return cannot(&why);
    }
    // Borrowed plain data a hand-written converter could copy into a table.
    if matches!(
        id.as_str(),
        "SoundPlayback" | "NativeMapInfo" | "NativeNoteBlockChange" | "NativeLootDrops"
    ) {
        return adapter(&format!("{} needs its table form", snake(&id)));
    }
    // A live mob a hand-written converter could snapshot as `{id, kind}`.
    if id == "Mob" {
        return adapter("a live mob needs its table form");
    }
    // Network packets: one decoded packet per hook, needing per-type
    // decoders no single converter covers.
    if id.contains("Packet") {
        return adapter("a packet needs its table form");
    }
    // Opaque mod-data handles a hand-written table could name.
    if matches!(
        id.as_str(),
        "ModBlockEntity"
            | "ModContainer"
            | "ModFluidTanks"
            | "ModEnergyBuffer"
            | "ItemEnergyBuffer"
            | "NativeFluidStack"
            | "NativeDispensed"
            | "NativeFluidTankView"
            | "NativeFluidDrain"
            | "NativeFluidAction"
            | "FluidState"
            | "BlockProperties"
            | "PlacementContext"
    ) {
        return adapter(&format!("a {} needs its table form", snake(&id)));
    }
    // Plain data a hand-written converter could map.
    if matches!(
        id.as_str(),
        "MerchantOffer"
            | "AnvilCraft"
            | "Suggestion"
            | "SpawnEntry"
            | "Music"
            | "MusicLayer"
            | "UseAnimation"
            | "NativeCommand"
            | "NativeMod"
            | "NativeConfig"
            | "NativeDataMapType"
            | "NativeMapDecoration"
            | "NativeReloadListener"
            | "NativeConfigFile"
            | "NativeLifecycleContext"
            | "RecipeBookCategory"
            | "LayerDefinition"
            | "EntityDataValue"
            | "Property"
            | "NativeServerPlayer"
            | "Block"
            | "Item"
    ) {
        return adapter(&format!("a {} needs its table form", snake(&id)));
    }
    if matches!(id.as_str(), "WorldPhase" | "ServerTickPhase") {
        return adapter("a phase needs its name");
    }
    if id == "Duration" {
        return adapter("a duration needs its ticks");
    }
    if id == "SocketAddr" {
        return adapter("an address needs its text form");
    }
    if id == "Path" {
        return adapter("a path needs its text form");
    }
    if matches!(id.as_str(), "Value" | "serde_json") {
        return adapter("json needs its lua form");
    }
    cannot(&format!("type {} has no mapping", ty_str(ty)))
}

/// `PascalCase` to `snake_case` for the adapter reasons.
fn snake(id: &str) -> String {
    let mut out = String::new();
    for (i, c) in id.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
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

/// Read one hook's model: its contract and out from the catalog, its
/// receiver and parameters from the native function. Mirrors
/// `horizon-mod-api-gen`'s contract/out/host rules (marked MIRRORED in the
/// type docs); anything the trait would reject lands in `cannot` instead of
/// generating code that does not compile.
// The `Err` carries the hook's class, never a model: boxing it would only
// hide which class a failed hook lands in.
#[allow(clippy::result_large_err)]
fn model_of(
    root: &Path,
    hook: &serde_json::Value,
    game: &[(String, syn::ItemFn)],
    frees: &mut BTreeMap<String, Vec<syn::ItemFn>>,
) -> Result<Model, Class> {
    let name = hook["name"]
        .as_str()
        .ok_or("hook name missing")
        .map_err(|e| Class::Cannot(format!("unsupported: {e}")))?;
    let fail = |why: &str| Class::Cannot(format!("{name}: unsupported: {why}"));
    let api = hook
        .get("mod_api")
        .ok_or_else(|| fail("hook has no mod_api entry"))?;
    let contract = api
        .get("contract")
        .and_then(|c| c.as_str())
        .ok_or_else(|| fail("contract missing"))?;
    if !matches!(contract, "listen" | "cancel" | "collect" | "replace") {
        return Err(fail(&format!("contract {contract} is not generated")));
    }
    let out = api.get("out").and_then(|v| v.as_str()).map(str::to_owned);
    // (cancel | replace | collect, out) or (listen, no out): the trait's own
    // rule; anything else never compiled.
    match (contract, out.as_deref()) {
        ("listen", None) | ("cancel" | "replace" | "collect", Some(_)) => {}
        _ => return Err(fail("invalid contract for this hook")),
    }
    let target = hook
        .get("target")
        .and_then(|t| t.as_str())
        .ok_or_else(|| fail("target missing"))?;
    let native = native_function(root, target, name, game, frees).map_err(|e| fail(&e))?;
    let mut receiver = None;
    let mut params: Vec<(String, syn::Type)> = Vec::new();
    for arg in &native.sig.inputs {
        match arg {
            syn::FnArg::Receiver(r) => {
                receiver = Some(r.mutability.is_some());
            }
            syn::FnArg::Typed(arg) => {
                let syn::Pat::Ident(pat) = &*arg.pat else {
                    return Err(fail("unsupported argument"));
                };
                params.push((pat.ident.to_string(), (*arg.ty).clone()));
            }
        }
    }
    // Any parameter named `ticks`: the level-tick host the Lua runtime does
    // not implement (a plain `u32` of that name is scheduling-adjacent too,
    // so the name alone decides).
    if params.iter().any(|(n, _)| n == "ticks") {
        return Err(Class::Tick);
    }
    // MIRRORED from `host_for`: which host type a receiver gets.
    let is_level = target.contains("::GameLevel<");
    let host = match receiver {
        None => Host::None,
        Some(mutable) => {
            if is_level {
                if mutable { Host::LevelMut } else { Host::Level }
            } else if mutable {
                Host::GameMut
            } else {
                Host::GameRef
            }
        }
    };
    // A collect out stays a parameter: the event carries it as a value for
    // every mod to edit in turn. Every other out leaves the parameters: the
    // method returns it instead.
    let collect = contract == "collect";
    let mut out_ty: Option<syn::Type> = None;
    if let Some(out) = &out {
        let index = params
            .iter()
            .position(|(n, _)| n == out)
            .ok_or_else(|| fail(&format!("missing output {out}")))?;
        let ty = if collect {
            params[index].1.clone()
        } else {
            params.remove(index).1.clone()
        };
        let syn::Type::Reference(reference) = &ty else {
            return Err(fail(&format!("output {out} is not a reference")));
        };
        if reference.mutability.is_none() {
            return Err(fail(&format!("output {out} is not written back")));
        }
        out_ty = Some((*reference.elem).clone());
    }
    // A cancel out is the handler's `horizon.cancel()`: it must be a bare
    // bool flag the trait copies.
    if contract == "cancel"
        && let Some(ty) = &out_ty
        && !is_path(ty, "bool")
    {
        return Err(fail("a cancel out must be a bool flag"));
    }
    // A replace out reads back through one of the supported shapes.
    let mut out_read: Option<Out> = None;
    if contract == "replace"
        && let Some(ty) = &out_ty
        && let Some(out) = &out
    {
        let inner = option_inner(ty).unwrap_or(ty);
        let read = match inner {
            syn::Type::Path(_) if is_path(inner, "i32") || is_path(inner, "u32") => {
                ReplaceRead::Int
            }
            syn::Type::Path(_) if is_path(inner, "bool") => ReplaceRead::Bool,
            syn::Type::Array(_) => ReplaceRead::Pair,
            syn::Type::Path(_) if is_path(inner, "ItemStack") => ReplaceRead::Item,
            _ => {
                return Err(Class::Adapter(vec![format!(
                    "{name}: adapter: a replace out needs its table form"
                )]));
            }
        };
        out_read = Some(Out {
            name: out.clone(),
            read,
            ret: ty_str(ty),
        });
    }
    // Every remaining parameter needs a converter. The struct field types
    // feed the lifetime computation: a collect out stays a parameter, so
    // the loop below covers every struct field exactly once (a skipped
    // `name` still lives in the event struct).
    let player_pair = host == Host::GameMut
        && params
            .iter()
            .any(|(n, t)| n == "player" && is_path(t, "u128"))
        && params.iter().any(|(n, _)| n == "name");
    let player_opt_pair = host == Host::GameMut
        && params
            .iter()
            .any(|(n, t)| n == "player" && option_inner(t).is_some_and(|i| is_path(i, "u128")))
        && params.iter().any(|(n, _)| n == "name");
    let mut fields = Vec::new();
    let mut skipped = Vec::new();
    let mut adapters = Vec::new();
    let mut cannots = Vec::new();
    let mut struct_tys = Vec::new();
    for (param, ty) in &params {
        // The player table stands in for `player`+`name`.
        if (player_pair || player_opt_pair) && (param == "player" || param == "name") {
            struct_tys.push(struct_field_ty(ty));
            if param == "player" {
                let (conv, ts) = if player_pair {
                    (Conv::Player, "PlayerRef")
                } else {
                    (Conv::PlayerOpt, "PlayerRef?")
                };
                fields.push(Field {
                    name: param.clone(),
                    conv,
                    ts: ts.into(),
                });
            } else {
                skipped.push(param.clone());
            }
            continue;
        }
        match field_of(
            name,
            param,
            ty,
            host,
            collect && Some(param) == out.as_ref(),
        ) {
            FieldOutcome::Conv(conv, ts) => {
                fields.push(Field {
                    name: param.clone(),
                    conv,
                    ts,
                });
                struct_tys.push(struct_field_ty(ty));
            }
            FieldOutcome::Adapter(why) => adapters.push(why),
            FieldOutcome::Cannot(why) => cannots.push(why),
        }
    }
    if !cannots.is_empty() {
        return Err(Class::Cannot(cannots.join("; ")));
    }
    if !adapters.is_empty() {
        return Err(Class::Adapter(adapters));
    }
    // The kind filter: a `kind` string field names an entity type (minus
    // the documented non-entity kinds), else the hook's single entity id
    // resolves through a game host.
    let kind = if params.iter().any(|(n, t)| n == "kind" && is_str_ref(t))
        && !NON_ENTITY_KINDS.iter().any(|(n, _)| n == &name)
    {
        Kind::Field
    } else if matches!(host, Host::GameMut | Host::GameRef) {
        let ids: Vec<(&String, &syn::Type)> = params
            .iter()
            .filter(|(n, t)| {
                ENTITY_ID_NAMES.contains(&n.as_str())
                    && matches!(
                        ty_str(t).as_str(),
                        "u32" | "i32" | "Option<u32>" | "Option<i32>"
                    )
            })
            .map(|(n, t)| (n, t))
            .collect();
        match ids.as_slice() {
            [(field, ty)] => Kind::Entity {
                field: (*field).clone(),
                opt: ty_str(ty).starts_with("Option"),
            },
            _ => Kind::None,
        }
    } else {
        Kind::None
    };
    let lifetimes = event_lifetimes(&struct_tys);
    let dispatch_params = params
        .iter()
        .map(|(n, t)| Param {
            name: n.clone(),
            ty: ty_str(t),
        })
        .collect();
    let cancel_flag = if contract == "cancel" {
        out.clone()
    } else {
        None
    };
    // The spike's hand notes, kept so its tests assert what they always
    // did: `block_burn` and `bell_ring` expose the block at their `pos`,
    // `is_bonemeal_success` exposes its `state` as `block`.
    let mut extras = Vec::new();
    if name == "block_burn" || name == "bell_ring" {
        extras.push(Extra {
            name: "block".to_owned(),
            conv: Conv::BlockAtPos,
        });
    }
    if name == "is_bonemeal_success" {
        // The spike exposes `state` as `block`; the parameter is still
        // fired (the dispatch call below matches it by type).
        fields.retain(|f| f.name != "state");
        extras.push(Extra {
            name: "block".to_owned(),
            conv: Conv::BlockFromState,
        });
    }
    Ok(Model {
        event: event_name(name),
        contract: contract.to_owned(),
        host,
        fields,
        extras,
        skipped,
        dispatch_params,
        out: out_read,
        cancel_flag,
        kind,
        lifetimes,
        _borrows: !struct_tys.is_empty()
            && struct_tys.iter().any(|t| {
                let (inner, borrowed) = deref(t);
                borrowed && !is_copied_mut(t) && !is_path(inner, "str")
            }),
        name: name.to_owned(),
    })
}

/// One sample per converter type: the values the smoke test fires and
/// asserts. `rust` builds the dispatch argument, `lua` is the literal the
/// handler asserts, `write_*` is the edited value (write-back fields only).
pub struct Sample {
    pub rust: String,
    pub lua: String,
    pub write_rust: Option<String>,
    pub write_lua: Option<String>,
    /// Needs a `let mut` binding (a `&mut` dispatch argument).
    pub mutable: bool,
}

/// `uuid` as `uuid_text` writes it: `8-4-4-4-12` hex digits.
pub fn uuid_sample(v: u128) -> String {
    let hex = format!("{v:032x}");
    [
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..],
    ]
    .join("-")
}

/// The handler lines asserting a field's sample: one `assert` per leaf, so
/// a table field checks every component it seeds. (`lua` carries the whole
/// literal; structured fields recompute their leaves from `n`, the same
/// index the samples use, so both sides agree.)
/// Lua field read: keywords (`end`, `in`, ...) are syntax errors after a
/// dot, so those use bracket access.
pub fn lua_read(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    if KEYWORDS.contains(&name) {
        format!("event[\"{name}\"]")
    } else {
        format!("event.{name}")
    }
}

pub fn assert_lines(conv: &Conv, field: &str, lua: &str, n: usize) -> Vec<String> {
    let a = |expr: &str| format!("    assert({expr}, \"{field}\")");
    let e = lua_read(field);
    match conv {
        Conv::Player | Conv::PlayerOpt => vec![
            a(&format!("{e}.uuid == {lua}")),
            a(&format!("{e}.name == \"test:name\"")),
        ],
        Conv::Pos | Conv::PosStruct | Conv::OptPos | Conv::MutPos => {
            let (x, y, z) = (1 + n as i32, 2 + n as i32, 3 + n as i32);
            vec![
                a(&format!("{e}.x == {x}")),
                a(&format!("{e}.y == {y}")),
                a(&format!("{e}.z == {z}")),
            ]
        }
        Conv::Chunk => vec![
            a(&format!("{e}.x == {}", 3 + n as i32)),
            a(&format!("{e}.z == {}", -5 - n as i32)),
        ],
        Conv::Pair | Conv::MutPair => vec![
            a(&format!("{e}.x == {}", 1.5 + n as f64)),
            a(&format!("{e}.y == {}", 2.5 + n as f64)),
        ],
        Conv::Vec3f | Conv::MutVec3 => vec![
            a(&format!("{e}.x == {}", 1.0 + n as f64)),
            a(&format!("{e}.y == 2.0")),
            a(&format!("{e}.z == 3.0")),
        ],
        Conv::IntSeq => vec![a(&format!("{e}[1] == {} and {e}[2] == {}", 4 + n, 9 + n))],
        Conv::StrSeq | Conv::MutStrVec => {
            vec![a(&format!("{e}[1] == \"test:seed\""))]
        }
        Conv::Item | Conv::OptItem => {
            vec![a(&format!("{e}.id == {lua}"))]
        }
        _ => vec![a(&format!("{e} == {lua}"))],
    }
}

/// The handler line editing a mutable field: the Rust side then asserts the
/// native value changed. Table fields replace the whole table; the string
/// list edits in place like a mod would.
pub fn mutate_line(conv: &Conv, field: &str, write_lua: &str) -> String {
    let e = lua_read(field);
    match conv {
        Conv::MutStrVec => {
            format!("    {e}[1] = \"test:edited\"; table.insert({e}, \"test:added\")")
        }
        _ => format!("    {e} = {write_lua}"),
    }
}

/// The seed for a plain scalar or string, from the parameter's real type:
/// numbers vary per field, strings use the field name, bools alternate.
fn plain_seed(ty: &str, field: &str, n: usize) -> (String, String) {
    let num = |base: i64| (base + n as i64).to_string();
    if ty.contains("f32") || ty.contains("f64") {
        let v = 1.5 + n as f64;
        (v.to_string(), v.to_string())
    } else if ty.contains("bool") {
        let v = n.is_multiple_of(2);
        (v.to_string(), v.to_string())
    } else if ty.contains("str") {
        (format!("\"test:{field}\""), format!("\"test:{field}\""))
    } else {
        (num(7), num(7))
    }
}

/// The edited value a write-back test leaves, from the seed.
fn plain_write(ty: &str, seed_lua: &str, _field: &str, n: usize) -> (String, String) {
    if ty.contains("f32") || ty.contains("f64") {
        let v = 15.5 + n as f64;
        (v.to_string(), v.to_string())
    } else if ty.contains("bool") {
        let v = seed_lua != "true";
        (v.to_string(), v.to_string())
    } else {
        let v = 50 + n as i64;
        (v.to_string(), v.to_string())
    }
}

pub fn sample(conv: &Conv, ty: &str, field: &str, n: usize) -> Sample {
    let plain = |rust: String, lua: String| Sample {
        rust,
        lua,
        write_rust: None,
        write_lua: None,
        mutable: false,
    };
    let mut_sample = |rust: String, lua: String, write_rust: String, write_lua: String| Sample {
        rust,
        lua,
        write_rust: Some(write_rust),
        write_lua: Some(write_lua),
        mutable: true,
    };
    match conv {
        Conv::Plain => {
            let (rust, lua) = plain_seed(ty, field, n);
            plain(rust, lua)
        }
        Conv::PlainString => plain(
            format!("\"test:{field}\".to_owned()"),
            format!("\"test:{field}\""),
        ),
        Conv::OptPlain => {
            let inner = ty.trim_start_matches("Option<").trim_end_matches('>');
            let (rust, lua) = plain_seed(inner, field, n);
            plain(format!("Some({rust})"), lua)
        }
        Conv::Uuid => plain(
            (7 + n).to_string(),
            format!("\"{}\"", uuid_sample((7 + n) as u128)),
        ),
        Conv::OptUuid => plain(
            format!("Some({})", 7 + n),
            format!("\"{}\"", uuid_sample((7 + n) as u128)),
        ),
        Conv::Player => plain(
            (7 + n).to_string(),
            format!("\"{}\"", uuid_sample((7 + n) as u128)),
        ),
        Conv::PlayerOpt => plain(
            format!("Some({})", 7 + n),
            format!("\"{}\"", uuid_sample((7 + n) as u128)),
        ),
        Conv::Dim => plain(
            "LevelDimension::Overworld".into(),
            "\"minecraft:overworld\"".into(),
        ),
        Conv::Pos => {
            let (x, y, z) = (1 + n as i32, 2 + n as i32, 3 + n as i32);
            plain(
                format!("[{x}, {y}, {z}]"),
                format!("{{ x = {x}, y = {y}, z = {z} }}"),
            )
        }
        Conv::PosStruct => {
            let (x, y, z) = (1 + n as i32, 2 + n as i32, 3 + n as i32);
            plain(
                format!("BlockPos::new({x}, {y}, {z})"),
                format!("{{ x = {x}, y = {y}, z = {z} }}"),
            )
        }
        Conv::OptPos => {
            let (x, y, z) = (1 + n as i32, 2 + n as i32, 3 + n as i32);
            plain(
                format!("Some([{x}, {y}, {z}])"),
                format!("{{ x = {x}, y = {y}, z = {z} }}"),
            )
        }
        Conv::Chunk => {
            let (x, z) = (3 + n as i32, -5 - n as i32);
            plain(format!("[{x}, {z}]"), format!("{{ x = {x}, z = {z} }}"))
        }
        Conv::Pair => {
            let (x, y) = (1.5 + n as f64, 2.5 + n as f64);
            plain(format!("[{x}, {y}]"), format!("{{ x = {x}, y = {y} }}"))
        }
        Conv::Vec3f => {
            let x = 1.0 + n as f64;
            plain(
                format!("[{x:?}, 2.0, 3.0]"),
                format!("{{ x = {x:?}, y = 2.0, z = 3.0 }}"),
            )
        }
        Conv::IntSeq => {
            let (a, b) = (4 + n, 9 + n);
            plain(
                format!("&[{a}, {b}]"),
                format!("{{ [{a}] = {a}, [{b}] = {b} }}"),
            )
        }
        Conv::StrSeq => plain(
            "&[\"test:seed\".to_owned()]".into(),
            "{ \"test:seed\" }".into(),
        ),
        Conv::Item => plain(
            format!(
                "&ItemStack {{ item: \"test:{field}\".into(), count: {}, damage: {}, extra: None }}",
                1 + n,
                n
            ),
            format!("\"test:{field}\""),
        ),
        Conv::OptItem => plain(
            format!(
                "Some(&ItemStack {{ item: \"test:{field}\".into(), count: 1, damage: 0, extra: None }})"
            ),
            format!("\"test:{field}\""),
        ),
        Conv::StateLevel | Conv::StateGame => plain(
            // Id 7 is stone (the spike's sample, kept by the fakes); the
            // model samples start at 8 so every state names itself.
            format!("BlockStateId({})", 8 + n),
            format!("\"test:state{}\"", 8 + n),
        ),
        // Extras never ride a dispatch call (no native parameter); the
        // tests assert the resolved block. The rust side is a placeholder.
        Conv::BlockAtPos | Conv::BlockFromState => {
            plain("BlockStateId(7)".into(), "\"minecraft:stone\"".into())
        }
        Conv::MutScalar => {
            // `&mut` scalars and `&mut Option` scalars: the seed copies in
            // and the edited value writes back.
            let inner = ty
                .trim_start_matches("&mut ")
                .trim_start_matches("Option<")
                .trim_end_matches('>');
            let (rust, lua) = plain_seed(inner, field, n);
            let (write_rust, write_lua) = plain_write(inner, &lua, field, n);
            let (rust, write_rust) = if ty.contains("Option<") {
                (format!("Some({rust})"), format!("Some({write_rust})"))
            } else {
                (rust, write_rust)
            };
            mut_sample(rust, lua, write_rust, write_lua)
        }
        Conv::MutString => mut_sample(
            format!("\"test:{field}\".to_owned()"),
            format!("\"test:{field}\""),
            format!("\"test:{field}:edited\".to_owned()"),
            format!("\"test:{field}:edited\""),
        ),
        Conv::MutOptString => mut_sample(
            format!("Some(\"test:{field}\".to_owned())"),
            format!("\"test:{field}\""),
            format!("Some(\"test:{field}:edited\".to_owned())"),
            format!("\"test:{field}:edited\""),
        ),
        Conv::MutPos => {
            let (x, y, z) = (1 + n as i32, 2 + n as i32, 3 + n as i32);
            let (a, b, c) = (x + 10, y + 10, z + 10);
            mut_sample(
                format!("[{x}, {y}, {z}]"),
                format!("{{ x = {x}, y = {y}, z = {z} }}"),
                format!("[{a}, {b}, {c}]"),
                format!("{{ x = {a}, y = {b}, z = {c} }}"),
            )
        }
        Conv::MutVec3 => {
            let x = 1.0 + n as f64;
            let w = x + 10.0;
            mut_sample(
                format!("[{x:?}, 2.0, 3.0]"),
                format!("{{ x = {x:?}, y = 2.0, z = 3.0 }}"),
                format!("[{w:?}, 2.0, 3.0]"),
                format!("{{ x = {w:?}, y = 2.0, z = 3.0 }}"),
            )
        }
        Conv::MutPair => {
            let (x, y) = (1.5 + n as f64, 2.5 + n as f64);
            let (a, b) = (x + 10.0, y + 10.0);
            mut_sample(
                format!("[{x}, {y}]"),
                format!("{{ x = {x}, y = {y} }}"),
                format!("[{a}, {b}]"),
                format!("{{ x = {a}, y = {b} }}"),
            )
        }
        Conv::MutStrVec => mut_sample(
            "vec![\"test:seed\".to_owned()]".into(),
            "{ \"test:seed\" }".into(),
            "vec![\"test:edited\".to_owned(), \"test:added\".to_owned()]".into(),
            "{ \"test:edited\", \"test:added\" }".into(),
        ),
    }
}

/// The `.d.lua` type of a replace reader.
pub fn replace_ts(read: ReplaceRead) -> &'static str {
    match read {
        ReplaceRead::Int => "number",
        ReplaceRead::Bool => "boolean",
        ReplaceRead::Pair => "Vec2",
        ReplaceRead::Item => "Item",
    }
}

/// The Rust literal a replace test writes.
pub fn replace_sample(read: ReplaceRead, n: usize) -> (String, String) {
    match read {
        ReplaceRead::Int => {
            let v = 81000 + n;
            (v.to_string(), v.to_string())
        }
        ReplaceRead::Bool => ("true".into(), "true".into()),
        ReplaceRead::Pair => ("{ x = 16, y = 8 }".into(), "{ x = 16, y = 8 }".into()),
        ReplaceRead::Item => (
            "{ id = \"test:result\", count = 1 }".into(),
            "{ id = \"test:result\", count = 1 }".into(),
        ),
    }
}

/// The hooks the hand-written runtime already serves (`LUA_HOOK_NAMES` in
/// `lua_mods.rs` plus every method its `impl Hooks for LuaMod` defines):
/// the native hooks of those names are not generated (the old hooks stay
/// as they are for now; removing them is #44). Generating them would define
/// the trait method twice.
fn handwritten_names(root: &Path) -> Result<Vec<String>, String> {
    let src = fs::read_to_string(root.join("crates/event-horizon-lua/src/lua_mods.rs"))
        .map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    let list = "pub const LUA_HOOK_NAMES";
    let start = src.find(list).ok_or("LUA_HOOK_NAMES missing")?;
    let end = start
        + src[start..]
            .find("];")
            .ok_or("LUA_HOOK_NAMES unterminated")?;
    names.extend(
        src[start..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned),
    );
    let file: syn::File = syn::parse_str(&src).map_err(|e| e.to_string())?;
    for item in &file.items {
        if let syn::Item::Impl(item) = item
            && let Some((_, trait_, _)) = &item.trait_
            && quote_trait_path(trait_) == "Hooks"
        {
            for inner in &item.items {
                if let syn::ImplItem::Fn(m) = inner {
                    names.push(m.sig.ident.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// The trait path of an `impl Trait for Type`, canonically rendered.
fn quote_trait_path(trait_: &syn::Path) -> String {
    trait_
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default()
}

/// Every hook in exactly one class, in catalog order.
pub fn classify(root: &Path) -> Result<Vec<HookClass>, String> {
    let catalog: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("native-hooks.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let hooks = catalog_hooks(root, &catalog)?;
    let game = game_methods(root)?;
    let handwritten = handwritten_names(root)?;
    let mut frees: BTreeMap<String, Vec<syn::ItemFn>> = BTreeMap::new();
    let mut out = Vec::new();
    for hook in &hooks {
        let name = hook["name"].as_str().ok_or("hook name missing")?.to_owned();
        if handwritten.contains(&name) {
            out.push(HookClass {
                class: Class::Cannot(format!(
                    "{name}: unsupported: the hand-written hook keeps this name (removing it is #44)"
                )),
                name,
            });
            continue;
        }
        match model_of(root, hook, &game, &mut frees) {
            Ok(model) => out.push(HookClass {
                name,
                class: Class::Generated(model),
            }),
            Err(class) => out.push(HookClass { name, class }),
        }
    }
    Ok(out)
}

/// `PascalCase` of a `snake_case` hook name, plus `Event`.
pub fn event_name(hook: &str) -> String {
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
