# Far-terrain benchmarks

Drivers for measuring the LOD (far terrain) renderer on a real GPU, offscreen, with the game
run from scratch homes (never a flatpak's data). They expect a layout under
`$HORIZON_BENCH_BASE` (default `~/dev/horizon-wt`):

- `bench-data/26.3`: a copy of the prepared resources (`.local/horizon-client/26.3`).
- `bench-results/`: where screenshots, stats and perf data land.
- `tmp/`: scratch homes (short path: Unix socket names are limited to ~108 bytes).
- `main/`, `mesh/` worktrees with `target-main`, `target-mesh` (`build_both.sh`, `--profile fast`).

| Script | What it measures |
|---|---|
| `lodab.py` | In-process A/B of mesh shaders on/off on the same resident tiles: screenshots, frame stats, GPU pass times, `mesh_counts`. `--spot mountain\|valley\|village\|ocean\|ground`, `--cpus` pins the game (via `taskset-wrap.sh`), `--workers`. |
| `lodbench.py` | Steady-state flights over four scenes, one build, one setting. |
| `lodload.py` | Streaming: how fast the far terrain fills in after a teleport. |
| `lodprof.py` | `perf record`/`perf stat` of the game from the `horizon` distrobox container (`perf-wrap.sh` launches the game under perf, since `perf_event_paranoid=2` forbids attaching). |
| `pixdiff.py` | Pixel difference of two screenshots (needs Pillow). |

Run one benchmark at a time with nothing else heavy on the machine, and check for an orphaned
`horizon-client` (it ignores SIGTERM) before trusting a number.
