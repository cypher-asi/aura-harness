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
    /// The named run's changes went into the user's workspace.
    Applied(String),
    /// The copies were removed.
    Dismissed,
}

impl BenchStatus {
    /// `running`, `done`, `applied a-2`, ...
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Running => "running".to_string(),
            Self::Done => "done".to_string(),
            Self::Cancelled => "stopped".to_string(),
            Self::Applied(run) => format!("applied {run}"),
            Self::Dismissed => "dismissed".to_string(),
        }
    }

    /// Whether the runs' copies still exist, to diff or apply.
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

/// Where run `name` (`a-1`, `b-2`) of arm `arm` goes among a benchmark's
/// runs: by its number, then A before B. A name with no number goes last.
#[must_use]
pub fn run_order(name: &str, arm: usize) -> (u32, usize) {
    let number = name
        .rsplit_once('-')
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(u32::MAX);
    (number, arm)
}

/// What follows a cost that leaves out calls with no known price.
pub const UNPRICED_MARK: &str = "+?";

/// A list-price cost and how many of the calls in it had no known price,
/// so that it is only what the others cost.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BenchCost {
    /// What the priced calls cost
    pub usd: f64,
    /// Calls with no known price, which `usd` leaves out
    pub unpriced_calls: u32,
}

impl BenchCost {
    /// `usd`, leaving out `unpriced_calls` calls.
    #[must_use]
    pub const fn new(usd: f64, unpriced_calls: u32) -> Self {
        Self {
            usd,
            unpriced_calls,
        }
    }

    /// Whether every call in it had a known price.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        self.unpriced_calls == 0
    }

    /// What it cost when every call had a price; `None` when some had
    /// none, so it cannot be set against another cost.
    #[must_use]
    pub const fn known(self) -> Option<f64> {
        if self.is_complete() {
            Some(self.usd)
        } else {
            None
        }
    }

    /// `money(usd)` when every call had a price, `$0.11+?` when some
    /// had none, and `unknown` when the priced ones cost nothing (or
    /// there were none).
    #[must_use]
    pub fn read(self, money: fn(f64) -> String) -> String {
        if self.is_complete() {
            money(self.usd)
        } else if self.usd <= 0.0 {
            "unknown".to_string()
        } else {
            money(self.usd) + UNPRICED_MARK
        }
    }
}

/// `+? some calls have no known price: glm-9, mystery` when `models`
/// made calls with no known price; `None` when there are none.
#[must_use]
pub fn unpriced_note<'a>(models: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut named: Vec<&str> = Vec::new();
    for model in models {
        if !named.contains(&model) {
            named.push(model);
        }
    }
    (!named.is_empty()).then(|| {
        format!(
            "{UNPRICED_MARK} some calls have no known price: {}",
            named.join(", ")
        )
    })
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

/// What each verified fix cost: everything the arm spent, failed runs
/// included, over the runs whose check passed. `None` before a check
/// passes.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn cost_per_fix(total: BenchCost, fixes: usize) -> Option<BenchCost> {
    (fixes > 0).then(|| BenchCost::new(total.usd / fixes as f64, total.unpriced_calls))
}

/// The "cost per fix" cells of two arms: what each verified fix cost, with
/// how the two compare when both are complete, or `no fix` for an arm
/// none of whose checks passed.
#[must_use]
pub fn per_fix_cells(
    a: Option<BenchCost>,
    b: Option<BenchCost>,
    money: fn(f64) -> String,
) -> [String; 2] {
    let known = |cost: Option<BenchCost>| cost.and_then(BenchCost::known);
    let [a_share, b_share] = cost_shares(known(a), known(b));
    let cell = |cost: Option<BenchCost>, share: String| {
        cost.map_or_else(|| "no fix".to_string(), |cost| cost.read(money) + &share)
    };
    [cell(a, a_share), cell(b, b_share)]
}

