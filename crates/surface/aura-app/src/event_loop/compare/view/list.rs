//! One line per past comparison, numbered for `/compare show <n>`.

use super::format::{arm_label, cost, headline, status};
use crate::record_loader::format_local_time;
use aura_core_types::{CompareArm, ComparePayload};
use aura_gate::compare::{summarize, ArmSummary};
use std::fmt::Write;

/// One line per past comparison, oldest first, numbered for
/// `/compare show <n>`.
pub(crate) fn list(comparisons: &[(String, Option<ComparePayload>)]) -> String {
    if comparisons.is_empty() {
        return "No comparisons yet. /compare [prompt] runs your last prompt (or the one \
                given) with Gate and with Opus side by side."
            .to_string();
    }
    let mut out = String::from("Comparisons (/compare show <n> for one):");
    for (n, (name, payload)) in (1..).zip(comparisons) {
        let Some(payload) = payload else {
            let _ = write!(out, "\n  {n}. {name} · no record");
            continue;
        };
        let _ = write!(
            out,
            "\n  {n}. {name} · {} · \"{}\" · {} vs {} · {} · A ",
            format_local_time(payload.started_ms),
            headline(&payload.task, 48),
            arm_label(&payload.arm_a),
            arm_label(&payload.arm_b),
            status(&payload.status),
        );
        write_arm(&mut out, &summarize(payload, CompareArm::A));
        out.push_str(" / B ");
        write_arm(&mut out, &summarize(payload, CompareArm::B));
    }
    out
}

/// `$0.06 median, 2/3 pass` for one arm: its median run cost, and how
/// many of its checks passed when any ran.
fn write_arm(out: &mut String, arm: &ArmSummary) {
    match arm.median_cost_usd {
        Some(usd) => out.push_str(&cost(usd, arm.finished_unpriced_calls)),
        None => out.push('-'),
    }
    out.push_str(" median");
    if arm.checks_run > 0 {
        let _ = write!(out, ", {}/{} pass", arm.checks_passed, arm.checks_run);
    }
}
