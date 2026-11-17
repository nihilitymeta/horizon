//! The outputs of each hook, read from the generated dispatch functions, and the fields of the
//! event structs, read from the same mod API text.
//!
//! The game calls `dispatch_<hook>(host, args..., out: &mut T)`: its `&mut` parameters that are
//! not trait objects are the hook's outputs (the host and draw contexts are the other `&mut`
//! ones, and a trait object is always a host). The dispatch copies each output back from the event
//! struct after the last listener ran, so an output's name is the event field of that name.
//! [`parse`] keeps the outputs and the struct fields; `emit_impl` decides which can be converted.

use std::collections::BTreeMap;

use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::{Block, Fields, FnArg, Item, Pat, Token, Type};

/// One output parameter of a dispatch function: its name, its type without spaces, and whether the
/// function's body copies the event's `cancelled` flag into it (`*ran = event.cancelled;`): the
/// cancel flag of a cancel hook, whatever the parameter is called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputSig {
    pub param: String,
    pub ty: String,
    pub copies_cancel: bool,
}

/// What the mod API text says about outputs and event fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchSigs {
    /// Per hook name, its outputs in parameter order. `None` when two dispatch functions name the
    /// hook, or when one of its `&mut` parameters has a pattern that is not a plain name.
    pub hooks: BTreeMap<String, Option<Vec<OutputSig>>>,
    /// Per event struct name, each field and its type without spaces.
    pub structs: BTreeMap<String, BTreeMap<String, String>>,
}

/// Reads every `dispatch_<hook>` function and every struct of `source`.
pub fn parse(source: &str) -> Result<DispatchSigs, String> {
    let file = syn::parse_file(source)
        .map_err(|error| format!("cannot parse the dispatch functions: {error}"))?;
    let mut sigs = DispatchSigs::default();
    for item in &file.items {
        match item {
            Item::Fn(function) => {
                let name = function.sig.ident.to_string();
                let Some(hook) = name.strip_prefix("dispatch_") else {
                    continue;
                };
                let outputs = outputs_of(&function.sig.inputs, &function.block);
                sigs.hooks
                    .entry(hook.to_owned())
                    .and_modify(|entry| *entry = None)
                    .or_insert(outputs);
            }
            Item::Struct(item) => {
                if let Fields::Named(fields) = &item.fields {
                    let mut types = BTreeMap::new();
                    for field in &fields.named {
                        if let Some(ident) = &field.ident {
                            types.insert(ident.to_string(), squash(&field.ty));
                        }
                    }
                    sigs.structs.insert(item.ident.to_string(), types);
                }
            }
            _ => {}
        }
    }
    Ok(sigs)
}

/// The outputs of one dispatch function, or `None` when one of its `&mut` parameters cannot be
/// named (the hook is then not decided from the text).
fn outputs_of(inputs: &Punctuated<FnArg, Token![,]>, body: &Block) -> Option<Vec<OutputSig>> {
    // The body without spaces, so `*ran = event . cancelled ;` reads `*ran=event.cancelled;`.
    let body: String = body
        .to_token_stream()
        .to_string()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    let mut outputs = Vec::new();
    for arg in inputs {
        let FnArg::Typed(param) = arg else {
            continue;
        };
        let Type::Reference(reference) = &*param.ty else {
            continue;
        };
        if reference.mutability.is_none() || matches!(&*reference.elem, Type::TraitObject(_)) {
            continue;
        }
        let Pat::Ident(ident) = &*param.pat else {
            return None;
        };
        let param = ident.ident.to_string();
        let copies_cancel = copies_cancelled(&body, &param);
        outputs.push(OutputSig {
            param,
            ty: squash(&reference.elem),
            copies_cancel,
        });
    }
    Some(outputs)
}

/// Whether `body` (without spaces) assigns `<something>.cancelled;` to the parameter `name`.
fn copies_cancelled(body: &str, name: &str) -> bool {
    let needle = format!("*{name}=");
    body.match_indices(&needle).any(|(at, _)| {
        let rest = &body[at + needle.len()..];
        let ident_end = rest
            .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
            .unwrap_or(rest.len());
        rest[ident_end..].starts_with(".cancelled;")
    })
}

