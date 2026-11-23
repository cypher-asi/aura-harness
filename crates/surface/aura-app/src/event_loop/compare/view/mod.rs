//! What `/compare` shows: the estimate before a comparison starts, the
//! per-arm (and, for a replayed session, per-prompt) view once it ends, the list of past comparisons, and run
//! diffs split per file for the diff view.

use aura_core_types::{
    ArmSpec, BenchSource, CompareArm, ComparePayload, CompareRun, CompareStatus, ExampleTaskInfo,
    ModelTokens, Plan, PlanShare, RunTurn,
};
use aura_gate::compare::{
    judge_cost_usd, summarize, task_cells, too_close, turn_cells, ArmSummary, Estimate, TaskCell,
    TurnCell, NO_DELEGATE,
};
use aura_model::model_catalog::route_label;
use aura_ui_terminal::events::{cost_per_fix, cost_shares, per_fix_cells, FileDiffData};
use std::fmt::Write;

/// Why plan usage is not shown when no call reported it.
const PLAN_USAGE_NOTE: &str = "Plan usage: no call reported a plan's usage (only Claude and \
ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.";

/// How the plan shares are worked out, shown with them.
const PLAN_SHARE_NOTE: &str = "Plan readings cover the whole account: each rise between \
readings goes to the run whose call saw it, so runs at the same time don't count it twice, \
and use outside the comparison while it ran lands on the next run to read.";

/// `$0.004`, `$1.25`, or `free` for nothing.
pub(crate) fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

/// `850ms`, `42s`, `3m 05s`.
pub(crate) fn duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m {:02}s", secs / 60, secs % 60)
}

/// `950`, `12k`, `1.4M` tokens.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", (n as f64 / 1_000.0).round())
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// The first line of `text`, cut to `max` characters.
pub(crate) fn headline(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// How an arm reads: its model and route, or Gate and whether its lead
/// must delegate.
pub(super) fn arm_label(spec: &ArmSpec) -> String {
    match spec {
        ArmSpec::Gate { delegate: true } => "gate".to_string(),
        ArmSpec::Gate { delegate: false } => {
            format!("gate without enforced delegation ({NO_DELEGATE})")
        }
        ArmSpec::Model { model } => route_label(model),
        ArmSpec::ByDifficulty { easy, medium, hard } => format!(
            "by difficulty: easy → {} · medium → {} · hard → {}",
            route_label(easy),
            route_label(medium),
            route_label(hard)
        ),
    }
}

/// What the user is asked to confirm before a comparison starts.
pub(super) struct EstimateView<'a> {
    pub task: &'a str,
    /// Prompts each run sends
    pub prompts: usize,
    /// The replayed session's title, when the work is one
    pub session: Option<&'a str>,
    /// The example tasks, when the work is those
    pub examples: &'a [ExampleTaskInfo],
    /// Arm A as it reads, with Gate's likely pick when Gate routes it
    pub a: String,
    pub b: String,
    pub judge: &'a str,
    pub repeats: u32,
    pub check: Option<&'a str>,
    pub budget_usd: Option<f64>,
    pub concurrency: usize,
    pub estimate: Estimate,
    pub notes: &'a [String],
}

impl EstimateView<'_> {
    /// A heading, then one labelled row per fact, as the BENCH form lays
    /// out its own rows.
    pub(super) fn render(&self) -> String {
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut out = match self.session {
            _ if !self.examples.is_empty() => format!(
                "Benchmark: {} example coding task{}",
                self.examples.len(),
                plural(self.examples.len()),
            ),
            Some(title) => format!(
                "Benchmark: replay the session \"{}\", {} prompt{} in order",
                headline(title, 72),
                self.prompts,
                plural(self.prompts),
            ),
            None => format!("Compare: \"{}\"", headline(self.task, 80)),
        };
        if !self.examples.is_empty() {
            let ids: Vec<&str> = self.examples.iter().map(|t| t.id.as_str()).collect();
            row(&mut out, "Tasks", &ids.join(", "));
        }
        row(&mut out, "A", &self.a);
        row(&mut out, "B", &self.b);
        let repeats = self.repeats as usize;
        let runs = if self.examples.is_empty() {
            format!(
                "{repeats} per arm, up to {} at a time,\neach in its own copy of the workspace",
                self.concurrency
            )
        } else {
            format!(
                "{repeats} of each task per arm ({} per arm), up to {} at a time,\n\
                 each in a fresh copy of its task's files",
                repeats * self.examples.len(),
                self.concurrency
            )
        };
        row(&mut out, "Runs", &runs);
        let check = match self.check {
            _ if !self.examples.is_empty() => {
                "each task's own command; hidden checks go in only after the run".to_string()
            }
            Some(check) => format!("`{check}` after each run"),
            None => "none · --check <cmd> or AURA_CHECK_COMMAND counts passes".to_string(),
        };
        row(&mut out, "Check", &check);
        let part = |usd: Option<f64>| usd.map_or_else(|| "unknown".to_string(), money);
        let mut estimate = format!(
            "~{} at list prices\nA ~{} · B ~{}\njudge ~{} on {}\n\
             for turns of about 150k input and 6k output tokens",
            money(self.estimate.known_usd()),
            part(self.estimate.a_usd),
            part(self.estimate.b_usd),
            part(self.estimate.judge_usd),
            route_label(self.judge),
        );
        if !self.estimate.fully_priced() {
            estimate.push_str("\nsome prices are unknown");
        }
        row(&mut out, "Estimate", &estimate);
        if let Some(budget) = self.budget_usd {
            let budget = format!(
                "{}: no new run starts once the finished ones cost that much",
                money(budget)
            );
            row(&mut out, "Budget", &budget);
        }
        for note in self.notes {
            row(&mut out, "Note", note);
        }
        out
    }
}

/// Characters an estimate row's value wraps at.
const VALUE_WIDTH: usize = 64;

/// Append the row `label` reading `value` to `out`: each line of `value`
/// wrapped at [`VALUE_WIDTH`], under the value column.
fn row(out: &mut String, label: &str, value: &str) {
    let mut label = label;
    for line in value.lines().flat_map(|line| wrap(line, VALUE_WIDTH)) {
        let _ = write!(out, "\n  {label:<14}{line}");
        label = "";
    }
}

/// `text` split at spaces into lines of at most `width` characters; a
/// word longer than that keeps a line of its own.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().expect("there is always a line");
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(word.to_string());
        }
    }
    lines
}

/// One line per past comparison, oldest first, numbered for
/// `/compare show <n>`.
pub(super) fn list(comparisons: &[(String, Option<ComparePayload>)]) -> String {
    if comparisons.is_empty() {
        return "No comparisons yet. /compare [prompt] runs your last prompt (or the one \
                given) with Gate and with Opus side by side."
            .to_string();
    }
    let mut out = String::from("Comparisons (/compare show <n> for one):");
    for (i, (name, payload)) in comparisons.iter().enumerate() {
        let Some(payload) = payload else {
            let _ = write!(out, "\n  {}. {name} · no record", i + 1);
            continue;
        };
        let a = summarize(payload, CompareArm::A);
        let b = summarize(payload, CompareArm::B);
        let _ = write!(
            out,
            "\n  {}. {} · {} · \"{}\" · {} vs {} · {} · A {} / B {}",
            i + 1,
            name,
            crate::record_loader::format_local_time(payload.started_ms),
            headline(&payload.task, 48),
            arm_label(&payload.arm_a),
            arm_label(&payload.arm_b),
            status(&payload.status),
            arm_line(&a),
            arm_line(&b),
        );
    }
    out
}

fn arm_line(arm: &ArmSummary) -> String {
    let checks = if arm.checks_run > 0 {
        format!(", {}/{} pass", arm.checks_passed, arm.checks_run)
    } else {
        String::new()
    };
    format!(
        "{} median{checks}",
        arm.median_cost_usd.map_or_else(|| "-".to_string(), money)
    )
}

fn status(status: &CompareStatus) -> String {
    match status {
        CompareStatus::Running => "running".to_string(),
        CompareStatus::Done => "done".to_string(),
        CompareStatus::Cancelled => "stopped".to_string(),
        CompareStatus::Applied(run) => format!("applied {run}"),
        CompareStatus::Dismissed => "dismissed".to_string(),
    }
}

/// `$0.05, 2 pass, 1 fail` for one arm's turns on one prompt.
fn turn_cell(cell: &TurnCell) -> String {
    if cell.runs == 0 {
        return "-".to_string();
    }
    let mut text = cell.mean_cost_usd.map_or_else(|| "-".to_string(), money);
    for (n, what) in [
        (cell.passed, "pass"),
        (cell.failed, "fail"),
        (cell.errors, "error"),
    ] {
        if n > 0 {
            let _ = write!(text, ", {n} {what}");
        }
    }
    text
}

