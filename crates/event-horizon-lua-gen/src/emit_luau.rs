//! The Luau type definitions of the Lua mod API (`horizon.d.luau`, issue 74, gate G2), written from the
//! same plan as the Rust glue: the emitted hooks' event types and the `horizon.on` overloads, the
//! Part handler tables with their events, and the bound handle members as one type per family.
//!
//! The file is the hand-written [`STATIC_API`] (the `horizon` global and the value shapes of the convert
//! crate) followed by the generated part. A Rust type with no Lua name is written `any`, with the
//! Rust type in a comment, and counted in [`Emitted::any`].

use std::collections::BTreeSet;

use crate::carried::{self, Shape};
use crate::emit_hooks::camel;
use crate::emit_impl::{EventShape, command_input, is_block_click, scalar_name};
use crate::handles::MemberIr;
use crate::types::{FloatKind, IntKind, TypeIr, map_type};

/// The hand-written part of the file: the `horizon` global and the convert crate's value shapes.
pub const STATIC_API: &str = include_str!("horizon_api.d.luau");

/// The `.luaurc` a mod folder may carry (`crates/event-horizon-lua/lua-types/.luaurc.example`). Untested
/// against luau-lsp: no Luau checker is on the box.
pub const LUAURC_EXAMPLE: &str = "{\n  \"languageMode\": \"strict\"\n}\n";

/// The four Part traits, in the order of [`Input::parts`]: the `horizon` function that declares a
/// Part of the kind, and the handler table type it takes.
pub const PART_KINDS: [(&str, &str); 4] = [
    ("block_part", "BlockPartHandlers"),
    ("item_part", "ItemPartHandlers"),
    ("block_entity_part", "BlockEntityPartHandlers"),
    ("fluid_part", "FluidPartHandlers"),
];

/// What the generated part is written from.
pub struct Input<'a> {
    /// The surface's hash, for the header.
    pub hash: &'a str,
    /// The Hooks trait's emitted hooks, in name order.
    pub hooks: &'a [EventShape],
    /// The emitted methods of each Part trait, in [`PART_KINDS`] order.
    pub parts: [&'a [EventShape]; 4],
    /// The generated handle families, in order.
    pub families: &'a [String],
    /// The bound handle members, in family and name order.
    pub members: &'a [&'a MemberIr],
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
}

/// The mapper's state: the families it can name, the plain types, and the `any` it has written.
struct Mapper<'a> {
    families: &'a [String],
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
            _ => self.unknown(name),
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

/// Writes the file: the hand-written part, then the generated one. Two events, two Parts or two
/// families may not share a name (the camel case of a hook is its event type's name).
pub fn emit(input: &Input<'_>) -> Result<Emitted, String> {
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
    Ok(Emitted {
        text,
        any: mapper.any,
        hooks: input.hooks.len(),
        parts,
        members: input.members.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handles::parse_signature;

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
        Mapper {
            families,
            any: Vec::new(),
        }
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
