//! The Luau type definitions of the Lua mod API (`horizon.d.luau`, issue 74, gate G2), written from the
//! same plan as the Rust glue: the emitted hooks' event types and the `horizon.on` overloads, the
//! Part handler tables with their events, and the bound handle members as one type per family.
//!
//! The file is the hand-written [`STATIC_API`] (the `horizon` global and the value shapes of the convert
//! crate) followed by the generated part. A Rust type with no Lua name is written `any`, with the
//! Rust type in a comment, and counted in [`Emitted::any`].

use std::collections::{BTreeMap, BTreeSet};

use crate::carried::{self, Shape};
use crate::emit_hooks::camel;
use crate::emit_impl::{EventShape, command_input, is_block_click, scalar_name};
use crate::emit_types::{Body, Field, Plain, Variant};
use crate::handles::MemberIr;
use crate::types::{FloatKind, IntKind, TypeIr, map_type};

/// The hand-written part of the file: the `horizon` global and the convert crate's value shapes.
pub const STATIC_API: &str = include_str!("horizon_api.d.luau");

/// The `.luaurc` a mod folder may carry (`crates/event-horizon-lua/lua-types/.luaurc.example`). Untested
/// against luau-lsp: no Luau checker is on the box.
pub const LUAURC_EXAMPLE: &str = "{\n  \"languageMode\": \"strict\"\n}\n";

/// The plain types the hand-written part names (`BlockClick` in the static file). Each is written
/// even when no hook names it, so the static part's names always resolve; a name that is not a plain
/// type is an error.
pub const STATIC_REFERENCES: &[&str] = &["BlockHit", "Hand", "LevelDimension", "LogicalSide"];

/// The six Part traits, in the order of [`Input::parts`]: the `horizon` function that declares a
/// Part of the kind, and the handler table type it takes.
pub const PART_KINDS: [(&str, &str); 6] = [
    ("block_part", "BlockPartHandlers"),
    ("item_part", "ItemPartHandlers"),
    ("block_entity_part", "BlockEntityPartHandlers"),
    ("fluid_part", "FluidPartHandlers"),
    ("entity_part", "EntityPartHandlers"),
    ("menu_part", "MenuPartHandlers"),
];

/// What the generated part is written from.
pub struct Input<'a> {
    /// The surface's hash, for the header.
    pub hash: &'a str,
    /// The Hooks trait's emitted hooks, in name order.
    pub hooks: &'a [EventShape],
    /// The emitted methods of each Part trait, in [`PART_KINDS`] order.
    pub parts: [&'a [EventShape]; 6],
    /// The generated handle families, in order.
    pub families: &'a [String],
    /// The bound handle members, in family and name order.
    pub members: &'a [&'a MemberIr],
    /// The plain types: a typed mention of one names its Luau type, which is written with the file.
    pub plain: &'a Plain,
}

/// The file, and what the mapper could not name.
#[derive(Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The whole `horizon.d.luau`.
    pub text: String,
    /// Each Rust type written `any`, one entry per occurrence, in the order written.
    pub any: Vec<String>,
    /// The hooks given an event type and an overload.
    pub hooks: usize,
    /// The Part handler tables' entries, all kinds.
    pub parts: usize,
    /// The handle members given a method type.
    pub members: usize,
    /// The plain types a typed mention names, each written as an `export type`.
    pub named: usize,
}

/// The mapper's state: the families it can name, the plain types it can name and the names they
/// take, the plain types it has named so far, and the `any` it has written.
struct Mapper<'a> {
    families: &'a [String],
    /// Each plain type's Luau name, by its path. A type with no entry stays `any`.
    names: BTreeMap<String, String>,
    used: BTreeSet<String>,
    any: Vec<String>,
}