/// Tokens and cost one model used across an arm's runs, for the runs'
/// own work or for one purpose beside it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTokens {
    pub model: String,
    /// What the calls were made for beside the runs' own work
    /// (`classify`, `criteria`, `judge`, `title`); `None` for the work
    pub purpose: Option<String>,
    pub calls: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// What its priced calls cost
    pub cost_usd: f64,
    /// Its calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
}

impl BenchTokens {
    /// What its calls cost, as far as they had a price.
    #[must_use]
    pub const fn cost(&self) -> BenchCost {
        BenchCost::new(self.cost_usd, self.unpriced_calls)
    }
}

/// How an arm's runs did on one prompt.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTurn {
    /// Runs that got to the prompt
    pub runs: usize,
    /// Mean cost of their turns
    pub mean_cost_usd: Option<f64>,
    /// Calls of their turns with no known price, which `mean_cost_usd`
    /// leaves out
    pub unpriced_calls: u32,
    /// Turns whose last test or build command passed
    pub passed: usize,
    /// Turns whose last test or build command failed
    pub failed: usize,
    /// Turns that ended in an error
    pub errors: usize,
}

impl BenchTurn {
    /// Mean cost of its turns, as far as their calls had a price.
    #[must_use]
    pub fn mean_cost(&self) -> Option<BenchCost> {
        self.mean_cost_usd
            .map(|usd| BenchCost::new(usd, self.unpriced_calls))
    }
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
    /// What the priced calls of every run cost
    pub total_cost_usd: f64,
    /// Median cost of the finished runs
    pub median_cost_usd: Option<f64>,
    /// Calls of every run with no known price, which `total_cost_usd`
    /// leaves out
    pub unpriced_calls: u32,
    /// Calls of the finished runs with no known price, which
    /// `median_cost_usd` leaves out
    pub finished_unpriced_calls: u32,
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
    /// Tokens per model and purpose, most expensive first
    pub tokens: Vec<BenchTokens>,
    /// One per prompt
    pub turns: Vec<BenchTurn>,
    /// Runs a failed attempt moved to a stronger model
    pub escalated: usize,
    /// Runs Gate asked to check a change they had not checked
    pub nudged: usize,
}

impl BenchArm {
    /// What every run cost, as far as its calls had a price.
    #[must_use]
    pub const fn total_cost(&self) -> BenchCost {
        BenchCost::new(self.total_cost_usd, self.unpriced_calls)
    }

    /// Median cost of the finished runs, as far as their calls had a
    /// price.
    #[must_use]
    pub fn median_cost(&self) -> Option<BenchCost> {
        self.median_cost_usd
            .map(|usd| BenchCost::new(usd, self.finished_unpriced_calls))
    }
}

/// One run of a benchmark.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchRun {
    /// `a-1`, `b-2`, ...
    pub name: String,
    /// 0 for arm A, 1 for arm B
    pub arm: usize,
    /// Model its first turn was sent to
    pub model: String,
    /// The stronger model a failed attempt last moved its work to, or
    /// the same model when the move only raised its effort
    pub escalated_to: Option<String>,
    /// The effort that move raised it to (`max`), when it raised one
    pub escalated_effort: Option<String>,
    /// What the classifier said its first prompt asks for, as stored
    /// (`multi_file_change`); `None` in older records
    pub category: Option<String>,
    /// How hard the classifier said its first prompt's work is (`hard`)
    pub difficulty: Option<String>,
    /// What the classifier found drove that difficulty (`many files,
    /// loose spec, deep exploring`); `None` when it named the difficulty
    /// itself, and in older records
    pub drivers: Option<String>,
    /// Whether Gate had its model lead a turn, handing the work to
    /// subtasks
    pub led: bool,
    /// The part of `cost_usd` its subtasks spent
    pub subtask_cost_usd: f64,
    pub wall_ms: u64,
    /// What its priced calls cost
    pub cost_usd: f64,
    /// Its calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    pub check_passed: Option<bool>,
    pub files_changed: u32,
    pub error: Option<String>,
    /// Id of the example task it worked on, in a benchmark of them
    pub task: Option<String>,
    /// Hex id of the agent it ran as, whose LOG tab shows its trace
    pub agent_id: String,
}

