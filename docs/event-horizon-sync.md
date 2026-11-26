# Taking a new upstream version into Event Horizon

Event Horizon is a fork of the upstream project (`horizon-rust/horizon`). It keeps its own crates
(the `event-horizon*` crates, listed in `tools/layers.json`) and makes a small number of edits to
files that upstream owns. A sync brings upstream's newer commits into a branch of this fork, keeps
those edits, regenerates what is generated from the mod API, and runs the checks.

Run the steps in order. Each command below exists in this tree; the script or crate it names is
the authority if the text and the code disagree.

## 1. What is pinned, and how to find it

No file records the upstream commit the fork is built on. The pin is the upstream commit that the
branch last merged, and git can give it back:

```sh
git fetch --no-tags --filter=blob:none <upstream URL> +refs/heads/main:refs/sync/upstream-main
git merge-base HEAD refs/sync/upstream-main
```

The upstream URL is the default in `tools/merge_base.py` (`HORIZON_UPSTREAM_URL` overrides it). The
second command prints the newest upstream commit that the branch contains. Before a sync it is the
pin; after a sync it is the new pin.

`tools/merge_base.py` defines a sync the same way: a merge in the branch whose second parent is a
commit of upstream main that the merge base does not have. The newest such parent is the sync's
upstream commit, and `sync_provenance` checks that it is an ancestor of upstream main. Full history
is required for that check, so `git rev-parse --is-shallow-repository` must print `false`.

A native mod names the revision it builds against in the `[native]` section of its
`horizon-mod.toml`: `game`, `horizon` and the exact `mixins` commit
([native-mod-source-revisions.md](mod-api/native-mod-source-revisions.md)). That names the mod's
target, not the branch's pin.

## 2. Before you merge

1. Start from a clean tree on the branch you sync into. Make the sync on a branch of its own:
   `git switch -c sync-<name> <target branch>`.
2. List the fork's edits to files that upstream owns, so the merge can carry them over:
   ```sh
   git diff --name-only <pin> HEAD | grep -v '^crates/event-horizon'
   ```
   Each file in that list is a place where upstream's new version can conflict with the fork's
   edit. Keep the list; the sync's commit message and its PR body name them.
3. Read the fork's commits on those files: `git log --oneline <pin>..HEAD -- <file>`.

## 3. Merge upstream

```sh
git merge --no-ff refs/sync/upstream-main
```

The merge's first parent is the fork and its second parent is upstream. For each conflict:

- **Upstream moved code the fork edited** (a large hunk whose side from the fork is the old code):
  take upstream's side, then port the fork's edit into the new place. Compare the fork's edit with
  its base, `git diff <base> <fork commit> -- <file>`, and apply it to upstream's copy by hand.
  Expect compile errors where the fork's enum matches miss upstream's new variants, and where
  upstream's new tests build structs without the fork's fields.
- **Upstream changed a line the fork also changed, in place**: keep both where the fork's change is
  still needed, and drop the fork's line only if upstream made the same change.
- **Language and JSON files that both sides changed**: this tree has no union scripts. Merge them by
  hand, then run the checks in section 6.

Record each conflict's disposition in `docs/sync/<name>.md`, one line per file, as
`docs/sync/c09.md` does: what upstream changed, what the fork kept, and what was left out and why.

After the merge, find the upstream changes it did not carry into the fork:

```sh
python3 tools/sync/dropped_upstream.py <merge commit> --markdown
```

The tool lists candidates, one row per upstream commit, and it can over-report when the fork
rewrote or moved code. Sort each commit by hand (`git show <commit>`, `git grep` at the merge) into
carried, rightly left out by the fork, or lost. Re-apply the lost ones. Its header says it compares
per file and by line, so a change that only removes or reorders lines can be missed.

## 4. Regenerate the Lua bindings

The Lua host's bindings are generated from the mod API. Regenerate them after any sync, and after any
change to a hook declaration.

1. **Export the surface.** `surface.json` is the mod API's export of the tree. Write it outside the
   tree, because it is not committed:
   ```sh
   cargo run -p horizon-mod-api-gen --bin horizon-stage -- surface . <scratch dir>/surface.json
   ```
   `horizon-stage surface ROOT OUT` reads the source tree at `ROOT` and writes only the export.
2. **If a hook was declared or changed**, fill the placeholder bodies first
   ([adding-a-hook.md](mod-api/adding-a-hook.md)):
   ```sh
   cargo run -p xtask -- mod-api
   ```
3. **Get the hooks trait.** The trait is `mod_api_generated.rs`, written by the build script of
   `horizon-mod-vocab` into Cargo's `OUT_DIR`:
   ```sh
   cargo build -p horizon-mod-vocab
   find target -path '*/horizon-mod-vocab-*/out/mod_api_generated.rs'
   ```
   Use the file from the build you just made. If the search prints more than one path, use the most
   recently modified one.