impl Mapper<'_> {
    /// The Luau type of a Rust type text. Anything it cannot name is `any`, and the text is recorded.
    fn ty(&mut self, text: &str) -> String {
        let text = text.replace("mod_api::", "");
        if let Some(shape) = carried::shape_of(&text) {
            return match shape {
                Shape::Stack => "ItemStackSpec".to_owned(),
                Shape::OptStack => "ItemStackSpec?".to_owned(),
                Shape::StackList => "{ItemStackSpec}".to_owned(),
                Shape::State => "BlockStateSpec".to_owned(),
                Shape::OptState => "BlockStateSpec?".to_owned(),
            };
        }
        if let Some(kind) = command_input(&text) {
            return if kind == "source" {
                "CommandSource".to_owned()
            } else {
                "CommandInvocation".to_owned()
            };
        }
        if is_block_click(&text) {
            return "BlockClick".to_owned();
        }
        match map_type(&text) {
            Ok(mapped) => self.of(&mapped.ty, &text),
            Err(_) => self.unknown(&text),
        }
    }

    fn of(&mut self, ty: &TypeIr, text: &str) -> String {
        match ty {
            TypeIr::Bool => "boolean".to_owned(),
            TypeIr::Int(IntKind::U128) => "string".to_owned(),
            TypeIr::Int(_) if scalar_name(ty).is_none() => self.unknown(text),
            TypeIr::Int(_) | TypeIr::Float(FloatKind::F32 | FloatKind::F64) => "number".to_owned(),
            TypeIr::Str => "string".to_owned(),
            TypeIr::Option(inner) => format!("{}?", self.of(inner, text)),
            TypeIr::List(elem) => format!("{{{}}}", self.of(elem, text)),
            TypeIr::Array(elem, len) => {
                let xyz = matches!(
                    (scalar_name(elem), len),
                    (Some("i32" | "f64" | "f32"), 2 | 3)
                );
                if xyz && *len == 3 {
                    "{ x: number, y: number, z: number }".to_owned()
                } else if xyz {
                    "{ x: number, y: number }".to_owned()
                } else {
                    format!("{{{}}}", self.of(elem, text))
                }
            }
            TypeIr::Named(name) => self.named(name),
            _ => self.unknown(text),
        }
    }

    fn named(&mut self, name: &str) -> String {
        match name.replace("mod_api::", "").as_str() {
            "BlockPos" => "BlockPos".to_owned(),
            "Rotation" => "Rotation".to_owned(),
            "Compound" => "Compound".to_owned(),
            "Text" | "text::Text" => "Text".to_owned(),
            "Identifier" | "BlockEntityTypeKey" | "char" | "Path" | "PathBuf" => {
                "string".to_owned()
            }
            "Duration" => "number".to_owned(),
            "BlockState" => "BlockStateSpec".to_owned(),
            "ItemStack" => "ItemStackSpec".to_owned(),
            "ConfigFile" => "ConfigFile".to_owned(),
            other if self.families.iter().any(|family| family == other) => other.to_owned(),
            other => match self.names.get(other).cloned() {
                Some(luau) => {
                    self.used.insert(other.to_owned());
                    luau
                }
                None => self.unknown(name),
            },
        }
    }

    /// `any`, with the Rust type recorded so the comment and the count name it.
    fn unknown(&mut self, text: &str) -> String {
        self.any.push(text.trim().to_owned());
        "any".to_owned()
    }

    /// A field or method argument line's type, with a comment when it is `any`.
    fn typed(&mut self, text: &str) -> (String, String) {
        let before = self.any.len();
        let ty = self.ty(text);
        let note = if self.any.len() > before {
            format!(" -- Rust: {}", self.any[before..].join(", "))
        } else {
            String::new()
        };
        (ty, note)
    }
}

/// Luau's reserved words: a key spelled as one is written `["key"]`.
const RESERVED: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function", "if",
    "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// A table key: the name, or `["name"]` when the name is reserved.
fn key(name: &str) -> String {
    if RESERVED.contains(&name) {
        format!("[\"{name}\"]")
    } else {
        name.to_owned()
    }
}

/// The event type of a hook or Part method: `<CamelName>Event`.
fn event_name(hook: &str) -> String {
    format!("{}Event", camel(hook))
}

