# Working on the fork: a feature branch, syncing upstream, rebuilding `engine-work`

## Building a feature

The target is: your branch rebases cleanly onto `main`, passes the repo's own self-check, and merges into the rest of `engine-work` without conflicts. Do these in order; stop at the first failure and fix it on the branch.

### 1. Branch from `main`, one concern

```sh
git fetch upstream
git checkout main && git merge --ff-only upstream/main   # see Syncing upstream
git checkout -b <feature> main
```

- One branch, one concern. A fix the feature needs rides with it; an unrelated fix gets its own branch.
- Do not branch from or depend on another feature branch. If you truly need code from another branch, branch from it, say so in your START_HERE.md row, and list yours after it; rebase it with `git rebase --onto <base-branch> <old-base> <feature>` when the base moves. `extras-keybinds` on `mod-registry`, `menus` on `extras-keybinds`, `lua-bindings` on `mob-ai-hooks` and `egui-tools` on `lua-bindings` are the cases today; keep it rare. Before rebasing a base branch, note the tip of each branch above it: `git merge-base` is the wrong point once the base has moved.
- Never commit on `main`; never push to `upstream` (horizon-rust). `origin` is the fork.
- Follow the upstream rules in [AGENTS.md](AGENTS.md): scratch `HORIZON_HOME` for every game you run, nothing from `.local/` in git, Horizon-only features off by default where they change the vanilla look.

### 2. Keep it mergeable while you write it

Every integrated branch edits the same hot files: `gfx.rs`, `graphics*.rs`, `lod.rs`, `game.rs`, `options.rs`, `debug_keys.rs`, `launcher/src/main.rs`. Conflicts between features almost always start there.

- Prefer a new module (`gfx/<feature>.rs`, `game/<feature>.rs`) with a few one-line hooks in the shared files over editing the shared files in place.
- Keep hooks additive and at the end of a list, block or match arm where you can; do not reorder, reformat or rename existing code you are not changing.
- Everything must compile on Linux (Vulkan) and macOS (Metal). Backend-specific code (`VK_*` extensions, HDR metadata, Anti-Lag) is behind runtime capability checks or `#[cfg]` and is inert elsewhere; request wgpu features backend-neutrally through `gfx.rs`.
- Small, labelled commits (`feat(render): …`, `fix(launcher): …`): they rebase and bisect; one giant commit does not.
- Patches to `third_party/` crates are a last resort and get their own commit with the upstream PR or issue in the message.
- A feature with an on/off switch (a native hook mod, an engine patch) declares it in one file, `crates/horizon-client-render/registry/<id>.json` (format: `registry/README.md`): the setting line that switches it, its kind and its texts. The file is inert on its own and never collides with another branch's; on `engine-work` the `mod-registry` branch lists it in Options > Mods and in the launcher's Mods page. Never hard-code a feature's switch in the launcher.
- Work in the one checkout ([setting up a machine](dev-setup.md)): switch branches with `git checkout`, and Cargo rebuilds what the branch changes. Don't add a worktree per branch, and never share one `CARGO_TARGET_DIR` between two checkouts: Cargo judges freshness by file time, so one tree runs another's compiled code.

### 3. Self-check on the branch

```sh
cargo check -p horizon-client                 # game workspace
python3 tools/pr_ready.py --quick             # fmt, clippy -D warnings, touched crates' tests
(cd launcher && cargo check)                  # only if you touched launcher/ (separate workspace)
cargo build --profile fast -p horizon-client  # then run it once, in a scratch HORIZON_HOME, and exercise the feature
tools/deploy_local_mac.sh                     # macOS: install the tree as a local version of the launcher (Linux: tools/deploy_test_launcher.sh)
```

The deploy scripts mark the version with its branch and commit (`.local_build`), which the launcher shows as `Local · <branch> · <commit>` so builds from different branches stay apart.

With the vanilla data in `.local/`, run `python3 tools/pr_ready.py` (no `--quick`) so the parity harnesses your files affect also run. The fork leaves upstream's `README.md` alone, so its README tests pass too. If your feature adds a dependency to a crate the launcher also builds (`horizon-protocol`, `horizon-mods`, `horizon-theme`…), refresh `launcher/Cargo.lock` too (`cd launcher && cargo metadata >/dev/null`) and commit it with the feature, or every `--locked` launcher check fails. Then rebase and push:

```sh
git rebase main
git push --force-with-lease origin <feature>
```

### 4. Trial build: does it fit `engine-work`?

Compatibility means the real build recipe succeeds with your branch appended. Run it on a scratch branch:

```sh
git checkout -B engine-work-trial main
git cherry-pick $(git log -1 --format=%H origin/engine-work -- START_HERE.md)
for b in mod-registry anti-lag websocket extras-keybinds menus debug-overlay launcher mob-ai-hooks lua-bindings egui-tools hot-reload recipe-book <feature>; do
  git merge --no-ff --no-edit "$b" || { echo "conflict merging $b"; break; }
done
cargo check -p horizon-client && python3 tools/pr_ready.py --quick
(cd launcher && cargo check)
cargo build --profile fast -p horizon-client   # run it: every feature on at once
```

