//! Chat and UI text between Lua and Rust: a [`mod_api::Text`].
//!
//! Text is either a plain Lua string (literal characters, no formatting codes read) or a
//! component table `{text=, color=, bold=, italic=, underlined=, strikethrough=, obfuscated=,
//! extra={...}}`. Output is a plain string when the text is one unstyled literal, and a
//! component table otherwise. A colour is one of the 16 named Minecraft colours or `#rrggbb`.
//!
//! The text comes from mods, so reading is bounded (depth, components and characters) and
//! strict: a wrongly typed known key is an error that names it, unknown keys are ignored, and
//! table fields are read raw, so no metamethod runs. Output is bounded by the same limits.
//! Components the Lua form cannot express (translations, selectors, scores, NBT, click and
//! hover events, fonts, insertions, shadows, unknown metadata) are an error on output, never
//! dropped silently.

use mlua::{Lua, Table, Value};
use mod_api::Text;
use mod_api::text::{TextContent, TextStyle};

use crate::scalar::{ConvertError, FromLuaValue, ToLuaValue};

/// The deepest nesting of `extra` that is accepted, counting the root component as 1.
pub const MAX_DEPTH: usize = 16;
/// The most components (the root included) one Text may hold.
pub const MAX_SPANS: usize = 1024;
/// The most characters one Text may hold, across all its components.
pub const MAX_CHARS: usize = 64 * 1024;

/// The 16 named colours, as Minecraft names them.
const NAMED_COLORS: [&str; 16] = [
    "black",
    "dark_blue",
    "dark_green",
    "dark_aqua",
    "dark_red",
    "dark_purple",
    "gold",
    "gray",
    "dark_gray",
    "blue",
    "green",
    "aqua",
    "red",
    "light_purple",
    "yellow",
    "white",
];

/// Counts components and characters over one Text, so the limits hold for the whole tree.
#[derive(Default)]
struct Budget {
    spans: usize,
    chars: usize,
}

impl Budget {
    /// Counts one component at `depth`, and its characters.
    fn component(&mut self, depth: usize, chars: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(format!("text nests deeper than {MAX_DEPTH} components"));
        }
        self.spans += 1;
        if self.spans > MAX_SPANS {
            return Err(format!("text has more than {MAX_SPANS} components"));
        }
        self.chars = self.chars.saturating_add(chars);
        if self.chars > MAX_CHARS {
            return Err(format!("text has more than {MAX_CHARS} characters"));
        }
        Ok(())
    }
}

impl FromLuaValue for Text {
    fn from_lua_value(value: &Value) -> Result<Self, ConvertError> {
        read_text(value, 1, &mut Budget::default())
    }
}

impl ToLuaValue for Text {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        write_text(lua, self, 1, &mut Budget::default())
    }
}

/// A plain string or a component table, at `depth`.
fn read_text(value: &Value, depth: usize, budget: &mut Budget) -> Result<Text, ConvertError> {
    match value {
        Value::String(_) => {
            let plain = String::from_lua_value(value)?;
            budget
                .component(depth, plain.chars().count())
                .map_err(ConvertError::new)?;
            Ok(Text::literal(plain))
        }
        Value::Table(table) => read_component(table, depth, budget),
        other => Err(ConvertError::expected("text", other)),
    }
}

/// A component table at `depth`: its own fields, then its `extra` children in order.
fn read_component(table: &Table, depth: usize, budget: &mut Budget) -> Result<Text, ConvertError> {
    let content = match raw_field(table, "text")? {
        Value::Nil => String::new(),
        value => string_field("text", &value)?,
    };
    budget
        .component(depth, content.chars().count())
        .map_err(ConvertError::new)?;
    let mut text = Text::literal(content);
    let style = read_style(table)?;
    text.style = style;
    if let Some(extra) = child_list(table)? {
        let count = extra.raw_len();
        for index in 1..=count {
            let child: Value = extra
                .raw_get(index)
                .map_err(|_| ConvertError::new(format!("cannot read extra[{index}]")))?;
            let child = read_text(&child, depth + 1, budget)
                .map_err(|e| ConvertError::new(format!("extra[{index}]: {e}")))?;
            text.children.push(child);
        }
    }
    Ok(text)
}

/// The style fields of a component table. Absent fields stay unset, so they inherit.
fn read_style(table: &Table) -> Result<TextStyle, ConvertError> {
    let mut style = TextStyle::default();
    style.color = match raw_field(table, "color")? {
        Value::Nil => None,
        value => Some(color_name(&string_field("color", &value)?)?),
    };
    style.bold = flag(table, "bold")?;
    style.italic = flag(table, "italic")?;
    style.underlined = flag(table, "underlined")?;
    style.strikethrough = flag(table, "strikethrough")?;
    style.obfuscated = flag(table, "obfuscated")?;
    Ok(style)
}