/// One event type: the read-only fields, then the writable outputs (`-- answer`).
fn event_type(shape: &EventShape, mapper: &mut Mapper<'_>) -> String {
    let mut text = format!("export type {} = {{\n", event_name(&shape.hook));
    for (name, rust) in &shape.fields {
        let (ty, note) = if rust == "@feedback" {
            ("Feedback".to_owned(), String::new())
        } else {
            mapper.typed(rust)
        };
        text.push_str(&format!("\t{}: {ty}, -- read-only{note}\n", key(name)));
    }
    for (name, rust) in &shape.outputs {
        let (ty, note) = mapper.typed(rust);
        // An optional output also takes `horizon.none`, which clears it.
        let ty = match ty.strip_suffix('?') {
            Some(inner) => format!("{inner} | NoneMarker?"),
            None => ty,
        };
        text.push_str(&format!("\t{}: {ty}, -- answer{note}\n", key(name)));
    }
    text.push_str("}\n\n");
    text
}

/// The handler type of a method: the event, and the answer a replace hook returns.
fn handler(shape: &EventShape, mapper: &mut Mapper<'_>) -> String {
    let answer = match &shape.answer {
        Some(answer) => format!("({})?", mapper.typed(answer).0),
        None => "()".to_owned(),
    };
    format!("(event: {}) -> {answer}", event_name(&shape.hook))
}

/// `export type OnHook`: one overload per emitted hook, so an unknown name matches none.
fn on_overloads(hooks: &[EventShape], mapper: &mut Mapper<'_>) -> String {
    let mut text = String::from("export type OnHook =\n");
    let lines: Vec<String> = hooks
        .iter()
        .map(|shape| {
            format!(
                "\t((name: \"{}\", handler: {}, options: HandlerOptions?) -> ())",
                shape.hook,
                handler(shape, mapper)
            )
        })
        .collect();
    text.push_str(&lines.join(" &\n"));
    text.push_str("\n\n");
    text
}

/// One Part trait's handler table: each emitted method's handler, optional.
fn part_table(name: &str, shapes: &[EventShape], mapper: &mut Mapper<'_>) -> String {
    let mut text = format!("export type {name} = {{\n");
    for shape in shapes {
        text.push_str(&format!(
            "\t{}: ({})?,\n",
            key(&shape.hook),
            handler(shape, mapper)
        ));
    }
    text.push_str("}\n\n");
    text
}

/// One handle family's type: a method per bound member, the receiver first. A host argument is
/// left out: the host is the turn's, not the mod's.
fn family_type(family: &str, members: &[&MemberIr], mapper: &mut Mapper<'_>) -> String {
    let mut text = format!("export type {family} = {{\n");
    for member in members {
        let skip = usize::from(member.params.first().is_some_and(|first| {
            matches!(
                first
                    .rust_type
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .as_str(),
                "&dyn Host" | "&mut dyn ServerHost"
            )
        }));
        let mut args = vec![family.to_owned()];
        let mut notes = String::new();
        for param in &member.params[skip..] {
            let (ty, note) = mapper.typed(&param.rust_type);
            args.push(ty);
            notes.push_str(&note);
        }
        let ret = match member.ret.as_deref() {
            None | Some("()") => "()".to_owned(),
            Some(text) => {
                let value = crate::emit_handles::ok_type(text).unwrap_or(text);
                if value == "()" {
                    "()".to_owned()
                } else {
                    mapper.typed(value).0
                }
            }
        };
        text.push_str(&format!(
            "\t{}: ({}) -> {ret},{notes}\n",
            key(&member.name),
            args.join(", ")
        ));
    }
    text.push_str("}\n\n");
    text
}

/// One plain type's Luau type. A struct is a table keyed by its field names; a unit struct is an
/// empty table; a tuple struct is an array of its field types (`{ A | B }` when they differ); a
/// newtype whose value is its getter's is that type. An enum with no data is a union of its
/// variants' names (`"main" | "off"`); any other enum is a union of tables, each with a `tag`.
fn plain_type(name: &str, body: &Body, mapper: &mut Mapper<'_>) -> String {
    match body {
        Body::Unit => format!("export type {name} = {{}}\n\n"),
        Body::Named(fields) => {
            let mut text = format!("export type {name} = {{\n");
            for field in fields {
                let (ty, note) = mapper.typed(&field.text);
                text.push_str(&format!("\t{}: {ty},{note}\n", key(&field.name)));
            }
            text.push_str("}\n\n");
            text
        }
        Body::Tuple(fields) => {
            let (union, notes) = positional(fields, mapper);
            format!("export type {name} = {{ {union} }}{notes}\n\n")
        }
        Body::Getter { ty, .. } => format!("export type {name} = {}\n\n", mapper.of(ty, "")),
        Body::Enum { variants, .. } => {
            if variants.iter().all(|variant| variant.fields.is_empty()) {
                let names: Vec<String> = variants
                    .iter()
                    .map(|variant| format!("\"{}\"", variant.lua))
                    .collect();
                return format!("export type {name} = {}\n\n", names.join(" | "));
            }
            let mut text = format!("export type {name} =\n");
            for variant in variants {
                let (line, notes) = variant_table(variant, mapper);
                text.push_str(&format!("\t| {line}{notes}\n"));
            }
            text.push('\n');
            text
        }
    }
}

