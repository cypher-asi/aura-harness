//! What `/compare` shows: the estimate before a comparison starts, the
//! per-arm view once it ends, the list of past comparisons, and run
//! diffs split per file for the diff view.

use aura_agent_gate::compare::{
    judge_cost_usd, summarize, too_close, ArmSummary, Estimate, NO_DELEGATE,
};
use aura_core_types::{
    CompareArm, ComparePayload, CompareRun, CompareStatus, ModelTokens, Plan, PlanShare,
};
use aura_model_reasoner::model_catalog::route_label;
use aura_terminal::events::FileDiffData;
use std::fmt::Write;

/// Why plan usage is not shown when no call reported it.
const PLAN_USAGE_NOTE: &str = "Plan usage: no call reported a plan's usage (only Claude and \
ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.";

/// How the plan shares are worked out, shown with them.
const PLAN_SHARE_NOTE: &str = "Plan readings cover the whole account: each rise between \
readings goes to the run whose call saw it, so runs at the same time don't count it twice, \
and use outside the comparison while it ran lands on the next run to read.";

/// `$0.004`, `$1.25`, or `free` for nothing.
pub(super) fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

/// `850ms`, `42s`, `3m 05s`.
pub(super) fn duration(ms: u64) -> String {
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
fn tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", (n as f64 / 1_000.0).round())
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// The first line of `text`, cut to `max` characters.
pub(super) fn headline(text: &str, max: usize) -> String {
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

/// How the other arm, `against` as the comparison records it, reads:
/// its model and route, or Gate with delegation off.
fn arm_label(against: &str) -> String {
    if against == NO_DELEGATE {
        format!("gate without enforced delegation ({NO_DELEGATE})")
    } else {
        route_label(against)
    }
}

/// What the user is asked to confirm before a comparison starts.
pub(super) struct EstimateView<'a> {
    pub task: &'a str,
    /// Model Gate is expected to pick for the task
    pub gate_model: Option<&'a str>,
    pub against: &'a str,
    pub judge: &'a str,
    pub repeats: u32,
    pub check: Option<&'a str>,
    pub budget_usd: Option<f64>,
    pub concurrency: usize,
    pub estimate: Estimate,
}

impl EstimateView<'_> {
    pub(super) fn render(&self) -> String {
        let mut out = format!("Compare: \"{}\"", headline(self.task, 80));
        let gate = self.gate_model.map_or_else(
            || "gate".to_string(),
            |m| format!("gate (likely {})", route_label(m)),
        );
        let _ = write!(
            out,
            "\n  {gate} vs {}, {} run{} each, up to {} at a time, each in its own copy of the workspace",
            arm_label(self.against),
            self.repeats,
            if self.repeats == 1 { "" } else { "s" },
            self.concurrency,
        );
        match self.check {
            Some(check) => {
                let _ = write!(out, "\n  Check after each run: `{check}`");
            }
            None => out.push_str(
                "\n  No check command: pass --check <cmd> (or set AURA_CHECK_COMMAND) to count passes",
            ),
        }
        let part = |usd: Option<f64>| usd.map_or_else(|| "unknown".to_string(), money);
        let _ = write!(
            out,
            "\n  Estimated at list prices: ~{} (gate ~{}, other ~{}, judge ~{} on {}), for runs of about 150k input and 6k output tokens{}",
            money(self.estimate.known_usd()),
            part(self.estimate.gate_usd),
            part(self.estimate.other_usd),
            part(self.estimate.judge_usd),
            route_label(self.judge),
            if self.estimate.fully_priced() {
                ""
            } else {
                "; some prices are unknown"
            },
        );
        if let Some(budget) = self.budget_usd {
            let _ = write!(
                out,
                "\n  Budget {}: no new run starts once the finished ones cost that much",
                money(budget)
            );
        }
        out.push_str("\n  /compare go starts it · /compare cancel drops it");
        out
    }
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
        let gate = summarize(payload, CompareArm::Gate);
        let other = summarize(payload, CompareArm::Other);
        let _ = write!(
            out,
            "\n  {}. {} · {} · \"{}\" · gate vs {} · {} · gate {} / other {}",
            i + 1,
            name,
            crate::record_loader::format_local_time(payload.started_ms),
            headline(&payload.task, 48),
            arm_label(&payload.against),
            status(&payload.status),
            arm_line(&gate),
            arm_line(&other),
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
        CompareStatus::Picked(arm) => format!("you picked {arm}"),
        CompareStatus::Applied(run) => format!("applied {run}"),
        CompareStatus::Dismissed => "dismissed".to_string(),
    }
}

