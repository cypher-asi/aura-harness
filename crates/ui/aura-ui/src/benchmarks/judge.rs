//! The blind judge and its verdicts.

use super::BenchCost;

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
