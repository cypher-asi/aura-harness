//! `/compare`: running the user's own task with Gate and with another
//! model, side by side, and reading the result.
//!
//! This module holds the parts that need no I/O: reading the command's
//! arguments, choosing the model Gate is compared with, the cost
//! estimate shown before anything runs, the order runs start in and when
//! the budget stops them, the tokens and cost per model of a run, and
//! the per-arm medians, pass counts and judge tallies the view shows.
//! Running the arms, the check command and the judge live with the app.

use std::collections::{BTreeMap, VecDeque};

use aura_core_types::{CompareArm, ComparePayload, ModelTokens};
use aura_model_reasoner::model_catalog::{
    backend_for, estimate_cost_usd, model_key, ModelBackend, ModelTier,
};
use aura_model_reasoner::Usage;

use crate::router::Candidate;

/// Runs per arm when the command names no number.
pub const DEFAULT_REPEATS: u32 = 3;

/// Most runs per arm one comparison may ask for.
pub const MAX_REPEATS: u32 = 10;

/// Runs that may work at the same time, across both arms.
pub const DEFAULT_CONCURRENCY: usize = 4;

/// Tokens of a typical agentic run (several model calls over a growing
/// context), for the estimate shown before a comparison starts.
pub const REFERENCE_RUN: Usage = Usage {
    input_tokens: 150_000,
    output_tokens: 6_000,
    cache_creation_input_tokens: None,
    cache_read_input_tokens: None,
};

/// Tokens of one judge call: the task, two diffs and two replies.
pub const REFERENCE_JUDGE: Usage = Usage {
    input_tokens: 12_000,
    output_tokens: 600,
    cache_creation_input_tokens: None,
    cache_read_input_tokens: None,
};

/// What a comparison runs.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareRequest {
    /// The prompt; `None` uses the user's last prompt (or `plan`).
    pub prompt: Option<String>,
    /// A plan file to `/build` in each copy instead of a prompt.
    pub plan: Option<String>,
    /// Model to compare Gate with; `None` for the strongest Opus.
    pub against: Option<String>,
    /// Runs per arm.
    pub repeats: u32,
    /// Command run in each copy afterwards; `None` for the project's.
    pub check: Option<String>,
    /// Spend after which no new run starts, in USD.
    pub budget_usd: Option<f64>,
}

impl Default for CompareRequest {
    fn default() -> Self {
        Self {
            prompt: None,
            plan: None,
            against: None,
            repeats: DEFAULT_REPEATS,
            check: None,
            budget_usd: None,
        }
    }
}

/// What `/compare <args>` asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum CompareCommand {
    /// `/compare` or `/compare list`: past comparisons.
    List,
    /// `/compare show [n]`: comparison `n` (1-based), or the latest.
    Show(Option<usize>),
    /// Prepare a comparison and show its estimate.
    Start(CompareRequest),
    /// `/compare go`: start the comparison waiting for confirmation.
    Go,
    /// `/compare cancel`: drop the one waiting, or stop the one running.
    Cancel,
    /// `/compare pick <arm>`: the user's verdict.
    Pick(String),
    /// `/compare apply <run>`: put that run's changes in the workspace.
    Apply(String),
    /// `/compare diff [run]`: show a run's changes, or the first of each
    /// arm.
    Diff(Option<String>),
    /// `/compare dismiss`: remove the copies.
    Dismiss,
}

/// How `/compare` is used, for error messages.
pub const USAGE: &str = "Usage: /compare [prompt | --plan <file>] [--against <model>] \
[--repeats N] [--check <cmd>] [--budget $] · /compare go | cancel | show [n] | \
diff [run] | pick <gate|other> | apply <run> | dismiss";

/// One word of the arguments: its text with quotes removed, and where it
/// sits in the raw arguments.
#[derive(Debug, Clone, PartialEq)]
struct Word {
    text: String,
    start: usize,
    end: usize,
}

/// Split `args` into words at whitespace; single or double quotes keep
/// whitespace inside a word.
fn words(args: &str) -> Result<Vec<Word>, String> {
    let mut out = Vec::new();
    let mut current: Option<Word> = None;
    let mut quote: Option<char> = None;
    for (i, c) in args.char_indices() {
        let end = i + c.len_utf8();
        match (quote, c) {
            (Some(q), _) if c == q => {
                quote = None;
                if let Some(word) = current.as_mut() {
                    word.end = end;
                }
            }
            (Some(_), _) => {
                let word = current.get_or_insert_with(|| Word {
                    text: String::new(),
                    start: i,
                    end,
                });
                word.text.push(c);
                word.end = end;
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                current.get_or_insert_with(|| Word {
                    text: String::new(),
                    start: i,
                    end,
                });
            }
            (None, _) if c.is_whitespace() => {
                if let Some(word) = current.take() {
                    out.push(word);
                }
            }
            (None, _) => {
                let word = current.get_or_insert_with(|| Word {
                    text: String::new(),
                    start: i,
                    end,
                });
                word.text.push(c);
                word.end = end;
            }
        }
    }
    if quote.is_some() {
        return Err("Unclosed quote in /compare arguments".to_string());
    }
    out.extend(current);
    Ok(out)
}

