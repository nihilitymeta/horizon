//! Read-only observation of server entities for the developer tools
//! (docs/dev-tools.md). The commands name what to watch and how
//! ([`View`]): `/inspect` the data `/data get entity` prints, `/goals` a
//! mob's goal selectors with their history, `/senses` what a mob perceives,
//! `/nav` its path, `/watch` a numeric NBT path over time. Each server tick
//! the game stores the data ([`set_data`]) and the watched values
//! ([`set_sample`]), the AI step snapshots a watched mob after its goals
//! ticked ([`observe`]), and the egui windows in the executable read it all
//! ([`snapshots`]). Nothing here changes an entity. The types, the store and
//! the wire live in the `event-horizon-observe` crate; this module keeps the
//! game's own part: the snapshot a mob's AI step takes, and the names the
//! game's callers use.
use super::{Mob, MobWorld};
pub use event_horizon_observe::*;
use std::time::Instant;

fn rows(selector: &super::goals::GoalSelector) -> Vec<GoalRow> {
    selector
        .entries()
        .map(|(priority, name, running, flags)| GoalRow {
            priority,
            name,
            running,
            flags,
        })
        .collect()
}

/// Called by the server's AI step for every mob, after its goals ticked:
/// snapshots the mob when it is watched. Nothing is touched.
pub fn observe(m: &Mob, w: &MobWorld) {
    observe_entity(m.id, m.age, || {
        let ai = &m.ai;
        let position = [m.position.x, m.position.y, m.position.z];
        BrainSnapshot {
            id: m.id,
            kind: m.kind.id.clone(),
            species: format!("{:?}", m.species()),
            custom_name: m.custom_name.clone(),
            game_time: w.game_time,
            position: [m.position.x, m.position.y, m.position.z],
            velocity: [m.velocity.x, m.velocity.y, m.velocity.z],
            health: m.health,
            max_health: m.attrs.max_health,
            age: m.age,
            alive: m.alive(),
            no_ai: m.no_ai,
            target: ai.target.map(|who| format!("{who:?}")),
            last_hurt_by: ai.last_hurt_by.map(|who| format!("{who:?}")),
            last_hurt_by_time: ai.last_hurt_by_time,
            aggressive: ai.aggressive,
            panicking: ai.panicking,
            no_action_time: ai.no_action_time,
            home: ai.home,
            follow_range: m.attrs.follow_range,
            seen: ai
                .seen_cache
                .iter()
                .map(|(who, seen)| (format!("{who:?}"), *seen))
                .collect(),
            nearest_player: w.nearest_player.map(|p| {
                let d = ((p.x - position[0]).powi(2)
                    + (p.y - position[1]).powi(2)
                    + (p.z - position[2]).powi(2))
                .sqrt();
                ([p.x, p.y, p.z], d)
            }),
            nav_kind: format!("{:?}", ai.nav.kind),
            nav_target: ai.nav.target_pos,
            nav_speed: ai.nav.speed_modifier,
            nav_stuck: ai.nav.stuck,
            path: ai.nav.path.as_ref().map(|p| PathSnapshot {
                nodes: p.nodes.iter().map(|n| [n.x, n.y, n.z]).collect(),
                next: p.next,
                target: p.target,
                reached: p.reached,
            }),
            move_operation: format!("{:?}", ai.mov.operation),
            move_wanted: ai.mov.wanted,
            input: [ai.xxa, ai.yya, ai.zza],
            speed: ai.speed,
            goals: rows(&ai.goals),
            targets: rows(&ai.targets),
            taken: Instant::now(),
        }
    });
}
