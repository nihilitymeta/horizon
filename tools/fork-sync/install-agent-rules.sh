#!/usr/bin/env bash
# Writes docs/agents.md to the files Kimi Code and Antigravity read, beside upstream's own AGENTS.md:
#   .kimi-code/AGENTS.md            Kimi Code's project instructions
#   .agents/rules/horizon-fork.md   an always-on Antigravity workspace rule
# Both are committed, so every clone has them. Run it after docs/agents.md changes, then commit the two files.
#   tools/fork-sync/install-agent-rules.sh
set -eu
cd "$(git rev-parse --show-toplevel)"
mkdir -p .kimi-code .agents/rules
cp docs/agents.md .kimi-code/AGENTS.md
{ printf -- '---\ntrigger: always_on\n---\n\n'; cat docs/agents.md; } > .agents/rules/horizon-fork.md
echo "wrote .kimi-code/AGENTS.md and .agents/rules/horizon-fork.md"
