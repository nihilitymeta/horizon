#!/usr/bin/env bash
# Continues the rebase in the current worktree until a conflict needs a person, then stops with the
# files left to resolve (exit 2). On the way it:
# - drops a generated mod API file a commit brings back (they are build outputs of horizon-mod-api-gen);
# - takes main's side of a conflicted Cargo.lock and lets cargo add the branch's dependencies;
# - skips a commit that became empty (upstream already has the change).
# Exit 0 when the rebase is done. Run it after resolving each stop.
set -u
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
GEN='crates/horizon-client/src/mod_api/generated.rs|crates/horizon-client/src/mod_api/generated_host.rs|crates/horizon-client/src/game/mod_api_host_generated.rs'
state() { git rev-parse --git-path rebase-merge; }
markers() { for f in $(git diff --name-only --diff-filter=U); do grep -lE '^(<<<<<<<|>>>>>>>) ' "$f"; done; }
while :; do
  # git add marks a file resolved: never with conflict markers still in it.
  m=$(markers); if [ -n "$m" ]; then echo "UNRESOLVED (conflict markers):"; echo "$m"; exit 2; fi
  git add -A
  if [ -d "$(state)" ] && [ -z "$(git diff --cached --name-only)" ] && [ -z "$(git diff --name-only --diff-filter=U)" ] \
     && git rev-parse -q --verify REBASE_HEAD >/dev/null; then
    echo "SKIP empty: $(git log -1 --format='%h %s' REBASE_HEAD)"; out=$(git rebase --skip 2>&1)
  else
    out=$(GIT_EDITOR=true git rebase --continue 2>&1)
  fi
  # A quiet rebase (-q) does not say "Successfully rebased": its state folder going away is the sign.
  if echo "$out" | grep -q 'Successfully rebased' || [ ! -d "$(state)" ]; then echo "DONE $(git rev-parse --short HEAD)"; exit 0; fi
  if ! echo "$out" | grep -q 'Could not apply\|CONFLICT' && [ ! -d "$(state)" ]; then echo "$out" | tail -5; exit 1; fi
  for f in $(git diff --name-only --diff-filter=U | grep -E "^($GEN)$"); do git rm -q "$f"; done
  for f in $(git ls-files | grep -E "^($GEN)$"); do git rm -q "$f"; done
  for f in $(git diff --name-only --diff-filter=U | grep -E '(^|/)Cargo.lock$'); do
    git checkout --ours "$f" && (cd "$(dirname "$f")" && cargo metadata --format-version 1 >/dev/null 2>&1) \
      && git add "$f" && echo "LOCK refreshed $f"
  done
  left=$(git diff --name-only --diff-filter=U)
  if [ -n "$left" ]; then
    echo "STOP at $(cat "$(state)/msgnum")/$(cat "$(state)/end"): $(echo "$out" | grep 'Could not apply' | cut -c1-120)"
    echo "$left"; exit 2
  fi
done
