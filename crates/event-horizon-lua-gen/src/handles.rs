//! The mod API's handle functions and trait methods, in the form the generator emits code from.
//!
//! A handle is an inherent function or const of an API type (`handle:<owner>::<name>`); a trait
//! method is a method of an API trait (`method:<trait>::<name>`). Each carries a Rust signature
//! string, which [`parse_signature`] reads with `syn`. Types, generics and bounds stay text: a
//! later step maps them.

use mod_surface::{Function, Method, Surface};
use quote::ToTokens;
use syn::{FnArg, ReturnType, Type};

/// How a member takes its receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    /// No receiver: an associated function or const.
    None,
    /// `&self`
    Ref,
    /// `&mut self`
    RefMut,
    /// `self`
    Value,
}

/// One parameter after the receiver, with its normalised Rust type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamIr {
    pub rust_type: String,
}

/// Whether a member is a function or a const item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Function,
    Const,
}

/// One handle or trait method of the surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberIr {
    /// The surface id, with its `handle:` or `method:` prefix once [`load`] has set it.
    pub id: String,
    /// The owner's path (`client::widgets::Button`) or the trait's name.
    pub owner: String,
    pub name: String,
    pub kind: MemberKind,
    /// The function's own generic parameters and where clause, as text; empty when it has none.
    pub generics: String,
    pub receiver: Receiver,
    pub params: Vec<ParamIr>,
    /// The return type; `None` for no `->`.
    pub ret: Option<String>,
    /// A const's type.
    pub const_type: Option<String>,
    pub is_const_fn: bool,
    /// A trait method with no default body; false for handles.
    pub required: bool,
    pub on_wasm: bool,
    pub from_trait: bool,
    pub hash: String,
}

/// Reads one signature string into a member. `owner` and `name` come from the id.
///
/// The returned `id` is `<owner>::<name>`; [`load`] replaces it with the surface id, and sets
/// `required`, `on_wasm`, `from_trait` and `hash` from the record.
pub fn parse_signature(owner: &str, name: &str, signature: &str) -> Result<MemberIr, String> {
    let base = MemberIr {
        id: format!("{owner}::{name}"),
        owner: owner.to_owned(),
        name: name.to_owned(),
        kind: MemberKind::Function,
        generics: String::new(),
        receiver: Receiver::None,
        params: Vec::new(),
        ret: None,
        const_type: None,
        is_const_fn: false,
        required: false,
        on_wasm: false,
        from_trait: false,
        hash: String::new(),
    };
    match syn::parse_str::<syn::Signature>(signature) {
        Ok(sig) => Ok(from_signature(base, &sig)),
        Err(fn_error) => match syn::parse_str::<syn::TraitItemConst>(&format!("{signature};")) {
            Ok(item) => Ok(MemberIr {
                kind: MemberKind::Const,
                const_type: Some(tidy(&item.ty.to_token_stream().to_string())),
                ..base
            }),
            Err(const_error) => Err(format!(
                "{owner}::{name}: `{signature}` is neither a fn nor a const ({fn_error}; {const_error})"
            )),
        },
    }
}

fn from_signature(base: MemberIr, sig: &syn::Signature) -> MemberIr {
    let receiver = match sig.inputs.first() {
        Some(FnArg::Receiver(receiver)) => receiver_of(receiver),
        _ => Receiver::None,
    };
    let params = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pat) => Some(ParamIr {
                rust_type: tidy(&pat.ty.to_token_stream().to_string()),
            }),
            FnArg::Receiver(_) => None,
        })
        .collect();
    let mut generics = sig.generics.to_token_stream().to_string();
    if let Some(clause) = &sig.generics.where_clause {
        generics.push(' ');
        generics.push_str(&clause.to_token_stream().to_string());
    }
    MemberIr {
        generics: tidy(&generics),
        receiver,
        params,
        ret: match &sig.output {
            ReturnType::Default => None,
            ReturnType::Type(_, ty) => Some(tidy(&ty.to_token_stream().to_string())),
        },
        is_const_fn: sig.constness.is_some(),
        ..base
    }
}

