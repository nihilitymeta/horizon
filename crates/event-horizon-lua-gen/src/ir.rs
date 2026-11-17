//! The hooks of a surface, in the form the generator emits code from.
//!
//! [`load`] reads a `surface.json` with [`horizon_mod_surface::Surface::read`] and keeps only what
//! the bindings need. The hashes are copied, not recomputed.

use mod_surface::{Argument, Hook, HookKind, Side as SurfaceSide, Surface};

/// What a hook's listeners may do with its event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Listen,
    Cancel,
    Collect,
    Replace,
}

/// Which side of the connection a hook runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
    Both,
}

/// A named argument or output of a hook, with its normalised Rust type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgIr {
    pub name: String,
    pub rust_type: String,
}

/// One hook of the surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookIr {
    /// `hook:<name>`, as the surface names it.
    pub id: String,
    /// The id without its `hook:` prefix; also the Lua event name.
    pub name: String,
    pub kind: Kind,
    pub side: Side,
    /// `None` when the surface leaves the flag out, which is not the same as `false`.
    pub cancellable: Option<bool>,
    /// The scope's kind and key, for a hook fired on the Parts of a named argument.
    pub scope: Option<(String, String)>,
    /// The game function that fires the hook (`engine.target`), e.g.
    /// `horizon_client::game::...` or `mod_render::render::world_drawn`.
    pub engine_target: String,
    pub arguments: Vec<ArgIr>,
    pub output: Option<ArgIr>,
    pub hash: String,
    pub version: u64,
}

impl HookIr {
    /// Whether the hook fires in the separate render crate (`mod_render::`), which the game's
    /// `HookId` does not name.
    pub fn is_render(&self) -> bool {
        self.engine_target.starts_with("mod_render::")
    }
}

/// The hooks of one surface, sorted by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ir {
    pub hooks: Vec<HookIr>,
    pub document_hash: String,
}

impl Ir {
    /// The hook with this name (without the `hook:` prefix).
    pub fn hook(&self, name: &str) -> Option<&HookIr> {
        self.hooks
            .binary_search_by(|hook| hook.name.as_str().cmp(name))
            .ok()
            .map(|index| &self.hooks[index])
    }
}

/// Reads a surface document and converts its hooks.
pub fn load(json: &[u8]) -> Result<Ir, String> {
    let surface = Surface::read(json).map_err(|error| error.to_string())?;
    let mut hooks = surface
        .hooks
        .iter()
        .map(convert)
        .collect::<Result<Vec<_>, _>>()?;
    hooks.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(pair) = hooks.windows(2).find(|pair| pair[0].name == pair[1].name) {
        return Err(format!("hook {} is listed twice", pair[0].name));
    }
    Ok(Ir {
        hooks,
        document_hash: surface.hash,
    })
}

fn convert(hook: &Hook) -> Result<HookIr, String> {
    let name = hook
        .id
        .strip_prefix("hook:")
        .ok_or_else(|| format!("hook id {} lacks the hook: prefix", hook.id))?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(format!("hook name {name} is not [a-z0-9_]+"));
    }
    let contract = &hook.contract;
    Ok(HookIr {
        id: hook.id.clone(),
        name: name.to_owned(),
        kind: match contract.contract {
            HookKind::Listen => Kind::Listen,
            HookKind::Cancel => Kind::Cancel,
            HookKind::Collect => Kind::Collect,
            HookKind::Replace => Kind::Replace,
        },
        side: match contract.side {
            SurfaceSide::Client => Side::Client,
            SurfaceSide::Server => Side::Server,
            SurfaceSide::Both => Side::Both,
        },
        cancellable: contract.cancellable,
        engine_target: hook.engine.target.clone(),
        scope: contract
            .scope
            .as_ref()
            .map(|scope| (scope.kind.clone(), scope.key.clone())),
        arguments: contract.arguments.iter().map(arg).collect(),
        output: contract.output.as_ref().map(arg),
        hash: hook.hash.clone(),
        version: contract.version,
    })
}

