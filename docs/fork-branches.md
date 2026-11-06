# What Event Horizon Adds

Each section is one addition to upstream: what it does today, how you turn it on, where its code lives, and what is
still planned. All of them are part of `engine-work`. Until 2026-11-04 each one lived on a branch of its own; the tag
`archive/<name>` keeps that branch's last version.

The `event-horizon` crate assembles the built-in mods and builds the `horizon-client` program. Its
`run` handles program startup and mod checks; the launcher's generated native-mod executable starts
through it too, so such a game also runs the built-in mods, behind the native mods.
`builtin_mods` puts the developer tools before the Lua mods.

## For mods

**Mob AI hooks** (`archive/mob-ai-hooks`). Mobs declared in a content mod's data: `entity_type/<name>.json` copies any
vanilla mob with the mod's own attributes, size, texture, nature flags, and either the template's goals or none for
scripts to fill. `/summon <id>:<name>` makes one, and it drops its own loot table.

- The `mob_ai_register` hook point fires as a species sets up a mob's goal selectors, so a mod can add, replace or
  clear goals per mob. `LevelWriteHost::summon` lets a random or scheduled tick make a mob.
- Goals from outside the crate: a `NativeBehaviour` sees a `MobView` (a copy of the mob: position, health, target,
  nearest player, navigation state) and answers with `GoalAction`s (navigate, steer, look, target, jump, attack,
  swing). `NativeGoal` wraps it as a `Goal`. Re-exported through `mod_api`.
- Such a mob spawns naturally through upstream's NeoForge biome modifiers (`neoforge:add_spawns` with `minCount` and
  `maxCount`), in its template's spawn category.
- Code: `crates/horizon-client/src/mobs/native_goal.rs`, `game/creatures.rs`, `game/native_mobs.rs`. Reference:
  [mob-ai-hooks.md](mob-ai-hooks.md).
- Planned: target and navigate helpers for tick hooks, mob templates in data, a per-AI-tick hook, brain species.

**Lua mods** (`archive/lua-bindings`). A content `.hmod` with the capability `scripts` carries Lua (Luau) in
`scripts/`. The game runs the scripts of every installed mod that is on, and of every `--dev-mod` folder: one state
per mod, libraries first (`depends`, `require`).

- `scripts/registry.lua` declares the mod's entity types, blocks, items, loot, recipes and tags in Lua instead of
  JSON.
- `scripts/init.lua` registers handlers on the hook points with `horizon.on`: server ticks, block ticks and clicks,
  block entities with saved data, mob spawn, hurt, death, interact and tick by kind of mob, a player's chat (dropped
  or answered, not changed), and more. It defines mob goals with `horizon.goal` and keeps saved data on mobs.
- `/reload` reads the scripts again. Each handler gets 250 ms, `init.lua` 5 s and each state 64 MiB, with no file
  access. The launcher installs such a mod as content.
- `cargo horizon-mod new --template lua` scaffolds a mod, and `cargo horizon-mod check` loads its scripts without a
  world.
- Code: `crates/event-horizon-lua/src/lua_mods.rs` and `lua_mods/`. Reference: [lua-bindings.md](lua-bindings.md), and
  the hook points in [api/hook-points.md](api/hook-points.md).

**Developer tools** (`archive/egui-tools`). Tools drawn with egui over the world, on only with `--dev-tools` or
`HORIZON_DEV_TOOLS=1`. Their commands exist only then, and only the host may use `/lua`. `F8` (the Free Mouse for Dev Windows binding) frees the mouse.

- `/inspect <targets>` shows an entity's data as a tree, `/goals` a mob's goal selectors, `/senses` what it
  perceives, `/nav` its path drawn in the world, `/watch <targets> <path>` a number over time, and `/mobai log` AI
  snapshots as JSON lines.
- `/lua` opens a console: a Luau state on the server thread with `mc.*` for commands, selectors and entity data. It
  also lists the scripted mods with their handlers and errors, and has a reload button. The console is a built-in
  mod (`crates/event-horizon-lua/src/console.rs`) that reads and writes the world through the engine's generic host
  helpers.
- Code: `crates/event-horizon-observe/` (the observation store and the wire), `crates/horizon-client/src/dev_input.rs` and
  `dev_input/` (the state the game and the tools share), `crates/event-horizon-dev-tools/` (the windows and their
  server half), `crates/event-horizon-lua/src/console.rs` (the console's runtime). Reference: [dev-tools.md](dev-tools.md).

**Content reload** (`archive/hot-reload`). Content mods reload without a restart. Run the game with `--dev-mod`
pointing at the mod folder; no `.hmod` is needed.

| Action | What it reloads |
|---|---|
| The resource-reload key (`key.debug.reloadResourcePacks`) | assets, models, lang, and recipes, tags and loot |
| `/reload` | recipes, tags and loot |
| Reopening the world | the block and item registries (a new block needs this) |

The mod-content cache is stamped on each file's size and time, so an edited file shows at the next reload.

A mod's declarations (its blocks, items, mob effects, entity types and menu types) load once, when the game starts,
and stay until a restart: a root built again while the game runs carries the declarations the game started with.
Code: `crates/horizon-mods/src/declarations.rs` (the declarations and what a reload keeps of them),
`crates/horizon-client/src/mod_content.rs` (the root build), `crates/horizon-client/src/game/mods.rs` (the calls).

**Recipe book** (`archive/recipe-book`). A content mod's recipes (`data/<id>/recipe/`, from files or
`scripts/registry.lua`) are in the recipe book and known to `/recipe give`, as vanilla's are. Code:
`crates/horizon-client/src/recipe_book.rs`.

