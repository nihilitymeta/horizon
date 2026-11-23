//! The format pass: StyLua's library over one script. Nothing is written: a script whose formatted
//! text differs is reported by the caller. The config is the mod folder's `stylua.toml` or
//! `.stylua.toml` when it has one, else StyLua's defaults with Luau syntax.

use std::path::Path;

use stylua_lib::{Config, LuaVersion, OutputVerification, format_code};

/// The config a mod's scripts are checked with. An archive has no folder, so it gets the default.
pub fn config_for(folder: Option<&Path>) -> Result<Config, String> {
    let defaults = || Config {
        syntax: LuaVersion::Luau,
        ..Config::default()
    };
    let Some(folder) = folder else {
        return Ok(defaults());
    };
    for name in ["stylua.toml", ".stylua.toml"] {
        let path = folder.join(name);
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{name}: cannot read it: {error}"))?;
        let mut config: Config =
            toml::from_str(&text).map_err(|error| format!("{name}: {error}"))?;
        let table: toml::Table = text.parse().map_err(|error| format!("{name}: {error}"))?;
        if !table.contains_key("syntax") {
            config.syntax = LuaVersion::Luau;
        }
        return Ok(config);
    }
    Ok(defaults())
}

/// Whether StyLua would change the script's text.
pub fn would_reformat(config: &Config, text: &str) -> Result<bool, String> {
    let formatted = format_code(text, *config, None, OutputVerification::None)
        .map_err(|error| error.to_string())?;
    Ok(formatted != text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luau() -> Config {
        config_for(None).expect("the default config")
    }

    #[test]
    fn a_script_in_stylua_form_would_not_be_reformatted() {
        let text = "local x = 1\nprint(x)\n";
        assert_eq!(would_reformat(&luau(), text), Ok(false));
    }

    #[test]
    fn a_script_in_another_form_would_be_reformatted() {
        let text = "local   x=1\nprint( x )\n";
        assert_eq!(would_reformat(&luau(), text), Ok(true));
    }

    #[test]
    fn a_folder_config_changes_the_result() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("stylua.toml"),
            "indent_type = \"Spaces\"\nindent_width = 2\n",
        )
        .unwrap();
        let config = config_for(Some(root.path())).unwrap();
        // Four spaces is not what the file asks for, so the two-space form is the stable one.
        let text = "if true then\n  print(1)\nend\n";
        assert_eq!(would_reformat(&config, text), Ok(false));
    }

    #[test]
    fn a_broken_config_is_an_error_naming_the_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("stylua.toml"), "column_width = \"wide\"\n").unwrap();
        let error = config_for(Some(root.path())).unwrap_err();
        assert!(error.starts_with("stylua.toml"), "{error}");
    }
}
