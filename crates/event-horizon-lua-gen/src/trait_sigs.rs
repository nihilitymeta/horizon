//! The hook signatures of the generated `Hooks` trait, read from its Rust text.
//!
//! The surface does not hold the parameter types of the hooks, but the generated trait in
//! `mod_api_generated.rs` has one method per hook. [`parse`] reads that trait with `syn` and
//! keeps the methods shaped `(&self, <host>: <type>, <event>: &mut <type>) [-> <type>]` or, for
//! the hooks with no host, `(&self, <event>: &mut <type>) [-> <type>]`. The generated dispatch
//! code must match these exactly; compiling it against the real trait is the drift check.

use quote::ToTokens;
use syn::{FnArg, Item, Pat, PatType, ReturnType, TraitItem, TraitItemFn, Type};

/// One hook method of the `Hooks` trait, with its parameter and return types as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSig {
    pub name: String,
    /// The host parameter's name: `host`, `context` or `level`; `None` for an event-only hook.
    pub host_name: Option<String>,
    /// The host parameter's type, e.g. `&mut dyn ServerHost`; `None` for an event-only hook.
    pub host_ty: Option<String>,
    /// The event parameter's name: `event` or `event_`.
    pub event_name: String,
    /// The event type without its `&mut`, e.g. `ServerTickEvent<'_>`.
    pub event_ty: String,
    /// The return type, e.g. `Flow`; `None` when the method returns nothing.
    pub ret: Option<String>,
}

impl HookSig {
    /// The method's signature as the generated file spells it, without the default body.
    pub fn signature(&self) -> String {
        let host = match (&self.host_name, &self.host_ty) {
            (Some(name), Some(ty)) => format!("{name}: {ty}, "),
            _ => String::new(),
        };
        let mut out = format!(
            "fn {}(&self, {}{}: &mut {})",
            self.name, host, self.event_name, self.event_ty
        );
        if let Some(ret) = &self.ret {
            out.push_str(" -> ");
            out.push_str(ret);
        }
        out
    }
}

/// Reads every hook method of `trait Hooks` in `source`, sorted by name.
///
/// A method is a hook when it takes `&self` and then either one parameter of type `&mut T` (an
/// event-only hook) or two parameters whose second is `&mut T` (a host and an event). Other
/// methods (`register`, `subscribed`, `listeners`) are skipped. A method whose second parameter
/// is `&mut T` but which has a third parameter after it is an error naming the method.
///
/// Reads the `Hooks` trait (see [`parse_trait`] for a Part trait such as `BlockPart`).
pub fn parse(source: &str) -> Result<Vec<HookSig>, String> {
    parse_trait(source, "Hooks")
}

/// [`parse`] for the trait named `trait_name` (`Hooks`, or a per-instance Part trait such as
/// `BlockPart`, whose hook-shaped methods take the same host and event arguments).
pub fn parse_trait(source: &str, trait_name: &str) -> Result<Vec<HookSig>, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("cannot parse the trait: {error}"))?;
    let hooks = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Trait(item) if item.ident == trait_name => Some(item),
            _ => None,
        })
        .ok_or_else(|| format!("no `trait {trait_name}` in the source"))?;
    let mut sigs = Vec::new();
    for item in &hooks.items {
        let TraitItem::Fn(method) = item else {
            continue;
        };
        if let Some(sig) = hook_sig(method)? {
            sigs.push(sig);
        }
    }
    sigs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sigs)
}

fn hook_sig(method: &TraitItemFn) -> Result<Option<HookSig>, String> {
    let name = method.sig.ident.to_string();
    let mut inputs = method.sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(receiver))
            if receiver.reference.is_some() && receiver.mutability.is_none() => {}
        _ => return Ok(None),
    }
    let params: Vec<&PatType> = inputs
        .filter_map(|arg| match arg {
            FnArg::Typed(param) => Some(param),
            FnArg::Receiver(_) => None,
        })
        .collect();
    let (host, event) = match params.as_slice() {
        [event] if mut_ref(&event.ty).is_some() => (None, *event),
        [host, event] if mut_ref(&event.ty).is_some() => (Some(*host), *event),
        [_, event, ..] if mut_ref(&event.ty).is_some() => {
            return Err(format!(
                "hook-shaped method `{name}` takes {} parameters after `&self`; expected 1 or 2",
                params.len()
            ));
        }
        _ => return Ok(None),
    };
    let Some(event_ref) = mut_ref(&event.ty) else {
        return Ok(None);
    };
    Ok(Some(HookSig {
        host_name: host.map(|host| ident_of(host, &name)).transpose()?,
        host_ty: host.map(|host| spell(&host.ty.to_token_stream().to_string())),
        event_name: ident_of(event, &name)?,
        event_ty: spell(&event_ref.elem.to_token_stream().to_string()),
        ret: match &method.sig.output {
            ReturnType::Default => None,
            ReturnType::Type(_, ty) => Some(spell(&ty.to_token_stream().to_string())),
        },
        name,
    }))
}

