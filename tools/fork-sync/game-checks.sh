#!/usr/bin/env bash
# The modkit's checks against the engine-work build in target/, one at a time, in a scratch game folder.
# One at a time matters: a check that fails can leave its game running, and two games on one folder spoil each
# other's world and log. Build first: cargo build --profile fast -p horizon-client -p horizon-client-lib,
# and cargo build -p cargo-horizon-mod.
#   tools/fork-sync/game-checks.sh [modkit folder]     # default: ~/src/horizon-modkit
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.local/bin:$PATH
H=$(cd "$(dirname "$0")/../.." && pwd)
MODKIT=${1:-$HOME/src/horizon-modkit}
export HORIZON=$H RESOURCES=${RESOURCES:-$HOME/src/horizon-resources/26.3} HORIZON_NO_AUDIO=1
scratch=$(mktemp -d)
export GAME_DIR=$scratch/game HORIZON_HOME=$scratch/game
mkdir -p "$GAME_DIR"
cd "$MODKIT" || exit 1
failed=0
for c in "check.sh --self-test" docs-check.sh scaffold-check.sh example-check.sh hook-coverage.sh; do
  log="$scratch/${c%% *}.log"; start=$(date +%s)
  tools/$c > "$log" 2>&1; rc=$?
  echo "$c: exit $rc, $(( $(date +%s)-start )) s: $(tail -1 "$log")"
  [ $rc = 0 ] || failed=1
  # Whatever a failed check left running goes before the next one starts.
  pkill -f "$H/target/fast/horizon-client" 2>/dev/null; sleep 1
done
echo "logs: $scratch"
exit $failed
