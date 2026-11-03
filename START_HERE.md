# Start Here

This fork's own page. [README.md](README.md) is upstream's, kept as upstream wrote it.

Engine work for **[horizon-parasite-mod](https://github.com/whos-carmen/horizon-parasite-mod)**, a content mod
written in Lua with **[horizon-modkit](https://github.com/whos-carmen/horizon-modkit)**. This repository is a fork
of [horizon-rust/horizon](https://github.com/horizon-rust/horizon) (Minecraft: Java Edition 26.3 in Rust) carrying
our engine changes: mob AI for mods, a Lua runtime inside content mods, developer tools, and graphics work. Mod
development happens in the mod's own repository; come here for engine work, or to build the game that runs it.

**Upstream base:** [`1989b1b59`](https://github.com/horizon-rust/horizon/commit/1989b1b59) (2026-11-04, night). The fork
is synced with upstream when upstream has something it wants: `main` moves to upstream, every branch below is
rebased onto it, and `engine-work` is rebuilt ([how](docs/fork-workflow.md#syncing-upstream)).

**Build `engine-work`** to play or test: [docs/building.md](docs/building.md) (what a machine needs, on Windows,
macOS and Linux; prepared resources; the run line). To set up a machine the same way as the others (folders, one checkout, builds, mods): [docs/dev-setup.md](docs/dev-setup.md). To work on the engine: [docs/fork-workflow.md](docs/fork-workflow.md).

## Branches

| Branch | What it is |
|---|---|
| [`main`](https://github.com/whos-carmen/horizon/tree/main) | A mirror of upstream `horizon-rust/horizon`; never commit here |
| [`engine-work`](https://github.com/whos-carmen/horizon/tree/engine-work) | `main` + the docs commit + every branch below, merged in order: the game to build |
| a feature branch | `main` + one concern; rebased onto `main`, never merged into |

The integrated table **is** the build list, in merge order. What each one does today, how it is turned on and
what is planned: [docs/fork-branches.md](docs/fork-branches.md); each branch's own `docs/*.md` has the details.

| Branch | Feature |
|---|---|
| [`mod-registry`](https://github.com/whos-carmen/horizon/tree/mod-registry) | A build declares its switchable features; Options > Mods and `--list-mods` list them |
| [`anti-lag`](https://github.com/whos-carmen/horizon/tree/anti-lag) | AMD Anti-Lag 2 (`VK_AMD_anti_lag`, Vulkan only) |
| [`websocket`](https://github.com/whos-carmen/horizon/tree/websocket) | WebSocket multiplayer transport for tunnels and HTTP-only relays; [docs/websocket.md](https://github.com/whos-carmen/horizon/blob/websocket/docs/websocket.md) |
| [`extras-keybinds`](https://github.com/whos-carmen/horizon/tree/extras-keybinds) | Zoom, minimap and HUD hook mods as registry entries, bindable keys. Builds on `mod-registry` |
| [`menus`](https://github.com/whos-carmen/horizon/tree/menus) | Mods and Extras menus restructured. Builds on `extras-keybinds` |
| [`debug-overlay`](https://github.com/whos-carmen/horizon/tree/debug-overlay) | F3 memory, frame lows and chart plumbing |
| [`launcher`](https://github.com/whos-carmen/horizon/tree/launcher) | Upstream's launcher with a switch that stops self-updates, and `tools/deploy_local_mac.sh` |
| [`mob-ai-hooks`](https://github.com/whos-carmen/horizon/tree/mob-ai-hooks) | Mobs from a content mod's data; the `mob_ai_register` hook point; goals from outside the crate; [docs/mob-ai-hooks.md](https://github.com/whos-carmen/horizon/blob/mob-ai-hooks/docs/mob-ai-hooks.md) |
| [`lua-bindings`](https://github.com/whos-carmen/horizon/tree/lua-bindings) | Lua (Luau) inside a content `.hmod`: hooks, goals, mob lifecycle and saved data, block entities, `scripts/registry.lua`, libraries; [docs/lua-bindings.md](https://github.com/whos-carmen/horizon/blob/lua-bindings/docs/lua-bindings.md). Builds on `mob-ai-hooks` |
| [`egui-tools`](https://github.com/whos-carmen/horizon/tree/egui-tools) | Developer tools with `--dev-tools`: `/inspect`, `/goals`, `/nav`, the `/lua` console; [docs/dev-tools.md](https://github.com/whos-carmen/horizon/blob/egui-tools/docs/dev-tools.md). Builds on `lua-bindings` |
| [`hot-reload`](https://github.com/whos-carmen/horizon/tree/hot-reload) | Content mods reload on `/reload` and the resource-reload key |
| [`recipe-book`](https://github.com/whos-carmen/horizon/tree/recipe-book) | A content mod's recipes in the recipe book and `/recipe` |

Dropped (2026-11-04): `amd-cas` (no visible effect) and the mesh-shader branches (`mesh-shaders`,
`metal-mesh-shaders`). Their last tips are the tags `archive/<branch>`.
