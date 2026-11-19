//! Handle values in Lua (issue 74, H1b): each handle family's value is wrapped in a [`Token`]
//! userdata, and the generated methods ([`crate::generated::handles`]) convert their arguments and
//! results through the helpers here.
//!
//! A token holds only the value the game gave it. Its methods reach the world through the turn's
//! host, so a token kept from an earlier fire is answered by the game (a stale one gives `nil`).
//! A token exposes no fields and cannot be built from Lua: the only tokens are the ones a method
//! returns.

use std::fmt::{Debug, Display};

use event_horizon_lua_convert::item::BlockStateSpec;
use event_horizon_lua_convert::scalar::FromLuaValue;
use mlua::{AnyUserData, Error, Lua, MetaMethod, MultiValue, UserData, UserDataMethods, Value};
use mod_api::{BlockState, Host, ServerHost};

use crate::host::LOG_TARGET;
use crate::hostcall::{NOT_HERE, TurnHost};
use crate::stacks;

/// The most items a list result holds: the cap the converters give a list.
const MAX_LIST: usize = 1024;

/// A handle family: the game's handle type, its Lua methods and its `tostring`. The generator
/// writes the impl for each family it binds.
pub trait Family: Copy + PartialEq + Send + Sync + 'static {
    /// The family's name, as the errors and `tostring` show it.
    const NAME: &'static str;
    /// Registers the family's methods on its userdata.
    fn add_methods<M: UserDataMethods<Token<Self>>>(methods: &mut M);
    /// The `tostring` text: the name, with the id when the family has an id accessor.
    fn describe(self) -> String;
}

/// A handle value as Lua userdata.
#[derive(Clone, Copy)]
pub struct Token<F>(pub F);

impl<F: Family> UserData for Token<F> {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        F::add_methods(methods);
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.describe()));
        // Two tokens are equal when they hold the same handle; another userdata is not.
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: AnyUserData| {
            Ok(other
                .borrow::<Token<F>>()
                .is_ok_and(|other| other.0 == this.0))
        });
    }
}

/// The handle a value holds when it is a userdata of family `F`.
fn family_of<F: Family>(value: &Value) -> Option<F> {
    match value {
        Value::UserData(ud) => ud.borrow::<Token<F>>().ok().map(|token| token.0),
        _ => None,
    }
}

/// The receiver of a method: the handle its userdata holds. A token of another family is refused.
pub fn receiver<F: Family>(this: &AnyUserData, method: &str) -> mlua::Result<F> {
    this.borrow::<Token<F>>()
        .map(|token| token.0)
        .map_err(|_| Error::runtime(format!("{method}: the receiver must be a {}", F::NAME)))
}

/// The host a read goes through. A turn with no host at all refuses the call.
pub fn read_of<'a>(host: &'a TurnHost<'_>) -> mlua::Result<&'a dyn Host> {
    host.read().ok_or_else(|| Error::runtime(NOT_HERE))
}

/// The server host a write goes through. A turn that is not a server hook refuses the call.
pub fn server_of<'a>(host: &'a mut TurnHost<'_>) -> mlua::Result<&'a mut dyn ServerHost> {
    host.server().ok_or_else(|| Error::runtime(NOT_HERE))
}

/// The most characters of a refusal's text a Lua error carries.
const REFUSAL_CHARS: usize = 120;

/// A refusal's `Debug` text cut to [`REFUSAL_CHARS`] characters, with `...` after a cut. The whole
/// value never reaches the mod.
pub fn refusal_text(debug: &str) -> String {
    let mut text: String = debug.chars().take(REFUSAL_CHARS).collect();
    if debug.chars().count() > REFUSAL_CHARS {
        text.push_str("...");
    }
    text
}

/// The `Ok` value of a method's `Result`, or a Lua error with the refusal's text.
pub fn ok_or_raise<T, E: Debug>(result: Result<T, E>) -> mlua::Result<T> {
    result.map_err(|err| Error::runtime(refusal_text(&format!("{err:?}"))))
}

/// The arguments of one method call, with the method's name for the errors.
pub struct Args {
    method: &'static str,
    values: Vec<Value>,
}

impl Args {
    /// The call's arguments, of which there may be no more than `count`.
    pub fn new(method: &'static str, values: MultiValue, count: usize) -> mlua::Result<Args> {
        let values = values.into_vec();
        if values.len() > count {
            return Err(Error::runtime(format!(
                "{method} takes {count} argument(s), not {}",
                values.len()
            )));
        }
        Ok(Args { method, values })
    }

    /// The argument at `index` as the Lua value it is, nil when missing.
    pub(crate) fn value(&self, index: usize) -> Value {
        self.values.get(index).cloned().unwrap_or(Value::Nil)
    }

    fn refuse(&self, index: usize, why: impl Display) -> Error {
        Error::runtime(format!("{}: argument {}: {why}", self.method, index + 1))
    }

    /// The argument at `index` converted as `T`. A missing argument is nil.
    pub fn get<T: FromLuaValue>(&self, index: usize) -> mlua::Result<T> {
        T::from_lua_value(&self.value(index)).map_err(|err| self.refuse(index, err))
    }