/// The union of a tuple's field types, and the notes of any `any` among them.
fn positional(fields: &[Field], mapper: &mut Mapper<'_>) -> (String, String) {
    let mut types = Vec::new();
    let mut notes = String::new();
    for field in fields {
        let (ty, note) = mapper.typed(&field.text);
        types.push(ty);
        notes.push_str(&note);
    }
    (types.join(" | "), notes)
}

/// One variant of a data enum as a table type: its `tag`, then its fields by name, or a
/// `[number]` indexer for a tuple variant. The notes go after the table, so they never sit inside it.
fn variant_table(variant: &Variant, mapper: &mut Mapper<'_>) -> (String, String) {
    let mut parts = vec![format!("tag: \"{}\"", variant.lua)];
    let mut notes = String::new();
    if variant.named {
        for field in &variant.fields {
            let (ty, note) = mapper.typed(&field.text);
            notes.push_str(&note);
            parts.push(format!("{}: {ty}", key(&field.name)));
        }
    } else if !variant.fields.is_empty() {
        let (union, positional_notes) = positional(&variant.fields, mapper);
        notes.push_str(&positional_notes);
        parts.push(format!("[number]: {union}"));
    }
    (format!("{{ {} }}", parts.join(", ")), notes)
}

/// The Luau name of each plain type: its last path segment, when that is one name among the plain
/// types and no name the file already defines (`reserved`). Any other plain type stays `any`.
fn plain_names(plain: &Plain, reserved: &BTreeSet<String>) -> BTreeMap<String, String> {
    let last = |path: &str| path.rsplit("::").next().unwrap_or(path).to_owned();
    let mut count = BTreeMap::<String, usize>::new();
    for path in plain.bodies().keys() {
        *count.entry(last(path)).or_default() += 1;
    }
    plain
        .bodies()
        .keys()
        .filter_map(|path| {
            let name = last(path);
            let unique = count.get(&name) == Some(&1) && !reserved.contains(&name);
            unique.then(|| (path.clone(), name))
        })
        .collect()
}

/// Names the file defines already, which a plain type may not take: the hand-written part's
/// `export type`s, the generated families, the Part tables, the events and the `OnHook` overloads.
fn reserved_names(input: &Input<'_>) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = STATIC_API
        .lines()
        .filter_map(|line| line.strip_prefix("export type "))
        .map(|rest| rest.split([' ', '<']).next().unwrap_or_default().to_owned())
        .collect();
    names.extend(input.families.iter().cloned());
    names.extend(PART_KINDS.iter().map(|(_, table)| (*table).to_owned()));
    names.extend(input.hooks.iter().map(|shape| event_name(&shape.hook)));
    for shapes in input.parts {
        names.extend(shapes.iter().map(|shape| event_name(&shape.hook)));
    }
    for name in [
        "OnHook", "any", "boolean", "buffer", "never", "nil", "number", "string", "thread",
        "unknown",
    ] {
        names.insert(name.to_owned());
    }
    names
}

/// Writes the file: the hand-written part, then the generated one. Two events, two Parts or two
/// families may not share a name (the camel case of a hook is its event type's name).
pub fn emit(input: &Input<'_>) -> Result<Emitted, String> {
    emit_with(input, STATIC_REFERENCES)
}