/// The reference of a `&mut T` type, if it is one.
fn mut_ref(ty: &Type) -> Option<&syn::TypeReference> {
    match ty {
        Type::Reference(reference) if reference.mutability.is_some() => Some(reference),
        _ => None,
    }
}

fn ident_of(param: &PatType, method: &str) -> Result<String, String> {
    match &*param.pat {
        Pat::Ident(ident) => Ok(ident.ident.to_string()),
        _ => Err(format!(
            "hook `{method}` has a parameter that is not a plain name"
        )),
    }
}

/// Turns the token text of a type into the spelling the generated file uses: no space after
/// `&`, none inside `<...>`, `(...)` or `[...]`, none around `::`, and none before a comma.
fn spell(tokens: &str) -> String {
    let mut out = tokens.to_string();
    for (from, to) in [
        ("& mut ", "&mut "),
        ("& ", "&"),
        ("< ", "<"),
        (" <", "<"),
        (" >", ">"),
        (" ,", ","),
        ("( ", "("),
        (" )", ")"),
        ("[ ", "["),
        (" ]", "]"),
        (" :: ", "::"),
        (":: ", "::"),
        (" ::", "::"),
    ] {
        out = out.replace(from, to);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/mini-hooks-trait.rs");

    fn find<'a>(sigs: &'a [HookSig], name: &str) -> &'a HookSig {
        sigs.iter()
            .find(|sig| sig.name == name)
            .unwrap_or_else(|| panic!("no hook named {name}"))
    }

    #[test]
    fn fixture_parses_to_the_four_hooks() {
        let sigs = parse(FIXTURE).expect("the fixture parses");
        let names: Vec<&str> = sigs.iter().map(|sig| sig.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "block_receive_game",
                "block_step_on",
                "level_tick_pre",
                "render_hand",
                "server_tick"
            ]
        );

        let tick = find(&sigs, "server_tick");
        assert_eq!(tick.host_name.as_deref(), Some("host"));
        assert_eq!(tick.host_ty.as_deref(), Some("&mut dyn ServerHost"));
        assert_eq!(tick.event_name, "event");
        assert_eq!(tick.event_ty, "ServerTickEvent<'_>");
        assert_eq!(tick.ret, None);

        let level = find(&sigs, "level_tick_pre");
        assert_eq!(level.host_ty.as_deref(), Some("HostSide"));
        assert_eq!(level.event_ty, "LevelTickPreEvent");
        assert_eq!(level.ret, None);

        let hand = find(&sigs, "render_hand");
        assert_eq!(hand.host_ty.as_deref(), Some("&dyn ClientHost"));
        assert_eq!(hand.event_ty, "RenderHandEvent<'_, '_>");
        assert_eq!(hand.ret.as_deref(), Some("Flow"));

        let receive = find(&sigs, "block_receive_game");
        assert_eq!(receive.event_name, "event_");
        assert_eq!(receive.event_ty, "BlockReceiveGameEvent<'_>");
        assert_eq!(receive.ret.as_deref(), Some("Flow"));

        let step = find(&sigs, "block_step_on");
        assert_eq!(step.host_name, None);
        assert_eq!(step.host_ty, None);
        assert_eq!(step.event_name, "event");
        assert_eq!(step.event_ty, "BlockStepOnEvent<'_>");
        assert_eq!(step.ret, None);
    }

    #[test]
    fn signatures_match_the_generated_spelling() {
        let sigs = parse(FIXTURE).expect("the fixture parses");
        assert_eq!(
            find(&sigs, "server_tick").signature(),
            "fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>)"
        );
        assert_eq!(
            find(&sigs, "level_tick_pre").signature(),
            "fn level_tick_pre(&self, host: HostSide, event: &mut LevelTickPreEvent)"
        );
        assert_eq!(
            find(&sigs, "render_hand").signature(),
            "fn render_hand(&self, host: &dyn ClientHost, event: &mut RenderHandEvent<'_, '_>) -> Flow"
        );
        assert_eq!(
            find(&sigs, "block_receive_game").signature(),
            "fn block_receive_game(&self, host: &mut dyn ServerHost, event_: &mut BlockReceiveGameEvent<'_>) -> Flow"
        );
        assert_eq!(
            find(&sigs, "block_step_on").signature(),
            "fn block_step_on(&self, event: &mut BlockStepOnEvent<'_>)"
        );
    }

    #[test]
    fn non_hook_methods_are_skipped() {
        // `register` takes `&self` and two more parameters, but its second is not `&mut`.
        let source = "pub trait Hooks {
            fn subscribed(&self) -> &'static [u8];
            fn listeners(&self) -> &'static [u8] { &[] }
            fn register(&self, r: &mut dyn Registrar, cx: &RegistrationContext) {}
            fn shared_event(&self, host: &mut dyn ServerHost, event: &ServerTickEvent);
            fn read_only_event(&self, event: &ServerTickEvent);
            fn plain(x: u32) -> u32;
        }";
        assert_eq!(parse(source), Ok(Vec::new()));
    }

    #[test]
    fn a_trait_without_hooks_is_an_error() {
        let error = parse("pub trait Other { fn f(&self); }").expect_err("no Hooks trait");
        assert!(error.contains("trait Hooks"), "{error}");
    }

    #[test]
    fn a_syntax_error_is_an_error() {
        assert!(parse("pub trait Hooks { fn server_tick(&self, host: ").is_err());
    }

    #[test]
    fn a_hook_with_a_third_parameter_is_an_error_naming_it() {
        let source = "pub trait Hooks {
            fn server_tick(&self, host: &mut dyn ServerHost, event: &mut ServerTickEvent<'_>, extra: u32);
        }";
        let error = parse(source).expect_err("three parameters after &self");
        assert!(error.contains("server_tick"), "{error}");
    }

    #[test]
    #[ignore = "reads the generated trait named by EH_HOOKS_TRAIT; run by hand"]
    fn the_real_trait_parses_completely() {
        let Ok(path) = std::env::var("EH_HOOKS_TRAIT") else {
            eprintln!("EH_HOOKS_TRAIT is not set: skipping the check of the real trait");
            return;
        };
        let source = std::fs::read_to_string(&path).expect("EH_HOOKS_TRAIT is readable");
        let sigs = parse(&source).expect("the real trait parses");
        println!("hook signatures: {}", sigs.len());
        assert_eq!(sigs.len(), 598);

        let names: std::collections::BTreeSet<&str> =
            sigs.iter().map(|sig| sig.name.as_str()).collect();
        assert_eq!(names.len(), sigs.len(), "duplicate hook names");

        let mut host_counts = std::collections::BTreeMap::<&str, usize>::new();
        for sig in &sigs {
            *host_counts
                .entry(sig.host_ty.as_deref().unwrap_or("(none)"))
                .or_default() += 1;
        }
        println!("host_ty counts:");
        for (host_ty, count) in &host_counts {
            println!("  {count:4}  {host_ty}");
        }
        let flow = sigs
            .iter()
            .filter(|sig| sig.ret.as_deref() == Some("Flow"))
            .count();
        let event_underscore = sigs.iter().filter(|sig| sig.event_name == "event_").count();
        println!("returning Flow: {flow}");
        println!("event named event_: {event_underscore}");

        // The file spells some return types unformatted (`Option < bool >`), so compare
        // without whitespace on both sides.
        let squashed: String = source.split_whitespace().collect();
        let unspelled = sigs
            .iter()
            .filter(|sig| {
                let text: String = sig.signature().split_whitespace().collect();
                !squashed.contains(&text)
            })
            .map(|sig| sig.name.as_str())
            .collect::<Vec<_>>();
        println!(
            "signatures not found verbatim in the file: {}",
            unspelled.len()
        );
        assert!(
            unspelled.is_empty(),
            "not spelled as in the file: {unspelled:?}"
        );

        let Ok(surface_path) = std::env::var("EH_SURFACE") else {
            eprintln!("EH_SURFACE is not set: skipping the comparison with the surface");
            return;
        };
        let json = std::fs::read(&surface_path).expect("EH_SURFACE is readable");
        let ir = crate::ir::load(&json).expect("the surface loads");
        let surface: std::collections::BTreeSet<&str> =
            ir.hooks.iter().map(|hook| hook.name.as_str()).collect();
        let render: std::collections::BTreeSet<&str> = ir
            .hooks
            .iter()
            .filter(|hook| hook.is_render())
            .map(|hook| hook.name.as_str())
            .collect();
        let non_render: std::collections::BTreeSet<&str> =
            surface.difference(&render).copied().collect();
        println!("surface hooks: {}", surface.len());
        println!("surface render hooks: {}", render.len());
        println!("surface hooks minus render: {}", non_render.len());
        println!(
            "trait names not in surface: {:?}",
            names.difference(&surface).collect::<Vec<_>>()
        );
        println!(
            "surface names not in trait: {:?}",
            surface.difference(&names).collect::<Vec<_>>()
        );
        println!("render hooks: {render:?}");
        let outside_trait = non_render.difference(&names).count();
        println!(
            "surface hooks minus render not in the Hooks trait (the other traits hold them): {outside_trait}"
        );
        assert!(
            names.is_subset(&surface),
            "the Hooks trait names a hook the surface does not have"
        );
    }
}
