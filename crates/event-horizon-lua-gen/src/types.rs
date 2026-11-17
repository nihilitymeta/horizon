//! The mod API's Rust types, read from their text into a structure and a conversion kind.
//!
//! [`map_type`] turns a type written as text (a hook argument, a handle parameter or a return
//! type) into a [`Mapped`]: how the value is passed ([`Access`]) and what it is ([`TypeIr`]).
//! Lifetimes are dropped. [`kind_of`] names the conversion a later step uses for it, and
//! [`coverage`] runs the mapper over a whole surface's distinct type texts.

use std::collections::BTreeSet;

use quote::ToTokens;
use syn::{
    Expr, ExprLit, GenericArgument, Lit, PathArguments, Type, TypeParamBound, TypeTraitObject,
};

/// How a type is passed. Only the outermost reference of a type counts; the lifetime is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Owned,
    Ref,
    RefMut,
}

/// A Rust integer type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntKind {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

/// A Rust float type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatKind {
    F32,
    F64,
}

/// Marker traits that may sit beside a `dyn` trait without changing what it is.
const AUTO_TRAITS: [&str; 3] = ["Send", "Sync", "Unpin"];

/// The structure of a type, with references and smart pointers left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeIr {
    Bool,
    Int(IntKind),
    Float(FloatKind),
    /// `str`, `String` or `Arc<str>`.
    Str,
    Unit,
    Option(Box<TypeIr>),
    /// `Vec<T>`, `[T]`.
    List(Box<TypeIr>),
    Array(Box<TypeIr>, usize),
    Tuple(Vec<TypeIr>),
    /// `dyn Trait`, with the trait's path as written.
    Dyn(String),
    /// Any other path, as written without its lifetimes: `item::item_stack::ItemStack`.
    Named(String),
}

/// A type as it is passed: how (`access`) and what (`ty`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapped {
    pub access: Access,
    pub ty: TypeIr,
}

/// The conversion a type needs, as a later step picks its Lua conversion.
#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    Scalar,
    Vector,
    Position,
    Text,
    ItemStack,
    BlockState,
    Nbt,
    Identifier,
    Handle,
    Collection,
    Event,
    DynHost,
    Other,
}

/// What the mapper made of a set of type texts.
#[derive(Debug)]
pub struct Coverage {
    /// The distinct texts seen.
    pub total: usize,
    /// How many of the texts that mapped have each kind, in the order of [`Kind`]'s variants.
    pub by_kind: Vec<(Kind, usize)>,
    /// Each text that did not map, with the reason.
    pub failed: Vec<(String, String)>,
}

/// Reads one type text. `&'a T` is [`Access::Ref`], `&'a mut T` is [`Access::RefMut`].
///
/// `Self` and other unsupported forms are errors, and the message names the type.
pub fn map_type(text: &str) -> Result<Mapped, String> {
    let ty = syn::parse_str::<Type>(text).map_err(|error| format!("`{text}`: {error}"))?;
    map_top(&ty).map_err(|error| format!("`{text}`: {error}"))
}

/// The conversion kind of a mapped type.
pub fn kind_of(mapped: &Mapped) -> Kind {
    kind_of_ty(&mapped.ty, mapped.access)
}

/// Maps every distinct text once. Texts that fail go to `failed`, not to the kinds.
pub fn coverage<'a>(types: impl Iterator<Item = &'a str>) -> Coverage {
    let distinct: BTreeSet<&str> = types.collect();
    let mut kinds = Vec::new();
    let mut failed = Vec::new();
    for text in &distinct {
        match map_type(text) {
            Ok(mapped) => kinds.push(kind_of(&mapped)),
            Err(error) => failed.push((text.to_string(), error)),
        }
    }
    let by_kind = [
        Kind::Scalar,
        Kind::Vector,
        Kind::Position,
        Kind::Text,
        Kind::ItemStack,
        Kind::BlockState,
        Kind::Nbt,
        Kind::Identifier,
        Kind::Handle,
        Kind::Collection,
        Kind::Event,
        Kind::DynHost,
        Kind::Other,
    ]
    .into_iter()
    .filter_map(|kind| {
        let count = kinds.iter().filter(|found| **found == kind).count();
        (count > 0).then_some((kind, count))
    })
    .collect();
    Coverage {
        total: distinct.len(),
        by_kind,
        failed,
    }
}

