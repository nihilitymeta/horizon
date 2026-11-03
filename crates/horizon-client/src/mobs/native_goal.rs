//! Goals written outside this crate: a trait mod or a script sees its mob as a
//! [`MobView`] and answers with [`GoalAction`]s, which the game applies with the
//! mob and the world after the call. Nothing of the mob's own borrows crosses
//! into the mod, so the same shape serves Rust and a scripting runtime, and a
//! goal keeps working while the mob's internals move. A mod that wants the
//! internals implements [`Goal`](super::goals::Goal) directly instead.
use super::goals::{Cx, Goal};
use super::{Mob, MobWorld, Who};
use std::fmt;
use std::sync::Arc;

/// What a goal outside the crate sees of its mob on each call. Fields that need
/// the world (`target_position`, `player`) are `None` when a call has none.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MobView {
    pub id: u32,
    /// The mob's type (`kind.id` is `minecraft:zombie`, or a content mod's own id).
    pub kind: Arc<super::MobType>,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub head_pitch: f32,
    pub health: f32,
    pub max_health: f32,
    pub age: u32,
    pub on_ground: bool,
    pub in_water: bool,
    pub in_lava: bool,
    /// Ticks of the hurt animation left: above zero right after a hit.
    pub hurt_time: i32,
    pub last_hurt_by: Option<Who>,
    pub target: Option<Who>,
    /// Where the target stands, while there is one the world knows.
    pub target_position: Option<[f64; 3]>,
    pub target_in_melee_range: bool,
    /// No path is being walked.
    pub navigation_done: bool,
    /// The nearest player of the level (`getNearestPlayer(mob, -1)`), alive or not.
    pub nearest_player: Option<[f64; 3]>,
    /// The home position and radius a mob is restricted to, if any.
    pub home: Option<([i32; 3], i32)>,
    /// The other mobs within the mob's follow range, nearest first (at most
    /// [`NEARBY_LIMIT`]); empty without the world.
    pub nearby: Vec<NearbyView>,
}

/// How many nearby mobs a view lists.
pub const NEARBY_LIMIT: usize = 32;

/// Another mob near the viewed one, as the world saw it this tick.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct NearbyView {
    pub id: u32,
    /// `minecraft:zombie`, or a content mod's own id.
    pub kind: String,
    pub position: [f64; 3],
    /// Straight-line distance from the viewed mob.
    pub distance: f64,
    pub health: f32,
    pub alive: bool,
    pub hostile: bool,
    pub baby: bool,
    pub target: Option<Who>,
}

impl MobView {
    /// The view of `m`; with `w`, the fields that need the world too.
    pub fn of(m: &Mob, w: Option<&MobWorld>) -> Self {
        let target =
            m.ai.target
                .and_then(|who| w.and_then(|w| w.target_info(who)));
        MobView {
            id: m.id,
            kind: m.kind.clone(),
            position: [m.position.x, m.position.y, m.position.z],
            velocity: [m.velocity.x, m.velocity.y, m.velocity.z],
            yaw: m.yaw,
            head_pitch: m.head_pitch,
            health: m.health,
            max_health: m.kind.max_health,
            age: m.age,
            on_ground: m.on_ground,
            in_water: m.fluids.in_water,
            in_lava: m.fluids.in_lava,
            hurt_time: m.hurt_time,
            last_hurt_by: m.ai.last_hurt_by,
            target: m.ai.target,
            target_position: target
                .as_ref()
                .map(|t| [t.position.x, t.position.y, t.position.z]),
            target_in_melee_range: target.as_ref().is_some_and(|t| m.within_melee_range(t)),
            navigation_done: m.ai.nav.is_done(),
            nearest_player: w.and_then(|w| w.nearest_player).map(|p| [p.x, p.y, p.z]),
            home: m.ai.home,
            nearby: w.map(|w| nearby(m, w)).unwrap_or_default(),
        }
    }
    /// Straight-line distance from the mob to `at`.
    pub fn distance_to(&self, at: [f64; 3]) -> f64 {
        let [x, y, z] = self.position;
        ((at[0] - x).powi(2) + (at[1] - y).powi(2) + (at[2] - z).powi(2)).sqrt()
    }
}

