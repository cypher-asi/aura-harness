//! `aura gate replay`: ask Gate's reviewer about attempts that are
//! already in the attempt log, so the review can be measured against
//! what the tasks' own checks proved before it is trusted with live
//! turns.
//!
//! An attempt can be replayed when Gate chose its model, the model is
//! below the strongest tier, Gate let the work stand, a benchmark task's
//! check then proved it right or wrong, its run's copy of the workspace
//! is still kept, and its session was saved. The reviewer is shown what
//! a live review would be shown: the request, everything the run changed
//! (the copy against its baseline), the commands and checks from the
//! saved session, and the final reply.
//!
//! With `--criteria` the work is also held to acceptance criteria, as a
//! live turn's is ([`aura_gate::judge::criteria`]): a writer lists them
//! from each request, once per request and writer (they are kept beside
//! the log, so every reviewer is asked about the same list), and the
//! reviewer goes through them.
//!
//! Each answer is appended to the log as a `review` line marked
//! `replayed`, which `aura gate eval` scores. The attempt's line is
//! written again with the files the run changed by any means and those
//! it wrote, so the signs that need them (`shell_edit`,
//! `no_test_changed`, `own_tests_only`) are scored too. Nothing else is
//! touched: no record, no table, no copy.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use aura_core_types::{AgentId, Difficulty, Hash};
use aura_gate::attempts::{
    self, below_strongest, Attempt, AttemptLog, CopyAt, Line, Review, Stood, Tier, Tiers,
    TruthSource,
};
use aura_gate::evidence::transcript::reply::final_reply;
use aura_gate::evidence::{flags, Evidence, Flag};
use aura_gate::followup::Why;
use aura_gate::judge::criteria::{self, Criteria};
use aura_gate::judge::turn::{self, shown_to_reviewer, Answer, ReviewVariant, Standing};
use aura_model::model_catalog::{estimate_cost_usd, infer_tier, route_label, ModelTier};
use aura_model::{Message, ModelProvider, StopReason, Usage};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::event_loop::processes::now_ms;
use crate::event_loop::tasks::TASKS_DIR;
use crate::isolation::changes;
use crate::session_checkpoint::Checkpoints;
use aura_gate::live::sent_text;

/// How long one review may take.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(420);

/// How long writing one request's criteria may take.
const CRITERIA_TIMEOUT: Duration = Duration::from_secs(120);

/// Where the criteria written for replays are kept, under the data
/// directory.
const CRITERIA_FILE: &str = "gate/replay_criteria.json";

/// What writing one request's criteria is assumed to cost when
/// estimating, in USD.
const ESTIMATED_CRITERIA_USD: f64 = 0.03;

/// Characters per token assumed when estimating what the reviews cost.
const ESTIMATE_CHARS_PER_TOKEN: f64 = 3.2;

/// Output tokens assumed per review when estimating: plain, thinking.
const ESTIMATED_OUTPUT: [u64; 2] = [600, 6_000];

/// What is said of a review whose reply names no verdict.
const NO_VERDICT: &str = "the reviewer gave no verdict";

/// Why an attempt is reviewed when nothing else says: it is being
/// replayed.
const REPLAY_REASON: &str = "the work is reviewed before it is accepted";

/// What `aura gate replay` is asked to do.
#[derive(Debug, Clone)]
pub struct ReplayOptions {
    /// The data directory whose attempt log and saved sessions are read
    pub data_dir: PathBuf,
    /// Only attempts at benchmark tasks whose id contains this
    pub task: Option<String>,
    /// At most this many attempts
    pub limit: Option<usize>,
    /// Ask the reviewer to reason before it answers
    pub thinking: bool,
    /// Attempts of any difficulty, not only those rated hard
    pub any_difficulty: bool,
    /// The model that reviews; by default the strongest model in the log
    pub judge: Option<String>,
    /// Say what would be asked and what it may cost, then stop
    pub dry_run: bool,
    /// Ask again about attempts already replayed the same way
    pub again: bool,
    /// Reviews asked at a time
    pub jobs: usize,
    /// Hold the work to acceptance criteria written from its request
    pub criteria: bool,
    /// The model that writes them; by default the latest standard-tier
    /// model in the log
    pub writer: Option<String>,
}

/// An attempt picked for replay.
#[derive(Debug, Clone, Copy)]
struct Pick<'a> {
    stood: Stood<'a>,
    copy: &'a CopyAt,
}

/// The attempts in `lines` to replay as `options` say, oldest first, and
/// how many were left out for each reason. `done` holds the agent and
/// prompt of the attempts already replayed the same way.
fn pick<'a>(
    lines: &'a [Line],
    options: &ReplayOptions,
    done: &HashSet<(&str, &str)>,
) -> (Vec<Pick<'a>>, BTreeMap<&'static str, usize>) {
    let mut left_out: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut picks = Vec::new();
    for stood in attempts::stood(lines) {
        let (attempt, truth) = (stood.attempt, stood.truth);
        let wanted = options.task.as_deref().is_none_or(|wanted| {
            truth
                .task
                .as_deref()
                .is_some_and(|task| task.contains(wanted))
        });
        if !wanted {
            continue;
        }
        let why_not = if truth.source != TruthSource::TaskCheck {
            Some("no benchmark task's check proved them")
        } else if !attempt.routed {
            Some("Gate did not choose their model")
        } else if !below_strongest(&attempt.model) {
            Some("on the strongest tier")
        } else if !options.any_difficulty && attempt.difficulty != Some(Difficulty::Hard) {
            Some("not rated hard (--all takes them)")
        } else if stood.attempts_by_agent > 1 {
            Some("their run moved between models or prompts")
        } else if !options.again
            && done.contains(&(attempt.agent.as_str(), attempt.prompt_tx.as_str()))
        {
            Some("already replayed this way (--again asks anew)")
        } else {
            None
        };
        if let Some(why_not) = why_not {
            *left_out.entry(why_not).or_default() += 1;
            continue;
        }
        match &truth.copy {
            Some(copy) if copy.root.is_dir() => picks.push(Pick { stood, copy }),
            Some(_) => *left_out.entry("their run's copy is not kept").or_default() += 1,
            None => {
                *left_out
                    .entry(
                        "the log does not say where their run's copy is (starting aura notes it)",
                    )
                    .or_default() += 1;
            }
        }
    }
    if let Some(limit) = options.limit {
        picks.truncate(limit);
    }
    (picks, left_out)
}

