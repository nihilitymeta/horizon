# Mob AI for mods

Status: the first hook point is built (`mob_ai_register`, pending its API number); the rest is the plan. Branch
`mob-ai-hooks` of whos-carmen/horizon, `main` + this one concern. Written for horizon-parasite-mod, whose first needs
are custom blocks and custom mob behaviour.

## What exists

- A mob's behaviour is a `GoalSelector` of `Goal`s (`crates/horizon-client/src/mobs/goals.rs`: `can_use`,
  `can_continue`, `start`, `tick`, `stop`, flags) plus a second selector of target goals, filled per species in
  `mobs/ai.rs` (`register`) and the family files (`mobs/cube.rs`, `mobs/raider.rs`…). Some species run a brain instead
  (`villager_brain.rs`, `warden_brain.rs`…).
- Hook points touch mobs from the outside only: `mob_spawned`, `mob_killed`, `entity_tick_pre/post`, the draw hooks
  (`mob_submit`, `mob_layer`) and the helpers (`native_spawn_mob` with `no_ai`, `native_teleport_mob`,
  `native_push_mob`). Nothing reaches the goals. The documented way to make "a mod's own creature" is a vanilla mob,
  redrawn, steered by hand from a tick hook ([native-mods.md](native-mods.md#mobs-drawn-by-a-mod)).

## Hook 1: `mob_ai_register` (built)

`horizon_client_lib::mobs::ai::native_mob_ai_register(mob: &mut Mob)`, a free function wired through
`native_client_hooks` like `native_living_breathe`. It runs in `Mob::new` right after the species' own `register`, so
the mob's vanilla goals are in `mob.ai.goals` and `mob.ai.targets` and nothing has ticked yet. A mod adds a goal
(`mob.ai.goals.add(priority, goal)`), adds a target goal, or clears a selector and rebuilds it. Trait mods get
`MobAiRegisterEvent { mob }` and the types `Mob`, `MobType`, `Goal`, `GoalSelector` and `GoalCx` through
`horizon_client::mod_api`. One call a mob, server side only.

Fragment: `tools/native-hook-fragments/mob_ai_register.json`. The API number is given when it is stamped
(`python3 tools/stamp_native_hooks.py`), which this fork does not do: a bundle on the fork pins the fork commit.

## The plan

| Step | What | Why |
|---|---|---|
| 2 | `native_set_mob_target(dimension, id, target)` and `native_mob_navigate_to(dimension, id, pos, speed)` helpers | A tick hook or a script can drive a mob without writing a `Goal` |
| 3 | A `NativeGoal` adapter: a `Goal` whose methods call `fn` callbacks (or a trait object) owned by the mod | Goals for mods that are not compiled inside the crate: trait mods and the Lua runtime ([lua-bindings.md](lua-bindings.md)) |
| 4 | Content entity types from a mob template: `"template": "minecraft:zombie"` in `entity_type/*.json`, with `attributes`, `size` and a `goals` list naming vanilla goals by name with their arguments | A parasite is declared in data, as a block is; its behaviour comes from step 1's hook or a script |
| 5 | `mob_ai_tick(mob)` listen hook each AI tick, and `mob_target_changed` | Reactive scripts (infection spreading, host conversion) without a goal |
| 6 | Brain species: a hook after a brain's activities are built | Villagers, piglins and wardens use brains, not goals |

Each step is one fragment and one hook or helper, named by the NeoForge rules in
[native-hooks-neoforge.md](native-hooks-neoforge.md) where an event exists (there is none for `registerGoals`, so
this one keeps a Horizon name). Steps 2 and 3 are what the Lua branch needs first.