/// A type's tokens with every space removed: `Option < u32 >` is `Option<u32>`.
fn squash(ty: &impl ToTokens) -> String {
    ty.to_token_stream()
        .to_string()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "
        pub struct ComputeFovEvent { pub partial: f32, pub degrees: f32 }
        pub struct BlockReceiveGameEvent<'a> { pub pos: [i32; 3], pub cancelled: bool, pub name: &'a str }
        #[doc(hidden)] pub fn dispatch_compute_fov(host: &dyn ClientHost, partial: f32, degrees: &mut f32) { *degrees = 1.0; }
        #[doc(hidden)] pub fn dispatch_block_receive_game(host: &mut dyn ServerHost, pos: [i32; 3], ran: &mut bool) { *ran = event_.cancelled; }
        pub fn dispatch_collect(host: &mut dyn ServerHost, lines: &mut Vec<String>, ids: &mut Option<Text>) {}
        pub fn dispatch_twice(a: &mut u32) {}
        pub fn dispatch_twice(a: &mut i32) {}
        pub fn dispatch_odd((a, b): &mut (u32, u32)) {}
        fn not_a_dispatch(c: &mut u32) {}
    ";

    fn outputs(sigs: &DispatchSigs, hook: &str) -> Vec<(String, String)> {
        sigs.hooks[hook]
            .as_ref()
            .expect("the hook has one dispatch function")
            .iter()
            .map(|out| (out.param.clone(), out.ty.clone()))
            .collect()
    }

    #[test]
    fn a_mut_parameter_that_is_not_a_trait_object_is_an_output() {
        let sigs = parse(SOURCE).expect("the source parses");
        assert_eq!(
            outputs(&sigs, "compute_fov"),
            [("degrees".to_owned(), "f32".to_owned())]
        );
        assert_eq!(
            outputs(&sigs, "block_receive_game"),
            [("ran".to_owned(), "bool".to_owned())]
        );
    }

    #[test]
    fn a_host_and_plain_inputs_are_not_outputs() {
        let sigs = parse(SOURCE).expect("the source parses");
        assert!(
            outputs(&sigs, "compute_fov")
                .iter()
                .all(|(p, _)| p != "partial")
        );
        assert_eq!(outputs(&sigs, "collect").len(), 2);
        assert_eq!(outputs(&sigs, "collect")[1].1, "Option<Text>");
        assert_eq!(outputs(&sigs, "collect")[0].1, "Vec<String>");
    }

    #[test]
    fn only_a_parameter_the_body_copies_the_cancel_flag_into_is_the_flag() {
        let sigs = parse(SOURCE).expect("the source parses");
        let flag = &sigs.hooks["block_receive_game"].as_ref().expect("decided")[0];
        assert_eq!(flag.param, "ran");
        assert!(flag.copies_cancel);
        let plain = &sigs.hooks["compute_fov"].as_ref().expect("decided")[0];
        assert!(!plain.copies_cancel);
    }

    #[test]
    fn two_functions_for_one_hook_or_an_unnamed_output_make_it_undecided() {
        let sigs = parse(SOURCE).expect("the source parses");
        assert_eq!(sigs.hooks["twice"], None);
        assert_eq!(sigs.hooks["odd"], None);
        assert!(!sigs.hooks.contains_key("not_a_dispatch"));
    }

    #[test]
    fn struct_fields_keep_their_types_without_spaces() {
        let sigs = parse(SOURCE).expect("the source parses");
        let fields = &sigs.structs["BlockReceiveGameEvent"];
        assert_eq!(fields["pos"], "[i32;3]");
        assert_eq!(fields["cancelled"], "bool");
        assert_eq!(sigs.structs["ComputeFovEvent"]["degrees"], "f32");
    }

    #[test]
    fn a_source_that_does_not_parse_is_an_error() {
        assert!(parse("pub fn dispatch_x(").is_err());
    }
}
