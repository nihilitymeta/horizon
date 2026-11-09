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
    /// Loot (`loot_drops` only): the block id or entity kind of the loot
    /// context, with the absent-both early-out (`loot_kind_key`).
    Loot,
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
    /// A `LuaStruct` type (plain struct, tuple, fieldless-enum name, text,
    /// JSON, duration, address, block position...): the runtime converts by
    /// the field's shape; `s_*`/`w_*` tags carry the native type.
    Struct,
    /// `&[u8]`: a Lua string (Luau strings are byte-safe).
    Bytes,
    /// `&Path`: its text.
    PathStr,
    /// `&mut Text`: the plain string plus the `_component` table.
    TextVal,
    /// `&mut Option<Text>`: nil or the plain string plus `_component`.
    TextOpt,
    /// `&mut Vec<Receiver<..>>`: `{count}` in, `refuse` out.
    PreLogin,
    /// `&NativeLifecycleContext`: seeds `authority` (the Debug name),
    /// `epoch` and `tick` flat, read-only.
    LifecycleCtx,
    /// `ServerTickPhase`: seeds `phase` (`"start"`/`"end"`) and
    /// `world_running`, read-only. Unknown phases never reach the seed: the
    /// arm suppresses them before building the table.
    TickPhase,
    /// `WorldPhase`: seeds `phase` (the Debug name), read-only.
    WorldPhaseName,
    /// `&NativeLootDrops`: seeds the nine context fields flat (`table`,
    /// `dimension`, `origin`, `block`, `entity`, `player`, `tool`,
    /// `explosion`, `used`), read-only.
    LootCtx,
    /// `&mut Vec<ItemStack>` of loot: seeded like a struct vec of stacks,
    /// written back by the component-keeping `loot_write` arm (a same-id
    /// entry keeps its stack), not plain `from_value`.
    LootDrops,
    /// A state field's `_props` note: seeded beside the state itself.
    StateProps,
}

/// How a `replace` out value reads back: nil stays `None`, a table or value
/// becomes `Some`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplaceRead {
    Int,
    Bool,
    Pair,
    Item,
    /// `Option<T>` for a `LuaStruct` `T`: nil or its table.
    Struct,
    /// `Option<Vec<T>>` for a `LuaStruct` `T`: nil or its sequence.
    StructVec,
    /// `Option<Vec<u8>>`: nil or a Lua string.
    Bytes,
    /// A placement answer: nil or a state of the placed block's own block.
    AnswerState,
}

/// One event-table field of a generated hook.
#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub conv: Conv,
    /// The `.d.lua` type of the field (`?` already applied).
    pub ts: String,
    /// The `LuaStruct` key, for `Conv::Struct` fields: what the runtime,
    /// samples and asserts look up.
    pub key: Option<String>,
}

/// An extra event-table field with no native parameter: the spike's hand
/// notes, kept so its tests assert what they always did.
#[derive(Clone, Debug)]
pub struct Extra {
    pub name: String,
    pub conv: Conv,
}