fn receiver_of(receiver: &syn::Receiver) -> Receiver {
    let borrowed_mut = |mutable: bool| {
        if mutable {
            Receiver::RefMut
        } else {
            Receiver::Ref
        }
    };
    if receiver.colon_token.is_some() {
        // `self: &Self` or `self: &mut Self`; any other typed receiver takes the value.
        return match &*receiver.ty {
            Type::Reference(reference) => borrowed_mut(reference.mutability.is_some()),
            _ => Receiver::Value,
        };
    }
    match receiver.reference {
        Some(_) => borrowed_mut(receiver.mutability.is_some()),
        None => Receiver::Value,
    }
}

/// Puts `quote`'s spaced token output into the usual Rust spelling.
fn tidy(text: &str) -> String {
    let mut out = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (from, to) in [
        (" :: ", "::"),
        (" < ", "<"),
        ("< ", "<"),
        (" <", "<"),
        (" > ", ">"),
        (" >", ">"),
        (" ,", ","),
        ("( ", "("),
        (" )", ")"),
        ("[ ", "["),
        (" ]", "]"),
        (" ;", ";"),
        (" :", ":"),
        ("& ", "&"),
        ("fn (", "fn("),
        ("Fn (", "Fn("),
    ] {
        out = out.replace(from, to);
    }
    out
}

/// Splits a surface id into its owner and its name.
fn split_id<'a>(id: &'a str, prefix: &str) -> Result<(&'a str, &'a str), String> {
    let rest = id
        .strip_prefix(prefix)
        .ok_or_else(|| format!("{id} lacks the {prefix} prefix"))?;
    let (owner, name) = rest
        .rsplit_once("::")
        .ok_or_else(|| format!("{id} has no ::<name> after its owner"))?;
    let is_name = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if owner.is_empty() || !is_name {
        return Err(format!("{id} is not an owner and a name"));
    }
    Ok((owner, name))
}

fn from_handle(function: &Function) -> Result<MemberIr, String> {
    let (owner, name) = split_id(&function.id, "handle:")?;
    let member = parse_signature(owner, name, &function.contract.signature)
        .map_err(|error| format!("{}: {error}", function.id))?;
    Ok(MemberIr {
        id: function.id.clone(),
        on_wasm: function.contract.wasm,
        hash: function.hash.clone(),
        ..member
    })
}

fn from_method(method: &Method) -> Result<MemberIr, String> {
    let (owner, name) = split_id(&method.id, "method:")?;
    let member = parse_signature(owner, name, &method.contract.signature)
        .map_err(|error| format!("{}: {error}", method.id))?;
    Ok(MemberIr {
        id: method.id.clone(),
        required: method.contract.required,
        on_wasm: method.contract.wasm,
        from_trait: true,
        hash: method.hash.clone(),
        ..member
    })
}

