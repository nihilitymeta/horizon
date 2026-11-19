//! The stacks and block states a generated hook carries: which shapes convert, and through which
//! host.
//!
//! A hook's argument or output of type `ItemStack`, `Option<ItemStack>`, `Vec<ItemStack>`,
//! `BlockState` or `Option<BlockState>` converts through the host its method has. The game's
//! `Host` gives items and states (`ItemStack::from_nbt`, `to_nbt`, `block_state_id`), so a `Host`
//! hook converts all of them. A level hook (`LevelHost`, or a `LevelSide`) has the state calls and no
//! item calls. Stacks do not need a host: a hook with no `Host` converts them through the
//! process-global tables (`GlobalHost`), as every hook may. A
//! hook whose host type is not one of these converts its states through no host and is skipped
//! with the reason [`refusal`] gives. Block states need a host or a level: the global tables give
//! none.
//!
//! The emitted expressions call the Lua crate's `stacks` module, whose functions take the host as
//! `&dyn Host`, `&dyn LevelHost` or `GlobalHost`. [`view`] is that expression for a method.

/// The shape of a carried value, as the generated code converts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// `ItemStack`: a non-optional stack.
    Stack,
    /// `Option<ItemStack>`: a stack, or no stack.
    OptStack,
    /// `Vec<ItemStack>`: a list of stacks (outputs only).
    StackList,
    /// `BlockState`.
    State,
    /// `Option<BlockState>`.
    OptState,
}

/// The host a hook's method gets, and so what it can convert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// A `Host` (`&dyn Host`, `&mut dyn ServerHost`, `&dyn ClientHost`, `&mut dyn ClientHost`) or a
    /// `HostSide`: items and block states.
    Host,
    /// A level (`&dyn LevelHost`, `&mut dyn LevelRandomHost`, `LevelSide`, ...): block states only.
    Level,
    /// No host parameter: items and block states through the process-global tables
    /// (`stacks::GlobalHost`).
    Global,
    /// A host type this generator does not carry: neither.
    Nothing,
}

/// The reach of a method whose host parameter has type `host_ty` (`None` for no host).
pub fn reach_of(host_ty: Option<&str>) -> Reach {
    match host_ty {
        None => Reach::Global,
        Some("&dyn Host" | "&mut dyn ServerHost" | "&dyn ClientHost" | "&mut dyn ClientHost") => {
            Reach::Host
        }
        Some("HostSide") => Reach::Host,
        Some(
            "&dyn LevelHost"
            | "&mut dyn LevelRandomHost"
            | "&mut dyn LevelWriteHost"
            | "&dyn LevelTicksHost"
            | "LevelSide",
        ) => Reach::Level,
        Some(_) => Reach::Nothing,
    }
}

/// The shape of a carried type as the surface or a dispatch function spells it (lifetimes, `&`
/// and `mut` left out), or `None` for any other type.
pub fn shape_of(ty: &str) -> Option<Shape> {
    let plain = plain(ty);
    Some(match plain.as_str() {
        "ItemStack" => Shape::Stack,
        "Option<ItemStack>" => Shape::OptStack,
        "Vec<ItemStack>" => Shape::StackList,
        "BlockState" => Shape::State,
        "Option<BlockState>" => Shape::OptState,
        _ => return None,
    })
}

/// The type without its module paths, lifetimes, references, `mut` or spaces.
fn plain(ty: &str) -> String {
    let text = ty
        .replace("mod_api::", "")
        .replace("item::item_stack::", "");
    let text: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    text.replace("'a", "").replace("&mut", "").replace('&', "")
}

/// Whether a value of `shape` converts through a host of `reach`. Stacks and block states convert
/// through the host, the level, or the global tables (`stacks::GlobalHost`, which reads a state's
/// block and properties from the installed definitions). A method with no host parameter at all
/// (`Reach::Nothing`) carries neither. A list is an output only, and a hook's list input is not
/// converted here (the caller refuses it).
pub fn allowed(shape: Shape, reach: Reach) -> bool {
    match shape {
        Shape::State | Shape::OptState => {
            matches!(reach, Reach::Host | Reach::Level | Reach::Global)
        }
        Shape::Stack | Shape::OptStack | Shape::StackList => true,
    }
}

