//! The chat panel's BENCH tab: comparisons of two arms (each Gate or a
//! pinned model) on the same work, what they cost, and how well they did.
//!
//! The work is one prompt, the user's last prompt, or an earlier session
//! replayed prompt by prompt. Benchmarks are `/compare` comparisons; the
//! tab lists them, sets new ones up and shows each one's numbers.

/// Where a benchmark stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BenchStatus {
    /// Runs are under way.
    #[default]
    Running,
    /// Every run ended; the copies are kept.
    Done,
    /// The user stopped it.
    Cancelled,
    /// The user picked arm 0 (A) or 1 (B).
    Picked(usize),
    /// The named run's changes went into the user's workspace.
    Applied(String),
    /// The copies were removed.
    Dismissed,
}

impl BenchStatus {
    /// `running`, `done`, `picked B`, ...
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Running => "running".to_string(),
            Self::Done => "done".to_string(),
            Self::Cancelled => "stopped".to_string(),
            Self::Picked(arm) => format!("picked {}", arm_letter(*arm)),
            Self::Applied(run) => format!("applied {run}"),
            Self::Dismissed => "dismissed".to_string(),
        }
    }

    /// Whether the runs' copies still exist, to diff, pick or apply.
    #[must_use]
    pub const fn has_copies(&self) -> bool {
        !matches!(self, Self::Applied(_) | Self::Dismissed)
    }
}

/// `A` for arm 0, `B` for arm 1.
#[must_use]
pub const fn arm_letter(arm: usize) -> &'static str {
    if arm == 0 {
        "A"
    } else {
        "B"
    }
}

/// How two costs compare, for after each: ` · 100%` for the larger and
/// ` · 33% less` for the other, both ` · 100%` when they are about the
/// same. Empty when either is unknown or the larger is nothing.
#[must_use]
pub fn cost_shares(a: Option<f64>, b: Option<f64>) -> [String; 2] {
    let (Some(a), Some(b)) = (a, b) else {
        return [String::new(), String::new()];
    };
    let larger = a.max(b);
    if larger <= 0.0 {
        return [String::new(), String::new()];
    }
    let share = |cost: f64| {
        let less = ((1.0 - cost / larger) * 100.0).round();
        if less < 1.0 {
            " · 100%".to_string()
        } else {
            format!(" · {less:.0}% less")
        }
    };
    [share(a), share(b)]
}

/// Tokens and cost one model used across an arm's runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTokens {
    pub model: String,
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// How an arm's runs did on one prompt.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTurn {
    /// Runs that got to the prompt
    pub runs: usize,
    /// Mean cost of their turns
    pub mean_cost_usd: Option<f64>,
    /// Turns whose last test or build command passed
    pub passed: usize,
    /// Turns whose last test or build command failed
    pub failed: usize,
    /// Turns that ended in an error
    pub errors: usize,
}

/// One arm of a benchmark.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchArm {
    /// What it runs on: `gate`, `gate:nodelegate`, or a model
    pub label: String,
    /// Runs that started
    pub runs: usize,
    /// Runs that ended without an error
    pub finished: usize,
    pub total_cost_usd: f64,
    pub median_cost_usd: Option<f64>,
    pub median_wall_ms: Option<u64>,
    pub checks_passed: usize,
    pub checks_run: usize,
    pub judge_wins: usize,
    pub judge_losses: usize,
    pub judge_ties: usize,
    /// Runs the judge rated better than, as good as, or worse than what
    /// the user kept from a replayed session
    pub ref_better: usize,
    pub ref_as_good: usize,
    pub ref_worse: usize,
    /// Tokens per model, most expensive first
    pub tokens: Vec<BenchTokens>,
    /// One per prompt
    pub turns: Vec<BenchTurn>,
}

/// One run of a benchmark.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchRun {
    /// `a-1`, `b-2`, ...
    pub name: String,
    /// 0 for arm A, 1 for arm B
    pub arm: usize,
    /// Model that ran its first turn
    pub model: String,
    pub wall_ms: u64,
    pub cost_usd: f64,
    pub check_passed: Option<bool>,
    pub files_changed: u32,
    pub error: Option<String>,
    /// Id of the example task it worked on, in a benchmark of them
    pub task: Option<String>,
}

/// How an arm's runs did on one example task.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTaskCell {
    /// Runs that worked on it
    pub runs: usize,
    /// Runs whose check passed
    pub passed: usize,
    /// Runs whose check ran
    pub checked: usize,
    pub mean_cost_usd: Option<f64>,
}