/// Reads a surface document and converts its handles and trait methods, sorted by id.
pub fn load(json: &[u8]) -> Result<Vec<MemberIr>, String> {
    let surface = Surface::read(json).map_err(|error| error.to_string())?;
    let mut members = surface
        .handles
        .iter()
        .map(from_handle)
        .collect::<Result<Vec<_>, _>>()?;
    for owner in &surface.traits {
        for method in &owner.methods {
            members.push(from_method(method)?);
        }
    }
    members.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(signature: &str) -> MemberIr {
        parse_signature("ActiveEffect", "new", signature).expect("the signature parses")
    }

    #[test]
    fn a_static_constructor() {
        let member = parse("fn new(_: EffectKey, _: i32) -> Self");
        assert_eq!(member.receiver, Receiver::None);
        assert_eq!(member.params.len(), 2);
        assert_eq!(member.params[0].rust_type, "EffectKey");
        assert_eq!(member.ret.as_deref(), Some("Self"));
        assert_eq!(member.kind, MemberKind::Function);
    }

    #[test]
    fn receivers_are_told_apart() {
        assert_eq!(parse("fn f(&self)").receiver, Receiver::Ref);
        assert_eq!(parse("fn f(&mut self)").receiver, Receiver::RefMut);
        assert_eq!(parse("fn f(self)").receiver, Receiver::Value);
        assert_eq!(parse("fn f(self: &Self)").receiver, Receiver::Ref);
        assert_eq!(parse("fn f()").receiver, Receiver::None);
        assert!(parse("fn f(&self)").params.is_empty());
    }

    #[test]
    fn a_const_item() {
        let member = parse("const MAX: u32");
        assert_eq!(member.kind, MemberKind::Const);
        assert_eq!(member.const_type.as_deref(), Some("u32"));
        assert_eq!(member.ret, None);
        assert!(!member.is_const_fn);
    }

    #[test]
    fn a_const_fn() {
        let member = parse("const fn id(&self) -> u32");
        assert!(member.is_const_fn);
        assert_eq!(member.kind, MemberKind::Function);
        assert_eq!(member.receiver, Receiver::Ref);
        assert_eq!(member.ret.as_deref(), Some("u32"));
    }

    #[test]
    fn references_arrays_and_options_are_kept_as_text() {
        let member = parse(
            "fn f(&self, _: &'a mut Vec<(text::Text, u32)>, _: [f32; 3], _: Option<&'a str>)",
        );
        let types: Vec<&str> = member
            .params
            .iter()
            .map(|param| param.rust_type.as_str())
            .collect();
        assert_eq!(
            types,
            [
                "&'a mut Vec<(text::Text, u32)>",
                "[f32; 3]",
                "Option<&'a str>"
            ]
        );
    }

    #[test]
    fn generics_are_kept_as_text() {
        let member = parse("fn bake<T: Clone>(&self, _: T) -> Result<T, Error>");
        assert_eq!(member.generics, "<T: Clone>");
        assert_eq!(member.ret.as_deref(), Some("Result<T, Error>"));
    }

    #[test]
    fn a_bad_signature_names_the_id() {
        let error = parse_signature("ActiveEffect", "new", "fn new(_: EffectKey")
            .expect_err("the parenthesis is open");
        assert!(error.contains("ActiveEffect::new"), "{error}");
    }

    /// A surface with one handle and one trait method, the minimum `Surface::read` takes.
    fn document(handle_id: &str, method_id: &str) -> String {
        serde_json::json!({
            "schema": 2,
            "hash": "00",
            "handles": [{
                "id": handle_id,
                "hash": "11",
                "signature": "fn new(_: i32) -> Self",
                "wasm": true,
                "refs": []
            }],
            "traits": [{
                "id": "trait:Apply",
                "hash": "22",
                "role": "implement",
                "side": "both",
                "methods": [{
                    "id": method_id,
                    "hash": "33",
                    "signature": "fn apply_target(&self, _: [f32; 3])",
                    "required": true,
                    "wasm": false,
                    "refs": []
                }],
                "refs": []
            }]
        })
        .to_string()
    }

    #[test]
    fn a_surface_loads_handles_then_methods_sorted_by_id() {
        let members =
            load(document("handle:client::Button::new", "method:Apply::apply_target").as_bytes())
                .expect("the document loads");
        let ids: Vec<&str> = members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["handle:client::Button::new", "method:Apply::apply_target"]
        );

        let handle = &members[0];
        assert_eq!(handle.owner, "client::Button");
        assert_eq!(handle.name, "new");
        assert!(!handle.from_trait && !handle.required && handle.on_wasm);
        assert_eq!(handle.hash, "11");

        let method = &members[1];
        assert_eq!(method.owner, "Apply");
        assert_eq!(method.receiver, Receiver::Ref);
        assert!(method.from_trait && method.required && !method.on_wasm);
        assert_eq!(method.hash, "33");
    }

    #[test]
    fn an_id_without_a_prefix_is_refused() {
        let error = load(document("client::Button::new", "method:Apply::apply_target").as_bytes())
            .expect_err("the handle id has no prefix");
        assert!(error.contains("client::Button::new"), "{error}");
    }

    #[test]
    #[ignore = "reads the real surface.json named by EH_SURFACE"]
    fn the_real_surface_parses_completely() {
        let Some(path) = std::env::var_os("EH_SURFACE") else {
            eprintln!("EH_SURFACE is not set: skipping the real surface");
            return;
        };
        let bytes = std::fs::read(&path).expect("EH_SURFACE names a readable file");
        let members = load(&bytes).expect("the real surface loads");
        let handles = members.iter().filter(|m| !m.from_trait).count();
        let methods = members.iter().filter(|m| m.from_trait).count();
        assert_eq!(handles, 1763);
        assert_eq!(methods, 1513);
    }
}