/// Read the arguments of `/compare`.
///
/// Subcommands (`go`, `pick gate`, ...) are recognised only when the
/// arguments are exactly that, so a prompt that happens to start with
/// one of their names is still a prompt; `--` makes the rest a prompt
/// whatever it says. The prompt keeps its text as typed.
///
/// # Errors
/// Returns what is wrong with the arguments, with the usage.
pub fn parse(args: &str) -> Result<CompareCommand, String> {
    let words = words(args)?;
    if let Some(command) = subcommand(&words) {
        return command;
    }
    let mut request = CompareRequest::default();
    let mut prompt: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = &words[i];
        if word.text == "--" && args[word.start..word.end] == *"--" {
            let rest = words.get(i + 1).map_or("", |w| args[w.start..].trim());
            if !rest.is_empty() {
                prompt.push(rest);
            }
            break;
        }
        let raw = &args[word.start..word.end];
        let Some(flag) = raw.strip_prefix("--") else {
            prompt.push(raw);
            i += 1;
            continue;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value.trim_matches(['"', '\'']).to_string())),
            None => (flag, None),
        };
        let value = match inline {
            Some(value) => value,
            None => {
                i += 1;
                words
                    .get(i)
                    .map(|w| w.text.clone())
                    .ok_or_else(|| format!("--{name} needs a value. {USAGE}"))?
            }
        };
        set_flag(&mut request, name, &value)?;
        i += 1;
    }
    let prompt = prompt.join(" ");
    let prompt = prompt.trim();
    if !prompt.is_empty() {
        if request.plan.is_some() {
            return Err(format!("Give a prompt or --plan, not both. {USAGE}"));
        }
        request.prompt = Some(prompt.to_string());
    }
    Ok(CompareCommand::Start(request))
}

/// The subcommand `words` are, when they are exactly one.
fn subcommand(words: &[Word]) -> Option<Result<CompareCommand, String>> {
    let first = words.first().map(|w| w.text.to_ascii_lowercase());
    let second = words.get(1).map(|w| w.text.clone());
    let command = match (first.as_deref(), words.len()) {
        (None, _) | (Some("list"), 1) => CompareCommand::List,
        (Some("show"), 1) => CompareCommand::Show(None),
        (Some("show"), 2) => {
            let n = second?.parse::<usize>().ok().filter(|n| *n > 0)?;
            CompareCommand::Show(Some(n))
        }
        (Some("go" | "start" | "yes" | "run"), 1) => CompareCommand::Go,
        (Some("cancel" | "stop"), 1) => CompareCommand::Cancel,
        (Some("dismiss"), 1) => CompareCommand::Dismiss,
        (Some("pick"), 2) => CompareCommand::Pick(second?.to_ascii_lowercase()),
        (Some("apply"), 2) => CompareCommand::Apply(second?.to_ascii_lowercase()),
        (Some("diff"), 1) => CompareCommand::Diff(None),
        (Some("diff"), 2) => CompareCommand::Diff(Some(second?.to_ascii_lowercase())),
        (Some("pick" | "apply"), 1) => {
            return Some(Err(format!(
                "/compare {} needs a name. {USAGE}",
                words[0].text
            )));
        }
        _ => return None,
    };
    Some(Ok(command))
}

fn set_flag(request: &mut CompareRequest, name: &str, value: &str) -> Result<(), String> {
    let value_of = |what: &str| -> Result<String, String> {
        let value = value.trim();
        if value.is_empty() {
            Err(format!("--{what} needs a value. {USAGE}"))
        } else {
            Ok(value.to_string())
        }
    };
    match name {
        "plan" => request.plan = Some(value_of(name)?),
        "against" | "vs" => request.against = Some(value_of(name)?),
        "check" => request.check = Some(value_of(name)?),
        "repeats" | "runs" | "n" => {
            let repeats = value.trim().parse::<u32>().ok();
            request.repeats = repeats
                .filter(|n| (1..=MAX_REPEATS).contains(n))
                .ok_or_else(|| {
                    format!("--repeats takes a number from 1 to {MAX_REPEATS}, not `{value}`")
                })?;
        }
        "budget" => {
            let budget = value.trim().trim_start_matches('$').parse::<f64>().ok();
            request.budget_usd = Some(budget.filter(|b| b.is_finite() && *b > 0.0).ok_or_else(
                || format!("--budget takes an amount in USD such as 2.50, not `{value}`"),
            )?);
        }
        other => return Err(format!("Unknown option --{other}. {USAGE}")),
    }
    Ok(())
}