/// One example task of a benchmark of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTaskRow {
    /// `coding/write-file`, ...
    pub id: String,
    pub title: String,
    /// Arm A, then arm B
    pub arms: [BenchTaskCell; 2],
}

/// The judge's verdict on one pair of runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchVerdict {
    pub a_run: String,
    pub b_run: String,
    /// Whether the judge gave a readable verdict
    pub parsed: bool,
    /// 0 for A, 1 for B; `None` for a tie
    pub winner: Option<usize>,
    /// `better`, `as good` or `worse` than the reference
    pub a_vs_reference: Option<String>,
    pub b_vs_reference: Option<String>,
    pub reasoning: String,
}

/// The blind judge of a benchmark.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchJudge {
    pub model: String,
    pub cost_usd: f64,
    pub verdicts: Vec<BenchVerdict>,
}

/// What a replayed session cost when it ran.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchBaseline {
    pub calls: u32,
    pub cost_usd: f64,
    /// Whether any of its calls had a price
    pub priced: bool,
    pub models: Vec<String>,
}

/// A benchmark as the BENCH tab shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchView {
    /// Hex id of the benchmark's own agent
    pub id: String,
    /// Hex id of the agent at the top of the tree it was asked from; the
    /// BENCH tab of an agent lists the benchmarks of its tree
    pub root: String,
    /// `Compare #2`
    pub name: String,
    /// What it is about: its prompt, or a replayed session's first one
    pub title: String,
    pub started_ms: u64,
    /// `prompt`, `plan <file>`, `session "<title>"`, or `5 example tasks`
    pub source: String,
    pub status: BenchStatus,
    /// Runs per arm, or per example task and arm
    pub repeats: u32,
    /// The example tasks it ran, when its work is example tasks
    pub tasks: Vec<BenchTaskRow>,
    pub check: Option<String>,
    pub budget_usd: Option<f64>,
    /// The prompts each run got, in order
    pub prompts: Vec<String>,
    /// Arm A, then arm B
    pub arms: [BenchArm; 2],
    pub runs: Vec<BenchRun>,
    pub judge: Option<BenchJudge>,
    /// What a replayed session cost when it ran
    pub baseline: Option<BenchBaseline>,
    /// Whether the judge had what the user kept to measure against
    pub has_reference: bool,
    pub notes: Vec<String>,
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
    /// The first `tasks` built-in example coding tasks, each in its own
    /// fixture and checked by its own command
    Examples { tasks: usize },
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
}

/// What the BENCH tab asks of the event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum BenchRequest {
    /// The benchmarks of the tree of the agent on screen
    /// ([`crate::UiCommand::BenchList`])
    List,
    /// The sessions of the agent on screen and the example tasks
    /// ([`crate::UiCommand::BenchSessions`])
    Sessions,
    /// Set up `spec` and say what it should cost
    /// ([`crate::UiCommand::BenchEstimate`])
    Prepare(BenchSpec),
    /// Start the benchmark set up last
    Start,
    /// Drop the one set up, or stop the one running
    Cancel,
    /// Rate every run of `arm` (0 or 1) of benchmark `id` good and the
    /// other arm's bad
    Pick { id: String, arm: usize },
    /// Put run `run`'s changes in the user's workspace
    Apply { id: String, run: String },
    /// Show what run `run` changed, or the first run of each arm
    Diff { id: String, run: Option<String> },
    /// Remove the copies of benchmark `id`
    Dismiss { id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_larger_cost_is_the_whole_and_the_other_says_how_much_less() {
        assert_eq!(
            cost_shares(Some(0.14), Some(0.21)),
            [" · 33% less", " · 100%"]
        );
        assert_eq!(
            cost_shares(Some(0.11), Some(0.04)),
            [" · 100%", " · 64% less"]
        );
        assert_eq!(cost_shares(Some(1.0), Some(0.996)), [" · 100%", " · 100%"]);
        assert_eq!(cost_shares(Some(0.0), Some(0.0)), ["", ""]);
        assert_eq!(cost_shares(None, Some(0.2)), ["", ""]);
    }

    #[test]
    fn statuses_read_naturally_and_know_their_copies() {
        assert_eq!(BenchStatus::Picked(1).label(), "picked B");
        assert_eq!(BenchStatus::Applied("a-2".into()).label(), "applied a-2");
        assert!(BenchStatus::Done.has_copies());
        assert!(!BenchStatus::Dismissed.has_copies());
        assert_eq!(arm_letter(0), "A");
    }
}
