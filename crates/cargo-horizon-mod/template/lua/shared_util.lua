-- Shared between the server and client states (scripts/shared/util.lua).
-- Either side may require("util").
local M = {}

function M.greeting(name)
  return "Hello, " .. name
end

return M