impl BenchRun {
    /// What it cost, as far as its calls had a price.
    #[must_use]
    pub const fn cost(&self) -> BenchCost {
        BenchCost::new(self.cost_usd, self.unpriced_calls)
    }
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
    /// Calls of its runs with no known price, which `mean_cost_usd`
    /// leaves out
    pub unpriced_calls: u32,
}

impl BenchTaskCell {
    /// Mean cost of its runs, as far as their calls had a price.
    #[must_use]
    pub fn mean_cost(&self) -> Option<BenchCost> {
        self.mean_cost_usd
            .map(|usd| BenchCost::new(usd, self.unpriced_calls))
    }
}

/// Where an example task's files stand while its benchmark sets up.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BenchTaskSetup {
    /// Its files are in place, for its runs to copy
    #[default]
    Ready,
    /// Waiting for a turn to fetch its files
    Queued,
    /// Its files are being fetched; `pull` when that pulls a container
    /// image, which can take minutes
    Fetching { pull: bool },
    /// Its files could not be had, so its runs do not run
    Failed(String),
}

impl BenchTaskSetup {
    /// `queued`, `pulling image`, ...; empty when it is ready.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Ready => String::new(),
            Self::Queued => "queued".to_string(),
            Self::Fetching { pull: true } => "pulling image".to_string(),
            Self::Fetching { pull: false } => "fetching files".to_string(),
            Self::Failed(why) => format!("failed: {why}"),
        }
    }
}

/// One example task of a benchmark of them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchTaskRow {
    /// `coding/write-file`, ...
    pub id: String,
    pub title: String,
    /// Arm A, then arm B
    pub arms: [BenchTaskCell; 2],
    pub setup: BenchTaskSetup,
}

/// A run that has started and not ended.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchLiveRun {
    /// `a-1`, ...
    pub name: String,
    /// 0 for A, 1 for B
    pub arm: usize,
    /// The example task it works on
    pub task: Option<String>,
    /// The model its prompt under way went to; `None` until it was routed
    pub model: Option<String>,
    /// What the classifier said that prompt asks for, as stored
    /// (`multi_file_change`), and how hard (`hard`)
    pub category: Option<String>,
    pub difficulty: Option<String>,
    /// How long it has worked; `None` while it waits for its task's files
    pub wall_ms: Option<u64>,
    /// What its priced model calls have cost so far
    pub cost_usd: f64,
    /// Its calls so far with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    /// Tokens its calls have used so far, per model and purpose
    pub tokens: Vec<BenchTokens>,
    /// Hex id of the agent it runs as; `None` while it waits for its
    /// task's files
    pub agent_id: Option<String>,
    /// What it is doing right now; `None` when that is not known
    pub doing: Option<BenchDoing>,
}

impl BenchLiveRun {
    /// What it has cost so far, as far as its calls had a price.
    #[must_use]
    pub const fn cost(&self) -> BenchCost {
        BenchCost::new(self.cost_usd, self.unpriced_calls)
    }
}

/// What a run under way is doing right now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BenchDoing {
    /// `thinking`, `writing`, `waiting for the model`, `running
    /// run_command`, ...
    pub what: String,
    /// How long it has been at it
    pub for_ms: u64,
    /// The model call it is on, counting from 1; 0 while tools run
    pub call: u32,
    /// Times this model call started its reply over
    pub restarts: u32,
    /// What its subtask is doing, while one runs
    pub subtask: Option<Box<BenchDoing>>,
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
    /// What its priced calls cost
    pub cost_usd: f64,
    /// Its calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    pub verdicts: Vec<BenchVerdict>,
}

impl BenchJudge {
    /// What its calls cost, as far as they had a price.
    #[must_use]
    pub const fn cost(&self) -> BenchCost {
        BenchCost::new(self.cost_usd, self.unpriced_calls)
    }
}

/// What a replayed session cost when it ran.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchBaseline {
    pub calls: u32,
    pub cost_usd: f64,
    /// Whether any of its calls had a price
    pub priced: bool,
    /// Calls with no known price, which `cost_usd` leaves out
    pub unpriced_calls: u32,
    pub models: Vec<String>,
}

