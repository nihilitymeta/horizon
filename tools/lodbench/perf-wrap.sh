#!/bin/bash
# Runs the game under perf record inside the container (perf_event_paranoid 2 allows
# profiling one's own child): PERF_WRAP_BIN is the real horizon-client, PERF_WRAP_OUT the
# data file, WRAP_CPUS the CPUs. horizon-ctl gives the game scratch XDG folders, which podman
# must not see: they go back to the game through env inside the container.
GAME_DATA="$XDG_DATA_HOME"; GAME_CONF="$XDG_CONFIG_HOME"
export XDG_DATA_HOME="$HOME/.local/share" XDG_CONFIG_HOME="$HOME/.config"
exec taskset -c "${WRAP_CPUS:-0-7}" distrobox enter horizon -- env "XDG_DATA_HOME=$GAME_DATA" "XDG_CONFIG_HOME=$GAME_CONF" perf record -F 499 -g -o "$PERF_WRAP_OUT" -- "$PERF_WRAP_BIN" "$@"