    /// The argument at `index` as a handle of family `F`.
    pub fn handle<F: Family>(&self, index: usize) -> mlua::Result<F> {
        family_of(&self.value(index))
            .ok_or_else(|| self.refuse(index, format!("must be a {}", F::NAME)))
    }

    /// The argument at `index` as a handle of family `F`, or nil for none.
    pub fn opt_handle<F: Family>(&self, index: usize) -> mlua::Result<Option<F>> {
        let value = self.value(index);
        if value.is_nil() {
            return Ok(None);
        }
        family_of(&value)
            .map(Some)
            .ok_or_else(|| self.refuse(index, format!("must be a {} or nil", F::NAME)))
    }

    /// The argument at `index` as a block state the host can name. `host` reads the state's text.
    pub fn state(&self, index: usize, host: &dyn Host) -> mlua::Result<BlockState> {
        let spec: BlockStateSpec = self.get(index)?;
        stacks::state_from_spec(host, &spec).map_err(|why| self.refuse(index, why))
    }
}

/// A handle as its Lua userdata.
pub fn family_out<F: Family>(lua: &Lua, handle: F) -> mlua::Result<Value> {
    Ok(Value::UserData(lua.create_userdata(Token(handle))?))
}

/// An optional handle: nil for none.
pub fn family_opt_out<F: Family>(lua: &Lua, handle: Option<F>) -> mlua::Result<Value> {
    match handle {
        Some(handle) => family_out(lua, handle),
        None => Ok(Value::Nil),
    }
}

/// A list of handles as a Lua array of userdata, refused past [`MAX_LIST`] items.
pub fn family_list_out<F: Family>(lua: &Lua, handles: Vec<F>) -> mlua::Result<Value> {
    if handles.len() > MAX_LIST {
        return Err(Error::runtime(format!(
            "a list of {} items is over the cap of {MAX_LIST}",
            handles.len()
        )));
    }
    let table = lua.create_table()?;
    for (index, handle) in handles.into_iter().enumerate() {
        table.raw_set(index + 1, family_out(lua, handle)?)?;
    }
    Ok(Value::Table(table))
}

/// A hook's list of handles as a Lua array of userdata. Unlike [`family_list_out`] it does not
/// refuse a long list: the first [`MAX_LIST`] items are given and one warning names the hook.
pub fn family_capped_out<F: Family>(lua: &Lua, hook: &str, handles: &[F]) -> mlua::Result<Value> {
    if handles.len() > MAX_LIST {
        log::warn!(
            target: LOG_TARGET,
            "{hook}: a list of {} handles is capped at {MAX_LIST}",
            handles.len()
        );
    }
    let table = lua.create_table()?;
    for (index, handle) in handles.iter().take(MAX_LIST).enumerate() {
        table.raw_set(index + 1, family_out(lua, *handle)?)?;
    }
    Ok(Value::Table(table))
}