/// The mobs within `m`'s follow range as the world saw them, nearest first.
fn nearby(m: &Mob, w: &MobWorld) -> Vec<NearbyView> {
    let range = m.kind.follow_range.max(1.0);
    let mut out: Vec<NearbyView> = w
        .others
        .iter()
        .filter(|s| s.id != m.id)
        .filter_map(|s| {
            let (dx, dy, dz) = (s.position.x - m.position.x, s.position.y - m.position.y, s.position.z - m.position.z);
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            (distance <= range).then(|| NearbyView {
                id: s.id,
                kind: s.kind.id.clone(),
                position: [s.position.x, s.position.y, s.position.z],
                distance,
                health: s.health,
                alive: s.alive,
                hostile: s.hostile,
                baby: s.baby,
                target: s.target,
            })
        })
        .collect();
    out.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    out.truncate(NEARBY_LIMIT);
    out
}

/// A kind or effect name as the game's events carry it: one static string
/// per distinct name for the process's life.
fn intern(name: &str) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<std::collections::HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// What a goal outside the crate asks of its mob. Applied in order after the
/// goal's method returns; an action that needs the world is dropped by a call
/// that has none.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum GoalAction {
    /// Find a path to `at` and walk it (`PathNavigation.moveTo`).
    NavigateTo {
        at: [f64; 3],
        speed: f64,
    },
    /// Find a path to the current target and walk it.
    NavigateToTarget {
        speed: f64,
    },
    StopNavigation,
    /// Steer straight at `at` without a path (`MoveControl.setWantedPosition`).
    MoveToward {
        at: [f64; 3],
        speed: f64,
    },
    /// Turn the head to `at` at the species' speed.
    LookAt {
        at: [f64; 3],
    },
    LookAtTarget,
    SetTarget(Option<Who>),
    Jump,
    /// Hit `who` with the mob's attack, as the species does (`Mob.doHurtTarget`), swinging.
    Attack(Who),
    /// Swing the arm only.
    Swing,
    /// Deal `amount` of mob-attack damage to `who`, with no swing and no
    /// species rules (an infection's bite, a touch).
    Hurt {
        who: Who,
        amount: f32,
    },
    /// Become a mob of `kind` (`Mob.convertTo`, equipment and name kept):
    /// this mob is discarded and the new one takes its place.
    Convert {
        kind: String,
    },
    /// Make a mob of `kind` at `at` (a conversion's spawn, a split, a brood).
    Summon {
        kind: String,
        at: [f64; 3],
    },
    /// Vanish without dying: no drops, no death event (`Entity.discard`).
    Despawn,
    /// Heal by `amount`, up to the mob's max health.
    Heal(f32),
    /// Give the mob an effect (`minecraft:poison`) for `ticks` at `amplifier`.
    Effect {
        effect: String,
        ticks: i32,
        amplifier: i32,
    },
}

