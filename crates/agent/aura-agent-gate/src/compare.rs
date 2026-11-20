//! `/compare` and the BENCH tab: running the same work on two arms, each
//! Gate or a pinned model, side by side, and reading the result.
//!
//! This module holds the parts that need no I/O: reading the command's
//! arguments, choosing what each arm runs on, the cost estimate shown
//! before anything runs, the order runs start in and when the budget
//! stops them, the tokens and cost per model of a run, the share of each
//! plan window the runs used, and the per-arm medians, pass counts,
//! judge tallies and per-prompt results the views show.
//! Running the arms, the check command and the judge live with the app.

use std::collections::{BTreeMap, VecDeque};

use aura_core_types::{
    ArmSpec, CompareArm, ComparePayload, ModelTokens, OutcomeSignal, Plan, PlanShare, PlanUsage,
    RefVerdict,
};
use aura_model::model_catalog::{
    backend_for, billed_tokens, estimate_cost_usd, model_key, ModelBackend, ModelTier,
};
use aura_model::Usage;

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

/// Which earlier session a comparison replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPick {
    /// The session the agent is in.
    Current,
    /// The session that started at this Unix millisecond.
    Started(u64),
}

/// What a comparison runs.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareRequest {
    /// The prompt; `None` uses the user's last prompt (or `plan`, or
    /// `session`).
    pub prompt: Option<String>,
    /// A plan file to `/build` in each copy instead of a prompt.
    pub plan: Option<String>,
    /// A session whose prompts each run replays instead of a prompt.
    pub session: Option<SessionPick>,
    /// What arm A runs on; `None` for Gate.
    pub a: Option<String>,
    /// What arm B runs on; `None` for the strongest Opus.
    pub against: Option<String>,
    /// Runs per arm.
    pub repeats: u32,
    /// Command run in each copy afterwards; `None` for the project's.
    pub check: Option<String>,
    /// Spend after which no new run starts, in USD.
    pub budget_usd: Option<f64>,
    /// How many of the built-in example coding tasks to run instead of a
    /// prompt; `repeats` runs of each per arm.
    pub examples: Option<usize>,
}

