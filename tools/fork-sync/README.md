# Fork sync helpers

Scripts for an upstream sync ([docs/fork-workflow.md](../../docs/fork-workflow.md#syncing-upstream)), in the one
checkout ([setting up a machine](../../docs/dev-setup.md)). They switch branches under themselves, so copy them out
of the checkout first (step 1 below) and run the copies.

| Script | What it does |
|---|---|
| `record-tips.sh [file]` | Writes every branch's tip to a file and tags it `pre-sync-<date>/<branch>` (`<date>b`, `<date>c`... for a second sync on one day); turns rerere on. The file's SHAs are the push's `--force-with-lease` values and the old tips for `--onto` |
| `step.sh` | Continues a rebase until a conflict needs a person. Refuses a file that still has conflict markers; drops generated mod API files; refreshes a conflicted `Cargo.lock` with cargo; skips commits that became empty |
| `cont.sh` | Runs `step.sh` until the rebase is done or stops; run it after you resolve each stop |
| `keep-both.py <file>...` | Resolves every conflict in the files by keeping both sides, HEAD's first |
| `json-both.py <file>...` | The same for JSON (lang files, `native-hooks.json`), fixing the comma between the sides; fails on invalid JSON, and on a duplicate key, which means two objects were joined and need splitting by hand |
| `api-docs-both.py docs/native-mods.md` | Resolves conflicts in the "API N adds" list: HEAD's side, plus the commit's paragraphs HEAD lacks (compared without their numbers) |
| `json-dups.py <file>...` | Prints duplicate keys, which a JSON parser accepts silently |
| `rebuild-engine-work.sh <docs commit>` | Rebuilds `engine-work` from `main`, the docs commit and the merges in table order, checking both lockfiles after each merge |
| `verify.sh <summary> [--full\|--touched\|--check-only] <branch>...` | Checks out each branch in turn: clippy (`--tests`) and fmt, then the whole suite (`--full`), the branch's own tests (`--touched`) or none (`--check-only`); a line per branch with its time. One build folder, kept |
| `touched-tests.py` | Turns a branch's changed files into a nextest filter for the tests in those files (`--touched`) |
| `install-agent-rules.sh` | Copies `docs/agents.md` to `.kimi-code/AGENTS.md` and `.agents/rules/horizon-fork.md` (Kimi Code, Antigravity), git-ignored so they stay on every branch. Not a sync step: once per machine, and after `docs/agents.md` changes |
| `metal-launch.sh [png]` | Opens the built game in a window, prints frame stats, takes a screenshot (wakes the display first) |

## A sync, in order

1. Copy the scripts out, record the tips, move `main`:

   ```sh
   cd ~/src/horizon && git checkout engine-work && git fetch upstream && git fetch origin
   T=~/src/horizon-sync-tools; rm -rf $T; mkdir -p $T
   git archive engine-work tools/fork-sync | tar -x -C $T --strip-components 2
   $T/record-tips.sh            # prints the tips file it wrote
   git branch -f main upstream/main
   ```

2. Rebase each branch, the single ones with `git rebase main <branch>`, the stacked ones with
   `git rebase --onto <new base> <old base tip> <branch>`, taking the old tip from the tips file:
   `mod-registry`, then `extras-keybinds` onto it, then `menus`; `mob-ai-hooks`, then `lua-bindings`, then
   `egui-tools`. When git stops on a conflict, resolve the files (the helpers above, or by hand), then run
   `$T/cont.sh`. Never run `cont.sh` or `step.sh` before resolving: they stop, but only because they check.
3. Check that no commit kept a marker: for each branch,
   `git log --oneline -G'^(<<<<<<<|>>>>>>>) ' main..<branch>` prints nothing.
4. Rebuild `engine-work` first (step 5), then check every branch, in the background, on an otherwise quiet
   machine:

   ```sh
   $T/verify.sh ~/sync-summary.txt --touched websocket debug-overlay hot-reload recipe-book anti-lag \
     launcher --check-only mod-registry extras-keybinds mob-ai-hooks lua-bindings --full menus egui-tools engine-work
   ```

   | Mode | Branches | What runs |
   |---|---|---|
   | `--full` | `engine-work`, the stack tips `menus` and `egui-tools` | clippy, fmt, the whole test suite |
   | `--touched` | the single branches | clippy and fmt (they build the whole game), the tests in the Rust files the branch changes |
   | `--check-only` | the stack bases | clippy and fmt; the tips' suites run their code |

   `engine-work` holds every branch, so its suite catches a branch breaking a module it didn't touch. A failure
   there that no branch shows: check the branches whose files the failing test reads, in `--full` mode.
5. Update START_HERE.md's **Upstream base** line in the docs commit (cherry-pick it onto the new `main` and amend),
   then run `$T/rebuild-engine-work.sh <new docs commit>`. Do this before step 4, which checks `engine-work`.
6. Build `engine-work` (`cargo build --profile fast -p horizon-client -p horizon-client-lib`, and
   `cargo build -p cargo-horizon-mod`), then in the modkit run `tools/check.sh --self-test`, `tools/example-check.sh`
   and `tools/hook-coverage.sh` with `HORIZON`, `RESOURCES` and `GAME_DIR` set, and `metal-launch.sh` on a Mac.
7. Push: `main`, then each branch with `--force-with-lease=<branch>:<recorded sha>`.

## What makes a sync slow

- Conflicts, not commits: a sync costs what the fork's files and upstream's changes share. Most of the work is in
  the Lua stack (`lua-bindings`, `egui-tools`), the hot render files (`graphics.rs`, `options_ui.rs`, `extras.rs`)
  and the lists everyone appends to (`native-hooks.json`, lang files, `docs/native-mods.md`).
- Other builds at the same time: they double every branch's check. Build and run the game after the queue.
- Upstream's slowest tests set a floor no runner beats: the foal pathing test takes about 150 s alone, three more
  mob tests 70-110 s.
- `CARGO_INCREMENTAL=0` in `verify.sh`: incremental caches only help small edits, and grew to 300 GB across
  worktrees.
- `cargo-nextest` runs the suite about 12% faster and skips building the empty doctests.

## Why one checkout

Measured on the Mac (M2 Max), 2026-11-04:

| Setup | 13 branches checked | Disk |
|---|---|---|
| a worktree per branch, its build folder kept | — | grew to ~500 GB over a few syncs |
| a worktree per branch, its build folder deleted after its check | about 35 min | small, every branch built cold |
| one checkout, one kept build folder | VERIFY_TIME | VERIFY_DISK |

- A shared `CARGO_TARGET_DIR` between worktrees is unsafe: Cargo's freshness is by file time, so one branch reused
  another's compiled `horizon-client-core`. One checkout is safe: git rewrites the files a branch changes, with new
  times, and Cargo rebuilds them.
- sccache only hits when the source and build-folder paths are the same, so it shares nothing between worktrees.

## Disk

- The prepared vanilla data lives once, in `~/src/horizon-resources/26.3`. The checkout's
  `.local/horizon-client/26.3` is a link to it (git ignores `.local`), and the modkit's checks take it as
  `RESOURCES`. Without the link every test that needs vanilla data is skipped, silently.
- The launcher keeps the selected local build and the newest nightly; old local builds go with
  `horizon-launcher remove <version>` (their prepared resources are clones and free little).
