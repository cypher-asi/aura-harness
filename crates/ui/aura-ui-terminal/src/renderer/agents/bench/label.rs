//! What a benchmark's row in the BENCH list says: its number and its work,
//! `#51 swe-pro ×20`, then its arms, `gate vs opus-5-5`. Only the list
//! shortens it so; everywhere else the benchmark goes by its agent's name,
//! `Compare #51`.

use crate::components::truncate_record_info;
use crate::events::BenchView;
use crate::renderer::benchmarks::{short_model, task_prefix};

/// The most columns a prompt, a replayed session's title or a plan's file
/// name takes, so the arms after it still fit.
const WORK_WIDTH: usize = 20;

/// Suites that go by a shorter name than their directory's.
const SUITE_NAMES: &[(&str, &str)] = &[("swe_bench_pro_hard", "swe-pro")];

/// `#51 swe-pro ×20`: the benchmark's name without its `Compare `, then
/// what it ran.
pub(super) fn name(view: &BenchView) -> String {
    let number = view.name.strip_prefix("Compare ").unwrap_or(&view.name);
    let work = work(view);
    if work.is_empty() {
        number.to_string()
    } else {
        format!("{number} {work}")
    }
}

/// `gate vs opus-5-5`: what each arm runs on, or `None` when either arm
/// has no label, as in benchmarks recorded before arms had one.
pub(super) fn arms(view: &BenchView) -> Option<String> {
    let [a, b] = &view.arms;
    if a.label.is_empty() || b.label.is_empty() {
        return None;
    }
    Some(format!(
        "{} vs {}",
        short_model(&a.label),
        short_model(&b.label)
    ))
}

/// What a benchmark ran: `swe-pro ×20` for example tasks of one suite,
/// `mixed ×20` for tasks of several, `↻ <title>` for a replayed session,
/// `plan <file name>` for a plan's build, else its prompt's first line.
fn work(view: &BenchView) -> String {
    if !view.tasks.is_empty() {
        let suite = task_prefix(view);
        let suite = suite.as_deref().map_or("mixed", suite_name);
        return format!("{suite} ×{}", view.tasks.len());
    }
    let title = first_line(&view.title);
    let text = if view.source.starts_with("session") {
        format!("↻ {title}")
    } else if let Some(plan) = view.source.strip_prefix("plan ") {
        format!("plan {}", plan.rsplit('/').next().unwrap_or(plan))
    } else {
        title.to_string()
    };
    truncate_record_info(&text, WORK_WIDTH)
}

/// `swe-pro` for `swe_bench_pro_hard`; other suites keep their name.
fn suite_name(suite: &str) -> &str {
    SUITE_NAMES
        .iter()
        .find(|&&(long, _)| long == suite)
        .map_or(suite, |&(_, short)| short)
}

/// The first line of `text` with anything on it, trimmed.
fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
}
