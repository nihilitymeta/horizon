//! Mob goals written in Lua. A script defines one with `horizon.goal(name,
//! def)`, where `def` holds `flags` (`{"move", "look", "jump", "target"}`),
//! `every_tick`, `interruptable` and the functions `can_use`, `can_continue`,
//! `start`, `tick` and `stop`, each called as `f(mob, state, out)`: `mob` is
//! the [`MobView`] as a table, `state` a table of the goal's own that lives as
//! long as the mob, and `out` the actions to ask for (`out.navigate_to(x, y,
//! z, speed)`...). Then a `mob_ai_register` handler attaches it:
//! `horizon.add_goal(priority, name)`. The game runs it through
//! [`NativeGoal`], as it runs a trait mod's.
use crate::mod_api::{
    Actions, GOAL_JUMP, GOAL_LOOK, GOAL_MOVE, GOAL_TARGET, GoalAction, MobView, NativeBehaviour,
    NativeGoal, Who,
};
use mlua::{Function, Lua, RegistryKey, Table, Value};
use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex};

/// The methods of a call's `out` table, each queuing `{name, args...}`.
const ACTIONS: [&str; 16] = [
    "navigate_to",
    "navigate_to_target",
    "stop_navigation",
    "move_toward",
    "look_at",
    "look_at_target",
    "set_target",
    "jump",
    "attack",
    "swing",
    "hurt",
    "convert",
    "summon",
    "despawn",
    "heal",
    "effect",
];

/// A goal a script defined with `horizon.goal(name, def)`.
#[derive(Clone, Debug)]
pub struct GoalDef {
    pub name: &'static str,
    pub flags: u8,
    pub every_tick: bool,
    pub interruptable: bool,
    /// The `def` table.
    def: Arc<RegistryKey>,
}
impl GoalDef {
    /// Reads `def`; `name` is the goal's name for F3 and the log, kept for
    /// the game's life (a mod defines a handful of goals, once).
    pub fn parse(lua: &Lua, name: &'static str, def: Table) -> mlua::Result<Self> {
        let mut flags = 0;
        if let Some(list) = def.get::<Option<Table>>("flags")? {
            for flag in list.sequence_values::<String>() {
                flags |= match flag?.as_str() {
                    "move" => GOAL_MOVE,
                    "look" => GOAL_LOOK,
                    "jump" => GOAL_JUMP,
                    "target" => GOAL_TARGET,
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "goal {name}: flag {other:?} is not move, look, jump or target"
                        )));
                    }
                };
            }
        }
        for key in ["can_use", "can_continue", "start", "tick", "stop"] {
            let v: Value = def.get(key)?;
            if !v.is_nil() && !v.is_function() {
                return Err(mlua::Error::runtime(format!(
                    "goal {name}: {key} is not a function"
                )));
            }
        }
        if def.get::<Value>("can_use")?.is_nil() {
            return Err(mlua::Error::runtime(format!(
                "goal {name}: can_use(mob, state, out) is required"
            )));
        }
        Ok(GoalDef {
            name,
            flags,
            every_tick: def.get::<Option<bool>>("every_tick")?.unwrap_or(false),
            interruptable: def.get::<Option<bool>>("interruptable")?.unwrap_or(true),
            def: Arc::new(lua.create_registry_value(def)?),
        })
    }
    /// The goal's decisions, running in `lua`: what [`GoalDef::goal`] wraps.
    pub fn behaviour(
        &self,
        lua: Lua,
        mod_id: &str,
        budget: super::Budget,
    ) -> mlua::Result<LuaBehaviour> {
        let (api, queue) = actions_table(&lua)?;
        Ok(LuaBehaviour {
            api: lua.create_registry_value(api)?,
            queue: lua.create_registry_value(queue)?,
            lua,
            mod_id: mod_id.to_owned(),
            budget,
            def: self.clone(),
            reported: Mutex::new(HashSet::new()),
        })
    }
    /// The goal to add to a mob, running in `lua`.
    pub fn goal(
        &self,
        lua: Lua,
        mod_id: &str,
        budget: super::Budget,
    ) -> mlua::Result<NativeGoal<LuaBehaviour>> {
        let behaviour = self.behaviour(lua, mod_id, budget)?;
        let mut goal = NativeGoal::new(self.name, self.flags, behaviour);
        if self.every_tick {
            goal = goal.updated_every_tick();
        }
        if !self.interruptable {
            goal = goal.uninterruptable();
        }
        Ok(goal)
    }
}

/// The per-mob `state` table of a Lua goal, made on its first call.
#[derive(Clone, Debug, Default)]
pub struct LuaGoalState {
    table: Option<Arc<RegistryKey>>,
}
impl LuaGoalState {
    /// Whether a call has made the table yet.
    pub fn started(&self) -> bool {
        self.table.is_some()
    }
}