/// An attempt as the reviewer is shown it.
#[derive(Debug, Clone)]
struct Prepared {
    /// The attempt's line, with the evidence as it is known now
    attempt: Attempt,
    /// `Compare #22 a-4`
    run: String,
    /// The benchmark task
    task_id: String,
    /// Whether the task's check passed
    right: bool,
    /// What the user asked
    task: String,
    /// Everything the run changed
    diff: String,
    commands: String,
    checks: String,
    reply: String,
    /// Why it is reviewed, as logged and as the reviewer is told
    why: Vec<(String, String)>,
    /// Every sign its evidence raises
    signs: Vec<Flag>,
    /// What the work is held to, when the replay holds it to criteria
    criteria: Option<Arc<Criteria>>,
}

impl Prepared {
    fn standing<'a>(&'a self, why: &'a [String]) -> Standing<'a> {
        Standing {
            task: &self.task,
            diff: &self.diff,
            whole: true,
            commands: &self.commands,
            checks: &self.checks,
            reply: &self.reply,
            why,
            criteria: self.criteria.as_deref(),
        }
    }

    /// The same attempt held to `criteria`.
    fn held_to(mut self, criteria: Arc<Criteria>) -> Self {
        let why = Why::Criteria;
        self.why
            .retain(|(name, _)| name != "replay" && name != why.name());
        self.why
            .push((why.name().to_string(), why.describe().to_string()));
        self.criteria = Some(criteria);
        self
    }

    /// What the reviewer is told of why it reviews.
    fn why_told(&self) -> Vec<String> {
        self.why.iter().map(|(_, told)| told.clone()).collect()
    }
}

/// Why the attempt of `pick`, which raises `signs`, would be reviewed
/// with the tiers `tiers`: as logged, and as the reviewer is told.
fn reasons(attempt: &Attempt, signs: &[Flag], tiers: &Tiers) -> Vec<(String, String)> {
    let mut why: Vec<Why> = Vec::new();
    if attempt.difficulty == Some(Difficulty::Hard) {
        why.push(Why::Hard);
    }
    why.extend(
        signs
            .iter()
            .filter(|sign| tiers.of(**sign) != Tier::Log)
            .map(|sign| Why::Sign(*sign)),
    );
    if why.is_empty() {
        return vec![("replay".to_string(), REPLAY_REASON.to_string())];
    }
    why.iter()
        .map(|why| (why.name().to_string(), why.describe().to_string()))
        .collect()
}

/// Read what the attempt of `pick` left: its saved session in
/// `sessions` and what its run changed in the kept copy.
fn prepare(pick: &Pick<'_>, sessions: &Checkpoints, tiers: &Tiers) -> Result<Prepared, String> {
    let (attempt, truth) = (pick.stood.attempt, pick.stood.truth);
    let agent = AgentId::from_hex(&attempt.agent).map_err(|e| format!("its agent id: {e}"))?;
    let saved = sessions
        .load_current(agent)
        .ok_or("its session is not saved")?;
    let start = saved
        .messages
        .iter()
        .position(|message| sent_text(message).is_some())
        .ok_or("its saved session holds no prompt")?;
    let messages: &[Message] = &saved.messages[start..];
    let task = sent_text(&messages[0]).unwrap_or_default();
    let copy = pick.copy;
    let diff = changes::readable_diff_since(
        &copy.root,
        copy.git_dir.as_deref(),
        &copy.baseline,
        &[TASKS_DIR],
    )
    .map_err(|e| format!("its copy could not be compared with its baseline: {e}"))?;
    let mut evidence = Evidence::of(messages);
    evidence.hit_limit = attempt.evidence.hit_limit;
    evidence.note_changed(&changes::paths_in(&diff));
    let signs = flags(&evidence, attempt.category);
    let why = reasons(attempt, &signs, tiers);
    let run = [truth.compare.as_deref(), truth.run.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let (commands, checks) = shown_to_reviewer(messages);
    Ok(Prepared {
        attempt: Attempt {
            evidence,
            ..attempt.clone()
        },
        run,
        task_id: truth.task.clone().unwrap_or_default(),
        right: truth.passed,
        task,
        diff,
        commands,
        checks,
        reply: final_reply(messages),
        why,
        signs,
        criteria: None,
    })
}

/// How a review asked as `variant` is logged: `plain`, or
/// `plain+criteria` when the work is held to criteria, as a live review
/// is.
fn asked_as(variant: ReviewVariant, criteria: bool) -> String {
    if criteria {
        format!("{}+criteria", variant.name())
    } else {
        variant.name().to_string()
    }
}

/// The latest standard-tier model the attempts in `lines` ran on: the
/// model that writes the criteria when none is named.
fn writer_in(lines: &[Line]) -> Option<String> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Attempt(attempt) => Some(attempt.model.as_str()),
            _ => None,
        })
        .filter(|model| infer_tier(model) == ModelTier::Standard)
        .max()
        .map(str::to_string)
}

