#!/bin/bash
# Runs the game on the CPUs in WRAP_CPUS (one hardware thread per core: 0-7), WRAP_BIN the real binary.
exec taskset -c "${WRAP_CPUS:-0-7}" "$WRAP_BIN" "$@"