/// The actions a goal's method collects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Actions(Vec<GoalAction>);
impl Actions {
    pub fn push(&mut self, action: GoalAction) {
        self.0.push(action);
    }
    pub fn navigate_to(&mut self, at: [f64; 3], speed: f64) {
        self.push(GoalAction::NavigateTo { at, speed });
    }
    pub fn navigate_to_target(&mut self, speed: f64) {
        self.push(GoalAction::NavigateToTarget { speed });
    }
    pub fn stop_navigation(&mut self) {
        self.push(GoalAction::StopNavigation);
    }
    pub fn move_toward(&mut self, at: [f64; 3], speed: f64) {
        self.push(GoalAction::MoveToward { at, speed });
    }
    pub fn look_at(&mut self, at: [f64; 3]) {
        self.push(GoalAction::LookAt { at });
    }
    pub fn look_at_target(&mut self) {
        self.push(GoalAction::LookAtTarget);
    }
    pub fn set_target(&mut self, who: Option<Who>) {
        self.push(GoalAction::SetTarget(who));
    }
    pub fn jump(&mut self) {
        self.push(GoalAction::Jump);
    }
    pub fn hurt(&mut self, who: Who, amount: f32) {
        self.push(GoalAction::Hurt { who, amount });
    }
    pub fn convert(&mut self, kind: impl Into<String>) {
        self.push(GoalAction::Convert { kind: kind.into() });
    }
    pub fn summon(&mut self, kind: impl Into<String>, at: [f64; 3]) {
        self.push(GoalAction::Summon { kind: kind.into(), at });
    }
    pub fn despawn(&mut self) {
        self.push(GoalAction::Despawn);
    }
    pub fn heal(&mut self, amount: f32) {
        self.push(GoalAction::Heal(amount));
    }
    pub fn effect(&mut self, effect: impl Into<String>, ticks: i32, amplifier: i32) {
        self.push(GoalAction::Effect { effect: effect.into(), ticks, amplifier });
    }
    pub fn attack(&mut self, who: Who) {
        self.push(GoalAction::Attack(who));
    }
    pub fn swing(&mut self) {
        self.push(GoalAction::Swing);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn as_slice(&self) -> &[GoalAction] {
        &self.0
    }
    /// Applies the actions to `m`, in order, dropping those that need a world
    /// when `cx` is `None`.
    pub fn apply(self, m: &mut Mob, mut cx: Option<&mut Cx>) {
        for action in self.0 {
            match action {
                GoalAction::NavigateTo { at, speed } => {
                    if let Some(cx) = cx.as_deref_mut() {
                        m.nav_move_to(cx.w, at, speed);
                    }
                }
                GoalAction::NavigateToTarget { speed } => {
                    if let Some(cx) = cx.as_deref_mut()
                        && let Some(t) = m.target_info(cx.w)
                    {
                        let at = [t.position.x, t.position.y, t.position.z];
                        m.nav_move_to_entity(cx.w, at, 0, speed);
                    }
                }
                GoalAction::StopNavigation => m.nav_stop(),
                GoalAction::MoveToward { at, speed } => {
                    m.ai.mov.set_wanted(at[0], at[1], at[2], speed)
                }
                GoalAction::LookAt { at } => m.look_at(at),
                GoalAction::LookAtTarget => {
                    if let Some(cx) = cx.as_deref_mut()
                        && let Some(t) = m.target_info(cx.w)
                    {
                        m.look_at([t.position.x, t.eye_y, t.position.z]);
                    }
                }
                GoalAction::SetTarget(who) => m.ai.target = who,
                GoalAction::Jump => m.ai.jump = true,
                GoalAction::Attack(who) => {
                    if let Some(cx) = cx.as_deref_mut() {
                        m.swing();
                        super::ai::do_hurt_target(m, who, cx);
                    }
                }
                GoalAction::Swing => m.swing(),
                GoalAction::Hurt { who, amount } => {
                    if let Some(cx) = cx.as_deref_mut() {
                        let from = [m.position.x, m.position.y, m.position.z];
                        cx.events.push(match who {
                            Who::Player => super::MobEvent::DamagePlayer {
                                amount,
                                from,
                                kind: "minecraft:mob_attack",
                            },
                            Who::Mob(target) => super::MobEvent::AttackMob {
                                target,
                                amount,
                                source: super::Damage {
                                    kind: super::DamageKind::MobAttack,
                                    attacker: Some(Who::Mob(m.id)),
                                    from: Some(from),
                                },
                            },
                        });
                    }
                }
                GoalAction::Convert { kind } => {
                    if let Some(cx) = cx.as_deref_mut() {
                        cx.events.push(super::MobEvent::Convert {
                            from: Box::new(m.clone()),
                            kind: intern(&kind),
                            params: super::conversion::Params::KEEP_ALL,
                        });
                        m.removed = true;
                    }
                }
                GoalAction::Summon { kind, at } => {
                    if let Some(cx) = cx.as_deref_mut() {
                        cx.events.push(super::MobEvent::SpawnKind {
                            kind,
                            at,
                            setup: super::SpawnSetup {
                                yaw: m.yaw,
                                ..Default::default()
                            },
                        });
                    }
                }
                GoalAction::Despawn => m.removed = true,
                GoalAction::Heal(amount) => {
                    m.health = (m.health + amount.max(0.0)).min(m.attrs.max_health);
                }
                GoalAction::Effect {
                    effect,
                    ticks,
                    amplifier,
                } => m.add_effect(intern(&effect), ticks, amplifier),
            }
        }
    }
}

/// A goal's decisions, written outside the crate. One value is shared by every
/// mob the goal is added to; what differs per mob lives in `State`, which starts
/// as its `Default` when the goal is added.
pub trait NativeBehaviour: Send + Sync + fmt::Debug + 'static {
    type State: Clone + Default + Send + Sync + fmt::Debug + 'static;
    /// `Goal.canUse`: whether to start, asked while the goal is not running.
    fn can_use(&self, state: &mut Self::State, mob: &MobView, out: &mut Actions) -> bool;
    /// `Goal.canContinueToUse`: whether to keep running. `can_use` again by default.
    fn can_continue(&self, state: &mut Self::State, mob: &MobView, out: &mut Actions) -> bool {
        self.can_use(state, mob, out)
    }
    fn start(&self, _state: &mut Self::State, _mob: &MobView, _out: &mut Actions) {}
    fn tick(&self, _state: &mut Self::State, _mob: &MobView, _out: &mut Actions) {}
    fn stop(&self, _state: &mut Self::State, _mob: &MobView, _out: &mut Actions) {}
}