fn map_top(ty: &Type) -> Result<Mapped, String> {
    match ty {
        Type::Reference(reference) => {
            let access = if reference.mutability.is_some() {
                Access::RefMut
            } else {
                Access::Ref
            };
            Ok(Mapped {
                access,
                ty: map_ty(&reference.elem)?,
            })
        }
        other => Ok(Mapped {
            access: Access::Owned,
            ty: map_ty(other)?,
        }),
    }
}

/// The structure of a type. References met here (inside `Option<&'a str>`, say) lose their access.
fn map_ty(ty: &Type) -> Result<TypeIr, String> {
    match ty {
        Type::Paren(paren) => map_ty(&paren.elem),
        Type::Group(group) => map_ty(&group.elem),
        Type::Reference(reference) => map_ty(&reference.elem),
        Type::Slice(slice) => Ok(TypeIr::List(Box::new(map_ty(&slice.elem)?))),
        Type::Array(array) => {
            let len = array_len(&array.len)
                .ok_or_else(|| format!("array length `{}` is not a literal", tokens(&array.len)))?;
            Ok(TypeIr::Array(Box::new(map_ty(&array.elem)?), len))
        }
        Type::Tuple(tuple) if tuple.elems.is_empty() => Ok(TypeIr::Unit),
        Type::Tuple(tuple) => tuple
            .elems
            .iter()
            .map(map_ty)
            .collect::<Result<Vec<_>, _>>()
            .map(TypeIr::Tuple),
        Type::TraitObject(object) => Ok(TypeIr::Dyn(dyn_name(object)?)),
        Type::Path(path) if path.qself.is_none() => map_path(&path.path),
        other => Err(format!("unsupported form `{}`", tokens(other))),
    }
}

fn map_path(path: &syn::Path) -> Result<TypeIr, String> {
    if path.segments.len() != 1 {
        let rendered = path_text(path)?;
        return Ok(if rendered == "std::string::String" {
            TypeIr::Str
        } else {
            TypeIr::Named(rendered)
        });
    }
    let segment = &path.segments[0];
    let name = segment.ident.to_string();
    let args = type_args(segment)?;
    let no_args = args.is_empty() && matches!(segment.arguments, PathArguments::None);
    let scalar = match name.as_str() {
        "bool" if no_args => Some(TypeIr::Bool),
        "str" | "String" if no_args => Some(TypeIr::Str),
        "i8" if no_args => Some(TypeIr::Int(IntKind::I8)),
        "i16" if no_args => Some(TypeIr::Int(IntKind::I16)),
        "i32" if no_args => Some(TypeIr::Int(IntKind::I32)),
        "i64" if no_args => Some(TypeIr::Int(IntKind::I64)),
        "i128" if no_args => Some(TypeIr::Int(IntKind::I128)),
        "isize" if no_args => Some(TypeIr::Int(IntKind::Isize)),
        "u8" if no_args => Some(TypeIr::Int(IntKind::U8)),
        "u16" if no_args => Some(TypeIr::Int(IntKind::U16)),
        "u32" if no_args => Some(TypeIr::Int(IntKind::U32)),
        "u64" if no_args => Some(TypeIr::Int(IntKind::U64)),
        "u128" if no_args => Some(TypeIr::Int(IntKind::U128)),
        "usize" if no_args => Some(TypeIr::Int(IntKind::Usize)),
        "f32" if no_args => Some(TypeIr::Float(FloatKind::F32)),
        "f64" if no_args => Some(TypeIr::Float(FloatKind::F64)),
        _ => None,
    };
    if let Some(scalar) = scalar {
        return Ok(scalar);
    }
    match (name.as_str(), args.as_slice()) {
        ("Self", []) if no_args => Err("`Self` has no type here: name the owner".to_string()),
        ("Option", [inner]) => Ok(TypeIr::Option(Box::new(map_ty(inner)?))),
        ("Vec", [inner]) => Ok(TypeIr::List(Box::new(map_ty(inner)?))),
        ("Box" | "Arc", [inner]) => map_ty(inner),
        ("Option" | "Vec" | "Box" | "Arc", _) => Err(format!("`{name}` takes one type argument")),
        _ => Ok(TypeIr::Named(path_text(path)?)),
    }
}