impl Model {
    /// The parameter the kind filter reads, if any: the `kind` field, the
    /// resolved entity id, or the loot context the kind derives from.
    pub fn kind_field(&self) -> Option<&str> {
        match &self.kind {
            Kind::Field => Some("kind"),
            Kind::Entity { field, .. } => Some(field.as_str()),
            Kind::Loot => Some("context"),
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
    /// A live handle only a design of its own can expose: the type name
    /// (`ModBlockEntity`, `Mob`).
    Handle(String),
    /// Raw protocol packets: a design of their own, not field tables.
    Protocol,
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

/// A `Vec` of borrowed string slices (`&str`, any lifetime): the seed reads
/// them like owned strings, the write interns them back to `&'static str`.
fn is_vec_of_str_ref(ty: &syn::Type) -> bool {
    if let syn::Type::Path(p) = ty
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Vec"
        && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return matches!(inner, syn::Type::Reference(r) if is_path(&r.elem, "str"));
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
            "&{}{}{}",
            if r.mutability.is_some() { "mut " } else { "" },
            r.lifetime
                .as_ref()
                .map(|l| format!("'{} ", l.ident))
                .unwrap_or_default(),
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
                // `'static` needs no event lifetime parameter (command names,
                // emissive layers); only elided and named-short lifetimes do.
                let is_static = r.lifetime.as_ref().is_some_and(|l| l.ident == "static");
                if !is_static {
                    if self.in_mut {
                        self.b = true;
                    } else {
                        self.a = true;
                    }
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
        // The original lifetimes: `'static` borrows take no parameter.
        lt.visit(ty);
    }
    match (lt.a, lt.b) {
        (_, true) => "<'_, '_>".to_owned(),
        (true, false) => "<'_>".to_owned(),
        (false, false) => String::new(),
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
    // Rich text crosses as its plain string plus the component table.
    if matches!(ident.as_deref(), Some("Text")) {
        return FieldOutcome::Conv(Conv::TextVal, "string".into());
    }
    if matches!(ident.as_deref(), Some("Compound" | "Value" | "Map")) {
        let ts = struct_ts(ident.as_deref().unwrap_or("?"));
        return FieldOutcome::Conv(Conv::Struct, ts.into());
    }
    if let Some(inner_t) = option_inner(inner) {
        let inner_id = last_ident(inner_t);
        if matches!(inner_id.as_deref(), Some("Text")) {
            return FieldOutcome::Conv(Conv::TextOpt, "string?".into());
        }
        if matches!(inner_id.as_deref(), Some("Compound" | "Value")) {
            let ts = struct_ts(inner_id.as_deref().unwrap_or("?"));
            return FieldOutcome::Conv(Conv::Struct, format!("{ts}?"));
        }
    }
    // Tuples map by their elements: a sequence in order.
    if matches!(inner, syn::Type::Tuple(_))
        && let Some((key, ts)) = tuple_struct(inner)
    {
        let _ = key;
        return FieldOutcome::Conv(Conv::Struct, ts);
    }
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
                let ts = struct_ts("Direction");
                return FieldOutcome::Conv(Conv::Struct, format!("{ts}?"));
            }
            Some("Hand") => {
                let ts = struct_ts("Hand");
                return FieldOutcome::Conv(Conv::Struct, format!("{ts}?"));
            }
            Some("BlockPos") => {
                return adapter("an optional pos needs its table form");
            }
            _ => {
                // Borrows keep their own arms below (`Option<&ItemStack>`
                // seeds like an optional stack); only owned inners struct up.
                if !matches!(inner_t, syn::Type::Reference(_))
                    && let Some(key) = struct_key_of(inner_t)
                {
                    let ts = struct_ts_owned(&key);
                    return FieldOutcome::Conv(Conv::Struct, format!("{ts}?"));
                }
            }
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
            Host::GameMut | Host::GameRef => FieldOutcome::Conv(Conv::StateGame, "string".into()),
            _ => adapter("a block state needs a host to name it"),
        },
        Some("BlockId") => adapter("a block id needs its name"),
        Some("Path" | "PathBuf") => FieldOutcome::Conv(Conv::PathStr, "string".into()),
        Some("Direction" | "Hand" | "UseAnimation") => {
            let ts = struct_ts(ident.as_deref().unwrap_or("?"));
            FieldOutcome::Conv(Conv::Struct, ts.into())
        }
        Some("ItemStack") if borrowed => FieldOutcome::Conv(Conv::Item, "Item".into()),
        Some("ItemStack") => FieldOutcome::Conv(Conv::Item, "Item".into()),
        Some("NativeLifecycleContext") if borrowed => {
            FieldOutcome::Conv(Conv::LifecycleCtx, "any".into())
        }
        Some("ServerTickPhase") if !borrowed => FieldOutcome::Conv(Conv::TickPhase, "any".into()),
        Some("WorldPhase") if !borrowed => FieldOutcome::Conv(Conv::WorldPhaseName, "any".into()),
        Some("NativeLootDrops") if borrowed => FieldOutcome::Conv(Conv::LootCtx, "any".into()),
        Some("ModBlockEntity" | "Mob") => FieldOutcome::Adapter(format!(
            "{}: handle: a {} handle needs its own design",
            hook,
            ident.as_deref().unwrap_or("?")
        )),
        Some("ClientboundPlayPacket" | "ServerboundPlayPacket") => FieldOutcome::Adapter(format!(
            "{hook}: protocol: raw packets need their own design"
        )),
        Some(id) if struct_key_of(inner).is_some() => {
            let _ = id;
            let key = struct_key_of(inner).unwrap_or_default();
            let ts = struct_ts_owned(&key);
            FieldOutcome::Conv(Conv::Struct, ts)
        }
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
    // The pre-login gate: mods answer refusals through it, never by
    // waiting on the Lua thread.
    if let Some(arg) = single_generic_arg(elem, &["Receiver"])
        && last_ident(arg).as_deref() == Some("Receiver")
    {
        return FieldOutcome::Conv(Conv::PreLogin, "{ count: number, refuse: string? }".into());
    }
    // The pre-login gate travels as `&mut Vec<Receiver<..>>`.
    if single_generic_arg(elem, &["Vec"])
        .is_some_and(|e| last_ident(e).as_deref() == Some("Receiver"))
    {
        return FieldOutcome::Conv(Conv::PreLogin, "{ count: number, refuse: string? }".into());
    }
    // Loot's drops: seeded like a struct vec of stacks, written back by the
    // component-keeping `loot_write` arm, not plain `from_value`.
    if hook == "loot_drops"
        && single_generic_arg(elem, &["Vec"]).is_some_and(|e| is_path(e, "ItemStack"))
    {
        return FieldOutcome::Conv(Conv::LootDrops, struct_ts_owned("Vec<ItemStack>"));
    }
    if mentions_stack(elem) {
        if let Some(key) = struct_key_of(elem) {
            let ts = struct_ts(&key);
            return FieldOutcome::Conv(Conv::Struct, ts.into());
        }
        return adapter(if is_out {
            "a stack out needs its table form"
        } else {
            "a mutable stack needs its table form"
        });
    }
    if is_path(elem, "Text") {
        return FieldOutcome::Conv(Conv::TextVal, "string".into());
    }
    if let Some(opt) = option_inner(elem)
        && last_ident(opt).as_deref() == Some("Text")
    {
        return FieldOutcome::Conv(Conv::TextOpt, "string?".into());
    }
    if let Some(key) = struct_key_of(elem) {
        let ts = struct_ts_owned(&key);
        return FieldOutcome::Conv(Conv::Struct, ts);
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
    if is_vec_of(elem, "String") || is_vec_of_str_ref(elem) {
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
                return FieldOutcome::Conv(Conv::Struct, struct_ts("[i32;2]").into());
            }
            (3, "bool") => {
                return FieldOutcome::Conv(Conv::Struct, struct_ts("[bool;3]").into());
            }
            (2, "usize") => {
                return FieldOutcome::Conv(Conv::Struct, struct_ts("[usize;2]").into());
            }
            _ if len == 8
                && let syn::Type::Array(inner) = &*array.elem
                && is_path(&inner.elem, "f32") =>
            {
                return FieldOutcome::Conv(Conv::Struct, struct_ts("[[f32;4];8]").into());
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
            return FieldOutcome::Conv(Conv::Bytes, "string".into());
        }
        if let Some(key) = struct_key_of(inner) {
            let ts = struct_ts_owned(&key);
            return FieldOutcome::Conv(Conv::Struct, ts);
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

/// The `LuaStruct` key of a native type, when the runtime converts it: plain
/// structs and tuples by Rust name, fieldless enums likewise, fixed arrays
/// and the JSON types by shape. Anything else (render borrows, handles, the
/// mesh a test cannot read) stays an adapter or cannot.
fn struct_key_of(ty: &syn::Type) -> Option<String> {
    const STRUCTS: &[&str] = &[
        "Duration",
        "SocketAddr",
        "Direction",
        "Hand",
        "UseAnimation",
        "Corner",
        "NativeFluidAction",
        "NativeConfigKind",
        "Text",
        "Compound",
        "Value",
        "Map",
        "ItemStack",
        "BlockPos",
        "LevelDimension",
        "NativeZoom",
        "NativeMinimap",
        "NativeMod",
        "NativeScreen",
        "ModContainer",
        "ModFluidTanks",
        "ModEnergyBuffer",
        "ItemEnergyBuffer",
        "NativeMapInfo",
        "NativeMapDecoration",
        "NativeDataMapType",
        "NativeConfig",
        "NativeConfigFile",
        "SpawnEntry",
        "RecipeBookCategory",
        "NativeReloadListener",
        "Suggestion",
        "ItemCost",
        "MerchantOffer",
        "AnvilCraft",
        "NativeNoteBlockChange",
        "NativeFluidStack",
        "NativeFluidDrain",
        "NativeFluidTankView",
        "Music",
        "MusicLayer",
        "SoundPlayback",
        "TickSpan",
        "BlockProperties",
        "PlacementContext",
        "FluidState",
    ];
    const ARRAYS: &[&str] = &[
        "[i32;2]",
        "[bool;3]",
        "[usize;2]",
        "[[f32;4];8]",
        "[f32;2]",
        "[i32;3]",
        "[f64;3]",
    ];
    const OPT_STRUCTS: &[&str] = &[
        "Music",
        "NativeZoom",
        "NativeMinimap",
        "NativeScreen",
        "ModContainer",
        "ModFluidTanks",
        "ModEnergyBuffer",
        "ItemEnergyBuffer",
        "ItemStack",
        "ItemCost",
        "NativeFluidStack",
        "[f64;3]",
        "Direction",
    ];
    if let Some(inner) = option_inner(ty) {
        let last = last_ident(inner);
        let id = last.as_deref().unwrap_or_default();
        if OPT_STRUCTS.contains(&id) {
            return Some(format!("Option<{id}>"));
        }
        if let Some((key, _)) = tuple_struct(inner) {
            return Some(format!("Option<{key}>"));
        }
        // A named tuple alias: the seed and sample use the tuple form.
        if last_ident(inner).as_deref() == Some("NativeDispensed") {
            return Some("Option<(Option<ItemStack>,i32)>".to_owned());
        }
        if let syn::Type::Array(_) = inner {
            let key = ty_str(inner).replace(' ', "");
            if ARRAYS.contains(&key.as_str()) {
                return Some(format!("Option<{key}>"));
            }
        }
        if let syn::Type::Path(_) = inner
            && let Some(key) = struct_key_of(inner)
            && key.starts_with("Vec<")
        {
            return Some(format!("Option<{key}>"));
        }
        return None;
    }
    if let syn::Type::Array(_) = ty {
        let key = ty_str(ty).replace(' ', "");
        if ARRAYS.contains(&key.as_str()) {
            return Some(key);
        }
        return None;
    }
    if let syn::Type::Slice(s) = ty {
        if is_path(&s.elem, "u8") || is_path(&s.elem, "BlockPos") || is_path(&s.elem, "TickSpan") {
            return Some(format!("&[{}]", ty_str(&s.elem).replace(' ', "")));
        }
        if let syn::Type::Array(a) = &*s.elem
            && is_path(&a.elem, "i32")
            && matches!(&a.len, syn::Expr::Lit(l) if matches!(&l.lit, syn::Lit::Int(n) if n.base10_digits() == "3"))
        {
            return Some("&[[i32;3]]".to_owned());
        }
        if let Some((key, _)) = tuple_struct(&s.elem) {
            return Some(format!("&[{key}]"));
        }
        return None;
    }
    if let syn::Type::Reference(r) = ty {
        if r.lifetime.as_ref().is_some_and(|l| l.ident == "static") && is_path(&r.elem, "str") {
            return Some("&'static str".to_owned());
        }
        return struct_key_of(&r.elem);
    }
    if let Some((key, _)) = tuple_struct(ty) {
        return Some(key);
    }
    if let syn::Type::Path(_) = ty
        && let Some(id) = last_ident(ty)
    {
        if STRUCTS.contains(&id.as_str()) {
            return Some(id);
        }
        // A named tuple alias: the seed and sample use the tuple form.
        if id == "NativeDispensed" {
            return Some("(Option<ItemStack>,i32)".to_owned());
        }
        if id == "Vec"
            && let Some(elem) = single_generic_arg(ty, &["Vec"])
            && let Some(key) = struct_key_of(elem)
            && !matches!(key.as_str(), "String" | "Option<String>" | "&'static str")
        {
            return Some(format!("Vec<{key}>"));
        }
    }
    None
}

/// The `.d.lua` type of a `LuaStruct` key.
fn struct_ts(key: &str) -> &'static str {
    match key {
        "Duration" => "number",
        "SocketAddr" => "string",
        "&'static str" => "string",
        "Direction" => "\"down\" | \"up\" | \"north\" | \"south\" | \"west\" | \"east\"",
        "Option<Direction>" => "\"down\" | \"up\" | \"north\" | \"south\" | \"west\" | \"east\"?",
        "Hand" => "\"main\" | \"off\"",
        "UseAnimation" => {
            "\"none\" | \"eat\" | \"drink\" | \"block\" | \"bow\" | \"trident\" | \"crossbow\" | \"spyglass\" | \"toot_horn\" | \"brush\" | \"bundle\" | \"spear\""
        }
        "Corner" => "\"top_left\" | \"top_right\" | \"bottom_left\" | \"bottom_right\"",
        "NativeFluidAction" => "\"simulate\" | \"execute\"",
        "NativeConfigKind" => "\"client\" | \"common\" | \"server\"",
        "Text" => "{ text: string, component: Component? }",
        "Compound" => "Data",
        "Value" | "Map" => "Data",
        "ItemStack" => "Item",
        "Option<ItemStack>" => "Item?",
        "ItemCost" => "{ stack: Item }",
        "Option<ItemCost>" => "{ stack: Item }?",
        "BlockPos" => "Vec3",
        "LevelDimension" => "string",
        "NativeZoom" => "{ factor: number, smooth: boolean, turn: number }",
        "NativeMinimap" => {
            "{ corner: string, size: number, blocks_per_pixel: number, rotate: boolean, round: boolean }"
        }
        "NativeMod" => {
            "{ id: string, title: string, description: string, keys: {{ key: string, label: string, default: string }} }"
        }
        "NativeScreen" => {
            "{ width: number, height: number, slots: {{ x: number, y: number }?}, inventory_y: number, inventory_x: number, own_labels: boolean, own_background: boolean }"
        }
        "ModContainer" => {
            "{ slots: number, faces: {{ number }}, inputs: { number }, outputs: { number }, menu: string? }"
        }
        "ModFluidTanks" => {
            "{ tanks: { number }, faces: {{ number }}, inputs: { number }, outputs: { number } }"
        }
        "ModEnergyBuffer" => {
            "{ capacity: number, max_insert: number, max_extract: number, insert_faces: { boolean }, extract_faces: { boolean } }"
        }
        "ItemEnergyBuffer" => "{ capacity: number, max_insert: number, max_extract: number }",
        "NativeMapInfo" => {
            "{ id: number, center: { x: number, z: number }, scale: number, dimension: string, locked: boolean, in_frame: boolean }"
        }
        "NativeMapDecoration" => {
            "{ sprite: string, x: number, y: number, rot: number, shown_on_frame: boolean }"
        }
        "NativeDataMapType" => "{ id: string, registry: string }",
        "NativeConfig" => "{ mod_id: string, kind: string, file: string, default: string }",
        "NativeConfigFile" => {
            "{ mod_id: string, kind: string, file: string, path: string, text: string }"
        }
        "SpawnEntry" => {
            "{ entity: string, weight: number, min: number, max: number, constant: boolean }"
        }
        "RecipeBookCategory" => "{ id: string, recipe_type: string, icon: string }",
        "NativeReloadListener" => "{ id: string, folder: string }",
        "Suggestion" => {
            "{ range: { start: number, end: number }, text: string, tooltip: string?, int_value: number? }"
        }
        "MerchantOffer" => {
            "{ base_cost_a: { stack: Item }, cost_b: { stack: Item }?, result: Item, uses: number, max_uses: number, reward_exp: boolean, special_price_diff: number, demand: number, price_multiplier: number, xp: number }"
        }
        "AnvilCraft" => "{ left: Item?, right: Item?, output: Item }",
        "NativeNoteBlockChange" => {
            "{ dimension: string, pos: Vec3, instrument: string, old_note: number }"
        }
        "NativeFluidStack" => "{ fluid: string, amount: number, components: Data }",
        "Option<NativeFluidStack>" => "{ fluid: string, amount: number, components: Data }?",
        "NativeFluidDrain" => {
            "{ kind: string, amount: number?, stack: { fluid: string, amount: number, components: Data }? }"
        }
        "NativeFluidTankView" => {
            "{ tank: number, content: { fluid: string, amount: number, components: Data }?, capacity: number, can_fill: boolean, can_drain: boolean }"
        }
        "Music" => {
            "{ sound: string, min_delay: number, max_delay: number, replace_current_music: boolean }"
        }
        "Option<Music>" => {
            "{ sound: string, min_delay: number, max_delay: number, replace_current_music: boolean }?"
        }
        "MusicLayer" => "{ sound: string, volume: number }",
        "SoundPlayback" => {
            "{ instance: number, identifier: string, source: number, position: Vec3, relative: boolean, starting: boolean }"
        }
        "TickSpan" => "{ name: string, ms: number, cpu_ms: number, depth: number }",
        "BlockProperties" => "{ [string]: string }",
        "PlacementContext" => {
            "{ pos: Vec3, hit_pos: Vec3, face: string, hit: Vec3, inside: boolean, yaw: number, pitch: number, sneaking: boolean, item: string, replace_clicked: boolean }"
        }
        "FluidState" => "{ kind: string, amount: number, falling: boolean }",
        "Option<[f64;3]>" => "Vec3?",
        "Option<[f32;2]>" => "Vec2?",
        "Option<(i32,u32)>" => "{ value: number, max: number }?",
        "NativeDispensed" | "(Option<ItemStack>,i32)" => "{ remainder: Item?, event: number }",
        "[i32;2]" => "{ x: number, y: number }",
        "[bool;3]" => "{ boolean }",
        "[usize;2]" => "{ start: number, end: number }",
        "(usize,usize)" => "{ start: number, end: number }",
        "[[f32;4];8]" => "{ { x: number, y: number, z: number, w: number } }",
        "[i32;3]" | "[f64;3]" | "[f32;3]" => "Vec3",
        "[f32;2]" => "Vec2",
        "(Text,u32)" => "{ text: string, component: Component?, count: number }",
        "(i32,u32)" => "{ value: number, max: number }",
        "(i32,i32)" => "{ min: number, max: number }",
        "(String,Value)" => "{ path: string, data: Data }",
        "(String,Vec<u8>)" => "{ path: string, data: string }",
        "(String,Compound)" => "{ name: string, tag: Data }",
        "(&'staticstr,&'staticstr)" => "{ a: string, b: string }",
        _ => "any",
    }
}

/// The smoke-test seed, handler literal, edited pair and leaf asserts of a
/// `LuaStruct` key: what dispatch builds, what the handler sees, what the
/// handler writes back, and what the Rust side checks.
#[derive(Clone)]
struct StructSample {
    rust: String,
    lua: String,
    write_rust: Option<String>,
    write_lua: Option<String>,
    leaves: Vec<(String, String)>,
}

impl StructSample {
    fn clone_write(&self) -> Option<(String, String)> {
        match (&self.write_rust, &self.write_lua) {
            (Some(rust), Some(lua)) => Some((rust.clone(), lua.clone())),
            _ => None,
        }
    }
}

/// Two elements for a sequence sample: the seeds at `n` and `n + 1`, the
/// write at `n + 50`. Slices seed `&[..]`; vecs seed `vec![..]`.
fn struct_vec_sample(inner: &str, field: &str, n: usize, borrowed: bool) -> StructSample {
    let e1 =
        struct_sample(inner, field, n).unwrap_or_else(|| panic!("no struct sample for {inner}"));
    let e2 = struct_sample(inner, field, n + 1)
        .unwrap_or_else(|| panic!("no struct sample for {inner}"));
    let e3 = struct_sample(inner, field, n + 50)
        .unwrap_or_else(|| panic!("no struct sample for {inner}"));
    let rust = if borrowed {
        format!("&[{}, {}]", e1.rust, e2.rust)
    } else {
        format!("vec![{}, {}]", e1.rust, e2.rust)
    };
    let mut leaves = Vec::new();
    for (suffix, value) in &e1.leaves {
        leaves.push((format!("[1]{suffix}"), value.clone()));
    }
    for (suffix, value) in &e2.leaves {
        leaves.push((format!("[2]{suffix}"), value.clone()));
    }
    // The write-back element prefers its edited pair (a tooltip line
    // reads back as the spans form, not the seed's literal); an
    // option's clear (`None`) is a field write, not an element.
    let edited = match &e3.write_rust {
        Some(rust) if rust != "None" => e3.write_lua.clone().map(|lua| (rust.clone(), lua)),
        _ => None,
    };
    let (wrust, wlua) = edited.unwrap_or((e3.rust, e3.lua));
    StructSample {
        rust,
        lua: format!("{{ {}, {} }}", e1.lua, e2.lua),
        write_rust: Some(format!("vec![{wrust}]")),
        write_lua: Some(format!("{{ {wlua} }}")),
        leaves,
    }
}

/// The canonical write-back element of a sequence field at sample index `n`,
/// and whether it is the edited pair: the edited pair when the shape has one
/// (a tooltip line reads back as the spans form, not the seed's literal),
/// else the seed element itself. Duo tests append one per mod, so both
/// contributions stay valid write-backs. Only one sequence layer is stripped:
/// an `Option` element stays an element (`Some(..)`), matching the seed the
/// dispatch builds.
pub fn canonical_elem(key: &str, field: &str, n: usize) -> Option<(String, String, bool)> {
    let mut elem = key;
    if let Some(inner) = elem.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
        elem = inner;
    } else if let Some(inner) = elem.strip_prefix("&[").and_then(|s| s.strip_suffix(']')) {
        elem = inner;
    }
    let s = struct_sample(elem, field, n)?;
    match (&s.write_rust, &s.write_lua) {
        (Some(rust), Some(lua)) if rust != "None" => Some((rust.clone(), lua.clone(), true)),
        _ => Some((s.rust, s.lua, false)),
    }
}

fn struct_sample(key: &str, field: &str, n: usize) -> Option<StructSample> {
    if let Some(inner) = key
        .strip_prefix("Option<")
        .and_then(|k| k.strip_suffix('>'))
    {
        let s = struct_sample(inner, field, n)?;
        return Some(StructSample {
            rust: format!("Some({})", s.rust),
            lua: s.lua,
            write_rust: Some("None".into()),
            write_lua: Some("nil".into()),
            leaves: s.leaves,
        });
    }
    if let Some(inner) = key.strip_prefix("Vec<").and_then(|k| k.strip_suffix('>')) {
        return Some(struct_vec_sample(inner, field, n, false));
    }
    if let Some(inner) = key.strip_prefix("&[").and_then(|k| k.strip_suffix(']')) {
        return Some(struct_vec_sample(inner, field, n, true));
    }
    let mutable = |rust: String, lua: String, write_rust: String, write_lua: String| StructSample {
        rust,
        lua,
        write_rust: Some(write_rust),
        write_lua: Some(write_lua),
        leaves: Vec::new(),
    };
    let fixed = |rust: String, lua: String, leaves: Vec<(String, String)>| StructSample {
        rust,
        lua,
        write_rust: None,
        write_lua: None,
        leaves,
    };
    let leaf = |suffix: &str, value: String| (format!(".{suffix}"), value);
    let item = |count: i32, damage: i32| {
        format!(
            "ItemStack {{ item: \"test:{field}\".into(), count: {count}, damage: {damage}, extra: None }}"
        )
    };
    let text_lua =
        |plain: &str| format!("{{ text = \"{plain}\", component = {{ text = \"{plain}\" }} }}",);
    let s = match key {
        "Duration" => fixed(
            format!("Duration::from_secs_f64({})", 1.5 + n as f64),
            format!("{}", 1.5 + n as f64),
            vec![],
        ),
        "SocketAddr" => fixed(
            format!("\"127.0.0.1:{}\".parse().unwrap()", 25565 + n),
            format!("\"127.0.0.1:{}\"", 25565 + n),
            vec![],
        ),
        "Direction" => {
            let names = ["down", "up", "north", "south", "west", "east"];
            let rusts = ["Down", "Up", "North", "South", "West", "East"];
            let i = n % 6;
            fixed(
                format!("Direction::{}", rusts[i]),
                format!("\"{}\"", names[i]),
                vec![],
            )
        }
        "Hand" => fixed(
            if n.is_multiple_of(2) {
                "Hand::Main".into()
            } else {
                "Hand::Off".into()
            },
            if n.is_multiple_of(2) {
                "\"main\"".into()
            } else {
                "\"off\"".into()
            },
            vec![],
        ),
        "Corner" => {
            let names = ["top_left", "top_right", "bottom_left", "bottom_right"];
            let rusts = ["TopLeft", "TopRight", "BottomLeft", "BottomRight"];
            let i = n % 4;
            fixed(
                format!("Corner::{}", rusts[i]),
                format!("\"{}\"", names[i]),
                vec![],
            )
        }
        "NativeFluidAction" => {
            if n.is_multiple_of(2) {
                fixed(
                    "NativeFluidAction::Simulate".into(),
                    "\"simulate\"".into(),
                    vec![],
                )
            } else {
                fixed(
                    "NativeFluidAction::Execute".into(),
                    "\"execute\"".into(),
                    vec![],
                )
            }
        }
        "NativeConfigKind" => {
            let names = ["client", "common", "server"];
            let rusts = ["Client", "Common", "Server"];
            let i = n % 3;
            fixed(
                format!("NativeConfigKind::{}", rusts[i]),
                format!("\"{}\"", names[i]),
                vec![],
            )
        }
        "UseAnimation" => mutable(
            "UseAnimation::Block".into(),
            "\"block\"".into(),
            "UseAnimation::Drink".into(),
            "\"drink\"".into(),
        ),
        "Text" => {
            // The write-back parses the component table, so the Rust side
            // is the spans form, not the seed's literal.
            let mut s = mutable(
                format!("Text::literal(\"test:line{n}\")"),
                text_lua(&format!("test:line{n}")),
                format!(
                    "Text::empty().with_component(serde_json::json!({{ \"text\": \"test:line{n}\" }}))"
                ),
                text_lua(&format!("test:line{n}")),
            );
            // The nested component carries the same text (see
            // `DUP_LEAF_ALLOWLIST`): still asserted, for the shape.
            s.leaves = vec![
                leaf("text", format!("\"test:line{n}\"")),
                (".component.text".into(), format!("\"test:line{n}\"")),
            ];
            s
        }
        "Compound" => fixed(
            format!(
                "Compound::from([(\"testkey\".into(), Tag::Int({}))])",
                7 + n as i32
            ),
            format!("{{ testkey = {} }}", 7 + n as i32),
            vec![leaf("testkey", format!("{}", 7 + n as i32))],
        ),
        "Value" => {
            let mut s = mutable(
                format!("serde_json::json!({{ \"testkey\": {} }})", 7 + n as i32),
                format!("{{ testkey = {} }}", 7 + n as i32),
                format!("serde_json::json!({{ \"testkey\": {} }})", 50 + n as i32),
                format!("{{ testkey = {} }}", 50 + n as i32),
            );
            s.leaves = vec![leaf("testkey", format!("{}", 7 + n as i32))];
            s
        }
        "Map" => {
            let mut s = mutable(
                format!(
                    "[(\"testkey\".to_owned(), serde_json::json!({}))].into_iter().collect()",
                    7 + n as i32
                ),
                format!("{{ testkey = {} }}", 7 + n as i32),
                format!(
                    "[(\"testkey\".to_owned(), serde_json::json!({}))].into_iter().collect()",
                    50 + n as i32
                ),
                format!("{{ testkey = {} }}", 50 + n as i32),
            );
            s.leaves = vec![leaf("testkey", format!("{}", 7 + n as i32))];
            s
        }
        "ItemStack" => {
            // The struct seed carries components (a repair cost), so `extra`
            // and the derived `plain` both assert non-default values: a
            // dropped `extra` key and a wrong `plain` each fail. The
            // borrowed `Conv::Item` seeds below stay plain (`extra` nil,
            // `plain` true), pinning the other direction. The write-back
            // still replaces the whole table with a plain stack, so the
            // write pair keeps `extra: None`.
            let repair = 20 + n as i32;
            let mut s = mutable(
                format!(
                    "ItemStack {{ item: \"test:{field}\".into(), count: {}, damage: {n}, extra: Some(Box::new(StackData {{ repair_cost: {repair}, ..Default::default() }})) }}",
                    1 + n as i32
                ),
                format!(
                    "{{ id = \"test:{field}\", count = {}, damage = {n}, extra = {{ [\"minecraft:repair_cost\"] = {repair} }}, plain = false }}",
                    1 + n as i32
                ),
                item(50 + n as i32, n as i32),
                format!(
                    "{{ id = \"test:{field}\", count = {}, damage = {n} }}",
                    50 + n as i32
                ),
            );
            s.leaves = vec![
                leaf("id", format!("\"test:{field}\"")),
                leaf("count", format!("{}", 1 + n as i32)),
                leaf("damage", format!("{n}")),
                (
                    ".extra[\"minecraft:repair_cost\"]".into(),
                    format!("{repair}"),
                ),
                leaf("plain", "false".into()),
            ];
            s
        }
        "Option<ItemStack>" => mutable(
            format!("Some({})", item(1, 0)),
            format!("{{ id = \"test:{field}\", count = 1, damage = 0 }}"),
            "None".into(),
            "nil".into(),
        ),
        "[f64;3]" => {
            // Every component varies with `n`: a constant `y` would collide
            // with `x` for some `n`, and swapping the two would pass.
            let mut s = mutable(
                format!(
                    "[{:?}, {:?}, {:?}]",
                    1.0 + n as f64,
                    2.0 + n as f64,
                    3.0 + n as f64
                ),
                format!(
                    "{{ x = {:?}, y = {:?}, z = {:?} }}",
                    1.0 + n as f64,
                    2.0 + n as f64,
                    3.0 + n as f64
                ),
                format!(
                    "[{:?}, {:?}, {:?}]",
                    11.0 + n as f64,
                    12.0 + n as f64,
                    13.0 + n as f64
                ),
                format!(
                    "{{ x = {:?}, y = {:?}, z = {:?} }}",
                    11.0 + n as f64,
                    12.0 + n as f64,
                    13.0 + n as f64
                ),
            );
            s.leaves = vec![
                leaf("x", format!("{:?}", 1.0 + n as f64)),
                leaf("y", format!("{:?}", 2.0 + n as f64)),
                leaf("z", format!("{:?}", 3.0 + n as f64)),
            ];
            s
        }
        "[f32;3]" => {
            // Every component varies with `n`, as for `[f64;3]` above.
            let mut s = mutable(
                format!(
                    "[{:?}, {:?}, {:?}]",
                    1.0 + n as f32,
                    2.0 + n as f32,
                    3.0 + n as f32
                ),
                format!(
                    "{{ x = {:?}, y = {:?}, z = {:?} }}",
                    1.0 + n as f32,
                    2.0 + n as f32,
                    3.0 + n as f32
                ),
                format!(
                    "[{:?}, {:?}, {:?}]",
                    11.0 + n as f32,
                    12.0 + n as f32,
                    13.0 + n as f32
                ),
                format!(
                    "{{ x = {:?}, y = {:?}, z = {:?} }}",
                    11.0 + n as f32,
                    12.0 + n as f32,
                    13.0 + n as f32
                ),
            );
            s.leaves = vec![
                leaf("x", format!("{:?}", 1.0 + n as f32)),
                leaf("y", format!("{:?}", 2.0 + n as f32)),
                leaf("z", format!("{:?}", 3.0 + n as f32)),
            ];
            s
        }
        "[f32;2]" => {
            // Both components vary with `n`, as for `[f64;3]` above.
            let mut s = mutable(
                format!("[{:?}, {:?}]", 1.0 + n as f32, 2.0 + n as f32),
                format!("{{ x = {:?}, y = {:?} }}", 1.0 + n as f32, 2.0 + n as f32),
                format!("[{:?}, {:?}]", 11.0 + n as f32, 12.0 + n as f32),
                format!("{{ x = {:?}, y = {:?} }}", 11.0 + n as f32, 12.0 + n as f32),
            );
            s.leaves = vec![
                leaf("x", format!("{:?}", 1.0 + n as f32)),
                leaf("y", format!("{:?}", 2.0 + n as f32)),
            ];
            s
        }
        "Option<[f64;3]>" => mutable(
            format!("Some([{:?}, 2.0, 3.0])", 1.0 + n as f64),
            format!("{{ x = {:?}, y = 2.0, z = 3.0 }}", 1.0 + n as f64),
            "None".into(),
            "nil".into(),
        ),
        "[i32;2]" => fixed(
            format!("[{}, {}]", 3 + n as i32, -5 - n as i32),
            format!("{{ x = {}, y = {} }}", 3 + n as i32, -5 - n as i32),
            vec![
                leaf("x", format!("{}", 3 + n as i32)),
                leaf("y", format!("{}", -5 - n as i32)),
            ],
        ),
        "[bool;3]" => fixed(
            format!("[{}, {}, {}]", n.is_multiple_of(2), n % 2 == 1, false),
            format!("{{ {}, {}, false }}", n.is_multiple_of(2), n % 2 == 1),
            vec![
                ("[1]".into(), format!("{}", n.is_multiple_of(2))),
                ("[2]".into(), format!("{}", n % 2 == 1)),
                ("[3]".into(), "false".into()),
            ],
        ),
        "[usize;2]" => fixed(
            format!("[{}, {}]", 7 + n, 12 + n),
            format!("{{ start = {}, [\"end\"] = {} }}", 7 + n, 12 + n),
            // `end` is a keyword, so the assert reads it with brackets.
            vec![
                leaf("start", format!("{}", 7 + n)),
                ("[\"end\"]".into(), format!("{}", 12 + n)),
            ],
        ),
        "(usize,usize)" => fixed(
            format!("({}, {})", 7 + n, 12 + n),
            format!("{{ start = {}, [\"end\"] = {} }}", 7 + n, 12 + n),
            vec![
                leaf("start", format!("{}", 7 + n)),
                ("[\"end\"]".into(), format!("{}", 12 + n)),
            ],
        ),
        "[[f32;4];8]" => {
            // Every row and every component is its own value (row `i`
            // counts up from `n` in tens): zeroing `y`/`z`/`w` or reversing
            // the rows changes an asserted leaf. Eight different write `x`
            // values, so a permuted write fails the Rust-side assert too.
            let seed_row = |i: usize| {
                let (x, y, z, w) = (
                    1.5 + n as f32 + 10.0 * i as f32,
                    2.5 + n as f32 + 10.0 * i as f32,
                    3.5 + n as f32 + 10.0 * i as f32,
                    4.5 + n as f32 + 10.0 * i as f32,
                );
                (
                    format!("[{x:?}, {y:?}, {z:?}, {w:?}]"),
                    format!("{{ x = {x:?}, y = {y:?}, z = {z:?}, w = {w:?} }}"),
                )
            };
            let seed_rows: Vec<(String, String)> = (0..8).map(seed_row).collect();
            let seed_rust = format!(
                "[{}]",
                seed_rows
                    .iter()
                    .map(|(rust, _)| rust.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let seed_lua = format!(
                "{{ {} }}",
                seed_rows
                    .iter()
                    .map(|(_, lua)| lua.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            let write_x: Vec<f32> = (0..8).map(|i| 11.5 + n as f32 + i as f32).collect();
            let write_rust = format!(
                "[{}]",
                write_x
                    .iter()
                    .map(|x| format!("[{x:?}, 2.5, 3.5, 4.5]"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let write_lua = format!(
                "{{ {} }}",
                write_x
                    .iter()
                    .map(|x| format!("{{ x = {x:?}, y = 2.5, z = 3.5, w = 4.5 }}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            let mut s = mutable(seed_rust, seed_lua, write_rust, write_lua);
            s.leaves = (0..8)
                .flat_map(|i: usize| {
                    let idx = i + 1;
                    ["x", "y", "z", "w"].map(|c| {
                        let v = match c {
                            "x" => 1.5 + n as f32 + 10.0 * i as f32,
                            "y" => 2.5 + n as f32 + 10.0 * i as f32,
                            "z" => 3.5 + n as f32 + 10.0 * i as f32,
                            _ => 4.5 + n as f32 + 10.0 * i as f32,
                        };
                        (format!("[{idx}].{c}"), format!("{v:?}"))
                    })
                })
                .collect();
            s
        }
        "[i32;3]" => {
            let mut s = mutable(
                format!("[{}, {}, {}]", 1 + n as i32, 2 + n as i32, 3 + n as i32),
                format!(
                    "{{ x = {}, y = {}, z = {} }}",
                    1 + n as i32,
                    2 + n as i32,
                    3 + n as i32
                ),
                format!("[{}, {}, {}]", 11 + n as i32, 12 + n as i32, 13 + n as i32),
                format!(
                    "{{ x = {}, y = {}, z = {} }}",
                    11 + n as i32,
                    12 + n as i32,
                    13 + n as i32
                ),
            );
            s.leaves = vec![
                leaf("x", format!("{}", 1 + n as i32)),
                leaf("y", format!("{}", 2 + n as i32)),
                leaf("z", format!("{}", 3 + n as i32)),
            ];
            s
        }
        "BlockPos" => fixed(
            format!(
                "BlockPos::new({}, {}, {})",
                1 + n as i32,
                2 + n as i32,
                3 + n as i32
            ),
            format!(
                "{{ x = {}, y = {}, z = {} }}",
                1 + n as i32,
                2 + n as i32,
                3 + n as i32
            ),
            vec![
                leaf("x", format!("{}", 1 + n as i32)),
                leaf("y", format!("{}", 2 + n as i32)),
                leaf("z", format!("{}", 3 + n as i32)),
            ],
        ),
        "&'static str" => fixed(
            format!("\"test:seed{n}\""),
            format!("\"test:seed{n}\""),
            vec![],
        ),
        "(Text,u32)" => {
            // The write-back parses the component table, so the Rust side
            // is the spans form, not the seed's literal.
            let spans = |n: usize| {
                format!(
                    "Text::empty().with_component(serde_json::json!({{ \"text\": \"test:line{n}\" }}))"
                )
            };
            // (`struct_vec_sample` shifts `n` by 50 for the write, so the
            // write pair below uses the same `n` as the seed pair.)
            let mut s = mutable(
                format!("(Text::literal(\"test:line{n}\"), {})", 2 + n as u32),
                format!(
                    "{{ text = \"test:line{n}\", component = {{ text = \"test:line{n}\" }}, count = {} }}",
                    2 + n as u32
                ),
                format!("({}, {})", spans(n), 2 + n as u32),
                format!(
                    "{{ text = \"test:line{n}\", component = {{ text = \"test:line{n}\" }}, count = {} }}",
                    2 + n as u32
                ),
            );
            s.leaves = vec![
                leaf("text", format!("\"test:line{n}\"")),
                // The nested component carries the same text (see
                // `DUP_LEAF_ALLOWLIST`): still asserted, for the shape.
                (".component.text".into(), format!("\"test:line{n}\"")),
                leaf("count", format!("{}", 2 + n as u32)),
            ];
            s
        }
        "(i32,u32)" => fixed(
            format!("({}, 2)", 7 + n),
            format!("{{ value = {}, max = 2 }}", 7 + n as i32),
            vec![
                leaf("value", format!("{}", 7 + n as i32)),
                leaf("max", "2".into()),
            ],
        ),
        "(i32,i32)" => fixed(
            format!("({}, {})", 1 + n, 2 + n),
            format!("{{ min = {}, max = {} }}", 1 + n as i32, 2 + n as i32),
            vec![
                leaf("min", format!("{}", 1 + n as i32)),
                leaf("max", format!("{}", 2 + n as i32)),
            ],
        ),
        "NativeDispensed" | "(Option<ItemStack>,i32)" => fixed(
            format!("(Some({}), {})", item(1, 0), 2000 + n),
            format!(
                "{{ remainder = {{ id = \"test:{field}\", count = 1, damage = 0 }}, event = {} }}",
                2000 + n
            ),
            vec![
                leaf("remainder.id", format!("\"test:{field}\"")),
                leaf("remainder.count", "1".into()),
                leaf("remainder.damage", "0".into()),
                leaf("event", format!("{}", 2000 + n)),
            ],
        ),
        "(String,Value)" => fixed(
            format!(
                "(\"test:path{n}\".to_owned(), serde_json::json!({}))",
                7 + n as i32
            ),
            format!("{{ path = \"test:path{n}\", data = {} }}", 7 + n as i32),
            vec![
                leaf("path", format!("\"test:path{n}\"")),
                leaf("data", format!("{}", 7 + n as i32)),
            ],
        ),
        // A data-pack file: the path and the raw bytes as a Lua string.
        // Mutable (a collect writes it back), with a distinct edited pair.
        "(String,Vec<u8>)" => {
            let mut s = mutable(
                format!("(\"test:path{n}\".to_owned(), vec![116u8, 0u8, 159u8])"),
                format!("{{ path = \"test:path{n}\", data = \"t\" .. string.char(0, 159) }}"),
                format!("(\"test:path{n}:edited\".to_owned(), vec![119u8, 1u8, 2u8])"),
                format!("{{ path = \"test:path{n}:edited\", data = \"w\" .. string.char(1, 2) }}"),
            );
            s.leaves = vec![
                leaf("path", format!("\"test:path{n}\"")),
                leaf("data", "\"t\" .. string.char(0, 159)".into()),
            ];
            s
        }
        "(String,Compound)" => fixed(
            format!(
                "(\"test:entity{n}\".to_owned(), Compound::from([(\"testkey\".into(), Tag::Int({}))]))",
                7 + n as i32
            ),
            format!(
                "{{ name = \"test:entity{n}\", tag = {{ testkey = {} }} }}",
                7 + n as i32
            ),
            vec![
                leaf("name", format!("\"test:entity{n}\"")),
                (".tag.testkey".into(), format!("{}", 7 + n as i32)),
            ],
        ),
        "(&'staticstr,&'staticstr)" | "EmissiveLayer" => fixed(
            format!("(\"test:a{n}\", \"test:b{n}\")"),
            format!("{{ a = \"test:a{n}\", b = \"test:b{n}\" }}"),
            vec![
                leaf("a", format!("\"test:a{n}\"")),
                leaf("b", format!("\"test:b{n}\"")),
            ],
        ),
        "NativeZoom" => fixed(
            format!(
                "NativeZoom {{ factor: {:?}, smooth: {}, turn: {:?} }}",
                1.5 + n as f32,
                n.is_multiple_of(2),
                2.5
            ),
            format!(
                "{{ factor = {:?}, smooth = {}, turn = 2.5 }}",
                1.5 + n as f32,
                n.is_multiple_of(2)
            ),
            vec![leaf("factor", format!("{:?}", 1.5 + n as f32))],
        ),
        "NativeMinimap" => fixed(
            format!(
                "NativeMinimap {{ corner: Corner::TopLeft, size: {:?}, blocks_per_pixel: {:?}, rotate: {}, round: {} }}",
                64.0 + n as f32,
                1.0,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            format!(
                "{{ corner = \"top_left\", size = {:?}, blocks_per_pixel = 1.0, rotate = {}, round = {} }}",
                64.0 + n as f32,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            vec![leaf("corner", "\"top_left\"".into())],
        ),
        "NativeMod" => fixed(
            format!(
                "NativeMod {{ id: \"test:mod{n}\".into(), title: \"test:title{n}\".into(), description: \"test:desc{n}\".into(), keys: vec![[\"test:key{n}\".into(), \"test:label{n}\".into(), \"test:default{n}\".into()]] }}"
            ),
            format!(
                "{{ id = \"test:mod{n}\", title = \"test:title{n}\", description = \"test:desc{n}\", keys = {{ {{ key = \"test:key{n}\", label = \"test:label{n}\", default = \"test:default{n}\" }} }} }}"
            ),
            vec![
                leaf("id", format!("\"test:mod{n}\"")),
                leaf("title", format!("\"test:title{n}\"")),
                leaf("description", format!("\"test:desc{n}\"")),
                leaf("keys[1].key", format!("\"test:key{n}\"")),
                leaf("keys[1].label", format!("\"test:label{n}\"")),
                leaf("keys[1].default", format!("\"test:default{n}\"")),
            ],
        ),
        "NativeScreen" => fixed(
            format!(
                "NativeScreen {{ width: {:?}, height: {:?}, slots: vec![Some([1.0, 2.0]), None], inventory_y: {:?}, inventory_x: {:?}, own_labels: {}, own_background: {} }}",
                100.0 + n as f32,
                50.0,
                10.0,
                20.0,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            format!(
                "{{ width = {:?}, height = 50.0, slots = {{ {{ x = 1.0, y = 2.0 }}, false }}, inventory_y = 10.0, inventory_x = 20.0, own_labels = {}, own_background = {} }}",
                100.0 + n as f32,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            vec![leaf("width", format!("{:?}", 100.0 + n as f32))],
        ),
        "ModContainer" => fixed(
            format!(
                "ModContainer {{ slots: {}, faces: [vec![0], vec![], vec![], vec![], vec![], vec![]], inputs: vec![0], outputs: vec![], menu: None }}",
                9 + n
            ),
            format!(
                "{{ slots = {}, faces = {{ {{ 0 }}, {{}}, {{}}, {{}}, {{}}, {{}} }}, inputs = {{ 0 }}, outputs = {{}}, menu = nil }}",
                9 + n
            ),
            vec![leaf("slots", format!("{}", 9 + n))],
        ),
        "ModFluidTanks" => fixed(
            format!(
                "ModFluidTanks {{ tanks: vec![{}], faces: [vec![0], vec![], vec![], vec![], vec![], vec![]], inputs: vec![0], outputs: vec![] }}",
                1000 + n as i32
            ),
            format!(
                "{{ tanks = {{ {} }}, faces = {{ {{ 0 }}, {{}}, {{}}, {{}}, {{}}, {{}} }}, inputs = {{ 0 }}, outputs = {{}} }}",
                1000 + n as i32
            ),
            vec![],
        ),
        "ModEnergyBuffer" => fixed(
            format!(
                "ModEnergyBuffer {{ capacity: {}, max_insert: {}, max_extract: {}, insert_faces: [true, false, false, false, false, false], extract_faces: [false; 6] }}",
                1000 + n as i32,
                100 + n as i32,
                100 + n as i32
            ),
            format!(
                "{{ capacity = {}, max_insert = {}, max_extract = {}, insert_faces = {{ true, false, false, false, false, false }}, extract_faces = {{ false, false, false, false, false, false }} }}",
                1000 + n as i32,
                100 + n as i32,
                100 + n as i32
            ),
            vec![],
        ),
        "ItemEnergyBuffer" => fixed(
            format!(
                "ItemEnergyBuffer {{ capacity: {}, max_insert: {}, max_extract: {} }}",
                1000 + n as i32,
                100 + n as i32,
                100 + n as i32
            ),
            format!(
                "{{ capacity = {}, max_insert = {}, max_extract = {} }}",
                1000 + n as i32,
                100 + n as i32,
                100 + n as i32
            ),
            vec![],
        ),
        "NativeMapInfo" => fixed(
            format!(
                "NativeMapInfo {{ id: {}, center: [{}, {}], scale: 0, dimension: \"minecraft:overworld\", locked: {}, in_frame: {} }}",
                7 + n as i32,
                3 + n as i32,
                -5 - n as i32,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            format!(
                "{{ id = {}, center = {{ x = {}, z = {} }}, scale = 0, dimension = \"minecraft:overworld\", locked = {}, in_frame = {} }}",
                7 + n as i32,
                3 + n as i32,
                -5 - n as i32,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            vec![
                leaf("id", format!("{}", 7 + n as i32)),
                leaf("center.x", format!("{}", 3 + n as i32)),
                leaf("center.z", format!("{}", -5 - n as i32)),
                leaf("scale", "0".into()),
                leaf("dimension", "\"minecraft:overworld\"".into()),
                leaf("locked", format!("{}", n.is_multiple_of(2))),
                leaf("in_frame", format!("{}", n % 2 == 1)),
            ],
        ),
        // `y` varies with `n`: a constant `y = 2` would collide with `x`
        // for `n = 1`, and swapping the two would pass.
        "NativeMapDecoration" => fixed(
            format!(
                "NativeMapDecoration {{ sprite: \"test:sprite{n}\".into(), x: {}, y: {}, rot: 0, shown_on_frame: {} }}",
                1 + n as i8,
                3 + n as i8,
                n.is_multiple_of(2)
            ),
            format!(
                "{{ sprite = \"test:sprite{n}\", x = {}, y = {}, rot = 0, shown_on_frame = {} }}",
                1 + n as i8,
                3 + n as i8,
                n.is_multiple_of(2)
            ),
            vec![
                leaf("sprite", format!("\"test:sprite{n}\"")),
                leaf("x", format!("{}", 1 + n as i8)),
                leaf("y", format!("{}", 3 + n as i8)),
                leaf("rot", "0".into()),
                leaf("shown_on_frame", format!("{}", n.is_multiple_of(2))),
            ],
        ),
        "NativeDataMapType" => fixed(
            format!(
                "NativeDataMapType {{ id: \"test:map{n}\".into(), registry: \"test:registry{n}\".into() }}"
            ),
            format!("{{ id = \"test:map{n}\", registry = \"test:registry{n}\" }}"),
            vec![
                leaf("id", format!("\"test:map{n}\"")),
                leaf("registry", format!("\"test:registry{n}\"")),
            ],
        ),
        "NativeConfig" => fixed(
            format!(
                "{{ let mut c = NativeConfig::new(\"test:mod{n}\", NativeConfigKind::Server, \"test:default{n}\"); c.file = \"test:file{n}\".into(); c }}"
            ),
            format!(
                "{{ mod_id = \"test:mod{n}\", kind = \"server\", file = \"test:file{n}\", default = \"test:default{n}\" }}"
            ),
            vec![
                leaf("mod_id", format!("\"test:mod{n}\"")),
                leaf("kind", "\"server\"".into()),
                leaf("file", format!("\"test:file{n}\"")),
                leaf("default", format!("\"test:default{n}\"")),
            ],
        ),
        "NativeConfigFile" => fixed(
            format!(
                "NativeConfigFile {{ mod_id: \"test:mod{n}\".into(), kind: NativeConfigKind::Server, file: \"test:file{n}\".into(), path: \"test:path{n}\".into(), text: \"test:text{n}\".into() }}"
            ),
            format!(
                "{{ mod_id = \"test:mod{n}\", kind = \"server\", file = \"test:file{n}\", path = \"test:path{n}\", text = \"test:text{n}\" }}"
            ),
            vec![
                leaf("mod_id", format!("\"test:mod{n}\"")),
                leaf("kind", "\"server\"".into()),
                leaf("file", format!("\"test:file{n}\"")),
                leaf("path", format!("\"test:path{n}\"")),
                leaf("text", format!("\"test:text{n}\"")),
            ],
        ),
        "SpawnEntry" => fixed(
            format!(
                "SpawnEntry {{ entity: \"test:entity{n}\".into(), weight: {}, min: 1, max: 2, constant: {} }}",
                7 + n as u32,
                n.is_multiple_of(2)
            ),
            format!(
                "{{ entity = \"test:entity{n}\", weight = {}, min = 1, max = 2, constant = {} }}",
                7 + n as u32,
                n.is_multiple_of(2)
            ),
            vec![
                leaf("entity", format!("\"test:entity{n}\"")),
                leaf("weight", format!("{}", 7 + n as u32)),
                leaf("min", "1".into()),
                leaf("max", "2".into()),
                leaf("constant", format!("{}", n.is_multiple_of(2))),
            ],
        ),
        "RecipeBookCategory" => fixed(
            format!(
                "RecipeBookCategory {{ id: \"test:cat{n}\".into(), recipe_type: \"test:type{n}\".into(), icon: \"test:icon{n}\".into() }}"
            ),
            format!(
                "{{ id = \"test:cat{n}\", recipe_type = \"test:type{n}\", icon = \"test:icon{n}\" }}"
            ),
            vec![
                leaf("id", format!("\"test:cat{n}\"")),
                leaf("recipe_type", format!("\"test:type{n}\"")),
                leaf("icon", format!("\"test:icon{n}\"")),
            ],
        ),
        "NativeReloadListener" => fixed(
            format!("NativeReloadListener::json(\"test:id{n}\", \"test:folder{n}\")"),
            format!("{{ id = \"test:id{n}\", folder = \"test:folder{n}\" }}"),
            vec![
                leaf("id", format!("\"test:id{n}\"")),
                leaf("folder", format!("\"test:folder{n}\"")),
            ],
        ),
        "Suggestion" => fixed(
            // `tooltip` and `int_value` seed non-default values (the runtime
            // writes both): a dropped key and a wrong value each fail.
            // `int_value` sits far above the range numbers for every `n`, so
            // swapping or zeroing any leaf changes an asserted value.
            format!(
                "Suggestion {{ range: ({} + {n}, {} + {n}), text: \"test:suggestion{n}\".into(), tooltip: Some(\"test:tooltip{n}\".into()), int_value: Some({}) }}",
                7,
                12,
                30 + n as i32
            ),
            format!(
                "{{ range = {{ start = {}, [\"end\"] = {} }}, text = \"test:suggestion{n}\", tooltip = \"test:tooltip{n}\", int_value = {} }}",
                7 + n,
                12 + n,
                30 + n as i32
            ),
            vec![
                leaf("range.start", format!("{}", 7 + n)),
                (".range[\"end\"]".into(), format!("{}", 12 + n)),
                leaf("text", format!("\"test:suggestion{n}\"")),
                leaf("tooltip", format!("\"test:tooltip{n}\"")),
                leaf("int_value", format!("{}", 30 + n as i32)),
            ],
        ),
        "MerchantOffer" => {
            // Every number below is its own Lua value (the cost and the
            // result are different stacks, `uses` sits far above the small
            // constants for every `n`): swapping or zeroing any leaf changes
            // an asserted value. `cost_b` is absent, read back as nil.
            let result_rust =
                "ItemStack { item: \"test:{field}:out\".into(), count: 2, damage: 3, extra: None }"
                    .replace("{field}", field);
            let result_lua = format!("{{ id = \"test:{field}:out\", count = 2, damage = 3 }}");
            let mut s = mutable(
                format!(
                    "MerchantOffer {{ base_cost_a: ItemCost {{ stack: {} }}, cost_b: None, result: {}, uses: {}, max_uses: 4, reward_exp: true, special_price_diff: 5, demand: 6, price_multiplier: 0.5, xp: 7 }}",
                    item(1, 0),
                    result_rust,
                    10 + n as i32
                ),
                format!(
                    "{{ base_cost_a = {{ stack = {{ id = \"test:{field}\", count = 1, damage = 0 }} }}, result = {result_lua}, uses = {}, max_uses = 4, reward_exp = true, special_price_diff = 5, demand = 6, price_multiplier = 0.5, xp = 7 }}",
                    10 + n as i32
                ),
                format!(
                    "MerchantOffer {{ base_cost_a: ItemCost {{ stack: {} }}, cost_b: None, result: {}, uses: {}, max_uses: 4, reward_exp: true, special_price_diff: 5, demand: 6, price_multiplier: 0.5, xp: 7 }}",
                    item(1, 0),
                    result_rust,
                    50 + n as i32
                ),
                format!(
                    "{{ base_cost_a = {{ stack = {{ id = \"test:{field}\", count = 1, damage = 0 }} }}, result = {result_lua}, uses = {}, max_uses = 4, reward_exp = true, special_price_diff = 5, demand = 6, price_multiplier = 0.5, xp = 7 }}",
                    50 + n as i32
                ),
            );
            s.leaves = vec![
                (".base_cost_a.stack.id".into(), format!("\"test:{field}\"")),
                (".base_cost_a.stack.count".into(), "1".into()),
                (".base_cost_a.stack.damage".into(), "0".into()),
                (".base_cost_a.stack.extra".into(), "nil".into()),
                (".base_cost_a.stack.plain".into(), "true".into()),
                (".cost_b".into(), "nil".into()),
                (".result.id".into(), format!("\"test:{field}:out\"")),
                (".result.count".into(), "2".into()),
                (".result.damage".into(), "3".into()),
                (".result.extra".into(), "nil".into()),
                (".result.plain".into(), "true".into()),
                leaf("uses", format!("{}", 10 + n as i32)),
                leaf("max_uses", "4".into()),
                leaf("reward_exp", "true".into()),
                leaf("special_price_diff", "5".into()),
                leaf("demand", "6".into()),
                leaf("price_multiplier", "0.5".into()),
                leaf("xp", "7".into()),
            ];
            s
        }
        "AnvilCraft" => fixed(
            // The input and the output are different stacks (different ids
            // and counts): swapping `left` and `output` changes an asserted
            // leaf. `right` is absent, read back as nil.
            format!(
                "AnvilCraft {{ left: Some({}), right: None, output: ItemStack {{ item: \"test:{field}:out\".into(), count: 2, damage: 3, extra: None }} }}",
                item(1, 0)
            ),
            format!(
                "{{ left = {{ id = \"test:{field}\", count = 1, damage = 0 }}, output = {{ id = \"test:{field}:out\", count = 2, damage = 3 }} }}"
            ),
            vec![
                leaf("left.id", format!("\"test:{field}\"")),
                leaf("left.count", "1".into()),
                leaf("left.damage", "0".into()),
                leaf("left.extra", "nil".into()),
                leaf("left.plain", "true".into()),
                leaf("right", "nil".into()),
                (".output.id".into(), format!("\"test:{field}:out\"")),
                (".output.count".into(), "2".into()),
                (".output.damage".into(), "3".into()),
                (".output.extra".into(), "nil".into()),
                (".output.plain".into(), "true".into()),
            ],
        ),
        "NativeNoteBlockChange" => fixed(
            format!(
                "NativeNoteBlockChange {{ dimension: LevelDimension::Overworld, pos: BlockPos::new({}, {}, {}), instrument: \"test:instrument{n}\", old_note: {} }}",
                1 + n as i32,
                2 + n as i32,
                3 + n as i32,
                7 + n as i32
            ),
            format!(
                "{{ dimension = \"minecraft:overworld\", pos = {{ x = {}, y = {}, z = {} }}, instrument = \"test:instrument{n}\", old_note = {} }}",
                1 + n as i32,
                2 + n as i32,
                3 + n as i32,
                7 + n as i32
            ),
            vec![
                leaf("dimension", "\"minecraft:overworld\"".into()),
                leaf("pos.x", format!("{}", 1 + n as i32)),
                leaf("pos.y", format!("{}", 2 + n as i32)),
                leaf("pos.z", format!("{}", 3 + n as i32)),
                leaf("instrument", format!("\"test:instrument{n}\"")),
                leaf("old_note", format!("{}", 7 + n as i32)),
            ],
        ),
        "NativeFluidStack" => fixed(
            format!(
                "NativeFluidStack {{ fluid: \"test:fluid{n}\".into(), amount: {}, components: Compound::from([(\"testkey\".into(), Tag::Int({}))]) }}",
                7 + n as i32,
                12 + n as i32
            ),
            format!(
                "{{ fluid = \"test:fluid{n}\", amount = {}, components = {{ testkey = {} }} }}",
                7 + n as i32,
                12 + n as i32
            ),
            // The components carry one test key (not an empty compound), so
            // the leaf has a value to assert.
            vec![
                leaf("fluid", format!("\"test:fluid{n}\"")),
                leaf("amount", format!("{}", 7 + n as i32)),
                (".components.testkey".into(), format!("{}", 12 + n as i32)),
            ],
        ),
        "Option<NativeFluidStack>" => mutable(
            format!(
                "Some(NativeFluidStack {{ fluid: \"test:fluid{n}\".into(), amount: {}, components: Compound::new() }})",
                7 + n as i32
            ),
            format!(
                "{{ fluid = \"test:fluid{n}\", amount = {}, components = {{}} }}",
                7 + n as i32
            ),
            "None".into(),
            "nil".into(),
        ),
        "NativeFluidDrain" => fixed(
            format!("NativeFluidDrain::Any({})", 7 + n as i32),
            format!("{{ kind = \"any\", amount = {} }}", 7 + n as i32),
            vec![
                leaf("kind", "\"any\"".into()),
                leaf("amount", format!("{}", 7 + n as i32)),
            ],
        ),
        "NativeFluidTankView" => fixed(
            format!(
                "NativeFluidTankView {{ tank: {n}, content: Some(NativeFluidStack {{ fluid: \"test:fluid{n}\".into(), amount: 7, components: Compound::new() }}), capacity: 1000, can_fill: true, can_drain: false }}"
            ),
            format!(
                "{{ tank = {n}, content = {{ fluid = \"test:fluid{n}\", amount = 7, components = {{}} }}, capacity = 1000, can_fill = true, can_drain = false }}"
            ),
            vec![leaf("tank", format!("{n}"))],
        ),
        "Music" => {
            let mut s = mutable(
                format!(
                    "Music {{ sound: \"test:sound{n}\".into(), min_delay: {}, max_delay: {}, replace_current_music: {} }}",
                    1 + n as i32,
                    2 + n as i32,
                    n.is_multiple_of(2)
                ),
                format!(
                    "{{ sound = \"test:sound{n}\", min_delay = {}, max_delay = {}, replace_current_music = {} }}",
                    1 + n as i32,
                    2 + n as i32,
                    n.is_multiple_of(2)
                ),
                "None".into(),
                "nil".into(),
            );
            s.leaves = vec![
                leaf("sound", format!("\"test:sound{n}\"")),
                leaf("min_delay", format!("{}", 1 + n as i32)),
                leaf("max_delay", format!("{}", 2 + n as i32)),
                leaf("replace_current_music", format!("{}", n.is_multiple_of(2))),
            ];
            s
        }
        "Option<Music>" => mutable(
            format!(
                "Some(Music {{ sound: \"test:sound{n}\".into(), min_delay: {}, max_delay: {}, replace_current_music: {} }})",
                1 + n as i32,
                2 + n as i32,
                n.is_multiple_of(2)
            ),
            format!(
                "{{ sound = \"test:sound{n}\", min_delay = {}, max_delay = {}, replace_current_music = {} }}",
                1 + n as i32,
                2 + n as i32,
                n.is_multiple_of(2)
            ),
            "None".into(),
            "nil".into(),
        ),
        "MusicLayer" => fixed(
            format!(
                "MusicLayer {{ sound: \"test:sound{n}\".into(), volume: {:?} }}",
                0.5 + n as f32
            ),
            format!(
                "{{ sound = \"test:sound{n}\", volume = {:?} }}",
                0.5 + n as f32
            ),
            vec![
                leaf("sound", format!("\"test:sound{n}\"")),
                leaf("volume", format!("{:?}", 0.5 + n as f32)),
            ],
        ),
        "SoundPlayback" => fixed(
            format!(
                "SoundPlayback {{ instance: {}, identifier: \"test:sound{n}\", source: 0, position: [{:?}, 2.0, 3.0], relative: {}, starting: {} }}",
                7 + n as u64,
                1.0 + n as f64,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            format!(
                "{{ instance = {}, identifier = \"test:sound{n}\", source = 0, position = {{ x = {:?}, y = 2.0, z = 3.0 }}, relative = {}, starting = {} }}",
                7 + n as u64,
                1.0 + n as f64,
                n.is_multiple_of(2),
                n % 2 == 1
            ),
            vec![leaf("identifier", format!("\"test:sound{n}\""))],
        ),
        "TickSpan" => fixed(
            format!(
                "TickSpan {{ name: \"test:span{n}\", ms: {:?}, cpu_ms: 0.5, depth: 0 }}",
                1.5 + n as f32
            ),
            format!(
                "{{ name = \"test:span{n}\", ms = {:?}, cpu_ms = 0.5, depth = 0 }}",
                1.5 + n as f32
            ),
            vec![
                leaf("name", format!("\"test:span{n}\"")),
                leaf("ms", format!("{:?}", 1.5 + n as f32)),
                leaf("cpu_ms", "0.5".into()),
                leaf("depth", "0".into()),
            ],
        ),
        "BlockProperties" => fixed("BlockProperties::new()".into(), "{}".into(), vec![]),
        "PlacementContext" => fixed(
            // The clicked block and the hit location differ (else swapping
            // `pos` and `hit_pos` passes), the fractional `hit` differs from
            // every integer leaf, and `yaw`/`pitch` differ from each other:
            // every numeric leaf below is its own Lua value.
            format!(
                "PlacementContext {{ pos: BlockPos::new({}, {}, {}), hit_pos: BlockPos::new({}, {}, {}), face: Direction::North, hit: horizon_core::geometry::Vec3 {{ x: {:?}, y: {:?}, z: {:?} }}, inside: {}, yaw: {:?}, pitch: {:?}, sneaking: {}, item: \"test:item{n}\".into(), replace_clicked: {} }}",
                1 + n as i32,
                2 + n as i32,
                3 + n as i32,
                4 + n as i32,
                5 + n as i32,
                6 + n as i32,
                7.5 + n as f64,
                8.5 + n as f64,
                9.5 + n as f64,
                n.is_multiple_of(2),
                90.0 + n as f32,
                30.0 + n as f32,
                n % 2 == 1,
                n.is_multiple_of(2)
            ),
            format!(
                "{{ pos = {{ x = {}, y = {}, z = {} }}, hit_pos = {{ x = {}, y = {}, z = {} }}, face = \"north\", hit = {{ x = {:?}, y = {:?}, z = {:?} }}, inside = {}, yaw = {:?}, pitch = {:?}, sneaking = {}, item = \"test:item{n}\", replace_clicked = {} }}",
                1 + n as i32,
                2 + n as i32,
                3 + n as i32,
                4 + n as i32,
                5 + n as i32,
                6 + n as i32,
                7.5 + n as f64,
                8.5 + n as f64,
                9.5 + n as f64,
                n.is_multiple_of(2),
                90.0 + n as f32,
                30.0 + n as f32,
                n % 2 == 1,
                n.is_multiple_of(2)
            ),
            vec![
                leaf("pos.x", format!("{}", 1 + n as i32)),
                leaf("pos.y", format!("{}", 2 + n as i32)),
                leaf("pos.z", format!("{}", 3 + n as i32)),
                leaf("hit_pos.x", format!("{}", 4 + n as i32)),
                leaf("hit_pos.y", format!("{}", 5 + n as i32)),
                leaf("hit_pos.z", format!("{}", 6 + n as i32)),
                leaf("face", "\"north\"".into()),
                leaf("hit.x", format!("{:?}", 7.5 + n as f64)),
                leaf("hit.y", format!("{:?}", 8.5 + n as f64)),
                leaf("hit.z", format!("{:?}", 9.5 + n as f64)),
                leaf("inside", format!("{}", n.is_multiple_of(2))),
                leaf("yaw", format!("{:?}", 90.0 + n as f32)),
                leaf("pitch", format!("{:?}", 30.0 + n as f32)),
                leaf("sneaking", format!("{}", n % 2 == 1)),
                leaf("item", format!("\"test:item{n}\"")),
                leaf("replace_clicked", format!("{}", n.is_multiple_of(2))),
            ],
        ),
        "FluidState" => fixed(
            // `EMPTY` is amount 0, not falling (the seed used to claim a
            // water-like 8/true, which no assert ever checked).
            "FluidState::EMPTY".into(),
            "{ kind = \"minecraft:empty\", amount = 0, falling = false }".into(),
            vec![
                leaf("kind", "\"minecraft:empty\"".into()),
                leaf("amount", "0".into()),
                leaf("falling", "false".into()),
            ],
        ),
        _ => return None,
    };
    Some(s)
}

/// The event-table keys a flat seed writes, with their `.d.lua` types, in
/// table order: the generated `horizon.d.lua` event type lists these, not
/// the native parameter name (which has no table key of its own).
pub fn flat_ts(conv: &Conv) -> Vec<(String, String)> {
    match conv {
        Conv::LifecycleCtx => vec![
            ("authority".into(), "string".into()),
            ("epoch".into(), "number".into()),
            ("tick".into(), "number".into()),
        ],
        Conv::TickPhase => vec![
            ("phase".into(), "string".into()),
            ("world_running".into(), "boolean".into()),
        ],
        Conv::WorldPhaseName => vec![("phase".into(), "string".into())],
        Conv::LootCtx => vec![
            ("table".into(), "string".into()),
            ("dimension".into(), "string".into()),
            ("origin".into(), "Vec3".into()),
            ("block".into(), "LootBlock?".into()),
            ("entity".into(), "IdKind?".into()),
            ("player".into(), "PlayerRef?".into()),
            ("tool".into(), "Item?".into()),
            ("explosion".into(), "number?".into()),
            ("used".into(), "boolean".into()),
        ],
        _ => Vec::new(),
    }
}

/// The `.d.lua` type of a `LuaStruct` key, with `Vec`, slice and `Option`
/// wrappers applied.
fn struct_ts_owned(key: &str) -> String {
    if let Some(inner) = key.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
        return format!("{{ {} }}", struct_ts_owned(inner));
    }
    if let Some(inner) = key.strip_prefix("&[").and_then(|s| s.strip_suffix(']')) {
        return format!("{{ {} }}", struct_ts_owned(inner));
    }
    if let Some(inner) = key
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return format!("{}?", struct_ts_owned(inner));
    }
    struct_ts(key).to_owned()
}

/// A known tuple shape: its struct key and `.d.lua` type. Anything else
/// keeps the tuple adapter.
fn tuple_struct(ty: &syn::Type) -> Option<(String, String)> {
    let syn::Type::Tuple(t) = ty else {
        return None;
    };
    // Tuple elements name their short form: `serde_json::Value` renders
    // qualified, but the samples spell `Value`.
    let norm: Vec<String> = t
        .elems
        .iter()
        .map(|e| ty_str(e).replace(' ', "").replace("serde_json::", ""))
        .collect();
    let key = format!("({})", norm.join(","));
    let ts = match norm
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["Text", "u32"] => "{ text: string, component: Component?, count: number }",
        ["i32", "u32"] => "{ value: number, max: number }",
        ["i32", "i32"] => "{ min: number, max: number }",
        ["Option<ItemStack>", "i32"] => "{ remainder: Item?, event: number }",
        ["String", "Value"] => "{ path: string, data: Data }",
        ["String", "Vec<u8>"] => "{ path: string, data: string }",
        ["String", "Compound"] => "{ name: string, tag: Data }",
        ["&'staticstr", "&'staticstr"] => "{ a: string, b: string }",
        ["usize", "usize"] => "{ start: number, end: number }",
        _ => return None,
    };
    Some((key, ts.to_owned()))
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
    hook: &str,
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
        if let Some(key) = struct_key_of(elem) {
            let ts = struct_ts_owned(&format!("Vec<{key}>"));
            return FieldOutcome::Conv(Conv::Struct, ts);
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
        let article = if "aeiouAEIOU".contains(elem_id.chars().next().unwrap_or('b')) {
            "an"
        } else {
            "a"
        };
        return adapter(&format!(
            "{article} {} list needs its table form",
            snake(&elem_id)
        ));
    }
    // An optional around one opaque thing: same, with optional wording.
    if let Some(inner) = single_generic_arg(ty, &["Option"]) {
        if let Some(why) = cannot_id(last_ident(inner).as_deref().unwrap_or_default()) {
            return cannot(&why);
        }
        if let Some(key) = struct_key_of(inner) {
            let ts = struct_ts_owned(&format!("Option<{key}>"));
            return FieldOutcome::Conv(Conv::Struct, ts);
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
    // Borrowed plain data the runtime copies into a table.
    if matches!(
        id.as_str(),
        "SoundPlayback" | "NativeMapInfo" | "NativeNoteBlockChange"
    ) {
        let ts = struct_ts_owned(&id);
        return FieldOutcome::Conv(Conv::Struct, ts);
    }
    if id == "NativeLootDrops" {
        return adapter(&format!("{} needs its table form", snake(&id)));
    }
    // A live mob only a design of its own can expose.
    if id == "Mob" {
        return FieldOutcome::Adapter(format!("{hook}: handle: a Mob handle needs its own design"));
    }
    // Network packets: one decoded packet per hook, needing per-type
    // decoders no single converter covers.
    if id.contains("Packet") {
        return FieldOutcome::Adapter(format!(
            "{hook}: protocol: raw packets need their own design"
        ));
    }
    if id == "ModBlockEntity" {
        return FieldOutcome::Adapter(format!(
            "{hook}: handle: a ModBlockEntity handle needs its own design"
        ));
    }
    // Opaque mod-data the runtime names as tables.
    if matches!(
        id.as_str(),
        "ModContainer"
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
        let ts = struct_ts_owned(&id);
        return FieldOutcome::Conv(Conv::Struct, ts);
    }
    // Plain data the runtime maps.
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
            | "RecipeBookCategory"
    ) {
        let ts = struct_ts_owned(&id);
        return FieldOutcome::Conv(Conv::Struct, ts);
    }
    if matches!(
        id.as_str(),
        "NativeLifecycleContext"
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
            // (`is_path` needs a bare ident, so it never matches a `Vec<T>`;
            // the single-generic check below already names the segment.)
            syn::Type::Path(_)
                if single_generic_arg(inner, &["Vec"]).is_some_and(|e| is_path(e, "u8")) =>
            {
                ReplaceRead::Bytes
            }
            syn::Type::Path(_)
                if single_generic_arg(inner, &["Vec"])
                    .is_some_and(|e| struct_key_of(e).is_some()) =>
            {
                ReplaceRead::StructVec
            }
            // `Option<Option<BlockStateId>>`: `inner` already strips the
            // outer `Option`, so one more strip reaches the state id.
            syn::Type::Path(_)
                if option_inner(inner).is_some_and(|o| is_path(o, "BlockStateId")) =>
            {
                ReplaceRead::AnswerState
            }
            _ if struct_key_of(inner).is_some() => ReplaceRead::Struct,
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
                    key: None,
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
                let key = if matches!(conv, Conv::Struct | Conv::LootDrops) {
                    struct_key_of(ty)
                } else {
                    None
                };
                fields.push(Field {
                    name: param.clone(),
                    conv,
                    ts,
                    key,
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
    // Raw protocol packets are a design of their own, not field tables.
    if matches!(name, "client_packet" | "server_packet" | "server_packets") {
        return Err(Class::Protocol);
    }
    if let Some(marker) = adapters.iter().find(|a| a.contains("handle: ")) {
        let ty = marker
            .split("handle: a ")
            .nth(1)
            .and_then(|s| s.split(" handle").next())
            .unwrap_or("?")
            .to_owned();
        return Err(Class::Handle(ty));
    }
    if adapters.iter().any(|a| a.contains("protocol: ")) {
        return Err(Class::Protocol);
    }
    if !adapters.is_empty() {
        return Err(Class::Adapter(adapters));
    }
    // The kind filter: a `kind` string field names an entity type (minus
    // the documented non-entity kinds), else the hook's single entity id
    // resolves through a game host. Loot filters by the block id or entity
    // kind of its context instead, with the absent-both early-out.
    let kind = if name == "loot_drops" {
        Kind::Loot
    } else if params.iter().any(|(n, t)| n == "kind" && is_str_ref(t))
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
    // Every state field seeds its `_props` table beside itself (the state
    // seed arms set both); the extra only names it for the docs and tests.
    for field in fields
        .iter()
        .filter(|f| matches!(f.conv, Conv::StateLevel | Conv::StateGame))
        .map(|f| f.name.clone())
        .collect::<Vec<_>>()
    {
        extras.push(Extra {
            name: format!("{field}_props"),
            conv: Conv::StateProps,
        });
    }
    // Seeds the smoke test cannot construct: `#[non_exhaustive]` structs
    // only the game builds. The runtime reads them, but the generated
    // test has no value to dispatch.
    if let Some(what) = fields.iter().find_map(|f| match f.key.as_deref() {
        Some("NativeConfigFile") => Some("a config file only the game constructs"),
        Some("SoundPlayback") => Some("a playback only the game constructs"),
        _ => None,
    }) {
        return Err(Class::Adapter(vec![format!("{name}: adapter: {what}")]));
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

pub fn assert_lines(
    conv: &Conv,
    field: &str,
    lua: &str,
    key: Option<&str>,
    n: usize,
) -> Vec<String> {
    let a = |expr: &str| format!("    assert({expr}, \"{field}\")");
    let e = lua_read(field);
    match conv {
        Conv::Struct => {
            let key = key.unwrap_or("");
            let s = struct_sample(key, field, n)
                .unwrap_or_else(|| panic!("no struct asserts for {key}"));
            if key == "BlockProperties" {
                // The sample is always empty (`BlockProperties::new()`); the
                // delivery is the empty table, asserted like a `_props` note.
                return vec![a(&format!("next({e}) == nil"))];
            }
            if s.leaves.is_empty() {
                // A scalar-shaped struct (a duration's seconds, an address,
                // a one-word enum): the seed compares as one value. A table
                // seed compares by identity, so table seeds need leaves;
                // an empty return marks the field uncovered.
                if s.lua.trim_start().starts_with('{') {
                    return Vec::new();
                }
                return vec![a(&format!("{e} == {}", s.lua))];
            }
            s.leaves
                .iter()
                .map(|(suffix, value)| a(&format!("{e}{suffix} == {value}")))
                .collect()
        }
        Conv::TextVal => vec![
            a(&format!("{e} == {lua}")),
            a(&format!("{e}_component.text == {lua}")),
        ],
        Conv::TextOpt => {
            if lua == "nil" {
                vec![
                    a(&format!("{e} == nil")),
                    a(&format!("{e}_component == nil")),
                ]
            } else {
                vec![
                    a(&format!("{e} == {lua}")),
                    a(&format!("{e}_component.text == {lua}")),
                ]
            }
        }
        Conv::PreLogin => vec![a(&format!("{e}.count == 0"))],
        Conv::StateProps => vec![a(&format!("next({e}) == nil"))],
        Conv::Bytes | Conv::PathStr => vec![a(&format!("{e} == {lua}"))],
        Conv::Player | Conv::PlayerOpt => vec![
            a(&format!("{e}.uuid == {lua}")),
            a(&format!("{e}.name == \"test:name\"")),
        ],
        Conv::Pos | Conv::PosStruct | Conv::MutPos => {
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
        Conv::Vec3f | Conv::MutVec3 => {
            // Same varying components as the `Vec3f` sample: constants here
            // would collide with `x` (and each other) for some `n`.
            let (x, y, z) = (1.0 + n as f64, 2.0 + n as f64, 3.0 + n as f64);
            vec![
                a(&format!("{e}.x == {x:?}")),
                a(&format!("{e}.y == {y:?}")),
                a(&format!("{e}.z == {z:?}")),
            ]
        }
        Conv::IntSeq => vec![a(&format!("{e}[1] == {} and {e}[2] == {}", 4 + n, 9 + n))],
        Conv::StrSeq | Conv::MutStrVec => {
            vec![
                a(&format!("{e}[1] == \"test:seed\"")),
                a(&format!("{e}[2] == \"test:seed2\"")),
            ]
        }
        Conv::Item => {
            // The borrowed seeds stay plain (`extra` is `None` in every one;
            // the struct `ItemStack` seeds carry components instead): `plain`
            // asserts the derived leaf the runtime writes, so a wrong or
            // missing `plain` fails.
            vec![
                a(&format!("{e}.id == {lua}")),
                a(&format!("{e}.count == {}", 1 + n)),
                a(&format!("{e}.damage == {n}")),
                a(&format!("{e}.extra == nil")),
                a(&format!("{e}.plain == true")),
            ]
        }
        Conv::OptItem => {
            if lua == "nil" {
                vec![a(&format!("{e} == nil"))]
            } else {
                // Same plain stack as `Conv::Item` (see `sample`).
                vec![
                    a(&format!("{e}.id == {lua}")),
                    a(&format!("{e}.count == {}", 1 + n)),
                    a(&format!("{e}.damage == {n}")),
                    a(&format!("{e}.extra == nil")),
                    a(&format!("{e}.plain == true")),
                ]
            }
        }
        Conv::OptPos => {
            if lua == "nil" {
                return vec![a(&format!("{e} == nil"))];
            }
            let (x, y, z) = (1 + n as i32, 2 + n as i32, 3 + n as i32);
            vec![
                a(&format!("{e}.x == {x}")),
                a(&format!("{e}.y == {y}")),
                a(&format!("{e}.z == {z}")),
            ]
        }
        // The flat seeds: one assert per seeded table key, each with the
        // sample's independent value. (`lua` is unused: the sample builds
        // the native value, and these lines recompute the Lua side from
        // `n`, like the position arms do.)
        Conv::LifecycleCtx => {
            let authority = if n.is_multiple_of(2) {
                "IntegratedServer"
            } else {
                "DedicatedServer"
            };
            vec![
                a(&format!("event.authority == \"{authority}\"")),
                a(&format!("event.epoch == {}", 7 + n)),
                a(&format!("event.tick == {}", 11 + n)),
            ]
        }
        Conv::TickPhase => {
            let (phase, running) = if n.is_multiple_of(2) {
                ("start", true)
            } else {
                ("end", false)
            };
            vec![
                a(&format!("event.phase == \"{phase}\"")),
                a(&format!("event.world_running == {running}")),
            ]
        }
        Conv::WorldPhaseName => {
            let phase = [
                "WorldOpenReady",
                "WorldOpenFailed",
                "WorldCloseFinal",
                "WorldCloseFailed",
            ][n % 4];
            vec![a(&format!("event.phase == \"{phase}\""))]
        }
        Conv::LootCtx => {
            let origin = |c: f64| format!("{c:?}");
            let (ox, oy, oz) = (1.0 + n as f64, 2.0 + n as f64, 3.0 + n as f64);
            let (x, y, z) = (1 + n as i32, 2 + n as i32, 3 + n as i32);
            let uuid = uuid_sample((7 + n) as u128);
            let tool_count = 1 + n;
            let explosion = format!("{}", 1.5 + n as f32);
            vec![
                a(&format!("event.table == \"test:table{n}\"")),
                a("event.dimension == \"minecraft:overworld\""),
                a(&format!("event.origin.x == {}", origin(ox))),
                a(&format!("event.origin.y == {}", origin(oy))),
                a(&format!("event.origin.z == {}", origin(oz))),
                a(&format!("event.block.x == {x}")),
                a(&format!("event.block.y == {y}")),
                a(&format!("event.block.z == {z}")),
                a("event.block.id == \"minecraft:stone\""),
                a("event.block.state == \"minecraft:stone\""),
                a(&format!("event.entity.id == {}", 11 + n)),
                a("event.entity.kind == \"minecraft:zombie\""),
                a(&format!("event.player.uuid == \"{uuid}\"")),
                a("event.tool.id == \"test:tool\""),
                a(&format!("event.tool.count == {tool_count}")),
                a(&format!("event.tool.damage == {n}")),
                a("event.tool.extra == nil"),
                a("event.tool.plain == true"),
                a(&format!("event.explosion == {explosion}")),
                a(&format!("event.used == {}", n.is_multiple_of(2))),
            ]
        }
        // Loot's drops assert like the struct vec of stacks they seed as.
        Conv::LootDrops => assert_lines(&Conv::Struct, field, lua, key, n),
        _ => vec![a(&format!("{e} == {lua}"))],
    }
}

/// A leaf that shares its asserted value with another leaf of the same seed
/// by construction: still asserted (for the delivery shape), exempt from the
/// distinct-values check below. A nested text component carries the same text
/// as the string beside it.
const DUP_LEAF_ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        "Text",
        ".component.text",
        "the component carries the same text",
    ),
    (
        "(Text,u32)",
        ".component.text",
        "the component carries the same text",
    ),
];

/// Every `(leaf suffix, value)` the generated handler asserts for one
/// table-shaped field: the same leaves `assert_lines` emits, as data, so the
/// distinct-values check cannot drift from the asserts. Scalar and
/// single-value converters assert one value or a fixed note, so they carry
/// nothing to reorder and list nothing here.
pub fn table_leaves(
    conv: &Conv,
    field: &str,
    lua: &str,
    key: Option<&str>,
    n: usize,
) -> Vec<(String, String)> {
    match conv {
        Conv::Struct => {
            let key = key.unwrap_or("");
            if key == "BlockProperties" {
                return Vec::new();
            }
            struct_sample(key, field, n)
                .unwrap_or_else(|| panic!("no struct asserts for {key}"))
                .leaves
        }
        // The plain stack of `sample`: `extra` is `None` in every borrowed
        // seed, `plain` the derived leaf the asserts pin.
        Conv::Item => vec![
            (".id".into(), lua.to_owned()),
            (".count".into(), format!("{}", 1 + n)),
            (".damage".into(), format!("{n}")),
            (".extra".into(), "nil".into()),
            (".plain".into(), "true".into()),
        ],
        Conv::OptItem => {
            if lua == "nil" {
                Vec::new()
            } else {
                // Same plain stack as `Conv::Item` (see `sample`).
                vec![
                    (".id".into(), lua.to_owned()),
                    (".count".into(), format!("{}", 1 + n)),
                    (".damage".into(), format!("{n}")),
                    (".extra".into(), "nil".into()),
                    (".plain".into(), "true".into()),
                ]
            }
        }
        Conv::Player | Conv::PlayerOpt => vec![
            (".uuid".into(), lua.to_owned()),
            (".name".into(), "\"test:name\"".into()),
        ],
        Conv::Pos | Conv::PosStruct | Conv::MutPos => vec![
            (".x".into(), format!("{}", 1 + n as i32)),
            (".y".into(), format!("{}", 2 + n as i32)),
            (".z".into(), format!("{}", 3 + n as i32)),
        ],
        Conv::OptPos => {
            if lua == "nil" {
                Vec::new()
            } else {
                vec![
                    (".x".into(), format!("{}", 1 + n as i32)),
                    (".y".into(), format!("{}", 2 + n as i32)),
                    (".z".into(), format!("{}", 3 + n as i32)),
                ]
            }
        }
        Conv::Chunk => vec![
            (".x".into(), format!("{}", 3 + n as i32)),
            (".z".into(), format!("{}", -5 - n as i32)),
        ],
        Conv::Pair | Conv::MutPair => vec![
            (".x".into(), format!("{}", 1.5 + n as f64)),
            (".y".into(), format!("{}", 2.5 + n as f64)),
        ],
        Conv::Vec3f | Conv::MutVec3 => vec![
            (".x".into(), format!("{:?}", 1.0 + n as f64)),
            (".y".into(), format!("{:?}", 2.0 + n as f64)),
            (".z".into(), format!("{:?}", 3.0 + n as f64)),
        ],
        Conv::IntSeq => vec![
            ("[1]".into(), format!("{}", 4 + n)),
            ("[2]".into(), format!("{}", 9 + n)),
        ],
        Conv::StrSeq | Conv::MutStrVec => vec![
            ("[1]".into(), "\"test:seed\"".into()),
            ("[2]".into(), "\"test:seed2\"".into()),
        ],
        // Loot's drops leaf like the struct vec of stacks they seed as.
        Conv::LootDrops => table_leaves(&Conv::Struct, field, lua, key, n),
        // The flat seeds assert their table keys directly (see
        // `assert_lines`): the `event.<field>`-rooted leaf check has no
        // suffix spelling them, so they carry no leaves. Their assert
        // values are hand-picked distinct per key.
        Conv::LifecycleCtx | Conv::TickPhase | Conv::WorldPhaseName | Conv::LootCtx => Vec::new(),
        _ => Vec::new(),
    }
}

/// A sequence row's leaf with its row prefix stripped (`[1].x` to `.x`):
/// the allowlist names the row-relative leaf.
fn bare_leaf(suffix: &str) -> &str {
    // `dot` counts in `rest` (one past the `[`): `[1].x` strips to `.x`.
    if let Some(rest) = suffix.strip_prefix('[')
        && let Some(dot) = rest.find("].")
    {
        &suffix[dot + 2..]
    } else {
        suffix
    }
}

/// A struct key with its `Vec`, slice and `Option` wrappers stripped: the
/// exemption below is about the inner type's shape, whatever wraps it (a
/// sequence of texts doubles the same component as one text).
fn inner_key(key: &str) -> &str {
    let mut key = key;
    loop {
        if let Some(inner) = key.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
            key = inner;
        } else if let Some(inner) = key.strip_prefix("&[").and_then(|s| s.strip_suffix(']')) {
            key = inner;
        } else if let Some(inner) = key
            .strip_prefix("Option<")
            .and_then(|s| s.strip_suffix('>'))
        {
            key = inner;
        } else {
            return key;
        }
    }
}

/// The first leaf asserting a value another leaf of the same row already
/// asserts, if any: `None` means every asserted value is its own. Boolean and
/// nil seeds carry no distinguishing value (three bool leaves cannot be
/// pairwise distinct; absence has one spelling), and `DUP_LEAF_ALLOWLIST`
/// names the inherent textual doubles. Anything else is a seed reordering or
/// zeroing cannot fail.
pub fn dup_leaf(key: Option<&str>, leaves: &[(String, String)]) -> Option<String> {
    let key = inner_key(key.unwrap_or(""));
    let exempt = |suffix: &str, value: &str| {
        matches!(value, "true" | "false" | "nil")
            || DUP_LEAF_ALLOWLIST
                .iter()
                .any(|(k, s, _)| *k == key && *s == bare_leaf(suffix))
    };
    for (i, (suffix, value)) in leaves.iter().enumerate() {
        if exempt(suffix, value) {
            continue;
        }
        let double = leaves[..i].iter().any(|(prev_suffix, prev_value)| {
            prev_value == value && !exempt(prev_suffix, prev_value)
        });
        if double {
            return Some(suffix.clone());
        }
    }
    None
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
        // A plain-string write must win while the seeded component sits
        // untouched, so the smoke handler sets only the string; clearing an
        // optional still clears both sides.
        Conv::TextVal => format!("    {e} = {write_lua}"),
        Conv::TextOpt => {
            format!("    {e} = {write_lua}; {e}_component = nil")
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

/// The `LuaStruct` key of a canonical parameter type string: parses it
/// back and classifies it, so samples and the emitter share one mapping.
/// Lifetimes render as `<?>` in canonical strings; they carry no shape.
fn struct_key_of_ty_str(ty: &str) -> Option<String> {
    syn::parse_str::<syn::Type>(&ty.replace("<?>", ""))
        .ok()
        .and_then(|t| struct_key_of(&t))
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
        Conv::Struct => {
            let key =
                struct_key_of_ty_str(ty).unwrap_or_else(|| panic!("no struct sample for {ty}"));
            let s = struct_sample(&key, field, n)
                .unwrap_or_else(|| panic!("no struct sample for {key}"));
            let norm: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
            let is_mut = norm.starts_with("&mut");
            if is_mut {
                let (write_rust, write_lua) = s
                    .clone_write()
                    .unwrap_or_else(|| panic!("no struct write for {key}"));
                return mut_sample(s.rust, s.lua, write_rust, write_lua);
            }
            if norm.starts_with('&') {
                // Borrowed seeds (`&[..]`, `Some(&..)`) already borrow.
                if s.rust.starts_with('&') {
                    return plain(s.rust, s.lua);
                }
                return plain(format!("&{}", s.rust), s.lua);
            }
            plain(s.rust, s.lua)
        }
        Conv::Bytes => plain(
            "&[116u8, 0u8, 159u8]".into(),
            "\"t\" .. string.char(0, 159)".into(),
        ),
        Conv::PathStr => plain(
            "std::path::Path::new(\"test:path\")".into(),
            "\"test:path\"".into(),
        ),
        Conv::TextVal => mut_sample(
            format!("Text::literal(\"test:{field}\")"),
            format!("\"test:{field}\""),
            format!("Text::literal(\"test:{field}:edited\")"),
            format!("\"test:{field}:edited\""),
        ),
        Conv::TextOpt => {
            if n.is_multiple_of(2) {
                mut_sample(
                    format!("Some(Text::literal(\"test:{field}\"))"),
                    format!("\"test:{field}\""),
                    "None".into(),
                    "nil".into(),
                )
            } else {
                plain("None".into(), "nil".into())
            }
        }
        Conv::PreLogin => mut_sample(
            "Vec::new()".into(),
            "{ count = 0 }".into(),
            "1".into(),
            "{ refuse = \"test:refused\" }".into(),
        ),
        Conv::StateProps => panic!("no sample for a state _props note"),
        // The lifecycle context both lifecycle hooks carry: the authority
        // alternates with `n`, the epoch and tick count up, so every leaf
        // has its own value.
        Conv::LifecycleCtx => {
            let authority = if n.is_multiple_of(2) {
                "IntegratedServer"
            } else {
                "DedicatedServer"
            };
            plain(
                format!(
                    "&horizon_client::test_events::native_lifecycle_context(horizon_client::mod_api::NativeAuthority::{authority}, horizon_client::mod_api::NativeWorldEpoch::new({}), horizon_client::mod_api::NativeTickId::new({}))",
                    7 + n,
                    11 + n
                ),
                "{}".into(),
            )
        }
        // The tick phase: end on odd dispatches (the timers' gate), start
        // otherwise; `world_running` alternates too.
        Conv::TickPhase => {
            let (phase, running) = if n.is_multiple_of(2) {
                ("ServerTickStart", true)
            } else {
                ("ServerTickEnd", false)
            };
            plain(
                format!(
                    "horizon_client::mod_api::ServerTickPhase::{phase} {{ world_running: {running} }}"
                ),
                "{}".into(),
            )
        }
        // The world phase: one variant per dispatch index, so the open,
        // failed-open, final-close and failed-close names are all covered.
        Conv::WorldPhaseName => {
            let phase = [
                "WorldOpenReady",
                "WorldOpenFailed",
                "WorldCloseFinal",
                "WorldCloseFailed",
            ][n % 4];
            plain(
                format!("horizon_client::mod_api::WorldPhase::{phase}"),
                "{}".into(),
            )
        }
        // Loot's context: a block-stone break at every parity (the kind
        // stays stone, so the kind plan's static registration holds), with
        // every other leaf counting up with `n`.
        Conv::LootCtx => plain(
            format!(
                "&horizon_client::test_events::native_loot_drops(\"test:table{n}\", \"minecraft:overworld\", [{:?}, {:?}, {:?}], Some(([{n1}, {n2}, {n3}], BlockStateId(7))), Some(({en}, \"minecraft:zombie\")), Some({pn}), Some(&ItemStack {{ item: \"test:tool\".into(), count: {cn}, damage: {n}, extra: None }}), Some({ex:?}), {used})",
                1.0 + n as f64,
                2.0 + n as f64,
                3.0 + n as f64,
                n1 = 1 + n as i32,
                n2 = 2 + n as i32,
                n3 = 3 + n as i32,
                en = 11 + n,
                pn = 7 + n,
                cn = 1 + n,
                ex = 1.5 + n as f32,
                used = n.is_multiple_of(2),
            ),
            "{}".into(),
        ),
        // Loot's drops: a struct vec of stacks (the seed and asserts are
        // the struct vec's); only the write-back differs.
        Conv::LootDrops => sample(&Conv::Struct, ty, field, n),
        Conv::Plain => {
            let (rust, lua) = plain_seed(ty, field, n);
            plain(rust, lua)
        }
        Conv::PlainString => plain(
            format!("\"test:{field}\".to_owned()"),
            format!("\"test:{field}\""),
        ),
        Conv::OptPlain => {
            if n % 2 == 1 {
                return plain("None".into(), "nil".into());
            }
            let inner = ty.trim_start_matches("Option<").trim_end_matches('>');
            let (rust, lua) = plain_seed(inner, field, n);
            plain(format!("Some({rust})"), lua)
        }
        Conv::Uuid => plain(
            (7 + n).to_string(),
            format!("\"{}\"", uuid_sample((7 + n) as u128)),
        ),
        Conv::OptUuid => {
            if n % 2 == 1 {
                return plain("None".into(), "nil".into());
            }
            plain(
                format!("Some({})", 7 + n),
                format!("\"{}\"", uuid_sample((7 + n) as u128)),
            )
        }
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
            if n % 2 == 1 {
                return plain("None".into(), "nil".into());
            }
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
            // Every component varies with `n`, as for the `[f64;3]` struct.
            let (x, y, z) = (1.0 + n as f64, 2.0 + n as f64, 3.0 + n as f64);
            plain(
                format!("[{x:?}, {y:?}, {z:?}]"),
                format!("{{ x = {x:?}, y = {y:?}, z = {z:?} }}"),
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
            "&[\"test:seed\".to_owned(), \"test:seed2\".to_owned()]".into(),
            "{ \"test:seed\", \"test:seed2\" }".into(),
        ),
        Conv::Item => plain(
            format!(
                "&ItemStack {{ item: \"test:{field}\".into(), count: {}, damage: {}, extra: None }}",
                1 + n,
                n
            ),
            format!("\"test:{field}\""),
        ),
        Conv::OptItem => {
            if n % 2 == 1 {
                return plain("None".into(), "nil".into());
            }
            // Same varying stack as `Conv::Item`, so the leaf asserts share
            // one formula (see `table_leaves`).
            plain(
                format!(
                    "Some(&ItemStack {{ item: \"test:{field}\".into(), count: {}, damage: {}, extra: None }})",
                    1 + n,
                    n
                ),
                format!("\"test:{field}\""),
            )
        }
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
            // Every component varies with `n`, as for the `[f64;3]` struct;
            // the write sits ten above the seed on every axis.
            let (x, y, z) = (1.0 + n as f64, 2.0 + n as f64, 3.0 + n as f64);
            let (wx, wy, wz) = (x + 10.0, y + 10.0, z + 10.0);
            mut_sample(
                format!("[{x:?}, {y:?}, {z:?}]"),
                format!("{{ x = {x:?}, y = {y:?}, z = {z:?} }}"),
                format!("[{wx:?}, {wy:?}, {wz:?}]"),
                format!("{{ x = {wx:?}, y = {wy:?}, z = {wz:?} }}"),
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
        Conv::MutStrVec => {
            // The handler edits element 1 and appends, so the middle seed
            // survives the write.
            if ty.contains("str") {
                return mut_sample(
                    "vec![\"test:seed\", \"test:seed2\"]".into(),
                    "{ \"test:seed\", \"test:seed2\" }".into(),
                    "vec![\"test:edited\", \"test:seed2\", \"test:added\"]".into(),
                    "{ \"test:edited\", \"test:added\" }".into(),
                );
            }
            mut_sample(
                "vec![\"test:seed\".to_owned(), \"test:seed2\".to_owned()]".into(),
                "{ \"test:seed\", \"test:seed2\" }".into(),
                "vec![\"test:edited\".to_owned(), \"test:seed2\".to_owned(), \"test:added\".to_owned()]"
                    .into(),
                "{ \"test:edited\", \"test:added\" }".into(),
            )
        }
    }
}

/// The `.d.lua` type of a replace reader.
pub fn replace_ts(read: ReplaceRead, ret: &str) -> String {
    match read {
        ReplaceRead::Int => "number".into(),
        ReplaceRead::Bool => "boolean".into(),
        ReplaceRead::Pair => "Vec2".into(),
        ReplaceRead::Item => "Item".into(),
        ReplaceRead::Struct => struct_ts_owned(
            &struct_key_of_ty_str(ret).unwrap_or_else(|| panic!("no struct key for {ret}")),
        ),
        ReplaceRead::StructVec => {
            let key =
                struct_key_of_ty_str(ret).unwrap_or_else(|| panic!("no struct key for {ret}"));
            let elem = key
                .strip_prefix("Option<Vec<")
                .and_then(|k| k.strip_suffix(">>"))
                .unwrap_or_else(|| panic!("no vec elem for {key}"));
            format!("{{ {} }}", struct_ts_owned(elem))
        }
        ReplaceRead::Bytes => "string".into(),
        ReplaceRead::AnswerState => "string?".into(),
    }
}

/// Strip the outer `Option<..>` layer of a replace return type.
fn replace_inner(ret: &str) -> String {
    struct_key_of_ty_str(ret)
        .unwrap_or_default()
        .strip_prefix("Option<")
        .and_then(|k| k.strip_suffix('>'))
        .unwrap_or_default()
        .to_owned()
}

/// The Rust literal a replace test writes.
pub fn replace_sample(read: ReplaceRead, ret: &str, n: usize) -> (String, String) {
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
        ReplaceRead::Struct => {
            let inner = replace_inner(ret);
            let s = struct_sample(&inner, "replace", n)
                .unwrap_or_else(|| panic!("no struct sample for {inner} (ret {ret})"));
            (s.rust, s.lua)
        }
        ReplaceRead::StructVec => {
            let mut inner = replace_inner(ret);
            if let Some(vec) = inner.strip_prefix("Vec<").and_then(|k| k.strip_suffix('>')) {
                inner = vec.to_owned();
            }
            let s = struct_sample(&inner, "replace", n)
                .unwrap_or_else(|| panic!("no struct sample for {inner}"));
            (format!("vec![{}]", s.rust), format!("{{ {} }}", s.lua))
        }
        ReplaceRead::Bytes => (
            "vec![116u8, 0u8, 159u8]".into(),
            "\"t\" .. string.char(0, 159)".into(),
        ),
        // The answer copies the seeded state: the block must match the
        // event's own state, and the properties stay empty.
        ReplaceRead::AnswerState => (String::new(), "event.state".into()),
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

/// What `model_of` says about one hook the hand-written set keeps: whether
/// an event table could carry it (so the deferral is its only reason), or
/// the class that would reject it anyway. `classify` short-circuits these
/// hooks before `model_of`, so the exposure doc asks here instead of
/// guessing the reason.
pub fn deferred_outcomes(root: &Path) -> Result<Vec<(String, Result<Model, Class>)>, String> {
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
        if !handwritten.contains(&name) {
            continue;
        }
        match model_of(root, hook, &game, &mut frees) {
            Ok(model) => out.push((name, Ok(model))),
            Err(class) => out.push((name, Err(class))),
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