- **A conflict is a defect of your branch**, not of the build: abort (`git merge --abort`), go back to the feature branch, move the change so the merge is clean (step 2), and try again. Do not resolve conflicts in `engine-work` and leave the branches overlapping.
- The one allowed exception is upstream's single-line settings serializer, the `serialize()` format string in `crates/horizon-client-render/src/graphics.rs`: every branch that adds a graphics setting edits that line, so two of them always collide there. Resolve it in the merge commit by keeping every key, in the same order as the arguments below it, and check nothing else conflicted.
- After every merge, `(cd launcher && cargo metadata --locked >/dev/null)` must pass. A branch that adds dependencies to a crate the launcher builds (`horizon-protocol`, `horizon-mods`…) carries its own `launcher/Cargo.lock` refresh; fix it on the branch, not in the merge.
- Build on the machine you have; the other platform is checked when `engine-work` is rebuilt there. If your feature is backend-specific, also run the trial build with your feature inert (the other backend, or the feature disabled) to confirm nothing else changed.
- `cargo check` passing is not enough: launch the trial binary and watch the features you touched plus one neighbour (F3 overlay, menus, a LAN session over `ws://` if you touched networking).
- Delete the scratch branch when done: `git branch -D engine-work-trial`.

### 5. Add it to the build list

Integration is a docs edit: add the branch to START_HERE.md's table, in the position you merged it in step 4, with a one-line description, and a paragraph to [fork-branches.md](fork-branches.md); amend the docs commit (one commit on top of `main`, cherry-picked into every rebuild). Then rebuild `engine-work` (below) and push it. Anything not in the table is not in the build, whatever its branch history says.

## Syncing upstream

The scripts in [tools/fork-sync/](../tools/fork-sync/README.md) do the mechanical parts: recording the tips, stepping through a rebase, resolving lang-file conflicts, rebuilding `engine-work`, checking every branch and opening the game. Its README has the whole sync in order, for the one checkout.

`main` is a clean mirror of [horizon-rust/horizon](https://github.com/horizon-rust/horizon). We take upstream fixes and game-version updates wholesale through it — no per-version patch stacks. Feature branches are rebased onto the new `main`, and `engine-work` is rebuilt from them, so history stays linear per feature and conflicts only show up in files we actually touched.

When: whenever upstream has something the fork wants, such as a fix, a new hook or a game update; mod work never needs a sync. Conflicts grow with how much upstream changed in the fork's files, not with the number of commits, so a sync after a quiet week is cheap and one after a big upstream refactor is not. Update START_HERE.md's **Upstream base** line in the docs commit at each sync.

### 1. Fast-forward `main`

```sh
git checkout main
git fetch upstream
git merge --ff-only upstream/main
git push origin main
```

If `--ff-only` fails, `main` has local commits — it should not; investigate before forcing anything.

### 2. Rebase every feature branch

```sh
git checkout <branch>          # each integrated branch, hot-reload
git rebase main
git push --force-with-lease origin <branch>
```

Resolve conflicts in place (ours cluster in `gfx.rs`, `graphics*.rs`, `lod.rs`, `game.rs`, launcher). Prefer keeping upstream’s structure and re-applying our delta when both sides changed the same logic. Re-run `cargo check -p horizon-client` before pushing if conflicts were non-trivial.

- Rebase only — never merge `main` into a feature branch.
- Stacked branches (`extras-keybinds` on `mod-registry`, `menus` on `extras-keybinds`; `lua-bindings` on `mob-ai-hooks`, `egui-tools` on `lua-bindings`): record each branch's tip before rebasing anything (`git rev-parse`), rebase the base onto `main`, then each branch above with `git rebase --onto <new base> <old tip of the base>`. `git merge-base` is not the right point once the base moved.
- Hook API numbers: upstream stamps its own (`@API:name@` placeholders, `tools/stamp_native_hooks.py`), so the fork never picks a number. A fork hook or helper lives in `tools/native-hook-fragments/<name>.json`, as a pending hook does upstream, and its doc comment says `API @API:<name>@`; `NATIVE_HOOKS_API` and `docs/native-mods.md`'s list stay upstream's. Upstream's stamper never runs on the fork, so these stay pending: nothing in the fork reads the number (Lua and the modkit call hooks by name).
- Turn on `git config rerere.enabled true`: a resolution comes back by itself when the same conflict reappears in a later commit or in the `engine-work` merges.
- The typed mod API (`mod_api_generated*.rs`, `mod_api_host_generated.rs`) is a build output of `horizon-mod-api-gen` since upstream eaa7da4fb, read from `native-hooks.json` and `tools/native-hook-fragments/`: a branch never carries a generated file. When a rebase brings one back, delete it. Regenerate the fork's checked-in references at the branch's tip (`cargo run -p xtask -- hooks-doc` and `lua-types` on `lua-bindings`), and refresh a conflicted `Cargo.lock` by taking `main`'s and running `cargo metadata`.
- `--force-with-lease` (not `--force`): refuses the push if someone else moved the remote branch.
- If a rebase gets messy, abort, get that branch clean on its own, then continue with the rest.
- `hot-reload` rebases onto `main` directly (it does not depend on engine features).

### 3. Rebuild `engine-work`

`engine-work` is not rebased; it is thrown away and rebuilt from `main` plus the integrated table, so it never accumulates its own fixes. Keep the current docs commit, then merge the list in table order:

```sh
git checkout engine-work
git checkout -B engine-work main
git cherry-pick $(git log -1 --format=%H origin/engine-work -- START_HERE.md)   # the docs commit: START_HERE.md, docs/, tools/fork-sync/
for b in mod-registry anti-lag websocket extras-keybinds menus debug-overlay launcher mob-ai-hooks lua-bindings egui-tools hot-reload recipe-book; do
  git merge --no-ff --no-edit "$b" || exit 1
done
cargo check -p horizon-client && python3 tools/pr_ready.py --quick && (cd launcher && cargo check)
git push --force-with-lease origin engine-work
```

A conflict here means two feature branches overlap: fix it on one of the feature branches (section 2 above), not in `engine-work`; the `graphics.rs` serializer line is the one exception (step 4 above).
