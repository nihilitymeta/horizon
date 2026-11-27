-- Probe for the four loot hooks: logs what each gives and drives a custom condition.
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
	if event.id ~= "minecraft:entities/zombie" then
		return
	end
	local t = copy(event.table)
	t.pools[#t.pools + 1] = {
		rolls = 1,
		condition = { type = "loot_probe:yes" },
		entries = { { type = "minecraft:item", name = "minecraft:emerald" } },
	}
	t.pools[#t.pools + 1] = {
		rolls = 1,
		entries = {
			{
				type = "minecraft:item",
				name = "minecraft:diamond",
				modifier = {
					{ type = "minecraft:set_count", count = 3 },
					{ type = "loot_function_sample:double" },
				},
			},
		},
	}
	event.table = t
	horizon.log("PROBE table_load zombie pools=" .. #t.pools)
end)

horizon.on("loot_condition", function(event)
	if event.kind ~= "loot_probe:yes" then
		return
	end
	horizon.log("PROBE condition kind=" .. event.kind .. " luck=" .. tostring(event.ctx.luck))
	event.holds = true
end)

horizon.on("loot_function", function(event)
	horizon.log("PROBE function kind=" .. event.kind .. " count=" .. tostring(event.stack.count))
end)

horizon.on("loot_drops", function(event)
	local c = event.context
	local names = {}
	for _, s in ipairs(event.drops) do
		names[#names + 1] = s.id .. "x" .. s.count
	end
	horizon.log("PROBE drops table=" .. c.table .. " player=" .. tostring(c.player) .. " n=" .. #event.drops .. " [" .. table.concat(names, ",") .. "]")
end)
