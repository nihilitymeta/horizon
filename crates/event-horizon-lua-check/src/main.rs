//! `event-horizon-lua-check`: the checker a Lua mod author runs on a mod.
//!
//! - `check <path>` loads a mod folder or `.hlua` archive headless through the Lua host, lints its
//!   scripts with selene and checks their format with StyLua. Nothing is written.
//! - `--list` (or `list`) prints the Lua mods a game folder and dev paths give: loaded, refused
//!   (with its translation key and reason) or disabled.
//! - `emit-types <dir>` writes `horizon.d.luau` and a `.luaurc` for `luau-lsp`.
//!
//! Exit codes: 0 fine; 1 a finding (a refused load, a syntax or selene error, or a warning under
//! `--strict`); 2 a usage error or a path that cannot be read.

mod check;
mod emit;
mod finding;
mod format;
mod lint;
mod list;
mod load;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  event-horizon-lua-check check <path> [--strict]
  event-horizon-lua-check --list [--game-dir DIR] [--dev PATH]...
  event-horizon-lua-check list [--game-dir DIR] [--dev PATH]...
  event-horizon-lua-check emit-types <dir> [--force]

  check <path>      a mod folder or a .hlua archive: load it, lint and format-check its scripts
  --list            the Lua mods the game folder (default: .) and the dev paths give
  emit-types <dir>  writes horizon.d.luau and .luaurc into <dir>
  --strict          a warning (a lint warning, would reformat) fails the check too
  --force           emit-types: overwrite files whose content differs
";

/// One parsed command line.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Check {
        path: PathBuf,
        strict: bool,
    },
    List {
        game_dir: PathBuf,
        dev: Vec<PathBuf>,
    },
    Emit {
        dir: PathBuf,
        force: bool,
    },
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Command::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Check { path, strict }) => check::run(&path, strict),
        Ok(Command::List { game_dir, dev }) => list::run(&game_dir, &dev),
        Ok(Command::Emit { dir, force }) => emit::run(&dir, force),
        Err(message) => {
            eprintln!("event-horizon-lua-check: {message}");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Parses the arguments after the program name. Flags are checked by hand: the tool has three
/// commands and no dependency on an argument parser.
fn parse(args: &[String]) -> Result<Command, String> {
    let Some((command, rest)) = args.split_first() else {
        return Err("no command given".to_owned());
    };
    match command.as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "check" => {
            let mut path = None;
            let mut strict = false;
            for arg in rest {
                match arg.as_str() {
                    "--strict" => strict = true,
                    flag if flag.starts_with('-') => {
                        return Err(format!("unknown option `{flag}`"));
                    }
                    value if path.is_none() => path = Some(PathBuf::from(value)),
                    _ => return Err("check takes one path".to_owned()),
                }
            }
            let path = path.ok_or("check needs a mod folder or a .hlua archive")?;
            Ok(Command::Check { path, strict })
        }
        "--list" | "list" => {
            let mut game_dir = PathBuf::from(".");
            let mut dev = Vec::new();
            let mut args = rest.iter();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--game-dir" => {
                        let value = args.next().ok_or("--game-dir needs a folder")?;
                        game_dir = PathBuf::from(value);
                    }
                    "--dev" => {
                        let value = args.next().ok_or("--dev needs a path")?;
                        dev.push(PathBuf::from(value));
                    }
                    other => return Err(format!("unexpected `{other}` for --list")),
                }
            }
            Ok(Command::List { game_dir, dev })
        }
        "emit-types" => {
            let mut dir = None;
            let mut force = false;
            for arg in rest {
                match arg.as_str() {
                    "--force" => force = true,
                    flag if flag.starts_with('-') => {
                        return Err(format!("unknown option `{flag}`"));
                    }
                    value if dir.is_none() => dir = Some(PathBuf::from(value)),
                    _ => return Err("emit-types takes one folder".to_owned()),
                }
            }
            let dir = dir.ok_or("emit-types needs a folder")?;
            Ok(Command::Emit { dir, force })
        }
        other => Err(format!("unknown command `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn check_takes_a_path_and_the_strict_flag() {
        assert_eq!(
            parse(&args(&["check", "mod", "--strict"])),
            Ok(Command::Check {
                path: PathBuf::from("mod"),
                strict: true
            })
        );
    }

    #[test]
    fn list_defaults_to_the_current_folder_and_takes_repeated_dev_paths() {
        assert_eq!(
            parse(&args(&["--list", "--dev", "a", "--dev", "b"])),
            Ok(Command::List {
                game_dir: PathBuf::from("."),
                dev: vec![PathBuf::from("a"), PathBuf::from("b")]
            })
        );
    }

    #[test]
    fn list_and_the_list_subcommand_are_the_same_command() {
        assert_eq!(
            parse(&args(&["list", "--game-dir", "g"])),
            parse(&args(&["--list", "--game-dir", "g"]))
        );
    }

    #[test]
    fn a_missing_or_unknown_argument_is_a_usage_error() {
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["check"])).is_err());
        assert!(parse(&args(&["check", "a", "b"])).is_err());
        assert!(parse(&args(&["check", "a", "--loud"])).is_err());
        assert!(parse(&args(&["--list", "--game-dir"])).is_err());
        assert!(parse(&args(&["fly"])).is_err());
    }

    #[test]
    fn emit_types_takes_a_folder_and_force() {
        assert_eq!(
            parse(&args(&["emit-types", "out", "--force"])),
            Ok(Command::Emit {
                dir: PathBuf::from("out"),
                force: true
            })
        );
    }
}