/// Numbers in a model's name, in order (`claude-opus-4-7` → `[4, 7]`),
/// which order its versions.
fn version(id: &str) -> Vec<u32> {
    model_key(id)
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// The strongest of `candidates`: the highest tier, then the latest
/// version, then the higher list price.
fn strongest<'a>(candidates: impl IntoIterator<Item = &'a Candidate>) -> Option<&'a Candidate> {
    candidates.into_iter().max_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then_with(|| version(&a.id).cmp(&version(&b.id)))
            .then_with(|| {
                a.cost_usd
                    .unwrap_or_default()
                    .total_cmp(&b.cost_usd.unwrap_or_default())
            })
    })
}

fn is_opus(candidate: &Candidate) -> bool {
    model_key(&candidate.id).contains("opus")
}

/// The model Gate is compared with by default, which also judges: the
/// strongest Opus available, else the strongest frontier model, else the
/// strongest model there is.
#[must_use]
pub fn default_against(candidates: &[Candidate]) -> Option<&Candidate> {
    strongest(candidates.iter().filter(|c| is_opus(c)))
        .or_else(|| strongest(candidates.iter().filter(|c| c.tier == ModelTier::Frontier)))
        .or_else(|| strongest(candidates))
}

/// The candidate `query` names: its id or label exactly, else the
/// strongest whose id or label contains it (`opus`, `sonnet`).
///
/// # Errors
/// Says which models are available when none matches, or when `query`
/// names Gate itself.
pub fn resolve_against<'a>(
    query: &str,
    candidates: &'a [Candidate],
) -> Result<&'a Candidate, String> {
    let query = query.trim().to_ascii_lowercase();
    if query == "gate" {
        return Err("Compare Gate with another model, e.g. --against opus".to_string());
    }
    let exact = candidates
        .iter()
        .find(|c| c.id.to_ascii_lowercase() == query || c.label.to_ascii_lowercase() == query);
    if let Some(candidate) = exact {
        return Ok(candidate);
    }
    strongest(candidates.iter().filter(|c| {
        c.id.to_ascii_lowercase().contains(&query) || c.label.to_ascii_lowercase().contains(&query)
    }))
    .ok_or_else(|| {
        let available: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        format!(
            "No available model matches `{query}`; available: {}",
            if available.is_empty() {
                "none (sign in or link a plan)".to_string()
            } else {
                available.join(", ")
            }
        )
    })
}

/// List price of `usage` on `model`: 0 for local models, `None` when the
/// price is unknown.
#[must_use]
pub fn list_price(model: &str, usage: &Usage) -> Option<f64> {
    if backend_for(model) == ModelBackend::Local {
        return Some(0.0);
    }
    estimate_cost_usd(model, usage)
}

/// What a comparison is expected to cost at list prices, before it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Every Gate run; `None` when the model's price is unknown.
    pub gate_usd: Option<f64>,
    /// Every run of the other arm.
    pub other_usd: Option<f64>,
    /// Every judge call.
    pub judge_usd: Option<f64>,
}

impl Estimate {
    /// The parts whose price is known, added up.
    #[must_use]
    pub fn known_usd(&self) -> f64 {
        [self.gate_usd, self.other_usd, self.judge_usd]
            .into_iter()
            .flatten()
            .sum()
    }

    /// Whether every part has a price.
    #[must_use]
    pub const fn fully_priced(&self) -> bool {
        self.gate_usd.is_some() && self.other_usd.is_some() && self.judge_usd.is_some()
    }
}

/// The estimate for `repeats` runs per arm, with the Gate arm on
/// `gate_model` (its expected pick), the other on `other_model`, and one
/// judge call per pair on `judge_model`, each run the size of
/// [`REFERENCE_RUN`].
#[must_use]
pub fn estimate(gate_model: &str, other_model: &str, judge_model: &str, repeats: u32) -> Estimate {
    let runs = f64::from(repeats);
    Estimate {
        gate_usd: list_price(gate_model, &REFERENCE_RUN).map(|usd| usd * runs),
        other_usd: list_price(other_model, &REFERENCE_RUN).map(|usd| usd * runs),
        judge_usd: list_price(judge_model, &REFERENCE_JUDGE).map(|usd| usd * runs),
    }
}