impl Default for CompareRequest {
    fn default() -> Self {
        Self {
            prompt: None,
            plan: None,
            session: None,
            a: None,
            against: None,
            repeats: DEFAULT_REPEATS,
            check: None,
            budget_usd: None,
            examples: None,
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
pub const USAGE: &str = "Usage: /compare [prompt | --plan <file> | --session current|<started_ms> | --examples N] \
[--a <model|gate>] [--against <model|gate|gate:nodelegate>] [--repeats N] [--check <cmd>] [--budget $] · \
/compare go | cancel | show [n] | diff [run] | pick <a|b> | apply <run> | dismiss";

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
    let sources = [
        !prompt.is_empty(),
        request.plan.is_some(),
        request.session.is_some(),
        request.examples.is_some(),
    ];
    if sources.into_iter().filter(|given| *given).count() > 1 {
        return Err(format!(
            "Give a prompt, --plan, --session or --examples, not more than one. {USAGE}"
        ));
    }
    if !prompt.is_empty() {
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
        "session" => {
            let value = value_of(name)?;
            request.session = Some(if value.eq_ignore_ascii_case("current") {
                SessionPick::Current
            } else {
                SessionPick::Started(value.parse().map_err(|_| {
                    format!("--session takes `current` or a session's start time, not `{value}`")
                })?)
            });
        }
        "examples" | "tasks" => {
            let tasks = value.trim().parse::<usize>().ok().filter(|n| *n > 0);
            request.examples = Some(tasks.ok_or_else(|| {
                format!("--examples takes how many example tasks to run, not `{value}`")
            })?);
        }
        "a" => request.a = Some(value_of(name)?),
        "against" | "vs" | "b" => request.against = Some(value_of(name)?),
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
        return Err(format!(
            "Compare Gate with another model, e.g. --against opus, or with itself \
             without enforced delegation: --against {NO_DELEGATE}"
        ));
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

/// What `--against` takes to compare Gate with Gate whose lead does not
/// have to delegate (as with `AURA_GATE_DELEGATE=off`).
pub const NO_DELEGATE: &str = ArmSpec::NO_DELEGATE;

/// What `--a` takes for Gate whose lead must delegate, whatever
/// `AURA_GATE_DELEGATE` says.
pub const DELEGATE: &str = "gate:delegate";

/// The arm `query` (an `--a` or `--against` value) names among
/// `candidates`: Gate (whose lead delegates as `delegate` says, unless
/// the query says), else a model as [`resolve_against`] finds it.
///
/// # Errors
/// As [`resolve_against`].
pub fn resolve_arm(
    query: &str,
    candidates: &[Candidate],
    delegate: bool,
) -> Result<ArmSpec, String> {
    let trimmed = query.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "gate" => Ok(ArmSpec::Gate { delegate }),
        NO_DELEGATE => Ok(ArmSpec::Gate { delegate: false }),
        DELEGATE => Ok(ArmSpec::Gate { delegate: true }),
        _ => resolve_against(query, candidates).map(|c| ArmSpec::Model {
            model: c.id.clone(),
        }),
    }
}

/// Both arms of a comparison from their queries. Plain `gate` delegates
/// as `delegate` says, except against [`NO_DELEGATE`], where it always
/// delegates so the comparison measures delegation itself.
///
/// # Errors
/// As [`resolve_arm`], or when both arms run on the same thing.
pub fn resolve_arms(
    (a, b): (&str, &str),
    candidates: &[Candidate],
    delegate: bool,
) -> Result<(ArmSpec, ArmSpec), String> {
    let off = |query: &str| query.trim().eq_ignore_ascii_case(NO_DELEGATE);
    let a_spec = resolve_arm(a, candidates, delegate || off(b))?;
    let b_spec = resolve_arm(b, candidates, delegate || off(a))?;
    if a_spec == b_spec {
        return Err(format!(
            "Both arms run on {}; pick two different ones",
            a_spec.label()
        ));
    }
    Ok((a_spec, b_spec))
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
    /// Every run of arm A; `None` when the model's price is unknown.
    pub a_usd: Option<f64>,
    /// Every run of arm B.
    pub b_usd: Option<f64>,
    /// Every judge call.
    pub judge_usd: Option<f64>,
}

impl Estimate {
    /// The parts whose price is known, added up.
    #[must_use]
    pub fn known_usd(&self) -> f64 {
        [self.a_usd, self.b_usd, self.judge_usd]
            .into_iter()
            .flatten()
            .sum()
    }

    /// Whether every part has a price.
    #[must_use]
    pub const fn fully_priced(&self) -> bool {
        self.a_usd.is_some() && self.b_usd.is_some() && self.judge_usd.is_some()
    }
}

/// The estimate for `repeats` runs per arm of `prompts` prompts each,
/// with arm A on `a_model` (Gate's expected pick when Gate routes it),
/// arm B on `b_model`, and one judge call per pair on `judge_model`, each
/// prompt's turn the size of [`REFERENCE_RUN`].
#[must_use]
pub fn estimate(
    a_model: &str,
    b_model: &str,
    judge_model: &str,
    repeats: u32,
    prompts: usize,
) -> Estimate {
    let runs = f64::from(repeats);
    let turns = runs * f64::from(u32::try_from(prompts.max(1)).unwrap_or(u32::MAX));
    Estimate {
        a_usd: list_price(a_model, &REFERENCE_RUN).map(|usd| usd * turns),
        b_usd: list_price(b_model, &REFERENCE_RUN).map(|usd| usd * turns),
        judge_usd: list_price(judge_model, &REFERENCE_JUDGE).map(|usd| usd * runs),
    }
}

/// Which runs start when: the arms alternate (`a-1`, `b-1`, `a-2`, ...) so a budget that runs out leaves them balanced; at most
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

/// The name of run `n` of `arm`: `a-1`, `b-2`.
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
        let billed = billed_tokens(&model, &usage);
        row.calls += 1;
        row.input_tokens += billed.uncached_input + billed.cache_write + billed.cache_read;
        row.output_tokens += usage.output_tokens;
        row.cache_read_tokens += billed.cache_read;
        row.cache_write_tokens += billed.cache_write;
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
    let mut ordered: Vec<(u64, usize, usize, &PlanUsage)> = readings
        .iter()
        .enumerate()
        .flat_map(|(run, calls)| {
            calls
                .iter()
                .enumerate()
                .map(move |(i, call)| (call.recorded_ms, run, i, &call.usage))
        })
        .collect();
    ordered.sort_by_key(|&(ms, run, i, _)| (ms, run, i));

    let mut highest: BTreeMap<(Plan, &str), (f64, Option<u64>)> = BTreeMap::new();
    let mut shares: Vec<Vec<PlanShare>> = vec![Vec::new(); readings.len()];
    for (_, run, _, usage) in ordered {
        let key = (usage.plan, usage.window.as_str());
        let added = match highest.get(&key).copied() {
            None => {
                highest.insert(key, (usage.used, usage.resets_at));
                0.0
            }
            Some((top, reset)) => {
                let restarted = matches!(
                    (reset, usage.resets_at),
                    (Some(old), Some(new)) if new > old.saturating_add(RESET_SLACK_SECS)
                );
                if restarted {
                    highest.insert(key, (usage.used, usage.resets_at));
                    usage.used
                } else if usage.used > top {
                    highest.insert(key, (usage.used, usage.resets_at.or(reset)));
                    usage.used - top
                } else {
                    0.0
                }
            }
        };
        let row = &mut shares[run];
        match row
            .iter_mut()
            .find(|share| share.plan == usage.plan && share.window == usage.window)
        {
            Some(share) => {
                share.last = usage.used;
                share.used += added;
            }
            None => row.push(PlanShare {
                plan: usage.plan,
                window: usage.window.clone(),
                first: usage.used,
                last: usage.used,
                used: added,
            }),
        }
    }
    for row in &mut shares {
        sort_shares(row);
    }
    shares
}

/// `rows` added up per plan window: the first `first`, the last `last`,
/// and every `used`.
#[must_use]
pub fn merge_plan_shares<'a>(rows: impl IntoIterator<Item = &'a PlanShare>) -> Vec<PlanShare> {
    let mut merged: Vec<PlanShare> = Vec::new();
    for row in rows {
        match merged
            .iter_mut()
            .find(|sum| sum.plan == row.plan && sum.window == row.window)
        {
            Some(sum) => {
                sum.last = row.last;
                sum.used += row.used;
            }
            None => merged.push(row.clone()),
        }
    }
    sort_shares(&mut merged);
    merged
}

fn sort_shares(rows: &mut [PlanShare]) {
    rows.sort_by(|a, b| a.plan.cmp(&b.plan).then_with(|| a.window.cmp(&b.window)));
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
    /// Share of each plan window the arm's runs used.
    pub plan_usage: Vec<PlanShare>,
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
    /// Runs the judge rated better than the reference.
    pub ref_better: usize,
    /// Runs the judge rated as good as the reference.
    pub ref_as_good: usize,
    /// Runs the judge rated worse than the reference.
    pub ref_worse: usize,
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
    let rated = |verdict: RefVerdict| {
        judged
            .iter()
            .filter(|j| j.vs_reference(arm) == Some(verdict))
            .count()
    };
    ArmSummary {
        arm,
        runs: runs.len(),
        finished: finished.len(),
        median_wall_ms: median(&walls).map(|ms| ms.round() as u64),
        total_cost_usd: runs.iter().map(|run| run.cost_usd).sum(),
        median_cost_usd: median(&costs),
        tokens: merge_tokens(runs.iter().flat_map(|run| run.tokens.iter())),
        plan_usage: merge_plan_shares(runs.iter().flat_map(|run| run.plan_usage.iter())),
        checks_passed: runs.iter().filter(|r| r.check_passed == Some(true)).count(),
        checks_run: runs.iter().filter(|r| r.check_passed.is_some()).count(),
        judge_wins: judged.iter().filter(|j| j.winner == Some(arm)).count(),
        judge_losses: judged
            .iter()
            .filter(|j| j.winner == Some(arm.opposite()))
            .count(),
        judge_ties: judged.iter().filter(|j| j.winner.is_none()).count(),
        ref_better: rated(RefVerdict::Better),
        ref_as_good: rated(RefVerdict::AsGood),
        ref_worse: rated(RefVerdict::Worse),
    }
}

/// How the runs of one arm did on one prompt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnCell {
    /// Runs that got to the prompt.
    pub runs: usize,
    /// Mean cost of their turns.
    pub mean_cost_usd: Option<f64>,
    /// Turns whose last test or build command passed.
    pub passed: usize,
    /// Turns whose last test or build command failed.
    pub failed: usize,
    /// Turns that ended in an error.
    pub errors: usize,
}

