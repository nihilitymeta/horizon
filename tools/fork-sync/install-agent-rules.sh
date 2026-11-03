#!/usr/bin/env bash
# Copies docs/agents.md (on engine-work) to the files Kimi Code and Antigravity read in this checkout:
#   .kimi-code/AGENTS.md            Kimi Code's project instructions
#   .agents/rules/horizon-fork.md   an always-on Antigravity workspace rule
# The copies are listed in .git/info/exclude, never committed, so they stay on every branch the checkout switches
# to. Upstream's own AGENTS.md at the root is left alone; both tools read it too. Run it once on a new machine and
# again after docs/agents.md changes.
#   tools/fork-sync/install-agent-rules.sh
set -eu
cd "$(git rev-parse --show-toplevel)"
src=$(git show engine-work:docs/agents.md)
mkdir -p .kimi-code .agents/rules
printf '%s\n' "$src" > .kimi-code/AGENTS.md
printf -- '---\ntrigger: always_on\n---\n\n%s\n' "$src" > .agents/rules/horizon-fork.md
ex=$(git rev-parse --git-path info/exclude)
for p in /.kimi-code/AGENTS.md /.agents/rules/horizon-fork.md; do grep -qxF "$p" "$ex" 2>/dev/null || echo "$p" >> "$ex"; done
echo "installed .kimi-code/AGENTS.md and .agents/rules/horizon-fork.md"
