# Mob AI for mods

Status: the hook point (`mob_ai_register`, pending its API number), the goal adapter for mods and mobs declared in a
content mod's data (a vanilla template with the mod's attributes, size, texture and goals) are built; the rest is the plan. Branch
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

## Goals from outside the crate (built)

`Goal` itself is public through `horizon_client::mod_api`, so a trait mod may implement it and touch everything a
vanilla goal does. Most mods, and every script, should not need to: `mobs/native_goal.rs` gives a goal a fixed
shape that borrows nothing of the mob.

- `MobView`: what a call sees of its mob (id, kind, position, velocity, health, target and where it stands, whether
  it is in melee range, whether a path is being walked, the nearest player, the home). A plain copy, made before the
  call.
- `Actions` / `GoalAction`: what the call asks for (`navigate_to`, `navigate_to_target`, `stop_navigation`,
  `move_toward`, `look_at`, `look_at_target`, `set_target`, `jump`, `attack`, `swing`; `hurt(who, amount)`,
  `convert(kind)` (become another mob, as a zombie drowns into a drowned), `summon(kind, at)`, `despawn`, `heal`,
  `effect(name, ticks, amplifier)`). Applied in order after the call, with the mob and the world.
- `MobView::nearby`: the other mobs within the follow range, nearest first (`NearbyView`: id, kind, position,
  distance, health, alive, hostile, baby, target), so an infection can look for hosts or a brood for its mother.
- `NativeBehaviour`: the decisions (`can_use`, `can_continue`, `start`, `tick`, `stop`) over a `MobView` and an
  `Actions`. One value is shared by every mob the goal is on; the per-mob part is its `State` (`Default` when added).
- `NativeGoal::new(name, flags, behaviour)`: the `Goal` to add, with `GOAL_MOVE | GOAL_LOOK | GOAL_JUMP | GOAL_TARGET`
  for the control flags it locks, `.updated_every_tick()` and `.uninterruptable()`.

```rust
use horizon_client::mod_api::{
    Actions, GOAL_LOOK, GOAL_MOVE, Hooks, MobAiRegisterEvent, MobView, NativeBehaviour, NativeGoal,
};

/// Walks at the nearest player while one is within range; a melee goal of its own would attack.
#[derive(Debug)]
struct Stalk { range: f64, speed: f64 }
impl NativeBehaviour for Stalk {
    type State = ();
    fn can_use(&self, _: &mut (), mob: &MobView, _: &mut Actions) -> bool {
        mob.nearest_player.is_some_and(|p| mob.distance_to(p) < self.range)
    }
    fn tick(&self, _: &mut (), mob: &MobView, out: &mut Actions) {
        if let Some(p) = mob.nearest_player {
            out.look_at(p);
            if mob.navigation_done { out.navigate_to(p, self.speed); }
        }
    }
}

struct Mod;
impl Hooks for Mod {
    fn mob_ai_register(&self, event: &mut MobAiRegisterEvent<'_>) {
        if event.mob.kind.id == "minecraft:zombie" {
            event.mob.ai.goals.add(2, NativeGoal::new("Stalk", GOAL_MOVE | GOAL_LOOK, Stalk { range: 24.0, speed: 1.1 }));
        }
    }
}
```

The same two types are what the Lua runtime hands a script's goal ([lua-bindings.md](lua-bindings.md), step 4): the
view becomes a table, the actions come back from it. Unit tests cover the view and the world-free actions; the path
and attack actions run only in a game with the vanilla data.

## Mobs from data (built)

A content mod declares a mob in `data/<id>/horizon/entity_type/<name>.json` with a vanilla mob as `template` and its
own `attributes`, `size`, `texture` and `goals` ([mods.md](mods.md#content-mods-api-02), entity types). The game
appends the type's row to `entity-types.json` (the template's, with the mod's values and a `horizon` object naming
the template, texture and goals: `mod_content::entity_types_json`), and `mobs::MobTypes::load` builds a `MobType`
for it from the template's species definition (model, sounds, animation, flags, brain) with the mod's id and texture.
The mob is the template's species everywhere species decides (`Mob::species`): AI families, conversion, burning,
breeding. Its texture joins the shared atlas through a `single` atlas source the content root writes
(`assets/<id>/atlases/horizon_entities.json`). With `"goals": "none"`, `Mob::new` clears both goal selectors after
the species registered them, so `mob_ai_register` (and a script's `horizon.add_goal`) sees empty selectors and the
species' controls. It drops `<id>:entities/<name>` when that loot table exists, else the template's.

## The plan

| Step | What | Why |
|---|---|---|
| 2 | `native_set_mob_target(dimension, id, target)` and `native_mob_navigate_to(dimension, id, pos, speed)` helpers | A tick hook can drive a mob it did not register a goal on. Later: the goal adapter above covers the parasite mod |
| 4 | ~~Content entity types from a mob template~~ built (above); left: a `goals` list naming vanilla goals by name with their arguments, natural spawning rules, a spawn egg | A parasite is declared in data, as a block is; its behaviour comes from step 1's hook or a script |
| 5 | `mob_ai_tick(mob)` listen hook each AI tick, and `mob_target_changed` | Reactive scripts (infection spreading, host conversion) without a goal |
| 6 | Brain species: a hook after a brain's activities are built | Villagers, piglins and wardens use brains, not goals |

Each step is one fragment and one hook or helper, named by the NeoForge rules in
[native-hooks-neoforge.md](native-hooks-neoforge.md) where an event exists (there is none for `registerGoals`, so
this one keeps a Horizon name). Step 4 is what the Lua branch needs next, so a parasite can be its own entity type rather than a redressed zombie.
