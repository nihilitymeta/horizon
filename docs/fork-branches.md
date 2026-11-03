# The fork's branches: what each can do today

Each integrated branch of [START_HERE.md](../START_HERE.md)'s table is one concern; this is what it does right now, how it is turned on,
and what is still planned. The branch's own `docs/*.md` has the details. To test or to develop the content mod, build `engine-work`: it has all of
this. Build a single branch only to work on that feature (`egui-tools` carries `lua-bindings` and `mob-ai-hooks`;
`hot-reload` is upstream plus content reload and nothing else).

**`mod-registry`** — A build declares its switchable features as `crates/horizon-client-render/registry/<id>.json`
(kind `hook` or `engine`, the setting it maps to, its default). Options > Mods lists them, `horizon-client --list-mods`
prints them as JSON for the launcher, and `horizon-mods.txt` carries `mod.<id>:true|false` lines so switching a feature
is writing one line. A feature branch registers by adding one file. The game's Options > Mods switches them; the
launcher no longer lists them (`launcher` below).

**`anti-lag`** — AMD Anti-Lag 2 through `VK_AMD_anti_lag`: Vulkan with an AMD driver only, inert elsewhere.
Graphics > Advanced > Frames row; registers `anti_lag`.

**`websocket`** — A WebSocket multiplayer transport (`ws://`, `wss://`) so a world can be reached through
Cloudflare Tunnels and other HTTP-only relays; the door's limits and settings, a tunnel player's forwarded address,
F3 shows the transport and path. [docs/websocket.md](https://github.com/whos-carmen/horizon/blob/websocket/docs/websocket.md).

**`extras-keybinds`** — The zoom, minimap, FPS counter and keystrokes hook mods as registry entries with their
Extras rows; zoom and minimap bindable in the standard Key Binds screen. Builds on `mod-registry`.

**`menus`** — The Mods and Extras menus restructured into two dedicated pages (installed mods with a settings button
only where a mod has one; all mod configs together); fullbright, dynamic lights, inventory sort, mouse tweaks, auto
reconnect and own name tag registered as hook mods. Builds on `extras-keybinds`.

**`debug-overlay`** — F3 additions: process memory, frame lows, and the chart plumbing they draw with.

**`launcher`** — Upstream's launcher (its redesign of 2026-11-04 included) with two additions: a switch that stops
launcher self-updates, so a locally built launcher is not replaced by a release, and `tools/deploy_local_mac.sh`,
which installs a working tree's game as a launcher version on macOS (`--launcher` installs the launcher itself as
`~/Applications/Horizon.app`). The fork's earlier Mods-page changes (the build's features by kind from
`--list-mods`, branch labels on local builds) were dropped at that sync: the build's features are switched in the
game's Options > Mods. `settings.json` `game_args` (`--dev-tools` for the tools below) is upstream's.

**`mob-ai-hooks`** — Mobs declared in a content mod's data: `entity_type/<name>.json` copies any vanilla mob with the
mod's own attributes, size, texture, nature flags and either the template's goals or none for scripts to fill;
`/summon <id>:<name>` makes one, it drops its own loot table. The `mob_ai_register` hook point fires as a species
sets up a mob's goal selectors, so a mod can add, replace or clear goals per mob; `LevelWriteHost::summon` lets a
random or scheduled tick make a mob. Goals from outside the crate: a `NativeBehaviour` sees a `MobView` (a copy
of the mob: position, health, target, nearest player, navigation state…) and answers with `GoalAction`s (navigate,
steer, look, target, jump, attack, swing) that the game applies after the call; `NativeGoal` wraps it as a `Goal`.
Re-exported through `mod_api`. Such a mob spawns naturally through upstream's NeoForge biome modifiers
(`neoforge:add_spawns` with NeoForge's `minCount`/`maxCount`), in its template's spawn category. Planned: target and navigate helpers for tick hooks, mob templates in data, a
per-AI-tick hook, brain species. [docs/mob-ai-hooks.md](https://github.com/whos-carmen/horizon/blob/mob-ai-hooks/docs/mob-ai-hooks.md).

**`lua-bindings`** — A content `.hmod` with capability `scripts` carries Lua (Luau) in `scripts/`; the game runs the
scripts of every installed mod that is on and of every `--dev-mod` folder, one state per mod, libraries first
(`depends`, `require`). `scripts/registry.lua` declares the mod's entity types, blocks, items, loot, recipes and tags
in Lua instead of JSON. `scripts/init.lua` registers handlers on the hook points (`horizon.on`: server ticks, block
ticks and clicks, block entities with saved data, mob spawn, hurt, death, interact and tick, by kind of mob; a
player's chat on upstream's `player_chat` hook point, which may drop the line or answer the sender but not change it),
defines mob goals (`horizon.goal`, a view in and actions out) and keeps saved data on mobs. `/reload` reads the
scripts again; 250 ms per handler, 5 s for `init.lua`, 64 MiB per state, no file access. The launcher installs
such a mod as content. Builds on `mob-ai-hooks`.
[docs/lua-bindings.md](https://github.com/whos-carmen/horizon/blob/lua-bindings/docs/lua-bindings.md).

**`egui-tools`** — Developer tools drawn with egui over the world, on only with `--dev-tools` or
`HORIZON_DEV_TOOLS=1`; their commands exist only then and only the host may use `/lua`. `F8` frees the mouse.
`/inspect <targets>` shows an entity's data as a tree, `/goals` a mob's goal selectors, `/senses` what it perceives,
`/nav` its path drawn in the world, `/watch <targets> <path>` a number over time, `/mobai log` AI snapshots as JSON
lines. `/lua` opens a console (a Luau state on the server thread with `mc.*` for commands, selectors and entity
data) that also lists the scripted mods with their handlers, errors and a reload button. Builds on `lua-bindings`.
[docs/dev-tools.md](https://github.com/whos-carmen/horizon/blob/egui-tools/docs/dev-tools.md).

**`hot-reload`** — Content mods reload without a restart: the resource-reload key re-layers assets, models, lang and
the data tables; `/reload` the data tables; registries still need the world reopened (section below).


Use [`hot-reload`](https://github.com/whos-carmen/horizon/tree/hot-reload) when iterating on a content mod — it is independent of every engine feature.

## Content hot-reload (`hot-reload`)

On [`hot-reload`](https://github.com/whos-carmen/horizon/tree/hot-reload) (based on `main`, not on engine features; also part of `engine-work`), run the game with `--dev-mod` pointing at the mod folder (no `.hmod` zip needed). In game:

| Action | Reloads |
|---|---|
| Debug **resource packs reload** key (`key.debug.reloadResourcePacks`) | Assets, models, lang **and** recipes / tags / loot (content mods re-layered first) |
| `/reload` | Data tables only (recipes, tags, loot) — content mods re-layered |
| Reopen the world | Block/item **registries** (new blocks still need this) |

The mod-content cache is stamped on every file’s size and mtime: edit a file, hit the key, see the change. No game restart for data or assets; world reopen only when you add or remove registered blocks/items.

**`recipe-book`** — A content mod's recipes (`data/<id>/recipe/`, from files or `scripts/registry.lua`) are in the
recipe book and known to `/recipe give`, as vanilla's are.
