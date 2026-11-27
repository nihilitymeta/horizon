//! Tests of the `lua-mod.toml` manifest: every key, cap and refusal named in the format.

use crate::hlua::Refusal;
use crate::manifest::{
    MAX_AUTHORS, MAX_DEPENDENCIES, MAX_ENTRIES, MAX_MANIFEST, MAX_POWERS, MAX_SETTINGS, Manifest,
    Power, Side, valid_id, valid_requirement,
};

fn parse(text: &str) -> Result<Manifest, Refusal> {
    Manifest::parse(text.as_bytes())
}

fn refused(text: &str) -> Refusal {
    match parse(text) {
        Err(refusal) => refusal,
        Ok(m) => panic!("accepted {text:?} as {m:?}"),
    }
}

/// A valid manifest with `extra` lines appended.
fn with(extra: &str) -> String {
    format!("id = \"alpha\"\nversion = \"1.0.0\"\nside = \"both\"\n{extra}")
}

#[test]
fn a_minimal_manifest_takes_its_defaults() {
    let m = parse(&with("")).unwrap();
    assert_eq!(m.id, "alpha");
    assert_eq!(m.name, "alpha");
    assert_eq!(m.version, "1.0.0");
    assert_eq!(m.side, Side::Both);
    assert_eq!(m.entries, vec!["main.luau"]);
    assert!(m.authors.is_empty() && m.dependencies.is_empty());
    assert!(m.powers.is_empty() && m.settings.is_empty() && m.icon.is_none());
}

#[test]
fn a_full_manifest_is_read_in_full() {
    let m = parse(&format!(
        "{}\n",
        with(
            "name = \"Alpha Mod\"\n\
             authors = [\"Ann\", \"Bo\"]\n\
             license = \"MIT\"\n\
             description = \"Line one.\\nLine two.\"\n\
             icon = \"icon.png\"\n\
             entries = [\"main.luau\", \"src/shop.luau\"]\n\
             powers = [\"commands\", \"nbt\"]\n\
             dependencies = { beta = \"^1\", gamma = { version = \">=1.2, <2\", optional = true } }\n\
             [[settings]]\n\
             key = \"volume\"\n\
             type = \"slider\"\n\
             label = \"Volume\"\n\
             min = 0\n\
             max = 10\n\
             step = 1\n\
             default = 3\n"
        )
    ))
    .unwrap();
    assert_eq!(m.name, "Alpha Mod");
    assert_eq!(m.authors, vec!["Ann", "Bo"]);
    assert_eq!(m.license, "MIT");
    assert_eq!(m.description, "Line one.\nLine two.");
    assert_eq!(m.icon.as_deref(), Some("icon.png"));
    assert_eq!(m.entries, vec!["main.luau", "src/shop.luau"]);
    assert_eq!(m.powers, vec![Power::Commands, Power::Nbt]);
    let deps: Vec<(&str, &str, bool)> = m
        .dependencies
        .iter()
        .map(|d| (d.id.as_str(), d.requirement.as_str(), d.optional))
        .collect();
    assert_eq!(
        deps,
        vec![("beta", "^1", false), ("gamma", ">=1.2, <2", true)]
    );
    assert_eq!(m.settings.len(), 1);
    assert_eq!(m.settings[0]["key"].as_str(), Some("volume"));
}

#[test]
fn an_unknown_top_level_key_is_refused_by_name() {
    assert_eq!(
        refused(&with("colour = \"red\"\n")),
        Refusal::UnknownKey {
            key: "colour".into()
        }
    );
}

#[test]
fn assets_is_refused_in_this_version() {
    assert_eq!(refused(&with("assets = []\n")), Refusal::AssetsReserved);
}

#[test]
fn missing_required_keys_are_named() {
    assert_eq!(
        refused("version = \"1\"\nside = \"both\"\n"),
        Refusal::MissingKey { key: "id".into() }
    );
    assert_eq!(
        refused("id = \"alpha\"\nside = \"both\"\n"),
        Refusal::MissingKey {
            key: "version".into()
        }
    );
    assert_eq!(
        refused("id = \"alpha\"\nversion = \"1\"\n"),
        Refusal::MissingKey { key: "side".into() }
    );
}

