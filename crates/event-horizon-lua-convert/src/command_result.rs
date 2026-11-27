//! `EntityRef::run_command_with_output`'s result as a Lua table: `{success = bool, feedback = {string, ...}}`.
//!
//! The feedback lines come from the game (a command's messages), so the output is bounded: at most
//! [`MAX_FEEDBACK_LINES`] lines, each cut to [`MAX_LINE_CHARS`] characters. When a line was cut or lines
//! were dropped, a final `"..."` entry follows the last line kept.

use mlua::{Lua, Value};
use mod_api::CommandResult;

use crate::scalar::ToLuaValue;

/// The most feedback lines one result carries to Lua.
pub const MAX_FEEDBACK_LINES: usize = 64;
/// The most characters one feedback line carries to Lua.
pub const MAX_LINE_CHARS: usize = 1024;

impl ToLuaValue for CommandResult {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        let table = lua.create_table()?;
        table.raw_set("success", self.success)?;
        let feedback = lua.create_table()?;
        let mut cut = self.feedback.len() > MAX_FEEDBACK_LINES;
        for (index, line) in self.feedback.iter().take(MAX_FEEDBACK_LINES).enumerate() {
            if line.chars().count() > MAX_LINE_CHARS {
                cut = true;
            }
            let text: String = line.chars().take(MAX_LINE_CHARS).collect();
            feedback.raw_set(index + 1, text)?;
        }
        if cut {
            feedback.raw_set(self.feedback.len().min(MAX_FEEDBACK_LINES) + 1, "...")?;
        }
        table.raw_set("feedback", feedback)?;
        Ok(Value::Table(table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_result(result: &CommandResult) -> (bool, Vec<String>) {
        let lua = Lua::new();
        let value = result.to_lua_value(&lua).expect("converts");
        let Value::Table(table) = value else {
            panic!("a table")
        };
        let success: bool = table.get("success").expect("success");
        let feedback: mlua::Table = table.get("feedback").expect("feedback");
        let lines = feedback
            .sequence_values::<String>()
            .collect::<mlua::Result<Vec<_>>>()
            .expect("strings");
        (success, lines)
    }

    #[test]
    fn a_result_keeps_its_success_and_its_lines_in_order() {
        let result = CommandResult::new(true, vec!["first".into(), "second".into()]);
        assert_eq!(
            lua_result(&result),
            (true, vec!["first".to_string(), "second".to_string()])
        );
    }

    #[test]
    fn more_than_64_lines_end_with_a_dots_entry() {
        let lines: Vec<String> = (0..70).map(|i| format!("line {i}")).collect();
        let (success, out) = lua_result(&CommandResult::new(false, lines));
        assert!(!success);
        assert_eq!(out.len(), MAX_FEEDBACK_LINES + 1);
        assert_eq!(out[0], "line 0");
        assert_eq!(out[MAX_FEEDBACK_LINES - 1], "line 63");
        assert_eq!(out[MAX_FEEDBACK_LINES], "...");
    }

    #[test]
    fn a_long_line_is_cut_to_1024_characters_and_ends_with_a_dots_entry() {
        let long = "x".repeat(MAX_LINE_CHARS + 5);
        let (_, out) = lua_result(&CommandResult::new(true, vec![long]));
        assert_eq!(out[0].chars().count(), MAX_LINE_CHARS);
        assert_eq!(out[1], "...");
    }

    #[test]
    fn exactly_64_short_lines_have_no_dots_entry() {
        let lines: Vec<String> = (0..MAX_FEEDBACK_LINES).map(|i| i.to_string()).collect();
        let (_, out) = lua_result(&CommandResult::new(true, lines));
        assert_eq!(out.len(), MAX_FEEDBACK_LINES);
    }
}
