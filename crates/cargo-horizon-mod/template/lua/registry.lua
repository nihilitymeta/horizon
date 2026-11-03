-- What the mod adds, declared once when the game starts (docs/lua-bindings.md,
-- "Registering content from Lua"). Adding an item, block or entity type here
-- needs the game restarted; scripts/init.lua reloads with /reload.
local ns = registry.id

-- A wand: a stick by template, with a stick's look and a name of its own
-- (assets/{id}/lang/en_us.json).
registry.item("wand", { template = "minecraft:stick", max_stack_size = 1 })
registry.asset("items/wand", { model = { type = "minecraft:model", model = ns .. ":item/wand" } })
registry.asset("models/item/wand", { parent = "minecraft:item/generated", textures = { layer0 = "minecraft:item/stick" } })
