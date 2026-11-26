//! The plain data types of the surface, and their `ToLuaValue` impls in `event-horizon-lua-convert`.
//!
//! A type is plain when it is a struct, tuple struct, unit struct or enum with no lifetime or type
//! parameter, every field is an owned value the convert crate can already convert (or another plain
//! type), and no field name is a Rust keyword. [`plan`] works that out as a fixed point: a type is
//! accepted once all the types its fields name are accepted, so a type in a cycle, or one that
//! depends on a refused type, is refused with the reason.
//!
//! The Lua shapes (Rust to Lua only):
//! - a struct is a table keyed by its field names; a tuple struct is an array table (`[1]`, `[2]`);
//! - a unit struct is an empty table;
//! - an enum whose variants all carry no data is the lowercase name of the variant as a string;
//! - any other enum is a table `{tag = "<variant>", ...}`: named fields by name, tuple fields as `[1]`, `[2]`.
//!
//! A borrowed type (only lifetime parameters, such as `ScreenView<'a>`) is plain too when every field
//! can be copied into an owned Lua value: a `&str` (or `Option<&str>`), a `&Path`, a slice of a
//! convertible item, or an owned field of a plain type. A reference to anything else, a `&mut`, a
//! `dyn` and a type parameter are refused. A slice or `Vec` becomes a Lua array through
//! `list_to_lua`, which fails past [`MAX_LIST_LEN`] items rather than panicking.
//!
//! Variant names are [`snake_case`] of the Rust name. The surface's own docs do not give stable
//! names (only 39 of 202 backticked unit variants match), so the rule is the one name source.
//!
//! [`plan`] returns the file's text and the [`Plain`] set the method emitter uses to accept a named
//! type as convertible. Nothing here compiles the text: the tests parse it with `syn`, and the
//! convert crate compiles it.

use std::collections::{BTreeMap, BTreeSet};

use mod_surface::{Function, Type, TypeKind};

use crate::emit_impl::{MAX_ARRAY_LEN, convertible, family_convertible, scalar_name};
use crate::types::{Access, Mapped, TypeIr, map_type};

/// The most items a borrowed list copies into a Lua array in one call.
pub const MAX_LIST_LEN: usize = 1024;

/// The names of the plain types. The method emitter asks it whether a named type converts, and
/// whether a unit enum also converts from Lua (it is an answer or an output of a hook).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plain {
    paths: BTreeSet<String>,
    decided: BTreeSet<String>,
    bodies: BTreeMap<String, Body>,
}

impl Plain {
    /// A set of the given paths, for tests and for the method emitter's own checks.
    pub fn from_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        Plain {
            paths: paths.into_iter().map(str::to_owned).collect(),
            decided: BTreeSet::new(),
            bodies: BTreeMap::new(),
        }
    }

    /// Plain paths, and the unit enums among them that are decided (for the method emitter's tests).
    pub fn deciding<'a>(
        paths: impl IntoIterator<Item = &'a str>,
        decided: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Plain {
            paths: paths.into_iter().map(str::to_owned).collect(),
            decided: decided.into_iter().map(str::to_owned).collect(),
            bodies: BTreeMap::new(),
        }
    }

    /// Whether the type at this path (as the mapper writes it, such as `LevelDimension`) is plain.
    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// Whether the unit enum at this path has a `FromLuaValue` impl in the file (its name table is
    /// emitted with it).
    pub fn decides(&self, path: &str) -> bool {
        self.decided.contains(path)
    }

    /// The shape of each plain type, by path: what the Luau emitter writes its type from.
    pub fn bodies(&self) -> &BTreeMap<String, Body> {
        &self.bodies
    }

    /// A set of plain types with the given shapes, for the Luau emitter's tests.
    pub fn from_bodies(bodies: BTreeMap<String, Body>) -> Self {
        Plain {
            paths: bodies.keys().cloned().collect(),
            decided: BTreeSet::new(),
            bodies,
        }
    }
}

/// What the plan made of the surface's types.
#[derive(Debug, PartialEq, Eq)]
pub struct TypesPlan {
    /// The whole `generated_types.rs`.
    pub text: String,
    /// The plain types, by path, in name order (the same as `plain`).
    pub emitted: Vec<String>,
    /// Each candidate that is not plain, with its reason, in name order.
    pub refused: Vec<(String, String)>,
    /// The set of plain paths, for the method emitter.
    pub plain: Plain,
}

/// Types the convert crate converts by hand (`impl ToLuaValue` in `vector.rs`, `text.rs`, ...).
/// They are leaves: a field of one of these types converts already, and nothing is generated for it.
pub const HAND_WRITTEN: &[&str] = &[
    "BlockPos",
    "Compound",
    "Text",
    "text::Text",
    "Identifier",
    "Rotation",
];

/// Types that have a surface record but are not generated here. `ItemStack` and `BlockState` wait
/// for their own spec chunk; the two `*Spec` types are the hand-written forms of the same values.
const OWN_CHUNK: &[&str] = &["ItemStack", "BlockState", "ItemStackSpec", "BlockStateSpec"];

/// Rust keywords: a field or variant with one of these names would need `r#`, so its type is refused.
const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn",
];

/// One field of a struct or a variant: its name (a tuple field's is its position), its type, and
/// whether its text is a reference (`&'a T`), which a `ToLuaValue` impl reads by `&self`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// The field's Rust type text, as the surface gives it.
    pub text: String,
    pub ty: Mapped,
    pub by_ref: bool,
}

/// One variant of an enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    /// The Rust name.
    pub name: String,
    /// The Lua name: [`snake_case`] of `name`.
    pub lua: String,
    /// Its fields have names (`{ … }`), not positions.
    pub named: bool,
    pub fields: Vec<Field>,
}

/// The shape of a plain type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// `struct Name;`
    Unit,
    /// `struct Name { a: A }`
    Named(Vec<Field>),
    /// `struct Name(A)`
    Tuple(Vec<Field>),
    /// A newtype whose field is private, read through its one getter (`fn get(self) -> T`, or `id`
    /// or `as_str`): its Lua value is what the getter returns. `text` marks a string result.
    Getter {
        method: String,
        text: bool,
        ty: TypeIr,
    },
    /// `enum Name { … }`; `non_exhaustive` adds a wildcard arm, since another crate matches it.
    Enum {
        non_exhaustive: bool,
        variants: Vec<Variant>,
    },
}

/// A candidate: its path, the shape, whether it has lifetime parameters, and the field types it
/// names (the leaves the fixed point checks).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Def {
    path: String,
    hash: String,
    /// The type has lifetime parameters: its fields may be borrowed, and its impl says `<'_>`.
    borrowed: bool,
    body: Body,
}

