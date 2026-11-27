//! The developer console's state: the window's output lines, its input and history, and the queue
//! of sources the server tick runs. The window is drawn on the render thread and a source runs on
//! the server thread, so the state is process-wide behind a mutex, as the overlay's is.
//!
//! `console` (in [`crate::commands`]) opens or closes the window and clears its output. The window
//! queues what is typed; [`tick`] (called from the mod's `server_tick`) runs at most [`PER_TICK`]
//! queued sources a tick, through the console's runner, and appends what each printed, returned
//! and raised. A source runs only while the world has an owner: the window's user is the one who
//! plays it.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use event_horizon_lua::console::RunResult;
use horizon_client::lang;
use mod_api::ServerHost;

/// The most output lines the window keeps. The oldest go first.
pub const MAX_LINES: usize = 500;
/// The most submitted lines the history keeps. The oldest go first.
pub const MAX_HISTORY: usize = 100;
/// The most queued sources one server tick runs. The rest wait for the next tick.
pub const PER_TICK: usize = 4;

/// What an output line is, which sets its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What was submitted: `> <source>`.
    Echo,
    /// A line the chunk printed.
    Out,
    /// The chunk's value: `= <value>`.
    Value,
    /// The chunk's error, or a refusal.
    Err,
}

/// What the `console` command does to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Opens the window, and asks for the pointer so the input can be typed in.
    Open,
    Close,
    /// Empties the output (the history stays).
    Clear,
}

/// The window's state. Empty and closed until `console` opens it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct State {
    pub open: bool,
    /// The window takes the keyboard focus on its next frame.
    pub focus: bool,
    /// Set by opening: the next frame of the tools takes the pointer for the window.
    pub want_pointer: bool,
    pub lines: Vec<(Kind, String)>,
    pub input: String,
    /// The submitted lines, newest last. No line repeats the one before it.
    pub history: Vec<String>,
    /// The history entry the input shows while Up and Down walk it; None while typing a new line.
    pub walk: Option<usize>,
    /// The sources waiting for the server tick, oldest first.
    pub queue: VecDeque<String>,
}

/// The process's console state, read by the window and written by the command and the tick.
static STATE: Mutex<State> = Mutex::new(State {
    open: false,
    focus: false,
    want_pointer: false,
    lines: Vec::new(),
    input: String::new(),
    history: Vec::new(),
    walk: None,
    queue: VecDeque::new(),
});

/// The process's console state. Hold the guard only for a short step: the window holds it while
/// it draws.
pub fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the window is open (`console` opened it and no `console close` has run since).
pub(crate) fn is_open() -> bool {
    state().open
}

/// Takes the pointer request the opening left: true once after `console`, then false.
pub fn take_want_pointer() -> bool {
    std::mem::take(&mut state().want_pointer)
}

/// Applies the `console` command to the process's state.
pub fn act(action: Action) {
    state().act(action);
}

/// One server tick: runs up to [`PER_TICK`] queued sources. `owned` says whether the world has an
/// owner (`dev_server_owner`); without one each source is refused with the owner's line and the
/// runner is not called. `run` is the console's runner; the state is locked only around each step,
/// never while a chunk runs.
pub fn tick(store: &Mutex<State>, owned: bool, mut run: impl FnMut(&str) -> RunResult) {
    let batch = lock(store).take_batch(PER_TICK);
    for source in batch {
        if owned {
            let result = run(&source);
            lock(store).append_run(result);
        } else {
            lock(store).push(Kind::Err, lang::get("horizon.dev_tools.owner_only"));
        }
    }
}

fn lock(store: &Mutex<State>) -> MutexGuard<'_, State> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The tick's run of the queue, on the process's state and the console's runner.
pub fn tick_host(host: &mut dyn ServerHost) {
    let owned = host.dev_server_owner().is_some();
    tick(&STATE, owned, |source| {
        event_horizon_lua::console::console().run_chunk(host, "console", source)
    });
}

impl State {
    /// Applies `action`: opening shows the window and asks for the pointer and the focus; closing
    /// hides it; clearing empties the output.
    pub fn act(&mut self, action: Action) {
        match action {
            Action::Open => {
                self.open = true;
                self.focus = true;
                self.want_pointer = true;
            }
            Action::Close => self.open = false,
            Action::Clear => self.lines.clear(),
        }
    }