/// A Lua goal's decisions: the `def` functions, called with the mob's view.
pub struct LuaBehaviour {
    lua: Lua,
    mod_id: String,
    /// The mod's deadline for the state's interrupt: each call gets `HANDLER_BUDGET`.
    budget: super::Budget,
    def: GoalDef,
    /// The `out` table handed to each call and the queue its methods fill.
    api: RegistryKey,
    queue: RegistryKey,
    /// The functions whose first error was logged: later ones are debug lines.
    reported: Mutex<HashSet<&'static str>>,
}
impl fmt::Debug for LuaBehaviour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LuaBehaviour")
            .field("mod", &self.mod_id)
            .field("goal", &self.def.name)
            .finish()
    }
}
impl LuaBehaviour {
    /// Calls `def.<which>` if the script defined it: its truthiness, or
    /// `None` without the function.
    fn try_call(
        &self,
        which: &str,
        state: &mut LuaGoalState,
        mob: &MobView,
        out: &mut Actions,
    ) -> mlua::Result<Option<bool>> {
        let lua = &self.lua;
        let def: Table = lua.registry_value(&self.def.def)?;
        let Some(f) = def.get::<Option<Function>>(which)? else {
            return Ok(None);
        };
        let st: Table = match &state.table {
            Some(key) => lua.registry_value(key)?,
            None => {
                let t = lua.create_table()?;
                state.table = Some(Arc::new(lua.create_registry_value(&t)?));
                t
            }
        };
        let view = view_table(lua, mob)?;
        let api: Table = lua.registry_value(&self.api)?;
        let queue: Table = lua.registry_value(&self.queue)?;
        let r: Value = self
            .budget
            .run(super::HANDLER_BUDGET, || f.call((view, st, api)))?;
        let drained = drain_actions(&queue, out);
        queue.clear()?;
        drained?;
        Ok(Some(!matches!(r, Value::Nil | Value::Boolean(false))))
    }
    /// `try_call`, with `fallback` when the function is not defined and
    /// `default` when neither is; an error is logged and reads as `default`.
    fn call(
        &self,
        which: &'static str,
        fallback: Option<&'static str>,
        state: &mut LuaGoalState,
        mob: &MobView,
        out: &mut Actions,
        default: bool,
    ) -> bool {
        let r = match self.try_call(which, state, mob, out) {
            Ok(Some(b)) => return b,
            Ok(None) => match fallback {
                Some(f) => self.try_call(f, state, mob, out),
                None => return default,
            },
            Err(e) => Err(e),
        };
        match r {
            Ok(Some(b)) => b,
            Ok(None) => default,
            Err(e) => {
                let first = self.reported.lock().map(|mut s| s.insert(which));
                let line = format!("[lua:{}] goal {} {which}: {e}", self.mod_id, self.def.name);
                if first.unwrap_or(true) {
                    log::error!("{line}");
                } else {
                    log::debug!("{line}");
                }
                default
            }
        }
    }
}
impl NativeBehaviour for LuaBehaviour {
    type State = LuaGoalState;
    fn can_use(&self, state: &mut LuaGoalState, mob: &MobView, out: &mut Actions) -> bool {
        self.call("can_use", None, state, mob, out, false)
    }
    fn can_continue(&self, state: &mut LuaGoalState, mob: &MobView, out: &mut Actions) -> bool {
        self.call("can_continue", Some("can_use"), state, mob, out, false)
    }
    fn start(&self, state: &mut LuaGoalState, mob: &MobView, out: &mut Actions) {
        self.call("start", None, state, mob, out, false);
    }
    fn tick(&self, state: &mut LuaGoalState, mob: &MobView, out: &mut Actions) {
        self.call("tick", None, state, mob, out, false);
    }
    fn stop(&self, state: &mut LuaGoalState, mob: &MobView, out: &mut Actions) {
        self.call("stop", None, state, mob, out, false);
    }
}