/// The criteria written for replays, by writer and request.
#[derive(Debug, Default)]
struct Written {
    path: PathBuf,
    kept: BTreeMap<String, Criteria>,
}

impl Written {
    /// Those kept under `data_dir`; none when the file is missing or
    /// unreadable.
    fn in_data_dir(data_dir: &Path) -> Self {
        let path = data_dir.join(CRITERIA_FILE);
        let kept = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, kept }
    }

    fn key(writer: &str, task: &str) -> String {
        format!("{writer} {}", Hash::from_content(task.as_bytes()).to_hex())
    }

    fn get(&self, writer: &str, task: &str) -> Option<Arc<Criteria>> {
        self.kept
            .get(&Self::key(writer, task))
            .map(|found| Arc::new(found.clone()))
    }

    fn keep(&mut self, writer: &str, task: &str, criteria: &Criteria) -> anyhow::Result<()> {
        self.kept.insert(Self::key(writer, task), criteria.clone());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.kept)?)
            .with_context(|| format!("writing {}", self.path.display()))
    }
}

/// Ask `writer` through `provider` for the criteria of `task`, and what
/// the call cost.
///
/// # Errors
/// Says why when the call failed or its reply held none, with what the
/// reply cost.
async fn write_criteria(
    provider: &Arc<dyn ModelProvider + Send + Sync>,
    writer: &str,
    task: &str,
    auth_token: Option<String>,
) -> Result<(Criteria, f64), (String, f64)> {
    let request = criteria::request(writer, task, auth_token)
        .map_err(|e| (format!("could not build the request: {e}"), 0.0))?;
    let response = match tokio::time::timeout(CRITERIA_TIMEOUT, provider.complete(request)).await {
        Err(_) => return Err(("the writer took too long".to_string(), 0.0)),
        Ok(Err(e)) => return Err((format!("the writer's call failed: {e}"), 0.0)),
        Ok(Ok(response)) => response,
    };
    let cost_usd = estimate_cost_usd(writer, &response.usage).unwrap_or(0.0);
    match criteria::criteria_of(&response) {
        Some(found) => Ok((found, cost_usd)),
        None => Err((criteria::none_because(&response.message), cost_usd)),
    }
}

/// Hold each of `prepared` to the criteria of its request, written by
/// `writer` through `provider` unless `written` already holds them.
/// Attempts whose request got none are left out, with a line saying why.
/// Returns the attempts and what the writing cost.
async fn hold_to_criteria(
    prepared: Vec<Prepared>,
    written: &mut Written,
    writer: &str,
    (provider, auth_token): (&Arc<dyn ModelProvider + Send + Sync>, Option<String>),
) -> anyhow::Result<(Vec<Prepared>, f64)> {
    let mut cost_usd = 0.0;
    let mut failed: BTreeMap<String, String> = BTreeMap::new();
    let mut held = Vec::with_capacity(prepared.len());
    for ready in prepared {
        if let Some(why) = failed.get(&ready.task) {
            println!("  left out {} {}: {why}", ready.run, ready.task_id);
            continue;
        }
        let found = match written.get(writer, &ready.task) {
            Some(found) => found,
            None => match write_criteria(provider, writer, &ready.task, auth_token.clone()).await {
                Ok((found, cost)) => {
                    cost_usd += cost;
                    written.keep(writer, &ready.task, &found)?;
                    println!(
                        "  {:40} {} criteria{} (${cost:.3})",
                        ready.task_id,
                        found.items.len(),
                        if found.strict { ", strict" } else { "" },
                    );
                    Arc::new(found)
                }
                Err((problem, cost)) => {
                    cost_usd += cost;
                    let why = format!(
                        "{} wrote no criteria for its request: {problem}",
                        route_label(writer)
                    );
                    println!("  left out {} {}: {why}", ready.run, ready.task_id);
                    failed.insert(ready.task.clone(), why);
                    continue;
                }
            },
        };
        held.push(ready.held_to(found));
    }
    Ok((held, cost_usd))
}

/// The strongest model the attempts in `lines` ran on, an Opus when one
/// is among them: the model that reviews when none is named.
fn strongest_in(lines: &[Line]) -> Option<String> {
    let mut models: Vec<&str> = lines
        .iter()
        .filter_map(|line| match line {
            Line::Attempt(attempt) => Some(attempt.model.as_str()),
            _ => None,
        })
        .filter(|model| infer_tier(model) == ModelTier::Frontier)
        .collect();
    models.sort_unstable();
    models.dedup();
    models
        .iter()
        .rev()
        .find(|model| model.contains("opus"))
        .or(models.last())
        .map(|model| (*model).to_string())
}

/// What asking `judge` about `prepared` as `variant` may cost at list
/// price; `None` when the model has no list price.
fn estimate(judge: &str, prepared: &[Prepared], variant: ReviewVariant) -> Option<f64> {
    let output = ESTIMATED_OUTPUT[usize::from(variant == ReviewVariant::Thinking)];
    prepared
        .iter()
        .map(|prepared| {
            let why = prepared.why_told();
            let chars = turn::REVIEW_SYSTEM_PROMPT.len()
                + turn::review_prompt(&prepared.standing(&why)).len();
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let input = (chars as f64 / ESTIMATE_CHARS_PER_TOKEN) as u64;
            estimate_cost_usd(judge, &Usage::new(input, output))
        })
        .sum()
}

