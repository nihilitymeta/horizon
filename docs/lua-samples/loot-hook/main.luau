-- Loot-hook sample: shows the container_loot_filled hook, which fires when a container's loot
-- table is rolled. The handler reads e.context.table (the loot table's id) and, for one chosen
-- vanilla table, answers e.drops with the drops the game would give plus one diamond. The change
-- is deterministic: every fill of that chest table gets exactly one extra diamond.
-- Hook used: container_loot_filled (listen, answer field: drops).

local TARGET_TABLE = "minecraft:chests/simple_dungeon"
local EXTRA_ITEM = "minecraft:diamond"

horizon.on("container_loot_filled", function(event)
	if event.context.table ~= TARGET_TABLE then
		return
	end

	-- Keep every stack the game rolled, in order, then append the extra one.
	local drops = {}
	for _, stack in ipairs(event.drops) do
		table.insert(drops, stack)
	end
	table.insert(drops, { id = EXTRA_ITEM, count = 1 })
	event.drops = drops
end)