#[test]
fn a_key_of_the_wrong_type_is_refused() {
    let text = "id = 1\nversion = \"1\"\nside = \"both\"\n";
    assert_eq!(refused(text), Refusal::WrongType { key: "id".into() });
    assert_eq!(
        refused(&with("authors = \"Ann\"\n")),
        Refusal::WrongType {
            key: "authors".into()
        }
    );
}

#[test]
fn the_id_rules_match_the_folder_rules_and_reserve_three_names() {
    assert!(valid_id("a"));
    assert!(valid_id(&"a".repeat(64)));
    assert!(valid_id("mod_2"));
    assert!(!valid_id(""));
    assert!(!valid_id(&"a".repeat(65)));
    assert!(!valid_id("Upper"));
    assert!(!valid_id("dash-id"));
    assert!(!valid_id("dot.id"));
    for bad in ["Bad", "", "dash-id"] {
        let text = format!("id = \"{bad}\"\nversion = \"1\"\nside = \"both\"\n");
        assert_eq!(refused(&text), Refusal::BadId, "{bad:?}");
    }
    for reserved in ["minecraft", "c", "horizon"] {
        let text = format!("id = \"{reserved}\"\nversion = \"1\"\nside = \"both\"\n");
        assert_eq!(refused(&text), Refusal::ReservedId, "{reserved}");
    }
}

#[test]
fn the_version_is_a_semver_string_padded_as_the_saved_worlds_are() {
    assert!(parse("id = \"alpha\"\nversion = \"2\"\nside = \"both\"\n").is_ok());
    assert!(parse("id = \"alpha\"\nversion = \"2.1.0-rc.1\"\nside = \"both\"\n").is_ok());
    for bad in ["", "1.x", "one"] {
        let text = format!("id = \"alpha\"\nversion = \"{bad}\"\nside = \"both\"\n");
        assert_eq!(refused(&text), Refusal::BadVersion, "{bad:?}");
    }
}

#[test]
fn the_side_is_one_of_three_words() {
    let text = "id = \"alpha\"\nversion = \"1\"\nside = \"Both\"\n";
    assert_eq!(refused(text), Refusal::BadSide);
    for (word, side) in [
        ("client", Side::Client),
        ("server", Side::Server),
        ("both", Side::Both),
    ] {
        let text = format!("id = \"alpha\"\nversion = \"1\"\nside = \"{word}\"\n");
        assert_eq!(parse(&text).unwrap().side, side);
    }
}

#[test]
fn text_fields_have_caps_and_refuse_control_characters() {
    let long = "x".repeat(65);
    assert_eq!(
        refused(&with(&format!("name = \"{long}\"\n"))),
        Refusal::TextTooLong {
            field: "name".into(),
            max: 64
        }
    );
    assert_eq!(
        refused(&with("name = \"a\\u0007b\"\n")),
        Refusal::BadText {
            field: "name".into()
        }
    );
    assert_eq!(
        refused(&with(&format!("license = \"{}\"\n", "L".repeat(65)))),
        Refusal::TextTooLong {
            field: "license".into(),
            max: 64
        }
    );
    let authors = format!(
        "authors = [{}]\n",
        vec!["\"a\""; MAX_AUTHORS + 1].join(", ")
    );
    assert_eq!(
        refused(&with(&authors)),
        Refusal::TooMany {
            field: "authors".into(),
            max: MAX_AUTHORS as u64
        }
    );
    assert_eq!(
        refused(&with(&format!("authors = [\"{}\"]\n", "A".repeat(65)))),
        Refusal::TextTooLong {
            field: "authors".into(),
            max: 64
        }
    );
}

#[test]
fn a_description_keeps_line_breaks_and_is_capped() {
    assert!(parse(&with("description = \"a\\n\\tb\"\n")).is_ok());
    assert_eq!(
        refused(&with("description = \"a\\u0007\"\n")),
        Refusal::BadText {
            field: "description".into()
        }
    );
    let long = "d".repeat(1025);
    assert_eq!(
        refused(&with(&format!("description = \"{long}\"\n"))),
        Refusal::TextTooLong {
            field: "description".into(),
            max: 1024
        }
    );
}