/// A [`Goal`] made of a [`NativeBehaviour`]: add it to a mob's goals or targets
/// from the `mob_ai_register` hook.
///
/// ```ignore
/// event.mob.ai.goals.add(2, NativeGoal::new("Stalk", GOAL_MOVE | GOAL_LOOK, Stalk { range: 24.0 }));
/// ```
#[derive(Debug)]
pub struct NativeGoal<B: NativeBehaviour> {
    name: &'static str,
    flags: u8,
    every_tick: bool,
    interruptable: bool,
    behaviour: Arc<B>,
    state: B::State,
}
impl<B: NativeBehaviour> Clone for NativeGoal<B> {
    fn clone(&self) -> Self {
        NativeGoal {
            name: self.name,
            flags: self.flags,
            every_tick: self.every_tick,
            interruptable: self.interruptable,
            behaviour: self.behaviour.clone(),
            state: self.state.clone(),
        }
    }
}
impl<B: NativeBehaviour> NativeGoal<B> {
    /// `name` is the goal's name for F3 and the log; `flags` are the MOVE, LOOK,
    /// JUMP and TARGET bits it locks (`GOAL_MOVE`... in `mod_api`).
    pub fn new(name: &'static str, flags: u8, behaviour: B) -> Self {
        NativeGoal {
            name,
            flags,
            every_tick: false,
            interruptable: true,
            behaviour: Arc::new(behaviour),
            state: B::State::default(),
        }
    }
    /// Ticks every tick instead of vanilla's every other tick (`Goal.requiresUpdateEveryTick`).
    pub fn updated_every_tick(mut self) -> Self {
        self.every_tick = true;
        self
    }
    /// A higher-priority goal cannot interrupt this one while it runs.
    pub fn uninterruptable(mut self) -> Self {
        self.interruptable = false;
        self
    }
    pub fn state(&self) -> &B::State {
        &self.state
    }
    fn call<R>(
        &mut self,
        m: &mut Mob,
        cx: &mut Cx,
        f: impl FnOnce(&B, &mut B::State, &MobView, &mut Actions) -> R,
    ) -> R {
        let view = MobView::of(m, Some(cx.w));
        let mut out = Actions::default();
        let r = f(&self.behaviour, &mut self.state, &view, &mut out);
        out.apply(m, Some(cx));
        r
    }
}
impl<B: NativeBehaviour> Goal for NativeGoal<B> {
    fn flags(&self) -> u8 {
        self.flags
    }
    fn can_use(&mut self, m: &mut Mob, cx: &mut Cx) -> bool {
        self.call(m, cx, |b, s, v, o| b.can_use(s, v, o))
    }
    fn can_continue(&mut self, m: &mut Mob, cx: &mut Cx) -> bool {
        self.call(m, cx, |b, s, v, o| b.can_continue(s, v, o))
    }
    fn interruptable(&self) -> bool {
        self.interruptable
    }
    fn start(&mut self, m: &mut Mob, cx: &mut Cx) {
        self.call(m, cx, |b, s, v, o| b.start(s, v, o))
    }
    fn stop(&mut self, m: &mut Mob, cx: &mut Cx) {
        self.call(m, cx, |b, s, v, o| b.stop(s, v, o))
    }
    fn every_tick(&self) -> bool {
        self.every_tick
    }
    fn tick(&mut self, m: &mut Mob, cx: &mut Cx) {
        self.call(m, cx, |b, s, v, o| b.tick(s, v, o))
    }
    fn box_clone(&self) -> Box<dyn Goal> {
        Box::new(self.clone())
    }
    fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobs::goals::{LOOK, MOVE};
    use crate::mobs::sensor_scan_tests::kind;
    use horizon_core::geometry::Vec3;

