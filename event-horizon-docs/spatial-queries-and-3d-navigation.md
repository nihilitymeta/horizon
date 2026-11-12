# Event Horizon Engine Proposals: 3D Spatial Queries & Volumetric Navigation

**Target Engine:** Event Horizon ([`nihilitymeta/horizon`](https://github.com/nihilitymeta/horizon) on `engine-work`)  
**Context:** Needed by [`horizon-parasite-mod`](https://github.com/nihilitymeta/horizon-parasite-mod) (`basic-senses` branch) for 3D perception and vertical mob navigation.  

---

## Filed Issues

The feature suggestions have been filed as issues on `nihilitymeta/horizon`:

1. **[#58: Lua / AI: native chunk-level line-of-sight and visibility queries (`horizon.query_los`)](https://github.com/nihilitymeta/horizon/issues/58)**
   * Native Amanatides-Woo voxel traversal through chunk section block palettes in memory (sub-2 µs vs. 50–100 µs FFI loops in Lua).
2. **[#59: AI / Tactical: environment spatial queries for vertical awareness (pillars, viewing standoff, ground elevation)](https://github.com/nihilitymeta/horizon/issues/59)**
   * Native recognition of $1\times1$ pillars, high-ground disparity, and surface standoff positions ensuring mobs can look up at elevated targets without straining neck pitch limits.
3. **[#60: AI / Pathfinding: 3D volumetric pathfinding and dynamic swarm flow fields](https://github.com/nihilitymeta/horizon/issues/60)**
   * Background 3D A* navigation with vertical drop/climb heuristics (`horizon.find_path`), plus native compact byte-buffer BFS flow fields (`horizon.create_flow_field`) for swarm steering.

---

## Detailed Specifications

### 1. Native Line-of-Sight (`horizon.query_los`)
```lua
local result = horizon.query_los(from_pos, to_pos, {
  see_through = "translucent", -- or array of block ids / tags to ignore
  max_distance = 48.0,
})

-- Returns:
-- result.visible: boolean
-- result.hit_block: string? (e.g. "minecraft:stone")
-- result.hit_pos: { x = number, y = number, z = number }?
-- result.traversed_steps: number
```

### 2. Tactical Environmental Queries (EQS Equivalent)
```lua
-- 1. Pillar and vertical disparity detection
local ctx = horizon.query_elevation_context(mob_pos, target_pos)
-- ctx.dy: number
-- ctx.dist_2d: number
-- ctx.pitch_angle: number
-- ctx.is_pillared: boolean
-- ctx.has_high_ground: boolean

-- 2. Optimal viewing standoff for looking up
local standoff = horizon.find_viewing_standoff(mob_pos, target_pos, {
  target_pitch = 35.0,    -- comfortable viewing elevation
  min_distance = 3.0,
  max_distance = 16.0,
  require_los = true,
})
-- Returns: ground { x, y, z }

-- 3. Surface elevation lookup
local ground_y = horizon.get_ground_y(x, z, { search_from = y, max_range = 16 })
```

### 3. 3D Volumetric Pathfinding (`horizon.find_path`)
```lua
local path = horizon.find_path(start_pos, goal_pos, {
  max_drop = 3,           -- max safe drop in blocks
  max_step_up = 1.0,      -- jump step limit
  can_climb = false,      -- ladder/vine climbing
  can_swim = true,        -- water traversal
  allow_partial = true,   -- return closest point if unreachable
  max_evaluations = 400,  -- node expansion budget
})
```

### 4. Dynamic Swarm Flow Fields (`horizon.create_flow_field`)
```lua
local field = horizon.create_flow_field({
  target = { x = 120, y = 64, z = 240 },
  radius = 24,
  mode = "attract", -- or "repel"
})

-- Mobs query steering vector in O(1):
local dir = field:sample(mob.position) -- { dx, dy, dz }
```

### 5. Head Pitch & Orientation Exposure
```lua
mob.head_yaw   -- actual clamped head yaw in degrees
mob.head_pitch -- actual head pitch in degrees (-90 to +90)
```