impl BenchBaseline {
    /// What the session cost as far as its calls had a price; `None`
    /// when none had one.
    #[must_use]
    pub fn cost(&self) -> Option<BenchCost> {
        self.priced
            .then(|| BenchCost::new(self.cost_usd, self.unpriced_calls))
    }
}

/// A benchmark as the BENCH tab shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BenchView {
    /// Hex id of the benchmark's own agent
    pub id: String,
    /// Hex id of the agent at the top of the tree it was asked from
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
    /// Model that classified the runs' prompts; empty for benchmarks
    /// recorded before they named it
    pub classifier: String,
    /// The prompts each run got, in order
    pub prompts: Vec<String>,
    /// Arm A, then arm B
    pub arms: [BenchArm; 2],
    /// Runs that ended, in [`run_order`]
    pub runs: Vec<BenchRun>,
    /// Runs under way, in [`run_order`]; only while it runs
    pub live: Vec<BenchLiveRun>,
    pub judge: Option<BenchJudge>,
    /// The classifier's calls, made once per prompt for both arms and in
    /// neither arm's costs; empty until it ends
    pub classify: Vec<BenchTokens>,
    /// What a replayed session cost when it ran
    pub baseline: Option<BenchBaseline>,
    /// Whether the judge had what the user kept to measure against
    pub has_reference: bool,
    pub notes: Vec<String>,
}

/// What a benchmark is doing, as its progress shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchPhase {
    /// Fetching example tasks' files before any run works
    SettingUp,
    Running,
    /// Every run ended; the judge compares them
    Judging,
    Done,
    /// The user stopped it
    Stopped,
}

impl BenchPhase {
    /// `Setting up`, `Running`, ...
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SettingUp => "Setting up",
            Self::Running => "Running",
            Self::Judging => "Judging",
            Self::Done => "Done",
            Self::Stopped => "Stopped",
        }
    }

    /// Whether it is still going.
    #[must_use]
    pub const fn under_way(self) -> bool {
        matches!(self, Self::SettingUp | Self::Running | Self::Judging)
    }
}

/// How far a benchmark's runs have got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchProgress {
    /// Runs it makes: per arm, `repeats` of each example task, or
    /// `repeats` of its work
    pub total: usize,
    /// Runs that ended
    pub done: usize,
    /// Runs that ended in an error
    pub failed: usize,
    /// Runs working now, not waiting for their task's files
    pub working: usize,
    pub phase: BenchPhase,
}