/// The `extra` list of a component table, or None when it has none.
fn child_list(table: &Table) -> Result<Option<Table>, ConvertError> {
    match raw_field(table, "extra")? {
        Value::Nil => Ok(None),
        Value::Table(list) => Ok(Some(list)),
        other => Err(ConvertError::new(format!(
            "extra: expected list of text, got {}",
            other.type_name()
        ))),
    }
}

/// A named colour or `#rrggbb`, returned as given. Anything else is an error that names it.
fn color_name(value: &str) -> Result<String, ConvertError> {
    if NAMED_COLORS.contains(&value) {
        return Ok(value.to_owned());
    }
    let hex = value.strip_prefix('#').unwrap_or_default();
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(value.to_owned());
    }
    Err(ConvertError::new(format!(
        "color: expected a named colour or #rrggbb, got {value:?}"
    )))
}

/// A boolean style field: nil is unset, and a non-boolean is an error that names the key.
fn flag(table: &Table, key: &str) -> Result<Option<bool>, ConvertError> {
    match raw_field(table, key)? {
        Value::Nil => Ok(None),
        Value::Boolean(b) => Ok(Some(b)),
        other => Err(ConvertError::new(format!(
            "{key}: expected boolean, got {}",
            other.type_name()
        ))),
    }
}

/// A string field, named in the error when it is not a valid UTF-8 string.
fn string_field(key: &str, value: &Value) -> Result<String, ConvertError> {
    if !matches!(value, Value::String(_)) {
        return Err(ConvertError::new(format!(
            "{key}: expected string, got {}",
            value.type_name()
        )));
    }
    String::from_lua_value(value).map_err(|e| ConvertError::new(format!("{key}: {e}")))
}

/// One field of a table, read raw: no `__index` metamethod runs.
fn raw_field(table: &Table, key: &str) -> Result<Value, ConvertError> {
    table
        .raw_get::<Value>(key)
        .map_err(|_| ConvertError::new(format!("cannot read {key}")))
}

/// Writes `text` at `depth` as a plain string or a component table, checking the limits.
fn write_text(lua: &Lua, text: &Text, depth: usize, budget: &mut Budget) -> mlua::Result<Value> {
    let literal = match &text.content {
        TextContent::Literal(value) => value,
        other => {
            return Err(mlua::Error::runtime(format!(
                "text content {} has no Lua form",
                content_kind(other)
            )));
        }
    };
    budget
        .component(depth, literal.chars().count())
        .map_err(mlua::Error::runtime)?;
    check_expressible(&text.style, text.metadata == Text::default().metadata)?;
    if text.children.is_empty() && text.style == TextStyle::default() {
        return Ok(Value::String(lua.create_string(literal)?));
    }
    let table = lua.create_table()?;
    table.raw_set("text", literal.as_str())?;
    let style = &text.style;
    if let Some(color) = &style.color {
        table.raw_set("color", color.as_str())?;
    }
    for (key, value) in [
        ("bold", style.bold),
        ("italic", style.italic),
        ("underlined", style.underlined),
        ("strikethrough", style.strikethrough),
        ("obfuscated", style.obfuscated),
    ] {
        if let Some(value) = value {
            table.raw_set(key, value)?;
        }
    }
    if !text.children.is_empty() {
        let extra = lua.create_table()?;
        for (index, child) in text.children.iter().enumerate() {
            extra.raw_set(index + 1, write_text(lua, child, depth + 1, budget)?)?;
        }
        table.raw_set("extra", extra)?;
    }
    Ok(Value::Table(table))
}

/// Refuses a style or metadata that the Lua form has no field for.
fn check_expressible(style: &TextStyle, metadata_empty: bool) -> mlua::Result<()> {
    let default = TextStyle::default();
    let unsupported = [
        ("shadow colour", style.shadow_color.is_some()),
        ("click event", style.click.is_some()),
        ("hover event", style.hover.is_some()),
        ("insertion", style.insertion.is_some()),
        ("font", style.font.is_some()),
        ("style metadata", style.metadata != default.metadata),
        ("component metadata", !metadata_empty),
    ];
    match unsupported.iter().find(|(_, present)| *present) {
        Some((what, _)) => Err(mlua::Error::runtime(format!("text {what} has no Lua form"))),
        None => Ok(()),
    }
}

