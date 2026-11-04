#!/usr/bin/env bash
# Brings upstream into Event Horizon: tags engine-work as it was, moves main to upstream, merges upstream into
# engine-work. Settles the mechanical conflicts itself (a lockfile: upstream's, then Cargo adds ours; a generated
# mod API file: removed, it is a build output) and lists the rest. Run it in the checkout, on engine-work.
#   tools/fork-sync/merge-upstream.sh
# After it stops: resolve the files it lists (keep-both.py, json-both.py, api-docs-both.py or by hand), check
# them with json-dups.py and `git diff --check`, then `git commit --no-edit`.
set -u
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
GEN='crates/horizon-client/src/mod_api/generated.rs|crates/horizon-client/src/mod_api/generated_host.rs|crates/horizon-client/src/game/mod_api_host_generated.rs'
[ "$(git rev-parse --abbrev-ref HEAD)" = engine-work ] || { echo "check out engine-work first"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "the checkout has changes: commit or stash them"; exit 1; }
git config rerere.enabled true
git fetch -q upstream && git fetch -q origin || exit 1
# A second sync on one day gets <date>b, then c...: an earlier sync's tag never moves.
date=$(date +%F)
for s in "" b c d e f g h; do git rev-parse -q --verify "refs/tags/pre-sync-$date$s" >/dev/null || break; done
git tag "pre-sync-$date$s" engine-work
echo "engine-work was $(git rev-parse --short engine-work): tag pre-sync-$date$s"
git branch -f main upstream/main
echo "main is upstream's $(git rev-parse --short main), $(git rev-list --count engine-work..main) commits new to engine-work"
if git merge --no-edit -m "Merge upstream $(git rev-parse --short main) into Event Horizon" main >/dev/null 2>&1; then
  echo "merged with no conflicts"
else
  for f in $(git diff --name-only --diff-filter=U | grep -E '(^|/)Cargo.lock$'); do
    git checkout --theirs "$f" && (cd "$(dirname "$f")" && cargo metadata --format-version 1 >/dev/null 2>&1) \
      && git add "$f" && echo "lockfile: upstream's, plus ours: $f"
  done
  for f in $(git diff --name-only --diff-filter=U | grep -E "^($GEN)$"); do git rm -q "$f" && echo "removed generated $f"; done
  left=$(git diff --name-only --diff-filter=U)
  if [ -n "$left" ]; then echo "resolve these, then git commit --no-edit:"; echo "$left"; exit 2; fi
  git commit -q --no-edit && echo "merged; the conflicts were mechanical"
fi
for f in $(git ls-files | grep -E "^($GEN)$"); do echo "a generated file is tracked, remove it: $f"; done
(cd launcher && cargo metadata --locked --offline --format-version 1 >/dev/null 2>&1) || echo "launcher/Cargo.lock is stale: cd launcher && cargo metadata"
cargo metadata --locked --offline --format-version 1 >/dev/null 2>&1 || echo "Cargo.lock is stale: cargo metadata"
echo "next: update START_HERE.md's Upstream base line, then verify.sh and game-checks.sh"
