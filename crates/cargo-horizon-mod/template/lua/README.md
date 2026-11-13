# {name}

A [Horizon](https://playhorizon.dev) mod written in Lua with [Horizon Modkit](https://github.com/nihilitymeta/horizon-modkit).
`scripts/registry.lua` declares what the mod adds.

```sh
horizon-client --dev-mod /path/to/horizon-modkit --dev-mod .   # play, with the modkit next to this mod
cargo horizon-mod build                                        # builds target/horizon-mod/{id}.hmod
cargo horizon-mod check target/horizon-mod/{id}.hmod           # runs the registry's checks
```

## Layout

- `scripts/server/init.lua` — entry that runs on the server (world logic).
- `scripts/client/init.lua` — entry that runs on the client (player's game).
- `scripts/shared/` — modules either side may `require`.
- `scripts/registry.lua` and `scripts/registry/` — the data stage, read once at start.

Each side is one Lua state. `horizon.side` is `"server"` or `"client"` in that state.
`require("util")` looks in the side's own folder first, then `scripts/shared/`.
A hook tagged for the other side is refused at load with an error that names
the folder to use.

Saved data (`horizon.world_data()`, `player_data`) is server state.
The client reads server data only through messages (planned).
Changes to `scripts/server/init.lua` or `scripts/client/init.lua` take effect
on `/reload`. A new item, block or entity type in `scripts/registry.lua` needs
the game restarted. The old `scripts/init.lua` is no longer loaded.

The `.luaurc` here names the modkit library next to this folder,
`../horizon-modkit/scripts/server/modkit`. With it, an editor that runs
luau-lsp resolves `require("@modkit")` and completes the API from the modkit's
`types/horizon.d.lua`. If the modkit lives somewhere else, change the path.
