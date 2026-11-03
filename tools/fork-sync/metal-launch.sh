#!/usr/bin/env bash
# Opens the built game in a window on this Mac, reads its frame stats, takes a screenshot, and quits:
# the "it renders" check after a sync. Wakes the display first (a sleeping display never presents a
# frame, and the screenshot waits forever).
#   tools/fork-sync/metal-launch.sh [screenshot.png]     # RESOURCES defaults to ~/src/horizon-resources/26.3
set -u
H=$(cd "$(dirname "$0")/../.." && pwd)
SHOT=${1:-$(mktemp -d)/metal.png}
GD=${GAME_DIR:-$HOME/Library/Application Support/horizon}
RESOURCES=${RESOURCES:-$HOME/src/horizon-resources/26.3}
export HORIZON_HOME="$GD"
SOCK=$(mktemp -d)/ctl.sock
C="$H/target/fast/horizon-ctl --socket $SOCK"
caffeinate -u -t 300 & wake=$!
sleep 2
$C launch --game "$H/target/fast/horizon-client" --wait --game-dir "$GD" --resources "$RESOURCES" --world "${WORLD:-New World}" --dev-tools >/dev/null || exit 1
$C wait-ready >/dev/null
sleep 6
$C stats | grep -oE '"avg_fps":[0-9.]+|"frames":[0-9]+'
timeout 60 $C screenshot "$SHOT" && echo "screenshot: $SHOT"
grep -m1 'Horizon renderer:' "$GD/logs/latest.log"
$C quit >/dev/null
kill $wake 2>/dev/null
