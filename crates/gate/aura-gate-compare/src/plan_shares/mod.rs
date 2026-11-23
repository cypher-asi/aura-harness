//! The share of each plan window each run of a comparison used.

#[cfg(test)]
mod tests;

use aura_core_types::{Plan, PlanShare, PlanUsage};
use std::collections::btree_map::{BTreeMap, Entry};

/// A plan reading one model call of a run got.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanReading {
    /// When the call was recorded, in Unix milliseconds.
    pub recorded_ms: u64,
    /// What the provider reported.
    pub usage: PlanUsage,
}

/// How far a window's reset time must move for a lower reading to mean
/// the window restarted, rather than a reading recorded late. Covers
/// resets computed from a relative `reset-after-seconds`.
const RESET_SLACK_SECS: u64 = 60;

/// A plan window: the plan and the window's name. Maps keyed by it list
/// windows in the order shares are shown in.
type Window<'a> = (Plan, &'a str);

/// The share of each plan window each run used, from the plan readings
/// of every run's model calls (`readings[i]` for run `i`).
///
/// A reading is of the whole account, so runs working at the same time
/// on the same plan see each other's usage, and a run's first and last
/// readings would count that usage once per run. Instead, every rise of
/// a window from its highest reading so far, across all runs in the
/// order their calls were recorded, goes to the run whose call saw it:
/// nothing is counted twice, and the shares add up to how far the window
/// moved during the comparison. The first reading of a window is where
/// it started and adds nothing. A lower reading is one recorded late and
/// adds nothing, unless the window's reset time moved on, when the
/// window restarted and the reading counts from zero. Usage from outside
/// the comparison while it ran (another session on the same plan) goes
/// to whichever run read the window next.
#[must_use]
pub fn plan_shares(readings: &[&[PlanReading]]) -> Vec<Vec<PlanShare>> {
    let mut tops = Tops::default();
    let mut shares: Vec<BTreeMap<Window, PlanShare>> = vec![BTreeMap::new(); readings.len()];
    for (run, usage) in in_recorded_order(readings) {
        let added = tops.rise(usage);
        count(&mut shares[run], usage, added);
    }
    shares
        .into_iter()
        .map(|row| row.into_values().collect())
        .collect()
}

/// Every reading with the run that got it, in the order the calls were
/// recorded.
fn in_recorded_order<'a>(
    readings: &[&'a [PlanReading]],
) -> impl Iterator<Item = (usize, &'a PlanUsage)> {
    let mut ordered: Vec<(u64, usize, &PlanUsage)> = readings
        .iter()
        .enumerate()
        .flat_map(|(run, &calls)| {
            calls
                .iter()
                .map(move |call| (call.recorded_ms, run, &call.usage))
        })
        .collect();
    // Stable: calls recorded in the same millisecond stay in run order,
    // then in the order each run made them.
    ordered.sort_by_key(|&(ms, _, _)| ms);
    ordered.into_iter().map(|(_, run, usage)| (run, usage))
}

/// The highest reading of each window so far, and its reset time.
#[derive(Default)]
struct Tops<'a>(BTreeMap<Window<'a>, (f64, Option<u64>)>);

impl<'a> Tops<'a> {
    /// How far `usage` raises its window above the highest reading so
    /// far, which it then becomes: nothing for the window's first reading
    /// or a late one, and all of it when the window restarted.
    fn rise(&mut self, usage: &'a PlanUsage) -> f64 {
        let (top, reset) = match self.0.entry((usage.plan, usage.window.as_str())) {
            Entry::Vacant(slot) => {
                slot.insert((usage.used, usage.resets_at));
                return 0.0;
            }
            Entry::Occupied(slot) => slot.into_mut(),
        };
        if restarted(*reset, usage.resets_at) {
            (*top, *reset) = (usage.used, usage.resets_at);
            return usage.used;
        }
        if usage.used > *top {
            let added = usage.used - *top;
            (*top, *reset) = (usage.used, usage.resets_at.or(*reset));
            return added;
        }
        0.0
    }
}

/// Whether a window that reset at `old` restarted, by a reading saying it
/// resets at `new`.
fn restarted(old: Option<u64>, new: Option<u64>) -> bool {
    matches!((old, new), (Some(old), Some(new)) if new > old.saturating_add(RESET_SLACK_SECS))
}

/// Count `usage`, a reading that raised its window by `added`, in its
/// run's shares.
fn count<'a>(shares: &mut BTreeMap<Window<'a>, PlanShare>, usage: &'a PlanUsage, added: f64) {
    match shares.entry((usage.plan, usage.window.as_str())) {
        Entry::Occupied(share) => {
            let share = share.into_mut();
            share.last = usage.used;
            share.used += added;
        }
        Entry::Vacant(slot) => {
            slot.insert(PlanShare {
                plan: usage.plan,
                window: usage.window.clone(),
                first: usage.used,
                last: usage.used,
                used: added,
            });
        }
    }
}

/// `rows` added up per plan window: the first `first`, the last `last`,
/// and every `used`.
pub(crate) fn merge_plan_shares<'a>(
    rows: impl IntoIterator<Item = &'a PlanShare>,
) -> Vec<PlanShare> {
    let mut merged: BTreeMap<Window, PlanShare> = BTreeMap::new();
    for row in rows {
        match merged.entry((row.plan, row.window.as_str())) {
            Entry::Occupied(sum) => {
                let sum = sum.into_mut();
                sum.last = row.last;
                sum.used += row.used;
            }
            Entry::Vacant(slot) => {
                slot.insert(row.clone());
            }
        }
    }
    merged.into_values().collect()
}