/// Whether `shape` is a stack shape (the carried values that the global tables also convert).
pub fn is_stack(shape: Shape) -> bool {
    matches!(shape, Shape::Stack | Shape::OptStack | Shape::StackList)
}

/// The reach a stack converts through: the method's host when it is a `Host`, else the global
/// tables. A stack never needs a level host, so a level hook's stacks go through the tables.
pub fn stack_reach(reach: Reach) -> Reach {
    match reach {
        Reach::Host => Reach::Host,
        Reach::Level | Reach::Nothing | Reach::Global => Reach::Global,
    }
}

/// The reason a value of `shape` is skipped when its host cannot convert it.
pub fn refusal(shape: Shape) -> String {
    match shape {
        Shape::State | Shape::OptState => "no host path for block states".to_owned(),
        Shape::Stack | Shape::OptStack | Shape::StackList => "no host path for stacks".to_owned(),
    }
}

/// The expression that is the method's host as the Lua crate takes it, or `None` with no host.
pub fn view(host_name: Option<&str>, host_ty: Option<&str>, reach: Reach) -> Option<String> {
    if reach == Reach::Global {
        return Some("&crate::stacks::GlobalHost".to_owned());
    }
    let name = host_name?;
    let ty = host_ty?;
    Some(match (reach, ty) {
        (Reach::Nothing | Reach::Global, _) => return None,
        (Reach::Host, "&dyn Host") => name.to_owned(),
        (Reach::Host, "HostSide") => format!("crate::stacks::side_host(&{name})"),
        (Reach::Host, _) => upcast(name, ty, "mod_api::Host"),
        (Reach::Level, "&dyn LevelHost") => name.to_owned(),
        (Reach::Level, "LevelSide") => format!("crate::stacks::side_level(&{name})"),
        (Reach::Level, _) => upcast(name, ty, "mod_api::LevelHost"),
    })
}

/// The host upcast to `target`: a `&mut` host is reborrowed first, an immutable one is cast as it is
/// (a reborrow of a shared reference is a no-op, and clippy reports it).
fn upcast(name: &str, ty: &str, target: &str) -> String {
    if ty.starts_with("&mut ") {
        format!("&*{name} as &dyn {target}")
    } else {
        format!("{name} as &dyn {target}")
    }
}

/// The `Option` of the value a conversion takes, for an input: `event.name` as the event field is.
/// The field's type (`raw`) decides whether it is a reference, an `Option` or a plain value.
fn input_value(raw: &str, field: &str) -> String {
    let compact: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.starts_with("Option<&") {
        field.to_owned()
    } else if compact.starts_with("Option<") {
        format!("{field}.as_ref()")
    } else if compact.starts_with('&') {
        format!("Some({field})")
    } else {
        format!("Some(&{field})")
    }
}

/// The current value of an output, read from its `RefCell` `out_<key>`, as the conversion takes it.
fn output_value(shape: Shape, key: &str) -> String {
    match shape {
        Shape::Stack | Shape::State => format!("Some(&*out_{key}.borrow())"),
        Shape::OptStack | Shape::OptState => format!("out_{key}.borrow().as_ref()"),
        Shape::StackList => format!("out_{key}.borrow().as_slice()"),
    }
}

/// The call that gives a carried value's spec, `None` when it has no id: `stack_in` or `state_in`
/// (or `stack_list_in`) of the view and the value.
pub fn spec_call(shape: Shape, view: &str, value: &str) -> String {
    let function = match shape {
        Shape::Stack | Shape::OptStack => "stack_in",
        Shape::StackList => "stack_list_in",
        Shape::State | Shape::OptState => "state_in",
    };
    format!("crate::stacks::{function}({view}, {value})")
}

