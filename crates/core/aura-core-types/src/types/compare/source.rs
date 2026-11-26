use crate::ids::AgentId;
use serde::{Deserialize, Serialize};

/// What a replayed session cost when it ran.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionBaseline {
    /// Model calls.
    pub calls: u32,
    /// List-price cost in USD.
    pub cost_usd: f64,
    /// Whether any call had a price.
    pub priced: bool,
    /// Calls with no known price, which `cost_usd` leaves out. 0 in
    /// baselines saved before it was kept.
    #[serde(default)]
    pub unpriced_calls: u32,
    /// Models the calls went to, first use first.
    pub models: Vec<String>,
}

impl SessionBaseline {
    /// What the session cost when every call had a price; `None` when
    /// some or all had none, so it cannot be set against another cost.
    #[must_use]
    pub const fn known_cost_usd(&self) -> Option<f64> {
        if self.priced && self.unpriced_calls == 0 {
            Some(self.cost_usd)
        } else {
            None
        }
    }
}

/// What the user kept from a replayed session, for the judge to measure
/// the runs against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// The files the session changed, as a unified diff; empty when it
    /// changed none.
    pub diff: String,
    /// The session's last reply.
    pub reply: String,
}

/// Where a comparison's prompts came from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BenchSource {
    /// One prompt (or the build prompt of a plan).
    #[default]
    Prompt,
    /// The prompts of an earlier session of `agent_id`, replayed in order
    /// from the files as they were when it started.
    Session {
        agent_id: AgentId,
        started_ms: u64,
        /// Its first prompt, cut short.
        title: String,
        /// What it cost when it ran.
        baseline: SessionBaseline,
        /// What it left; `None` when nothing could be read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<Reference>,
    },
    /// Built-in example coding tasks, each in its own fixture and checked
    /// by its own command. Run `n` of either arm works on task
    /// `(n - 1) % tasks.len()`.
    Examples { tasks: Vec<ExampleTaskInfo> },
}

/// One example coding task of a benchmark.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleTaskInfo {
    /// `<suite>/<task>`, as `coding/write-file`.
    pub id: String,
    pub title: String,
    /// The prompts its runs get, in order.
    pub prompts: Vec<String>,
    /// Command run in its copy after the run; exit 0 passes.
    pub check: String,
}

impl BenchSource {
    /// The reference to judge runs against, if any.
    #[must_use]
    pub const fn reference(&self) -> Option<&Reference> {
        match self {
            Self::Session { reference, .. } => reference.as_ref(),
            Self::Prompt | Self::Examples { .. } => None,
        }
    }

    /// What the replayed session cost, if this is one.
    #[must_use]
    pub const fn baseline(&self) -> Option<&SessionBaseline> {
        match self {
            Self::Session { baseline, .. } => Some(baseline),
            Self::Prompt | Self::Examples { .. } => None,
        }
    }

    /// The example tasks, if this is a benchmark of them.
    #[must_use]
    pub fn examples(&self) -> &[ExampleTaskInfo] {
        match self {
            Self::Examples { tasks } => tasks,
            Self::Prompt | Self::Session { .. } => &[],
        }
    }

    /// The example task run `n` (from 1) works on, if any.
    #[must_use]
    pub fn example_for(&self, n: u32) -> Option<&ExampleTaskInfo> {
        let tasks = self.examples();
        let n = usize::try_from(n.max(1) - 1).ok()?;
        (!tasks.is_empty()).then(|| &tasks[n % tasks.len()])
    }
}