impl BenchView {
    /// How far its runs have got and what it is doing.
    #[must_use]
    pub fn progress(&self) -> BenchProgress {
        let repeats = usize::try_from(self.repeats.max(1)).unwrap_or(usize::MAX);
        let done = self.runs.len();
        let total = repeats
            .saturating_mul(self.tasks.len().max(1))
            .saturating_mul(2)
            .max(done + self.live.len());
        let working = self.live.iter().filter(|r| r.wall_ms.is_some()).count();
        let fetching = self.tasks.iter().any(|t| {
            matches!(
                t.setup,
                BenchTaskSetup::Queued | BenchTaskSetup::Fetching { .. }
            )
        });
        let phase = match self.status {
            BenchStatus::Running if done == 0 && working == 0 && fetching => BenchPhase::SettingUp,
            BenchStatus::Running if done >= total && self.live.is_empty() => BenchPhase::Judging,
            BenchStatus::Running => BenchPhase::Running,
            BenchStatus::Cancelled => BenchPhase::Stopped,
            _ => BenchPhase::Done,
        };
        BenchProgress {
            total,
            done,
            failed: self.runs.iter().filter(|r| r.error.is_some()).count(),
            working,
            phase,
        }
    }
}

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

    fn money(usd: f64) -> String {
        if usd <= 0.0 {
            "free".to_string()
        } else {
            format!("${usd:.2}")
        }
    }

    #[test]
    fn a_cost_with_unpriced_calls_reads_as_a_part_or_unknown_never_free() {
        assert_eq!(BenchCost::new(0.0, 0).read(money), "free", "a local model");
        assert_eq!(BenchCost::new(0.11, 0).read(money), "$0.11");
        assert_eq!(BenchCost::new(0.11, 2).read(money), "$0.11+?");
        assert_eq!(BenchCost::new(0.0, 3).read(money), "unknown");
        assert_eq!(BenchCost::new(0.11, 2).known(), None);
        assert_eq!(BenchCost::new(0.11, 0).known(), Some(0.11));
    }

    #[test]
    fn a_partly_priced_baseline_has_no_known_cost() {
        let baseline = |priced, unpriced_calls| BenchBaseline {
            calls: 4,
            cost_usd: 0.2,
            priced,
            unpriced_calls,
            models: Vec::new(),
        };
        assert_eq!(
            baseline(true, 0).cost().and_then(BenchCost::known),
            Some(0.2)
        );
        let partly = baseline(true, 1).cost();
        assert_eq!(partly.map(|c| c.read(money)).as_deref(), Some("$0.20+?"));
        assert_eq!(partly.and_then(BenchCost::known), None);
        assert_eq!(baseline(false, 4).cost(), None);
    }

    #[test]
    fn per_fix_cells_compare_only_complete_costs() {
        let fix = |usd, unpriced| cost_per_fix(BenchCost::new(usd, unpriced), 2);
        assert_eq!(
            per_fix_cells(fix(0.4, 0), fix(0.2, 0), money),
            ["$0.20 · 100%", "$0.10 · 50% less"]
        );
        assert_eq!(
            per_fix_cells(fix(0.4, 0), fix(0.0, 4), money),
            ["$0.20", "unknown"],
            "no -100% from a missing price"
        );
        assert_eq!(
            per_fix_cells(None, fix(0.2, 1), money),
            ["no fix", "$0.10+?"]
        );
    }

    #[test]
    fn the_unpriced_note_names_each_model_once() {
        assert_eq!(unpriced_note([]), None);
        assert_eq!(
            unpriced_note(["glm-9", "mystery", "glm-9"]).as_deref(),
            Some("+? some calls have no known price: glm-9, mystery")
        );
    }

    #[test]
    fn progress_counts_runs_and_follows_the_phases() {
        let task = |setup| BenchTaskRow {
            setup,
            ..BenchTaskRow::default()
        };
        let mut view = BenchView {
            repeats: 2,
            tasks: vec![
                task(BenchTaskSetup::Fetching { pull: true }),
                task(BenchTaskSetup::Queued),
            ],
            live: vec![BenchLiveRun::default()],
            ..BenchView::default()
        };
        let progress = view.progress();
        assert_eq!((progress.total, progress.done), (8, 0));
        assert_eq!(progress.phase, BenchPhase::SettingUp);

        view.tasks[0].setup = BenchTaskSetup::Ready;
        view.live[0].wall_ms = Some(1_000);
        view.runs = vec![
            BenchRun::default(),
            BenchRun {
                error: Some("timed out".into()),
                ..BenchRun::default()
            },
        ];
        let progress = view.progress();
        assert_eq!(progress.phase, BenchPhase::Running);
        assert_eq!(
            (progress.done, progress.failed, progress.working),
            (2, 1, 1)
        );

        view.live.clear();
        view.runs = vec![BenchRun::default(); 8];
        assert_eq!(view.progress().phase, BenchPhase::Judging);
        view.status = BenchStatus::Applied("a-1".into());
        assert_eq!(view.progress().phase, BenchPhase::Done);
        assert!(!BenchPhase::Done.under_way());
        view.status = BenchStatus::Cancelled;
        assert_eq!(view.progress().phase, BenchPhase::Stopped);
    }

    #[test]
    fn statuses_read_naturally_and_know_their_copies() {
        assert_eq!(BenchStatus::Applied("a-2".into()).label(), "applied a-2");
        assert!(BenchStatus::Done.has_copies());
        assert!(!BenchStatus::Dismissed.has_copies());
        assert_eq!(arm_letter(0), "A");
    }
}
