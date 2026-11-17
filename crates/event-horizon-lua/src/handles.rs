//! Handle values in Lua (issue 74, H1b): each handle family's value is wrapped in a [`Token`]
//! userdata, and the generated methods ([`crate::generated::handles`]) convert their arguments and
//! results through the helpers here.
//!
//! A token holds only the value the game gave it. Its methods reach the world through the turn's
//! host, so a token kept from an earlier fire is answered by the game (a stale one gives `nil`).
//! A token exposes no fields and cannot be built from Lua: the only tokens are the ones a method
//! returns.

use std::fmt::Display;

use event_horizon_lua_convert::item::BlockStateSpec;
use event_horizon_lua_convert::scalar::FromLuaValue;
use mlua::{AnyUserData, Error, Lua, MetaMethod, MultiValue, UserData, UserDataMethods, Value};
use mod_api::{BlockState, Host};

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

    fn value(&self, index: usize) -> Value {
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
}
