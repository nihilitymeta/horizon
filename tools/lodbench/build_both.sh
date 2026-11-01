#!/bin/bash
export PATH=$HOME/.cargo/bin:$PATH
BASE=${HORIZON_BENCH_BASE:-$HOME/dev/horizon-wt}
for w in main mesh; do
  cd $BASE/$w || exit 1
  echo "=== building $w $(date +%T)"
  CARGO_TARGET_DIR=$BASE/target-$w cargo build --profile fast -p horizon-client --bins 2>&1 | grep -E "^(error|warning: unused)|Finished|-->" | head -40
  echo "=== done $w $(date +%T)"
done
echo BUILDSDONE