/// The view of one comparison.
pub(crate) fn render(name: &str, payload: &ComparePayload) -> String {
    let a = summarize(payload, CompareArm::A);
    let b = summarize(payload, CompareArm::B);
    let (a_label, b_label) = (
        format!("A {}", payload.arm_a.label()),
        format!("B {}", payload.arm_b.label()),
    );
    let mut out = format!(
        "{name} · \"{}\" · {}",
        headline(&payload.task, 72),
        status(&payload.status)
    );
    if let BenchSource::Session { title, .. } = &payload.source {
        let _ = write!(
            out,
            "\n  Replays the session \"{}\": {} prompt{} in order",
            headline(title, 60),
            payload.prompts.len(),
            if payload.prompts.len() == 1 { "" } else { "s" },
        );
    }
    let examples = payload.source.examples();
    let _ = write!(
        out,
        "\n  A {} vs B {}, {} run{} {}{}",
        arm_label(&payload.arm_a),
        arm_label(&payload.arm_b),
        payload.repeats,
        if payload.repeats == 1 { "" } else { "s" },
        if examples.is_empty() {
            "each".to_string()
        } else {
            format!(
                "of each of {} example task{} per arm",
                examples.len(),
                if examples.len() == 1 { "" } else { "s" }
            )
        },
        match payload.check.as_deref() {
            _ if !examples.is_empty() => ", each task's own check".to_string(),
            Some(c) => format!(", check `{c}`"),
            None => ", no check command".to_string(),
        },
    );

    let width = a_label.chars().count().max(32);
    let row = |out: &mut String, label: &str, a: String, b: String| {
        let _ = write!(out, "\n  {label:<14}{a:<width$}  {b}");
    };
    let _ = write!(out, "\n\n  {:<14}{a_label:<width$}  {b_label}", "");
    let runs = |arm: &ArmSummary| format!("{}/{}", arm.finished, arm.runs);
    row(&mut out, "finished", runs(&a), runs(&b));
    let wall = |arm: &ArmSummary| arm.median_wall_ms.map_or_else(|| "-".to_string(), duration);
    row(&mut out, "median time", wall(&a), wall(&b));
    let [a_share, b_share] = cost_shares(Some(a.total_cost_usd), Some(b.total_cost_usd));
    row(
        &mut out,
        "total cost",
        money(a.total_cost_usd) + &a_share,
        money(b.total_cost_usd) + &b_share,
    );
    let median = |arm: &ArmSummary| arm.median_cost_usd.map_or_else(|| "-".to_string(), money);
    let [a_share, b_share] = cost_shares(a.median_cost_usd, b.median_cost_usd);
    row(
        &mut out,
        "median cost",
        median(&a) + &a_share,
        median(&b) + &b_share,
    );
    let checks = |arm: &ArmSummary| {
        if arm.checks_run == 0 {
            "-".to_string()
        } else {
            format!("{}/{} passed", arm.checks_passed, arm.checks_run)
        }
    };
    row(&mut out, "check", checks(&a), checks(&b));
    if a.checks_run + b.checks_run > 0 {
        let per_fix = |arm: &ArmSummary| cost_per_fix(arm.total_cost_usd, arm.checks_passed);
        let [x, y] = per_fix_cells(per_fix(&a), per_fix(&b), money);
        row(&mut out, "cost per fix", x, y);
    }
    let judged = |arm: &ArmSummary| {
        format!(
            "{} won, {} lost, {} tied",
            arm.judge_wins, arm.judge_losses, arm.judge_ties
        )
    };
    if !payload.judgments.is_empty() {
        row(&mut out, "judge", judged(&a), judged(&b));
    }
    let rated = |arm: &ArmSummary| arm.ref_better + arm.ref_as_good + arm.ref_worse;
    if rated(&a) + rated(&b) > 0 {
        let vs = |arm: &ArmSummary| {
            format!(
                "{} better, {} as good, {} worse",
                arm.ref_better, arm.ref_as_good, arm.ref_worse
            )
        };
        row(&mut out, "vs yours", vs(&a), vs(&b));
    }

    let runs_each = payload.repeats;
    if a.checks_run > 0 && b.checks_run > 0 && too_close(a.checks_passed, b.checks_passed) {
        let _ = write!(
            out,
            "\n  Too close to call with {runs_each} runs: check passes are within one of each other."
        );
    }
    if !payload.judgments.is_empty() && too_close(a.judge_wins, b.judge_wins) {
        let _ = write!(
            out,
            "\n  Too close to call with {runs_each} runs: judge wins are within one of each other."
        );
    }
    if let Some(baseline) = payload.source.baseline() {
        let _ = write!(
            out,
            "\n  The session itself: {} call{}, {}{}",
            baseline.calls,
            if baseline.calls == 1 { "" } else { "s" },
            if baseline.priced {
                money(baseline.cost_usd)
            } else {
                "no price".to_string()
            },
            if baseline.models.is_empty() {
                String::new()
            } else {
                format!(" on {}", baseline.models.join(", "))
            },
        );
    }

    if !examples.is_empty() {
        out.push_str("\n\n  Per task (checks passed; mean run cost)");
        let (a_cells, b_cells) = (
            task_cells(payload, CompareArm::A),
            task_cells(payload, CompareArm::B),
        );
        for (i, task) in examples.iter().enumerate() {
            let cell = |cells: &[TaskCell]| cells.get(i).map_or_else(|| "-".to_string(), task_cell);
            let cost = |cells: &[TaskCell]| cells.get(i).and_then(|c| c.mean_cost_usd);
            let [a_share, b_share] = cost_shares(cost(&a_cells), cost(&b_cells));
            let _ = write!(
                out,
                "\n    {:<34}{:<width$}  {}",
                headline(&task.id, 32),
                cell(&a_cells) + &a_share,
                cell(&b_cells) + &b_share,
            );
        }
    }

    if payload.prompts.len() > 1 {
        out.push_str(
            "\n\n  Per prompt (mean turn cost; test or build commands that passed or failed)",
        );
        let (a_cells, b_cells) = (
            turn_cells(payload, CompareArm::A),
            turn_cells(payload, CompareArm::B),
        );
        for (i, prompt) in payload.prompts.iter().enumerate() {
            let cell = |cells: &[TurnCell]| cells.get(i).map_or_else(|| "-".to_string(), turn_cell);
            let _ = write!(
                out,
                "\n    {:<10}{:<width$}  {}  \"{}\"",
                format!("{}.", i + 1),
                cell(&a_cells),
                cell(&b_cells),
                headline(prompt, 40),
            );
        }
    }

    out.push_str("\n\n  Tokens per model");
    for (arm, summary) in [("A", &a), ("B", &b)] {
        let _ = write!(out, "\n    {arm}: {}", model_tokens(&summary.tokens));
    }
    if a.plan_usage.is_empty() && b.plan_usage.is_empty() {
        let _ = write!(out, "\n  {PLAN_USAGE_NOTE}");
    } else {
        out.push_str("\n\n  Plan usage, share of each window's limit");
        for (arm, summary) in [("A", &a), ("B", &b)] {
            let _ = write!(out, "\n    {arm}: {}", plan_shares(&summary.plan_usage));
        }
        let _ = write!(out, "\n  {PLAN_SHARE_NOTE}");
    }
    let unparsed = payload.judgments.iter().filter(|j| !j.parsed).count();
    if let Some(judgment) = payload.judgments.first() {
        let _ = write!(
            out,
            "\n  Judge: {} blind call{} on {}, {} (not in the arm totals){}",
            payload.judgments.len(),
            if payload.judgments.len() == 1 {
                ""
            } else {
                "s"
            },
            route_label(&judgment.model),
            money(judge_cost_usd(payload)),
            if unparsed > 0 {
                format!("; {unparsed} gave no verdict")
            } else {
                String::new()
            },
        );
    }

    out.push_str("\n\n  Runs");
    for run in &payload.runs {
        out.push_str(&run_line(run));
    }
    for note in &payload.notes {
        let _ = write!(out, "\n  Note: {note}");
    }
    if payload.status.has_copies() && payload.status != CompareStatus::Running {
        out.push_str(if examples.is_empty() {
            "\n\n  /compare diff [run] · /compare apply <run> · /compare dismiss"
        } else {
            "\n\n  /compare diff [run] · /compare dismiss"
        });
    }
    out
}

/// `2/3 passed, $0.40` for one arm's runs of one example task.
fn task_cell(cell: &TaskCell) -> String {
    if cell.runs == 0 {
        return "-".to_string();
    }
    let cost = cell.mean_cost_usd.map_or_else(|| "-".to_string(), money);
    if cell.checked == 0 {
        return format!(
            "{} run{}, {cost}",
            cell.runs,
            if cell.runs == 1 { "" } else { "s" }
        );
    }
    format!("{}/{} passed, {cost}", cell.passed, cell.checked)
}

