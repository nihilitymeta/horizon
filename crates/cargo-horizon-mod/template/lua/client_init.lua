-- This file runs on the client (scripts/client/init.lua).
-- It hears client_tick_post, which fires every client tick.
local util = require("util")
local ticks = 0
horizon.on("client_tick_post", function()
  ticks = ticks + 1
  if ticks == 200 then
    horizon.log(util.greeting("client"))
  end
end)
