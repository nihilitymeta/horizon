# Setting Up a Machine

This page sets up a machine for the fork, the modkit and Lua mods, the same way on each computer. Follow it on a new
machine, or to bring an old one in line. [Building](building.md) lists the compilers and libraries; this page says
where everything goes and how the builds are kept small.

## The folders

Everything lives in one folder, `~/src` here. Use the same names on each machine, so commands and notes work
anywhere.

| Folder | What it holds |
|---|---|
| `horizon` | the fork: **one** checkout, normally on `engine-work` |
| `horizon-resources/26.3` | the prepared vanilla data, once for every build and check |
| `horizon-modkit` | the modkit |
| `horizon-parasite-mod` | the mod |
| `horizon-sync-tools` | a copy of `tools/fork-sync`, made at the start of a sync |

Don't make a worktree for each branch. Each worktree builds the whole workspace into its own build folder, and a
dozen of them filled 500 GB. One checkout switches branches instead, and Cargo rebuilds only what a branch changes.

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
for b in mod-registry anti-lag websocket extras-keybinds menus debug-overlay launcher \
         mob-ai-hooks lua-bindings egui-tools hot-reload recipe-book; do
  git branch --track "$b" "origin/$b"
done
git checkout engine-work
```

The local branches are only needed to sync upstream; to build and play, `engine-work` is enough.

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

To play the build from the launcher, install it as a local version: `tools/deploy_local_mac.sh` on macOS,
`tools/deploy_test_launcher.sh` on Linux.

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
build into `target/debug` only, so the game in `target/fast` stays as it was until you build `engine-work` again.

The modkit's [getting started](https://github.com/whos-carmen/horizon-modkit/blob/main/docs/getting-started.md)
walks through a first mod.

## 6. Give coding agents the rules

The modkit and the mod each have an `AGENTS.md` at the top, which Kimi Code and Antigravity read. The fork's root
`AGENTS.md` is upstream's, so the fork's own rules ([agents.md](agents.md)) are installed beside it, in files git
ignores so they stay on every branch:

```sh
cd ~/src/horizon && git checkout engine-work && tools/fork-sync/install-agent-rules.sh
```

Run it again after `docs/agents.md` changes. Claude Code reads its own memory and `CLAUDE.md`, not these files.

## Keep the builds small

The checkout has two build folders, and nothing else uses disk:

| Folder | What builds it | About |
|---|---|---|
| `target/fast` | the game, `cargo build --profile fast` | 4 GB |
| `target/debug` | the checks and tests | SIZE_DEBUG |

- The checks build with `CARGO_INCREMENTAL=0`. The incremental cache speeds up small edits only, and grows without
  limit across branches.
- If `target` grows past 60 GB, delete `target/debug`. The next check builds it again in a few minutes.
- The launcher keeps the build you chose and the newest nightly. Remove old local builds with
  `horizon-launcher remove <version>`.

## Sync upstream

Syncing brings upstream's changes into the fork: `main` moves to upstream, every branch is rebased onto it, and
`engine-work` is rebuilt. Sync when upstream has something you want, not on a schedule. Mod work doesn't need it.
[Working on the fork](fork-workflow.md#syncing-upstream) has the steps, and
[tools/fork-sync](../tools/fork-sync/README.md) the scripts that do them.