4. **Generate.**
   ```sh
   cargo run -p event-horizon-lua-gen --release -- --surface <scratch dir>/surface.json --hooks-trait <path from step 3> --out crates/event-horizon-lua/src/generated --types crates/event-horizon-lua-convert/src/generated_types.rs
   ```
   Without `--lua-types`, the generator also writes `horizon.d.luau` and `.luaurc.example` to
   `crates/event-horizon-lua/lua-types`. Pass `--lua-types DIR` to choose another folder.
   The generated files are `hooks.rs`, `hooks_impl.rs`, `handles.rs`, `powers.rs` and their coverage
   files (`hooks_coverage.txt`, `handles_coverage.txt`) under `crates/event-horizon-lua/src/generated`,
   plus `generated_types.rs`.
5. **Check that nothing is stale.** Run the same command with `--check` added. Exit 0 means every file
   matches what would be written; exit 1 names a file that differs; exit 2 is bad input or arguments.
   Regenerate and commit the files when it exits 1.

## 5. Contract hashes

Every record of `surface.json` has a `hash`: the first 128 bits, as 32 hex digits, of the SHA-256 of
the record's contract. The contract leaves out `id`, `hash`, `doc`, `wasm_reason`, `paths`, and a few
wasm and fit-test fields; the rule is in the header of
[crates/horizon-mod-surface/src/lib.rs](../crates/horizon-mod-surface/src/lib.rs) (`# Hashes`).
A record whose hash changed has a changed contract. The generated `handles_coverage.txt` header names
the surface hash it was made from, so a stale file is visible.

To see what changed in the mod API since the pin:

1. Export the surface at the pin, in a separate worktree:
   ```sh
   git worktree add <scratch dir>/pin <pin>
   cargo run -p horizon-mod-api-gen --bin horizon-stage -- surface <scratch dir>/pin <scratch dir>/old.json
   ```
2. Compare it with the new export:
   ```sh
   cargo run -p horizon-mod-api-gen --bin horizon-stage -- surface-diff <scratch dir>/old.json <scratch dir>/surface.json --notes crates/horizon-mod-vocab/changes --base-notes <scratch dir>/pin/crates/horizon-mod-vocab/changes --gate
   ```
   The diff prints breaking changes first. `--notes` attaches the change notes that are new on the
   branch; `--gate` exits 1 when a breaking change has no note. A new note that names an id that did not
   change also makes it exit 1. Exit 2 means an input could not be read.
3. Remove the worktree when done: `git worktree remove <scratch dir>/pin`.

`tools/check_mod_api_wire.py` checks the other side of the same contract: the `Wire` declarations in
the generator's templates. It reads source only and needs no build.

## 6. Checks

Run these before the sync is pushed, and again after any fix. Each one proves a different thing.

| Command | What it proves |
|---|---|
| `python3 tools/pr_ready.py --base <target branch>` | rustfmt, clippy with warnings denied, the tests of the crates the branch touches, the Python tests and the public-tree check. Add `--quick` to skip the vanilla harnesses; `--harnesses` runs the harnesses the branch affects, which need the game data in `.local/`. |
| `python3 tools/check_layers.py --base <target branch>` | each crate depends only on the layers its own layer may use (`tools/layers.json`), and no exception was added since the base. |
| `python3 tools/check_addresses.py --base <target branch>` | every built-in outside address is where `docs/layers.md` says it may be (`tools/addresses.json`), and the list did not grow. |
| `python3 tools/check_public_tree.py --range <target branch>..HEAD` | no game jar, class, asset, decompiled source or `.local` file is in the commits being added. Run without arguments to check the tree as it is. |
| `python3 tools/check_mod_api_wire.py` | every `Wire` declaration in the API generator's templates is one the rule allows. |
| `python3 tools/check_mod_vocabulary_state.py` | every process-global declaration in `horizon-mod-vocab` has a written reason. A stale ownership page fails it; `--write-doc` regenerates the page. |
| `python3 -m unittest discover -s tests` | the Python unit tests. `pr_ready.py` runs them too. |

Read the failures before you change anything. A check that fails on a line the sync did not touch
is a finding about the branch as it was, and the sync report should say so.

## 7. Finish the merge

1. Run the Lua generation in section 4 with `--check` and confirm exit 0.
2. Write the merge commit message with the new pin (`git rev-parse refs/sync/upstream-main`), the
   files in the list from section 2, and the link to `docs/sync/<name>.md`.
3. Put the `dropped_upstream.py --markdown` table and the surface diff (section 5) in the PR body.
4. Land the sync branch with a merge commit, not a squash or a rebase. The upstream parent must stay in
   the history, because `tools/merge_base.py` reads it to find the sync on the next run.
