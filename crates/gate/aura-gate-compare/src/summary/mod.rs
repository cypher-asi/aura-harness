//! What a comparison's runs and judgments add up to, per arm, per prompt
//! of a replay and per example task, as the views show it.

mod arm;
mod cells;

#[cfg(test)]
mod tests;

pub use arm::{summarize, ArmSummary};
pub use cells::{task_cells, turn_cells, TaskCell, TurnCell};

use aura_core_types::ComparePayload;

/// What every judge call of `payload` cost, as far as each had a price.
#[must_use]
pub fn judge_cost_usd(payload: &ComparePayload) -> f64 {
    payload.judgments.iter().map(|j| j.cost_usd).sum()
}

/// The judge calls of `payload` with no known price, which
/// [`judge_cost_usd`] leaves out.
#[must_use]
pub fn judge_unpriced_calls(payload: &ComparePayload) -> u32 {
    payload.judgments.iter().map(|j| j.unpriced_calls).sum()
}

/// Whether two counts out of a handful of runs are too close to tell
/// the arms apart: within one of each other.
#[must_use]
pub const fn too_close(a: usize, b: usize) -> bool {
    a.abs_diff(b) <= 1
}

/// The median of `values`; the mean of the middle two for an even count.
fn median(values: &[f64]) -> Option<f64> {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

/// The mean of `values`; `None` when there are none.
#[allow(clippy::cast_precision_loss)]
fn mean(values: impl ExactSizeIterator<Item = f64>) -> Option<f64> {
    let count = values.len();
    (count > 0).then(|| values.sum::<f64>() / count as f64)
}
