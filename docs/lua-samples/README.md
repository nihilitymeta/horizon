# Lua sample mods

Four small Event Horizon Lua mods, each a folder with its `lua-mod.toml` and `main.luau`.
They are checked with `event-horizon-lua-check check <folder>`.

To install one, copy its folder to `lua-mods/<id>/` in the game folder, where `<id>` is the
`id` in its `lua-mod.toml` (for example `lua-mods/loot_hook_sample/`).

- `loot-hook/`: uses the `container_loot_filled` hook (listen, answer field `drops`). For the
  vanilla table `minecraft:chests/simple_dungeon` it keeps the rolled drops and adds one diamond.
- `asset-settings/`: declares one toggle setting, `loud`, read with `horizon.setting`. The command
  `greet` (run through `run_mod_command`) answers in capitals when it is on. It also ships
  `lang/en_us.json`, published under the mod's namespace.
- `loot-function/`: uses `loot_table_load` to add the function `loot_function_sample:double` to the modifier
  list of every entry in `minecraft:chests/simple_dungeon`, and `loot_function` to double the count of that
  stack (capped at 64).
- `loot-drops/`: uses `loot_drops` to add one `minecraft:bone` to the drops of a zombie that a player killed.