/// [`emit`], naming the plain types the given `references` name as written, instead of the
/// static part's own.
fn emit_with(input: &Input<'_>, references: &[&str]) -> Result<Emitted, String> {
    let mut names = BTreeSet::new();
    for shape in input
        .hooks
        .iter()
        .chain(input.parts.iter().flat_map(|s| s.iter()))
    {
        if !names.insert(event_name(&shape.hook)) {
            return Err(format!(
                "two hooks share the event type {}",
                event_name(&shape.hook)
            ));
        }
    }
    let mut mapper = Mapper {
        families: input.families,
        names: plain_names(input.plain, &reserved_names(input)),
        used: BTreeSet::new(),
        any: Vec::new(),
    };
    let mut text = format!(
        "-- @generated by event-horizon-lua-gen from surface {}: do not edit.\n-- Hand-written part first, then the generated part.\n\n",
        input.hash
    );
    text.push_str(STATIC_API);
    text.push_str("\n-- The generated part: one event type per emitted hook and Part method.\n\n");
    for shape in input.hooks {
        text.push_str(&event_type(shape, &mut mapper));
    }
    text.push_str(&on_overloads(input.hooks, &mut mapper));
    let mut parts = 0;
    for ((_, table), shapes) in PART_KINDS.iter().zip(input.parts) {
        for shape in shapes {
            text.push_str(&event_type(shape, &mut mapper));
        }
        text.push_str(&part_table(table, shapes, &mut mapper));
        parts += shapes.len();
    }
    for family in input.families {
        let members: Vec<&MemberIr> = input
            .members
            .iter()
            .copied()
            .filter(|member| member.owner == *family)
            .collect();
        let mut members = members;
        members.sort_by(|a, b| a.name.cmp(&b.name));
        text.push_str(&family_type(family, &members, &mut mapper));
    }
    // A plain type a mention names is written once; its own fields may name more, so this runs
    // until no new type is named.
    for name in references {
        let path = mapper
            .names
            .iter()
            .find_map(|(path, luau)| (luau == name).then(|| path.clone()))
            .ok_or_else(|| format!("the static part names {name}, which is not a plain type"))?;
        mapper.used.insert(path);
    }
    let mut done = BTreeSet::new();
    loop {
        let next: Vec<String> = mapper.used.difference(&done).cloned().collect();
        if next.is_empty() {
            break;
        }
        for path in next {
            let Some(body) = input.plain.bodies().get(&path) else {
                continue;
            };
            let name = mapper.names[&path].clone();
            text.push_str(&plain_type(&name, body, &mut mapper));
            done.insert(path);
        }
    }
    Ok(Emitted {
        text,
        any: mapper.any,
        hooks: input.hooks.len(),
        parts,
        members: input.members.len(),
        named: done.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handles::parse_signature;
    use std::collections::BTreeMap;

    fn shape(
        hook: &str,
        fields: &[(&str, &str)],
        outputs: &[(&str, &str)],
        answer: Option<&str>,
    ) -> EventShape {
        EventShape {
            hook: hook.to_owned(),
            fields: fields
                .iter()
                .map(|(k, t)| ((*k).to_owned(), (*t).to_owned()))
                .collect(),
            outputs: outputs
                .iter()
                .map(|(k, t)| ((*k).to_owned(), (*t).to_owned()))
                .collect(),
            answer: answer.map(str::to_owned),
        }
    }

    fn mapper(families: &[String]) -> Mapper<'_> {
        mapper_naming(families, &[])
    }

    /// A mapper that names the given plain types (path, Luau name).
    fn mapper_naming<'a>(families: &'a [String], names: &[(&str, &str)]) -> Mapper<'a> {
        Mapper {
            families,
            names: names
                .iter()
                .map(|(path, name)| ((*path).to_owned(), (*name).to_owned()))
                .collect(),
            used: BTreeSet::new(),
            any: Vec::new(),
        }
    }

    fn field(name: &str, text: &str) -> Field {
        Field {
            name: name.to_owned(),
            text: text.to_owned(),
            ty: map_type(text).expect("a type"),
            by_ref: false,
        }
    }

    fn unit_variant(name: &str) -> Variant {
        Variant {
            name: name.to_owned(),
            lua: crate::emit_types::snake_case(name),
            named: false,
            fields: Vec::new(),
        }
    }

    #[test]
    fn a_unit_enum_is_a_union_of_its_snake_case_names() {
        let mut m = mapper(&[]);
        let hand = Body::Enum {
            non_exhaustive: false,
            variants: vec![unit_variant("Main"), unit_variant("OffHand")],
        };
        assert_eq!(
            plain_type("Hand", &hand, &mut m),
            "export type Hand = \"main\" | \"off_hand\"\n\n"
        );
        assert!(m.any.is_empty());
    }

    #[test]
    fn a_data_enum_is_a_union_of_tagged_tables() {
        let families: [String; 0] = [];
        let mut m = mapper_naming(&families, &[("Hand", "Hand")]);
        let action = Body::Enum {
            non_exhaustive: true,
            variants: vec![
                unit_variant("Idle"),
                Variant {
                    name: "Hurt".to_owned(),
                    lua: "hurt".to_owned(),
                    named: true,
                    fields: vec![field("amount", "f32")],
                },
                Variant {
                    name: "Throw".to_owned(),
                    lua: "throw".to_owned(),
                    named: false,
                    fields: vec![field("0", "Hand")],
                },
            ],
        };
        assert_eq!(
            plain_type("Action", &action, &mut m),
            "export type Action =\n\
             \t| { tag: \"idle\" }\n\
             \t| { tag: \"hurt\", amount: number }\n\
             \t| { tag: \"throw\", [number]: Hand }\n\n"
        );
        assert_eq!(m.used.iter().collect::<Vec<_>>(), [&"Hand".to_owned()]);
    }

    #[test]
    fn a_struct_with_an_option_field_is_an_optional_key() {
        let mut m = mapper(&[]);
        let label = Body::Named(vec![field("label", "Option<String>")]);
        assert_eq!(
            plain_type("Label", &label, &mut m),
            "export type Label = {\n\tlabel: string?,\n}\n\n"
        );
    }

    #[test]
    fn a_reserved_word_field_is_a_quoted_key_in_a_struct() {
        let mut m = mapper(&[]);
        let span = Body::Named(vec![field("end", "i32"), field("start", "i32")]);
        assert_eq!(
            plain_type("Span", &span, &mut m),
            "export type Span = {\n\t[\"end\"]: number,\n\tstart: number,\n}\n\n"
        );
    }

    #[test]
    fn a_nested_plain_type_is_named_and_written_once() {
        let inner = Body::Named(vec![field("x", "i32")]);
        let outer = Body::Named(vec![field("inner", "Inner"), field("again", "Inner")]);
        let plain = Plain::from_bodies(BTreeMap::from([
            ("Inner".to_owned(), inner),
            ("Outer".to_owned(), outer),
        ]));
        let hooks = [shape("probe", &[("outer", "Outer")], &[], None)];
        let emitted = emit_with(
            &Input {
                hash: "test",
                hooks: &hooks,
                parts: [&[], &[], &[], &[], &[], &[]],
                families: &[],
                members: &[],
                plain: &plain,
            },
            &[],
        )
        .expect("emits");
        let tail = emitted
            .text
            .split("-- The generated part")
            .nth(1)
            .expect("the generated part");
        assert!(tail.contains("export type Outer = {\n\tinner: Inner,\n\tagain: Inner,\n}\n\n"));
        assert!(tail.contains("export type Inner = {\n\tx: number,\n}\n\n"));
        assert_eq!(tail.matches("export type Inner =").count(), 1);
        assert_eq!(emitted.named, 2);
        assert!(emitted.any.is_empty());
    }

    #[test]
    fn a_name_the_static_part_uses_must_be_a_plain_type() {
        let plain = Plain::from_bodies(BTreeMap::new());
        let result = emit(&Input {
            hash: "test",
            hooks: &[],
            parts: [&[], &[], &[], &[], &[], &[]],
            families: &[],
            members: &[],
            plain: &plain,
        });
        assert!(result.is_err_and(|error| error.contains("the static part names BlockHit")));
    }

    #[test]
    fn a_refused_type_stays_any_with_its_rust_name() {
        let mut m = mapper(&[]);
        let odd = Body::Named(vec![field("thing", "Refused")]);
        assert_eq!(
            plain_type("Holder", &odd, &mut m),
            "export type Holder = {\n\tthing: any, -- Rust: Refused\n}\n\n"
        );
        assert_eq!(m.any, ["Refused"]);
    }

    #[test]
    fn an_optional_output_also_takes_horizon_none() {
        let families: [String; 0] = [];
        let mut m = mapper_naming(&families, &[("Hand", "Hand")]);
        let shape = shape("pick_hand", &[], &[("hand", "Option<Hand>")], None);
        assert_eq!(
            event_type(&shape, &mut m),
            "export type PickHandEvent = {\n\thand: Hand | NoneMarker?, -- answer\n}\n\n"
        );
    }

    #[test]
    fn a_cancel_hook_writes_its_flag_as_an_answer() {
        let families = ["EntityRef".to_owned()];
        let mut m = mapper(&families);
        let cancel = shape(
            "animal_tame",
            &[("entity", "EntityRef"), ("kind", "&str")],
            &[("cancelled", "bool")],
            None,
        );
        assert_eq!(
            event_type(&cancel, &mut m),
            "export type AnimalTameEvent = {\n\
             \tentity: EntityRef, -- read-only\n\
             \tkind: string, -- read-only\n\
             \tcancelled: boolean, -- answer\n\
             }\n\n"
        );
        assert!(m.any.is_empty());
    }

    #[test]
    fn a_list_output_is_a_writable_array_of_text() {
        let mut m = mapper(&[]);
        let collect = shape("block_lines", &[], &[("lines", "Vec<Text>")], None);
        assert_eq!(
            event_type(&collect, &mut m),
            "export type BlockLinesEvent = {\n\tlines: {Text}, -- answer\n}\n\n"
        );
    }

    #[test]
    fn a_handle_has_a_read_and_a_write_and_drops_the_host() {
        let families = ["PlayerRef".to_owned()];
        let mut m = mapper(&families);
        let read = parse_signature(
            "PlayerRef",
            "name",
            "fn name(self, _: &dyn Host) -> Option<String>",
        )
        .expect("a signature");
        let write = parse_signature(
            "PlayerRef",
            "set_name",
            "fn set_name(self, _: &mut dyn ServerHost, _: &str) -> ()",
        )
        .expect("a signature");
        assert_eq!(
            family_type("PlayerRef", &[&read, &write], &mut m),
            "export type PlayerRef = {\n\
             \tname: (PlayerRef) -> string?,\n\
             \tset_name: (PlayerRef, string) -> (),\n\
             }\n\n"
        );
    }

    #[test]
    fn a_type_it_cannot_name_is_any_with_its_rust_name_and_counted() {
        let mut m = mapper(&[]);
        let odd = shape("odd", &[("thing", "Mystery")], &[], None);
        assert_eq!(
            event_type(&odd, &mut m),
            "export type OddEvent = {\n\tthing: any, -- read-only -- Rust: Mystery\n}\n\n"
        );
        assert_eq!(m.any, ["Mystery"]);
    }

    #[test]
    fn the_overloads_name_each_emitted_hook_and_no_other() {
        let mut m = mapper(&[]);
        let hooks = [
            shape("animal_tame", &[], &[("cancelled", "bool")], None),
            shape("server_tick", &[], &[], None),
        ];
        let text = on_overloads(&hooks, &mut m);
        assert!(text.contains("name: \"animal_tame\", handler: (event: AnimalTameEvent) -> ()"));
        assert!(text.contains("name: \"server_tick\""));
        assert!(!text.contains("\"nope\""));
        assert_eq!(text.matches("name: \"").count(), 2);
    }

    #[test]
    fn a_replace_hook_returns_its_answer_or_nothing() {
        let mut m = mapper(&[]);
        let pick = shape("pick", &[], &[], Some("Text"));
        assert_eq!(handler(&pick, &mut m), "(event: PickEvent) -> (Text)?");
    }

    #[test]
    fn a_reserved_word_is_a_quoted_key() {
        assert_eq!(key("end"), "[\"end\"]");
        assert_eq!(key("entity"), "entity");
    }
}