/// The Lua name of a Rust variant: its Pascal name in snake_case.
///
/// An underscore goes before an uppercase letter that follows a lowercase letter or a digit, or
/// that follows an uppercase letter when the next letter is lowercase. Digits never start a word:
/// `Vec3` is `vec3`, `ItemV2` is `item_v2`, `XPRepair` is `xp_repair`, `HTTPServer` is `http_server`.
pub fn snake_case(pascal: &str) -> String {
    let chars: Vec<char> = pascal.chars().collect();
    let mut out = String::with_capacity(pascal.len() + 4);
    for (index, &ch) in chars.iter().enumerate() {
        if index > 0 && ch.is_ascii_uppercase() {
            let prev = chars[index - 1];
            let next_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            let split = prev.is_ascii_lowercase()
                || (prev.is_ascii_uppercase() && next_lower)
                || (prev.is_ascii_digit() && next_lower);
            if split {
                out.push('_');
            }
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// The `[T; N]` impls the convert crate's `vector.rs` writes by hand (keyed tables, `x`, `y`, `z`).
/// The emitter writes every other scalar array it meets, as a Lua array.
const HAND_WRITTEN_ARRAYS: &[(&str, usize)] = &[
    ("i32", 2),
    ("i32", 3),
    ("f64", 2),
    ("f64", 3),
    ("f32", 2),
    ("f32", 3),
];

/// Plans the plain types of `types` (the surface's `types`, in any order) and writes their impls.
/// `handles` are the surface's handle functions, which name the getters of private newtypes. The
/// same input always gives the same text.
pub fn plan(types: &[Type], handles: &[Function], hash: &str) -> TypesPlan {
    plan_with(types, handles, hash, &[])
}

/// [`plan`], also given the other type texts the method emitter will convert (the hook arguments),
/// so that every scalar array any of them names gets its impl here.
pub fn plan_with(types: &[Type], handles: &[Function], hash: &str, texts: &[String]) -> TypesPlan {
    plan_decided(types, handles, hash, texts, &[])
}

/// [`plan_with`], also given the type texts of the hooks' answers and outputs (`decisions`, such as
/// `Option<TriState>`). Each unit enum a decision names gets a `FromLuaValue` impl and a name table,
/// so a mod's string converts back to the variant; a data enum is never decided (its hook is
/// skipped by the method emitter).
pub fn plan_decided(
    types: &[Type],
    handles: &[Function],
    hash: &str,
    texts: &[String],
    decisions: &[String],
) -> TypesPlan {
    let decided_words = words(decisions);
    let mut arrays = BTreeSet::new();
    for text in texts {
        collect_arrays(text, &mut arrays);
    }
    for ty in types {
        for field in &ty.fields {
            collect_arrays(&field.contract.ty, &mut arrays);
        }
        for variant in &ty.variants {
            for arg in &variant.contract.fields {
                collect_arrays(&arg.ty, &mut arrays);
            }
        }
    }
    for handle in handles {
        if let Some((_, ret)) = handle.contract.signature.split_once("->") {
            collect_arrays(ret, &mut arrays);
        }
    }

    let mut refused: BTreeMap<String, String> = BTreeMap::new();
    let mut pending: BTreeMap<String, Def> = BTreeMap::new();
    for ty in types {
        let Some(path) = ty.id.strip_prefix("type:") else {
            continue;
        };
        if !matches!(
            ty.contract.kind,
            TypeKind::Struct | TypeKind::TupleStruct | TypeKind::UnitStruct | TypeKind::Enum
        ) {
            continue;
        }
        if HAND_WRITTEN.contains(&path) {
            refused.insert(
                path.to_owned(),
                "a hand-written impl in the convert crate".to_owned(),
            );
            continue;
        }
        if OWN_CHUNK.contains(&path) {
            refused.insert(
                path.to_owned(),
                "waits for the ItemStack/BlockState spec chunk".to_owned(),
            );
            continue;
        }
        match describe(ty, handles) {
            Ok(body) => {
                pending.insert(
                    path.to_owned(),
                    Def {
                        path: path.to_owned(),
                        hash: ty.hash.clone(),
                        borrowed: !ty.contract.generics.is_empty(),
                        body,
                    },
                );
            }
            Err(reason) => {
                refused.insert(path.to_owned(), reason);
            }
        }
    }

    // The fixed point: accept a candidate once every named field type is convertible given the
    // types accepted so far. What never gets accepted is in a cycle, or depends on one.
    let mut accepted: BTreeMap<String, Def> = BTreeMap::new();
    loop {
        let plain = Plain::from_paths(accepted.keys().map(String::as_str));
        let ready: Vec<String> = pending
            .iter()
            .filter(|(path, def)| {
                // A loot family type may name itself (`LootEntity.vehicle`): it is plain while its
                // own fields are checked.
                let blocker = if is_loot_family(path) {
                    let own = Plain::from_paths(
                        accepted
                            .keys()
                            .map(String::as_str)
                            .chain(std::iter::once(path.as_str())),
                    );
                    first_blocker(&def.body, def.borrowed, true, &own)
                } else {
                    first_blocker(&def.body, def.borrowed, false, &plain)
                };
                blocker.is_none()
            })
            .map(|(path, _)| path.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for path in ready {
            if let Some(def) = pending.remove(&path) {
                accepted.insert(path, def);
            }
        }
    }
    let decided: BTreeSet<String> = accepted
        .values()
        .filter(|def| unit_variants(def).is_some_and(|variants| !variants.is_empty()))
        .map(|def| def.path.clone())
        .filter(|path| decided_words.contains(path))
        .collect();
    let plain = Plain {
        paths: accepted.keys().cloned().collect(),
        decided: decided.clone(),
        bodies: accepted
            .iter()
            .map(|(path, def)| (path.clone(), def.body.clone()))
            .collect(),
    };
    for (path, def) in &pending {
        let reason = match first_blocker(&def.body, def.borrowed, is_loot_family(&def.path), &plain)
        {
            Some(reason) => reason,
            None => "no conversion".to_owned(),
        };
        refused.insert(path.clone(), reason);
    }

    let mut text =
        format!("// @generated by event-horizon-lua-gen from surface {hash}: do not edit.\n");
    if decided.is_empty() {
        text.push_str("use crate::scalar::ToLuaValue;\n");
    } else {
        text.push_str("use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};\n");
    }
    text.push_str("use mlua::{Lua, Value};\n");
    for (name, len) in &arrays {
        text.push_str(&array_impl(name, *len));
    }
    let lists = accepted.values().any(uses_list);
    if lists {
        text.push_str(LIST_HELPER);
    }
    for def in accepted.values() {
        text.push_str(&impl_text(def));
        if decided.contains(&def.path) {
            text.push_str(&unit_from_text(def));
        }
    }
    text.push_str(STD_IMPLS);
    text.push_str(RUNTIME_TESTS);
    if lists {
        text.push_str(LIST_TESTS);
    }
    if !decided.is_empty() {
        text.push_str(&unit_tests_text(
            accepted.values().filter(|def| decided.contains(&def.path)),
        ));
    }

    TypesPlan {
        text,
        emitted: accepted.keys().cloned().collect(),
        refused: refused.into_iter().collect(),
        plain,
    }
}

/// The identifiers in the type texts, without `mod_api` and the other path words: the names a
/// decision can refer to.
fn words(texts: &[String]) -> BTreeSet<String> {
    texts
        .iter()
        .flat_map(|text| text.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')))
        .filter(|word| !word.is_empty() && *word != "mod_api")
        .map(str::to_owned)
        .collect()
}

/// The variants of a unit enum (no variant carries data), or `None` for any other shape.
fn unit_variants(def: &Def) -> Option<&[Variant]> {
    match &def.body {
        Body::Enum { variants, .. }
            if variants
                .iter()
                .all(|variant| variant.fields.is_empty() && !variant.named) =>
        {
            Some(variants)
        }
        _ => None,
    }
}

/// The name table of a unit enum: `TRI_STATE_NAMES` for `TriState`. Both the `FromLuaValue` match
/// and the error that lists the allowed names read it, and the test checks it against the variants.
fn names_const(path: &str) -> String {
    format!(
        "{}_NAMES",
        snake_case(&path.replace("::", "_")).to_ascii_uppercase()
    )
}

/// The name table and the `FromLuaValue` impl of a decided unit enum, with leading blank lines.
fn unit_from_text(def: &Def) -> String {
    let variants = unit_variants(def).unwrap_or_default();
    let target = format!("mod_api::{}", def.path);
    let names = names_const(&def.path);
    let list: Vec<String> = variants
        .iter()
        .map(|variant| format!("{:?}", variant.lua))
        .collect();
    let mut out = format!("\nconst {names}: &[&str] = &[{}];\n", list.join(", "));
    out.push_str(&format!(
        "\nimpl FromLuaValue for {target} {{\n    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {{\n"
    ));
    out.push_str("        let name = <String as FromLuaValue>::from_lua_value(value)?;\n");
    out.push_str("        match name.as_str() {\n");
    for variant in variants {
        out.push_str(&format!(
            "            {:?} => Ok({target}::{}),\n",
            variant.lua, variant.name
        ));
    }
    out.push_str(&format!(
        "            _ => Err(ConvertError::unknown_variant(&name, {names})),\n"
    ));
    out.push_str("        }\n    }\n}\n");
    out
}

/// The tests of the decided unit enums: every variant's name is in its table, reads back as the
/// variant, and a name that is not in the table is refused (a long one clipped). One module.
fn unit_tests_text<'a>(defs: impl Iterator<Item = &'a Def>) -> String {
    let mut out = String::from(UNIT_TESTS_HEAD);
    for def in defs {
        let variants = unit_variants(def).unwrap_or_default();
        let list: Vec<String> = variants
            .iter()
            .map(|variant| format!("mod_api::{}::{}", def.path, variant.name))
            .collect();
        out.push_str(&format!(
            "\n    #[test]\n    fn {}_names_round_trip() {{\n        let lua = Lua::new();\n        check(&lua, super::{}, vec![{}]);\n    }}\n",
            snake_case(&def.path.replace("::", "_")),
            names_const(&def.path),
            list.join(", ")
        ));
    }
    out.push_str("}\n");
    out
}

/// The head of the unit enum tests: the shared check, then one test per decided enum.
const UNIT_TESTS_HEAD: &str = r#"
#[cfg(test)]
mod unit_enum_tests {
    use crate::scalar::{FromLuaValue, ToLuaValue};
    use mlua::{Lua, Value};

    fn name_of(value: &Value) -> String {
        match value {
            Value::String(text) => text.to_str().expect("utf-8").to_string(),
            other => panic!("expected a string, got {}", other.type_name()),
        }
    }

    fn check<T: FromLuaValue + ToLuaValue>(lua: &Lua, names: &[&str], variants: Vec<T>) {
        assert_eq!(variants.len(), names.len(), "one name per variant");
        for variant in variants {
            let name = name_of(&variant.to_lua_value(lua).expect("converts"));
            assert!(names.contains(&name.as_str()), "{name} is not in the table");
            let text = Value::String(lua.create_string(&name).expect("string"));
            let back = T::from_lua_value(&text).expect("reads back");
            assert_eq!(name_of(&back.to_lua_value(lua).expect("converts")), name);
        }
        let unknown = Value::String(lua.create_string("nope").expect("string"));
        let err = T::from_lua_value(&unknown).err().expect("an unknown name is refused");
        assert!(!err.to_string().contains("..."), "{err}");
        let long = Value::String(lua.create_string("x".repeat(60)).expect("string"));
        let err = T::from_lua_value(&long).err().expect("a long unknown name is refused");
        assert!(err.to_string().contains("..."), "{err}");
    }
"#;

/// Records each `[T; N]` in a type text (at any depth) whose scalar `T` and length `N` convert and
/// have no hand-written impl. A text that does not parse names no array.
fn collect_arrays(text: &str, out: &mut BTreeSet<(String, usize)>) {
    if let Ok(ty) = syn::parse_str::<syn::Type>(text) {
        walk_arrays(&ty, out);
    }
}

fn walk_arrays(ty: &syn::Type, out: &mut BTreeSet<(String, usize)>) {
    match ty {
        syn::Type::Array(array) => {
            if let Some(found) =
                scalar_array(ty).filter(|found| !HAND_WRITTEN_ARRAYS.contains(found))
            {
                out.insert((found.0.to_owned(), found.1));
            }
            walk_arrays(&array.elem, out);
        }
        syn::Type::Reference(reference) => walk_arrays(&reference.elem, out),
        syn::Type::Slice(slice) => walk_arrays(&slice.elem, out),
        syn::Type::Paren(paren) => walk_arrays(&paren.elem, out),
        syn::Type::Group(group) => walk_arrays(&group.elem, out),
        syn::Type::Tuple(tuple) => tuple.elems.iter().for_each(|elem| walk_arrays(elem, out)),
        syn::Type::Path(path) => {
            for segment in &path.path.segments {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            walk_arrays(inner, out);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// The scalar name and the length of a `[T; N]` whose `T` converts and whose `N` is in range.
fn scalar_array(ty: &syn::Type) -> Option<(&'static str, usize)> {
    let mapped = map_type(&tokens(ty)).ok()?;
    let TypeIr::Array(elem, len) = mapped.ty else {
        return None;
    };
    if !(1..=MAX_ARRAY_LEN).contains(&len) {
        return None;
    }
    Some((scalar_name(&elem)?, len))
}

/// The impl of `[name; len]` as a Lua array: one entry per item, 1-based.
fn array_impl(name: &str, len: usize) -> String {
    format!(
        "\nimpl ToLuaValue for [{name}; {len}] {{\n    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {{\n        let table = lua.create_table()?;\n        for (index, item) in self.iter().enumerate() {{\n            table.raw_set(index + 1, item.to_lua_value(lua)?)?;\n        }}\n        Ok(Value::Table(table))\n    }}\n}}\n"
    )
}

/// Whether a surface type's generics are lifetime parameters only (`<'a>`, `<'key, 'b>`).
fn lifetimes_only(generics: &str) -> bool {
    let inner = generics
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>');
    inner
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .all(|item| item.starts_with('\''))
}

/// Reads a surface type's shape, or the reason it is not plain.
fn describe(ty: &Type, handles: &[Function]) -> Result<Body, String> {
    let generics = ty.contract.generics.as_str();
    if !generics.is_empty() && !lifetimes_only(generics) {
        return Err("has a type parameter (generic, not borrowed)".to_owned());
    }
    let borrowed = !generics.is_empty();
    match ty.contract.kind {
        TypeKind::UnitStruct => Ok(Body::Unit),
        TypeKind::Struct => {
            if ty.fields.is_empty() {
                return getter(ty, handles)
                    .ok_or_else(|| "no public field is listed: its fields are private".to_owned());
            }
            let fields = ty
                .fields
                .iter()
                .map(|field| {
                    let name = field.id.rsplit('.').next().unwrap_or_default().to_owned();
                    field_of(name, &field.contract.ty, borrowed)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Body::Named(fields))
        }
        TypeKind::TupleStruct => {
            if ty.fields.is_empty() {
                return getter(ty, handles)
                    .ok_or_else(|| "no public field is listed: its fields are private".to_owned());
            }
            let fields = ty
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| field_of(index.to_string(), &field.contract.ty, borrowed))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Body::Tuple(fields))
        }
        TypeKind::Enum => {
            if ty.variants.is_empty() {
                return Err("an enum with no variants".to_owned());
            }
            let mut variants = Vec::new();
            for variant in &ty.variants {
                let name = variant.id.rsplit('.').next().unwrap_or_default().to_owned();
                let fields = variant
                    .contract
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        let field_name = if variant.contract.named {
                            arg.name.clone()
                        } else {
                            index.to_string()
                        };
                        field_of(field_name, &arg.ty, borrowed)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                variants.push(Variant {
                    lua: snake_case(&name),
                    name,
                    named: variant.contract.named,
                    fields,
                });
            }
            let mut seen = BTreeSet::new();
            for variant in &variants {
                if !seen.insert(variant.lua.clone()) {
                    return Err(format!(
                        "two variants are both named `{}` in Lua",
                        variant.lua
                    ));
                }
            }
            Ok(Body::Enum {
                non_exhaustive: ty.contract.non_exhaustive,
                variants,
            })
        }
        _ => Err("not a struct or enum".to_owned()),
    }
}

/// The getter names a private newtype may be read by, in the order they are looked for.
const GETTERS: &[&str] = &["get", "id", "as_str"];

/// The getter of a newtype with private fields: exactly one of [`GETTERS`] is a handle of the
/// type, takes `self` (a `Copy` type) or `&self`, and returns a scalar or a string the convert
/// crate converts. Its Lua value is what it returns.
fn getter(ty: &Type, handles: &[Function]) -> Option<Body> {
    let path = ty.id.strip_prefix("type:")?;
    let found: Vec<(&str, &str)> = GETTERS
        .iter()
        .filter_map(|method| {
            let handle = handles
                .iter()
                .find(|h| h.id == format!("handle:{path}::{method}"))?;
            Some((*method, handle.contract.signature.as_str()))
        })
        .collect();
    let [(method, signature)] = found.as_slice() else {
        return None;
    };
    let ret = signature
        .strip_prefix(&format!("fn {method}(self) -> "))
        .map(|ret| (ret, true))
        .or_else(|| {
            signature
                .strip_prefix(&format!("fn {method}(&self) -> "))
                .map(|ret| (ret, false))
        })?;
    let (ret, by_value) = ret;
    if by_value && !ty.impls.iter().any(|name| name == "Copy") {
        return None;
    }
    let mapped = map_type(ret).ok()?;
    if !convertible(&mapped.ty, &Plain::default()) {
        return None;
    }
    Some(Body::Getter {
        method: (*method).to_owned(),
        text: mapped.ty == TypeIr::Str,
        ty: mapped.ty,
    })
}

/// One field. In a plain type it must be owned (no `&`). In a borrowed type (`borrowed`) a reference
/// is allowed when it is copied: `&str`, `&Path` or a slice, as [`copyable_references`] checks.
/// The text must map to a type the mapper knows.
fn field_of(name: String, text: &str, borrowed: bool) -> Result<Field, String> {
    if KEYWORDS.contains(&name.as_str()) {
        return Err(format!("field `{name}` is a Rust keyword"));
    }
    if borrowed && text.contains('&') {
        let parsed = syn::parse_str::<syn::Type>(text)
            .map_err(|error| format!("field `{name}`: `{text}`: {error}"))?;
        copyable_references(&parsed).map_err(|reason| format!("field `{name}`: {reason}"))?;
    } else if text.contains('&') {
        return Err(format!(
            "field `{name}` has a reference type `{text}`: not owned"
        ));
    }
    let ty = map_type(text).map_err(|error| format!("field `{name}`: {error}"))?;
    Ok(Field {
        name,
        text: text.to_owned(),
        ty,
        by_ref: borrowed && text.contains('&'),
    })
}

/// Checks that every reference in a borrowed field's type is one the emitter copies. A `&mut` is
/// refused; so is a `dyn` behind a reference, and a list of references. A `&str`, a `&Path` and a
/// reference to any other type pass here: that type must convert, which the field's type checks
/// (the mapper drops the reference, so `&'a Text` is the convertible `Text`).
fn copyable_references(ty: &syn::Type) -> Result<(), String> {
    match ty {
        syn::Type::Reference(reference) => {
            if reference.mutability.is_some() {
                return Err(format!(
                    "`{}` is a mutable reference: not copied",
                    tokens(ty)
                ));
            }
            match &*reference.elem {
                syn::Type::Slice(slice) => {
                    if matches!(&*slice.elem, syn::Type::Reference(_)) {
                        return Err(format!("a list of references `{}`: not copied", tokens(ty)));
                    }
                    copyable_references(&slice.elem)
                }
                syn::Type::Path(path)
                    if path.qself.is_none()
                        && (is_name(&path.path, "str") || is_name(&path.path, "Path")) =>
                {
                    Ok(())
                }
                syn::Type::TraitObject(_) => Err(format!(
                    "a reference to `{}`: a `dyn` is not copied",
                    tokens(&*reference.elem)
                )),
                other => copyable_references(other),
            }
        }
        syn::Type::Path(path) => {
            path.path
                .segments
                .iter()
                .try_for_each(|segment| match &segment.arguments {
                    syn::PathArguments::AngleBracketed(args) => {
                        args.args.iter().try_for_each(|arg| match arg {
                            syn::GenericArgument::Type(inner) => copyable_references(inner),
                            _ => Ok(()),
                        })
                    }
                    _ => Ok(()),
                })
        }
        syn::Type::Slice(slice) => copyable_references(&slice.elem),
        syn::Type::Array(array) => copyable_references(&array.elem),
        syn::Type::Tuple(tuple) => tuple.elems.iter().try_for_each(copyable_references),
        syn::Type::Paren(paren) => copyable_references(&paren.elem),
        syn::Type::Group(group) => copyable_references(&group.elem),
        other => Err(format!("`{}`: not copied", tokens(other))),
    }
}

/// Whether a path is the one bare name `word`.
fn is_name(path: &syn::Path, word: &str) -> bool {
    path.segments.len() == 1 && path.segments[0].ident == word
}

/// A type as Rust text, for a reason.
fn tokens(ty: &impl quote::ToTokens) -> String {
    quote::ToTokens::to_token_stream(ty).to_string()
}

/// Whether a field of a borrowed type is a list the emitter copies: a slice or `Vec` of an item
/// that converts. A list of text is refused (the helper takes `ToLuaValue` items, and `&str` has no
/// impl).
fn list_convertible(elem: &TypeIr, plain: &Plain) -> bool {
    !has_text(elem) && convertible(elem, plain)
}

/// Whether a type is text, or an optional text.
fn has_text(ty: &TypeIr) -> bool {
    match ty {
        TypeIr::Str => true,
        TypeIr::Option(inner) => matches!(**inner, TypeIr::Str),
        _ => false,
    }
}

/// Whether a field's type converts: a list only in a borrowed type, anything else as [`convertible`].
/// A loot family type takes its lists, tuples and leaves as [`family_convertible`] says.
fn field_convertible(ty: &TypeIr, borrowed: bool, family: bool, plain: &Plain) -> bool {
    if family {
        return family_convertible(ty, plain);
    }
    match ty {
        TypeIr::List(elem) => borrowed && list_convertible(elem, plain),
        _ => convertible(ty, plain),
    }
}

/// Whether a type is one of the loot context's types (`world::snapshots::LootContext` and its
/// nested types): the chunk that gives them Lua conversions (#66) widens what they convert.
fn is_loot_family(path: &str) -> bool {
    path.starts_with("world::snapshots::Loot")
}

/// The fields the Lua side takes as a decimal string, as `(plain path, field)`. A roll's seed is a
/// full `i64`, and a Lua number is a double that cannot hold it exactly, so the generic integer
/// rule (which refuses an inexact value) would fail every loot hook. Other 64-bit fields keep that
/// rule on purpose: an id must never be rounded silently.
const DECIMAL_FIELDS: &[(&str, &str)] = &[("world::snapshots::LootContext", "seed")];

/// Whether `field` of the plain type at `path` is written as a decimal string (see [`DECIMAL_FIELDS`]).
pub fn is_decimal_field(path: &str, field: &str) -> bool {
    DECIMAL_FIELDS.contains(&(path, field))
}

/// Whether any field of the type is a list (so the file needs [`LIST_HELPER`]).
fn uses_list(def: &Def) -> bool {
    let fields: Vec<&Field> = match &def.body {
        Body::Unit | Body::Getter { .. } => Vec::new(),
        Body::Named(fields) | Body::Tuple(fields) => fields.iter().collect(),
        Body::Enum { variants, .. } => variants.iter().flat_map(|v| v.fields.iter()).collect(),
    };
    fields
        .iter()
        .any(|field| matches!(field.ty.ty, TypeIr::List(_)))
}

/// The reason the body is not yet convertible given `plain`, or `None` when every field converts.
fn first_blocker(body: &Body, borrowed: bool, family: bool, plain: &Plain) -> Option<String> {
    let fields: Vec<&Field> = match body {
        Body::Unit | Body::Getter { .. } => Vec::new(),
        Body::Named(fields) | Body::Tuple(fields) => fields.iter().collect(),
        Body::Enum { variants, .. } => variants.iter().flat_map(|v| v.fields.iter()).collect(),
    };
    fields
        .into_iter()
        .find(|field| !field_convertible(&field.ty.ty, borrowed, family, plain))
        .map(|field| {
            format!(
                "field `{}` has type `{}`: no Lua conversion (or a type that is not plain)",
                field.name,
                type_text(&field.ty.ty)
            )
        })
}

/// A type's text, for a reason.
fn type_text(ty: &TypeIr) -> String {
    match ty {
        TypeIr::Named(name) => name.clone(),
        TypeIr::Option(inner) => format!("Option<{}>", type_text(inner)),
        other => format!("{other:?}"),
    }
}

/// The `impl ToLuaValue` of one plain type, with its leading blank line.
fn impl_text(def: &Def) -> String {
    let target = format!("mod_api::{}", def.path);
    let generic = if def.borrowed { "<'_>" } else { "" };
    let mut out = format!("\nimpl ToLuaValue for {target}{generic} {{\n");
    out.push_str("    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {\n");
    match &def.body {
        Body::Unit => {
            out.push_str("        Ok(Value::Table(lua.create_table()?))\n");
        }
        Body::Getter { method, text, .. } => {
            let call = format!("self.{method}()");
            if *text {
                out.push_str(&format!("        {call}.to_string().to_lua_value(lua)\n"));
            } else {
                out.push_str(&format!("        {call}.to_lua_value(lua)\n"));
            }
        }
        Body::Named(fields) => {
            out.push_str("        let table = lua.create_table()?;\n");
            for field in fields {
                let value = if is_decimal_field(&def.path, &field.name) {
                    format!("self.{}.to_string().to_lua_value(lua)?", field.name)
                } else {
                    value_expr(&format!("self.{}", field.name), field)
                };
                out.push_str(&format!(
                    "        table.raw_set({:?}, {value})?;\n",
                    field.name
                ));
            }
            out.push_str("        Ok(Value::Table(table))\n");
        }
        Body::Tuple(fields) => {
            out.push_str("        let table = lua.create_table()?;\n");
            for (index, field) in fields.iter().enumerate() {
                out.push_str(&format!(
                    "        table.raw_set({}, {})?;\n",
                    index + 1,
                    value_expr(&format!("self.{}", field.name), field)
                ));
            }
            out.push_str("        Ok(Value::Table(table))\n");
        }
        Body::Enum {
            non_exhaustive,
            variants,
        } => {
            if variants
                .iter()
                .all(|variant| variant.fields.is_empty() && !variant.named)
            {
                out.push_str("        let name = match self {\n");
                for variant in variants {
                    out.push_str(&format!(
                        "            {target}::{} => {:?},\n",
                        variant.name, variant.lua
                    ));
                }
                if *non_exhaustive {
                    out.push_str(&unknown_arm(&def.path));
                }
                out.push_str("        };\n");
                out.push_str("        Ok(Value::String(lua.create_string(name)?))\n");
            } else {
                out.push_str("        let table = lua.create_table()?;\n");
                out.push_str("        match self {\n");
                for variant in variants {
                    out.push_str(&variant_arm(&target, variant));
                }
                if *non_exhaustive {
                    out.push_str(&unknown_arm(&def.path));
                }
                out.push_str("        }\n");
                out.push_str("        Ok(Value::Table(table))\n");
            }
        }
    }
    out.push_str("    }\n}\n");
    out
}

/// The runtime test at the end of `generated_types.rs`: it converts a struct, a unit enum and two
/// data enums (one unit variant, one with named fields) and reads the Lua values back. The expected
/// values are written here by hand, not computed from the impls.
const RUNTIME_TESTS: &str = r#"
#[cfg(test)]
mod runtime_tests {
    use crate::scalar::ToLuaValue;
    use mlua::{Lua, Table, Value};

    fn table_of(value: Value) -> Table {
        match value {
            Value::Table(table) => table,
            other => panic!("expected a table, got {}", other.type_name()),
        }
    }

    #[test]
    fn a_struct_reads_back_its_fields() {
        let lua = Lua::new();
        let value = mod_api::ChunkPos { x: 3, z: -4 };
        let table = table_of(value.to_lua_value(&lua).expect("converts"));
        assert_eq!(table.get::<i32>("x").expect("x"), 3);
        assert_eq!(table.get::<i32>("z").expect("z"), -4);
    }

    #[test]
    fn a_unit_enum_is_its_snake_case_name() {
        let lua = Lua::new();
        let value = mod_api::Corner::BottomLeft.to_lua_value(&lua).expect("converts");
        let Value::String(name) = value else {
            panic!("expected a string, got {}", value.type_name());
        };
        assert_eq!(name.to_str().expect("utf-8").to_string(), "bottom_left");
    }

    #[test]
    fn a_data_enum_has_a_tag_and_its_tuple_fields_by_position() {
        let lua = Lua::new();
        let table = table_of(mod_api::SwapWith::Hotbar(5).to_lua_value(&lua).expect("converts"));
        assert_eq!(table.get::<String>("tag").expect("tag"), "hotbar");
        assert_eq!(table.get::<u8>(1).expect("first field"), 5);
        let unit = table_of(mod_api::SwapWith::Offhand.to_lua_value(&lua).expect("converts"));
        assert_eq!(unit.get::<String>("tag").expect("tag"), "offhand");
        assert!(unit.get::<Value>(1).expect("no field").is_nil());
    }

    #[test]
    fn a_getter_newtype_reads_as_its_getter_value() {
        let lua = Lua::new();
        let value = mod_api::TickId::new(9).to_lua_value(&lua).expect("converts");
        assert!(matches!(value, Value::Integer(9)), "{}", value.type_name());
    }

    #[test]
    fn the_server_tick_phase_names_its_variant_and_field() {
        let lua = Lua::new();
        let phase = mod_api::ServerTickPhase::ServerTickEnd {
            world_running: false,
        };
        let table = table_of(phase.to_lua_value(&lua).expect("converts"));
        assert_eq!(table.get::<String>("tag").expect("tag"), "server_tick_end");
        assert!(!table.get::<bool>("world_running").expect("world_running"));
    }

    #[test]
    fn a_four_array_is_a_one_based_lua_array() {
        let lua = Lua::new();
        let table = table_of([1.5f32, 2.0, 3.0, 4.0].to_lua_value(&lua).expect("converts"));
        assert_eq!(table.raw_len(), 4);
        assert_eq!(table.get::<f32>(1).expect("first"), 1.5);
        assert_eq!(table.get::<f32>(4).expect("last"), 4.0);
    }

    #[test]
    fn a_borrowed_struct_copies_its_text_and_its_options() {
        let lua = Lua::new();
        let value = mod_api::HurtSource {
            damage_type: "fall",
            causing: 4,
            direct: -1,
            position: None,
        };
        let table = table_of(value.to_lua_value(&lua).expect("converts"));
        assert_eq!(table.get::<String>("damage_type").expect("damage_type"), "fall");
        assert_eq!(table.get::<i32>("causing").expect("causing"), 4);
        assert_eq!(table.get::<i32>("direct").expect("direct"), -1);
        assert!(table.get::<Value>("position").expect("position").is_nil());
    }

    #[test]
    fn a_char_is_a_one_character_string_and_a_path_is_its_text() {
        let lua = Lua::new();
        let Value::String(letter) = 'é'.to_lua_value(&lua).expect("converts") else {
            panic!("expected a string");
        };
        assert_eq!(letter.to_str().expect("utf-8").to_string(), "é");
        let path = std::path::Path::new("saves/player");
        let Value::String(text) = path.to_lua_value(&lua).expect("converts") else {
            panic!("expected a string");
        };
        assert_eq!(text.to_str().expect("utf-8").to_string(), "saves/player");
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_an_error_for_that_call() {
        use std::os::unix::ffi::OsStrExt;
        let lua = Lua::new();
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(b"\xff"));
        assert!(path.to_lua_value(&lua).is_err());
    }

    #[test]
    fn a_data_enum_with_named_fields_keys_them_by_name() {
        let lua = Lua::new();
        let value = mod_api::DimensionTransition::Conditional {
            to: "nether".to_owned(),
            from: "overworld".to_owned(),
        };
        let table = table_of(value.to_lua_value(&lua).expect("converts"));
        assert_eq!(table.get::<String>("tag").expect("tag"), "conditional");
        assert_eq!(table.get::<String>("to").expect("to"), "nether");
        assert_eq!(table.get::<String>("from").expect("from"), "overworld");
    }
}
"#;

/// The helper a borrowed list field uses: a Lua array table (1-based), or an error past the cap.
/// Emitted only when some type has a list field, so the file has no unused function.
const LIST_HELPER: &str = r#"
/// The most items a borrowed list copies into a Lua array in one call.
const MAX_LIST_LEN: usize = 1024;

/// A borrowed list as a Lua array table, or an error past [`MAX_LIST_LEN`] items.
fn list_to_lua<T: ToLuaValue>(items: &[T], lua: &Lua) -> mlua::Result<Value> {
    if items.len() > MAX_LIST_LEN {
        return Err(mlua::Error::runtime(format!(
            "a list of {} items is over the cap of {MAX_LIST_LEN}",
            items.len()
        )));
    }
    let table = lua.create_table()?;
    for (index, item) in items.iter().enumerate() {
        table.raw_set(index + 1, item.to_lua_value(lua)?)?;
    }
    Ok(Value::Table(table))
}
"#;

/// The conversions of the std types a surface hook names: a `char` is a one-character string, a
/// `Path` is its string (a path that is not UTF-8 is an error for that call). `Duration` already has
/// its impl in the convert crate's `scalar.rs`.
const STD_IMPLS: &str = r#"
impl ToLuaValue for char {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let mut buffer = [0u8; 4];
        Ok(Value::String(lua.create_string(self.encode_utf8(&mut buffer))?))
    }
}

impl ToLuaValue for std::path::Path {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let text = self
            .to_str()
            .ok_or_else(|| mlua::Error::runtime("a path is not valid UTF-8"))?;
        Ok(Value::String(lua.create_string(text)?))
    }
}

impl ToLuaValue for std::path::PathBuf {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        self.as_path().to_lua_value(lua)
    }
}
"#;

/// The tests of [`LIST_HELPER`], emitted with it: the cap is exact, and a list under it converts.
const LIST_TESTS: &str = r#"
#[cfg(test)]
mod list_tests {
    use super::list_to_lua;
    use mlua::{Lua, Value};

    #[test]
    fn a_list_at_the_cap_converts_and_one_past_it_is_an_error() {
        let lua = Lua::new();
        let full = vec![[1i32, 2, 3]; super::MAX_LIST_LEN];
        let Value::Table(table) = list_to_lua(&full, &lua).expect("at the cap") else {
            panic!("expected a table");
        };
        assert_eq!(table.raw_len(), super::MAX_LIST_LEN);
        let over = vec![[0i32, 0, 0]; super::MAX_LIST_LEN + 1];
        assert!(list_to_lua(&over, &lua).is_err());
    }

    #[test]
    fn a_list_is_a_one_based_array() {
        let lua = Lua::new();
        let items = [7u8, 8];
        let Value::Table(table) = list_to_lua(&items, &lua).expect("converts") else {
            panic!("expected a table");
        };
        assert_eq!(table.get::<u8>(1).expect("first"), 7);
        assert_eq!(table.get::<u8>(2).expect("second"), 8);
    }
}
"#;

/// The wildcard arm of a non-exhaustive enum: another crate may add variants, which is an error here.
fn unknown_arm(path: &str) -> String {
    format!("            _ => return Err(mlua::Error::runtime(\"unknown variant of {path}\")),\n")
}

/// One arm of a data enum's `match`: the tag, then the fields.
fn variant_arm(target: &str, variant: &Variant) -> String {
    let (pattern, bindings): (String, Vec<String>) = if variant.fields.is_empty() {
        if variant.named {
            (format!("{target}::{} {{}}", variant.name), Vec::new())
        } else {
            (format!("{target}::{}", variant.name), Vec::new())
        }
    } else if variant.named {
        let names: Vec<String> = variant.fields.iter().map(|f| f.name.clone()).collect();
        (
            format!("{target}::{} {{ {} }}", variant.name, names.join(", ")),
            names,
        )
    } else {
        let names: Vec<String> = (0..variant.fields.len()).map(|i| format!("f{i}")).collect();
        (
            format!("{target}::{}({})", variant.name, names.join(", ")),
            names,
        )
    };
    let mut out = format!("            {pattern} => {{\n");
    out.push_str(&format!(
        "                table.raw_set(\"tag\", {:?})?;\n",
        variant.lua
    ));
    for (index, field) in variant.fields.iter().enumerate() {
        let key = if variant.named {
            format!("{:?}", field.name)
        } else {
            (index + 1).to_string()
        };
        out.push_str(&format!(
            "                table.raw_set({key}, {})?;\n",
            value_expr(&bindings[index], field)
        ));
    }
    out.push_str("            }\n");
    out
}

/// The Lua value of `expr` (a field of `self`, or a binding), as an `mlua::Value`. Strings go
/// through `to_string` so `Arc<str>` and `&str` read the same as `String`. An optional reference
/// (`Option<&'a T>`) maps through `as_ref`, since `Option<&T>` has no impl of its own.
fn value_expr(expr: &str, field: &Field) -> String {
    let mapped = &field.ty;
    match &mapped.ty {
        TypeIr::Str => format!("{expr}.to_string().to_lua_value(lua)?"),
        // A borrowed slice is already a `&[T]`; an owned `Vec` is lent as one.
        TypeIr::List(_) if mapped.access == Access::Ref => format!("list_to_lua({expr}, lua)?"),
        TypeIr::List(_) => format!("list_to_lua(&{expr}, lua)?"),
        TypeIr::Option(inner) if matches!(**inner, TypeIr::Str) => {
            format!("{expr}.as_ref().map(|value| value.to_string()).to_lua_value(lua)?")
        }
        TypeIr::Option(_) if field.by_ref => format!(
            "{expr}.as_ref().map(|value| value.to_lua_value(lua)).transpose()?.unwrap_or(Value::Nil)"
        ),
        _ => format!("{expr}.to_lua_value(lua)?"),
    }
}

/// `snake_case` and the impl writer, with a golden set of names and a few small types. The file's
/// generated tests and the real surface are checked by the generator's own `--check` run.
#[cfg(test)]
mod tests {
    use super::*;

    /// Each Pascal name with the Lua name it must give.
    const GOLDEN: [(&str, &str); 14] = [
        ("ServerTickStart", "server_tick_start"),
        ("Vec3", "vec3"),
        ("XPRepair", "xp_repair"),
        ("HTTPServer", "http_server"),
        ("Field2Name", "field2_name"),
        ("Dimension3D", "dimension3d"),
        ("ItemV2", "item_v2"),
        ("PlayerV2Ok", "player_v2_ok"),
        ("KeyA", "key_a"),
        ("Ab", "ab"),
        ("A", "a"),
        ("Boolean", "boolean"),
        ("False", "false"),
        ("Clicked", "clicked"),
    ];

    #[test]
    fn snake_case_has_one_rule_for_the_tricky_names() {
        for (pascal, lua) in GOLDEN {
            assert_eq!(snake_case(pascal), lua, "for {pascal}");
        }
    }

    /// Three hand-picked types in the surface's record form: a struct, a unit enum and a data enum,
    /// plus a tuple struct that names the struct. The wire fields are the ones the surface writes.
    const MINI: &str = r#"[
      {"id": "type:Shade", "hash": "h1", "kind": "enum", "wire": true, "constructible": true,
       "variants": [
         {"id": "type:Shade.Light", "hash": "v1", "fields": []},
         {"id": "type:Shade.DarkSide", "hash": "v2", "fields": []}]},
      {"id": "type:Probe", "hash": "h2", "kind": "struct", "wire": false, "constructible": true,
       "fields": [
         {"id": "type:Probe.level", "hash": "f1", "type": "u32"},
         {"id": "type:Probe.shade", "hash": "f2", "type": "Shade"},
         {"id": "type:Probe.tag", "hash": "f3", "type": "Option<String>"}]},
      {"id": "type:Event", "hash": "h3", "kind": "enum", "wire": false, "constructible": true,
       "variants": [
         {"id": "type:Event.Moved", "hash": "v3", "named": true,
          "fields": [{"name": "dx", "type": "i32"}, {"name": "shade", "type": "Shade"}]},
         {"id": "type:Event.Pair", "hash": "v4",
          "fields": [{"name": "0", "type": "f32"}, {"name": "1", "type": "Probe"}]},
         {"id": "type:Event.Stop", "hash": "v5", "fields": []}]}
    ]"#;

    fn mini_types() -> Vec<Type> {
        serde_json::from_str(MINI).expect("the mini types parse")
    }

    #[test]
    fn three_hand_picked_types_convert_as_written() {
        let plan = plan(&mini_types(), &[], "abc");
        assert_eq!(plan.emitted, ["Event", "Probe", "Shade"]);
        assert!(plan.refused.is_empty(), "{:?}", plan.refused);
        let text = &plan.text;
        for piece in [
            "// @generated by event-horizon-lua-gen from surface abc: do not edit.\n",
            // A unit enum: a string from a match on the variants, both named.
            "impl ToLuaValue for mod_api::Shade {\n",
            "            mod_api::Shade::Light => \"light\",\n",
            "            mod_api::Shade::DarkSide => \"dark_side\",\n",
            "        Ok(Value::String(lua.create_string(name)?))\n",
            // A struct: keyed by its field names.
            "impl ToLuaValue for mod_api::Probe {\n",
            "        table.raw_set(\"level\", self.level.to_lua_value(lua)?)?;\n",
            "        table.raw_set(\"shade\", self.shade.to_lua_value(lua)?)?;\n",
            "        table.raw_set(\"tag\", self.tag.as_ref().map(|value| value.to_string()).to_lua_value(lua)?)?;\n",
            // A data enum: a tag, named fields by name, tuple fields by position.
            "            mod_api::Event::Moved { dx, shade } => {\n",
            "                table.raw_set(\"tag\", \"moved\")?;\n",
            "                table.raw_set(\"dx\", dx.to_lua_value(lua)?)?;\n",
            "            mod_api::Event::Pair(f0, f1) => {\n",
            "                table.raw_set(\"tag\", \"pair\")?;\n",
            "                table.raw_set(1, f0.to_lua_value(lua)?)?;\n",
            "                table.raw_set(2, f1.to_lua_value(lua)?)?;\n",
            "            mod_api::Event::Stop => {\n",
        ] {
            assert!(text.contains(piece), "missing {piece:?} in:\n{text}");
        }
        syn::parse_file(text).expect("the generated text parses");
    }

    #[test]
    fn a_decided_unit_enum_reads_its_names_and_a_data_enum_is_never_decided() {
        let types = mini_types();
        let plan = plan_decided(
            &types,
            &[],
            "abc",
            &[],
            &["Shade".to_owned(), "Event".to_owned()],
        );
        assert!(plan.plain.decides("Shade"));
        assert!(
            !plan.plain.decides("Event"),
            "a data enum has no FromLuaValue"
        );
        let text = &plan.text;
        for piece in [
            "use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};\n",
            "const SHADE_NAMES: &[&str] = &[\"light\", \"dark_side\"];\n",
            "impl FromLuaValue for mod_api::Shade {\n",
            "            \"dark_side\" => Ok(mod_api::Shade::DarkSide),\n",
            "            _ => Err(ConvertError::unknown_variant(&name, SHADE_NAMES)),\n",
            "fn shade_names_round_trip() {\n",
            "        check(&lua, super::SHADE_NAMES, vec![mod_api::Shade::Light, mod_api::Shade::DarkSide]);\n",
        ] {
            assert!(text.contains(piece), "missing {piece:?} in:\n{text}");
        }
        assert!(!text.contains("FromLuaValue for mod_api::Event"), "{text}");
        assert_eq!(text.matches("impl FromLuaValue").count(), 1);
        syn::parse_file(text).expect("the generated text parses");
    }

    #[test]
    fn an_undecided_unit_enum_gets_no_from_impl_and_the_file_keeps_its_old_imports() {
        let plan = plan(&mini_types(), &[], "abc");
        assert!(!plan.plain.decides("Shade"));
        assert!(!plan.text.contains("FromLuaValue"), "{}", plan.text);
        assert!(plan.text.starts_with("// @generated"));
        assert!(plan.text.contains("use crate::scalar::ToLuaValue;\n"));
    }

    #[test]
    fn a_type_in_a_cycle_is_refused_and_so_is_what_depends_on_it() {
        let cycle = r#"[
          {"id": "type:Ping", "hash": "a", "kind": "struct", "wire": false, "constructible": true,
           "fields": [{"id": "type:Ping.pong", "hash": "b", "type": "Pong"}]},
          {"id": "type:Pong", "hash": "c", "kind": "struct", "wire": false, "constructible": true,
           "fields": [{"id": "type:Pong.ping", "hash": "d", "type": "Ping"}]},
          {"id": "type:Wrap", "hash": "e", "kind": "struct", "wire": false, "constructible": true,
           "fields": [{"id": "type:Wrap.inner", "hash": "f", "type": "Ping"}]}
        ]"#;
        let types: Vec<Type> = serde_json::from_str(cycle).expect("parses");
        let plan = plan(&types, &[], "h");
        assert!(plan.emitted.is_empty());
        let names: Vec<&str> = plan.refused.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Ping", "Pong", "Wrap"]);
        assert!(plan.refused[2].1.contains("Ping"), "{:?}", plan.refused[2]);
    }

    /// A borrowed struct and a borrowed enum whose fields are copied, in the surface's record form.
    const BORROWED: &str = r#"[
      {"id": "type:Lent", "hash": "l1", "kind": "struct", "wire": false, "constructible": true,
       "generics": "<'a>",
       "fields": [
         {"id": "type:Lent.name", "hash": "f1", "type": "&'a str"},
         {"id": "type:Lent.menu", "hash": "f2", "type": "Option<&'a str>"},
         {"id": "type:Lent.cells", "hash": "f3", "type": "&'a[[i32; 3]]"},
         {"id": "type:Lent.path", "hash": "f4", "type": "&'a Path"}]},
      {"id": "type:Lend", "hash": "l2", "kind": "enum", "wire": false, "constructible": true,
       "generics": "<'a>",
       "variants": [
         {"id": "type:Lend.Named", "hash": "v1", "named": true,
          "fields": [{"name": "who", "type": "&'a str"}]},
         {"id": "type:Lend.Pair", "hash": "v2",
          "fields": [{"name": "0", "type": "&'a str"}, {"name": "1", "type": "u8"}]},
         {"id": "type:Lend.Stop", "hash": "v3", "fields": []}]}
    ]"#;

    #[test]
    fn a_borrowed_struct_and_enum_copy_their_fields_into_lua() {
        let types: Vec<Type> = serde_json::from_str(BORROWED).expect("parses");
        let plan = plan(&types, &[], "b1");
        assert_eq!(plan.emitted, ["Lend", "Lent"]);
        assert!(plan.refused.is_empty(), "{:?}", plan.refused);
        let text = &plan.text;
        for piece in [
            // The impls name the lifetime as `<'_>`.
            "impl ToLuaValue for mod_api::Lent<'_> {\n",
            "        table.raw_set(\"name\", self.name.to_string().to_lua_value(lua)?)?;\n",
            "        table.raw_set(\"menu\", self.menu.as_ref().map(|value| value.to_string()).to_lua_value(lua)?)?;\n",
            "        table.raw_set(\"cells\", list_to_lua(self.cells, lua)?)?;\n",
            "        table.raw_set(\"path\", self.path.to_lua_value(lua)?)?;\n",
            "impl ToLuaValue for mod_api::Lend<'_> {\n",
            "            mod_api::Lend::Named { who } => {\n",
            "                table.raw_set(\"who\", who.to_string().to_lua_value(lua)?)?;\n",
            "            mod_api::Lend::Pair(f0, f1) => {\n",
            "                table.raw_set(1, f0.to_string().to_lua_value(lua)?)?;\n",
            "                table.raw_set(2, f1.to_lua_value(lua)?)?;\n",
            "            mod_api::Lend::Stop => {\n",
            // The helper and the std impls the text needs.
            "fn list_to_lua<T: ToLuaValue>(items: &[T], lua: &Lua) -> mlua::Result<Value> {\n",
            "impl ToLuaValue for char {\n",
            "impl ToLuaValue for std::path::Path {\n",
            "mod list_tests {\n",
        ] {
            assert!(text.contains(piece), "missing {piece:?} in:\n{text}");
        }
        syn::parse_file(text).expect("the generated text parses");
    }

    /// A borrowed struct with a `&Text` (optional and not), and a `[f32; 4]`.
    #[test]
    fn a_text_reference_and_a_four_array_convert() {
        let text = r#"[
          {"id": "type:Carrier", "hash": "c1", "kind": "struct", "wire": false, "constructible": true,
           "generics": "<'a>",
           "fields": [
             {"id": "type:Carrier.title", "hash": "f1", "type": "&'a text::Text"},
             {"id": "type:Carrier.shown", "hash": "f2", "type": "Option<&'a text::Text>"},
             {"id": "type:Carrier.rect", "hash": "f3", "type": "[f32; 4]"}]}
        ]"#;
        let types: Vec<Type> = serde_json::from_str(text).expect("parses");
        let plan = plan(&types, &[], "t");
        assert_eq!(plan.emitted, ["Carrier"]);
        assert!(plan.refused.is_empty(), "{:?}", plan.refused);
        let text = &plan.text;
        for piece in [
            "        table.raw_set(\"title\", self.title.to_lua_value(lua)?)?;\n",
            "        table.raw_set(\"shown\", self.shown.as_ref().map(|value| value.to_lua_value(lua)).transpose()?.unwrap_or(Value::Nil))?;\n",
            "        table.raw_set(\"rect\", self.rect.to_lua_value(lua)?)?;\n",
            "impl ToLuaValue for [f32; 4] {\n",
            "            table.raw_set(index + 1, item.to_lua_value(lua)?)?;\n",
        ] {
            assert!(text.contains(piece), "missing {piece:?} in:\n{text}");
        }
        assert!(
            !text.contains("impl ToLuaValue for [f32; 2]"),
            "a hand-written impl is not repeated"
        );
        syn::parse_file(text).expect("the generated text parses");
    }

    #[test]
    fn a_loot_seed_is_a_decimal_string_and_other_64_bit_fields_stay_exact() {
        let text = r#"[
          {"id": "type:world::snapshots::LootContext", "hash": "a", "kind": "struct", "wire": false,
           "constructible": true,
           "fields": [{"id": "type:world::snapshots::LootContext.seed", "hash": "b", "type": "i64"},
                      {"id": "type:world::snapshots::LootContext.entity", "hash": "c", "type": "i64"}]}
        ]"#;
        let types: Vec<Type> = serde_json::from_str(text).expect("parses");
        let plan = plan(&types, &[], "h");
        assert!(
            plan.text
                .contains("table.raw_set(\"seed\", self.seed.to_string().to_lua_value(lua)?)?;"),
            "{}",
            plan.text
        );
        assert!(
            plan.text
                .contains("table.raw_set(\"entity\", self.entity.to_lua_value(lua)?)?;"),
            "an id stays on the exact integer rule: {}",
            plan.text
        );
    }

    #[test]
    fn a_scalar_array_named_by_a_hook_argument_gets_its_impl() {
        let plan = plan_with(
            &[],
            &[],
            "h",
            &["Option<[u8; 5]>".to_owned(), "[f64; 3]".to_owned()],
        );
        assert!(
            plan.text.contains("impl ToLuaValue for [u8; 5] {\n"),
            "{}",
            plan.text
        );
        assert!(
            !plan.text.contains("[f64; 3]"),
            "a hand-written impl is not repeated"
        );
        assert!(plan.emitted.is_empty());
    }

    #[test]
    fn a_reference_a_mutable_reference_or_a_type_parameter_stays_refused() {
        let text = r#"[
          {"id": "type:Borrowed", "hash": "a", "kind": "struct", "wire": false, "constructible": false,
           "generics": "<'a>", "fields": [{"id": "type:Borrowed.name", "hash": "b", "type": "&'a str"}]},
          {"id": "type:Lends", "hash": "c", "kind": "struct", "wire": false, "constructible": true,
           "generics": "<'a>", "fields": [{"id": "type:Lends.inner", "hash": "d", "type": "&'a Unknown"}]},
          {"id": "type:Dyn", "hash": "n", "kind": "struct", "wire": false, "constructible": true,
           "generics": "<'a>", "fields": [{"id": "type:Dyn.any", "hash": "o", "type": "&'a dyn Thing"}]},
          {"id": "type:Copies", "hash": "p", "kind": "struct", "wire": false, "constructible": true,
           "generics": "<'a>", "fields": [{"id": "type:Copies.inner", "hash": "q", "type": "&'a Borrowed<'a>"}]},
          {"id": "type:Mut", "hash": "e", "kind": "struct", "wire": false, "constructible": true,
           "generics": "<'a>", "fields": [{"id": "type:Mut.out", "hash": "f", "type": "&'a mut u8"}]},
          {"id": "type:Items", "hash": "g", "kind": "struct", "wire": false, "constructible": true,
           "generics": "<'a>", "fields": [{"id": "type:Items.names", "hash": "h", "type": "&'a [&'a str]"}]},
          {"id": "type:Generic", "hash": "i", "kind": "struct", "wire": false, "constructible": true,
           "generics": "<T>", "fields": [{"id": "type:Generic.value", "hash": "j", "type": "u8"}]},
          {"id": "type:Private", "hash": "k", "kind": "struct", "wire": false, "constructible": false},
          {"id": "type:Holds", "hash": "l", "kind": "struct", "wire": false, "constructible": true,
           "fields": [{"id": "type:Holds.r", "hash": "m", "type": "&'static str"}]}
        ]"#;
        let types: Vec<Type> = serde_json::from_str(text).expect("parses");
        let plan = plan(&types, &[], "h");
        assert_eq!(plan.emitted, ["Borrowed", "Copies"]);
        let reason = |name: &str| {
            plan.refused
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, r)| r.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        assert!(
            reason("Lends").contains("no Lua conversion"),
            "{:?}",
            plan.refused
        );
        assert!(reason("Dyn").contains("dyn"), "{:?}", plan.refused);
        assert!(
            reason("Mut").contains("mutable reference"),
            "{:?}",
            plan.refused
        );
        assert!(
            reason("Items").contains("list of references"),
            "{:?}",
            plan.refused
        );
        assert!(
            reason("Generic").contains("type parameter"),
            "{:?}",
            plan.refused
        );
        assert!(reason("Private").contains("private"), "{:?}", plan.refused);
        assert!(reason("Holds").contains("not owned"), "{:?}", plan.refused);
    }

    #[test]
    fn a_keyword_field_and_a_variant_name_clash_are_refused() {
        let text = r#"[
          {"id": "type:Kw", "hash": "a", "kind": "struct", "wire": false, "constructible": true,
           "fields": [{"id": "type:Kw.type", "hash": "b", "type": "u8"}]},
          {"id": "type:Clash", "hash": "c", "kind": "enum", "wire": false, "constructible": true,
           "variants": [{"id": "type:Clash.AB", "hash": "d", "fields": []},
                        {"id": "type:Clash.Ab", "hash": "e", "fields": []}]}
        ]"#;
        let types: Vec<Type> = serde_json::from_str(text).expect("parses");
        let plan = plan(&types, &[], "h");
        let reason = |name: &str| {
            plan.refused
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, r)| r.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        assert!(reason("Kw").contains("keyword"), "{:?}", plan.refused);
        assert!(
            reason("Clash").contains("both named `ab`"),
            "{:?}",
            plan.refused
        );
    }
}
