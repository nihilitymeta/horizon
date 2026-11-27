//! Outcome tests for the console runner: what a chunk prints, returns, fails with, and what
//! `mc.cmd` runs. Each test makes its own VM with [`ConsoleVm::start`], so no run leaks into another.

use std::time::Instant;

use mod_api::prelude::ServicesServerHost;
use mod_api::{CommandResult, Commands, Host, ServerHost, Text, TextFormError};

use super::*;

/// The most a test lets a stopped chunk take. Far past [`CHUNK_BUDGET`], so a chunk that was not
/// stopped fails the test rather than hanging it.
const SLACK: Duration = Duration::from_secs(5);

/// The server's commands as the stub gives them: each line run, and the feedback every run gets.
#[derive(Default)]
struct Cmds {
    ran: Vec<String>,
    answer: Vec<String>,
}

impl Commands for Cmds {
    fn run_command(&mut self, line: &str) -> bool {
        self.ran.push(line.to_owned());
        true
    }
    fn run_command_with_output(&mut self, line: &str) -> CommandResult {
        self.ran.push(line.to_owned());
        CommandResult::new(true, self.answer.clone())
    }
    fn feedback(&mut self, _line: Text) -> Result<(), TextFormError> {
        Ok(())
    }
}

/// A host whose only service is the commands one.
#[derive(Default)]
struct Stub {
    commands: Cmds,
}

impl Host for Stub {}
mod_api::impl_host_areas!(Stub);
impl ServerHost for Stub {}
mod_api::impl_server_host_areas!(Stub, except(services));
impl ServicesServerHost for Stub {
    fn commands(&mut self) -> &mut dyn Commands {
        &mut self.commands
    }
}

/// Runs `source` on a fresh VM with a host that answers `answer` to every command.
fn run(source: &str) -> RunResult {
    let mut host = Stub::default();
    ConsoleVm::start().run_chunk(&mut host, "chunk", source)
}

#[test]
fn print_writes_one_line_with_its_arguments_tab_joined() {
    let result = run("print(1+1)");
    assert_eq!(result.lines, ["2"]);
    assert_eq!(result.error, None);
    let result = run("print('a', 1, nil, true)");
    assert_eq!(result.lines, ["a\t1\tnil\ttrue"]);
}

#[test]
fn two_prints_are_two_lines_in_order() {
    let result = run("print('first'); print('second')");
    assert_eq!(result.lines, ["first", "second"]);
}

#[test]
fn an_expression_gives_its_value_as_text() {
    let result = run("1+1");
    assert_eq!(result.value.as_deref(), Some("2"));
    assert_eq!(result.error, None);
}

#[test]
fn a_return_statement_gives_its_value() {
    let result = run("return \"a\"..\"b\"");
    assert_eq!(result.value.as_deref(), Some("ab"));
    assert_eq!(result.error, None);
}

#[test]
fn a_chunk_that_returns_nothing_has_no_value() {
    let result = run("local x = 1");
    assert_eq!(result.value, None);
    assert_eq!(result.error, None);
}

#[test]
fn tables_and_functions_are_named_not_printed() {
    assert_eq!(run("return {1, 2}").value.as_deref(), Some("table"));
    assert_eq!(run("return print").value.as_deref(), Some("function"));
}

#[test]
fn the_sandbox_has_no_io_os_execute_or_require() {
    let result = run("return io == nil and os.execute == nil and require == nil");
    assert_eq!(result.value.as_deref(), Some("true"), "{result:?}");
}

#[test]
fn an_endless_loop_is_stopped_by_the_budget() {
    let started = Instant::now();
    let result = run("while true do end");
    let elapsed = started.elapsed();
    assert!(
        result.error.is_some(),
        "the loop must be stopped: {result:?}"
    );
    assert!(elapsed < SLACK, "not stopped in time: {elapsed:?}");
}