fn run_line(run: &CompareRun) -> String {
    let check = match run.check_passed {
        Some(true) => "check passed",
        Some(false) => "check failed",
        None => "",
    };
    let mut line = format!(
        "\n    {:<8} {:<34} {:>8}  {:>8}  {check:<12}  {} file{}",
        run.name,
        route_label(&run.model),
        duration(run.wall_ms),
        money(run.cost_usd),
        run.files_changed,
        if run.files_changed == 1 { "" } else { "s" },
    );
    if let Some(task) = &run.task {
        let _ = write!(line, "  {task}");
    }
    if let Some(routing) = routing(run) {
        let _ = write!(line, "  · {routing}");
    }
    if let Some(error) = &run.error {
        let _ = write!(line, "  ({})", headline(error, 80));
    }
    line
}

/// How a run's turns went: what they asked for and how hard, whether
/// they were led (and then what the lead and its subtasks cost), and how
/// often Gate asked the model to check its change and a failed attempt
/// moved up a tier, e.g. `debugging/hard, direct, moved up 1×`. `None`
/// when its turns do not say.
fn routing(run: &CompareRun) -> Option<String> {
    let routed: Vec<&RunTurn> = run.turns.iter().filter(|t| t.category.is_some()).collect();
    let mut asked: Vec<String> = Vec::new();
    for turn in &routed {
        let Some(category) = turn.category else {
            continue;
        };
        let what = turn
            .difficulty
            .map_or_else(|| category.to_string(), |d| format!("{category}/{d}"));
        if !asked.contains(&what) {
            asked.push(what);
        }
    }
    if asked.is_empty() {
        return None;
    }
    let mut text = asked.join(", ");
    match routed.iter().filter(|t| t.led == Some(true)).count() {
        0 => text.push_str(", direct"),
        led if led == routed.len() => text.push_str(", led"),
        led => {
            let _ = write!(text, ", {led} of {} turns led", routed.len());
        }
    }
    let subtasks: f64 = routed.iter().map(|t| t.subtask_cost_usd).sum();
    if subtasks > 0.0 {
        let total: f64 = routed.iter().map(|t| t.cost_usd).sum();
        let _ = write!(
            text,
            ": lead {} + subtasks {}",
            money((total - subtasks).max(0.0)),
            money(subtasks)
        );
    }
    let nudged: u32 = routed.iter().map(|t| t.nudges).sum();
    if nudged > 0 {
        let _ = write!(text, ", asked to check {nudged}×");
    }
    let sent_back: u32 = routed.iter().map(|t| t.sent_back).sum();
    if sent_back > 0 {
        let _ = write!(text, ", sent back {sent_back}×");
    }
    let moved: u32 = routed.iter().map(|t| t.escalations).sum();
    if moved > 0 {
        let _ = write!(text, ", moved up {moved}×");
    }
    Some(text)
}