fn array_len(len: &Expr) -> Option<usize> {
    match len {
        Expr::Lit(ExprLit {
            lit: Lit::Int(int), ..
        }) => int.base10_parse().ok(),
        _ => None,
    }
}

/// The type arguments of a path segment; lifetimes are left out.
fn type_args(segment: &syn::PathSegment) -> Result<Vec<&Type>, String> {
    match &segment.arguments {
        PathArguments::None => Ok(Vec::new()),
        PathArguments::AngleBracketed(args) => {
            let mut types = Vec::new();
            for arg in &args.args {
                match arg {
                    GenericArgument::Type(ty) => types.push(ty),
                    GenericArgument::Lifetime(_) => {}
                    other => return Err(format!("unsupported argument `{}`", tokens(other))),
                }
            }
            Ok(types)
        }
        PathArguments::Parenthesized(_) => Err("parenthesised arguments are not supported".into()),
    }
}

/// A path as written, with its lifetimes and their brackets left out.
fn path_text(path: &syn::Path) -> Result<String, String> {
    let mut segments = Vec::new();
    for segment in &path.segments {
        let mut text = segment.ident.to_string();
        let args = type_args(segment)?
            .into_iter()
            .map(render_type)
            .collect::<Result<Vec<_>, _>>()?;
        if !args.is_empty() {
            text.push_str(&format!("<{}>", args.join(", ")));
        }
        segments.push(text);
    }
    Ok(segments.join("::"))
}

/// A type as written inside a path's arguments, with its lifetimes left out.
fn render_type(ty: &Type) -> Result<String, String> {
    match ty {
        Type::Path(path) if path.qself.is_none() => path_text(&path.path),
        Type::Reference(reference) => Ok(format!(
            "&{}{}",
            if reference.mutability.is_some() {
                "mut "
            } else {
                ""
            },
            render_type(&reference.elem)?
        )),
        Type::Slice(slice) => Ok(format!("[{}]", render_type(&slice.elem)?)),
        Type::Array(array) => Ok(format!(
            "[{}; {}]",
            render_type(&array.elem)?,
            tokens(&array.len)
        )),
        Type::Tuple(tuple) => {
            let elems = tuple
                .elems
                .iter()
                .map(render_type)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match elems.len() {
                1 => format!("({},)", elems[0]),
                _ => format!("({})", elems.join(", ")),
            })
        }
        Type::TraitObject(object) => Ok(format!("dyn {}", dyn_name(object)?)),
        Type::Paren(paren) => render_type(&paren.elem),
        Type::Group(group) => render_type(&group.elem),
        other => Err(format!("unsupported form `{}`", tokens(other))),
    }
}

/// The one trait of a `dyn`; the auto traits (`Send`, `Sync`, `Unpin`) beside it are left out.
fn dyn_name(object: &TypeTraitObject) -> Result<String, String> {
    let traits: Vec<&syn::Path> = object
        .bounds
        .iter()
        .filter_map(|bound| match bound {
            TypeParamBound::Trait(bound) => Some(&bound.path),
            _ => None,
        })
        .filter(|path| !AUTO_TRAITS.contains(&path_text(path).unwrap_or_default().as_str()))
        .collect();
    match traits.as_slice() {
        [single] => path_text(single),
        _ => Err("`dyn` needs exactly one trait".to_string()),
    }
}

fn kind_of_ty(ty: &TypeIr, access: Access) -> Kind {
    match ty {
        TypeIr::Bool | TypeIr::Int(_) | TypeIr::Float(_) | TypeIr::Str => Kind::Scalar,
        TypeIr::Unit | TypeIr::Tuple(_) => Kind::Other,
        TypeIr::Option(inner) => kind_of_ty(inner, access),
        TypeIr::List(_) => Kind::Collection,
        TypeIr::Array(elem, len) => match elem.as_ref() {
            TypeIr::Int(IntKind::I32) if *len == 3 => Kind::Position,
            TypeIr::Int(_) | TypeIr::Float(_) if (2..=4).contains(len) => Kind::Vector,
            _ => Kind::Collection,
        },
        TypeIr::Dyn(_) => Kind::DynHost,
        TypeIr::Named(name) => named_kind(name, access),
    }
}

