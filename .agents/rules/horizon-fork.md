---
trigger: always_on
---

# Working on Event Horizon: Rules for Agents

This is Event Horizon (`whos-carmen/horizon`), a fork of [horizon-rust/horizon](https://github.com/horizon-rust/horizon)
(Minecraft: Java Edition 26.3 in Rust). It carries what one mod needs from the engine: a Lua runtime for content
mods, mob AI hooks, developer tools and a few game and launcher additions. The mod is
[horizon-parasite-mod](https://github.com/whos-carmen/horizon-parasite-mod), written with
[horizon-modkit](https://github.com/whos-carmen/horizon-modkit).

The person you work for knows modding well but is not a software developer. Say what you did and what it means in
plain words, and ask before anything you can't undo.

The source of this page is `docs/agents.md` on `engine-work`. `tools/fork-sync/install-agent-rules.sh` copies it to
the places Kimi Code and Antigravity read; edit the source, not the copies.

## Upstream's `AGENTS.md`

Upstream's `AGENTS.md` is a map of the code, and its rules on code apply here: never commit Mojang's files or
`.local/`, run every game in scratch `HORIZON_HOME` folders, no hard-coded user-facing strings, Horizon-only
features off by default. Its rules on contributing don't apply: Event Horizon never opens pull requests, issues,
claims or comments on horizon-rust, and the lines about Ludovic's account are not about this person.

## Never

- Push to `upstream` (horizon-rust), or post anything there.
- Push to `origin` without the person's word. Never force-push `engine-work` or `main`.
- Commit on `main`. It mirrors upstream.
- Add a git worktree, or point two checkouts at one `CARGO_TARGET_DIR`. Worktrees once filled 500 GB, and a shared
  build folder ran one tree's compiled code in another.
- Commit a generated mod API file (`mod_api_generated*.rs`, `mod_api_host_generated.rs`). They are build outputs.
- Give an Event Horizon hook an API number. Its hooks are pending fragments in `tools/native-hook-fragments/`.
- Run two builds, or two games on one game folder, at the same time.

## Where work goes

There is one branch, `engine-work`, in one checkout, `~/src/horizon`. Every change is a commit on it, labelled with
its area (`lua: …`, `mob-ai: …`, `dev-tools: …`, `launcher: …`). [What Event Horizon adds](fork-branches.md) says
where each feature's code lives; [the machine setup](dev-setup.md) says where the folders are.

A gameplay feature belongs in a Lua mod, not in Rust. Add engine code only when a mod can't do the job with the
bindings that exist. Keep engine changes small in upstream's files, so upstream merges stay easy
([fork-workflow.md](fork-workflow.md)).

## Build and check

- On macOS, Cargo is Homebrew's keg-only `rustup`: start each shell command with
  `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`.
- The game: `cargo build --profile fast -p event-horizon -p horizon-client-lib`. Mod tooling:
  `cargo build -p cargo-horizon-mod`.
- Before committing: `cargo fmt --all`, then `cargo clippy -p <crate> --tests` and `cargo nextest run -p <crate>`
  for each crate you touched, with `CARGO_INCREMENTAL=0`. Before a push of a large change:
  `tools/fork-sync/verify.sh`.
- A change to the Lua API also needs `cargo run -p xtask -- hooks-doc` and `cargo run -p xtask -- lua-types`; copy
  the new `docs/api/horizon.d.lua` to the modkit's `types/horizon.d.lua`.
- Then check it in the game: `tools/fork-sync/game-checks.sh` runs the modkit's in-game checks one at a time. Write
  your own game drives as bash scripts: zsh does not split a `$CTL` variable into words.
- Check the free disk space (`df -h /`) before a long build. Delete `target/debug` if `target` passes 60 GB.

## Syncing upstream

Only when the person asks. Follow [tools/fork-sync/README.md](../tools/fork-sync/README.md) step by step.

- Resolve each conflict before committing the merge, then run `git diff --check` and check every resolved JSON
  file with `json-dups.py`.
- Two upstream tests fail on macOS on plain upstream `main` too; `verify.sh` lists them apart. Don't chase them.

## Docs

Docs follow the [GNOME developer documentation style](https://developer.gnome.org/documentation/guidelines/devel-docs.html):
short sentences, second person, sentence-case headings, lists and tables instead of long paragraphs, no "just",
"simply" or "easy". Upstream's `README.md` stays as upstream wrote it; Event Horizon's page is `START_HERE.md`.
