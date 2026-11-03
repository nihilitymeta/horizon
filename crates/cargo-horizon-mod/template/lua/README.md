# {name}

A [Horizon](https://playhorizon.dev) mod written in Lua with [Horizon Modkit](https://github.com/whos-carmen/horizon-modkit):
`scripts/registry.lua` says what it adds, `scripts/init.lua` what it does.

```sh
horizon-client --dev-mod /path/to/horizon-modkit --dev-mod .   # the modkit beside this mod, then play
cargo horizon-mod build                                        # target/horizon-mod/{id}.hmod
cargo horizon-mod check target/horizon-mod/{id}.hmod           # the registry's checks
```

Edits to `scripts/init.lua` are live: `/reload` in the game runs them again. A new item, block or entity type in
`scripts/registry.lua` needs the game restarted. The `.luaurc` here names the modkit checkout beside this folder
(`../horizon-modkit`): change the paths if it lives elsewhere.