/// Which runs start when: the arms alternate (`gate-1`, `other-1`,
/// `gate-2`, ...) so a budget that runs out leaves them balanced; at most
/// `cap` work at once; and once what the finished runs cost reaches the
/// budget, no new run starts.
#[derive(Debug, Clone)]
pub struct Schedule {
    pending: VecDeque<(CompareArm, u32)>,
    running: usize,
    cap: usize,
    budget_usd: Option<f64>,
    spent_usd: f64,
}

impl Schedule {
    /// `repeats` runs per arm, `cap` at a time, stopping at `budget_usd`.
    #[must_use]
    pub fn new(repeats: u32, cap: usize, budget_usd: Option<f64>) -> Self {
        let pending = (1..=repeats)
            .flat_map(|n| CompareArm::BOTH.map(|arm| (arm, n)))
            .collect();
        Self {
            pending,
            running: 0,
            cap: cap.max(1),
            budget_usd,
            spent_usd: 0.0,
        }
    }

    /// The next run to start now, if one may: there is room under the
    /// cap and money left in the budget. Counts it as running.
    pub fn start_next(&mut self) -> Option<(CompareArm, u32)> {
        if self.running >= self.cap || self.over_budget() {
            return None;
        }
        let next = self.pending.pop_front()?;
        self.running += 1;
        Some(next)
    }

    /// A run ended, having cost `cost_usd`.
    pub fn finished(&mut self, cost_usd: f64) {
        self.running = self.running.saturating_sub(1);
        self.spent_usd += cost_usd.max(0.0);
    }

    /// Something other than a run cost `cost_usd` (the judge).
    pub fn spend(&mut self, cost_usd: f64) {
        self.spent_usd += cost_usd.max(0.0);
    }

    /// Whether the budget is spent.
    #[must_use]
    pub fn over_budget(&self) -> bool {
        self.budget_usd
            .is_some_and(|budget| self.spent_usd >= budget)
    }

    /// Runs still working.
    #[must_use]
    pub const fn running(&self) -> usize {
        self.running
    }

    /// What the finished runs cost so far.
    #[must_use]
    pub const fn spent_usd(&self) -> f64 {
        self.spent_usd
    }

    /// Drop the runs that have not started (the comparison is stopped,
    /// or the budget ran out) and return them.
    pub fn drop_pending(&mut self) -> Vec<(CompareArm, u32)> {
        self.pending.drain(..).collect()
    }
}

/// The name of run `n` of `arm`: `gate-1`, `other-2`.
#[must_use]
pub fn run_name(arm: CompareArm, n: u32) -> String {
    format!("{arm}-{n}")
}

/// Tokens and list-price cost per model of `calls` (each a model call's
/// model and usage), most expensive first.
#[must_use]
pub fn tally(calls: impl IntoIterator<Item = (String, Usage)>) -> Vec<ModelTokens> {
    let mut by_model: BTreeMap<String, ModelTokens> = BTreeMap::new();
    for (model, usage) in calls {
        let row = by_model
            .entry(model.clone())
            .or_insert_with(|| ModelTokens {
                model: model.clone(),
                ..ModelTokens::default()
            });
        row.calls += 1;
        row.input_tokens += usage.input_tokens;
        row.output_tokens += usage.output_tokens;
        row.cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
        row.cache_write_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
        row.cost_usd += list_price(&model, &usage).unwrap_or(0.0);
    }
    sorted(by_model.into_values().collect())
}

/// `rows` added up per model, most expensive first.
#[must_use]
pub fn merge_tokens<'a>(rows: impl IntoIterator<Item = &'a ModelTokens>) -> Vec<ModelTokens> {
    let mut by_model: BTreeMap<&str, ModelTokens> = BTreeMap::new();
    for row in rows {
        let sum = by_model.entry(&row.model).or_insert_with(|| ModelTokens {
            model: row.model.clone(),
            ..ModelTokens::default()
        });
        sum.calls += row.calls;
        sum.input_tokens += row.input_tokens;
        sum.output_tokens += row.output_tokens;
        sum.cache_read_tokens += row.cache_read_tokens;
        sum.cache_write_tokens += row.cache_write_tokens;
        sum.cost_usd += row.cost_usd;
    }
    sorted(by_model.into_values().collect())
}

