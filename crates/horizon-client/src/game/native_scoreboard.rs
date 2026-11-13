//! The scoreboard for native server mods (docs/native-mods.md, "Scoreboard",
//! #61): a score read for any objective, vanilla's and a command block's too,
//! and a score written, an objective created or removed only in the mod's own
//! namespace `<mod_id>.<name>`. The server owns the board; a client refuses.
//! The writes change the board only: the once-per-tick sync
//! (`lan_sync_scoreboard`) sends the change to the clients.
use super::Game;
use horizon_client::command::Registries;
use horizon_client::scoreboard::{self as sb, Scoreboard, literal};

/// Whether `name` is an objective of mod `mod_id`: `<mod_id>.<rest>` with a
/// non-empty `rest`. A mod id holds no `.`, so the first `.` ends it, and
/// `ab` never reaches `abc.x`.
fn own_objective(mod_id: &str, name: &str) -> bool {
    !mod_id.is_empty()
        && !mod_id.contains('.')
        && name
            .strip_prefix(mod_id)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|rest| !rest.is_empty())
}

/// The characters an unquoted objective name may hold, as `/scoreboard
/// objectives add` reads it (`Reader::read_unquoted_string`).
fn word(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
}

/// Whether mod `mod_id` may write objective `name`: its own, existing, and
/// not of a read-only criteria (`ObjectiveCriteria.isReadOnly`).
fn writable(board: &Scoreboard, mod_id: &str, name: &str) -> bool {
    own_objective(mod_id, name)
        && board
            .objective(name)
            .is_some_and(|o| !sb::criteria_read_only(&o.criteria))
}

/// `holder`'s score in `objective`; None when the objective or the score does
/// not exist. Never creates one.
pub(super) fn score(board: &Scoreboard, holder: &str, objective: &str) -> Option<i32> {
    board.objective(objective)?;
    board.score(holder, objective).map(|s| s.value)
}

/// Sets `holder`'s score in mod `mod_id`'s own writable objective; false
/// (nothing changed) when the objective is not the mod's, not there, or
/// read-only.
pub(super) fn set_score(
    board: &mut Scoreboard,
    mod_id: &str,
    holder: &str,
    objective: &str,
    value: i32,
) -> bool {
    if !writable(board, mod_id, objective) {
        return false;
    }
    board.set(holder, objective, value);
    true
}

/// Adds `delta` to `holder`'s score in mod `mod_id`'s own writable objective,
/// saturating at the i32 bounds; a score not yet set starts at 0. False
/// (nothing changed) under the same rules as [`set_score`].
pub(super) fn add_score(
    board: &mut Scoreboard,
    mod_id: &str,
    holder: &str,
    objective: &str,
    delta: i32,
) -> bool {
    if !writable(board, mod_id, objective) {
        return false;
    }
    let value = board.get(holder, objective).saturating_add(delta);
    board.set(holder, objective, value);
    true
}

/// The names of every objective, in the board's order.
pub(super) fn objectives(board: &Scoreboard) -> Vec<String> {
    board.objectives.iter().map(|o| o.name.clone()).collect()
}

/// Creates objective `name` with criteria `criteria` (canonicalized as the
/// command's criteria argument does), in mod `mod_id`'s namespace. False when
/// the name is not the mod's or not a valid unquoted name, the objective
/// already exists, or the criteria is unknown. A read-only criteria is
/// allowed, as `/scoreboard objectives add` allows it; the mod still cannot
/// write its scores.
pub(super) fn add_objective(
    board: &mut Scoreboard,
    reg: &Registries,
    mod_id: &str,
    name: &str,
    criteria: &str,
) -> bool {
    if !own_objective(mod_id, name) || !word(name) || board.objective(name).is_some() {
        return false;
    }
    let Ok(Some(canonical)) = sb::criteria_by_name(criteria, reg) else {
        return false;
    };
    let render = sb::criteria_render_type(&canonical);
    board.add_objective(name, &canonical, literal(name), render, false, None)
}

