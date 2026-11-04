#!/usr/bin/env bash
# Checks engine-work as it is checked out: clippy (--tests) and fmt for the game workspace, the whole test suite
# (nextest), and the launcher workspace's clippy and suite. A line per step goes to <summary file>.
#   tools/fork-sync/verify.sh [summary file]       # default: ~/sync-summary.txt
# Run it on a quiet machine: another build at the same time doubles its time. About 6 minutes on an M2 Max.
# Upstream tests known to fail on macOS, on plain upstream main too, are listed apart from real failures.
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
# Incremental caches speed up only small edits, and grow without limit.
export CARGO_INCREMENTAL=0
KNOWN="entities::drop_motion_tests::drops_match_vanillas_motion_bit_for_bit native_mods::tests::the_log_is_fresh_per_attempt_and_keeps_early_failures"
PKGS=(-p horizon-client -p horizon-client-lib -p xtask)
OUT=${1:-$HOME/sync-summary.txt}
OUT=$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")
: > "$OUT"
summary() { # <name> <log>: the counts, the real failures, the known ones
  local fails real known
  fails=$(grep -E '^ +FAIL ' "$2" | awk '{print $NF}' | sort -u)
  real=$(for f in $fails; do [[ " $KNOWN " == *" $f "* ]] || echo "$f"; done | tr '\n' ' ')
  known=$(for f in $fails; do [[ " $KNOWN " == *" $f "* ]] && echo "$f"; done | tr '\n' ' ')
  echo "$1: $(grep -E '^ +Summary' "$2" | tail -1 | sed -E 's/ +Summary \[ *([0-9.]+)s\] /\1 s, /')" >> "$OUT"
  [ -n "$real" ] && echo "   FAILED: $real" >> "$OUT"
  [ -n "$known" ] && echo "   known upstream failures: $known" >> "$OUT"
}
start=$(date +%s)
echo "engine-work $(git rev-parse --short HEAD)" >> "$OUT"
clip=$(cargo clippy "${PKGS[@]}" --tests 2>&1 | grep -E '^(error|warning)(\[|:)' | sort | uniq -c)
echo "clippy: ${clip:-clean}" >> "$OUT"
echo "fmt: $(cargo fmt --all --check 2>/dev/null | grep -c '^Diff in') files to format" >> "$OUT"
cargo nextest run "${PKGS[@]}" --no-fail-fast > "$OUT.game.log" 2>&1
summary "game tests" "$OUT.game.log"
lclip=$(cd launcher && cargo clippy --tests 2>&1 | grep -E '^(error|warning)(\[|:)' | sort | uniq -c)
echo "launcher clippy: ${lclip:-clean}" >> "$OUT"
(cd launcher && cargo nextest run --no-fail-fast > "$OUT.launcher.log" 2>&1)
summary "launcher tests" "$OUT.launcher.log"
echo "$(( ($(date +%s)-start) / 60 )) min, build folder $(du -sh target 2>/dev/null | cut -f1)" >> "$OUT"
cat "$OUT"