/// The view of one comparison.
pub(super) fn render(name: &str, payload: &ComparePayload) -> String {
    let gate = summarize(payload, CompareArm::Gate);
    let other = summarize(payload, CompareArm::Other);
    let against = arm_label(&payload.against);
    let mut out = format!(
        "{name} · \"{}\" · {}",
        headline(&payload.task, 72),
        status(&payload.status)
    );
    let _ = write!(
        out,
        "\n  gate vs {against}, {} run{} each{}",
        payload.repeats,
        if payload.repeats == 1 { "" } else { "s" },
        payload.check.as_deref().map_or_else(
            || ", no check command".to_string(),
            |c| format!(", check `{c}`")
        ),
    );

    let width = against.chars().count().max(16);
    let row = |out: &mut String, label: &str, g: String, o: String| {
        let _ = write!(out, "\n  {label:<14}{g:<width$}  {o}");
    };
    let _ = write!(out, "\n\n  {:<14}{:<width$}  {against}", "", "gate");
    let runs = |arm: &ArmSummary| format!("{}/{}", arm.finished, arm.runs);
    row(&mut out, "finished", runs(&gate), runs(&other));
    let wall = |arm: &ArmSummary| arm.median_wall_ms.map_or_else(|| "-".to_string(), duration);
    row(&mut out, "median time", wall(&gate), wall(&other));
    row(
        &mut out,
        "total cost",
        money(gate.total_cost_usd),
        money(other.total_cost_usd),
    );
    let median = |arm: &ArmSummary| arm.median_cost_usd.map_or_else(|| "-".to_string(), money);
    row(&mut out, "median cost", median(&gate), median(&other));
    let checks = |arm: &ArmSummary| {
        if arm.checks_run == 0 {
            "-".to_string()
        } else {
            format!("{}/{} passed", arm.checks_passed, arm.checks_run)
        }
    };
    row(&mut out, "check", checks(&gate), checks(&other));
    let judged = |arm: &ArmSummary| {
        format!(
            "{} won, {} lost, {} tied",
            arm.judge_wins, arm.judge_losses, arm.judge_ties
        )
    };
    if !payload.judgments.is_empty() {
        row(&mut out, "judge", judged(&gate), judged(&other));
    }

    let runs_each = payload.repeats;
    if gate.checks_run > 0
        && other.checks_run > 0
        && too_close(gate.checks_passed, other.checks_passed)
    {
        let _ = write!(
            out,
            "\n  Too close to call with {runs_each} runs: check passes are within one of each other."
        );
    }
    if !payload.judgments.is_empty() && too_close(gate.judge_wins, other.judge_wins) {
        let _ = write!(
            out,
            "\n  Too close to call with {runs_each} runs: judge wins are within one of each other."
        );
    }

    out.push_str("\n\n  Tokens per model");
    for (arm, summary) in [("gate", &gate), ("other", &other)] {
        let _ = write!(out, "\n    {arm}: {}", model_tokens(&summary.tokens));
    }
    if gate.plan_usage.is_empty() && other.plan_usage.is_empty() {
        let _ = write!(out, "\n  {PLAN_USAGE_NOTE}");
    } else {
        out.push_str("\n\n  Plan usage, share of each window's limit");
        for (arm, summary) in [("gate", &gate), ("other", &other)] {
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
        out.push_str(
            "\n\n  /compare diff [run] · /compare pick gate|other · /compare apply <run> · /compare dismiss",
        );
    }
    out
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
    if let Some(error) = &run.error {
        let _ = write!(line, "  ({})", headline(error, 80));
    }
    line
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
    use aura_core_types::{AgentId, CompareCopy, CompareJudgment, SystemKind};
    use std::path::PathBuf;

    fn run(arm: CompareArm, n: u32, model: &str, cost: f64, check: bool) -> CompareRun {
        CompareRun {
            name: format!("{arm}-{n}"),
            arm,
            agent_id: AgentId::generate(),
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
            check_passed: Some(check),
            files_changed: 2,
            error: None,
        }
    }

    fn payload() -> ComparePayload {
        let sonnet = "aura-claude-sonnet-4-6";
        let opus = "aura-claude-opus-4-7";
        ComparePayload {
            system_kind: SystemKind::Compare,
            task: "fix the flaky parser test\nmore detail".to_string(),
            plan: None,
            against: opus.to_string(),
            repeats: 3,
            check: Some("cargo test".to_string()),
            budget_usd: None,
            requested_by: AgentId::generate(),
            apply_root: PathBuf::from("/repo"),
            started_ms: 0,
            runs: vec![
                run(CompareArm::Gate, 1, sonnet, 0.05, true),
                run(CompareArm::Other, 1, opus, 0.40, true),
                run(CompareArm::Gate, 2, sonnet, 0.07, true),
                run(CompareArm::Other, 2, opus, 0.50, true),
                run(CompareArm::Gate, 3, sonnet, 0.06, false),
                run(CompareArm::Other, 3, opus, 0.45, true),
            ],
            judgments: vec![CompareJudgment {
                gate_run: "gate-1".to_string(),
                other_run: "other-1".to_string(),
                gate_first: true,
                winner: Some(CompareArm::Other),
                parsed: true,
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
        assert_eq!(line(&text, "median time"), ["median time", "42s", "42s"]);
        assert_eq!(line(&text, "total cost"), ["total cost", "$0.18", "$1.35"]);
        assert_eq!(
            line(&text, "median cost"),
            ["median cost", "$0.06", "$0.45"]
        );
        assert_eq!(line(&text, "check"), ["check", "2/3 passed", "3/3 passed"]);
        assert_eq!(
            line(&text, "judge "),
            ["judge", "0 won, 1 lost, 0 tied", "1 won, 0 lost, 0 tied"]
        );
        assert!(
            text.contains("Too close to call with 3 runs: check passes"),
            "{text}"
        );
        assert!(
            text.contains("gate: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18"),
            "{text}"
        );
        assert!(text.contains("Plan usage: no call reported a plan's usage"));
        assert!(!text.contains("share of each window"));
        assert!(
            text.contains("Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03"),
            "{text}"
        );
        assert!(text.contains("Note: the budget stopped 2 runs"));
        assert!(text.contains("/compare pick gate|other"));
        let gate_3 = line(&text, "gate-3");
        assert!(gate_3.contains(&"check failed"), "{gate_3:?}");
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
            text.contains("gate: Claude plan 5h window +3.1%, 7d opus window +0.2%"),
            "{text}"
        );
        assert!(
            text.contains("other: Claude plan 5h window +9.0%; ChatGPT plan 5h window +1.2%"),
            "{text}"
        );
        assert!(text.contains("don't count it twice"), "{text}");
        assert!(!text.contains("no call reported a plan's usage"), "{text}");

        let mut one_arm = payload();
        one_arm.runs[0].plan_usage = vec![share(Plan::Claude, "5h", 0.05)];
        let text = render("Compare #1", &one_arm);
        assert!(text.contains("other: no plan usage reported"), "{text}");
    }

    #[test]
    fn applied_comparisons_offer_nothing_more_and_list_compactly() {
        let mut applied = payload();
        applied.status = CompareStatus::Applied("gate-2".to_string());
        let text = render("Compare #1", &applied);
        assert!(text.contains("applied gate-2"));
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
        assert!(first.contains("gate $0.06 median, 2/3 pass"), "{first}");
        assert!(listed.contains("2. Compare #2 · no record"));
        assert!(list(&[]).starts_with("No comparisons yet"));
    }

    #[test]
    fn the_estimate_names_the_models_runs_and_budget() {
        let text = EstimateView {
            task: "fix it",
            gate_model: Some("aura-claude-sonnet-4-6"),
            against: "aura-claude-opus-4-7",
            judge: "aura-claude-opus-4-7",
            repeats: 3,
            check: None,
            budget_usd: Some(2.0),
            concurrency: 4,
            estimate: Estimate {
                gate_usd: Some(1.5),
                other_usd: Some(4.0),
                judge_usd: None,
            },
        }
        .render();
        assert!(text.contains("gate (likely aura-claude-sonnet-4-6 (cloud)) vs aura-claude-opus-4-7 (cloud), 3 runs each"), "{text}");
        assert!(
            text.contains("~$5.50 (gate ~$1.50, other ~$4.00, judge ~unknown"),
            "{text}"
        );
        assert!(text.contains("some prices are unknown"));
        assert!(text.contains("No check command"));
        assert!(text.contains("Budget $2.00"));
        assert!(text.contains("/compare go"));
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
}
