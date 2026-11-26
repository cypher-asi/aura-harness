//! Example tasks of a benchmark of them and how each arm did on them.

use super::BenchCost;

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