## For the game

**Feature registry** (`archive/mod-registry`). A build declares its switchable features, one file each, as
`crates/horizon-client-render/registry/<id>.json`: its kind (`hook` or `engine`), the setting it maps to, and its
default. Options > Mods lists them, and `horizon-client --list-mods` prints them as JSON.
`horizon-mods.txt` carries `mod.<id>:true|false` lines, so switching a feature is writing one line. Format:
[registry/README.md](../crates/horizon-client-render/registry/README.md).

**Extras and key binds** (`archive/extras-keybinds`). The zoom, minimap, FPS counter and keystrokes extras are the built-in
native mod `crates/event-horizon-extras` (issue #29): one trait mod, four rows in Options > Mods, each with its own
`mod.<id>` switch (seeded from the old `horizon-extras.txt` lines) and settings page. The engine keeps the minimap's
tiles and drawing, the zoom's field-of-view plumbing, the frame counter for mods and the keybind rows
(`key.zoom`, `key.minimap`, bound in the standard Key Binds screen). Code: `crates/event-horizon-extras/`,
`crates/horizon-client-render/src/mod_list.rs` (the switch seeding), `crates/horizon-client/src/game/extras.rs`.

**Menus** (`archive/menus`). The Mods and Extras menus are two dedicated pages: installed mods, with a settings button
only where a mod has one, and every mod's settings together. Fullbright, dynamic lights, inventory sort, mouse tweaks,
auto reconnect and own name tag are registered as hook mods.

**Debug overlay** (`archive/debug-overlay`). F3 shows process memory and frame lows. Code:
`crates/horizon-client/src/debug_overlay.rs`, `debug_chart.rs`; the pure helpers:
`crates/event-horizon-debug/`.

**Event Horizon's own name and no telemetry** (since 2026-11-04). The title screen says `Event Horizon 26.3 (Rust)`.
The game and the launcher send nothing to Horizon's servers, the "I'm playing" ping included, unless
`HORIZON_TELEMETRY_URL` names a server (`crates/horizon-telemetry/src/lib.rs`, `endpoint`).

**Anti-Lag 2** (`archive/anti-lag`). AMD Anti-Lag 2 through `VK_AMD_anti_lag`, with Vulkan and an AMD driver only;
it does nothing elsewhere. Graphics > Advanced > Frames; registers `anti_lag`.

**WebSocket transport** (`archive/websocket`). A multiplayer transport over `ws://` and `wss://`, so a world can be
reached through Cloudflare Tunnels and other HTTP-only relays. It covers the door's limits and settings and a tunnel
player's forwarded address, and F3 shows the transport and path. Code: `crates/event-horizon-websocket/`; generic
transport traits and registration: `crates/horizon-transport/`. Reference: [websocket.md](websocket.md).

## For the launcher

**Launcher additions** (`archive/launcher`, and since 2026-11-04 on `engine-work`). Upstream's launcher with these
additions:

- **GitHub sources**: the Versions page installs game builds a repository publishes as GitHub releases, such as
  Event Horizon's own ([event-horizon-releases.md](event-horizon-releases.md)), each with an **Update
  automatically** switch. Code: `launcher/src/game_sources.rs`, `sources_page.rs`; `horizon-launcher sources`.
- **Event Horizon's builds named as such**: the Versions page lists them in a section of their own, as
  `Event Horizon 0.1.0` or `Event Horizon · local <commit>`, everywhere a version is named; Horizon's newer Nightly
  is no "Update available" for them, and the Play button offers an update only from the GitHub source that installed
  the build. Code: `game_sources::label`, `game_sources::pending`.
- **The modkit card** on the Mods page: the modkit installed into every instance that plays an Event Horizon build,
  its version tracked, with its own update switch. Code: `launcher/src/modkit.rs`, `modkit_page.rs`;
  `horizon-launcher modkit`, and `modkit install <file>` for a `.hmod` you built.
- **Nothing updates itself unless turned on**: installing new Horizon versions and the launcher's own self-update
  are off by default, so nothing upstream replaces an Event Horizon build (Settings > Launcher, or
  `horizon-launcher settings auto-update-launcher on`)
- `tools/deploy_local_mac.sh`, which installs a working tree's game as a launcher version on macOS (`--launcher`
  installs the launcher itself as `~/Applications/Horizon.app`)