/// An optional list of handles: nil for none.
pub fn family_opt_list_out<F: Family>(lua: &Lua, handles: Option<Vec<F>>) -> mlua::Result<Value> {
    match handles {
        Some(handles) => family_list_out(lua, handles),
        None => Ok(Value::Nil),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test family: a number in a Rust struct, with the id as its `tostring`.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Probe(u32);

    impl Family for Probe {
        const NAME: &'static str = "Probe";
        fn add_methods<M: UserDataMethods<Token<Self>>>(methods: &mut M) {
            methods.add_method("value", |_, this, ()| Ok(this.0.0));
        }
        fn describe(self) -> String {
            format!("Probe({})", self.0)
        }
    }

    /// A second family, to refuse a token of the wrong one.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Other;

    impl Family for Other {
        const NAME: &'static str = "Other";
        fn add_methods<M: UserDataMethods<Token<Self>>>(_: &mut M) {}
        fn describe(self) -> String {
            String::from("Other")
        }
    }

    fn args(values: Vec<Value>) -> MultiValue {
        MultiValue::from_vec(values)
    }

    #[test]
    fn a_handle_round_trips_and_a_token_of_another_family_is_refused() {
        let lua = Lua::new();
        let value = family_out(&lua, Probe(7)).unwrap();
        let call = args(vec![value.clone()]);
        let args = Args::new("probe", call, 1).unwrap();
        assert_eq!(args.handle::<Probe>(0).ok(), Some(Probe(7)));
        let error = args.handle::<Other>(0).unwrap_err().to_string();
        assert!(
            error.contains("probe: argument 1: must be a Other"),
            "{error}"
        );
    }

    #[test]
    fn too_many_arguments_are_refused_by_the_method_name() {
        let call = args(vec![Value::Nil, Value::Nil]);
        let error = Args::new("probe.value", call, 0).err().unwrap().to_string();
        assert!(
            error.contains("probe.value takes 0 argument(s), not 2"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_argument_is_nil_and_an_optional_handle_takes_it() {
        let args = Args::new("probe", args(vec![]), 1).unwrap();
        assert_eq!(args.opt_handle::<Probe>(0).ok(), Some(None));
        assert!(args.handle::<Probe>(0).is_err());
        assert!(args.get::<u32>(0).is_err());
    }

    #[test]
    fn an_optional_handle_result_and_a_list_are_userdata_or_nil() {
        let lua = Lua::new();
        assert!(family_opt_out::<Probe>(&lua, None).unwrap().is_nil());
        let list = family_list_out(&lua, vec![Probe(1), Probe(2)]).unwrap();
        let Value::Table(table) = list else {
            panic!("a list is a table");
        };
        let second: AnyUserData = table.get(2).unwrap();
        assert_eq!(second.borrow::<Token<Probe>>().unwrap().0, Probe(2));
    }

    #[test]
    fn a_refusal_raises_its_debug_text_clipped_to_120_characters() {
        let payload = "x".repeat(300);
        let clipped = refusal_text(&format!("{payload:?}"));
        assert_eq!(clipped.chars().count(), REFUSAL_CHARS + 3);
        assert!(clipped.ends_with("..."), "{clipped}");
        let err = ok_or_raise::<(), _>(Err(payload.clone()))
            .unwrap_err()
            .to_string();
        assert!(err.contains(&clipped), "{err}");
        assert!(
            !err.contains(&"x".repeat(121)),
            "the whole value must not reach the mod"
        );
        assert_eq!(refusal_text("short"), "short");
        assert_eq!(ok_or_raise::<u32, ()>(Ok(5)).ok(), Some(5));
    }

    #[test]
    fn the_powerful_writes_are_not_generated_and_say_why() {
        let coverage = include_str!("generated/handles_coverage.txt");
        let text = include_str!("generated/handles.rs");
        let denied = [
            "EntityRef::run_command",
            "EntityRef::teleport_to_level",
            "EntityRef::merge_nbt",
            "LevelRef::spawn_mob",
            "LevelRef::explode",
            "LevelRef::set_game_rule",
            "EntityRef::remove",
            "EntityRef::discard",
            "LevelRef::strike_lightning",
            "LevelRef::set_world_border_size",
            "PlayerRef::disconnect",
            "PlayerRef::set_game_mode",
            "LevelRef::invalidate_capabilities",
            "PlayerRef::award_stat",
            "PlayerRef::reset_stat",
        ];
        for member in denied {
            assert!(
                coverage.contains(&format!(
                    "handle:{member} skipped: powerful (needs manifest power)"
                )),
                "{member} is listed as powerful"
            );
            assert!(
                !text.contains(&format!("\"{member}\"")),
                "{member} is not bound"
            );
        }
        assert!(
            !text.contains("Registrar"),
            "no Registrar member is generated"
        );
    }

    #[test]
    fn the_bound_writes_are_the_reviewed_snapshot() {
        let coverage = include_str!("generated/handles_coverage.txt");
        let reviewed_text = include_str!("reviewed_writes.txt");
        let generated: std::collections::BTreeSet<&str> = coverage
            .lines()
            .filter_map(|line| {
                line.trim_start()
                    .strip_prefix("handle:")?
                    .strip_suffix(" bound, cost 10")
            })
            .collect();
        let reviewed: std::collections::BTreeSet<&str> = reviewed_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert!(
            !generated.is_empty(),
            "no bound writes found in the coverage"
        );
        let new: Vec<&str> = generated.difference(&reviewed).copied().collect();
        let removed: Vec<&str> = reviewed.difference(&generated).copied().collect();
        assert!(
            new.is_empty() && removed.is_empty(),
            "the bound writes differ from reviewed_writes.txt\n  new (bound, not reviewed): {new:?}\n  removed (reviewed, no longer bound): {removed:?}\nreview each new write against the powerful-name rules, then update the snapshot (crates/event-horizon-lua/src/reviewed_writes.txt)"
        );
    }

    #[test]
    fn an_ordinary_write_is_bound_with_its_cost() {
        assert_eq!(
            crate::generated::handles::cost("EntityRef::set_glowing"),
            Some(10)
        );
        assert_eq!(
            crate::generated::handles::cost("EntityRef::position"),
            Some(1)
        );
    }
    #[test]
    fn a_hook_list_of_handles_is_usable_and_capped_at_1024_with_one_warning() {
        use crate::host::tests::{captured_with_prefix, install_capture};
        install_capture();
        let lua = Lua::new();
        let three = family_capped_out(&lua, "list_hook", &[Probe(1), Probe(2), Probe(3)]).unwrap();
        lua.globals().set("three", three).unwrap();
        assert_eq!(
            lua.load("return three[3]:value()").eval::<u32>().unwrap(),
            3
        );
        let long: Vec<Probe> = (0..1025).map(Probe).collect();
        let capped = family_capped_out(&lua, "list_hook", &long).unwrap();
        let Value::Table(table) = capped else {
            panic!("a list is a table");
        };
        assert_eq!(table.raw_len(), MAX_LIST);
        let warnings = captured_with_prefix("list_hook: a list of 1025 handles is capped at 1024");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }
}