/// What one review came to.
#[derive(Debug)]
struct Asked {
    run: String,
    task_id: String,
    model: String,
    right: bool,
    /// The answer, what was found and what it cost; the problem when the
    /// reviewer gave none
    answer: Result<(Answer, String, f64), String>,
    /// What a reply without a verdict cost
    wasted_usd: f64,
    /// How many of its criteria the reviewer found met, when it was held
    /// to any
    criteria: String,
}

/// Ask `judge` through `provider` about `prepared` as `variant`, and
/// append its answer to `log`.
async fn ask(
    provider: Arc<dyn ModelProvider + Send + Sync>,
    judge: String,
    variant: ReviewVariant,
    auth_token: Option<String>,
    log: Arc<AttemptLog>,
    prepared: Prepared,
) -> Asked {
    let why = prepared.why_told();
    let request = turn::review_request(&judge, &prepared.standing(&why), variant, auth_token);
    let mut wasted_usd = 0.0;
    let answer = match request {
        Err(e) => Err(format!("could not build the request: {e}")),
        Ok(request) => match tokio::time::timeout(REVIEW_TIMEOUT, provider.complete(request)).await
        {
            Err(_) => Err("the reviewer took too long".to_string()),
            Ok(Err(e)) => Err(format!("the reviewer's call failed: {e}")),
            Ok(Ok(response)) => {
                let cost_usd = estimate_cost_usd(&judge, &response.usage).unwrap_or(0.0);
                let parsed = turn::parse_answer(&response.message.text_content());
                if parsed.is_none() {
                    wasted_usd = cost_usd;
                }
                parsed
                    .map(|(answer, findings)| (answer, findings, cost_usd))
                    .ok_or_else(|| {
                        let cut = if response.stop_reason == StopReason::MaxTokens {
                            "its reply was cut off at the token limit"
                        } else {
                            "its reply ended without one"
                        };
                        format!(
                            "{NO_VERDICT}: {cut} after {} tokens, ${cost_usd:.3}",
                            response.usage.output_tokens
                        )
                    })
            }
        },
    };
    if let Ok((verdict, findings, cost_usd)) = &answer {
        let line = Line::Review(Review {
            at_ms: now_ms(),
            agent: prepared.attempt.agent.clone(),
            prompt_tx: prepared.attempt.prompt_tx.clone(),
            why: prepared.why.iter().map(|(name, _)| name.clone()).collect(),
            signs: prepared.signs.clone(),
            verdict: *verdict,
            findings: findings.clone(),
            judge: judge.clone(),
            cost_usd: *cost_usd,
            variant: asked_as(variant, prepared.criteria.is_some()),
            criteria: prepared
                .criteria
                .as_deref()
                .map(|held| criteria::checked(held, findings))
                .unwrap_or_default(),
            acted: false,
            replayed: true,
        });
        if let Err(e) = log.append(&line) {
            return Asked {
                answer: Err(format!("could not write the attempt log: {e}")),
                ..asked(&prepared)
            };
        }
    }
    let criteria = match (&answer, prepared.criteria.as_deref()) {
        (Ok((_, findings, _)), Some(held)) => criteria::tally(&criteria::checked(held, findings)),
        _ => String::new(),
    };
    Asked {
        answer,
        wasted_usd,
        criteria,
        ..asked(&prepared)
    }
}

fn asked(prepared: &Prepared) -> Asked {
    Asked {
        run: prepared.run.clone(),
        task_id: prepared.task_id.clone(),
        model: route_label(&prepared.attempt.model),
        right: prepared.right,
        answer: Err(String::new()),
        wasted_usd: 0.0,
        criteria: String::new(),
    }
}

/// One line saying how a review went, then what it found when it found
/// the work unfinished.
fn row(asked: &Asked) -> String {
    let proved = if asked.right { "right" } else { "wrong" };
    let head = format!(
        "  {:16} {:40} {:32} {proved:5}",
        asked.run, asked.task_id, asked.model
    );
    match &asked.answer {
        Ok((answer, findings, cost_usd)) => {
            let said = match answer {
                Answer::Model => "unfinished",
                Answer::Ok => "done",
                Answer::Other => "held up",
            };
            let mut line = format!("{head}  {said:10}  ${cost_usd:.3}{}", asked.criteria);
            if *answer == Answer::Model {
                for finding in findings.lines().filter(|l| !l.trim().is_empty()).take(6) {
                    line.push_str("\n      ");
                    line.push_str(finding.trim_end());
                }
            }
            line
        }
        Err(problem) => format!("{head}  no answer: {problem}"),
    }
}

/// How the reviews in `asked` held up, in a few lines.
fn tally(asked: &[Asked]) -> String {
    let answered: Vec<(&Asked, Answer, f64)> = asked
        .iter()
        .filter_map(|asked| {
            let (answer, _, cost_usd) = asked.answer.as_ref().ok()?;
            Some((asked, *answer, *cost_usd))
        })
        .collect();
    let count = |right: bool, unfinished: Option<bool>| {
        answered
            .iter()
            .filter(|(asked, answer, _)| {
                asked.right == right
                    && unfinished.is_none_or(|unfinished| (*answer == Answer::Model) == unfinished)
            })
            .count()
    };
    let cost: f64 = answered
        .iter()
        .map(|(_, _, cost_usd)| cost_usd)
        .sum::<f64>()
        + asked.iter().map(|asked| asked.wasted_usd).sum::<f64>();
    format!(
        "{} reviews answered, {} not, ${cost:.2} at list price\n\
         wrong work: {} of {} found unfinished\n\
         right work: {} of {} found unfinished",
        answered.len(),
        asked.len() - answered.len(),
        count(false, Some(true)),
        count(false, None),
        count(true, Some(true)),
        count(true, None),
    )
}

