//! The chat panel's LOG tab: the run log of the agent on screen and of
//! every agent it spawned, as the event loop wrote it to `log.json`, laid
//! out for drawing. Every text is ready to show; the JSON of each run,
//! turn and step is there to copy.

use serde::{Deserialize, Serialize};

/// How a run, turn or step went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LogStatus {
    #[default]
    Ok,
    /// Needed a nudge or a retry, was blocked, or stopped on its token cap
    Warn,
    /// Failed
    Error,
    /// Not finished yet
    Running,
}

/// One `label: value` row of a panel's metadata.
pub type LogField = (String, String);

/// A block of text in an opened step: its thinking, reply, a call's input
/// or a tool's output.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogSection {
    pub title: String,
    pub body: String,
    /// Drawn as code (JSON, tool output) rather than prose
    pub code: bool,
}

/// One step of a turn.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogStepView {
    pub seq: u64,
    /// `prompt`, `model`, `tool`, `decision` or `end`
    pub kind: String,
    /// What it did: a model call's tool calls with their key argument
    /// (`read_file src/x.rs`) or the first line of its reply; the call a
    /// tool result answers
    pub label: String,
    /// What came of it in a few words: `12 entries`, `exit 0 · 3 lines`,
    /// `error: …`
    pub summary: String,
    /// The model call a tool result answers, by `seq`
    pub parent: Option<u64>,
    /// `14k tok`, or empty
    pub tokens: String,
    /// `$0.04`, or empty
    pub cost: String,
    /// `3s`, or empty
    pub took: String,
    pub status: LogStatus,
    pub fields: Vec<LogField>,
    pub sections: Vec<LogSection>,
    /// Runs of the agents the step's calls spawned, by index into
    /// [`LogView::runs`]
    pub spawned: Vec<usize>,
    /// The step's object in `log.json`, pretty-printed
    pub json: String,
}

/// One user turn.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogTurnView {
    pub turn: u32,
    /// First line of the prompt, empty when it is the run's title
    pub prompt: String,
    /// `32 calls · end_turn`
    pub detail: String,
    pub tokens: String,
    pub cost: String,
    pub took: String,
    pub status: LogStatus,
    pub steps: Vec<LogStepView>,
    /// Runs spawned in the turn that no step names, by index into
    /// [`LogView::runs`]
    pub spawned: Vec<usize>,
    pub json: String,
}

/// One run: the agent on screen's, or one it spawned at any depth.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogRunView {
    pub run_id: String,
    pub title: String,
    /// `sonnet-5-5 · 3 turns · $0.21 · 4m02s`
    pub detail: String,
    pub status: LogStatus,
    /// How many runs up its spawner is; 0 for the run asked for
    pub depth: usize,
    pub fields: Vec<LogField>,
    /// Files of its bundle on disk
    pub files: Vec<LogField>,
    /// What looks wrong with the run (calls that never finished, ...)
    pub problems: Vec<String>,
    pub turns: Vec<LogTurnView>,
    /// Indices in [`LogView::runs`] of runs below it that none of its
    /// turns started, such as a benchmark's, listed after its turns
    pub runs: Vec<usize>,
    /// Its log in `log.json`, its subagents' included, pretty-printed
    pub json: String,
}

/// The LOG tab's content.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogView {
    /// Hex id of the agent it was asked for
    pub agent_id: String,
    /// Where `log.json` was written
    pub path: String,
    /// The whole of `log.json`
    pub json: String,
    /// The run asked for first, then the runs below it, each after the
    /// run that spawned it
    pub runs: Vec<LogRunView>,
}

impl LogView {
    /// The run asked for.
    #[must_use]
    pub fn root(&self) -> Option<&LogRunView> {
        self.runs.first()
    }
}

/// What the LOG tab asks of the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogRequest {
    /// The log of the agent on screen's latest run, written to its
    /// `log.json` ([`crate::UiCommand::LogView`])
    Load,
}
