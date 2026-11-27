//! Tests of the `commands` power's line rules (`command_guard.rs`), on the Rust text of the arguments:
//! the default set, the owner's lists, the never list, the level cap and the line checks.

use crate::command_guard::{CommandPolicy, DEFAULT_ALLOW, NEVER_ALLOW, check_text};

/// The policy of mod `alpha` with the owner's list `allow` (`None` for the default).
fn policy(allow: Option<&[&str]>) -> CommandPolicy {
    CommandPolicy {
        owner: "alpha".to_owned(),
        allow: allow.map(|names| names.iter().map(|name| (*name).to_owned()).collect()),
    }
}

/// The result of one call at level `gamemasters`.
fn run(line: &str, allow: Option<&[&str]>) -> Result<(), String> {
    check_text(Some(line), Some("gamemasters"), &policy(allow))
}

/// The default set allows the output and effect commands, and refuses `give` with the owner's hint.
#[test]
fn the_default_set_allows_output_and_effect_commands_and_refuses_give() {
    for name in DEFAULT_ALLOW {
        assert!(run(&format!("{name} x"), None).is_ok(), "{name}");
    }
    assert_eq!(
        run("give @s stone", None).unwrap_err(),
        "horizon: run_command refused: \"give\" is not an allowed command for alpha; the owner may list it under [command_allow]"
    );
}

/// The name is the first token after the trimmed whitespace, one leading slash and a `minecraft:` prefix.
#[test]
fn the_name_is_read_past_whitespace_a_slash_and_the_namespace() {
    for line in [
        "say hi",
        "/say hi",
        "  say hi  ",
        "minecraft:say hi",
        " /minecraft:say hi",
    ] {
        assert!(run(line, None).is_ok(), "{line:?}");
    }
    // A second slash is part of the name, and the name is case-sensitive: neither is `say`.
    assert!(run("//say hi", None).is_err());
    assert!(run("Say hi", None).is_err());
}

/// The never list is refused whatever the owner lists, `*` included, and the refusal names it.
#[test]
fn the_never_list_is_refused_even_when_the_owner_lists_it() {
    for name in NEVER_ALLOW {
        let line = format!("{name} x");
        let refusal = run(&line, Some(&["*", *name][..])).unwrap_err();
        assert_eq!(
            refusal,
            format!("horizon: run_command refused: \"{name}\" cannot be run by a mod"),
            "{name}"
        );
    }
    assert!(run("minecraft:kick steve", Some(&["*"])).is_err());
}

/// An owner's list replaces the default: only what it names runs, `*` allows any name not on the never
/// list, and an empty list allows nothing.
#[test]
fn an_owner_list_replaces_the_default_set() {
    assert!(run("give @s stone", Some(&["say", "give"])).is_ok());
    assert!(run("title @a title hi", Some(&["give"])).is_err());
    assert!(run("give @s stone", Some(&["*"])).is_ok());
    assert!(run("say hi", Some(&[][..])).is_err());
}

/// The level: `all`, `moderators` and `gamemasters` pass; `admins` and `owners` are capped, and any other
/// name is not a level name.
#[test]
fn the_level_is_capped_at_two() {
    for level in ["all", "moderators", "gamemasters"] {
        assert!(
            check_text(Some("say hi"), Some(level), &policy(None)).is_ok(),
            "{level}"
        );
    }
    for level in ["admins", "owners"] {
        assert_eq!(
            check_text(Some("say hi"), Some(level), &policy(None)).unwrap_err(),
            "horizon: run_command refused: permission level is capped at 2",
            "{level}"
        );
    }
    assert!(check_text(Some("say hi"), Some("gamemaster"), &policy(None)).is_err());
    assert!(check_text(Some("say hi"), None, &policy(None)).is_err());
}

/// The line: an empty line or one with only a slash has no command; a newline, a carriage return, a NUL,
/// or more than 1024 characters is not a valid line. 1024 characters is the most that passes.
#[test]
fn the_line_must_name_a_command_and_be_a_plain_short_line() {
    assert_eq!(
        run("", None).unwrap_err(),
        "horizon: run_command refused: no command"
    );
    assert_eq!(
        run("  /  ", None).unwrap_err(),
        "horizon: run_command refused: no command"
    );
    for line in ["say hi\nkick x", "say\rhi", "say\0hi"] {
        assert_eq!(
            run(line, None).unwrap_err(),
            "horizon: run_command refused: the command line is not valid",
            "{line:?}"
        );
    }
    let long = format!("say {}", "a".repeat(1025 - 4));
    assert_eq!(
        run(&long, None).unwrap_err(),
        "horizon: run_command refused: the command line is not valid"
    );
    let fits = format!("say {}", "a".repeat(1024 - 4));
    assert!(run(&fits, None).is_ok());
    assert_eq!(
        check_text(None, Some("gamemasters"), &policy(None)).unwrap_err(),
        "horizon: run_command refused: the command line is not valid"
    );
}

/// Each mod's rule is its own: the owner's list for `beta` does not reach `alpha`, and the refusal names the
/// mod that asked.
#[test]
fn the_guard_honours_the_list_of_its_own_mod_only() {
    let beta_policy = CommandPolicy {
        owner: "beta".to_owned(),
        allow: Some(vec!["give".to_owned()]),
    };
    assert!(check_text(Some("give @s stone"), Some("all"), &beta_policy).is_ok());
    let alpha_refusal = check_text(Some("give @s stone"), Some("all"), &policy(None)).unwrap_err();
    assert!(
        alpha_refusal.ends_with("for alpha; the owner may list it under [command_allow]"),
        "{alpha_refusal}"
    );
}