fn sorted(mut rows: Vec<ModelTokens>) -> Vec<ModelTokens> {
    rows.sort_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then_with(|| a.model.cmp(&b.model))
    });
    rows
}

/// The median of `values`; the mean of the middle two for an even count.
#[must_use]
pub fn median(values: &[f64]) -> Option<f64> {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

/// One arm of a comparison, as the view shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmSummary {
    pub arm: CompareArm,
    /// Runs that started.
    pub runs: usize,
    /// Runs that ended without an error.
    pub finished: usize,
    /// Median wall time of the finished runs.
    pub median_wall_ms: Option<u64>,
    /// What every run cost.
    pub total_cost_usd: f64,
    /// Median cost of the finished runs.
    pub median_cost_usd: Option<f64>,
    /// Tokens per model over every run, most expensive first.
    pub tokens: Vec<ModelTokens>,
    /// Runs whose check passed.
    pub checks_passed: usize,
    /// Runs whose check ran.
    pub checks_run: usize,
    /// Pairs the judge gave this arm.
    pub judge_wins: usize,
    /// Pairs the judge gave the other arm.
    pub judge_losses: usize,
    /// Pairs the judge called even.
    pub judge_ties: usize,
}

/// What `payload` says about `arm`.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn summarize(payload: &ComparePayload, arm: CompareArm) -> ArmSummary {
    let runs: Vec<_> = payload.arm_runs(arm).collect();
    let finished: Vec<_> = runs.iter().filter(|run| run.error.is_none()).collect();
    #[allow(clippy::cast_precision_loss)]
    let walls: Vec<f64> = finished.iter().map(|run| run.wall_ms as f64).collect();
    let costs: Vec<f64> = finished.iter().map(|run| run.cost_usd).collect();
    let judged: Vec<_> = payload.judgments.iter().filter(|j| j.parsed).collect();
    ArmSummary {
        arm,
        runs: runs.len(),
        finished: finished.len(),
        median_wall_ms: median(&walls).map(|ms| ms.round() as u64),
        total_cost_usd: runs.iter().map(|run| run.cost_usd).sum(),
        median_cost_usd: median(&costs),
        tokens: merge_tokens(runs.iter().flat_map(|run| run.tokens.iter())),
        checks_passed: runs.iter().filter(|r| r.check_passed == Some(true)).count(),
        checks_run: runs.iter().filter(|r| r.check_passed.is_some()).count(),
        judge_wins: judged.iter().filter(|j| j.winner == Some(arm)).count(),
        judge_losses: judged
            .iter()
            .filter(|j| j.winner == Some(arm.opposite()))
            .count(),
        judge_ties: judged.iter().filter(|j| j.winner.is_none()).count(),
    }
}

/// What every judge call of `payload` cost.
#[must_use]
pub fn judge_cost_usd(payload: &ComparePayload) -> f64 {
    payload.judgments.iter().map(|j| j.cost_usd).sum()
}

