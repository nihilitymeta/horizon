#!/usr/bin/env bash
# Rebuilds engine-work: main, the docs commit, then every integrated branch merged --no-ff in table order.
# rerere replays the known resolutions (the graphics.rs serialize() string); any other conflict stops it.
# After each merge both lockfiles must pass --locked. Run in the engine-work checkout.
#   tools/fork-sync/rebuild-engine-work.sh <docs commit>
set -u
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
docs=${1:?the docs commit (git log -1 --format=%H origin/engine-work -- START_HERE.md)}
ORDER="mod-registry anti-lag websocket extras-keybinds menus debug-overlay launcher mob-ai-hooks lua-bindings egui-tools hot-reload recipe-book"
git checkout -q -B engine-work main && git cherry-pick "$docs" >/dev/null || { echo "cherry-pick of $docs failed"; exit 1; }
git log --oneline -2 | cat
for b in $ORDER; do
  note=""
  if ! git merge --no-ff -q --no-edit "$b" >/dev/null 2>&1; then
    if [ -n "$(git diff --name-only --diff-filter=U)" ]; then echo "CONFLICT merging $b:"; git diff --name-only --diff-filter=U; exit 2; fi
    git commit -q --no-edit; note=" (rerere resolved it)"
  fi
  (cd launcher && cargo metadata --locked --offline --format-version 1 >/dev/null 2>&1) || note="$note; launcher/Cargo.lock STALE: fix it on $b"
  cargo metadata --locked --offline --format-version 1 >/dev/null 2>&1 || note="$note; Cargo.lock STALE: fix it on $b"
  echo "merged $b$note"
done
for b in main $ORDER; do git merge-base --is-ancestor "$b" engine-work || echo "NOT an ancestor: $b"; done
echo "engine-work $(git rev-parse --short HEAD)"
