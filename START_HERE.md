# Start Here

Event Horizon's own page. [README.md](README.md) is upstream's, kept as upstream wrote it.

Event Horizon is a fork of [horizon-rust/horizon](https://github.com/horizon-rust/horizon) (Minecraft: Java Edition
26.3 in Rust) that carries the engine changes
**[horizon-parasite-mod](https://github.com/whos-carmen/horizon-parasite-mod)** needs: mob AI for mods, a Lua runtime
inside content mods, developer tools, and a few game and launcher additions. Mods are written in Lua with
**[horizon-modkit](https://github.com/whos-carmen/horizon-modkit)**. Mod work happens in the mod's own repository;
come here for engine work, or to build the game that runs it.

**Upstream base:** [`1989b1b59`](https://github.com/horizon-rust/horizon/commit/1989b1b59) (2026-11-04). Event
Horizon follows upstream when upstream has something it wants: upstream is merged into `engine-work`
([how](docs/fork-workflow.md#syncing-upstream)).

## Where to go

| To… | Read |
|---|---|
| build the game and play or test it | [docs/building.md](docs/building.md) |
| set up a machine the same way as the others | [docs/dev-setup.md](docs/dev-setup.md) |
| see what Event Horizon adds, and where each part lives | [docs/fork-branches.md](docs/fork-branches.md) |
| change the engine, or sync with upstream | [docs/fork-workflow.md](docs/fork-workflow.md) |
| give a coding agent the rules | [docs/agents.md](docs/agents.md) |

## Branches

| Branch | What it is |
|---|---|
| [`engine-work`](https://github.com/whos-carmen/horizon/tree/engine-work) | Event Horizon: upstream plus every addition. Build and change this one |
| [`main`](https://github.com/whos-carmen/horizon/tree/main) | A mirror of upstream `horizon-rust/horizon`. Never commit here |

Until 2026-11-04 each addition lived on a branch of its own, merged into `engine-work` at every sync. They are now
part of `engine-work`. The tag `archive/<branch>` keeps each old branch's last version, including the ones that were
dropped: `amd-cas` (no visible effect) and the mesh-shader branches (`mesh-shaders`, `metal-mesh-shaders`).