#[test]
fn a_pcall_does_not_swallow_the_budget() {
    let started = Instant::now();
    let result = run("while true do pcall(function() while true do end end) end");
    let elapsed = started.elapsed();
    assert!(
        result.error.is_some(),
        "the budget must still stop it: {result:?}"
    );
    assert!(elapsed < SLACK, "not stopped in time: {elapsed:?}");
}

#[test]
fn an_allocation_past_the_memory_limit_is_an_error_not_a_crash() {
    let result = run("return string.rep('x', 1e9)");
    assert!(result.error.is_some(), "{result:?}");
    assert_eq!(result.value, None);
}

#[test]
fn a_syntax_error_names_the_label_and_the_line() {
    let mut host = Stub::default();
    let result = ConsoleVm::start().run_chunk(&mut host, "lbl", "local = 1");
    let error = result.error.expect("a syntax error");
    assert!(error.starts_with("lbl:1:"), "{error}");
}

#[test]
fn a_runtime_error_gives_the_label_line_and_message() {
    let mut host = Stub::default();
    let result = ConsoleVm::start().run_chunk(&mut host, "lbl", "error('boom')");
    assert_eq!(result.error.as_deref(), Some("lbl:1: boom"));
}

#[test]
fn the_print_buffer_keeps_the_last_256_lines() {
    let result = run("for i = 1, 300 do print(i) end");
    assert_eq!(result.lines.len(), 256);
    assert_eq!(result.lines.first().map(String::as_str), Some("45"));
    assert_eq!(result.lines.last().map(String::as_str), Some("300"));
}

#[test]
fn a_long_printed_line_is_cut_with_an_ellipsis() {
    let result = run("print(string.rep('x', 5000))");
    assert_eq!(result.lines.len(), 1);
    assert!(result.lines[0].len() <= MAX_LINE_BYTES);
    assert!(result.lines[0].ends_with("..."));
}

#[test]
fn a_second_run_while_one_holds_the_vm_is_busy() {
    let vm = ConsoleVm::start();
    let mut host = Stub::default();
    let held = vm.lua.as_ref().expect("the VM starts").lock().unwrap();
    let result = vm.run_chunk(&mut host, "lbl", "return 1");
    drop(held);
    assert_eq!(result.error.as_deref(), Some(BUSY));
    assert_eq!(result.value, None);
    // Once the holder is gone the VM runs again.
    let result = vm.run_chunk(&mut host, "lbl", "return 1");
    assert_eq!(result.value.as_deref(), Some("1"));
}

#[test]
fn mc_cmd_runs_the_line_on_the_host_and_returns_its_lines() {
    let mut host = Stub {
        commands: Cmds {
            ran: Vec::new(),
            answer: vec!["hi there".to_owned()],
        },
    };
    let result = ConsoleVm::start().run_chunk(&mut host, "chunk", "return mc.cmd('say hi')[1]");
    assert_eq!(result.value.as_deref(), Some("hi there"), "{result:?}");
    assert_eq!(host.commands.ran, ["say hi"]);
}

#[test]
fn mc_cmd_called_after_its_run_fails_and_runs_nothing() {
    let vm = ConsoleVm::start();
    let mut host = Stub::default();
    let first = vm.run_chunk(&mut host, "chunk", "kept = mc.cmd");
    assert_eq!(first.error, None, "{first:?}");
    // The global survives the run, so the second run really calls the function it kept.
    let kept = vm.run_chunk(&mut host, "chunk", "return type(kept)");
    assert_eq!(kept.value.as_deref(), Some("function"), "{kept:?}");
    let second = vm.run_chunk(&mut host, "chunk", "return kept('say later')");
    let error = second.error.expect("the kept function must fail");
    assert!(!error.contains("nil"), "not a missing function: {error}");
    assert!(host.commands.ran.is_empty(), "{:?}", host.commands.ran);
}

#[test]
fn warn_is_not_in_the_console_vm() {
    // Luau has no `warn` global, so the console adds none.
    let result = run("return warn == nil");
    assert_eq!(result.value.as_deref(), Some("true"), "{result:?}");
}
