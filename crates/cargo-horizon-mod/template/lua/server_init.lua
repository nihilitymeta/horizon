-- This file runs on the server (scripts/server/init.lua).
-- What the mod does, with Horizon Modkit (its docs/api.md). This file and
-- the modules it requires run again on /reload.
local modkit = require("@modkit")
local M = modkit.mod("{id}")
local util = require("util")

-- A right click with the wand, in the air or on a block: a word to everyone,
-- sparks where the player stands. Return true and the click is taken
-- (nothing else acts on it).
local Wand = M:item("wand")
function Wand:on_use(item, player)
  local p = player.position
  horizon.broadcast(("%s waves the wand"):format(player.name))
  modkit.particles("minecraft:happy_villager", p.x, p.y + 1, p.z, 12, 0.5, 0.05)
  horizon.log(util.greeting(player.name))
  return true
end
function Wand:on_use_on(item, player, block, face)
  return self:on_use(item, player)
end
