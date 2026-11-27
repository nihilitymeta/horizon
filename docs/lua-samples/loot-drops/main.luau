-- Loot-drops sample: a zombie that a player kills drops one extra bone.
-- loot_drops fires when a loot table rolls for a block, a mob or an explosion. event.context
-- says what dropped the items; event.drops is the list the game rolled, each entry a stack
-- table {id, count, nbt}. The handler answers by assigning a new list.
-- Hook used: loot_drops (listen, answer field: drops).

local TABLE_ID = "minecraft:entities/zombie"
local EXTRA_ITEM = "minecraft:bone"

horizon.on("loot_drops", function(event)
	local context = event.context
	if context.table ~= TABLE_ID or context.player == nil then
		return
	end

	-- The drops are read-only: keep every stack in a new list, then append the extra one.
	local drops = {}
	for _, stack in ipairs(event.drops) do
		table.insert(drops, stack)
	end
	table.insert(drops, { id = EXTRA_ITEM, count = 1 })
	event.drops = drops
end)
