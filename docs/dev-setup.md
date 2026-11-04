# Setting Up a Machine

This page sets up a machine for Event Horizon, the modkit and Lua mods, the same way on each computer. Follow it on a new
machine, or to bring an old one in line. [Building](building.md) lists the compilers and libraries; this page says
where everything goes and how the builds are kept small.

## The folders

Everything lives in one folder, `~/src` here. Use the same names on each machine, so commands and notes work
anywhere.

| Folder | What it holds |
|---|---|
| `horizon` | Event Horizon: **one** checkout, on `engine-work` |
| `horizon-resources/26.3` | the prepared vanilla data, once for every build and check |
| `horizon-modkit` | the modkit |
| `horizon-parasite-mod` | the mod |

Don't add git worktrees. Each one builds the whole workspace into its own build folder, and a dozen of them once
filled 500 GB.

Never point two checkouts at one build folder (`CARGO_TARGET_DIR`): Cargo decides what to rebuild by file times,
and one checkout then runs another's compiled code.

## 1. Install the tools

Install what [building](building.md) lists for your system, then:

- `cargo install cargo-nextest`, or `brew install cargo-nextest`: the checks run the tests with it
- [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp/releases), on your `PATH`: it checks Lua scripts
- on macOS with Homebrew's `rustup`, which is keg-only: add `export PATH=/opt/homebrew/opt/rustup/bin:$PATH` to
  `~/.zshrc`

## 2. Clone the repositories

```sh
mkdir -p ~/src && cd ~/src
git clone https://github.com/whos-carmen/horizon.git
git clone https://github.com/whos-carmen/horizon-modkit.git
git clone https://github.com/whos-carmen/horizon-parasite-mod.git
cd horizon
git remote add upstream https://github.com/horizon-rust/horizon.git
git fetch upstream
git config rerere.enabled true
git checkout engine-work
```

## 3. Prepare the vanilla data

Install the [Horizon launcher](https://playhorizon.dev), sign in and press Play once: it prepares the data from
your copy of the game. Copy the result to `~/src/horizon-resources/26.3`, and link the checkout to it:

```sh
mkdir -p ~/src/horizon-resources
cp -R "<launcher data>/versions/<version>/.local/horizon-client/26.3" ~/src/horizon-resources/
cd ~/src/horizon
mkdir -p .local/horizon-client && ln -sfn ~/src/horizon-resources/26.3 .local/horizon-client/26.3
```

The launcher's data is `~/Library/Application Support/horizon-launcher` on macOS, `~/.local/share/horizon-launcher`
on Linux and `%APPDATA%\horizon-launcher` on Windows. Without the link, every test that needs vanilla data is
skipped, and nothing says so.

## 4. Build the game

```sh
cd ~/src/horizon
cargo build --profile fast -p horizon-client -p horizon-client-lib
cargo build -p cargo-horizon-mod
```

This makes `target/fast/horizon-client` (the game), `target/fast/horizon-ctl` (drives a running game for the
in-game checks) and `target/debug/cargo-horizon-mod` (makes, checks and packs mods). The modkit's scripts find all
three through `HORIZON=~/src/horizon`. The first build takes a while; later ones rebuild only what changed.

To play the build from the launcher, install it as a local version: `tools/deploy_local_mac.sh` on macOS. On
Linux, use the launcher's Versions page, **Import a game build…**, and pick `target/fast/horizon-client`.

## 5. Write mods

A Lua mod needs no engine build. Keep the build you have while you write mods, and run it with the modkit and the
mod:

```sh
~/src/horizon/target/fast/horizon-client --game-dir ~/horizon-dev --resources ~/src/horizon-resources/26.3 \
  --world "New World" --dev-tools --dev-mod ~/src/horizon-modkit --dev-mod ~/src/horizon-parasite-mod
```

- `/reload` in the game runs the scripts again. Only a change to `scripts/registry.lua` needs a restart.
- `~/src/horizon/target/debug/cargo-horizon-mod horizon-mod check <mod> --game ~/src/horizon/target/fast/horizon-client --dev-mod ~/src/horizon-modkit`
  loads the scripts without a world, in about a second.
- luau-lsp in your editor, or `luau-lsp analyze`, finds mistakes before the game does.
- `HORIZON=~/src/horizon RESOURCES=~/src/horizon-resources/26.3 GAME_DIR=~/horizon-dev tools/check.sh` in the
  modkit checks the modkit, the example mod and the probes.

Rebuild the engine only when a mod needs something the engine doesn't have yet, such as a new hook. A sync's checks
build into `target/debug` only, so the game in `target/fast` stays as it was until you build it again.

The modkit's [getting started](https://github.com/whos-carmen/horizon-modkit/blob/main/docs/getting-started.md)
walks through a first mod.

## 6. Coding agents

Kimi Code and Antigravity read an `AGENTS.md` at the top of the modkit and of the mod. Event Horizon's root
`AGENTS.md` is upstream's, so its own rules ([agents.md](agents.md)) are also in `.kimi-code/AGENTS.md` and
`.agents/rules/horizon-fork.md`, where those tools look. A clone has them already. After you change
`docs/agents.md`, run `tools/fork-sync/install-agent-rules.sh` to update both copies, and commit them.

Claude Code reads its own memory and `CLAUDE.md`, not these files.

## Keep the builds small

The checkout has two build folders, and nothing else uses disk:

| Folder | What builds it | About |
|---|---|---|
| `target/fast` | the game, `cargo build --profile fast` | 4 GB |
| `target/debug` | the checks and tests | 30 GB |

- The checks build with `CARGO_INCREMENTAL=0`. The incremental cache speeds up small edits only, and grows without
  limit.
- If `target` grows past 60 GB, delete `target/debug`. The next check builds it again in a few minutes.
- The launcher keeps the build you chose and the newest nightly. Remove old local builds with
  `horizon-launcher remove <version>`.

## Sync upstream

A sync merges upstream's changes into `engine-work`. Sync when upstream has something you want, not on a schedule;
mod work doesn't need it. [Working on Event Horizon](fork-workflow.md#syncing-upstream) has the steps, and
[tools/fork-sync](../tools/fork-sync/README.md) the scripts that do them.
