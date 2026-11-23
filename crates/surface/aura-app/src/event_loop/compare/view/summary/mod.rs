//! The view of one comparison: its heading, then its sections in order.
//!
//! - `arms`: the arms side by side, whether they are too close to call,
//!   and what the replayed session itself cost
//! - `breakdown`: each arm per example task and per prompt
//! - `usage`: the tokens and plan usage of each arm, and the judge's calls
//! - `runs`: one line per run, the notes, and what can be done next

mod arms;
mod breakdown;
mod runs;
mod usage;

use super::format::{arm_label, headline, plural, status};
use aura_core_types::{BenchSource, CompareArm, ComparePayload};
use aura_gate::compare::{summarize, ArmSummary};
use std::fmt::Write;

/// Characters arm A's column takes at least.
const MIN_ARM_WIDTH: usize = 32;

/// The view of one comparison.
pub(crate) fn render(name: &str, payload: &ComparePayload) -> String {
    let a_label = format!("A {}", payload.arm_a.label());
    let summary = Summary {
        width: a_label.chars().count().max(MIN_ARM_WIDTH),
        a_label,
        b_label: format!("B {}", payload.arm_b.label()),
        a: summarize(payload, CompareArm::A),
        b: summarize(payload, CompareArm::B),
        payload,
    };
    let mut out = String::new();
    summary.render_heading(&mut out, name);
    summary.render_arms(&mut out);
    summary.render_breakdown(&mut out);
    summary.render_usage(&mut out);
    summary.render_runs(&mut out);
    out
}

/// One comparison and what each arm's runs came to.
struct Summary<'a> {
    payload: &'a ComparePayload,
    a: ArmSummary,
    b: ArmSummary,
    /// Arm A's column heading
    a_label: String,
    /// Arm B's column heading
    b_label: String,
    /// Characters arm A's column takes
    width: usize,
}

impl Summary<'_> {
    /// The comparison's name, task and status, the session it replays,
    /// and what its arms ran.
    fn render_heading(&self, out: &mut String, name: &str) {
        let payload = self.payload;
        let _ = write!(
            out,
            "{name} · \"{}\" · {}",
            headline(&payload.task, 72),
            status(&payload.status)
        );
        if let BenchSource::Session { title, .. } = &payload.source {
            let prompts = payload.prompts.len();
            let _ = write!(
                out,
                "\n  Replays the session \"{}\": {prompts} prompt{} in order",
                headline(title, 60),
                plural(prompts),
            );
        }
        let _ = write!(
            out,
            "\n  A {} vs B {}, {} run{} ",
            arm_label(&payload.arm_a),
            arm_label(&payload.arm_b),
            payload.repeats,
            plural(payload.repeats),
        );
        let examples = payload.source.examples().len();
        if examples == 0 {
            out.push_str("each");
        } else {
            let _ = write!(
                out,
                "of each of {examples} example task{} per arm",
                plural(examples)
            );
        }
        match payload.check.as_deref() {
            _ if examples > 0 => out.push_str(", each task's own check"),
            Some(check) => {
                let _ = write!(out, ", check `{check}`");
            }
            None => out.push_str(", no check command"),
        }
    }
}