/// One cell per prompt of `payload`, for the runs of `arm`.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn turn_cells(payload: &ComparePayload, arm: CompareArm) -> Vec<TurnCell> {
    (0..payload.prompts.len())
        .map(|i| {
            let turns: Vec<_> = payload
                .arm_runs(arm)
                .filter_map(|run| run.turns.get(i))
                .collect();
            let count = |signal: OutcomeSignal| turns.iter().filter(|t| t.signal == signal).count();
            TurnCell {
                runs: turns.len(),
                mean_cost_usd: (!turns.is_empty())
                    .then(|| turns.iter().map(|t| t.cost_usd).sum::<f64>() / turns.len() as f64),
                passed: count(OutcomeSignal::ChecksPassed),
                failed: count(OutcomeSignal::ChecksFailed),
                errors: count(OutcomeSignal::Error),
            }
        })
        .collect()
}

/// How the runs of one arm did on one example task.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskCell {
    /// Runs that worked on it.
    pub runs: usize,
    /// Runs whose check passed.
    pub passed: usize,
    /// Runs whose check ran.
    pub checked: usize,
    /// Mean cost of its runs.
    pub mean_cost_usd: Option<f64>,
}

/// One cell per example task of `payload`, for the runs of `arm`; empty
/// unless the work is example tasks.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn task_cells(payload: &ComparePayload, arm: CompareArm) -> Vec<TaskCell> {
    payload
        .source
        .examples()
        .iter()
        .map(|task| {
            let runs: Vec<_> = payload
                .arm_runs(arm)
                .filter(|run| run.task.as_deref() == Some(task.id.as_str()))
                .collect();
            TaskCell {
                runs: runs.len(),
                passed: runs.iter().filter(|r| r.check_passed == Some(true)).count(),
                checked: runs.iter().filter(|r| r.check_passed.is_some()).count(),
                mean_cost_usd: (!runs.is_empty())
                    .then(|| runs.iter().map(|r| r.cost_usd).sum::<f64>() / runs.len() as f64),
            }
        })
        .collect()
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
        AgentId, CompareCopy, CompareJudgment, CompareRun, CompareStatus, RunTurn, SystemKind,
    };
    use aura_model::model_catalog::catalog;
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
                session: None,
                a: None,
                against: Some("opus".to_string()),
                repeats: 5,
                check: Some("cargo test -p parser".to_string()),
                budget_usd: Some(2.5),
                examples: None,
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
        assert_eq!(start("--examples 5 --repeats 1").examples, Some(5));
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
        assert!(error("fix it --plan p.plan.md").contains("not more than one"));
        assert!(error("--plan p.plan.md --session current").contains("not more than one"));
        assert!(error("fix it --examples 3").contains("not more than one"));
        assert!(error("--examples none").contains("how many example tasks"));
        assert!(error("--examples 0").contains("how many example tasks"));
        assert!(error("--session yesterday").contains("start time"));
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
    fn arms_name_gate_with_or_without_delegation_or_a_model() {
        let all = with_claude_plan();
        assert_eq!(
            start("fix it --against gate:nodelegate").against.as_deref(),
            Some(NO_DELEGATE)
        );
        let request = start("fix it --a sonnet --b opus");
        assert_eq!(request.a.as_deref(), Some("sonnet"));
        assert_eq!(request.against.as_deref(), Some("opus"));
        assert_eq!(
            start("--session current").session,
            Some(SessionPick::Current)
        );
        assert_eq!(
            start("--session=1700000000000").session,
            Some(SessionPick::Started(1_700_000_000_000))
        );
        for query in [NO_DELEGATE, " Gate:NoDelegate "] {
            assert_eq!(
                resolve_arm(query, &all, true),
                Ok(ArmSpec::Gate { delegate: false }),
                "{query}"
            );
        }
        assert_eq!(
            resolve_arm("gate", &all, false),
            Ok(ArmSpec::Gate { delegate: false }),
            "plain gate follows the setting"
        );
        assert_eq!(
            resolve_arm(DELEGATE, &all, false),
            Ok(ArmSpec::Gate { delegate: true })
        );
        assert_eq!(
            resolve_arm("opus", &all, true),
            Ok(ArmSpec::Model {
                model: "claude/claude-opus-5-5".to_string()
            })
        );
        assert!(resolve_arm("llama", &all, true).is_err());
    }

    #[test]
    fn gate_against_gate_without_delegation_always_delegates() {
        let all = with_claude_plan();
        let nodelegate = ArmSpec::Gate { delegate: false };
        assert_eq!(
            resolve_arms(("gate", NO_DELEGATE), &all, false),
            Ok((ArmSpec::Gate { delegate: true }, nodelegate.clone()))
        );
        assert_eq!(
            resolve_arms((NO_DELEGATE, "gate"), &all, false),
            Ok((nodelegate.clone(), ArmSpec::Gate { delegate: true }))
        );
        assert_eq!(
            resolve_arms(("gate", "opus"), &all, false),
            Ok((
                nodelegate,
                ArmSpec::Model {
                    model: "claude/claude-opus-5-5".to_string()
                }
            ))
        );
        assert!(resolve_arms(("opus", "opus"), &all, true)
            .unwrap_err()
            .contains("Both arms run on claude/claude-opus-5-5"));
        assert!(resolve_arms(("gate", "gate"), &all, true).is_err());
    }

    #[test]
    fn the_estimate_prices_every_run_and_judge_call() {
        let estimate = estimate(
            "aura-claude-sonnet-4-6",
            "aura-claude-opus-4-7",
            "aura-claude-opus-4-7",
            3,
            1,
        );
        let one_sonnet = estimate_cost_usd("aura-claude-sonnet-4-6", &REFERENCE_RUN).unwrap();
        let one_opus = estimate_cost_usd("aura-claude-opus-4-7", &REFERENCE_RUN).unwrap();
        let one_judge = estimate_cost_usd("aura-claude-opus-4-7", &REFERENCE_JUDGE).unwrap();
        assert!((estimate.a_usd.unwrap() - 3.0 * one_sonnet).abs() < 1e-9);
        assert!((estimate.b_usd.unwrap() - 3.0 * one_opus).abs() < 1e-9);
        assert!((estimate.judge_usd.unwrap() - 3.0 * one_judge).abs() < 1e-9);
        assert!(estimate.fully_priced());
        assert!(estimate.b_usd > estimate.a_usd);

        let replay = super::estimate(
            "aura-claude-sonnet-4-6",
            "aura-claude-opus-4-7",
            "aura-claude-opus-4-7",
            3,
            4,
        );
        assert!(
            (replay.a_usd.unwrap() - 12.0 * one_sonnet).abs() < 1e-9,
            "each prompt is a turn"
        );
        assert!(
            (replay.judge_usd.unwrap() - 3.0 * one_judge).abs() < 1e-9,
            "but each pair is judged once"
        );

        let local = super::estimate("local/qwen3:4b", "mystery-model", "local/qwen3:4b", 2, 1);
        assert_eq!(local.a_usd, Some(0.0));
        assert_eq!(local.b_usd, None);
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
                (CompareArm::A, 1),
                (CompareArm::B, 1),
                (CompareArm::A, 2),
                (CompareArm::B, 2),
            ]
        );
        assert_eq!(schedule.running(), 4);
        schedule.finished(0.1);
        assert_eq!(drain(&mut schedule), [(CompareArm::A, 3)]);
        schedule.finished(0.1);
        assert_eq!(drain(&mut schedule), [(CompareArm::B, 3)]);
        assert!(drain(&mut schedule).is_empty());
        assert!(schedule.drop_pending().is_empty());
        assert_eq!(run_name(CompareArm::B, 3), "b-3");
    }

    #[test]
    fn the_budget_stops_new_runs_once_spent() {
        let mut schedule = Schedule::new(3, 1, Some(0.25));
        assert_eq!(drain(&mut schedule), [(CompareArm::A, 1)]);
        schedule.finished(0.10);
        assert_eq!(drain(&mut schedule), [(CompareArm::B, 1)]);
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
        assert_eq!(
            rows[0].input_tokens, 32_000,
            "cache reads are prompt tokens"
        );
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
    fn tally_counts_the_whole_prompt_however_the_provider_reports_cache() {
        let anthropic = Usage {
            input_tokens: 10,
            output_tokens: 390,
            cache_creation_input_tokens: Some(9_391),
            cache_read_input_tokens: Some(17_888),
        };
        let openai = Usage {
            input_tokens: 1_000,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(800),
        };
        let rows = tally([
            ("claude-sonnet-5-5".to_string(), anthropic),
            ("gpt-5.4".to_string(), openai),
        ]);
        let row = |model: &str| rows.iter().find(|r| r.model == model).unwrap();
        let sonnet = row("claude-sonnet-5-5");
        assert_eq!(sonnet.input_tokens, 10 + 9_391 + 17_888);
        assert_eq!(
            (sonnet.cache_read_tokens, sonnet.cache_write_tokens),
            (17_888, 9_391)
        );
        let gpt = row("gpt-5.4");
        assert_eq!(gpt.input_tokens, 1_000, "OpenAI counts cache reads inside");
        assert_eq!(gpt.cache_read_tokens, 800);
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
            turns: Vec::new(),
            plan_usage: Vec::new(),
            check_passed: check,
            files_changed: 1,
            error: None,
            task: None,
        }
    }

    fn reading(ms: u64, plan: Plan, window: &str, used: f64, reset: Option<u64>) -> PlanReading {
        PlanReading {
            recorded_ms: ms,
            usage: PlanUsage {
                plan,
                window: window.to_string(),
                used,
                resets_at: reset,
            },
        }
    }

    fn used(shares: &[PlanShare], window: &str) -> f64 {
        shares
            .iter()
            .find(|s| s.window == window)
            .map_or(f64::NAN, |s| s.used)
    }

    #[test]
    fn a_run_alone_on_a_plan_gets_each_rise_after_the_first_reading() {
        let calls = [
            reading(1, Plan::Claude, "5h", 0.10, Some(5_000)),
            reading(1, Plan::Claude, "7d", 0.50, Some(9_000)),
            reading(2, Plan::Claude, "5h", 0.12, Some(5_000)),
            reading(3, Plan::Claude, "5h", 0.15, Some(5_000)),
            reading(3, Plan::Claude, "7d", 0.51, Some(9_000)),
        ];
        let shares = plan_shares(&[&calls]);
        assert_eq!(shares.len(), 1);
        let five = &shares[0][0];
        assert_eq!(
            (five.window.as_str(), five.first, five.last),
            ("5h", 0.10, 0.15)
        );
        assert!((five.used - 0.05).abs() < 1e-9);
        assert!((used(&shares[0], "7d") - 0.01).abs() < 1e-9);
        assert!(plan_shares(&[&[]])[0].is_empty(), "no readings, no shares");
    }

    #[test]
    fn runs_at_the_same_time_split_the_rises_instead_of_each_counting_them() {
        // gate-1 and other-1 work at once on one Claude plan, which goes
        // from 10% to 20% while they run.
        let gate = [
            reading(10, Plan::Claude, "5h", 0.10, Some(5_000)),
            reading(30, Plan::Claude, "5h", 0.14, Some(5_000)),
            reading(50, Plan::Claude, "5h", 0.20, Some(5_000)),
        ];
        let other = [
            reading(20, Plan::Claude, "5h", 0.12, Some(5_000)),
            reading(40, Plan::Claude, "5h", 0.13, Some(5_000)),
            reading(45, Plan::Claude, "5h", 0.17, Some(5_000)),
        ];
        let shares = plan_shares(&[&gate, &other]);
        let (g, o) = (used(&shares[0], "5h"), used(&shares[1], "5h"));
        assert!(
            (g - 0.05).abs() < 1e-9,
            "gate saw 0.14-0.12 and 0.20-0.17: {g}"
        );
        assert!(
            (o - 0.05).abs() < 1e-9,
            "other saw 0.12-0.10 and 0.17-0.14: {o}"
        );
        assert!(
            (g + o - 0.10).abs() < 1e-9,
            "the shares add up to how far the window moved"
        );
        let naive = (0.20 - 0.10) + (0.17 - 0.12);
        assert!(
            g + o < naive,
            "first-to-last per run would count overlap twice"
        );
        assert_eq!((shares[1][0].first, shares[1][0].last), (0.12, 0.17));
    }

    #[test]
    fn late_readings_add_nothing_and_a_restarted_window_counts_from_zero() {
        let calls = [
            reading(1, Plan::Chatgpt, "5h", 0.30, Some(10_000)),
            reading(2, Plan::Chatgpt, "5h", 0.35, Some(10_030)),
            reading(3, Plan::Chatgpt, "5h", 0.33, Some(10_010)),
            reading(4, Plan::Chatgpt, "5h", 0.36, None),
            reading(5, Plan::Chatgpt, "5h", 0.02, Some(28_000)),
            reading(6, Plan::Chatgpt, "5h", 0.04, Some(28_000)),
        ];
        let shares = plan_shares(&[&calls]);
        let share = &shares[0][0];
        assert!(
            (share.used - (0.05 + 0.01 + 0.02 + 0.02)).abs() < 1e-9,
            "{share:?}"
        );
        assert_eq!(share.last, 0.04);
    }

    #[test]
    fn plans_and_windows_are_kept_apart_and_merged_per_arm() {
        let one = [
            reading(1, Plan::Claude, "5h", 0.10, None),
            reading(2, Plan::Claude, "5h", 0.20, None),
        ];
        let two = [
            reading(3, Plan::Chatgpt, "5h", 0.50, None),
            reading(4, Plan::Chatgpt, "5h", 0.60, None),
            reading(5, Plan::Claude, "5h", 0.25, None),
        ];
        let shares = plan_shares(&[&one, &two]);
        assert_eq!(shares[1].len(), 2);
        assert_eq!(shares[1][0].plan, Plan::Claude, "sorted by plan");
        assert!((shares[1][0].used - 0.05).abs() < 1e-9);
        assert!((shares[1][1].used - 0.10).abs() < 1e-9);

        let merged = merge_plan_shares(shares.iter().flatten());
        assert_eq!(merged.len(), 2);
        let claude = &merged[0];
        assert_eq!((claude.first, claude.last), (0.10, 0.25));
        assert!((claude.used - 0.15).abs() < 1e-9);
    }

    fn judged(a_run: &str, winner: Option<CompareArm>, parsed: bool) -> CompareJudgment {
        CompareJudgment {
            a_run: a_run.to_string(),
            b_run: a_run.replacen('a', "b", 1),
            a_first: true,
            winner,
            parsed,
            a_vs_reference: None,
            b_vs_reference: None,
            model: "judge".to_string(),
            cost_usd: 0.01,
            reasoning: String::new(),
        }
    }

    #[test]
    fn summaries_give_medians_pass_counts_and_judge_tallies_per_arm() {
        let mut failed = run(CompareArm::B, 3, 1, 0.5, None);
        failed.error = Some("model fell over".to_string());
        let payload = ComparePayload {
            system_kind: SystemKind::Compare,
            task: "fix it".to_string(),
            plan: None,
            arm_a: ArmSpec::Gate { delegate: true },
            arm_b: ArmSpec::from_label("opus"),
            against: "opus".to_string(),
            prompts: vec!["fix it".to_string()],
            source: aura_core_types::BenchSource::Prompt,
            repeats: 3,
            check: Some("true".to_string()),
            budget_usd: None,
            requested_by: AgentId::generate(),
            apply_root: PathBuf::new(),
            started_ms: 0,
            runs: vec![
                run(CompareArm::A, 1, 30_000, 0.10, Some(true)),
                run(CompareArm::B, 1, 60_000, 0.90, Some(true)),
                run(CompareArm::A, 2, 10_000, 0.20, Some(false)),
                run(CompareArm::B, 2, 50_000, 1.10, Some(true)),
                run(CompareArm::A, 3, 20_000, 0.30, Some(true)),
                failed,
            ],
            judgments: vec![
                judged("a-1", Some(CompareArm::A), true),
                judged("a-2", Some(CompareArm::B), true),
                judged("a-3", None, true),
                judged("a-4", Some(CompareArm::A), false),
            ],
            notes: Vec::new(),
            status: CompareStatus::Done,
        };

        let gate = summarize(&payload, CompareArm::A);
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
        assert!(gate.plan_usage.is_empty());

        let other = summarize(&payload, CompareArm::B);
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
        assert_eq!(
            (gate.ref_better, gate.ref_as_good, gate.ref_worse),
            (0, 0, 0)
        );
    }

    fn turn(cost: f64, signal: OutcomeSignal) -> RunTurn {
        RunTurn {
            model: "m".to_string(),
            cost_usd: cost,
            wall_ms: 1,
            signal,
        }
    }

    #[test]
    fn replays_show_each_prompts_cost_and_outcome_per_arm_and_the_reference_tally() {
        let mut a1 = run(CompareArm::A, 1, 1, 0.3, None);
        a1.turns = vec![
            turn(0.1, OutcomeSignal::ChecksPassed),
            turn(0.2, OutcomeSignal::ChecksFailed),
        ];
        let mut a2 = run(CompareArm::A, 2, 1, 0.5, None);
        a2.turns = vec![
            turn(0.3, OutcomeSignal::ChecksPassed),
            turn(0.2, OutcomeSignal::Error),
        ];
        let mut b1 = run(CompareArm::B, 1, 1, 0.9, None);
        b1.turns = vec![turn(0.9, OutcomeSignal::None)];
        let mut first = judged("a-1", Some(CompareArm::A), true);
        first.a_vs_reference = Some(RefVerdict::AsGood);
        first.b_vs_reference = Some(RefVerdict::Worse);
        let mut second = judged("a-2", None, true);
        second.a_vs_reference = Some(RefVerdict::Better);
        let payload = ComparePayload {
            system_kind: SystemKind::Compare,
            task: "one".to_string(),
            plan: None,
            arm_a: ArmSpec::from_label("sonnet"),
            arm_b: ArmSpec::from_label("opus"),
            against: "opus".to_string(),
            prompts: vec!["one".to_string(), "two".to_string()],
            source: aura_core_types::BenchSource::Prompt,
            repeats: 2,
            check: None,
            budget_usd: None,
            requested_by: AgentId::generate(),
            apply_root: PathBuf::new(),
            started_ms: 0,
            runs: vec![a1, b1, a2],
            judgments: vec![first, second],
            notes: Vec::new(),
            status: CompareStatus::Done,
        };

        let a = turn_cells(&payload, CompareArm::A);
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].runs, a[0].passed, a[0].failed), (2, 2, 0));
        assert!((a[0].mean_cost_usd.unwrap() - 0.2).abs() < 1e-9);
        assert_eq!((a[1].passed, a[1].failed, a[1].errors), (0, 1, 1));
        let b = turn_cells(&payload, CompareArm::B);
        assert_eq!(b[0].runs, 1);
        assert_eq!(b[1].runs, 0, "b-1 stopped after the first prompt");
        assert_eq!(b[1].mean_cost_usd, None);

        let a = summarize(&payload, CompareArm::A);
        assert_eq!((a.ref_better, a.ref_as_good, a.ref_worse), (1, 1, 0));
        let b = summarize(&payload, CompareArm::B);
        assert_eq!((b.ref_better, b.ref_as_good, b.ref_worse), (0, 0, 1));
    }

    #[test]
    fn example_tasks_count_passes_per_task_and_arm() {
        let on = |mut run: CompareRun, task: &str| {
            run.task = Some(task.to_string());
            run
        };
        let task = |id: &str| aura_core_types::ExampleTaskInfo {
            id: id.to_string(),
            ..aura_core_types::ExampleTaskInfo::default()
        };
        let payload = ComparePayload {
            system_kind: SystemKind::Compare,
            task: "2 example coding tasks".to_string(),
            plan: None,
            arm_a: ArmSpec::from_label("sonnet"),
            arm_b: ArmSpec::from_label("opus"),
            against: "opus".to_string(),
            prompts: Vec::new(),
            source: aura_core_types::BenchSource::Examples {
                tasks: vec![task("coding/one"), task("coding/two")],
            },
            repeats: 2,
            check: None,
            budget_usd: None,
            requested_by: AgentId::generate(),
            apply_root: PathBuf::new(),
            started_ms: 0,
            runs: vec![
                on(run(CompareArm::A, 1, 1, 0.2, Some(true)), "coding/one"),
                on(run(CompareArm::B, 1, 1, 0.8, Some(true)), "coding/one"),
                on(run(CompareArm::A, 2, 1, 0.4, Some(false)), "coding/two"),
                on(run(CompareArm::A, 3, 1, 0.4, Some(true)), "coding/one"),
            ],
            judgments: Vec::new(),
            notes: Vec::new(),
            status: CompareStatus::Done,
        };
        let a = task_cells(&payload, CompareArm::A);
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].runs, a[0].passed, a[0].checked), (2, 2, 2));
        assert!((a[0].mean_cost_usd.unwrap() - 0.3).abs() < 1e-9);
        assert_eq!((a[1].runs, a[1].passed, a[1].checked), (1, 0, 1));
        let b = task_cells(&payload, CompareArm::B);
        assert_eq!((b[1].runs, b[1].mean_cost_usd), (0, None));
        let prompt = ComparePayload {
            source: aura_core_types::BenchSource::Prompt,
            ..payload
        };
        assert!(task_cells(&prompt, CompareArm::A).is_empty());
    }
}