/// The Lua value of an input: its spec's table. Pre-checked, so the `ok_or_else` never fires.
pub fn input_lua(shape: Shape, raw: &str, field: &str, view: &str) -> String {
    format!(
        "{}.ok_or_else(|| mlua::Error::runtime(\"a carried value has no id\"))?.to_lua_value(lua)?",
        spec_call(shape, view, &input_value(raw, field))
    )
}

/// The Lua value of an output's current value.
pub fn output_lua(shape: Shape, key: &str, view: &str) -> String {
    format!(
        "{}.ok_or_else(|| mlua::Error::runtime(\"a carried value has no id\"))?.to_lua_value(lua)?",
        spec_call(shape, view, &output_value(shape, key))
    )
}

/// The pre-check call of an output's current value, read from its `RefCell`.
pub fn output_check(shape: Shape, key: &str, view: &str) -> String {
    spec_call(shape, view, &output_value(shape, key))
}

/// The pre-check call of an input, from its raw field type.
pub fn input_check(shape: Shape, raw: &str, field: &str, view: &str) -> String {
    spec_call(shape, view, &input_value(raw, field))
}

/// The converter `accept_by` (or `accept_opt_by`, for an `Option` output) takes for an output: the
/// conversion of a mod's value to the value's own type. An `Option` output's `horizon.none` is
/// handled by the dispatch, so the optional shapes convert like their inner shapes.
pub fn converter(shape: Shape, view: &str) -> String {
    let function = match shape {
        Shape::Stack | Shape::OptStack => "stack_value",
        Shape::StackList => "stack_list_value",
        Shape::State | Shape::OptState => "state_value",
    };
    format!("|value| crate::stacks::{function}({view}, value)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_shapes_are_read_from_their_spellings() {
        assert_eq!(
            shape_of("&'a item::item_stack::ItemStack"),
            Some(Shape::Stack)
        );
        assert_eq!(
            shape_of("Option<&'a item::item_stack::ItemStack>"),
            Some(Shape::OptStack)
        );
        assert_eq!(
            shape_of("&'a mut Option<item::item_stack::ItemStack>"),
            Some(Shape::OptStack)
        );
        assert_eq!(
            shape_of("&'a mut Vec<item::item_stack::ItemStack>"),
            Some(Shape::StackList)
        );
        assert_eq!(shape_of("BlockState"), Some(Shape::State));
        assert_eq!(shape_of("&'a mut BlockState"), Some(Shape::State));
        assert_eq!(shape_of("Option<BlockState>"), Some(Shape::OptState));
        assert_eq!(
            shape_of("&'a mut Option<mod_api::BlockState>"),
            Some(Shape::OptState)
        );
    }

    #[test]
    fn lists_of_stacks_and_other_arrays_are_not_one_shape() {
        assert_eq!(
            shape_of("&'a[Option<item::item_stack::ItemStack>; 3]"),
            None
        );
        assert_eq!(
            shape_of("&'a mut[Option<item::item_stack::ItemStack>; 5]"),
            None
        );
        assert_eq!(shape_of("&'a[Option<item::item_stack::ItemStack>]"), None);
        assert_eq!(shape_of("Option<u32>"), None);
    }

    #[test]
    fn reach_follows_the_host_parameter() {
        assert_eq!(reach_of(Some("&mut dyn ServerHost")), Reach::Host);
        assert_eq!(reach_of(Some("&dyn ClientHost")), Reach::Host);
        assert_eq!(reach_of(Some("HostSide")), Reach::Host);
        assert_eq!(reach_of(Some("&dyn LevelHost")), Reach::Level);
        assert_eq!(reach_of(Some("&mut dyn LevelRandomHost")), Reach::Level);
        assert_eq!(reach_of(None), Reach::Global);
        assert_eq!(reach_of(Some("&dyn WorldgenHost")), Reach::Nothing);
    }

    #[test]
    fn a_level_has_states_and_stacks_come_from_the_tables() {
        assert!(allowed(Shape::State, Reach::Level));
        assert!(allowed(Shape::OptState, Reach::Host));
        assert!(allowed(Shape::Stack, Reach::Level));
        assert!(allowed(Shape::StackList, Reach::Level));
        assert!(!allowed(Shape::State, Reach::Nothing));
        assert!(allowed(Shape::State, Reach::Global));
        assert!(allowed(Shape::Stack, Reach::Nothing));
        assert_eq!(stack_reach(Reach::Level), Reach::Global);
        assert_eq!(stack_reach(Reach::Host), Reach::Host);
        assert_eq!(refusal(Shape::OptState), "no host path for block states");
    }

    #[test]
    fn a_level_side_is_a_level() {
        assert_eq!(reach_of(Some("LevelSide")), Reach::Level);
        assert_eq!(
            view(Some("level"), Some("LevelSide"), Reach::Level).as_deref(),
            Some("crate::stacks::side_level(&level)")
        );
    }

    #[test]
    fn the_view_is_the_host_as_the_lua_crate_takes_it() {
        assert_eq!(
            view(Some("host"), Some("&dyn Host"), Reach::Host).as_deref(),
            Some("host")
        );
        assert_eq!(
            view(Some("host"), Some("&mut dyn ServerHost"), Reach::Host).as_deref(),
            Some("&*host as &dyn mod_api::Host")
        );
        assert_eq!(
            view(Some("host"), Some("HostSide"), Reach::Host).as_deref(),
            Some("crate::stacks::side_host(&host)")
        );
        assert_eq!(
            view(
                Some("level"),
                Some("&mut dyn LevelRandomHost"),
                Reach::Level
            )
            .as_deref(),
            Some("&*level as &dyn mod_api::LevelHost")
        );
        assert_eq!(
            view(Some("host"), Some("&dyn ClientHost"), Reach::Host).as_deref(),
            Some("host as &dyn mod_api::Host")
        );
        assert_eq!(
            view(Some("level"), Some("&dyn LevelTicksHost"), Reach::Level).as_deref(),
            Some("level as &dyn mod_api::LevelHost")
        );
        assert_eq!(view(None, None, Reach::Nothing), None);
        assert_eq!(
            view(None, None, Reach::Global).as_deref(),
            Some("&crate::stacks::GlobalHost")
        );
    }

    #[test]
    fn a_hook_with_no_host_takes_items_through_the_global_table_and_no_states() {
        assert!(allowed(Shape::Stack, Reach::Global));
        assert!(allowed(Shape::OptStack, Reach::Global));
        assert!(allowed(Shape::StackList, Reach::Global));
        assert!(allowed(Shape::State, Reach::Global));
        assert!(allowed(Shape::OptState, Reach::Global));
    }

    #[test]
    fn an_input_is_read_from_its_field_by_its_spelling() {
        assert_eq!(
            input_check(
                Shape::Stack,
                "&'a item::item_stack::ItemStack",
                "event.stack",
                "v"
            ),
            "crate::stacks::stack_in(v, Some(event.stack))"
        );
        assert_eq!(
            input_check(
                Shape::OptStack,
                "Option<&'a item::item_stack::ItemStack>",
                "event.stack",
                "v"
            ),
            "crate::stacks::stack_in(v, event.stack)"
        );
        assert_eq!(
            input_check(Shape::State, "BlockState", "event.state", "v"),
            "crate::stacks::state_in(v, Some(&event.state))"
        );
        assert_eq!(
            input_check(Shape::OptState, "Option<BlockState>", "event.state", "v"),
            "crate::stacks::state_in(v, event.state.as_ref())"
        );
    }

    #[test]
    fn an_output_is_read_from_its_cell() {
        assert_eq!(
            output_check(Shape::StackList, "items", "v"),
            "crate::stacks::stack_list_in(v, out_items.borrow().as_slice())"
        );
        assert_eq!(
            converter(Shape::OptState, "v"),
            "|value| crate::stacks::state_value(v, value)"
        );
    }
}
