//! One line of the checker's output: a finding in a script, or about the whole mod.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

impl fmt::Display for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Level::Error => formatter.write_str("error"),
            Level::Warning => formatter.write_str("warning"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The script's name in the mod, as its manifest lists it.
    pub file: String,
    /// 1-based, or 0 for a finding about the whole file.
    pub line: usize,
    /// 1-based, or 0 for a finding about the whole file.
    pub column: usize,
    pub level: Level,
    pub text: String,
}

impl Finding {
    /// A finding at a place in the file: `file:line:col: level: text`.
    pub fn at(file: &str, line: usize, column: usize, level: Level, text: String) -> Finding {
        Finding {
            file: file.to_owned(),
            line,
            column,
            level,
            text,
        }
    }

    /// A finding about the whole file: `file: level: text`.
    pub fn whole(file: &str, level: Level, text: String) -> Finding {
        Finding::at(file, 0, 0, level, text)
    }

    pub fn render(&self) -> String {
        if self.line == 0 {
            format!("{}: {}: {}", self.file, self.level, self.text)
        } else {
            format!(
                "{}:{}:{}: {}: {}",
                self.file, self.line, self.column, self.level, self.text
            )
        }
    }
}

/// The 1-based line and column of a byte offset in `text` (the column counts bytes). An offset past
/// the end is the end.
pub fn line_col(text: &str, byte: usize) -> (usize, usize) {
    let before = &text.as_bytes()[..byte.min(text.len())];
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |index| index + 1);
    (line, before.len() - start + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_are_one_based() {
        let text = "local a = 1\nprint(a)\n";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 6), (1, 7));
        assert_eq!(line_col(text, 12), (2, 1));
        assert_eq!(line_col(text, 18), (2, 7));
    }

    #[test]
    fn an_offset_past_the_end_is_the_end() {
        assert_eq!(line_col("ab", 99), (1, 3));
    }

    #[test]
    fn a_finding_renders_with_its_place_or_without_one() {
        let at = Finding::at("main.luau", 3, 5, Level::Error, "boom".to_owned());
        assert_eq!(at.render(), "main.luau:3:5: error: boom");
        let whole = Finding::whole("main.luau", Level::Warning, "would reformat".to_owned());
        assert_eq!(whole.render(), "main.luau: warning: would reformat");
    }
}
