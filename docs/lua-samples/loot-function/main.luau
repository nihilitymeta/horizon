-- Loot-function sample: a loot function of this mod's own kind, reached from a vanilla table.
-- loot_table_load adds the function "loot_function_sample:double" to the modifier list of every
-- entry in minecraft:chests/simple_dungeon. loot_function then answers that kind: the stack's
-- count is doubled, capped at 64. Other kinds are left alone.
-- Hooks used: loot_table_load (listen, answer field: table), loot_function (listen, answer field: stack).

local TABLE_ID = "minecraft:chests/simple_dungeon"
local KIND = "loot_function_sample:double"
local MAX_COUNT = 64

-- A copy of a read-only JSON value. An empty table is shared, not copied: a copy would lose the
-- marker that keeps an empty JSON object an object.
local function copy(value)
	if type(value) ~= "table" or next(value) == nil then
		return value
	end
	local result = {}
	for key, item in pairs(value) do
		result[key] = copy(item)
	end
	return result
end

horizon.on("loot_table_load", function(event)
	if event.id ~= TABLE_ID then
		return
	end
	local table_copy = copy(event.table)
	for _, pool in ipairs(table_copy.pools) do
		for _, entry in ipairs(pool.entries) do
			local modifiers = {}
			for _, existing in ipairs(entry.modifier or {}) do
				modifiers[#modifiers + 1] = copy(existing)
			end
			modifiers[#modifiers + 1] = { type = KIND }
			entry.modifier = modifiers
		end
	end
	event.table = table_copy
end)

horizon.on("loot_function", function(event)
	if event.kind ~= KIND then
		return
	end
	local stack = copy(event.stack)
	stack.count = math.min(stack.count * 2, MAX_COUNT)
	event.stack = stack
end)
