//! The lint pass: each script is parsed (full_moon, Luau syntax) and linted with selene's default
//! lints. The mod API's global, `horizon`, is declared as a name of any shape, so a call into the
//! API is not an undefined variable. Selene's own lint levels are kept: a lint that is a warning in
//! selene is a warning here.

use full_moon::ast::Ast;
use selene_lib::lints::Severity;
use selene_lib::standard_library::{Field, FieldKind, StandardLibrary};
use selene_lib::{Checker, CheckerConfig};

use crate::finding::{Finding, Level, line_col};

/// The global names the Lua mod API declares (`declare horizon: HorizonApi` in `horizon.d.luau`).
const API_GLOBALS: &[&str] = &["horizon"];

pub struct Linter {
    checker: Checker<serde_json::Value>,
}

impl Linter {
    pub fn new() -> Result<Linter, String> {
        let mut std =
            StandardLibrary::from_name("luau").ok_or("selene has no Luau standard library")?;
        let mut api = StandardLibrary::default();
        for name in API_GLOBALS {
            api.globals
                .insert((*name).to_owned(), Field::from_field_kind(FieldKind::Any));
        }
        std.extend(api);
        let checker = Checker::new(CheckerConfig::default(), std)
            .map_err(|error| format!("selene's configuration: {error}"))?;
        Ok(Linter { checker })
    }

    /// Lints a parsed script. A lint that selene sets to allow is not reported.
    pub fn lint(&self, file: &str, text: &str, ast: &Ast) -> Vec<Finding> {
        self.checker
            .test_on(ast)
            .into_iter()
            .filter_map(|found| {
                let level = match found.severity {
                    Severity::Error => Level::Error,
                    Severity::Warning => Level::Warning,
                    Severity::Allow => return None,
                };
                let (line, column) =
                    line_col(text, found.diagnostic.primary_label.range.0 as usize);
                Some(Finding::at(
                    file,
                    line,
                    column,
                    level,
                    format!("{} ({})", found.diagnostic.message, found.diagnostic.code),
                ))
            })
            .collect()
    }
}

/// Parses a script with Luau syntax. A syntax error is a finding at the place full_moon names.
pub fn parse(file: &str, text: &str) -> Result<Ast, Vec<Finding>> {
    full_moon::parse(text).map_err(|errors| {
        errors
            .iter()
            .map(|error| {
                let start = match error {
                    full_moon::Error::AstError(found) => found.range().0,
                    full_moon::Error::TokenizerError(found) => found.range().0,
                };
                Finding::at(
                    file,
                    start.line(),
                    start.character(),
                    Level::Error,
                    format!("syntax error: {}", error.error_message()),
                )
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint_text(text: &str) -> Vec<Finding> {
        let linter = Linter::new().expect("selene's standard library");
        let ast = parse("main.luau", text).expect("the script parses");
        linter.lint("main.luau", text, &ast)
    }

    #[test]
    fn a_call_into_the_mod_api_is_not_an_undefined_variable() {
        assert!(
            lint_text("horizon.on(\"server_tick\", function() end)\nhorizon.log(\"hi\")\n")
                .iter()
                .all(|f| !f.text.contains("undefined_variable"))
        );
    }

    #[test]
    fn an_unused_local_is_a_warning_at_its_line() {
        let found = lint_text("local ok = 1\nlocal unused = 2\nprint(ok)\n");
        let unused: Vec<&Finding> = found
            .iter()
            .filter(|f| f.text.ends_with("(unused_variable)"))
            .collect();
        assert_eq!(unused.len(), 1, "{found:?}");
        assert_eq!(unused[0].level, Level::Warning);
        assert_eq!((unused[0].line, unused[0].column), (2, 7));
    }

    #[test]
    fn a_syntax_error_names_its_line() {
        let errors = parse("main.luau", "local x = 1\nlocal = 2\n").unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .all(|f| f.level == Level::Error && f.line == 2)
        );
    }
}
