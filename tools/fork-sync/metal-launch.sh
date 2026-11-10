#!/usr/bin/env bash
# Opens the built game in a window on this Mac, reads its frame stats, takes a screenshot, and quits:
# the "it renders" check after a sync. Wakes the display first (a sleeping display never presents a
# frame, and the screenshot waits forever).
#   tools/fork-sync/metal-launch.sh [screenshot.png]     # RESOURCES defaults to ~/src/horizon-resources/26.3
# The game runs in a fresh scratch folder, never the player's own save folder, unless GAME_DIR is set.
set -u
H=$(cd "$(dirname "$0")/../.." && pwd)
SHOT=${1:-$(mktemp -d)/metal.png}
SCRATCH=$(mktemp -d)
GD=${GAME_DIR:-$SCRATCH/game}
mkdir -p "$GD"
RESOURCES=${RESOURCES:-$HOME/src/horizon-resources/26.3}
export HORIZON_HOME="$GD"
# The launcher and XDG homes and the updater and audio go to scratch too (CLAUDE.md, real player data).
export HORIZON_LAUNCHER_HOME="${HORIZON_LAUNCHER_HOME:-$SCRATCH/launcher}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-$SCRATCH/xdg-data}" XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$SCRATCH/xdg-config}"
export HORIZON_LAUNCHER_NO_SELF_UPDATE=1 HORIZON_NO_AUDIO=1
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