/// A position as `{x, y, z}` fields.
pub fn vec3(lua: &Lua, p: [f64; 3]) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("x", p[0])?;
    t.set("y", p[1])?;
    t.set("z", p[2])?;
    Ok(t)
}
fn vec3_of(t: &Table) -> mlua::Result<[f64; 3]> {
    Ok([t.get("x")?, t.get("y")?, t.get("z")?])
}
/// `Who` in Lua: `"player"`, a mob id, or nil for none.
pub fn who_to_lua(lua: &Lua, who: Option<Who>) -> mlua::Result<Value> {
    Ok(match who {
        None => Value::Nil,
        Some(Who::Player) => Value::String(lua.create_string("player")?),
        Some(Who::Mob(id)) => Value::Integer(id as i64),
    })
}
fn who_from_lua(v: Value) -> mlua::Result<Option<Who>> {
    match v {
        Value::Nil => Ok(None),
        Value::Integer(id) => u32::try_from(id)
            .map(|id| Some(Who::Mob(id)))
            .map_err(|_| mlua::Error::runtime(format!("{id} is not a mob id"))),
        Value::Number(n) if n.fract() == 0.0 && n >= 0.0 => Ok(Some(Who::Mob(n as u32))),
        Value::String(s) if s.to_str().is_ok_and(|s| &*s == "player") => Ok(Some(Who::Player)),
        other => Err(mlua::Error::runtime(format!(
            "{} is not \"player\", a mob id or nil",
            other.type_name()
        ))),
    }
}

/// The [`MobView`] as a table: `id`, `kind`, `position`, `velocity`, `yaw`,
/// `head_pitch`, `health`, `max_health`, `age`, `on_ground`, `in_water`,
/// `in_lava`, `hurt_time`, `last_hurt_by`, `target`, `target_position`,
/// `target_in_melee_range`, `navigation_done`, `nearest_player`, `home`.
pub fn view_table(lua: &Lua, v: &MobView) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", v.id)?;
    t.set("kind", v.kind.id.as_str())?;
    t.set("position", vec3(lua, v.position)?)?;
    t.set("velocity", vec3(lua, v.velocity)?)?;
    t.set("yaw", v.yaw)?;
    t.set("head_pitch", v.head_pitch)?;
    t.set("health", v.health)?;
    t.set("max_health", v.max_health)?;
    t.set("age", v.age)?;
    t.set("on_ground", v.on_ground)?;
    t.set("in_water", v.in_water)?;
    t.set("in_lava", v.in_lava)?;
    t.set("hurt_time", v.hurt_time)?;
    t.set("last_hurt_by", who_to_lua(lua, v.last_hurt_by)?)?;
    t.set("target", who_to_lua(lua, v.target)?)?;
    if let Some(p) = v.target_position {
        t.set("target_position", vec3(lua, p)?)?;
    }
    t.set("target_in_melee_range", v.target_in_melee_range)?;
    t.set("navigation_done", v.navigation_done)?;
    if let Some(p) = v.nearest_player {
        t.set("nearest_player", vec3(lua, p)?)?;
    }
    if let Some((c, r)) = v.home {
        let home = vec3(lua, [c[0] as f64, c[1] as f64, c[2] as f64])?;
        home.set("radius", r)?;
        t.set("home", home)?;
    }
    // The mobs around, nearest first: `mob.nearby[1].kind`, `.distance`...
    let nearby = lua.create_table()?;
    for (i, n) in v.nearby.iter().enumerate() {
        let row = lua.create_table()?;
        row.set("id", n.id)?;
        row.set("kind", n.kind.as_str())?;
        row.set("position", vec3(lua, n.position)?)?;
        row.set("distance", n.distance)?;
        row.set("health", n.health)?;
        row.set("alive", n.alive)?;
        row.set("hostile", n.hostile)?;
        row.set("baby", n.baby)?;
        row.set("target", who_to_lua(lua, n.target)?)?;
        nearby.set(i + 1, row)?;
    }
    t.set("nearby", nearby)?;
    Ok(t)
}

/// The `out` table of a goal call and the queue its methods fill with
/// `{name, args...}` rows.
pub fn actions_table(lua: &Lua) -> mlua::Result<(Table, Table)> {
    let api = lua.create_table()?;
    let queue = lua.create_table()?;
    for name in ACTIONS {
        let queue = queue.clone();
        api.set(
            name,
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let row = lua.create_table()?;
                row.push(name)?;
                for v in args {
                    row.push(v)?;
                }
                queue.push(row)
            })?,
        )?;
    }
    Ok((api, queue))
}

