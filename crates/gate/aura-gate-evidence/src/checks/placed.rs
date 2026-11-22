//! The checks of a turn, read out of its tool calls and results in order.

use aura_model::{Message, ToolResultContent};

use super::command::is_check_command;
use super::stash::Stashing;
use super::verdict::verdict;
use crate::transcript::{shows_failures, tools, Output, Tool};

/// A check that ran to its end, with its place among the tool calls.
#[derive(Debug)]
pub(crate) struct Placed {
    /// Its place among the tool calls (0 for the first)
    pub at: usize,
    /// The command line it ran
    pub command: String,
    pub passed: bool,
    /// Whether what it printed decided whether it passed
    pub by_output: bool,
    pub output: Output,
}

/// Every check in `messages` that ran to its end, in the order they were
/// called.
pub(super) fn checks_placed(messages: &[Message]) -> Vec<Placed> {
    let mut placing = Placing::default();
    for tool in tools(messages) {
        placing.read(tool);
    }
    placing.finish()
}

/// The checks of a turn as its tool calls and results are read.
#[derive(Debug, Default)]
pub(crate) struct Placing<'a> {
    checks: Vec<Called<'a>>,
    /// Whether the change is stashed away as of the latest call
    stashed: bool,
    /// The calls that changed that, with what it was before them
    stashes: Vec<(&'a str, bool)>,
}

/// A `run_command` call that runs a check.
#[derive(Debug)]
struct Called<'a> {
    /// Its call id
    id: &'a str,
    /// Its place among the tool calls
    at: usize,
    command: String,
    stashing: Stashing,
    /// What it printed and whether it failed, once it has a result
    result: Option<(Output, bool)>,
}

impl<'a> Placing<'a> {
    /// Read `tool`, the turn's next tool call or result.
    pub(crate) fn read(&mut self, tool: Tool<'a>) {
        match tool {
            Tool::Command {
                id,
                at,
                line,
                background,
            } => self.command(id, at, line, background),
            Tool::Result {
                id,
                content,
                is_error,
            } => self.result(id, content, is_error),
            Tool::Call { .. } => {}
        }
    }

    fn command(&mut self, id: &'a str, at: usize, command: String, background: bool) {
        let stashing = Stashing::of(&command, self.stashed);
        if stashing.stashed != self.stashed {
            self.stashes.push((id, self.stashed));
            self.stashed = stashing.stashed;
        }
        if !background && is_check_command(&command) {
            self.checks.push(Called {
                id,
                at,
                command,
                stashing,
                result: None,
            });
        }
    }

    fn result(&mut self, id: &str, content: &ToolResultContent, is_error: bool) {
        // A stash that failed stashed nothing.
        if let Some((_, before)) = self
            .stashes
            .iter()
            .find(|(stash, _)| is_error && *stash == id)
        {
            self.stashed = *before;
        }
        if let Some(check) = self.checks.iter_mut().find(|check| check.id == id) {
            check.result = Some((Output::of(content), is_error));
        }
    }

    /// The checks read that ran to their end with the change in place, in
    /// the order they were called. A failing one is left out when every
    /// check it ran failed with the change stashed away too.
    pub(crate) fn finish(self) -> Vec<Placed> {
        let (with_change, without): (Vec<_>, Vec<_>) = self
            .checks
            .into_iter()
            .partition(|check| check.stashing.without.is_empty());
        // The checks a run without the change showed failing.
        let failed_before: Vec<&str> = without
            .iter()
            .filter(|check| {
                check
                    .result
                    .as_ref()
                    .is_some_and(|(output, _)| shows_failures(output.printed()))
            })
            .flat_map(|check| check.stashing.without.iter().map(String::as_str))
            .collect();
        with_change
            .into_iter()
            .filter_map(|check| check.placed(&failed_before))
            .collect()
    }
}

impl Called<'_> {
    /// The check as it ran: `None` when it did not run to its end, or
    /// failed where every check it ran is among those `failed_before` the
    /// change.
    fn placed(self, failed_before: &[&str]) -> Option<Placed> {
        let (output, is_error) = self.result?;
        let (passed, by_output) = verdict(&output, is_error, &self.command)?;
        let with = &self.stashing.with;
        if !passed
            && !with.is_empty()
            && with.iter().all(|run| failed_before.contains(&run.as_str()))
        {
            return None;
        }
        Some(Placed {
            at: self.at,
            command: self.command,
            passed,
            by_output,
            output,
        })
    }
}