fn arg(argument: &Argument) -> ArgIr {
    ArgIr {
        name: argument.name.clone(),
        rust_type: argument.ty.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/mini-surface.json");

    fn fixture() -> Ir {
        load(FIXTURE.as_bytes()).expect("the fixture loads")
    }

    #[test]
    fn the_fixture_loads_and_is_sorted_by_name() {
        let ir = fixture();
        let names: Vec<&str> = ir.hooks.iter().map(|hook| hook.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "add_attribute_tooltips",
                "add_server_reload_listeners",
                "affect_neighbors_after_removal",
                "animal_tame",
                "attack_entity",
            ]
        );
        assert_eq!(
            ir.hook("animal_tame").map(|hook| hook.id.as_str()),
            Some("hook:animal_tame")
        );
    }

    #[test]
    fn kinds_sides_scope_and_outputs_are_carried_over() {
        let ir = fixture();
        let listen = ir.hook("add_server_reload_listeners").expect("present");
        assert_eq!(listen.kind, Kind::Listen);
        assert_eq!(listen.side, Side::Server);
        assert_eq!(listen.scope, None);
        assert_eq!(listen.output, None);
        assert_eq!(
            listen.arguments,
            [ArgIr {
                name: "listeners".to_owned(),
                rust_type: "&'a mut dyn ServerReloadListeners".to_owned(),
            }]
        );

        let client = ir.hook("add_attribute_tooltips").expect("present");
        assert_eq!(client.side, Side::Client);
        assert_eq!(client.kind, Kind::Listen);
        assert_eq!(client.cancellable, Some(false));

        let collect = ir.hook("affect_neighbors_after_removal").expect("present");
        assert_eq!(collect.kind, Kind::Collect);
        assert_eq!(
            collect.scope,
            Some(("block".to_owned(), "state".to_owned()))
        );
        assert_eq!(
            collect.output,
            Some(ArgIr {
                name: "updates".to_owned(),
                rust_type: "&'a mut Vec<NeighbourUpdate>".to_owned(),
            })
        );
        assert_eq!(collect.arguments.len(), 4);

        let cancel = ir.hook("animal_tame").expect("present");
        assert_eq!(cancel.kind, Kind::Cancel);
        assert_eq!(cancel.cancellable, Some(true));
        assert_eq!(
            cancel.arguments.last().map(|a| a.name.as_str()),
            Some("cancelled")
        );
    }

    #[test]
    fn the_engine_target_marks_render_hooks() {
        let ir = fixture();
        let client = ir.hook("animal_tame").expect("present");
        assert!(client.engine_target.starts_with("horizon_client::"));
        assert!(!client.is_render());

        let mut render = client.clone();
        render.engine_target = "mod_render::render::world_drawn".to_owned();
        assert!(render.is_render());
    }

    #[test]
    fn an_absent_cancellable_stays_absent() {
        let ir = fixture();
        let attack = ir.hook("attack_entity").expect("present");
        assert_eq!(attack.kind, Kind::Cancel);
        assert_eq!(attack.cancellable, None);
        assert_eq!(
            ir.hook("add_server_reload_listeners")
                .map(|h| h.cancellable),
            Some(None)
        );
    }

    #[test]
    fn a_hook_id_without_the_prefix_is_refused() {
        let edited = FIXTURE.replace("\"id\": \"hook:animal_tame\"", "\"id\": \"animal_tame\"");
        assert_ne!(edited, FIXTURE, "the fixture names animal_tame");
        let error = load(edited.as_bytes()).expect_err("no prefix");
        assert!(error.contains("animal_tame"), "{error}");
    }

    #[test]
    fn a_duplicate_name_is_refused() {
        let mut document: serde_json::Value =
            serde_json::from_str(FIXTURE).expect("the fixture is JSON");
        let hooks = document["hooks"].as_array_mut().expect("a hooks array");
        let first = hooks[0].clone();
        hooks.push(first);
        let text = serde_json::to_string(&document).expect("serializes");
        let error = load(text.as_bytes()).expect_err("listed twice");
        assert!(error.contains("add_attribute_tooltips"), "{error}");
    }

    #[test]
    fn schema_errors_are_returned_not_panicked() {
        assert!(load(&FIXTURE.as_bytes()[..FIXTURE.len() / 2]).is_err());
        let wrong = FIXTURE.replace("\"schema\": 2", "\"schema\": 3");
        assert_ne!(wrong, FIXTURE, "the fixture has schema 2");
        let error = load(wrong.as_bytes()).expect_err("wrong schema");
        assert!(error.contains("schema 3"), "{error}");
    }

    #[test]
    #[ignore = "reads the real surface.json named by EH_SURFACE"]
    fn the_real_surface_loads() {
        let Some(path) = std::env::var_os("EH_SURFACE") else {
            eprintln!("EH_SURFACE is not set: skipping the real surface");
            return;
        };
        let bytes = std::fs::read(&path).expect("EH_SURFACE names a readable file");
        let ir = load(&bytes).expect("the real surface loads");
        assert_eq!(ir.hooks.len(), 749);
    }
}
