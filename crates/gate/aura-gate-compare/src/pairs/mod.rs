//! Which runs the judge compares, the order it sees each pair in, and
//! what it is told the runs were asked.

#[cfg(test)]
mod tests;

use aura_core_types::{AgentId, CompareArm};
use std::collections::BTreeMap;
use std::fmt::Write;

/// A run that ended, as pairing runs for the judge sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndedRun {
    /// The arm it ran on.
    pub arm: CompareArm,
    /// Its number within its arm, from 1.
    pub n: u32,
    /// Whether it ended in an error, which leaves it unjudged.
    pub errored: bool,
}

/// Which run of arm A the judge compares with which run of arm B, as
/// indices into `runs`. Runs that errored are left out. The rest pair in
/// order, or, when `by_number`, each with arm B's first run of the same
/// number, which worked on the same example task.
#[must_use]
pub fn judge_pairs(runs: &[EndedRun], by_number: bool) -> Vec<(usize, usize)> {
    let finished = |arm: CompareArm| {
        runs.iter()
            .enumerate()
            .filter(move |(_, run)| run.arm == arm && !run.errored)
    };
    if !by_number {
        return finished(CompareArm::A)
            .zip(finished(CompareArm::B))
            .map(|((a, _), (b, _))| (a, b))
            .collect();
    }
    let mut b_by_number: BTreeMap<u32, usize> = BTreeMap::new();
    for (b, run) in finished(CompareArm::B) {
        b_by_number.entry(run.n).or_insert(b);
    }
    finished(CompareArm::A)
        .filter_map(|(a, run)| Some((a, *b_by_number.get(&run.n)?)))
        .collect()
}

/// A number from `agent` to seed the judge's order with.
#[must_use]
pub fn seed(agent: AgentId) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&agent.0[..8]);
    u64::from_le_bytes(bytes)
}

/// What the judge is told the runs were asked: the prompt, or every
/// prompt in order.
#[must_use]
pub fn judge_task(prompts: &[String]) -> String {
    if let [only] = prompts {
        return only.clone();
    }
    let mut task = String::from("The user sent these messages in order:\n");
    for (i, prompt) in prompts.iter().enumerate() {
        let _ = write!(task, "\n### Message {}\n{prompt}\n", i + 1);
    }
    task
}