fn model_tokens(rows: &[ModelTokens]) -> String {
    if rows.is_empty() {
        return "no model calls".to_string();
    }
    rows.iter()
        .map(|row| {
            format!(
                "{} {} call{}, {} in / {} out, {}",
                row.model,
                row.calls,
                if row.calls == 1 { "" } else { "s" },
                tokens(row.input_tokens),
                tokens(row.output_tokens),
                money(row.cost_usd),
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// `Claude plan 5h window +3.1%, 7d window +0.4%`, one plan after another.
fn plan_shares(rows: &[PlanShare]) -> String {
    if rows.is_empty() {
        return "no plan usage reported".to_string();
    }
    let mut plans: Vec<(Plan, Vec<String>)> = Vec::new();
    for row in rows {
        let window = format!(
            "{} window +{:.1}%",
            row.window.replace('_', " "),
            row.used * 100.0
        );
        match plans.iter_mut().find(|(plan, _)| *plan == row.plan) {
            Some((_, windows)) => windows.push(window),
            None => plans.push((row.plan, vec![window])),
        }
    }
    plans
        .into_iter()
        .map(|(plan, windows)| format!("{} {}", plan.label(), windows.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `patch` (a `git diff`) as one entry per file, each with its hunks.
pub(super) fn split_patch(patch: &str) -> Vec<FileDiffData> {
    let mut files: Vec<FileDiffData> = Vec::new();
    let mut in_hunks = false;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            let path = header
                .rsplit_once(" b/")
                .map_or(header, |(_, path)| path)
                .to_string();
            files.push(FileDiffData {
                path,
                diff: String::new(),
            });
            in_hunks = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            in_hunks = true;
        }
        if line.starts_with("Binary files") || line == "GIT binary patch" {
            file.diff = "(binary file)".to_string();
            in_hunks = false;
            continue;
        }
        if in_hunks {
            file.diff.push_str(line);
            file.diff.push('\n');
        }
    }
    for file in &mut files {
        if file.diff.is_empty() {
            file.diff = "(mode change or empty file)".to_string();
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{
        AgentId, CompareCopy, CompareJudgment, OutcomeSignal, RefVerdict, RunTurn, SessionBaseline,
        SystemKind,
    };
    use aura_ui_terminal::events::{BenchBaseline, BenchStatus, BenchView};
    use std::path::PathBuf;

    fn run(arm: CompareArm, n: u32, model: &str, cost: f64, check: bool) -> CompareRun {
        let id = u8::try_from(n).unwrap() + 0x10 * u8::from(arm == CompareArm::B);
        CompareRun {
            name: format!("{arm}-{n}"),
            arm,
            agent_id: AgentId::new([id; 32]),
            prompt_tx: None,
            model: model.to_string(),
            copy: CompareCopy {
                root: PathBuf::from("/c"),
                workdir: PathBuf::from("/c"),
                git_dir: None,
                baseline: "t".to_string(),
            },
            wall_ms: 40_000 + u64::from(n) * 1_000,
            cost_usd: cost,
            tokens: vec![ModelTokens {
                model: model.trim_start_matches("aura-").to_string(),
                calls: 3,
                input_tokens: 42_000,
                output_tokens: 1_500,
                cost_usd: cost,
                ..ModelTokens::default()
            }],
            plan_usage: Vec::new(),
            turns: Vec::new(),
            check_passed: Some(check),
            files_changed: 2,
            error: None,
            task: None,
        }
    }

    fn payload() -> ComparePayload {
        let sonnet = "aura-claude-sonnet-4-6";
        let opus = "aura-claude-opus-4-7";
        ComparePayload {
            system_kind: SystemKind::Compare,
            task: "fix the flaky parser test\nmore detail".to_string(),
            plan: None,
            arm_a: ArmSpec::Gate { delegate: true },
            arm_b: ArmSpec::Model {
                model: opus.to_string(),
            },
            against: opus.to_string(),
            prompts: vec!["fix the flaky parser test\nmore detail".to_string()],
            source: BenchSource::Prompt,
            repeats: 3,
            check: Some("cargo test".to_string()),
            budget_usd: None,
            requested_by: AgentId::generate(),
            apply_root: PathBuf::from("/repo"),
            started_ms: 0,
            runs: vec![
                run(CompareArm::A, 1, sonnet, 0.05, true),
                run(CompareArm::B, 1, opus, 0.40, true),
                run(CompareArm::A, 2, sonnet, 0.07, true),
                run(CompareArm::B, 2, opus, 0.50, true),
                run(CompareArm::A, 3, sonnet, 0.06, false),
                run(CompareArm::B, 3, opus, 0.45, true),
            ],
            judgments: vec![CompareJudgment {
                a_run: "a-1".to_string(),
                b_run: "b-1".to_string(),
                a_first: true,
                winner: Some(CompareArm::B),
                parsed: true,
                a_vs_reference: None,
                b_vs_reference: None,
                model: opus.to_string(),
                cost_usd: 0.03,
                reasoning: String::new(),
            }],
            notes: vec!["the budget stopped 2 runs".to_string()],
            status: CompareStatus::Done,
        }
    }

    fn line<'a>(text: &'a str, start: &str) -> Vec<&'a str> {
        text.lines()
            .find(|l| l.trim_start().starts_with(start))
            .unwrap_or_else(|| panic!("no {start:?} line in:\n{text}"))
            .split("  ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn the_view_shows_both_arms_side_by_side() {
        let text = render("Compare #1", &payload());
        assert!(text.starts_with("Compare #1 · \"fix the flaky parser test\" · done"));
        assert!(
            text.contains("A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each"),
            "{text}"
        );
        assert_eq!(line(&text, "median time"), ["median time", "42s", "42s"]);
        assert_eq!(
            line(&text, "total cost"),
            ["total cost", "$0.18 · 87% less", "$1.35 · 100%"]
        );
        assert_eq!(
            line(&text, "median cost"),
            ["median cost", "$0.06 · 87% less", "$0.45 · 100%"]
        );
        assert_eq!(line(&text, "check"), ["check", "2/3 passed", "3/3 passed"]);
        assert_eq!(
            line(&text, "cost per fix"),
            ["cost per fix", "$0.09 · 80% less", "$0.45 · 100%"]
        );
        assert_eq!(
            line(&text, "judge "),
            ["judge", "0 won, 1 lost, 0 tied", "1 won, 0 lost, 0 tied"]
        );
        assert!(
            text.contains("Too close to call with 3 runs: check passes"),
            "{text}"
        );
        assert!(
            text.contains("A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18"),
            "{text}"
        );
        assert!(text.contains("Plan usage: no call reported a plan's usage"));
        assert!(!text.contains("share of each window"));
        assert!(
            text.contains("Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03"),
            "{text}"
        );
        assert!(text.contains("Note: the budget stopped 2 runs"));
        assert!(text.contains("/compare diff [run] · "));
        assert!(!text.contains("Per prompt"), "{text}");
        assert!(!text.contains("vs yours"), "{text}");
        let a_3 = line(&text, "a-3");
        assert!(a_3.contains(&"check failed"), "{a_3:?}");
        assert!(
            !text.contains(" · direct"),
            "older runs say nothing of routing"
        );
    }

    #[test]
    fn each_run_says_how_its_turns_were_routed() {
        use aura_core_types::{Difficulty, PromptCategory};
        let mut payload = payload();
        payload.runs[0].turns = vec![RunTurn {
            category: Some(PromptCategory::Debugging),
            difficulty: Some(Difficulty::Hard),
            led: Some(false),
            escalations: 1,
            nudges: 1,
            ..RunTurn::new("m", 0.05, 1, OutcomeSignal::ChecksPassed)
        }];
        payload.runs[2].turns = vec![
            RunTurn {
                category: Some(PromptCategory::MultiFileChange),
                led: Some(true),
                subtask_cost_usd: 0.17,
                ..RunTurn::new("m", 0.77, 1, OutcomeSignal::ChecksPassed)
            },
            RunTurn {
                category: Some(PromptCategory::Question),
                led: Some(false),
                ..RunTurn::new("m", 0.03, 1, OutcomeSignal::JudgedPass)
            },
        ];
        let text = render("Compare #1", &payload);
        assert!(
            line(&text, "a-1")
                .contains(&"· debugging/hard, direct, asked to check 1×, moved up 1×"),
            "{text}"
        );
        assert!(
            line(&text, "a-2").contains(
                &"· multi_file_change, question, 1 of 2 turns led: lead $0.63 + subtasks $0.17"
            ),
            "{text}"
        );
    }

    fn share(plan: Plan, window: &str, used: f64) -> PlanShare {
        PlanShare {
            plan,
            window: window.to_string(),
            first: 0.1,
            last: 0.1 + used,
            used,
        }
    }

    #[test]
    fn plan_shares_per_arm_replace_the_note_when_a_plan_reported_usage() {
        let mut with_plan = payload();
        with_plan.runs[0].plan_usage = vec![
            share(Plan::Claude, "5h", 0.021),
            share(Plan::Claude, "7d_opus", 0.002),
        ];
        with_plan.runs[2].plan_usage = vec![share(Plan::Claude, "5h", 0.010)];
        with_plan.runs[1].plan_usage = vec![
            share(Plan::Claude, "5h", 0.090),
            share(Plan::Chatgpt, "5h", 0.012),
        ];
        let text = render("Compare #1", &with_plan);
        assert!(
            text.contains("A: Claude plan 5h window +3.1%, 7d opus window +0.2%"),
            "{text}"
        );
        assert!(
            text.contains("B: Claude plan 5h window +9.0%; ChatGPT plan 5h window +1.2%"),
            "{text}"
        );
        assert!(text.contains("don't count it twice"), "{text}");
        assert!(!text.contains("no call reported a plan's usage"), "{text}");

        let mut one_arm = payload();
        one_arm.runs[0].plan_usage = vec![share(Plan::Claude, "5h", 0.05)];
        let text = render("Compare #1", &one_arm);
        assert!(text.contains("B: no plan usage reported"), "{text}");
    }

    #[test]
    fn applied_comparisons_offer_nothing_more_and_list_compactly() {
        let mut applied = payload();
        applied.status = CompareStatus::Applied("a-2".to_string());
        let text = render("Compare #1", &applied);
        assert!(text.contains("applied a-2"));
        assert!(!text.contains("/compare apply"));
        let listed = list(&[
            ("Compare #1".to_string(), Some(applied)),
            ("Compare #2".to_string(), None),
        ]);
        let first = listed.lines().nth(1).unwrap();
        assert!(first.contains("1. Compare #1"), "{first}");
        assert!(
            first.contains("gate vs aura-claude-opus-4-7 (cloud)"),
            "{first}"
        );
        assert!(first.contains("A $0.06 median, 2/3 pass"), "{first}");
        assert!(listed.contains("2. Compare #2 · no record"));
        assert!(list(&[]).starts_with("No comparisons yet"));
    }

    #[test]
    fn the_estimate_names_the_arms_runs_and_budget() {
        let text = EstimateView {
            task: "fix it",
            prompts: 1,
            session: None,
            examples: &[],
            a: "gate (likely aura-claude-sonnet-4-6 (cloud))".to_string(),
            b: "aura-claude-opus-4-7 (cloud)".to_string(),
            judge: "aura-claude-opus-4-7",
            repeats: 3,
            check: None,
            budget_usd: Some(2.0),
            concurrency: 4,
            estimate: Estimate {
                a_usd: Some(1.5),
                b_usd: Some(4.0),
                judge_usd: None,
            },
            notes: &[],
        }
        .render();
        assert!(text.starts_with("Compare: \"fix it\"\n"), "{text}");
        for row in [
            "\n  A             gate (likely aura-claude-sonnet-4-6 (cloud))\n",
            "\n  B             aura-claude-opus-4-7 (cloud)\n",
            "\n  Runs          3 per arm, up to 4 at a time,\n                each in its own copy of the workspace\n",
            "\n  Check         none · --check <cmd> or AURA_CHECK_COMMAND counts passes\n",
            "\n  Estimate      ~$5.50 at list prices\n                A ~$1.50 · B ~$4.00\n                judge ~unknown on ",
            "\n                some prices are unknown\n",
            "\n  Budget        $2.00: no new run starts",
        ] {
            assert!(text.contains(row), "{row:?} in\n{text}");
        }
        assert!(
            text.lines()
                .all(|line| line.chars().count() <= 16 + VALUE_WIDTH),
            "{text}"
        );
    }

    #[test]
    fn long_values_wrap_at_spaces_under_the_value_column() {
        let mut out = String::from("Heading");
        let words = "word ".repeat(20);
        row(&mut out, "Label", words.trim_end());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        assert!(lines[1].starts_with("  Label         word word"));
        assert!(lines[2].starts_with("                word"));
        assert!(lines[1].chars().count() <= 16 + VALUE_WIDTH);
        assert_eq!(wrap("one-long-word", 4), ["one-long-word"]);
    }

    #[test]
    fn a_session_estimate_says_what_it_replays_with_its_notes() {
        let notes = vec!["shell changes are not undone".to_string()];
        let text = EstimateView {
            task: "fix it",
            prompts: 4,
            session: Some("fix the parser"),
            examples: &[],
            a: "claude-sonnet-4-6".to_string(),
            b: "claude-opus-4-7".to_string(),
            judge: "claude-opus-4-7",
            repeats: 2,
            check: Some("cargo test"),
            budget_usd: None,
            concurrency: 4,
            estimate: Estimate {
                a_usd: Some(1.0),
                b_usd: Some(2.0),
                judge_usd: Some(0.5),
            },
            notes: &notes,
        }
        .render();
        assert!(
            text.starts_with(
                "Benchmark: replay the session \"fix the parser\", 4 prompts in order"
            ),
            "{text}"
        );
        assert!(
            text.contains("\n  A             claude-sonnet-4-6\n"),
            "{text}"
        );
        assert!(text.contains("\n  Runs          2 per arm"), "{text}");
        assert!(
            text.contains("\n  Check         `cargo test` after each run\n"),
            "{text}"
        );
        assert!(
            text.contains("\n  Note          shell changes are not undone"),
            "{text}"
        );
    }

    #[test]
    fn a_replayed_session_shows_each_prompt_its_cost_and_the_reference() {
        let mut replay = payload();
        replay.prompts = vec!["fix the parser".to_string(), "now the docs".to_string()];
        replay.source = BenchSource::Session {
            agent_id: AgentId::generate(),
            started_ms: 1,
            title: "fix the parser".to_string(),
            baseline: SessionBaseline {
                calls: 12,
                cost_usd: 0.9,
                priced: true,
                models: vec!["claude-opus-4-7".to_string()],
            },
            reference: None,
        };
        let turn = |cost: f64, signal: OutcomeSignal| RunTurn::new("", cost, 1, signal);
        for run in &mut replay.runs {
            run.turns = vec![
                turn(0.02, OutcomeSignal::ChecksPassed),
                turn(0.04, OutcomeSignal::ChecksFailed),
            ];
        }
        replay.judgments[0].a_vs_reference = Some(RefVerdict::Worse);
        replay.judgments[0].b_vs_reference = Some(RefVerdict::Better);
        let text = render("Compare #1", &replay);
        assert!(
            text.contains("Replays the session \"fix the parser\": 2 prompts in order"),
            "{text}"
        );
        assert!(text.contains("The session itself: 12 calls, $0.90 on claude-opus-4-7"));
        assert_eq!(
            line(&text, "vs yours"),
            [
                "vs yours",
                "0 better, 0 as good, 1 worse",
                "1 better, 0 as good, 0 worse"
            ]
        );
        assert!(text.contains("Per prompt"), "{text}");
        let first = line(&text, "1.");
        assert!(first.contains(&"$0.02, 3 pass"), "{first:?}");
        let second = line(&text, "2.");
        assert!(second.contains(&"$0.04, 3 fail"), "{second:?}");
    }

    fn example(id: &str) -> ExampleTaskInfo {
        ExampleTaskInfo {
            id: id.to_string(),
            title: id.to_string(),
            prompts: vec![format!("do {id}")],
            check: "true".to_string(),
        }
    }

    #[test]
    fn example_tasks_show_per_task_passes_and_offer_no_apply() {
        let mut bench = payload();
        bench.task = "2 example coding tasks".to_string();
        bench.prompts = Vec::new();
        bench.check = None;
        bench.source = BenchSource::Examples {
            tasks: vec![
                example("coding/write-file"),
                example("coding/fix-shell-bug"),
            ],
        };
        for (i, run) in bench.runs.iter_mut().enumerate() {
            let id = if (i / 2) % 2 == 0 {
                "coding/write-file"
            } else {
                "coding/fix-shell-bug"
            };
            run.task = Some(id.to_string());
        }
        let text = render("Compare #1", &bench);
        assert!(
            text.contains("3 runs of each of 2 example tasks per arm, each task's own check"),
            "{text}"
        );
        assert!(text.contains("Per task"), "{text}");
        let write_file = line(&text, "coding/write-file");
        assert_eq!(write_file.len(), 3, "{write_file:?}");
        assert!(
            write_file[1].starts_with("1/2 passed, $0.0"),
            "{write_file:?}"
        );
        assert!(
            write_file[2].starts_with("2/2 passed, $0.4"),
            "{write_file:?}"
        );
        assert!(line(&text, "a-2").contains(&"coding/fix-shell-bug"));
        assert!(!text.contains("/compare apply"), "{text}");

        let tasks = [
            example("coding/write-file"),
            example("gate_hard/retry-spec"),
        ];
        let estimate = EstimateView {
            task: "2 example coding tasks",
            prompts: 1,
            session: None,
            examples: &tasks,
            a: "gate".to_string(),
            b: "opus".to_string(),
            judge: "opus",
            repeats: 2,
            check: None,
            budget_usd: None,
            concurrency: 4,
            estimate: Estimate {
                a_usd: Some(1.0),
                b_usd: Some(2.0),
                judge_usd: Some(0.5),
            },
            notes: &[],
        }
        .render();
        assert!(
            estimate.starts_with(
                "Benchmark: 2 example coding tasks\n  Tasks         coding/write-file, gate_hard/retry-spec\n"
            ),
            "{estimate}"
        );
        assert!(
            estimate.contains(
                "\n  Runs          2 of each task per arm (4 per arm), up to 4 at a time,\n"
            ),
            "{estimate}"
        );
        assert!(
            estimate.contains("\n  Check         each task's own command"),
            "{estimate}"
        );
        assert!(!estimate.contains("--check <cmd>"), "{estimate}");
    }

    #[test]
    fn patches_split_into_files_with_their_hunks() {
        let patch = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1..2 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\ndiff --git a/logo.png b/logo.png\nGIT binary patch\nliteral 3\nabc\n";
        let files = split_patch(patch);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/lib.rs", "new.txt", "logo.png"]);
        assert_eq!(files[0].diff, "@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(files[1].diff, "@@ -0,0 +1 @@\n+hi\n");
        assert_eq!(files[2].diff, "(binary file)");
        assert!(split_patch("").is_empty());
    }

    #[test]
    fn numbers_read_naturally() {
        assert_eq!(duration(850), "850ms");
        assert_eq!(duration(42_400), "42s");
        assert_eq!(duration(185_000), "3m 05s");
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(126_000), "126k");
        assert_eq!(tokens(1_400_000), "1.4M");
        assert_eq!(money(0.0), "free");
        assert_eq!(money(0.004), "$0.0040");
        assert_eq!(headline("  \nfirst line\nsecond", 80), "first line");
        assert_eq!(headline("abcdef", 4), "abc…");
    }

    #[test]
    fn the_view_is_pinned_byte_for_byte() {
        let expected = [
            "Compare #1 · \"fix the flaky parser test\" · done",
            "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each, check `cargo test`",
            "",
            "                A gate                            B aura-claude-opus-4-7",
            "  finished      3/3                               3/3",
            "  median time   42s                               42s",
            "  total cost    $0.18 · 87% less                  $1.35 · 100%",
            "  median cost   $0.06 · 87% less                  $0.45 · 100%",
            "  check         2/3 passed                        3/3 passed",
            "  cost per fix  $0.09 · 80% less                  $0.45 · 100%",
            "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
            "  Too close to call with 3 runs: check passes are within one of each other.",
            "  Too close to call with 3 runs: judge wins are within one of each other.",
            "",
            "  Tokens per model",
            "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
            "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
            "  Plan usage: no call reported a plan's usage (only Claude and ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.",
            "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
            "",
            "  Runs",
            "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files",
            "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files",
            "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files",
            "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50  check passed  2 files",
            "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files",
            "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files",
            "  Note: the budget stopped 2 runs",
            "",
            "  /compare diff [run] · /compare apply <run> · /compare dismiss",
        ];
        assert_eq!(render("Compare #1", &payload()), expected.join("\n"));
    }

    #[test]
    fn a_stopped_comparison_with_routed_runs_is_pinned_byte_for_byte() {
        let expected = [
            "Compare #2 · \"fix the flaky parser test\" · stopped",
            "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each, check `cargo test`",
            "",
            "                A gate                            B aura-claude-opus-4-7",
            "  finished      3/3                               2/3",
            "  median time   42s                               42s",
            "  total cost    $0.18 · 87% less                  $1.35 · 100%",
            "  median cost   $0.06 · 86% less                  $0.43 · 100%",
            "  check         2/3 passed                        2/2 passed",
            "  cost per fix  $0.09 · 87% less                  $0.68 · 100%",
            "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
            "  Too close to call with 3 runs: check passes are within one of each other.",
            "  Too close to call with 3 runs: judge wins are within one of each other.",
            "",
            "  Tokens per model",
            "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
            "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
            "  Plan usage: no call reported a plan's usage (only Claude and ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.",
            "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
            "",
            "  Runs",
            "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files  · debugging/hard, direct, asked to check 1×, moved up 1×",
            "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files",
            "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files  · multi_file_change, question, 1 of 2 turns led: lead $0.63 + subtasks $0.17, sent back 2×",
            "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50                2 files  (the model fell over)",
            "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files",
            "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files",
            "  Note: the budget stopped 2 runs",
            "",
            "  /compare diff [run] · /compare apply <run> · /compare dismiss",
        ];
        assert_eq!(render("Compare #2", &routed()), expected.join("\n"));
    }

    #[test]
    fn a_replayed_session_is_pinned_byte_for_byte() {
        let expected = [
            "Compare #3 · \"fix the flaky parser test\" · done",
            "  Replays the session \"fix the parser\": 2 prompts in order",
            "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each, check `cargo test`",
            "",
            "                A gate                            B aura-claude-opus-4-7",
            "  finished      3/3                               3/3",
            "  median time   42s                               42s",
            "  total cost    $0.18 · 87% less                  $1.35 · 100%",
            "  median cost   $0.06 · 87% less                  $0.45 · 100%",
            "  check         2/3 passed                        3/3 passed",
            "  cost per fix  $0.09 · 80% less                  $0.45 · 100%",
            "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
            "  vs yours      0 better, 0 as good, 1 worse      1 better, 0 as good, 0 worse",
            "  Too close to call with 3 runs: check passes are within one of each other.",
            "  Too close to call with 3 runs: judge wins are within one of each other.",
            "  The session itself: 12 calls, $0.90 on claude-opus-4-7",
            "",
            "  Per prompt (mean turn cost; test or build commands that passed or failed)",
            "    1.        $0.02, 3 pass                     $0.02, 3 pass  \"fix the parser\"",
            "    2.        $0.04, 3 error                    $0.04, 3 error  \"now the docs\"",
            "",
            "  Tokens per model",
            "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
            "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
            "",
            "  Plan usage, share of each window's limit",
            "    A: Claude plan 5h window +2.1%, 7d opus window +0.2%",
            "    B: ChatGPT plan 5h window +1.2%",
            "  Plan readings cover the whole account: each rise between readings goes to the run whose call saw it, so runs at the same time don't count it twice, and use outside the comparison while it ran lands on the next run to read.",
            "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
            "",
            "  Runs",
            "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files",
            "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files",
            "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files",
            "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50  check passed  2 files",
            "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files",
            "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files",
            "  Note: the budget stopped 2 runs",
            "",
            "  /compare diff [run] · /compare apply <run> · /compare dismiss",
        ];
        assert_eq!(render("Compare #3", &replay()), expected.join("\n"));
    }

    #[test]
    fn example_tasks_are_pinned_byte_for_byte() {
        let expected = [
            "Compare #4 · \"2 example coding tasks\" · done",
            "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs of each of 2 example tasks per arm, each task's own check",
            "",
            "                A gate                            B aura-claude-opus-4-7",
            "  finished      3/3                               3/3",
            "  median time   42s                               42s",
            "  total cost    $0.18 · 87% less                  $1.35 · 100%",
            "  median cost   $0.06 · 87% less                  $0.45 · 100%",
            "  check         2/3 passed                        3/3 passed",
            "  cost per fix  $0.09 · 80% less                  $0.45 · 100%",
            "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
            "  Too close to call with 3 runs: check passes are within one of each other.",
            "  Too close to call with 3 runs: judge wins are within one of each other.",
            "",
            "  Per task (checks passed; mean run cost)",
            "    coding/write-file                 1/2 passed, $0.06 · 87% less      2/2 passed, $0.43 · 100%",
            "    coding/fix-shell-bug              1/1 passed, $0.07 · 86% less      1/1 passed, $0.50 · 100%",
            "",
            "  Tokens per model",
            "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
            "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
            "  Plan usage: no call reported a plan's usage (only Claude and ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.",
            "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
            "",
            "  Runs",
            "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files  coding/write-file",
            "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files  coding/write-file",
            "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files  coding/fix-shell-bug",
            "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50  check passed  2 files  coding/fix-shell-bug",
            "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files  coding/write-file",
            "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files  coding/write-file",
            "  Note: the budget stopped 2 runs",
            "",
            "  /compare diff [run] · /compare dismiss",
        ];
        assert_eq!(render("Compare #4", &examples()), expected.join("\n"));
    }

    #[test]
    fn the_list_is_pinned_byte_for_byte() {
        let mut running = payload();
        running.status = CompareStatus::Running;
        let mut applied = payload();
        applied.status = CompareStatus::Applied("a-2".to_string());
        let listed = list(&[
            ("Compare #1".to_string(), Some(payload())),
            ("Compare #2".to_string(), Some(routed())),
            ("Compare #3".to_string(), Some(replay())),
            ("Compare #4".to_string(), Some(examples())),
            ("Compare #5".to_string(), None),
            ("Compare #6".to_string(), Some(running)),
            ("Compare #7".to_string(), Some(applied)),
        ]);
        let at = crate::record_loader::format_local_time(0);
        let expected = [
            "Comparisons (/compare show <n> for one):".to_string(),
            format!("  1. Compare #1 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · done · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
            format!("  2. Compare #2 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · stopped · A $0.06 median, 2/3 pass / B $0.43 median, 2/2 pass"),
            format!("  3. Compare #3 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · done · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
            format!("  4. Compare #4 · {at} · \"2 example coding tasks\" · gate vs aura-claude-opus-4-7 (cloud) · done · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
            "  5. Compare #5 · no record".to_string(),
            format!("  6. Compare #6 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · running · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
            format!("  7. Compare #7 · {at} · \"fix the flaky parser test\" · gate vs aura-claude-opus-4-7 (cloud) · applied a-2 · A $0.06 median, 2/3 pass / B $0.45 median, 3/3 pass"),
        ];
        assert_eq!(listed, expected.join("\n"));
    }

    /// Notes of a replayed session's estimate.
    fn notes() -> Vec<String> {
        vec![
            "shell changes are not undone".to_string(),
            "Learn mode: Gate arms explore as routed turns do (5% of turns), so the runs teach \
             Gate about models it would not pick yet"
                .to_string(),
        ]
    }

    /// Two example tasks of different suites.
    fn tasks() -> [ExampleTaskInfo; 2] {
        [
            example("coding/write-file"),
            example("gate_hard/retry-spec"),
        ]
    }

    /// The estimate of 2 runs per arm, up to 4 at a time, of Gate against
    /// a model per difficulty, whose price is unknown, judged by Opus.
    fn estimate_of(
        session: Option<&str>,
        examples: &[ExampleTaskInfo],
        check: Option<&str>,
        budget_usd: Option<f64>,
        notes: &[String],
    ) -> String {
        EstimateView {
            task: "fix the flaky parser test\nmore detail",
            prompts: 3,
            session,
            examples,
            a: "gate (likely aura-claude-sonnet-4-6 (cloud))".to_string(),
            b: "by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium → \
                aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7 (cloud)"
                .to_string(),
            judge: "aura-claude-opus-4-7",
            repeats: 2,
            check,
            budget_usd,
            concurrency: 4,
            estimate: Estimate {
                a_usd: Some(1.5),
                b_usd: None,
                judge_usd: Some(0.004),
            },
            notes,
        }
        .render()
    }

    #[test]
    fn the_estimate_of_a_prompt_is_pinned_byte_for_byte() {
        let expected = [
            "Compare: \"fix the flaky parser test\"",
            "  A             gate (likely aura-claude-sonnet-4-6 (cloud))",
            "  B             by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium →",
            "                aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7",
            "                (cloud)",
            "  Runs          2 per arm, up to 4 at a time,",
            "                each in its own copy of the workspace",
            "  Check         none · --check <cmd> or AURA_CHECK_COMMAND counts passes",
            "  Estimate      ~$1.50 at list prices",
            "                A ~$1.50 · B ~unknown",
            "                judge ~$0.0040 on aura-claude-opus-4-7 (cloud)",
            "                for turns of about 150k input and 6k output tokens",
            "                some prices are unknown",
            "  Budget        $2.00: no new run starts once the finished ones cost that much",
        ];
        assert_eq!(
            estimate_of(None, &[], None, Some(2.0), &[]),
            expected.join("\n")
        );
    }

    #[test]
    fn the_estimate_of_a_replayed_session_is_pinned_byte_for_byte() {
        let expected = [
            "Benchmark: replay the session \"fix the parser\", 3 prompts in order",
            "  A             gate (likely aura-claude-sonnet-4-6 (cloud))",
            "  B             by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium →",
            "                aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7",
            "                (cloud)",
            "  Runs          2 per arm, up to 4 at a time,",
            "                each in its own copy of the workspace",
            "  Check         `cargo test` after each run",
            "  Estimate      ~$1.50 at list prices",
            "                A ~$1.50 · B ~unknown",
            "                judge ~$0.0040 on aura-claude-opus-4-7 (cloud)",
            "                for turns of about 150k input and 6k output tokens",
            "                some prices are unknown",
            "  Note          shell changes are not undone",
            "  Note          Learn mode: Gate arms explore as routed turns do (5% of turns),",
            "                so the runs teach Gate about models it would not pick yet",
        ];
        assert_eq!(
            estimate_of(
                Some("fix the parser"),
                &[],
                Some("cargo test"),
                None,
                &notes()
            ),
            expected.join("\n")
        );
    }

    #[test]
    fn the_estimate_of_example_tasks_is_pinned_byte_for_byte() {
        let expected = [
            "Benchmark: 2 example coding tasks",
            "  Tasks         coding/write-file, gate_hard/retry-spec",
            "  A             gate (likely aura-claude-sonnet-4-6 (cloud))",
            "  B             by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium →",
            "                aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7",
            "                (cloud)",
            "  Runs          2 of each task per arm (4 per arm), up to 4 at a time,",
            "                each in a fresh copy of its task's files",
            "  Check         each task's own command; hidden checks go in only after the run",
            "  Estimate      ~$1.50 at list prices",
            "                A ~$1.50 · B ~unknown",
            "                judge ~$0.0040 on aura-claude-opus-4-7 (cloud)",
            "                for turns of about 150k input and 6k output tokens",
            "                some prices are unknown",
            "  Note          Learn mode: Gate arms explore as routed turns do (5% of turns),",
            "                so the runs teach Gate about models it would not pick yet",
        ];
        assert_eq!(
            estimate_of(None, &tasks(), Some("cargo test"), None, &notes()[1..]),
            expected.join("\n")
        );
    }

    /// The BENCH tab's view of `payload`, held by an agent of its own in
    /// another agent's tree.
    fn bench_view(payload: &ComparePayload) -> BenchView {
        let (host, root) = (AgentId::new([0xc0; 32]), AgentId::new([0x0f; 32]));
        crate::event_loop::bench::view(host, root, "Compare #1", payload)
    }

    /// Each of `items` as `Debug` writes it on one line.
    fn debug<T: std::fmt::Debug>(items: &[T]) -> Vec<String> {
        items.iter().map(|item| format!("{item:?}")).collect()
    }

    /// `view` without its arms, runs, tasks and judge.
    fn rest(view: BenchView) -> BenchView {
        BenchView {
            arms: Default::default(),
            runs: Vec::new(),
            tasks: Vec::new(),
            judge: None,
            ..view
        }
    }

    #[test]
    fn the_bench_tab_lists_each_run_as_it_ran() {
        let expected = [
            "BenchRun { name: \"a-1\", arm: 0, model: \"aura-claude-haiku-4-5\", \
             escalated_to: Some(\"aura-claude-sonnet-4-6\"), escalated_effort: Some(\"high\"), \
             category: Some(\"debugging\"), difficulty: Some(\"hard\"), led: false, \
             subtask_cost_usd: 0.0, wall_ms: 41000, cost_usd: 0.05, check_passed: Some(true), \
             files_changed: 2, error: None, task: None, \
             agent_id: \"0101010101010101010101010101010101010101010101010101010101010101\" }",
            "BenchRun { name: \"b-1\", arm: 1, model: \"aura-claude-opus-4-7\", escalated_to: None, \
             escalated_effort: None, category: None, difficulty: None, led: false, \
             subtask_cost_usd: -0.0, wall_ms: 41000, cost_usd: 0.4, check_passed: Some(true), \
             files_changed: 2, error: None, task: None, \
             agent_id: \"1111111111111111111111111111111111111111111111111111111111111111\" }",
            "BenchRun { name: \"a-2\", arm: 0, model: \"m\", escalated_to: None, \
             escalated_effort: None, category: Some(\"multi_file_change\"), difficulty: None, \
             led: true, subtask_cost_usd: 0.17, wall_ms: 42000, cost_usd: 0.07, \
             check_passed: Some(true), files_changed: 2, error: None, task: None, \
             agent_id: \"0202020202020202020202020202020202020202020202020202020202020202\" }",
            "BenchRun { name: \"b-2\", arm: 1, model: \"aura-claude-opus-4-7\", escalated_to: None, \
             escalated_effort: None, category: None, difficulty: None, led: false, \
             subtask_cost_usd: -0.0, wall_ms: 42000, cost_usd: 0.5, check_passed: None, \
             files_changed: 2, error: Some(\"the model fell over\\nand stayed down\"), task: None, \
             agent_id: \"1212121212121212121212121212121212121212121212121212121212121212\" }",
            "BenchRun { name: \"a-3\", arm: 0, model: \"aura-claude-sonnet-4-6\", \
             escalated_to: None, escalated_effort: None, category: None, difficulty: None, \
             led: false, subtask_cost_usd: -0.0, wall_ms: 43000, cost_usd: 0.06, \
             check_passed: Some(false), files_changed: 2, error: None, task: None, \
             agent_id: \"0303030303030303030303030303030303030303030303030303030303030303\" }",
            "BenchRun { name: \"b-3\", arm: 1, model: \"aura-claude-opus-4-7\", escalated_to: None, \
             escalated_effort: None, category: None, difficulty: None, led: false, \
             subtask_cost_usd: -0.0, wall_ms: 43000, cost_usd: 0.45, check_passed: Some(true), \
             files_changed: 2, error: None, task: None, \
             agent_id: \"1313131313131313131313131313131313131313131313131313131313131313\" }",
        ];
        assert_eq!(debug(&bench_view(&routed()).runs), expected);
    }

    #[test]
    fn the_bench_tab_sums_up_each_arm() {
        let routed_arms = [
            "BenchArm { label: \"gate\", runs: 3, finished: 3, total_cost_usd: 0.18, \
             median_cost_usd: Some(0.06), median_wall_ms: Some(42000), checks_passed: 2, \
             checks_run: 3, judge_wins: 0, judge_losses: 1, judge_ties: 0, ref_better: 0, \
             ref_as_good: 0, ref_worse: 0, tokens: [BenchTokens { model: \"claude-sonnet-4-6\", \
             calls: 9, input_tokens: 126000, output_tokens: 4500, cost_usd: 0.18 }], \
             turns: [BenchTurn { runs: 2, mean_cost_usd: Some(0.41000000000000003), passed: 2, \
             failed: 0, errors: 0 }], escalated: 1, nudged: 1 }",
            "BenchArm { label: \"aura-claude-opus-4-7\", runs: 3, finished: 2, \
             total_cost_usd: 1.35, median_cost_usd: Some(0.42500000000000004), \
             median_wall_ms: Some(42000), checks_passed: 2, checks_run: 2, judge_wins: 1, \
             judge_losses: 0, judge_ties: 0, ref_better: 0, ref_as_good: 0, ref_worse: 0, \
             tokens: [BenchTokens { model: \"claude-opus-4-7\", calls: 9, input_tokens: 126000, \
             output_tokens: 4500, cost_usd: 1.35 }], turns: [BenchTurn { runs: 0, \
             mean_cost_usd: None, passed: 0, failed: 0, errors: 0 }], escalated: 0, nudged: 0 }",
        ];
        assert_eq!(debug(&bench_view(&routed()).arms), routed_arms);
        let replayed_arms = [
            "BenchArm { label: \"gate\", runs: 3, finished: 3, total_cost_usd: 0.18, \
             median_cost_usd: Some(0.06), median_wall_ms: Some(42000), checks_passed: 2, \
             checks_run: 3, judge_wins: 0, judge_losses: 1, judge_ties: 0, ref_better: 0, \
             ref_as_good: 0, ref_worse: 1, tokens: [BenchTokens { model: \"claude-sonnet-4-6\", \
             calls: 9, input_tokens: 126000, output_tokens: 4500, cost_usd: 0.18 }], \
             turns: [BenchTurn { runs: 3, mean_cost_usd: Some(0.02), passed: 3, failed: 0, \
             errors: 0 }, BenchTurn { runs: 3, mean_cost_usd: Some(0.04), passed: 0, failed: 0, \
             errors: 3 }], escalated: 0, nudged: 0 }",
            "BenchArm { label: \"aura-claude-opus-4-7\", runs: 3, finished: 3, \
             total_cost_usd: 1.35, median_cost_usd: Some(0.45), median_wall_ms: Some(42000), \
             checks_passed: 3, checks_run: 3, judge_wins: 1, judge_losses: 0, judge_ties: 0, \
             ref_better: 1, ref_as_good: 0, ref_worse: 0, \
             tokens: [BenchTokens { model: \"claude-opus-4-7\", calls: 9, input_tokens: 126000, \
             output_tokens: 4500, cost_usd: 1.35 }], turns: [BenchTurn { runs: 3, \
             mean_cost_usd: Some(0.02), passed: 3, failed: 0, errors: 0 }, BenchTurn { runs: 3, \
             mean_cost_usd: Some(0.04), passed: 0, failed: 0, errors: 3 }], escalated: 0, \
             nudged: 0 }",
        ];
        assert_eq!(debug(&bench_view(&replay()).arms), replayed_arms);
    }

    #[test]
    fn the_bench_tab_names_a_comparison_and_its_judge() {
        let view = bench_view(&routed());
        let judge = [
            "Some(BenchJudge { model: \"aura-claude-opus-4-7\", cost_usd: 0.03, \
             verdicts: [BenchVerdict { a_run: \"a-1\", b_run: \"b-1\", parsed: true, \
             winner: Some(1), a_vs_reference: None, b_vs_reference: None, reasoning: \"\" }] })",
        ];
        assert_eq!(debug(&[&view.judge]), judge);
        let expected = BenchView {
            id: "c0".repeat(32),
            root: "0f".repeat(32),
            name: "Compare #1".to_string(),
            title: "fix the flaky parser test".to_string(),
            source: "prompt".to_string(),
            status: BenchStatus::Cancelled,
            repeats: 3,
            check: Some("cargo test".to_string()),
            prompts: vec!["fix the flaky parser test\nmore detail".to_string()],
            notes: vec!["the budget stopped 2 runs".to_string()],
            ..BenchView::default()
        };
        assert_eq!(rest(view), expected);
    }

    #[test]
    fn the_bench_tab_shows_a_replayed_session_with_what_it_cost() {
        let view = bench_view(&replay());
        let judge = [
            "Some(BenchJudge { model: \"aura-claude-opus-4-7\", cost_usd: 0.03, \
             verdicts: [BenchVerdict { a_run: \"a-1\", b_run: \"b-1\", parsed: true, \
             winner: Some(1), a_vs_reference: Some(\"worse\"), b_vs_reference: Some(\"better\"), \
             reasoning: \"\" }] })",
        ];
        assert_eq!(debug(&[&view.judge]), judge);
        let expected = BenchView {
            id: "c0".repeat(32),
            root: "0f".repeat(32),
            name: "Compare #1".to_string(),
            title: "fix the parser".to_string(),
            source: "session \"fix the parser\"".to_string(),
            status: BenchStatus::Done,
            repeats: 3,
            check: Some("cargo test".to_string()),
            prompts: vec!["fix the parser".to_string(), "now the docs".to_string()],
            baseline: Some(BenchBaseline {
                calls: 12,
                cost_usd: 0.9,
                priced: true,
                models: vec!["claude-opus-4-7".to_string()],
            }),
            has_reference: true,
            notes: vec!["the budget stopped 2 runs".to_string()],
            ..BenchView::default()
        };
        assert_eq!(rest(view), expected);
    }

    #[test]
    fn the_bench_tab_has_a_row_per_example_task() {
        let view = bench_view(&examples());
        let tasks = [
            "BenchTaskRow { id: \"coding/write-file\", title: \"coding/write-file\", \
             arms: [BenchTaskCell { runs: 2, passed: 1, checked: 2, \
             mean_cost_usd: Some(0.055) }, BenchTaskCell { runs: 2, passed: 2, checked: 2, \
             mean_cost_usd: Some(0.42500000000000004) }], setup: Ready }",
            "BenchTaskRow { id: \"coding/fix-shell-bug\", title: \"coding/fix-shell-bug\", \
             arms: [BenchTaskCell { runs: 1, passed: 1, checked: 1, mean_cost_usd: Some(0.07) }, \
             BenchTaskCell { runs: 1, passed: 1, checked: 1, mean_cost_usd: Some(0.5) }], \
             setup: Ready }",
        ];
        assert_eq!(debug(&view.tasks), tasks);
        let worked_on: Vec<(&str, Option<&str>)> = (view.runs.iter())
            .map(|run| (run.name.as_str(), run.task.as_deref()))
            .collect();
        let (write, fix) = (Some("coding/write-file"), Some("coding/fix-shell-bug"));
        let order = [("a-1", write), ("b-1", write), ("a-2", fix), ("b-2", fix)];
        assert_eq!(
            worked_on,
            [
                order[0],
                order[1],
                order[2],
                order[3],
                ("a-3", write),
                ("b-3", write)
            ]
        );
        let expected = BenchView {
            id: "c0".repeat(32),
            root: "0f".repeat(32),
            name: "Compare #1".to_string(),
            title: "2 example coding tasks".to_string(),
            source: "2 example tasks".to_string(),
            status: BenchStatus::Done,
            repeats: 3,
            notes: vec!["the budget stopped 2 runs".to_string()],
            ..BenchView::default()
        };
        assert_eq!(rest(view), expected);
    }

    #[test]
    fn the_bench_tab_names_a_plans_work_and_each_status() {
        let mut planned = payload();
        planned.plan = Some("plans/parser.md".to_string());
        planned.task = "Build the plan\nin plans/parser.md".to_string();
        planned.budget_usd = Some(2.5);
        let view = rest(bench_view(&planned));
        assert_eq!(
            (view.title.as_str(), view.source.as_str(), view.budget_usd),
            ("Build the plan", "plan plans/parser.md", Some(2.5))
        );
        let applied = || CompareStatus::Applied("a-2".to_string());
        for (status, shown) in [
            (CompareStatus::Running, BenchStatus::Running),
            (CompareStatus::Done, BenchStatus::Done),
            (CompareStatus::Cancelled, BenchStatus::Cancelled),
            (applied(), BenchStatus::Applied("a-2".to_string())),
            (CompareStatus::Dismissed, BenchStatus::Dismissed),
        ] {
            let payload = ComparePayload {
                status,
                ..payload()
            };
            assert_eq!(bench_view(&payload).status, shown);
        }
    }

    /// [`payload`] whose runs say how their turns were routed, as a
    /// comparison the user stopped with a run that stopped too.
    fn routed() -> ComparePayload {
        use aura_core_types::{Difficulty, PromptCategory};
        let mut payload = payload();
        payload.runs[0].turns = vec![RunTurn {
            category: Some(PromptCategory::Debugging),
            difficulty: Some(Difficulty::Hard),
            led: Some(false),
            escalations: 1,
            nudges: 1,
            first_model: Some("aura-claude-haiku-4-5".to_string()),
            effort: Some("high".to_string()),
            first_effort: Some("medium".to_string()),
            ..RunTurn::new(
                "aura-claude-sonnet-4-6",
                0.05,
                1,
                OutcomeSignal::ChecksPassed,
            )
        }];
        payload.runs[2].turns = vec![
            RunTurn {
                category: Some(PromptCategory::MultiFileChange),
                led: Some(true),
                subtask_cost_usd: 0.17,
                sent_back: 2,
                ..RunTurn::new("m", 0.77, 1, OutcomeSignal::ChecksPassed)
            },
            RunTurn {
                category: Some(PromptCategory::Question),
                led: Some(false),
                ..RunTurn::new("m", 0.03, 1, OutcomeSignal::JudgedPass)
            },
        ];
        payload.runs[3].error = Some("the model fell over\nand stayed down".to_string());
        payload.runs[3].check_passed = None;
        payload.status = CompareStatus::Cancelled;
        payload
    }

    /// [`payload`] as a replayed session of two prompts, judged against
    /// what the user kept, whose runs reported plan usage.
    fn replay() -> ComparePayload {
        let mut replay = payload();
        replay.prompts = vec!["fix the parser".to_string(), "now the docs".to_string()];
        replay.source = BenchSource::Session {
            agent_id: AgentId::generate(),
            started_ms: 1,
            title: "fix the parser".to_string(),
            baseline: SessionBaseline {
                calls: 12,
                cost_usd: 0.9,
                priced: true,
                models: vec!["claude-opus-4-7".to_string()],
            },
            reference: Some(aura_core_types::Reference {
                diff: "--- a/x\n+++ b/x\n".to_string(),
                reply: "done".to_string(),
            }),
        };
        let turn = |cost: f64, signal: OutcomeSignal| RunTurn::new("", cost, 1, signal);
        for run in &mut replay.runs {
            run.turns = vec![
                turn(0.02, OutcomeSignal::ChecksPassed),
                turn(0.04, OutcomeSignal::Error),
            ];
        }
        replay.judgments[0].a_vs_reference = Some(RefVerdict::Worse);
        replay.judgments[0].b_vs_reference = Some(RefVerdict::Better);
        replay.runs[0].plan_usage = vec![
            share(Plan::Claude, "5h", 0.021),
            share(Plan::Claude, "7d_opus", 0.002),
        ];
        replay.runs[1].plan_usage = vec![share(Plan::Chatgpt, "5h", 0.012)];
        replay
    }

    /// [`payload`] as two example tasks, the runs taking turns at them.
    fn examples() -> ComparePayload {
        let mut bench = payload();
        bench.task = "2 example coding tasks".to_string();
        bench.prompts = Vec::new();
        bench.check = None;
        bench.source = BenchSource::Examples {
            tasks: vec![
                example("coding/write-file"),
                example("coding/fix-shell-bug"),
            ],
        };
        for (i, run) in bench.runs.iter_mut().enumerate() {
            let id = if (i / 2) % 2 == 0 {
                "coding/write-file"
            } else {
                "coding/fix-shell-bug"
            };
            run.task = Some(id.to_string());
        }
        bench
    }
}