/// Moves the queued rows into `out`, in order.
pub fn drain_actions(queue: &Table, out: &mut Actions) -> mlua::Result<()> {
    for row in queue.sequence_values::<Table>() {
        let row = row?;
        let name: String = row.get(1)?;
        let num = |i: i64| -> mlua::Result<f64> { row.get::<f64>(i) };
        let at = |i: i64| -> mlua::Result<[f64; 3]> { Ok([num(i)?, num(i + 1)?, num(i + 2)?]) };
        let speed = |i: i64| -> mlua::Result<f64> { Ok(row.get::<Option<f64>>(i)?.unwrap_or(1.0)) };
        out.push(match name.as_str() {
            "navigate_to" => GoalAction::NavigateTo {
                at: at(2)?,
                speed: speed(5)?,
            },
            "navigate_to_target" => GoalAction::NavigateToTarget { speed: speed(2)? },
            "stop_navigation" => GoalAction::StopNavigation,
            "move_toward" => GoalAction::MoveToward {
                at: at(2)?,
                speed: speed(5)?,
            },
            "look_at" => GoalAction::LookAt { at: at(2)? },
            "look_at_target" => GoalAction::LookAtTarget,
            "set_target" => GoalAction::SetTarget(who_from_lua(row.get(2)?)?),
            "jump" => GoalAction::Jump,
            "attack" => match who_from_lua(row.get(2)?)? {
                Some(who) => GoalAction::Attack(who),
                None => {
                    return Err(mlua::Error::runtime("attack(who): who is required"));
                }
            },
            "swing" => GoalAction::Swing,
            "hurt" => match who_from_lua(row.get(2)?)? {
                Some(who) => GoalAction::Hurt {
                    who,
                    amount: num(3)? as f32,
                },
                None => {
                    return Err(mlua::Error::runtime("hurt(who, amount): who is required"));
                }
            },
            "convert" => GoalAction::Convert { kind: row.get(2)? },
            "summon" => GoalAction::Summon {
                kind: row.get(2)?,
                at: at(3)?,
            },
            "despawn" => GoalAction::Despawn,
            "heal" => GoalAction::Heal(num(2)? as f32),
            "effect" => GoalAction::Effect {
                effect: row.get(2)?,
                ticks: row.get::<Option<i32>>(3)?.unwrap_or(200),
                amplifier: row.get::<Option<i32>>(4)?.unwrap_or(0),
            },
            other => {
                return Err(mlua::Error::runtime(format!("unknown action {other}")));
            }
        });
    }
    Ok(())
}

/// `horizon.distance(a, b)`: between two `{x, y, z}` tables.
pub fn distance(_: &Lua, (a, b): (Table, Table)) -> mlua::Result<f64> {
    let (a, b) = (vec3_of(&a)?, vec3_of(&b)?);
    Ok(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_reads_its_flags_and_needs_can_use() {
        let lua = Lua::new();
        let def: Table = lua
            .load(r#"{ flags = {"move", "look"}, every_tick = true, can_use = function() return true end }"#)
            .eval()
            .unwrap();
        let g = GoalDef::parse(&lua, "stalk", def).unwrap();
        assert_eq!(
            (g.flags, g.every_tick, g.interruptable),
            (GOAL_MOVE | GOAL_LOOK, true, true)
        );
        let bad: Table = lua.load(r#"{ tick = function() end }"#).eval().unwrap();
        let e = GoalDef::parse(&lua, "idle", bad).unwrap_err().to_string();
        assert!(e.contains("can_use"), "{e}");
        let bad: Table = lua
            .load(r#"{ flags = {"fly"}, can_use = function() end }"#)
            .eval()
            .unwrap();
        let e = GoalDef::parse(&lua, "fly", bad).unwrap_err().to_string();
        assert!(e.contains("fly"), "{e}");
    }

    #[test]
    fn the_out_table_queues_actions_in_order_with_lua_targets() {
        let lua = Lua::new();
        let (api, queue) = actions_table(&lua).unwrap();
        lua.globals().set("out", api).unwrap();
        lua.load(
            r#"
            out.look_at(1, 2, 3)
            out.navigate_to(4, 5, 6, 1.25)
            out.navigate_to_target()
            out.set_target("player")
            out.attack(12)
            out.set_target(nil)
            out.jump()
            "#,
        )
        .exec()
        .unwrap();
        let mut out = Actions::default();
        drain_actions(&queue, &mut out).unwrap();
        assert_eq!(
            out.as_slice(),
            [
                GoalAction::LookAt {
                    at: [1.0, 2.0, 3.0]
                },
                GoalAction::NavigateTo {
                    at: [4.0, 5.0, 6.0],
                    speed: 1.25
                },
                GoalAction::NavigateToTarget { speed: 1.0 },
                GoalAction::SetTarget(Some(Who::Player)),
                GoalAction::Attack(Who::Mob(12)),
                GoalAction::SetTarget(None),
                GoalAction::Jump,
            ]
        );
        let bad: Table = lua.load(r#"{ {"attack"} }"#).eval().unwrap();
        assert!(drain_actions(&bad, &mut Actions::default()).is_err());
    }

    #[test]
    fn distance_is_euclidean() {
        let lua = Lua::new();
        lua.globals()
            .set("distance", lua.create_function(distance).unwrap())
            .unwrap();
        let d: f64 = lua
            .load("return distance({x=0,y=0,z=0}, {x=3,y=4,z=12})")
            .eval()
            .unwrap();
        assert_eq!(d, 13.0);
    }
}
