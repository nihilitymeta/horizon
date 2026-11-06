# Sync Helpers

Scripts for bringing upstream into Event Horizon ([fork-workflow.md](../../docs/fork-workflow.md#syncing-upstream))
and checking the result. Run them in the one checkout, on `engine-work`
([setting up a machine](../../docs/dev-setup.md)).

| Script | What it does |
|---|---|
| `merge-upstream.sh` | Tags `engine-work` as `pre-sync-<date>`, moves `main` to upstream, and merges it into `engine-work`. Settles lockfile and generated-file conflicts itself, and lists the rest |
| `keep-both.py <file>...` | Resolves every conflict in the files by keeping both sides, ours first |
| `json-both.py <file>...` | The same for JSON (lang files, `native-hooks.json`), fixing the comma between the sides. Fails on invalid JSON, and on a duplicate key, which means two objects were joined and need splitting by hand |
| `api-docs-both.py docs/native-mods.md` | Resolves conflicts in the "API N adds" list: our side, plus the paragraphs upstream added |
| `json-dups.py <file>...` | Prints duplicate keys, which a JSON parser accepts silently |
| `verify.sh [summary]` | Clippy (`--tests`), fmt and the whole test suite for the game, then the launcher's clippy and suite. Lists upstream's known macOS failures apart from real ones |
| `game-checks.sh [modkit]` | The modkit's checks against the built game, one at a time, in a scratch game folder |
| `metal-launch.sh [png]` | Opens the built game in a window, prints frame stats and takes a screenshot (macOS) |
| `install-agent-rules.sh` | Copies `docs/agents.md` to `.kimi-code/AGENTS.md` and `.agents/rules/horizon-fork.md`, the files Kimi Code and Antigravity read. Run it after `docs/agents.md` changes, and commit both |

## A sync, in order

1. Merge:

   ```sh
   cd ~/src/horizon && git checkout engine-work
   tools/fork-sync/merge-upstream.sh
   ```

2. If it lists files, resolve each one, then check that no marker or broken JSON is left:

   ```sh
   git diff --check
   python3 tools/fork-sync/json-dups.py <each JSON file you resolved>
   git commit --no-edit
   ```

3. Update the **Upstream base** line in `START_HERE.md`, and commit it.
4. Check, on an otherwise quiet machine:

   ```sh
   tools/fork-sync/verify.sh
   cargo build --profile fast -p event-horizon -p horizon-client-lib && cargo build -p cargo-horizon-mod
   tools/fork-sync/game-checks.sh
   tools/fork-sync/metal-launch.sh          # macOS
   ```

5. Push: `git push origin main engine-work`. A merge doesn't rewrite history, so no force is needed.

If the merge goes badly, `git merge --abort` puts `engine-work` back as it was. After a merge commit,
`git reset --hard pre-sync-<date>` does the same.

## What to expect

- **Conflicts** come from files that both sides change: the render settings (`graphics.rs`, `options_ui.rs`,
  `extras.rs`), the lists everyone adds to (`native-hooks.json`, lang files, `docs/native-mods.md`) and the Lua
  runtime's content side (`content.rs`; the runtime itself lives in `event-horizon-lua`). Their number grows with
  how much upstream changed in those files, not with the number of upstream commits.
- **Breakage without a conflict** happens too: upstream adds a field, a helper of the same name or a language, and
  Event Horizon's code no longer builds. `verify.sh` finds these.
- **Upstream's known failures on macOS:** `drops_match_vanillas_motion_bit_for_bit` (a one-bit float difference on
  Apple Silicon) and `the_log_is_fresh_per_attempt_and_keeps_early_failures` (fails only inside the whole launcher
  suite). Both fail on plain upstream `main`.
- **A flaky game check:** `example-check.sh` sometimes misses the nest growing, which waits on random block ticks.
  Run it again before you look for a fault.
- **Time:** `verify.sh` takes about 6 minutes on an M2 Max, most of it upstream's slow mob tests. Building the game
  and running the game checks take about 15 more.

## Disk

- The prepared vanilla data lives once, in `~/src/horizon-resources/26.3`. The checkout's
  `.local/horizon-client/26.3` is a link to it, and the checks take it as `RESOURCES`. Without the link, every
  test that needs vanilla data is skipped, and nothing says so.
- The build folder holds `target/fast` (the game) and `target/debug` (the checks), about 35 GB together. Delete
  `target/debug` if `target` passes 60 GB.
- The launcher keeps the build you chose and the newest nightly. Remove old local builds with
  `horizon-launcher remove <version>`.