#[test]
fn an_icon_must_be_a_relative_png_name() {
    assert!(parse(&with("icon = \"icon.png\"\n")).is_ok());
    for bad in ["icon.jpg", "../icon.png", "/icon.png", "a\\\\icon.png"] {
        assert_eq!(
            refused(&with(&format!("icon = \"{bad}\"\n"))),
            Refusal::BadIcon,
            "{bad}"
        );
    }
}

#[test]
fn dependencies_are_capped_and_their_requirements_checked() {
    let many: Vec<String> = (0..=MAX_DEPENDENCIES)
        .map(|i| format!("dep{i} = \"1\""))
        .collect();
    assert_eq!(
        refused(&with(&format!(
            "dependencies = {{ {} }}\n",
            many.join(", ")
        ))),
        Refusal::TooMany {
            field: "dependencies".into(),
            max: MAX_DEPENDENCIES as u64
        }
    );
    assert_eq!(
        refused(&with("dependencies = { Bad = \"1\" }\n")),
        Refusal::BadDependency { id: "Bad".into() }
    );
    assert_eq!(
        refused(&with("dependencies = { minecraft = \"1\" }\n")),
        Refusal::BadDependency {
            id: "minecraft".into()
        }
    );
    assert_eq!(
        refused(&with("dependencies = { beta = \"*\" }\n")),
        Refusal::BadRequirement { id: "beta".into() }
    );
    assert_eq!(
        refused(&with(
            "dependencies = { beta = { version = \"1\", colour = \"x\" } }\n"
        )),
        Refusal::UnknownKey {
            key: "dependencies.beta.colour".into()
        }
    );
    assert_eq!(
        refused(&with(
            "dependencies = { beta = { version = \"1\", optional = \"yes\" } }\n"
        )),
        Refusal::WrongType {
            key: "dependencies.beta.optional".into()
        }
    );
}

#[test]
fn the_requirement_subset_accepts_the_common_forms_only() {
    for ok in [
        "1",
        "^1",
        "0",
        "=1.2.3",
        ">=1.2, <2",
        "~0.1",
        "<=10.20.30",
        "^0.9",
    ] {
        assert!(valid_requirement(ok), "{ok}");
    }
    let five = ["1"; 5].join(", ");
    for bad in [
        "",
        "*",
        "1.x",
        "01",
        "1.2.3.4",
        ">= 1 >",
        "1.2.3-rc",
        "x",
        "1..2",
        "1.",
        ">>1",
        five.as_str(),
    ] {
        assert!(!valid_requirement(bad), "{bad:?}");
    }
}

#[test]
fn entries_are_relative_luau_paths_listed_once() {
    assert_eq!(refused(&with("entries = []\n")), Refusal::NoEntries);
    assert_eq!(
        refused(&with("entries = [\"a.lua\"]\n")),
        Refusal::BadEntry {
            value: "a.lua".into()
        }
    );
    assert_eq!(
        refused(&with("entries = [\"/abs.luau\"]\n")),
        Refusal::BadEntry {
            value: "/abs.luau".into()
        }
    );
    assert_eq!(
        refused(&with("entries = [\"../x.luau\"]\n")),
        Refusal::BadEntry {
            value: "../x.luau".into()
        }
    );
    assert_eq!(
        refused(&with("entries = [\"main.luau\", \"main.luau\"]\n")),
        Refusal::BadEntry {
            value: "main.luau".into()
        }
    );
    let many = vec!["\"a.luau\""; MAX_ENTRIES + 1].join(", ");
    assert_eq!(
        refused(&with(&format!("entries = [{many}]\n"))),
        Refusal::TooMany {
            field: "entries".into(),
            max: MAX_ENTRIES as u64
        }
    );
}

#[test]
fn powers_are_known_names_listed_once() {
    assert_eq!(
        refused(&with("powers = [\"fly\"]\n")),
        Refusal::UnknownPower { name: "fly".into() }
    );
    assert_eq!(
        refused(&with("powers = [\"nbt\", \"nbt\"]\n")),
        Refusal::DuplicatePower { name: "nbt".into() }
    );
    let many = vec!["\"commands\""; MAX_POWERS + 1].join(", ");
    assert_eq!(
        refused(&with(&format!("powers = [{many}]\n"))),
        Refusal::TooMany {
            field: "powers".into(),
            max: MAX_POWERS as u64
        }
    );
    let all = [
        "commands",
        "nbt",
        "spawn",
        "registration",
        "explosions",
        "world",
        "game_rules",
        "transactions",
        "inventory",
        "network",
        "files",
    ];
    let m = parse(&with(&format!(
        "powers = [{}]\n",
        all.map(|p| format!("\"{p}\"")).join(", ")
    )))
    .unwrap();
    assert_eq!(m.powers.len(), 11);
}