fn named_kind(name: &str, access: Access) -> Kind {
    let last = name.rsplit("::").next().unwrap_or(name);
    if last == "BlockPos" {
        Kind::Position
    } else if last == "BlockState" {
        Kind::BlockState
    } else if last == "Compound" {
        Kind::Nbt
    } else if name == "text::Text" || name == "Text" {
        Kind::Text
    } else if last == "ItemStack" {
        Kind::ItemStack
    } else if last == "Identifier" {
        Kind::Identifier
    } else if last.ends_with("Ref") {
        Kind::Handle
    } else if access != Access::Owned {
        Kind::Event
    } else {
        Kind::Other
    }
}

fn tokens<T: ToTokens>(node: &T) -> String {
    node.to_token_stream().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(text: &str) -> Mapped {
        map_type(text).unwrap_or_else(|error| panic!("{error}"))
    }

    fn owned(ty: TypeIr) -> Mapped {
        Mapped {
            access: Access::Owned,
            ty,
        }
    }

    #[test]
    fn scalars_and_containers_keep_their_structure() {
        assert_eq!(
            map("Option<[i32; 3]>"),
            owned(TypeIr::Option(Box::new(TypeIr::Array(
                Box::new(TypeIr::Int(IntKind::I32)),
                3
            ))))
        );
        assert_eq!(
            map("&'a[(Arc<str>, u32)]"),
            Mapped {
                access: Access::Ref,
                ty: TypeIr::List(Box::new(TypeIr::Tuple(vec![
                    TypeIr::Str,
                    TypeIr::Int(IntKind::U32)
                ]))),
            }
        );
        assert_eq!(
            map("[[f32; 4]; 8]"),
            owned(TypeIr::Array(
                Box::new(TypeIr::Array(Box::new(TypeIr::Float(FloatKind::F32)), 4)),
                8
            ))
        );
    }

    #[test]
    fn access_is_read_from_the_outer_reference() {
        assert_eq!(
            map("&'a str"),
            Mapped {
                access: Access::Ref,
                ty: TypeIr::Str
            }
        );
        assert_eq!(
            map("&'a mut String"),
            Mapped {
                access: Access::RefMut,
                ty: TypeIr::Str
            }
        );
        assert_eq!(map("i32"), owned(TypeIr::Int(IntKind::I32)));
    }

    #[test]
    fn lifetimes_are_dropped_from_names() {
        assert_eq!(
            map("&'a ScreenView<'a>"),
            Mapped {
                access: Access::Ref,
                ty: TypeIr::Named("ScreenView".into()),
            }
        );
    }

    #[test]
    fn trait_objects_are_dyn() {
        assert_eq!(
            map("&'a mut dyn DrawContext"),
            Mapped {
                access: Access::RefMut,
                ty: TypeIr::Dyn("DrawContext".into()),
            }
        );
    }

    #[test]
    fn smart_pointers_are_transparent_and_generic_names_keep_their_arguments() {
        assert_eq!(
            map("Arc<dyn EntityPart>").ty,
            TypeIr::Dyn("EntityPart".into())
        );
        assert_eq!(map("Box<dyn Hooks>").ty, TypeIr::Dyn("Hooks".into()));
        assert_eq!(
            map("Result<&mut dyn ClientHost, PayloadContextError>").ty,
            TypeIr::Named("Result<&mut dyn ClientHost, PayloadContextError>".into())
        );
    }

    #[test]
    fn unsupported_forms_are_errors_that_name_the_type() {
        for text in ["fn(i32)", "impl Trait", "*const u8", "Self", "Option<Self>"] {
            let error = map_type(text).expect_err(text);
            assert!(error.contains(text), "{error} does not name {text}");
        }
    }

    #[test]
    fn kinds_follow_the_names_and_shapes() {
        let kind = |text: &str| kind_of(&map(text));
        assert_eq!(kind("BlockPos"), Kind::Position);
        assert_eq!(kind("[i32; 3]"), Kind::Position);
        assert_eq!(kind("[f64; 3]"), Kind::Vector);
        assert_eq!(kind("EntityRef"), Kind::Handle);
        assert_eq!(kind("&'a item::item_stack::ItemStack"), Kind::ItemStack);
        assert_eq!(kind("&'a text::Text"), Kind::Text);
        assert_eq!(kind("&'a Compound"), Kind::Nbt);
        assert_eq!(kind("&'a Identifier"), Kind::Identifier);
        assert_eq!(kind("&'a BlockState"), Kind::BlockState);
        assert_eq!(kind("Option<u128>"), Kind::Scalar);
        assert_eq!(kind("&'a str"), Kind::Scalar);
        assert_eq!(kind("&'a mut Vec<String>"), Kind::Collection);
        assert_eq!(kind("&'a mut dyn DrawContext"), Kind::DynHost);
        assert_eq!(kind("&'a ModBlockEntity"), Kind::Event);
        assert_eq!(kind("ModBlockEntityView<'a>"), Kind::Other);
    }

    #[test]
    fn coverage_counts_each_distinct_text_once() {
        let cov = coverage(["i32", "i32", "BlockPos", "fn(i32)"].into_iter());
        assert_eq!(cov.total, 3);
        assert_eq!(cov.by_kind, vec![(Kind::Scalar, 1), (Kind::Position, 1)]);
        assert_eq!(cov.failed.len(), 1);
        assert_eq!(cov.failed[0].0, "fn(i32)");
    }

    /// Replaces each whole identifier `Self` in `text` with `owner`.
    fn name_self(text: &str, owner: &str) -> String {
        let mut out = String::new();
        let mut word = String::new();
        for ch in text.chars().chain(std::iter::once(' ')) {
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
                continue;
            }
            out.push_str(if word == "Self" { owner } else { &word });
            word.clear();
            out.push(ch);
        }
        out.pop();
        out
    }

    /// Reads `$EH_SURFACE`: maps every hook argument and output, and every handle and trait
    /// parameter and return type. Run by hand, and print the table with `--nocapture`.
    #[test]
    #[ignore = "reads $EH_SURFACE; run by hand"]
    fn the_real_surface_types_all_map() {
        let Ok(path) = std::env::var("EH_SURFACE") else {
            eprintln!("EH_SURFACE is not set: skipping");
            return;
        };
        let bytes = std::fs::read(&path).expect("EH_SURFACE is readable");
        let hooks = crate::ir::load(&bytes).expect("the hooks load");
        let members = crate::handles::load(&bytes).expect("the handles load");

        let mut texts: Vec<String> = Vec::new();
        for hook in &hooks.hooks {
            texts.extend(hook.arguments.iter().map(|arg| arg.rust_type.clone()));
            texts.extend(hook.output.iter().map(|out| out.rust_type.clone()));
        }
        for member in &members {
            // In an inherent item `Self` is the owner; a trait method's `Self` is not known here.
            let own = |text: &String| {
                if member.from_trait {
                    text.clone()
                } else {
                    name_self(text, &member.owner)
                }
            };
            texts.extend(member.params.iter().map(|param| own(&param.rust_type)));
            texts.extend(member.ret.iter().map(own));
            texts.extend(member.const_type.iter().map(own));
        }

        let cov = coverage(texts.iter().map(String::as_str));
        println!("distinct types: {}", cov.total);
        for (kind, count) in &cov.by_kind {
            println!("  {kind:?}: {count}");
        }
        for (text, error) in &cov.failed {
            println!("FAIL {text}: {error}");
        }
        // `impl Trait`, a bare `Self` and fn pointers have no Lua form: the generator lists them
        // as "not on Lua". Any other failure is a mapping gap, and the count is pinned so that a
        // new refusal shows up here.
        let unexpected: Vec<_> = cov
            .failed
            .iter()
            .filter(|(text, _)| {
                !(text.contains("impl ")
                    || text == "Self"
                    || text.starts_with("fn(")
                    || text.starts_with("[PathType;"))
            })
            .collect();
        assert!(unexpected.is_empty(), "unmapped types: {unexpected:?}");
        assert!(cov.failed.len() <= 50, "{} refusals", cov.failed.len());
    }
}
