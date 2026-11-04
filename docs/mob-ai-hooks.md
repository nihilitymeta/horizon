# Mob AI for Mods

This page describes how mods change what mobs do: a hook point that runs as a mob's goals are set up, a goal type
that mods can write without borrowing the mob, and mobs that a content mod declares in its data. It lives on the
`mob-ai-hooks` branch of whos-carmen/horizon, which is `main` plus this one concern. It was written for
horizon-parasite-mod, which needs custom blocks and custom mob behaviour first.

| Part | Status |
|---|---|
| The `mob_ai_register` hook point | built; its API number is not stamped yet |
| Goals for mods: `MobView`, `Actions`, `NativeBehaviour`, `NativeGoal` | built |
| Mobs declared in a content mod's data | built |
| Natural spawning and spawn eggs for those mobs | built |
| Models, animations and sounds for those mobs | built |
| The rest | [planned](#plan) |

## How mobs think

A mob's behaviour is a `GoalSelector` of `Goal`s, plus a second selector of target goals
(`crates/horizon-client/src/mobs/goals.rs`). A goal has `can_use`, `can_continue`, `start`, `tick`, `stop` and the
control flags it locks. `mobs/ai.rs` (`register`) and the family files, such as `mobs/cube.rs` and
`mobs/raider.rs`, fill the selectors for each species. Some species run a brain instead, such as
`villager_brain.rs` and `warden_brain.rs`.

Before this branch, hook points touched mobs only from the outside: `mob_spawned`, `mob_killed`,
`entity_tick_pre` and `entity_tick_post`, the draw hooks `mob_submit` and `mob_layer`, and the helpers
`native_spawn_mob` (with `no_ai`), `native_teleport_mob` and `native_push_mob`. Nothing reached the goals. The
documented way to make a mod's own creature was a vanilla mob, redrawn and steered by hand from a tick hook
([native-mods.md](native-mods.md#mobs-drawn-by-a-mod)).

## The `mob_ai_register` hook

`horizon_client_lib::mobs::ai::native_mob_ai_register(mob: &mut Mob)` is called as a mob's goals are set up. It is
a free function, wired through `native_client_hooks` like `native_living_breathe`.

It runs in `Mob::new`, right after the species' own `register`. At that point the mob's vanilla goals are in
`mob.ai.goals` and `mob.ai.targets`, and nothing has ticked yet. A handler can:

- add a goal, with `mob.ai.goals.add(priority, goal)`
- add a target goal
- clear a selector and fill it again

The hook runs once for each mob, on the server side only. `Mob::new_client` builds the mobs that a client only
draws, such as a server's mob seen over the network, the first-person player model and a spawner's display. It sets
up the species' goals too, but does not call the hook.

Trait mods get `MobAiRegisterEvent { mob }`, and the types `Mob`, `MobType`, `Goal`, `GoalSelector` and `GoalCx`,
through `horizon_client::mod_api`.

The hook's fragment is `tools/native-hook-fragments/mob_ai_register.json`. Its API number is given when the fragment
is stamped with `python3 tools/stamp_native_hooks.py`, which this fork does not do. A bundle built on the fork pins
the fork's commit instead.

## Goals for mods

`Goal` is public through `horizon_client::mod_api`, so a trait mod can implement it and touch everything a vanilla
goal does. Most mods, and every script, don't need that much. `mobs/native_goal.rs` gives a goal a fixed shape that
borrows nothing of the mob.

### `MobView`

A copy of what a goal sees of its mob, made before each call: its id, kind, position, velocity, health, its target
and where the target stands, whether the target is in melee range, whether a path is being walked, the nearest
player, and the mob's home.

`MobView::nearby` lists the other mobs within the mob's follow range, nearest first, up to 32. Each entry is a
`NearbyView`: id, kind (`kind.id` is the type's name), position, distance, health, alive, hostile, baby and target.
An infection can use it to look for hosts, or a brood to find its mother. It reads the tick's spatial grid, so its
cost grows with the mobs around, not with the whole level.

### `Actions` and `GoalAction`

What a goal call asks the game to do. The actions run in order after the call, with the mob and the world at hand:

- moving: `navigate_to`, `navigate_to_target`, `stop_navigation`, `move_toward`, `look_at`, `look_at_target`,
  `jump`
- fighting: `set_target`, `attack`, `swing`, `hurt(who, amount)`
- living: `convert(kind)`, which turns the mob into another kind as a drowning zombie becomes a drowned;
  `summon(kind, at)`, `despawn`, `heal`, and `effect(name, ticks, amplifier)`

The game drops an action it cannot apply, logs a warning, and leaves the mob as it was. This happens for a
position, speed or amount that is not a finite number, an effect that has no definition, and a kind that the level
has no mob type for (`GoalAction::refused`). For `convert` and `summon`, a bare name is vanilla's.

### `NativeBehaviour`

The decisions of a goal: `can_use`, `can_continue`, `start`, `tick` and `stop`, each over a `MobView` and an
`Actions`. One value is shared by every mob that has the goal. The part that belongs to each mob is the
behaviour's `State`, which starts as `Default` when the goal is added.

### `NativeGoal`

`NativeGoal::new(name, flags, behaviour)` creates the `Goal` to add. *flags* are the controls it locks, any of
`GOAL_MOVE`, `GOAL_LOOK`, `GOAL_JUMP` and `GOAL_TARGET`. `.updated_every_tick()` ticks it every tick, and
`.uninterruptable()` keeps a more pressing goal from stopping it.

### Example

This trait mod makes every zombie walk at the nearest player within 24 blocks:

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

The Lua runtime hands a script's goal the same two types ([lua-bindings.md](lua-bindings.md)): the view becomes a
table, and the actions come back from it. Unit tests cover the view and the actions that need no world. The path and
attack actions run only in a game with the vanilla data.

## Mobs declared in data

A content mod declares a mob in `data/<id>/horizon/entity_type/<name>.json`, with a vanilla mob as its `template`
and its own `attributes`, `size`, `texture` and `goals` ([mods.md](mods.md#content-mods-api-02), entity types).

When the game reads the mod:

1. It adds the type's row to `entity-types.json`: the template's row, with the mod's values and a `horizon` object
   that names the template, texture and goals (`mod_content::entity_types_json`).
2. `mobs::MobTypes::load` builds a `MobType` for it from the template's species definition (model, sounds,
   animation, flags and brain), with the mod's id and texture.
3. The texture joins the shared atlas through a `single` atlas source that the content root writes
   (`assets/<id>/atlases/horizon_entities.json`).

Wherever the species decides (`Mob::species`), the mob is its template's species: AI families, conversion, burning
and breeding.

With `"goals": "none"`, `Mob::new` clears both goal selectors after the species has filled them. `mob_ai_register`,
and a script's `horizon.add_goal`, then see empty selectors with the species' controls.

The mob drops its own loot table, `<id>:entities/<name>`, when the table exists, and the template's otherwise.

## Natural spawning and spawn eggs

A content mob spawns on its own where a NeoForge biome modifier of type `neoforge:add_spawns` puts it, in
`data/<id>/neoforge/biome_modifier/<name>.json`:

- `biomes` is a biome, a tag or a list.
- `spawners` holds entries with `type`, `weight`, `minCount` and `maxCount`, as NeoForge writes them.

Each entry joins the natural spawns of those biomes, at the end of its kind's spawn category, next to vanilla's. The
category is the kind's own; for a mod's mob, that is its template's category, from `entity-types.json`. Upstream's
biome modifiers do this (`mod_content_biomes.rs`, which also reads `remove_spawns`, `add_spawn_costs` and
`remove_spawn_costs`); this branch puts the mod's mobs in `entity-types.json` before the modifiers run, so they
find them. `add_carvers` and the other modifier types are still skipped, with a warning.

A spawn egg is any item whose id ends in `_spawn_egg`. The kind is the id before that suffix, so
`<id>:crawler_spawn_egg` (template `minecraft:zombie_spawn_egg`, with a texture of its own) spawns `<id>:crawler`, as
vanilla's eggs do, on a spawner too.

## Models, animations and sounds

A content mob's `entity_type` can name a look of its own. Each part falls back to the template's.

- `model`: `<id>:entity/<name>`, which is `assets/<id>/models/entity/<name>.json`. It is a tree of parts holding
  cubes, written as a vanilla `LayerDefinition` is, and as Blockbench exports it: a `texture_size`, and `parts` by
  name, each with an `offset`, a `rotation` in degrees, `cubes` (each with `origin`, `size`, `uv`, `inflate` and
  `mirror`) and `children`. The texture is laid out as the model's `uv`s say.
- `animation`: a set that moves the parts by their vanilla names, such as `zombie`, `skeleton`, `quadruped`,
  `iron_golem` or `spider`. Every set the game has is available, in snake case.
- `sounds`: sound events by `ambient`, `hurt`, `death` and `step`.

The content check, run by `cargo horizon-mod check` and by the game as it reads a mod, refuses:

- a `model` that is not `<id>:entity/<name>` with the file in the mod (`horizon_mods::content::mob_model_file`)
- an `animation` that is not one of the game's sets (`MOB_ANIMATIONS`)
- a sound under any other key, or a sound without a namespace

## Plan

| Step | What | Why |
|---|---|---|
| 2 | The helpers `native_set_mob_target(dimension, id, target)` and `native_mob_navigate_to(dimension, id, pos, speed)` | A tick hook could drive a mob that it did not register a goal on. The goal adapter above covers the parasite mod for now |
| 4 | A `goals` list that names vanilla goals, with their arguments. Mobs declared in data, natural spawning and spawn eggs are built | A parasite is declared in data, as a block is |
| 5 | A `mob_ai_tick(mob)` listen hook on each AI tick, and `mob_target_changed` | Scripts that react, such as an infection spreading or a host converting, without a goal |
| 6 | For species with a brain: a hook after the brain's activities are built | Villagers, piglins and wardens use brains, not goals |

Each step is one fragment and one hook or helper. Hooks are named by the NeoForge rules in
[native-hooks-neoforge.md](native-hooks-neoforge.md) where NeoForge has an event. It has none for `registerGoals`,
so `mob_ai_register` keeps a Horizon name.