    /// Appends a line, dropping the oldest lines past [`MAX_LINES`].
    pub fn push(&mut self, kind: Kind, text: impl Into<String>) {
        self.lines.push((kind, text.into()));
        let extra = self.lines.len().saturating_sub(MAX_LINES);
        self.lines.drain(..extra);
    }

    /// Submits the input: echoes it, records it, queues it for the tick and clears the input.
    /// A blank input submits nothing and returns None.
    pub fn submit(&mut self) -> Option<String> {
        let source = std::mem::take(&mut self.input);
        if source.trim().is_empty() {
            return None;
        }
        self.push(Kind::Echo, format!("> {source}"));
        self.remember(&source);
        self.queue.push_back(source.clone());
        self.walk = None;
        Some(source)
    }

    /// Records a submitted line, unless it repeats the line before it; the oldest go past
    /// [`MAX_HISTORY`].
    fn remember(&mut self, source: &str) {
        if self.history.last().map(String::as_str) != Some(source) {
            self.history.push(source.to_owned());
        }
        let extra = self.history.len().saturating_sub(MAX_HISTORY);
        self.history.drain(..extra);
    }

    /// Up: the input shows the previous line of the history; at the oldest it stays there.
    pub fn history_up(&mut self) {
        let Some(newest) = self.history.len().checked_sub(1) else {
            return;
        };
        let index = match self.walk {
            None => newest,
            Some(i) => i.saturating_sub(1),
        };
        self.walk = Some(index);
        self.input = self.history[index].clone();
    }

    /// Down: the input shows the next line of the history; past the newest it is empty again.
    pub fn history_down(&mut self) {
        match self.walk {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.walk = Some(i + 1);
                self.input = self.history[i + 1].clone();
            }
            Some(_) => {
                self.walk = None;
                self.input.clear();
            }
        }
    }

    /// Takes up to `n` queued sources, oldest first.
    pub fn take_batch(&mut self, n: usize) -> Vec<String> {
        let k = n.min(self.queue.len());
        self.queue.drain(..k).collect()
    }

    /// Appends what a run gave: its printed lines, its value as `= <value>`, its error.
    pub fn append_run(&mut self, run: RunResult) {
        for line in run.lines {
            self.push(Kind::Out, line);
        }
        if let Some(value) = run.value {
            self.push(Kind::Value, format!("= {value}"));
        }
        if let Some(error) = run.error {
            self.push(Kind::Err, error);
        }
    }
}

