//! Reading an attempt's evidence out of its messages, in one pass: its
//! checks, the files it changed, and its final reply.

use aura_model::{Message, Role};

use super::types::KEPT_PATHS;
use super::{CheckEvidence, Evidence};
use crate::checks::{Placed, Placing};
use crate::transcript::reply::is_reply;
use crate::transcript::said::is_nudge;
use crate::transcript::{Tool, Tools, FILE_TOOLS};

/// Checks kept with their output, latest last.
const KEPT_CHECKS: usize = 4;

/// Characters kept of a check's output (its end) and of the reply (its
/// start).
pub(super) const KEPT_TEXT: usize = 4000;

/// Characters kept of a check's command line.
const KEPT_COMMAND: usize = 600;

/// Text in a command line that changes files in the workspace, as far as
/// a command line shows it.
const SHELL_WRITES: &[&str] = &[
    "sed -i",
    "perl -pi",
    "perl -i",
    "git apply",
    "git checkout ",
    "git restore ",
    "git stash",
    "patch -p",
    ".write(",
    "write_text(",
];

impl Evidence {
    /// The evidence in `messages`, an attempt's own.
    #[must_use]
    pub fn of(messages: &[Message]) -> Self {
        let mut scan = Scan::default();
        for message in messages {
            scan.message(message);
        }
        scan.evidence()
    }
}

/// What a pass over an attempt's messages has read so far.
#[derive(Debug, Default)]
struct Scan<'a> {
    tools: Tools,
    checks: Placing<'a>,
    calls: u32,
    commands: u32,
    nudges: u32,
    files_edited: Vec<String>,
    files_written: Vec<String>,
    /// The place of the tool call that last changed a file, among the
    /// tool calls
    last_change_at: Option<usize>,
    /// The final reply as of the messages read
    reply: Option<&'a Message>,
}

impl<'a> Scan<'a> {
    fn message(&mut self, message: &'a Message) {
        match message.role {
            Role::Assistant => {
                self.calls += 1;
                if is_reply(message) {
                    self.reply = Some(message);
                }
            }
            Role::User if is_nudge(&message.text_content()) => self.nudges += 1,
            Role::User => {}
        }
        for block in &message.content {
            if let Some(tool) = self.tools.read(block) {
                self.tool(&tool);
                self.checks.read(tool);
            }
        }
    }

    fn tool(&mut self, tool: &Tool<'_>) {
        match *tool {
            Tool::Command { at, ref line, .. } => {
                self.commands += 1;
                if SHELL_WRITES.iter().any(|mark| line.contains(mark)) {
                    self.last_change_at = Some(at);
                }
            }
            Tool::Call { at, name, input } if FILE_TOOLS.contains(&name) => {
                self.last_change_at = Some(at);
                self.file_changed(name, input["path"].as_str().unwrap_or_default());
            }
            Tool::Call { .. } | Tool::Result { .. } => {}
        }
    }

    /// Note that the file tool `tool` changed the file at `path`.
    fn file_changed(&mut self, tool: &str, path: &str) {
        let known = self.files_edited.iter().any(|edited| edited == path);
        if path.is_empty() || known || self.files_edited.len() >= KEPT_PATHS {
            return;
        }
        if tool == "write_file" {
            self.files_written.push(path.to_string());
        }
        self.files_edited.push(path.to_string());
    }

    fn evidence(self) -> Evidence {
        let placed = self.checks.finish();
        let last_check_at = placed.last().map(|check| check.at);
        let reply = self.reply.map(Message::text_content).unwrap_or_default();
        Evidence {
            calls: self.calls,
            commands: self.commands,
            checks_run: u32::try_from(placed.len()).unwrap_or(u32::MAX),
            nudges: self.nudges,
            checks: kept(&placed),
            files_edited: self.files_edited,
            files_written: self.files_written,
            files_changed: None,
            changed_paths: None,
            changed_after_check: last_check_at
                .zip(self.last_change_at)
                .is_some_and(|(check, change)| change > check),
            hit_limit: None,
            reply: head(reply.trim(), KEPT_TEXT).to_string(),
        }
    }
}

/// The last [`KEPT_CHECKS`] of `placed`, as the evidence keeps them.
fn kept(placed: &[Placed]) -> Vec<CheckEvidence> {
    placed[placed.len().saturating_sub(KEPT_CHECKS)..]
        .iter()
        .map(|check| CheckEvidence {
            command: head(&check.command, KEPT_COMMAND).to_string(),
            passed: check.passed,
            output: tail(check.output.printed(), KEPT_TEXT).to_string(),
        })
        .collect()
}

/// The first `chars` characters of `text`.
fn head(text: &str, chars: usize) -> &str {
    text.char_indices()
        .nth(chars)
        .map_or(text, |(at, _)| &text[..at])
}

/// The last `chars` characters of `text`.
fn tail(text: &str, chars: usize) -> &str {
    let start = text
        .char_indices()
        .rev()
        .take(chars)
        .last()
        .map_or(text.len(), |(at, _)| at);
    &text[start..]
}