/// The name of a non-literal content kind, for the error on output.
fn content_kind(content: &TextContent) -> &'static str {
    match content {
        TextContent::Literal(_) => "literal",
        TextContent::Translatable { .. } => "translatable",
        TextContent::Keybind(_) => "keybind",
        TextContent::Selector { .. } => "selector",
        TextContent::Score { .. } => "score",
        TextContent::Nbt { .. } => "nbt",
        TextContent::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(lua: &Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk runs")
    }

    fn read(source: &str) -> Result<Text, ConvertError> {
        let lua = Lua::new();
        let value = eval(&lua, source);
        Text::from_lua_value(&value)
    }

    /// A chain of `depth` nested component tables, the innermost holding "x".
    fn nested(lua: &Lua, depth: usize) -> Value {
        let mut inner = lua.create_table().expect("table");
        inner.raw_set("text", "x").expect("set");
        for _ in 1..depth {
            let outer = lua.create_table().expect("table");
            let list = lua.create_table().expect("table");
            list.raw_set(1, inner).expect("set");
            outer.raw_set("extra", list).expect("set");
            inner = outer;
        }
        Value::Table(inner)
    }

    #[test]
    fn plain_string_reads_as_a_literal_and_writes_back_as_a_string() {
        let lua = Lua::new();
        let v = eval(&lua, "return 'hi §a'");
        let text = Text::from_lua_value(&v).expect("plain");
        assert_eq!(text, Text::literal("hi §a"));
        let back = text.to_lua_value(&lua).expect("to lua");
        assert_eq!(String::from_lua_value(&back), Ok("hi §a".to_string()));
    }

    #[test]
    fn styled_component_reads_and_writes_back() {
        let lua = Lua::new();
        let v = eval(&lua, "return {text = 'warn', color = 'red', bold = true}");
        let text = Text::from_lua_value(&v).expect("component");
        assert_eq!(text.style.color.as_deref(), Some("red"));
        assert_eq!(text.style.bold, Some(true));
        assert_eq!(text.style.italic, None);
        assert_eq!(text.content, TextContent::Literal("warn".to_string()));
        let back = text.to_lua_value(&lua).expect("to lua");
        let Value::Table(t) = &back else {
            panic!("expected a table, got {}", back.type_name());
        };
        assert_eq!(t.raw_get::<String>("text").expect("text"), "warn");
        assert_eq!(t.raw_get::<String>("color").expect("color"), "red");
        assert!(t.raw_get::<bool>("bold").expect("bold"));
        let round = Text::from_lua_value(&back).expect("read back");
        assert_eq!(round, text);
    }

    #[test]
    fn nested_extra_keeps_its_order() {
        let text = read("return {text = 'a', extra = {'b', {text = 'c', bold = false}, 'd'}}")
            .expect("nested");
        assert_eq!(text.content, TextContent::Literal("a".to_string()));
        let kids: Vec<&TextContent> = text.children.iter().map(|c| &c.content).collect();
        assert_eq!(
            kids,
            vec![
                &TextContent::Literal("b".to_string()),
                &TextContent::Literal("c".to_string()),
                &TextContent::Literal("d".to_string()),
            ]
        );
        assert_eq!(text.children[1].style.bold, Some(false));
    }

    #[test]
    fn output_of_a_tree_has_its_children_as_an_ordered_list() {
        let lua = Lua::new();
        let mut style = TextStyle::default();
        style.color = Some("#ff8800".to_string());
        let text = Text::literal("root")
            .append(Text::literal("one"))
            .append(Text::literal("two").with_style(style));
        let v = text.to_lua_value(&lua).expect("to lua");
        let Value::Table(t) = &v else {
            panic!("expected a table");
        };
        let extra: Table = t.raw_get("extra").expect("extra");
        assert_eq!(extra.raw_get::<String>(1).expect("first"), "one");
        let second: Table = extra.raw_get(2).expect("second");
        assert_eq!(second.raw_get::<String>("color").expect("color"), "#ff8800");
        assert_eq!(Text::from_lua_value(&v), Ok(text));
    }

    #[test]
    fn hex_colour_is_accepted_and_kept() {
        let text = read("return {text = 'x', color = '#ff8800'}").expect("hex");
        assert_eq!(text.style.color.as_deref(), Some("#ff8800"));
    }

    #[test]
    fn every_named_colour_is_accepted() {
        for name in NAMED_COLORS {
            let text = read(&format!("return {{text = 'x', color = '{name}'}}")).expect(name);
            assert_eq!(text.style.color.as_deref(), Some(name));
        }
    }

    #[test]
    fn bad_colour_is_an_error_that_names_it() {
        let err = read("return {text = 'x', color = '#ff88'}").expect_err("short hex");
        assert!(err.to_string().contains("#ff88"), "{err}");
        let err = read("return {text = 'x', color = 'crimson'}").expect_err("unknown name");
        assert!(err.to_string().contains("crimson"), "{err}");
        assert!(read("return {text = 'x', color = '#gg0000'}").is_err());
    }

    #[test]
    fn wrongly_typed_flag_is_an_error_that_names_the_key() {
        let err = read("return {text = 'x', bold = 'yes'}").expect_err("bold");
        assert!(err.to_string().contains("bold"), "{err}");
        assert!(err.to_string().contains("expected boolean"), "{err}");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let text = read("return {text = 'x', colour = 'nonsense', weight = 3}").expect("ignored");
        assert_eq!(text, Text::literal("x"));
    }

    #[test]
    fn depth_sixteen_works_and_seventeen_is_refused() {
        let lua = Lua::new();
        let ok = Text::from_lua_value(&nested(&lua, MAX_DEPTH));
        assert!(ok.is_ok(), "{ok:?}");
        let err = Text::from_lua_value(&nested(&lua, MAX_DEPTH + 1)).expect_err("too deep");
        assert!(err.to_string().contains("16"), "{err}");
    }

    #[test]
    fn self_referential_table_is_refused_without_overflowing() {
        let lua = Lua::new();
        let t = lua.create_table().expect("table");
        t.raw_set("text", "loop").expect("set");
        let list = lua.create_table().expect("table");
        list.raw_set(1, t.clone()).expect("set");
        t.raw_set("extra", list).expect("set");
        let err = Text::from_lua_value(&Value::Table(t)).expect_err("cycle");
        assert!(err.to_string().contains("deeper"), "{err}");
    }

    #[test]
    fn more_than_1024_components_are_refused() {
        let lua = Lua::new();
        let list = lua.create_table().expect("table");
        for i in 1..=MAX_SPANS {
            list.raw_set(i, "a").expect("set");
        }
        let root = lua.create_table().expect("table");
        root.raw_set("extra", list.clone()).expect("set");
        // the root plus 1024 children is 1025 components
        let err = Text::from_lua_value(&Value::Table(root)).expect_err("1025");
        assert!(err.to_string().contains("1024"), "{err}");
        list.raw_set(MAX_SPANS, Value::Nil).expect("remove one");
        let root = lua.create_table().expect("table");
        root.raw_set("extra", list).expect("set");
        assert!(Text::from_lua_value(&Value::Table(root)).is_ok());
    }

    #[test]
    fn characters_past_the_limit_are_refused() {
        let lua = Lua::new();
        let exact = "a".repeat(MAX_CHARS);
        let ok = Text::from_lua_value(&Value::String(lua.create_string(&exact).expect("s")));
        assert!(ok.is_ok());
        let over = "a".repeat(MAX_CHARS + 1);
        let err = Text::from_lua_value(&Value::String(lua.create_string(&over).expect("s")))
            .expect_err("65537");
        assert!(err.to_string().contains("65536"), "{err}");
    }

    #[test]
    fn characters_add_up_across_components() {
        let lua = Lua::new();
        let half = "a".repeat(MAX_CHARS / 2 + 1);
        let root = lua.create_table().expect("table");
        root.raw_set("text", half.as_str()).expect("set");
        let list = lua.create_table().expect("table");
        list.raw_set(1, half.as_str()).expect("set");
        root.raw_set("extra", list).expect("set");
        assert!(Text::from_lua_value(&Value::Table(root)).is_err());
    }

    #[test]
    fn metamethods_are_not_run() {
        let lua = Lua::new();
        let t = lua.create_table().expect("table");
        t.raw_set("text", "raw").expect("set");
        let meta = lua.create_table().expect("meta");
        let failing = lua
            .create_function(|_, ()| -> mlua::Result<Value> {
                Err(mlua::Error::runtime("metamethod ran"))
            })
            .expect("function");
        meta.raw_set("__index", failing).expect("set");
        t.set_metatable(Some(meta)).expect("metatable");
        let text = Text::from_lua_value(&Value::Table(t)).expect("raw fields only");
        assert_eq!(text, Text::literal("raw"));
    }

    #[test]
    fn non_text_input_is_an_error_naming_the_type() {
        let lua = Lua::new();
        let err = Text::from_lua_value(&Value::Number(3.0)).expect_err("number");
        assert_eq!(err.to_string(), "expected text, got number");
        assert!(Text::from_lua_value(&eval(&lua, "return nil")).is_err());
    }

    #[test]
    fn output_refuses_what_lua_cannot_express() {
        let lua = Lua::new();
        let translated = Text::translate("chat.type.text", Vec::new());
        assert!(translated.to_lua_value(&lua).is_err());
        let mut style = TextStyle::default();
        style.font = Some("minecraft:default".to_string());
        let fonted = Text::literal("go").with_style(style);
        assert!(fonted.to_lua_value(&lua).is_err());
    }
}