#[cfg(test)]
pub(crate) fn test_guard() -> MutexGuard<'static, ()> {
    static TESTS: Mutex<()> = Mutex::new(());
    TESTS.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[(Kind, String)]) -> Vec<String> {
        lines.iter().map(|(_, t)| t.clone()).collect()
    }

    fn typed(state: &mut State, source: &str) {
        state.input = source.to_owned();
        state.submit();
    }

    #[test]
    fn a_submitted_line_is_echoed_recorded_and_queued_and_the_input_clears() {
        let mut s = State {
            input: "print(1+1)".to_owned(),
            ..State::default()
        };
        assert_eq!(s.submit().as_deref(), Some("print(1+1)"));
        assert_eq!(s.lines, vec![(Kind::Echo, "> print(1+1)".to_owned())]);
        assert_eq!(s.history, vec!["print(1+1)".to_owned()]);
        assert_eq!(s.queue, VecDeque::from(["print(1+1)".to_owned()]));
        assert!(s.input.is_empty());
    }

    #[test]
    fn a_blank_line_submits_nothing() {
        let mut s = State {
            input: "   ".to_owned(),
            ..State::default()
        };
        assert_eq!(s.submit(), None);
        assert!(s.lines.is_empty() && s.history.is_empty() && s.queue.is_empty());
    }

    #[test]
    fn history_drops_a_repeat_of_the_line_before_it_only() {
        let mut s = State::default();
        for line in ["a", "a", "b", "a"] {
            typed(&mut s, line);
        }
        assert_eq!(s.history, vec!["a", "b", "a"]);
    }

    #[test]
    fn history_keeps_the_newest_hundred() {
        let mut s = State::default();
        for i in 0..150 {
            typed(&mut s, &format!("line {i}"));
        }
        assert_eq!(s.history.len(), MAX_HISTORY);
        assert_eq!(s.history.first().map(String::as_str), Some("line 50"));
        assert_eq!(s.history.last().map(String::as_str), Some("line 149"));
    }

    #[test]
    fn up_and_down_walk_the_history_and_stop_at_the_ends() {
        let mut s = State::default();
        for line in ["one", "two", "three"] {
            typed(&mut s, line);
        }
        s.history_up();
        assert_eq!(s.input, "three");
        s.history_up();
        assert_eq!(s.input, "two");
        s.history_up();
        s.history_up();
        assert_eq!(s.input, "one", "the oldest stays");
        s.history_down();
        assert_eq!(s.input, "two");
        s.history_down();
        s.history_down();
        assert!(s.input.is_empty(), "past the newest, a new empty line");
        s.history_down();
        assert!(
            s.input.is_empty() && s.walk.is_none(),
            "Down with nothing to walk does nothing"
        );
    }

    #[test]
    fn up_with_no_history_does_nothing() {
        let mut s = State::default();
        s.history_up();
        assert!(s.input.is_empty() && s.walk.is_none());
    }

    #[test]
    fn output_keeps_the_newest_five_hundred_lines() {
        let mut s = State::default();
        for i in 0..520 {
            s.push(Kind::Out, format!("{i}"));
        }
        assert_eq!(s.lines.len(), MAX_LINES);
        assert_eq!(s.lines.first().map(|l| l.1.as_str()), Some("20"));
        assert_eq!(s.lines.last().map(|l| l.1.as_str()), Some("519"));
    }

    #[test]
    fn console_clear_empties_the_output_and_keeps_the_history() {
        let mut s = State::default();
        typed(&mut s, "1+1");
        s.act(Action::Clear);
        assert!(s.lines.is_empty());
        assert_eq!(s.history, vec!["1+1"]);
    }

    #[test]
    fn opening_shows_the_window_and_asks_for_the_pointer_once() {
        let mut s = State::default();
        s.act(Action::Open);
        assert!(s.open && s.focus && s.want_pointer);
        s.act(Action::Close);
        assert!(!s.open);
    }

    #[test]
    fn a_run_appends_its_lines_then_its_value_then_its_error() {
        let mut s = State::default();
        s.append_run(RunResult {
            lines: vec!["a".to_owned(), "b".to_owned()],
            value: Some("2".to_owned()),
            error: Some("boom".to_owned()),
        });
        assert_eq!(
            s.lines,
            vec![
                (Kind::Out, "a".to_owned()),
                (Kind::Out, "b".to_owned()),
                (Kind::Value, "= 2".to_owned()),
                (Kind::Err, "boom".to_owned()),
            ]
        );
    }

    #[test]
    fn with_an_owner_the_tick_runs_each_source_once_and_its_lines_come_in_order() {
        let store = Mutex::new(State::default());
        for line in ["print(1)", "1+1"] {
            typed(&mut lock(&store), line);
        }
        let mut seen = Vec::new();
        tick(&store, true, |source| {
            seen.push(source.to_owned());
            match source {
                "print(1)" => RunResult {
                    lines: vec!["1".to_owned()],
                    ..RunResult::default()
                },
                _ => RunResult {
                    value: Some("2".to_owned()),
                    ..RunResult::default()
                },
            }
        });
        assert_eq!(seen, vec!["print(1)", "1+1"]);
        assert_eq!(
            text(&lock(&store).lines),
            vec!["> print(1)", "> 1+1", "1", "= 2"]
        );
        assert!(lock(&store).queue.is_empty());
    }

    #[test]
    fn without_an_owner_the_tick_refuses_each_source_and_never_runs_it() {
        let store = Mutex::new(State::default());
        typed(&mut lock(&store), "error('x')");
        let mut calls = 0;
        tick(&store, false, |_| {
            calls += 1;
            RunResult::default()
        });
        assert_eq!(calls, 0, "the runner is not called");
        let s = lock(&store);
        assert_eq!(
            s.lines.last(),
            Some(&(Kind::Err, lang::get("horizon.dev_tools.owner_only")))
        );
        assert!(s.queue.is_empty(), "the refused source is dropped");
    }

    #[test]
    fn the_tick_runs_at_most_four_sources_and_the_rest_wait_for_the_next() {
        let store = Mutex::new(State::default());
        for i in 0..6 {
            typed(&mut lock(&store), &format!("s{i}"));
        }
        let mut runs = 0;
        tick(&store, true, |_| {
            runs += 1;
            RunResult::default()
        });
        assert_eq!(runs, PER_TICK);
        assert_eq!(lock(&store).queue.len(), 2);
        tick(&store, true, |_| {
            runs += 1;
            RunResult::default()
        });
        assert_eq!(runs, 6);
        assert!(lock(&store).queue.is_empty());
    }
}
