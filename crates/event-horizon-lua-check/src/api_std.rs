//! The mod API's globals as selene sees them, read from the shipped type definitions
//! (`emit::TYPES`). Each `declare <name>: <Type>` line is a global. When `<Type>` is an object type
//! (`export type <Type> = { ... }`), its member names are the global's fields, each of any value:
//! selene then reports `horizon.on_ticks` as a field the API does not have. Arguments are not
//! checked; an overloaded member would give false positives. A type that is not an object (or not
//! found) makes the global `any`, as before.

use selene_lib::standard_library::{Field, FieldKind, StandardLibrary};

pub fn from_types(types: &str) -> StandardLibrary {
    let text = strip_comments(types);
    let mut lib = StandardLibrary::default();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("declare ") else {
            continue;
        };
        let Some((name, ty)) = rest.split_once(':') else {
            continue;
        };
        let (name, ty) = (name.trim(), ty.trim());
        if !is_ident(name) || !is_ident(ty) {
            continue;
        }
        match object_members(&text, ty) {
            Some(members) => {
                // The struct's key carries the type's name; selene only needs it to be unique.
                let key = format!("{name} ({ty})");
                lib.structs.insert(
                    key.clone(),
                    members
                        .into_iter()
                        .map(|member| (member, Field::from_field_kind(FieldKind::Any)))
                        .collect(),
                );
                lib.globals.insert(
                    name.to_owned(),
                    Field::from_field_kind(FieldKind::Struct(key)),
                );
            }
            None => {
                lib.globals
                    .insert(name.to_owned(), Field::from_field_kind(FieldKind::Any));
            }
        }
    }
    lib
}

/// The member names of `export type <ty> = { ... }`, or `None` when the type is not declared as an
/// object. Members are the entries at the body's top level (split at `,` and newlines) that start
/// with `name:`; a continuation line (`| "x"`) is not a member.
fn object_members(text: &str, ty: &str) -> Option<Vec<String>> {
    let head = format!("type {ty} = {{");
    let start = text.find(&head)? + head.len();
    let mut depth = 0usize;
    let mut body_end = None;
    for (index, ch) in text[start..].char_indices() {
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' if depth == 0 => {
                body_end = Some(start + index);
                break;
            }
            '}' | ')' | ']' => depth -= 1,
            _ => {}
        }
    }
    let body = &text[start..body_end?];
    let mut members = Vec::new();
    let mut entry = String::new();
    let mut depth = 0usize;
    for ch in body.chars().chain(std::iter::once(',')) {
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && (ch == ',' || ch == '\n') {
            if let Some(name) = member_name(&entry) {
                members.push(name);
            }
            entry.clear();
        } else {
            entry.push(ch);
        }
    }
    Some(members)
}

fn member_name(entry: &str) -> Option<String> {
    let entry = entry.trim_start();
    let name: String = entry
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    let after = entry[name.len()..].trim_start();
    (is_ident(&name) && after.starts_with(':')).then_some(name)
}

/// Drops `--` comments to the end of their line. The shipped file has no `--` inside a string.
fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_ident(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::TYPES;

    /// The field names of a global that is an object type, sorted.
    fn fields_of(lib: &StandardLibrary, global: &str) -> Vec<String> {
        let Some(Field {
            field_kind: FieldKind::Struct(key),
            ..
        }) = lib.globals.get(global)
        else {
            panic!("`{global}` is not a struct global");
        };
        lib.structs[key].keys().cloned().collect()
    }

    const SNIPPET: &str = "\
export type Inner = { a: number }
export type Api = {
	-- a comment with a , and a { brace
	on: ((name: string) -> ()) & ((name: string, f: () -> ()) -> ()),
	block: (record: { id: string, template: string }) -> (),
	setting: (key: string) -> boolean | number | string,
	inner: Inner,
	kind: \"client\"
		| \"common\",
}
declare api: Api
declare plain: number
";

    #[test]
    fn a_synthetic_object_type_gives_its_members_and_other_declares_are_any() {
        let lib = from_types(SNIPPET);
        assert_eq!(
            fields_of(&lib, "api"),
            ["block", "inner", "kind", "on", "setting"]
        );
        assert_eq!(lib.globals["plain"].field_kind, FieldKind::Any);
    }

    #[test]
    fn the_shipped_api_has_its_members_and_the_declared_global() {
        let lib = from_types(TYPES);
        let members = fields_of(&lib, "horizon");
        assert!(members.len() >= 20, "{members:?}");
        for name in [
            "on",
            "block",
            "item",
            "setting",
            "log",
            "none",
            "attachment_get",
        ] {
            assert!(members.iter().any(|m| m == name), "missing {name}");
        }
        assert!(!members.iter().any(|m| m == "on_ticks"));
    }
}
