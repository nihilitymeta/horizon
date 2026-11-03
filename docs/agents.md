# Working on the Fork: Rules for Agents

This is `whos-carmen/horizon`, a private fork of [horizon-rust/horizon](https://github.com/horizon-rust/horizon)
(Minecraft: Java Edition 26.3 in Rust). It carries a Lua runtime for content mods, mob AI hooks, developer tools and
graphics work, for one mod: [horizon-parasite-mod](https://github.com/whos-carmen/horizon-parasite-mod), written with
[horizon-modkit](https://github.com/whos-carmen/horizon-modkit).

The person you work for knows modding well but is not a software developer. Say what you did and what it means in
plain words, and ask before anything you can't undo.

The source of this page is `docs/agents.md` on `engine-work`. `tools/fork-sync/install-agent-rules.sh` copies it to
the places Kimi Code and Antigravity read; edit the source, not the copies.

## Upstream's `AGENTS.md`

Upstream's `AGENTS.md` is a map of the code, and its rules on code apply here: never commit Mojang's files or
`.local/`, run every game in scratch `HORIZON_HOME` folders, no hard-coded user-facing strings, Horizon-only
features off by default. Its rules on contributing don't apply: this fork never opens pull requests, issues,
claims or comments on horizon-rust, and the lines about Ludovic's account are not about this person.

## Never

- Push to `upstream` (horizon-rust), or post anything there.
- Push to `origin` without the person's word. Push with `--force-with-lease=<branch>:<sha you recorded>`.
- Commit on `main`. It mirrors upstream.
- Fix something in `engine-work`. It is rebuilt from the branches; fix the branch that owns the code.
- Add a git worktree, or point two checkouts at one `CARGO_TARGET_DIR`. Worktrees once filled 500 GB, and a shared
  build folder ran one branch's compiled code in another.
- Commit a generated mod API file (`mod_api_generated*.rs`, `mod_api_host_generated.rs`). They are build outputs.
- Give a fork hook an API number. Fork hooks are pending fragments in `tools/native-hook-fragments/`.
- Run two builds, or two games on one game folder, at the same time.

## Where work goes

Read [the machine setup](dev-setup.md) for the folders, and [the branches](fork-branches.md) for what each one holds.
There is one checkout, `~/src/horizon`; switch branches in it with `git checkout`.

| The change | The branch |
|---|---|
| a Lua binding or hook, the Lua runtime | `lua-bindings` |
| `/inspect`, `/goals`, `/nav`, `/lua` and other dev tools | `egui-tools` |
| mob AI hook points, mobs from content data | `mob-ai-hooks` |
| content mods reloading | `hot-reload` |
| anything else | the branch in START_HERE.md's table that owns it, or a new `main` + one concern branch |

`lua-bindings` is built on `mob-ai-hooks`, and `egui-tools` on `lua-bindings`; `extras-keybinds` is built on
`mod-registry`, and `menus` on `extras-keybinds`. After you change a base branch, rebase the branches above it with
`git rebase --onto <base> <old base tip> <branch>`, then rebuild `engine-work` with
`tools/fork-sync/rebuild-engine-work.sh`.

A gameplay feature belongs in a Lua mod, not in Rust. Add engine code only when a mod can't do the job with the
bindings that exist.

## Build and check

- On macOS, Cargo is Homebrew's keg-only `rustup`: start each shell command with
  `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`.
- The game: `cargo build --profile fast -p horizon-client -p horizon-client-lib`. Mod tooling:
  `cargo build -p cargo-horizon-mod`.
- Before committing on a branch: `cargo fmt --all`, then `cargo clippy -p <crate> --tests` and
  `cargo nextest run -p <crate>` for each crate you touched, with `CARGO_INCREMENTAL=0`.
- A change to the Lua API also needs `cargo run -p xtask -- hooks-doc` and `cargo run -p xtask -- lua-types` on
  `lua-bindings`; copy the new `docs/api/horizon.d.lua` to the modkit's `types/horizon.d.lua`.
- Then check it in the game. The modkit's `tools/check.sh`, `tools/example-check.sh` and `tools/hook-coverage.sh`
  drive a game through `horizon-ctl`; run them with `HORIZON=~/src/horizon`,
  `RESOURCES=~/src/horizon-resources/26.3` and `GAME_DIR=<scratch folder>`. Write game drives as bash scripts:
  zsh does not split a `$CTL` variable into words.
- Check the free disk space (`df -h /`) before a long build. Delete `target/debug` if `target` passes 60 GB.

## Syncing upstream

Only when the person asks. Follow [tools/fork-sync/README.md](../tools/fork-sync/README.md) step by step. Two
things go wrong easily:

- Resolve a conflict before running `step.sh` or `cont.sh`. Check every resolved JSON file with `json-dups.py`.
- After the rebases, no commit may add a conflict marker:
  `git log --oneline -G'^(<<<<<<<|>>>>>>>) ' main..<branch>` prints nothing for each branch.

## Docs

Docs follow the [GNOME developer documentation style](https://developer.gnome.org/documentation/guidelines/devel-docs.html):
short sentences, second person, sentence-case headings, lists and tables instead of long paragraphs, no "just",
"simply" or "easy". READMEs stay short; detail goes in `docs/`.
