#!/usr/bin/env bash
# Before a sync: writes every fork branch's tip to <file> and tags it pre-sync-<date>/<branch>, turns on
# rerere. The file's SHAs are the --force-with-lease values for the push after the sync.
#   tools/fork-sync/record-tips.sh [file]     # default ~/src/horizon-pre-sync-tips-<date>.txt
set -eu
# A second sync on one day gets 2026-11-04b, then c...: never move an earlier sync's tags.
date=$(date +%F)
for s in "" b c d e f g h; do git rev-parse -q --verify "refs/tags/pre-sync-$date$s/main" >/dev/null || break; done
date=$date$s
out=${1:-$HOME/src/horizon-pre-sync-tips-$date.txt}
git config rerere.enabled true
git config rerere.autoupdate true
: > "$out"
for b in main engine-work websocket debug-overlay hot-reload recipe-book anti-lag launcher \
         mod-registry extras-keybinds menus mob-ai-hooks lua-bindings egui-tools; do
  sha=$(git rev-parse "$b") && echo "$b $sha" >> "$out" && git tag -f "pre-sync-$date/$b" "$sha" >/dev/null
done
cat "$out"