/// The provider `aura` reaches models through with the user's sign-ins,
/// and the token sent along where a route needs one.
fn provider() -> anyhow::Result<(Arc<dyn ModelProvider + Send + Sync>, Option<String>)> {
    let auth = Arc::new(aura_auth::AuthManager::new().context("loading sign-in state")?);
    let selection = crate::harness::build_provider("anthropic", &auth)?;
    let auth_token =
        crate::session_helpers::default_agent_config(aura_model::ENV_FALLBACK_MODEL).auth_token;
    Ok((selection.provider, auth_token))
}

/// Run `aura gate replay` as `options` say, printing what it does.
///
/// # Errors
/// When the attempt log or the sign-in state cannot be read, or no model
/// to review with (or to write the criteria with) is named or found in
/// the log.
#[allow(clippy::too_many_lines)]
pub async fn gate_replay(options: ReplayOptions) -> anyhow::Result<()> {
    let log = Arc::new(AttemptLog::in_data_dir(&options.data_dir));
    let read = log
        .read()
        .with_context(|| format!("reading {}", log.path().display()))?;
    let variant = if options.thinking {
        ReviewVariant::Thinking
    } else {
        ReviewVariant::Plain
    };
    let asked_as = asked_as(variant, options.criteria);
    let judge = options.judge.clone().or_else(|| strongest_in(&read.lines));
    // Asked the same way: by the same reviewer, held to criteria or not.
    let done: HashSet<(&str, &str)> = read
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Review(review)
                if review.replayed
                    && review.variant == asked_as
                    && Some(&review.judge) == judge.as_ref() =>
            {
                Some((review.agent.as_str(), review.prompt_tx.as_str()))
            }
            _ => None,
        })
        .collect();
    let (picks, left_out) = pick(&read.lines, &options, &done);
    println!("{}", log.path().display());
    println!("{} attempts to replay", picks.len());
    for (why, count) in &left_out {
        println!("  {count} left out: {why}");
    }
    if picks.is_empty() {
        return Ok(());
    }
    let judge = judge.context("no model to review with: name one with --judge")?;
    let writer = if options.criteria {
        let writer = options.writer.clone().or_else(|| writer_in(&read.lines));
        Some(writer.context("no model to write the criteria with: name one with --writer")?)
    } else {
        None
    };

    let tiers = attempts::tiers(&read.lines);
    let sessions = Checkpoints::new(&options.data_dir);
    let mut prepared = Vec::with_capacity(picks.len());
    for pick in &picks {
        match prepare(pick, &sessions, &tiers) {
            Ok(ready) => prepared.push(ready),
            Err(problem) => println!(
                "  left out {} {}: {problem}",
                pick.stood.truth.run.as_deref().unwrap_or_default(),
                pick.stood.truth.task.as_deref().unwrap_or_default()
            ),
        }
    }
    let wrong = prepared.iter().filter(|p| !p.right).count();
    println!(
        "asking {} ({asked_as}) about {} attempts, {wrong} of them wrong{}",
        route_label(&judge),
        prepared.len(),
        estimate(&judge, &prepared, variant).map_or_else(String::new, |usd| format!(
            ": about ${usd:.2} at list price"
        )),
    );
    let mut written = Written::in_data_dir(&options.data_dir);
    if let Some(writer) = &writer {
        let requests: HashSet<&str> = prepared.iter().map(|p| p.task.as_str()).collect();
        let to_write = requests
            .iter()
            .filter(|task| written.get(writer, task).is_none())
            .count();
        #[allow(clippy::cast_precision_loss)]
        let about = to_write as f64 * ESTIMATED_CRITERIA_USD;
        println!(
            "held to the criteria of {} requests: {to_write} to write with {} (about ${about:.2}), \
             the rest kept in {}; reviews held to criteria cost somewhat more than estimated",
            requests.len(),
            route_label(writer),
            written.path.display(),
        );
    }
    if options.dry_run {
        for ready in &prepared {
            let signs: Vec<&str> = ready.signs.iter().map(|sign| sign.name()).collect();
            println!(
                "  {:16} {:40} {:32} {:5}  {} files, {} chars of diff  {}",
                ready.run,
                ready.task_id,
                route_label(&ready.attempt.model),
                if ready.right { "right" } else { "wrong" },
                ready.attempt.evidence.files_changed.unwrap_or_default(),
                ready.diff.len(),
                signs.join(" ")
            );
        }
        return Ok(());
    }

    // The attempts' lines again, with what their runs changed.
    for (ready, pick) in prepared.iter().zip(&picks) {
        if ready.attempt.evidence != pick.stood.attempt.evidence {
            log.append(&Line::Attempt(ready.attempt.clone()))
                .with_context(|| format!("writing {}", log.path().display()))?;
        }
    }

    let (provider, auth_token) = provider()?;
    let mut criteria_usd = 0.0;
    if let Some(writer) = &writer {
        let by = (&provider, auth_token.clone());
        (prepared, criteria_usd) = hold_to_criteria(prepared, &mut written, writer, by).await?;
    }
    // The first review goes alone: when the reviewer cannot be reached
    // (not signed in, no network), the rest would only wait as long.
    let mut prepared = prepared.into_iter();
    let mut all = Vec::new();
    if let Some(first) = prepared.next() {
        let asked = ask(
            Arc::clone(&provider),
            judge.clone(),
            variant,
            auth_token.clone(),
            Arc::clone(&log),
            first,
        )
        .await;
        println!("{}", row(&asked));
        if let Err(problem) = &asked.answer {
            if !problem.starts_with(NO_VERDICT) {
                anyhow::bail!(
                    "{} could not be asked ({problem}); no other attempt was asked about",
                    route_label(&judge)
                );
            }
        }
        all.push(asked);
    }
    let jobs = Arc::new(Semaphore::new(options.jobs.max(1)));
    let mut asking = JoinSet::new();
    for ready in prepared {
        let (provider, judge, auth_token, log, jobs) = (
            Arc::clone(&provider),
            judge.clone(),
            auth_token.clone(),
            Arc::clone(&log),
            Arc::clone(&jobs),
        );
        asking.spawn(async move {
            let _permit = jobs.acquire_owned().await;
            ask(provider, judge, variant, auth_token, log, ready).await
        });
    }
    while let Some(joined) = asking.join_next().await {
        let asked = joined.context("a review stopped unexpectedly")?;
        println!("{}", row(&asked));
        all.push(asked);
    }
    println!("{}", tally(&all));
    if let Some(writer) = &writer {
        println!(
            "writing the criteria cost ${criteria_usd:.2} on {}",
            route_label(writer)
        );
    }
    println!("`aura gate eval` scores every review in the log.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{Hash, OutcomePayload, OutcomeSignal, PromptCategory};
    use aura_gate::attempts::{Terms, Truth};
    use aura_model::{ContentBlock, Role, ToolResultContent};
    use serde_json::json;
    use std::path::Path;
    use tempfile::TempDir;

    const SONNET: &str = "claude/claude-sonnet-5-5";
    const OPUS: &str = "claude/claude-opus-5-5";

    fn options(data_dir: &Path) -> ReplayOptions {
        ReplayOptions {
            data_dir: data_dir.to_path_buf(),
            task: None,
            limit: None,
            thinking: false,
            any_difficulty: false,
            judge: None,
            dry_run: false,
            again: false,
            jobs: 2,
            criteria: false,
            writer: None,
        }
    }

    fn attempt(agent: AgentId, n: u8, model: &str, difficulty: Option<Difficulty>) -> Attempt {
        let ended = OutcomePayload {
            prompt_tx: Hash::from_content(&[n]),
            category: PromptCategory::Debugging,
            model: model.to_string(),
            success: Some(true),
            signal: OutcomeSignal::ChecksPassed,
            cost_usd: 0.1,
            subtask_cost_usd: 0.0,
            latency_ms: 10,
        };
        let terms = Terms {
            difficulty,
            led: Some(false),
            routed: true,
        };
        let mut attempt = Attempt::new(1, agent.to_hex(), &ended, terms, Evidence::default());
        attempt.backfilled = true;
        attempt
    }

    fn truth(attempt: &Attempt, passed: bool, copy: Option<&Path>) -> Truth {
        Truth {
            at_ms: 2,
            agent: attempt.agent.clone(),
            prompt_tx: attempt.prompt_tx.clone(),
            source: TruthSource::TaskCheck,
            passed,
            compare: Some("Compare #22".to_string()),
            run: Some("a-6".to_string()),
            task: Some("ansible-d72025be".to_string()),
            copy: copy.map(|root| CopyAt {
                root: root.join("a-6"),
                git_dir: Some(root.join("a-6.git")),
                baseline: String::new(),
            }),
        }
    }

    /// A kept copy under `root` whose run changed `lib.py` with a shell
    /// command, and the place it is kept at.
    fn kept_copy(root: &Path) -> Option<CopyAt> {
        let copy = root.join("a-6");
        let git_dir = root.join("a-6.git");
        std::fs::create_dir_all(copy.join("test")).unwrap();
        std::fs::write(copy.join("lib.py"), "def f():\n    return 1\n").unwrap();
        std::fs::write(copy.join("test/test_lib.py"), "def test_f(): pass\n").unwrap();
        let baseline = changes::baseline(&copy, Some(&git_dir)).ok()?;
        std::fs::write(copy.join("lib.py"), "def f():\n    return 2\n").unwrap();
        Some(CopyAt {
            root: copy,
            git_dir: Some(git_dir),
            baseline,
        })
    }

    fn session() -> Vec<Message> {
        let call = |id: &str, command: &str| {
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: "run_command".to_string(),
                    input: json!({ "command": command }),
                }],
            )
        };
        let result = |id: &str, output: &str| {
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: ToolResultContent::Text(output.to_string()),
                    is_error: false,
                }],
            )
        };
        vec![
            Message::user("Make f() return 2."),
            call("1", "sed -i 's/return 1/return 2/' lib.py"),
            result("1", ""),
            call("2", "python -m pytest -q test"),
            result("2", "1 passed in 0.01s"),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "Done; the tests pass.".to_string(),
                }],
            ),
        ]
    }

    #[test]
    fn only_standing_hard_routed_work_with_a_kept_copy_is_picked() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("a-6")).unwrap();
        let agents: Vec<AgentId> = (0..7).map(|_| AgentId::generate()).collect();
        let hard = Some(Difficulty::Hard);
        let picked = attempt(agents[0], 0, SONNET, hard);
        let easy = attempt(agents[1], 1, SONNET, Some(Difficulty::Easy));
        let on_opus = attempt(agents[2], 2, OPUS, hard);
        let pinned = Attempt {
            routed: false,
            ..attempt(agents[3], 3, SONNET, hard)
        };
        let no_copy = attempt(agents[4], 4, SONNET, hard);
        let moved = [
            attempt(agents[5], 5, SONNET, hard),
            attempt(agents[5], 6, SONNET, hard),
        ];
        let copy_gone = attempt(agents[6], 7, SONNET, hard);
        let elsewhere = dir.path().join("removed");
        let mut lines = Vec::new();
        for attempt in [&picked, &easy, &on_opus, &pinned, &moved[0], &moved[1]] {
            lines.push(Line::Attempt(attempt.clone()));
            lines.push(Line::Truth(truth(attempt, false, Some(dir.path()))));
        }
        lines.push(Line::Attempt(no_copy.clone()));
        lines.push(Line::Truth(truth(&no_copy, false, None)));
        lines.push(Line::Attempt(copy_gone.clone()));
        lines.push(Line::Truth(truth(&copy_gone, false, Some(&elsewhere))));

        let options = options(dir.path());
        let (picks, left_out) = pick(&lines, &options, &HashSet::new());
        assert_eq!(picks.len(), 1, "{left_out:?}");
        assert_eq!(picks[0].stood.attempt.agent, picked.agent);
        let count = |start: &str| {
            let mut counts = left_out
                .iter()
                .filter(|(why, _)| why.starts_with(start))
                .map(|(_, count)| *count);
            (counts.next(), counts.next())
        };
        assert_eq!(count("Gate did not choose"), (Some(1), None));
        assert_eq!(count("not rated hard"), (Some(1), None));
        assert_eq!(count("on the strongest tier"), (Some(1), None));
        assert_eq!(count("their run moved"), (Some(2), None));
        assert_eq!(count("the log does not say"), (Some(1), None));
        assert_eq!(count("their run's copy is not kept"), (Some(1), None));
        assert_eq!(left_out.len(), 6, "{left_out:?}");

        // Any difficulty on request; what was replayed is not asked again
        // unless asked for.
        let all = ReplayOptions {
            any_difficulty: true,
            ..options.clone()
        };
        assert_eq!(pick(&lines, &all, &HashSet::new()).0.len(), 2);
        let done = HashSet::from([(picked.agent.as_str(), picked.prompt_tx.as_str())]);
        assert!(pick(&lines, &options, &done).0.is_empty());
        let again = ReplayOptions {
            again: true,
            ..options.clone()
        };
        assert_eq!(pick(&lines, &again, &done).0.len(), 1);
        let other_task = ReplayOptions {
            task: Some("element-web".to_string()),
            ..options
        };
        let (picks, left_out) = pick(&lines, &other_task, &HashSet::new());
        assert!(picks.is_empty() && left_out.is_empty());
        assert_eq!(strongest_in(&lines).as_deref(), Some(OPUS));
    }

    #[tokio::test]
    async fn the_reviewer_is_shown_what_the_run_left_and_its_answer_is_logged() {
        let dir = TempDir::new().unwrap();
        let Some(copy) = kept_copy(dir.path()) else {
            return;
        };
        let agent = AgentId::generate();
        let line = attempt(agent, 0, SONNET, Some(Difficulty::Hard));
        let proved = Truth {
            copy: Some(copy),
            ..truth(&line, false, None)
        };
        let sessions = Checkpoints::new(dir.path());
        let saved = crate::session_checkpoint::SessionCheckpoint::new(
            5,
            7,
            session(),
            Vec::new(),
            Default::default(),
        );
        sessions.save(agent, &saved).await.unwrap();
        let lines = [Line::Attempt(line.clone()), Line::Truth(proved)];
        let options = options(dir.path());
        let (picks, _) = pick(&lines, &options, &HashSet::new());
        let ready = prepare(&picks[0], &sessions, &Tiers::default()).unwrap();

        assert_eq!(ready.task, "Make f() return 2.");
        assert!(
            ready.diff.contains("-    return 1\n+    return 2"),
            "{}",
            ready.diff
        );
        assert_eq!(
            ready.attempt.evidence.changed_paths.as_deref(),
            Some(&["lib.py".to_string()][..])
        );
        assert_eq!(
            ready.signs,
            [Flag::ShellEdit, Flag::NoTestChanged],
            "the change was made by a command and touched no test"
        );
        assert_eq!(
            ready.why,
            [("hard".to_string(), Why::Hard.describe().to_string())]
        );
        assert!(ready.commands.contains("sed -i"), "{}", ready.commands);
        assert!(ready.checks.contains("1 passed"), "{}", ready.checks);
        assert_eq!(ready.reply, "Done; the tests pass.");
        assert_eq!(
            (ready.run.as_str(), ready.right),
            ("Compare #22 a-6", false)
        );
        assert!(estimate(OPUS, std::slice::from_ref(&ready), ReviewVariant::Plain).is_some());

        // The reviewer's answer goes to the log, marked as replayed.
        let provider = Arc::new(aura_model::MockProvider::simple_response(
            "1. `f` is not covered for negative input.\nVERDICT: MODEL",
        ));
        let log = Arc::new(AttemptLog::in_data_dir(dir.path()));
        let asked = ask(
            provider,
            OPUS.to_string(),
            ReviewVariant::Plain,
            None,
            Arc::clone(&log),
            ready,
        )
        .await;
        assert!(
            matches!(&asked.answer, Ok((Answer::Model, _, _))),
            "{asked:?}"
        );
        let shown = row(&asked);
        assert!(shown.contains("wrong  unfinished"), "{shown}");
        assert!(shown.contains("1. `f` is not covered"), "{shown}");
        let written = log.read().unwrap().lines;
        let [Line::Review(review)] = &written[..] else {
            panic!("{written:?}");
        };
        assert!(review.replayed && !review.acted);
        assert_eq!(
            (review.variant.as_str(), review.judge.as_str()),
            ("plain", OPUS)
        );
        assert_eq!(review.why, ["hard"]);
        assert_eq!(review.signs, [Flag::ShellEdit, Flag::NoTestChanged]);
        assert_eq!(review.prompt_tx, line.prompt_tx);
        let summary = tally(&[asked]);
        assert!(
            summary.contains("wrong work: 1 of 1 found unfinished"),
            "{summary}"
        );
        assert!(
            summary.contains("right work: 0 of 0 found unfinished"),
            "{summary}"
        );
    }

    /// A reply that calls the criteria tool with `items`.
    fn criteria_reply(items: serde_json::Value) -> aura_model::MockResponse {
        aura_model::MockResponse::tool_use(
            "toolu_1",
            criteria::CRITERIA_TOOL,
            json!({"items": items, "strict": true}),
        )
    }

    #[tokio::test]
    async fn with_criteria_each_request_gets_them_once_and_the_review_goes_through_them() {
        let dir = TempDir::new().unwrap();
        let Some(copy) = kept_copy(dir.path()) else {
            return;
        };
        let agent = AgentId::generate();
        let line = attempt(agent, 0, SONNET, Some(Difficulty::Hard));
        let proved = Truth {
            copy: Some(copy),
            ..truth(&line, false, None)
        };
        let sessions = Checkpoints::new(dir.path());
        let saved = crate::session_checkpoint::SessionCheckpoint::new(
            5,
            7,
            session(),
            Vec::new(),
            Default::default(),
        );
        sessions.save(agent, &saved).await.unwrap();
        let lines = [Line::Attempt(line.clone()), Line::Truth(proved)];
        let (picks, _) = pick(&lines, &options(dir.path()), &HashSet::new());
        let ready = prepare(&picks[0], &sessions, &Tiers::default()).unwrap();
        assert_eq!(writer_in(&lines).as_deref(), Some(SONNET));

        // Two attempts at one request: its criteria are written once,
        // and kept for the next replay.
        let writer: Arc<dyn ModelProvider + Send + Sync> = Arc::new(
            aura_model::MockProvider::new().with_response(criteria_reply(json!([
                {"text": "f() returns 2", "kind": "behaviour"},
                {"text": "f(-1) raises ValueError", "kind": "error"},
            ]))),
        );
        let mut written = Written::in_data_dir(dir.path());
        let (held, cost_usd) = hold_to_criteria(
            vec![ready.clone(), ready.clone()],
            &mut written,
            SONNET,
            (&writer, None),
        )
        .await
        .unwrap();
        assert_eq!(held.len(), 2);
        assert!(cost_usd > 0.0);
        let criteria = held[0].criteria.clone().unwrap();
        assert_eq!((criteria.items.len(), criteria.strict), (2, true));
        assert_eq!(
            held[0].why,
            [
                ("hard".to_string(), Why::Hard.describe().to_string()),
                ("criteria".to_string(), Why::Criteria.describe().to_string()),
            ]
        );
        let kept = Written::in_data_dir(dir.path());
        assert_eq!(kept.get(SONNET, &ready.task), Some(criteria));
        assert_eq!(
            kept.get(OPUS, &ready.task),
            None,
            "another writer's are its own"
        );

        // The reviewer is shown them, and its line for each is logged.
        let why = held[0].why_told();
        let shown = turn::review_prompt(&held[0].standing(&why));
        assert!(
            shown.contains("2. [error] f(-1) raises ValueError"),
            "{shown}"
        );
        let reviewer = Arc::new(aura_model::MockProvider::simple_response(
            "CRITERION 1: MET by test_f\nCRITERION 2: UNMET, nothing raises\n\
             1. Negative input is not rejected.\nVERDICT: MODEL",
        ));
        let log = Arc::new(AttemptLog::in_data_dir(dir.path()));
        let asked = ask(
            reviewer,
            OPUS.to_string(),
            ReviewVariant::Plain,
            None,
            Arc::clone(&log),
            held[0].clone(),
        )
        .await;
        let shown = row(&asked);
        assert!(
            shown.contains("unfinished") && shown.contains(", criteria 1/2 met, 1 unmet"),
            "{shown}"
        );
        let written = log.read().unwrap().lines;
        let [Line::Review(review)] = &written[..] else {
            panic!("{written:?}");
        };
        assert_eq!(review.variant, "plain+criteria");
        assert_eq!(review.why, ["hard", "criteria"]);
        assert_eq!(
            review
                .criteria
                .iter()
                .map(|checked| checked.status)
                .collect::<Vec<_>>(),
            [Some(criteria::Status::Met), Some(criteria::Status::Unmet)]
        );

        // A request the writer gives no criteria for is left out.
        let silent: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(aura_model::MockProvider::new().with_response(criteria_reply(json!([]))));
        let mut none = Written::in_data_dir(&dir.path().join("elsewhere"));
        let (held, _) = hold_to_criteria(vec![ready], &mut none, SONNET, (&silent, None))
            .await
            .unwrap();
        assert!(held.is_empty());
    }
}
