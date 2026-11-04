# {name}

A [Horizon](https://playhorizon.dev) mod written in Lua with [Horizon Modkit](https://github.com/whos-carmen/horizon-modkit).
`scripts/registry.lua` declares what the mod adds, and `scripts/init.lua` says what it does.

```sh
horizon-client --dev-mod /path/to/horizon-modkit --dev-mod .   # play, with the modkit next to this mod
cargo horizon-mod build                                        # builds target/horizon-mod/{id}.hmod
cargo horizon-mod check target/horizon-mod/{id}.hmod           # runs the registry's checks
```

Changes to `scripts/init.lua` take effect when you type `/reload` in the game. A new item, block or entity type in
`scripts/registry.lua` needs the game restarted.

The `.luaurc` here names the modkit checkout next to this folder, `../horizon-modkit`. With it, an editor that runs
luau-lsp resolves `require("@modkit")` and completes the API from the modkit's `types/horizon.d.lua`. If the modkit
lives somewhere else, change the path.