    fn mob() -> Mob {
        Mob::new(7, kind(), Vec3::new(0.5, 64.0, 0.5), 90.0)
    }

    #[test]
    fn the_view_reads_the_mob_without_a_world() {
        let mut m = mob();
        m.health = 3.5;
        m.ai.target = Some(Who::Player);
        let v = MobView::of(&m, None);
        assert_eq!((v.id, v.kind.id.as_str()), (7, "minecraft:cow"));
        assert_eq!(v.position, [0.5, 64.0, 0.5]);
        assert_eq!((v.health, v.max_health), (3.5, 10.0));
        assert_eq!(v.target, Some(Who::Player));
        // Without a world, nothing the world knows.
        assert_eq!(v.target_position, None);
        assert!(!v.target_in_melee_range);
        assert_eq!(v.nearest_player, None);
        assert!(v.navigation_done);
        assert_eq!(v.distance_to([3.5, 64.0, 4.5]), 5.0);
    }

    #[test]
    fn local_actions_apply_without_a_world_and_world_actions_wait_for_one() {
        let mut m = mob();
        let mut out = Actions::default();
        out.set_target(Some(Who::Mob(12)));
        out.look_at([10.0, 64.0, 0.5]);
        out.move_toward([2.0, 64.0, 2.0], 1.5);
        out.jump();
        out.navigate_to([20.0, 64.0, 20.0], 1.0);
        out.attack(Who::Player);
        out.apply(&mut m, None);
        assert_eq!(m.ai.target, Some(Who::Mob(12)));
        assert_eq!(m.ai.look.wanted, [10.0, 64.0, 0.5]);
        assert_eq!(m.ai.mov.wanted, [2.0, 64.0, 2.0]);
        assert_eq!(m.ai.mov.speed_modifier, 1.5);
        assert!(m.ai.jump);
        // No path was made and no swing started: those need the world.
        assert!(m.ai.nav.is_done());
        assert!(!m.ai.swinging);
    }

    /// Counts its calls per mob: the behaviour is shared, the state is the mob's.
    #[derive(Debug)]
    struct Counting;
    impl NativeBehaviour for Counting {
        type State = u32;
        fn can_use(&self, state: &mut u32, mob: &MobView, out: &mut Actions) -> bool {
            *state += 1;
            out.look_at([mob.position[0], mob.position[1] + 1.0, mob.position[2]]);
            mob.health < mob.max_health
        }
    }

    #[test]
    fn a_native_goal_is_a_goal_with_its_own_state_per_clone() {
        let goal = NativeGoal::new("Counting", MOVE | LOOK, Counting).updated_every_tick();
        assert_eq!(goal.name(), "Counting");
        assert_eq!(goal.flags(), MOVE | LOOK);
        assert!(Goal::every_tick(&goal));
        assert!(Goal::interruptable(&goal));
        assert_eq!(*goal.state(), 0);
        let boxed: Box<dyn Goal> = goal.box_clone();
        assert_eq!(boxed.name(), "Counting");
        let mut m = mob();
        m.ai.goals.add(1, goal);
        assert!(m.ai.goals.has("Counting"));
        assert!(!m.ai.goals.is_running("Counting"));
    }
}
