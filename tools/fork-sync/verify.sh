#!/usr/bin/env bash
# Checks branches one after another in this one checkout, a summary line per branch appended to <summary file>
# (passed/failed counts, failing tests, clippy lines, the first errors). Returns to the branch it started on.
#   tools/fork-sync/verify.sh <summary file> [--full|--touched|--check-only] <branch>... [--full|...] <branch>...
# Each mode applies to the branches after it (the default is --full):
#   --full        clippy (--tests) and fmt, and the whole test suite: engine-work and the stack tips (menus,
#                 egui-tools). engine-work holds every branch, so its suite is what catches one branch's change
#                 breaking another module.
#   --touched     clippy and fmt (they build the whole game), and only the tests in the Rust files the branch
#                 changes (touched-tests.py): the single branches.
#   --check-only  clippy and fmt: the stack bases, whose code the tips' tests run.
# A branch that changes launcher/ also gets that workspace's clippy and its whole suite.
# One checkout, one build folder: git rewrites only the files a branch changes, so Cargo rebuilds those crates.
# Run it on a quiet machine. Copy tools/fork-sync out of the checkout first (it switches branches under itself).
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
# Incremental caches only speed up small edits, and grew to ~300 GB across worktrees.
export CARGO_INCREMENTAL=0
HERE=$(cd "$(dirname "$0")" && pwd)
PKGS=(-p horizon-client -p horizon-client-lib -p xtask)
OUT=${1:?summary file}; shift
OUT=$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "the checkout has changes: commit or stash them"; exit 1; }
back=$(git rev-parse --abbrev-ref HEAD)
nextest() { # <log> <nextest args>...: runs, then sets rc, passed, fails
  local log=$1; shift
  cargo nextest run --no-fail-fast "$@" > "$log" 2>&1; rc=$?
  passed=$(grep -E '^ +Summary' "$log" | sed -E 's/.* ([0-9]+) tests? run: ([0-9]+) passed.*/\2 passed/' | tail -1)
  fails=$(grep -E '^ +FAIL ' "$log" | awk '{print $NF}' | sort -u | tr '\n' ' ')
  passed="${passed:-0 passed}, $(echo $fails | wc -w | tr -d ' ') failed"
}
mode=--full
for b in "$@"; do
  case "$b" in --full|--touched|--check-only) mode=$b; continue;; --) mode=--full; continue;; esac
  git checkout -q "$b" || { echo "== $b checkout failed" >> "$OUT"; continue; }
  sha=$(git rev-parse --short HEAD)
  start=$(date +%s)
  clip=$(cargo clippy "${PKGS[@]}" --tests 2>&1 | grep -E '^(error|warning)(\[|:)' | grep -v 'generated [0-9]* warning' | sort | uniq -c)
  fmt=$(cargo fmt --all --check 2>/dev/null | grep -c '^Diff in')
  log="$OUT.$b.test.log"; line="== $b $sha ${mode#--}"
  case $mode in
    --check-only) line="$line clippy+fmt" ;;
    --full) nextest "$log" "${PKGS[@]}"; line="$line test-exit=$rc $passed" ;;
    --touched)
      changed=$(git diff --name-only main...HEAD)
      filter=$(echo "$changed" | python3 "$HERE/touched-tests.py")
      if [ -z "$filter" ]; then line="$line no Rust tests touched"
      else
        pk=(); for p in $(echo "$changed" | python3 "$HERE/touched-tests.py" --packages); do pk+=(-p "$p"); done
        nextest "$log" "${pk[@]}" -E "$filter"; line="$line test-exit=$rc $passed"
      fi ;;
  esac
  if [ "$mode" != --check-only ] && git diff --name-only main...HEAD | grep -q '^launcher/'; then
    lclip=$(cd launcher && cargo clippy --tests 2>&1 | grep -E '^(error|warning)(\[|:)' | grep -v 'generated [0-9]* warning' | sort | uniq -c)
    (cd launcher && nextest "$log.launcher"; echo "$rc|$passed|$fails" > "$log.launcher.rc")
    IFS='|' read -r lrc lpassed lfails < "$log.launcher.rc"; rm -f "$log.launcher.rc"
    line="$line; launcher test-exit=$lrc $lpassed"; [ -n "$lfails" ] && fails="$fails$lfails"; clip="$clip$lclip"
  fi
  { echo "$line fmt-diffs=$fmt $(( $(date +%s)-start ))s"; [ -n "${fails:-}" ] && [ "$mode" != --check-only ] && echo "   failures: $fails"
    [ -n "$clip" ] && echo "   clippy: $clip"; [ -f "$log" ] && grep -m3 -E '^error(\[|:)' "$log" | sed 's/^/   /'; } >> "$OUT"
  fails=""
done
git checkout -q "$back"
echo "build folder $(du -sh target 2>/dev/null | cut -f1)" >> "$OUT"
echo ALLDONE >> "$OUT"