/// `inventory` is a known power: it parses from its name, is listed in rank order after `transactions`
/// and before `network`, and a manifest asking for it holds it in `powers`.
#[test]
fn inventory_is_a_known_power_in_rank_order() {
    assert_eq!(Power::from_name("inventory"), Some(Power::Inventory));
    assert_eq!(Power::Inventory.name(), "inventory");
    assert!(Power::Transactions.rank() < Power::Inventory.rank());
    assert!(Power::Inventory.rank() < Power::Network.rank());
    let m = parse(&with("powers = [\"inventory\"]\n")).unwrap();
    assert_eq!(m.powers, vec![Power::Inventory]);
}

#[test]
fn settings_are_an_array_of_tables_within_caps() {
    assert_eq!(
        refused(&with("settings = \"x\"\n")),
        Refusal::WrongType {
            key: "settings".into()
        }
    );
    assert_eq!(
        refused(&with("settings = [1]\n")),
        Refusal::WrongType {
            key: "settings".into()
        }
    );
    let tables = vec!["{ key = \"a\" }"; MAX_SETTINGS + 1].join(", ");
    assert_eq!(
        refused(&with(&format!("settings = [{tables}]\n"))),
        Refusal::TooMany {
            field: "settings".into(),
            max: MAX_SETTINGS as u64
        }
    );
    assert_eq!(
        refused(&with(&format!(
            "settings = [{{ key = \"{}\" }}]\n",
            "s".repeat(257)
        ))),
        Refusal::TextTooLong {
            field: "settings".into(),
            max: 256
        }
    );
}

#[test]
fn settings_nest_at_most_four_containers_deep() {
    // Depth 4 is within the cap: the schema refuses the unknown field, not the depth.
    assert!(matches!(
        parse(&with("settings = [{ a = { b = { c = 1 } } }]\n")),
        Err(Refusal::Setting(_))
    ));
    // A table at depth 5 is refused.
    assert_eq!(
        refused(&with("settings = [{ a = { b = { c = { d = 1 } } } }]\n")),
        Refusal::SettingsTooDeep
    );
}

#[test]
fn a_manifest_over_the_cap_or_not_utf8_or_not_toml_is_refused() {
    let big = format!("{}# {}\n", with(""), "x".repeat(MAX_MANIFEST as usize));
    assert_eq!(
        Manifest::parse(big.as_bytes()),
        Err(Refusal::ManifestTooLarge)
    );
    assert_eq!(
        Manifest::parse(b"id = \"a\xff\"\n"),
        Err(Refusal::ManifestNotUtf8)
    );
    assert_eq!(
        refused("id = \"alpha\"\nversion\n"),
        Refusal::ManifestNotToml
    );
}

#[test]
fn a_deeply_nested_manifest_is_refused_without_crashing() {
    let nested = format!("x = {}{}\n", "[".repeat(30_000), "]".repeat(30_000));
    assert_eq!(
        Manifest::parse(nested.as_bytes()),
        Err(Refusal::ManifestNotToml)
    );
}

#[test]
fn every_refusal_has_a_translation_key_under_the_lua_mod_prefix() {
    let samples = [
        Refusal::Symlink,
        Refusal::BadId,
        Refusal::UnknownKey { key: "k".into() },
        Refusal::TooMany {
            field: "f".into(),
            max: 1,
        },
        Refusal::AssetsReserved,
    ];
    for refusal in samples {
        assert!(refusal.key().starts_with("lua_mod.refused."), "{refusal:?}");
        assert!(!refusal.reason().is_empty());
    }
    assert_eq!(
        Refusal::TooMany {
            field: "entries".into(),
            max: 16
        }
        .args(),
        vec![("field", "entries".to_string()), ("max", "16".to_string())]
    );
}
