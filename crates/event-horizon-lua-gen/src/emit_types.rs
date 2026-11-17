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
//! Variant names are [`snake_case`] of the Rust name. The surface's own docs do not give stable
//! names (only 39 of 202 backticked unit variants match), so the rule is the one name source.
//!
//! [`plan`] returns the file's text and the [`Plain`] set the method emitter uses to accept a named
//! type as convertible. Nothing here compiles the text: the tests parse it with `syn`, and the
//! convert crate compiles it.

use std::collections::{BTreeMap, BTreeSet};

use mod_surface::{Function, Type, TypeKind};

use crate::emit_impl::convertible;
use crate::types::{Access, Mapped, TypeIr, map_type};

/// The names of the plain types. The method emitter asks it whether a named type converts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plain(BTreeSet<String>);

impl Plain {
    /// A set of the given paths, for tests and for the method emitter's own checks.
    pub fn from_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        Plain(paths.into_iter().map(str::to_owned).collect())
    }

    /// Whether the type at this path (as the mapper writes it, such as `LevelDimension`) is plain.
    pub fn contains(&self, path: &str) -> bool {
        self.0.contains(path)
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

/// One field of a struct or a variant: its name (a tuple field's is its position) and its type.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Field {
    name: String,
    ty: Mapped,
}

/// One variant of an enum.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Variant {
    /// The Rust name.
    name: String,
    /// The Lua name: [`snake_case`] of `name`.
    lua: String,
    /// Its fields have names (`{ … }`), not positions.
    named: bool,
    fields: Vec<Field>,
}

/// The shape of a plain type.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Body {
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

/// A candidate: its path, the shape, and the field types it names (the leaves the fixed point checks).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Def {
    path: String,
    hash: String,
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

/// Plans the plain types of `types` (the surface's `types`, in any order) and writes their impls.
/// `handles` are the surface's handle functions, which name the getters of private newtypes. The
/// same input always gives the same text.
pub fn plan(types: &[Type], handles: &[Function], hash: &str) -> TypesPlan {
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
        let plain = Plain(accepted.keys().cloned().collect());
        let ready: Vec<String> = pending
            .iter()
            .filter(|(_, def)| first_blocker(&def.body, &plain).is_none())
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
    let plain = Plain(accepted.keys().cloned().collect());
    for (path, def) in &pending {
        let reason = match first_blocker(&def.body, &plain) {
            Some(reason) => reason,
            None => "no conversion".to_owned(),
        };
        refused.insert(path.clone(), reason);
    }

    let mut text =
        format!("// @generated by event-horizon-lua-gen from surface {hash}: do not edit.\n");
    text.push_str("use crate::scalar::ToLuaValue;\nuse mlua::{Lua, Value};\n");
    for def in accepted.values() {
        text.push_str(&impl_text(def));
    }
    text.push_str(RUNTIME_TESTS);

    TypesPlan {
        text,
        emitted: accepted.keys().cloned().collect(),
        refused: refused.into_iter().collect(),
        plain,
    }
}

/// Reads a surface type's shape, or the reason it is not plain.
fn describe(ty: &Type, handles: &[Function]) -> Result<Body, String> {
    if !ty.contract.generics.is_empty() {
        return Err("has a lifetime or type parameter (a borrowed type)".to_owned());
    }
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
                    field_of(name, &field.contract.ty)
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
                .map(|(index, field)| field_of(index.to_string(), &field.contract.ty))
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
                        field_of(field_name, &arg.ty)
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

/// One field: its type must be owned (no `&`) and map to a type the mapper knows.
fn field_of(name: String, text: &str) -> Result<Field, String> {
    if KEYWORDS.contains(&name.as_str()) {
        return Err(format!("field `{name}` is a Rust keyword"));
    }
    if text.contains('&') {
        return Err(format!(
            "field `{name}` has a reference type `{text}`: not owned"
        ));
    }
    let ty = map_type(text).map_err(|error| format!("field `{name}`: {error}"))?;
    if ty.access != Access::Owned {
        return Err(format!(
            "field `{name}` has a reference type `{text}`: not owned"
        ));
    }
    Ok(Field { name, ty })
}

/// The reason the body is not yet convertible given `plain`, or `None` when every field converts.
fn first_blocker(body: &Body, plain: &Plain) -> Option<String> {
    let fields: Vec<&Field> = match body {
        Body::Unit | Body::Getter { .. } => Vec::new(),
        Body::Named(fields) | Body::Tuple(fields) => fields.iter().collect(),
        Body::Enum { variants, .. } => variants.iter().flat_map(|v| v.fields.iter()).collect(),
    };
    fields
        .into_iter()
        .find(|field| !convertible(&field.ty.ty, plain))
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
    let mut out = format!("\nimpl ToLuaValue for {target} {{\n");
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
                out.push_str(&format!(
                    "        table.raw_set({:?}, {})?;\n",
                    field.name,
                    value_expr(&format!("self.{}", field.name), &field.ty)
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
                    value_expr(&format!("self.{}", field.name), &field.ty)
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
            value_expr(&bindings[index], &field.ty)
        ));
    }
    out.push_str("            }\n");
    out
}

/// The Lua value of `expr` (a field of `self`, or a binding), as an `mlua::Value`. Strings go
/// through `to_string` so `Arc<str>` and `&str` read the same as `String`.
fn value_expr(expr: &str, mapped: &Mapped) -> String {
    match &mapped.ty {
        TypeIr::Str => format!("{expr}.to_string().to_lua_value(lua)?"),
        TypeIr::Option(inner) if matches!(**inner, TypeIr::Str) => {
            format!("{expr}.as_ref().map(|value| value.to_string()).to_lua_value(lua)?")
        }
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

    #[test]
    fn a_borrowed_or_private_or_reference_field_type_is_refused() {
        let text = r#"[
          {"id": "type:Borrowed", "hash": "a", "kind": "struct", "wire": false, "constructible": false,
           "generics": "<'a>", "fields": [{"id": "type:Borrowed.name", "hash": "b", "type": "String"}]},
          {"id": "type:Private", "hash": "c", "kind": "struct", "wire": false, "constructible": false},
          {"id": "type:Holds", "hash": "d", "kind": "struct", "wire": false, "constructible": true,
           "fields": [{"id": "type:Holds.r", "hash": "e", "type": "&'static str"}]}
        ]"#;
        let types: Vec<Type> = serde_json::from_str(text).expect("parses");
        let plan = plan(&types, &[], "h");
        assert!(plan.emitted.is_empty());
        let reason = |name: &str| {
            plan.refused
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, r)| r.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        assert!(
            reason("Borrowed").contains("lifetime"),
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