/// Whether two counts out of a handful of runs are too close to tell
/// the arms apart: within one of each other.
#[must_use]
pub const fn too_close(a: usize, b: usize) -> bool {
    a.abs_diff(b) <= 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::candidates;
    use aura_core_auth::ProviderKind;
    use aura_core_types::{
        AgentId, CompareCopy, CompareJudgment, CompareRun, CompareStatus, SystemKind,
    };
    use aura_model_reasoner::model_catalog::catalog;
    use std::path::PathBuf;

    fn start(args: &str) -> CompareRequest {
        match parse(args) {
            Ok(CompareCommand::Start(request)) => request,
            other => panic!("{args:?} gave {other:?}"),
        }
    }

    #[test]
    fn no_arguments_list_past_comparisons() {
        assert_eq!(parse(""), Ok(CompareCommand::List));
        assert_eq!(parse("  list "), Ok(CompareCommand::List));
    }

    #[test]
    fn flags_and_the_prompt_are_read_in_any_order() {
        let request = start(
            r#"--repeats 5 fix the "flaky" parser test --check "cargo test -p parser" --budget $2.50 --against=opus"#,
        );
        assert_eq!(
            request,
            CompareRequest {
                prompt: Some(r#"fix the "flaky" parser test"#.to_string()),
                plan: None,
                against: Some("opus".to_string()),
                repeats: 5,
                check: Some("cargo test -p parser".to_string()),
                budget_usd: Some(2.5),
            }
        );
    }

    #[test]
    fn defaults_apply_when_nothing_is_given() {
        assert_eq!(
            start("--check true"),
            CompareRequest {
                check: Some("true".to_string()),
                ..CompareRequest::default()
            }
        );
        assert_eq!(CompareRequest::default().repeats, DEFAULT_REPEATS);
        assert_eq!(
            start("--plan .aura/plans/x.plan.md").plan.as_deref(),
            Some(".aura/plans/x.plan.md")
        );
    }

    #[test]
    fn subcommands_are_whole_arguments_only() {
        assert_eq!(parse("go"), Ok(CompareCommand::Go));
        assert_eq!(parse("cancel"), Ok(CompareCommand::Cancel));
        assert_eq!(parse("dismiss"), Ok(CompareCommand::Dismiss));
        assert_eq!(parse("show"), Ok(CompareCommand::Show(None)));
        assert_eq!(parse("show 2"), Ok(CompareCommand::Show(Some(2))));
        assert_eq!(parse("pick Gate"), Ok(CompareCommand::Pick("gate".into())));
        assert_eq!(
            parse("apply other-2"),
            Ok(CompareCommand::Apply("other-2".into()))
        );
        assert_eq!(parse("diff"), Ok(CompareCommand::Diff(None)));
        assert_eq!(
            parse("diff gate-1"),
            Ok(CompareCommand::Diff(Some("gate-1".into())))
        );
        assert!(parse("pick").is_err());
        assert_eq!(
            start("pick the fastest sort").prompt.as_deref(),
            Some("pick the fastest sort")
        );
        assert_eq!(start("-- go").prompt.as_deref(), Some("go"));
        assert_eq!(
            start("show me the parser").prompt.as_deref(),
            Some("show me the parser")
        );
    }

    #[test]
    fn bad_arguments_say_what_is_wrong() {
        let error = |args: &str| parse(args).unwrap_err();
        assert!(error("--repeats 0").contains("1 to 10"));
        assert!(error("--repeats eleven").contains("1 to 10"));
        assert!(error("--budget -1").contains("USD"));
        assert!(error("--colour red x").contains("Unknown option --colour"));
        assert!(error("fix it --check").contains("needs a value"));
        assert!(error("fix it --plan p.plan.md").contains("not both"));
        assert!(error("\"unclosed").contains("Unclosed quote"));
    }

    fn zos_candidates() -> Vec<Candidate> {
        candidates(
            &catalog("local/qwen3:4b", &["qwen3:4b".to_string()]),
            |p| p == ProviderKind::Zos,
            |_| false,
        )
    }

    fn with_claude_plan() -> Vec<Candidate> {
        let mut all = zos_candidates();
        let mut opus55 = all
            .iter()
            .find(|c| c.id == "aura-claude-opus-4-7")
            .unwrap()
            .clone();
        opus55.id = "claude/claude-opus-5-5".to_string();
        all.push(opus55);
        all
    }

    #[test]
    fn the_strongest_opus_is_the_default_opponent() {
        let zos = zos_candidates();
        assert_eq!(default_against(&zos).unwrap().id, "aura-claude-opus-4-7");
        assert_eq!(
            default_against(&with_claude_plan()).unwrap().id,
            "claude/claude-opus-5-5",
            "the later version wins"
        );
        let no_opus: Vec<Candidate> = zos.iter().filter(|c| !is_opus(c)).cloned().collect();
        assert_eq!(
            default_against(&no_opus).unwrap().id,
            "aura-gpt-5-4",
            "else the strongest frontier model"
        );
        let local: Vec<Candidate> = zos
            .iter()
            .filter(|c| c.tier == ModelTier::Local)
            .cloned()
            .collect();
        assert_eq!(default_against(&local).unwrap().id, "local/qwen3:4b");
        assert_eq!(default_against(&[]), None);
    }

    #[test]
    fn against_names_a_model_exactly_or_by_part_of_its_name() {
        let all = with_claude_plan();
        assert_eq!(
            resolve_against("opus", &all).unwrap().id,
            "claude/claude-opus-5-5"
        );
        assert_eq!(
            resolve_against("AURA-CLAUDE-OPUS-4-7", &all).unwrap().id,
            "aura-claude-opus-4-7"
        );
        assert_eq!(
            resolve_against("sonnet", &all).unwrap().id,
            "aura-claude-sonnet-4-6"
        );
        assert!(resolve_against("gate", &all)
            .unwrap_err()
            .contains("another model"));
        let missing = resolve_against("llama", &all).unwrap_err();
        assert!(missing.contains("aura-claude-haiku-4-5"), "{missing}");
        assert!(resolve_against("opus", &[])
            .unwrap_err()
            .contains("sign in"));
    }

    #[test]
    fn the_estimate_prices_every_run_and_judge_call() {
        let estimate = estimate(
            "aura-claude-sonnet-4-6",
            "aura-claude-opus-4-7",
            "aura-claude-opus-4-7",
            3,
        );
        let one_sonnet = estimate_cost_usd("aura-claude-sonnet-4-6", &REFERENCE_RUN).unwrap();
        let one_opus = estimate_cost_usd("aura-claude-opus-4-7", &REFERENCE_RUN).unwrap();
        let one_judge = estimate_cost_usd("aura-claude-opus-4-7", &REFERENCE_JUDGE).unwrap();
        assert!((estimate.gate_usd.unwrap() - 3.0 * one_sonnet).abs() < 1e-9);
        assert!((estimate.other_usd.unwrap() - 3.0 * one_opus).abs() < 1e-9);
        assert!((estimate.judge_usd.unwrap() - 3.0 * one_judge).abs() < 1e-9);
        assert!(estimate.fully_priced());
        assert!(estimate.other_usd > estimate.gate_usd);

        let local = super::estimate("local/qwen3:4b", "mystery-model", "local/qwen3:4b", 2);
        assert_eq!(local.gate_usd, Some(0.0));
        assert_eq!(local.other_usd, None);
        assert!(!local.fully_priced());
        assert!(local.known_usd().abs() < f64::EPSILON);
    }

    fn drain(schedule: &mut Schedule) -> Vec<(CompareArm, u32)> {
        std::iter::from_fn(|| schedule.start_next()).collect()
    }

    #[test]
    fn runs_alternate_between_arms_up_to_the_cap() {
        let mut schedule = Schedule::new(3, 4, None);
        assert_eq!(
            drain(&mut schedule),
            [
                (CompareArm::Gate, 1),
                (CompareArm::Other, 1),
                (CompareArm::Gate, 2),
                (CompareArm::Other, 2),
            ]
        );
        assert_eq!(schedule.running(), 4);
        schedule.finished(0.1);
        assert_eq!(drain(&mut schedule), [(CompareArm::Gate, 3)]);
        schedule.finished(0.1);
        assert_eq!(drain(&mut schedule), [(CompareArm::Other, 3)]);
        assert!(drain(&mut schedule).is_empty());
        assert!(schedule.drop_pending().is_empty());
        assert_eq!(run_name(CompareArm::Other, 3), "other-3");
    }

    #[test]
    fn the_budget_stops_new_runs_once_spent() {
        let mut schedule = Schedule::new(3, 1, Some(0.25));
        assert_eq!(drain(&mut schedule), [(CompareArm::Gate, 1)]);
        schedule.finished(0.10);
        assert_eq!(drain(&mut schedule), [(CompareArm::Other, 1)]);
        schedule.finished(0.20);
        assert!(schedule.over_budget());
        assert!(
            drain(&mut schedule).is_empty(),
            "no run starts past the budget"
        );
        assert_eq!(schedule.running(), 0);
        assert_eq!(schedule.drop_pending().len(), 4);
        assert!((schedule.spent_usd() - 0.30).abs() < 1e-9);

        let mut unlimited = Schedule::new(1, 0, None);
        assert_eq!(drain(&mut unlimited).len(), 1, "a cap of 0 still runs one");
    }

    fn usage(input: u64, output: u64, cache_read: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(cache_read),
        }
    }

    #[test]
    fn tally_adds_up_tokens_and_cost_per_model() {
        let rows = tally([
            ("claude-haiku-4-5".to_string(), usage(1_000, 10, 0)),
            ("claude-opus-4-7".to_string(), usage(10_000, 500, 2_000)),
            ("claude-opus-4-7".to_string(), usage(20_000, 500, 0)),
            ("local/qwen3:4b".to_string(), usage(5_000, 100, 0)),
        ]);
        let models: Vec<&str> = rows.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(
            models,
            ["claude-opus-4-7", "claude-haiku-4-5", "local/qwen3:4b"],
            "most expensive first"
        );
        assert_eq!(rows[0].calls, 2);
        assert_eq!(rows[0].input_tokens, 30_000);
        assert_eq!(rows[0].output_tokens, 1_000);
        assert_eq!(rows[0].cache_read_tokens, 2_000);
        let expected = estimate_cost_usd("claude-opus-4-7", &usage(10_000, 500, 2_000)).unwrap()
            + estimate_cost_usd("claude-opus-4-7", &usage(20_000, 500, 0)).unwrap();
        assert!((rows[0].cost_usd - expected).abs() < 1e-12);
        assert!(
            rows[2].cost_usd.abs() < f64::EPSILON,
            "local models are free"
        );

        let merged = merge_tokens(rows.iter().chain(rows.iter()));
        assert_eq!(merged[0].calls, 4);
        assert!((merged[0].cost_usd - 2.0 * expected).abs() < 1e-12);
    }

    #[test]
    fn medians_take_the_middle_or_the_mean_of_the_middle_two() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0]), Some(3.0));
        assert_eq!(median(&[9.0, 1.0, 5.0]), Some(5.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 10.0]), Some(3.5));
    }

    fn run(arm: CompareArm, n: u32, wall_ms: u64, cost: f64, check: Option<bool>) -> CompareRun {
        CompareRun {
            name: run_name(arm, n),
            arm,
            agent_id: AgentId::generate(),
            prompt_tx: None,
            model: "m".to_string(),
            copy: CompareCopy {
                root: PathBuf::new(),
                workdir: PathBuf::new(),
                git_dir: None,
                baseline: String::new(),
            },
            wall_ms,
            cost_usd: cost,
            tokens: vec![ModelTokens {
                model: format!("model-{arm}"),
                calls: 2,
                input_tokens: 100,
                cost_usd: cost,
                ..ModelTokens::default()
            }],
            check_passed: check,
            files_changed: 1,
            error: None,
        }
    }

    fn judged(gate_run: &str, winner: Option<CompareArm>, parsed: bool) -> CompareJudgment {
        CompareJudgment {
            gate_run: gate_run.to_string(),
            other_run: gate_run.replace("gate", "other"),
            gate_first: true,
            winner,
            parsed,
            model: "judge".to_string(),
            cost_usd: 0.01,
            reasoning: String::new(),
        }
    }

    #[test]
    fn summaries_give_medians_pass_counts_and_judge_tallies_per_arm() {
        let mut failed = run(CompareArm::Other, 3, 1, 0.5, None);
        failed.error = Some("model fell over".to_string());
        let payload = ComparePayload {
            system_kind: SystemKind::Compare,
            task: "fix it".to_string(),
            plan: None,
            against: "opus".to_string(),
            repeats: 3,
            check: Some("true".to_string()),
            budget_usd: None,
            requested_by: AgentId::generate(),
            apply_root: PathBuf::new(),
            started_ms: 0,
            runs: vec![
                run(CompareArm::Gate, 1, 30_000, 0.10, Some(true)),
                run(CompareArm::Other, 1, 60_000, 0.90, Some(true)),
                run(CompareArm::Gate, 2, 10_000, 0.20, Some(false)),
                run(CompareArm::Other, 2, 50_000, 1.10, Some(true)),
                run(CompareArm::Gate, 3, 20_000, 0.30, Some(true)),
                failed,
            ],
            judgments: vec![
                judged("gate-1", Some(CompareArm::Gate), true),
                judged("gate-2", Some(CompareArm::Other), true),
                judged("gate-3", None, true),
                judged("gate-4", Some(CompareArm::Gate), false),
            ],
            notes: Vec::new(),
            status: CompareStatus::Done,
        };

        let gate = summarize(&payload, CompareArm::Gate);
        assert_eq!((gate.runs, gate.finished), (3, 3));
        assert_eq!(gate.median_wall_ms, Some(20_000));
        assert!((gate.total_cost_usd - 0.60).abs() < 1e-9);
        assert!((gate.median_cost_usd.unwrap() - 0.20).abs() < 1e-9);
        assert_eq!((gate.checks_passed, gate.checks_run), (2, 3));
        assert_eq!(
            (gate.judge_wins, gate.judge_losses, gate.judge_ties),
            (1, 1, 1)
        );
        assert_eq!(gate.tokens.len(), 1);
        assert_eq!(gate.tokens[0].calls, 6);

        let other = summarize(&payload, CompareArm::Other);
        assert_eq!((other.runs, other.finished), (3, 2));
        assert_eq!(
            other.median_wall_ms,
            Some(55_000),
            "the failed run is left out"
        );
        assert!(
            (other.total_cost_usd - 2.50).abs() < 1e-9,
            "but its cost counts"
        );
        assert_eq!((other.checks_passed, other.checks_run), (2, 2));
        assert_eq!((other.judge_wins, other.judge_losses), (1, 1));
        assert!((judge_cost_usd(&payload) - 0.04).abs() < 1e-9);
        assert!(too_close(gate.checks_passed, other.checks_passed));
        assert!(!too_close(3, 1));
    }
}