/// Removes objective `name` and every score in it, when it is mod `mod_id`'s
/// and exists. False otherwise, with nothing changed.
pub(super) fn remove_objective(board: &mut Scoreboard, mod_id: &str, name: &str) -> bool {
    if !own_objective(mod_id, name) || board.objective(name).is_none() {
        return false;
    }
    board.remove_objective(name);
    true
}

/// Helpers for server mods, kept as stable as the hook points. A client
/// window has no server board: reads answer None or empty, writes return
/// false.
impl Game<'_> {
    /// `holder`'s score in `objective`, of any objective (vanilla's, a command
    /// block's or another mod's): as `/scoreboard players get` reads it. None
    /// when the objective or the score does not exist, or off a server. Holder
    /// is a player name or an entity's UUID text. API @API:scoreboard@.
    pub(super) fn native_scoreboard_score(&self, holder: &str, objective: &str) -> Option<i32> {
        if self.remote.is_some() {
            return None;
        }
        score(&self.score.board, holder, objective)
    }
    /// Sets `holder`'s score in objective `mod_id.<name>`, the mod's own
    /// objective, to `value`. False, with nothing changed, when the objective
    /// is another mod's or vanilla's, does not exist, is of a read-only
    /// criteria, or off a server. Read it back with `native_scoreboard_score`.
    /// API @API:scoreboard@.
    pub(super) fn native_scoreboard_set_score(
        &mut self,
        mod_id: &str,
        holder: &str,
        objective: &str,
        value: i32,
    ) -> bool {
        if self.remote.is_some() {
            return false;
        }
        self.load_scoreboard();
        set_score(&mut self.score.board, mod_id, holder, objective, value)
    }
    /// Adds `delta` to `holder`'s score in the mod's own objective `objective`,
    /// saturating at the i32 bounds (a score not yet set starts at 0). False,
    /// with nothing changed, under the rules of `native_scoreboard_set_score`.
    /// API @API:scoreboard@.
    pub(super) fn native_scoreboard_add_score(
        &mut self,
        mod_id: &str,
        holder: &str,
        objective: &str,
        delta: i32,
    ) -> bool {
        if self.remote.is_some() {
            return false;
        }
        self.load_scoreboard();
        add_score(&mut self.score.board, mod_id, holder, objective, delta)
    }
    /// The names of every objective on the board, vanilla's, a command
    /// block's and every mod's, in the order `/scoreboard objectives list`
    /// shows them. Empty off a server. API @API:scoreboard@.
    pub(super) fn native_scoreboard_objectives(&self) -> Vec<String> {
        if self.remote.is_some() {
            return Vec::new();
        }
        objectives(&self.score.board)
    }
    /// Creates the objective `name` (`<mod_id>.<rest>`, the mod's own) with
    /// criteria `criteria` (`dummy`, `health`, `minecraft.mined:minecraft.stone`
    /// ...). False, with nothing created, when the name is not the mod's or
    /// not a valid name, it exists already, the criteria is unknown, or off a
    /// server. API @API:scoreboard@.
    pub(super) fn native_scoreboard_add_objective(
        &mut self,
        mod_id: &str,
        name: &str,
        criteria: &str,
    ) -> bool {
        if self.remote.is_some() {
            return false;
        }
        self.load_scoreboard();
        add_objective(
            &mut self.score.board,
            &self.ui.registries,
            mod_id,
            name,
            criteria,
        )
    }
    /// Removes the mod's own objective `name` and every score in it. False,
    /// with nothing changed, when the objective is not the mod's, does not
    /// exist, or off a server. API @API:scoreboard@.
    pub(super) fn native_scoreboard_remove_objective(&mut self, mod_id: &str, name: &str) -> bool {
        if self.remote.is_some() {
            return false;
        }
        self.load_scoreboard();
        remove_objective(&mut self.score.board, mod_id, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLDER: &str = "Steve";

    /// A board with a vanilla objective `kills` (Steve at 1) and a mod `mymod`
    /// objective `mymod.pts` (Steve at 2), each `dummy`.
    fn board() -> Scoreboard {
        let mut b = Scoreboard::default();
        let reg = Registries::default();
        // A vanilla objective is made by the game, not by a mod: no namespace check.
        assert!(b.add_objective(
            "kills",
            "dummy",
            literal("kills"),
            sb::criteria_render_type("dummy"),
            false,
            None,
        ));
        b.set(HOLDER, "kills", 1);
        assert!(add_objective(&mut b, &reg, "mymod", "mymod.pts", "dummy"));
        b.set(HOLDER, "mymod.pts", 2);
        b
    }

    #[test]
    fn reads_any_objective_and_never_creates() {
        let b = board();
        // An objective of another mod, and a vanilla one, read alike.
        assert_eq!(score(&b, HOLDER, "kills"), Some(1));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), Some(2));
        // A holder with no score, and an objective that is not there.
        assert_eq!(score(&b, "Alex", "kills"), None);
        assert_eq!(score(&b, HOLDER, "nope.x"), None);
        assert!(!b.scores.contains_key("Alex"));
        assert_eq!(b.objectives.len(), 2);
    }

    #[test]
    fn writes_only_in_the_own_namespace() {
        let mut b = board();
        assert!(set_score(&mut b, "mymod", HOLDER, "mymod.pts", 7));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), Some(7));
        // Vanilla's objective, and an objective with no namespace of this mod.
        assert!(!set_score(&mut b, "mymod", HOLDER, "kills", 9));
        assert!(!add_score(&mut b, "mymod", HOLDER, "kills", 9));
        assert_eq!(score(&b, HOLDER, "kills"), Some(1));
        // A write to an objective that does not exist is refused, and creates nothing.
        assert!(!set_score(&mut b, "mymod", HOLDER, "mymod.none", 5));
        assert!(!add_score(&mut b, "mymod", HOLDER, "mymod.none", 5));
        assert!(b.objective("mymod.none").is_none());
        assert!(
            b.scores
                .get(HOLDER)
                .is_some_and(|s| !s.contains_key("mymod.none"))
        );
    }

    #[test]
    fn a_mod_id_that_prefixes_another_cannot_write_it() {
        let mut b = Scoreboard::default();
        let reg = Registries::default();
        assert!(add_objective(&mut b, &reg, "abc", "abc.x", "dummy"));
        b.set(HOLDER, "abc.x", 3);
        assert!(!set_score(&mut b, "ab", HOLDER, "abc.x", 99));
        assert!(!add_score(&mut b, "ab", HOLDER, "abc.x", 99));
        assert_eq!(score(&b, HOLDER, "abc.x"), Some(3));
        // The same mod id can write its own `ab.x`.
        assert!(add_objective(&mut b, &reg, "ab", "ab.x", "dummy"));
        assert!(set_score(&mut b, "ab", HOLDER, "ab.x", 4));
        assert_eq!(score(&b, HOLDER, "ab.x"), Some(4));
    }

    #[test]
    fn set_then_read_back() {
        let mut b = board();
        assert!(set_score(&mut b, "mymod", "Alex", "mymod.pts", -12));
        assert_eq!(score(&b, "Alex", "mymod.pts"), Some(-12));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), Some(2));
    }

    #[test]
    fn add_applies_delta_and_saturates() {
        let mut b = board();
        assert!(add_score(&mut b, "mymod", HOLDER, "mymod.pts", 5));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), Some(7));
        assert!(add_score(&mut b, "mymod", HOLDER, "mymod.pts", -10));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), Some(-3));
        // A new holder starts at zero.
        assert!(add_score(&mut b, "mymod", "Alex", "mymod.pts", 4));
        assert_eq!(score(&b, "Alex", "mymod.pts"), Some(4));
        // Saturation at both bounds.
        assert!(set_score(
            &mut b,
            "mymod",
            HOLDER,
            "mymod.pts",
            i32::MAX - 1
        ));
        assert!(add_score(&mut b, "mymod", HOLDER, "mymod.pts", 10));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), Some(i32::MAX));
        assert!(set_score(
            &mut b,
            "mymod",
            HOLDER,
            "mymod.pts",
            i32::MIN + 1
        ));
        assert!(add_score(&mut b, "mymod", HOLDER, "mymod.pts", -10));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), Some(i32::MIN));
    }

    #[test]
    fn objective_names_are_checked() {
        let mut b = Scoreboard::default();
        let reg = Registries::default();
        // No `.rest`, an empty mod id, a dotted mod id, a foreign prefix, a space.
        assert!(!add_objective(&mut b, &reg, "mymod", "mymod", "dummy"));
        assert!(!add_objective(&mut b, &reg, "mymod", "mymod.", "dummy"));
        assert!(!add_objective(&mut b, &reg, "", ".x", "dummy"));
        assert!(!add_objective(&mut b, &reg, "a.b", "a.b.x", "dummy"));
        assert!(!add_objective(&mut b, &reg, "mymod", "other.x", "dummy"));
        assert!(!add_objective(&mut b, &reg, "mymod", "mymod.a b", "dummy"));
        assert!(objectives(&b).is_empty());
        assert!(add_objective(
            &mut b,
            &reg,
            "mymod",
            "mymod.a+b_c-1",
            "dummy"
        ));
    }

    #[test]
    fn duplicate_and_unknown_criteria_are_refused() {
        let mut b = board();
        let reg = Registries::default();
        assert!(!add_objective(&mut b, &reg, "mymod", "mymod.pts", "dummy"));
        assert!(!add_objective(
            &mut b,
            &reg,
            "mymod",
            "mymod.new",
            "nosuchcriteria"
        ));
        assert!(b.objective("mymod.new").is_none());
        assert_eq!(objectives(&b).len(), 2);
    }

    #[test]
    fn remove_clears_that_objectives_scores() {
        let mut b = board();
        assert!(set_score(&mut b, "mymod", "Alex", "mymod.pts", 6));
        assert!(remove_objective(&mut b, "mymod", "mymod.pts"));
        assert!(b.objective("mymod.pts").is_none());
        assert_eq!(score(&b, HOLDER, "mymod.pts"), None);
        assert_eq!(score(&b, "Alex", "mymod.pts"), None);
        // The vanilla objective keeps its score.
        assert_eq!(score(&b, HOLDER, "kills"), Some(1));
        // A second remove, a foreign one, and a write after the remove are refused.
        assert!(!remove_objective(&mut b, "mymod", "mymod.pts"));
        assert!(!remove_objective(&mut b, "mymod", "kills"));
        assert!(!set_score(&mut b, "mymod", HOLDER, "mymod.pts", 1));
        assert_eq!(score(&b, HOLDER, "mymod.pts"), None);
    }

    #[test]
    fn read_only_criteria_refuse_writes_but_may_be_created() {
        let mut b = Scoreboard::default();
        let reg = Registries::default();
        assert!(add_objective(&mut b, &reg, "mymod", "mymod.hp", "health"));
        assert!(!set_score(&mut b, "mymod", HOLDER, "mymod.hp", 5));
        assert!(!add_score(&mut b, "mymod", HOLDER, "mymod.hp", 5));
        assert_eq!(score(&b, HOLDER, "mymod.hp"), None);
    }

    #[test]
    fn lists_every_objective_name() {
        let b = board();
        let mut names = objectives(&b);
        names.sort();
        assert_eq!(names, vec!["kills".to_owned(), "mymod.pts".to_owned()]);
    }
}
