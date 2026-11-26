//! Setting up new benchmarks: examples, sessions, specs and requests.

/// A built-in example task, as the BENCH tab lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BenchExample {
    /// The suite it is from, as `gate_hard`
    pub suite: String,
    pub title: String,
}

/// An earlier session of the agent on screen, to replay.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchSession {
    pub started_ms: u64,
    /// Its first prompt, cut short
    pub title: String,
    /// Prompts it would replay
    pub prompts: usize,
    /// What it cost when it ran; `None` when none of its calls had a price
    pub cost_usd: Option<f64>,
    /// Calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    pub models: Vec<String>,
    /// Whether it is the agent's current session
    pub current: bool,
}

/// What each run of a new benchmark works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BenchWork {
    /// This prompt
    Prompt(String),
    /// The user's last prompt
    LastPrompt,
    /// The agent's current session, replayed
    CurrentSession,
    /// The session that started at this Unix millisecond, replayed
    Session(u64),
    /// The first `tasks` built-in example coding tasks of `suite` (of
    /// every suite when `None`), each in its own fixture and checked by
    /// its own command
    Examples { suite: Option<String>, tasks: usize },
}

/// A new benchmark as the BENCH tab's form sets it up.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchSpec {
    pub work: BenchWork,
    /// Arm A: `gate`, `gate:nodelegate`, or a model id
    pub a: String,
    /// Arm B
    pub b: String,
    /// Runs per arm, or per example task and arm
    pub repeats: u32,
    /// Command run in each copy after its run; `None` for the project's.
    /// Example tasks use their own.
    pub check: Option<String>,
    pub budget_usd: Option<f64>,
    /// Model that classifies each run's prompt; `None` for Gate's
    /// automatic choice
    pub classifier: Option<String>,
}

/// What the BENCH tab asks of the event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum BenchRequest {
    /// Every benchmark, of every tree ([`crate::UiCommand::BenchList`])
    List,
    /// The sessions of the agent it asks for and the example tasks
    /// ([`crate::UiCommand::BenchSessions`])
    Sessions,
    /// Set up `spec` and say what it should cost
    /// ([`crate::UiCommand::BenchEstimate`])
    Prepare(BenchSpec),
    /// Start the benchmark set up last
    Start,
    /// Drop the one set up, or stop the one running
    Cancel,
    /// Put run `run`'s changes in the user's workspace
    Apply { id: String, run: String },
    /// Show what run `run` changed, or the first run of each arm
    Diff { id: String, run: Option<String> },
    /// Remove the copies of benchmark `id`
    Dismiss { id: String },
}
