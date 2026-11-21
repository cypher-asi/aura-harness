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
//! Each answer is appended to the log as a `review` line marked
//! `replayed`, which `aura gate eval` scores. The attempt's line is
//! written again with the files the run changed by any means, so the
//! signs that need them (`shell_edit`, `no_test_changed`) are scored
//! too. Nothing else is touched: no record, no table, no copy.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use aura_agent_gate::attempt_log::{
    self, below_strongest, Attempt, AttemptLog, CopyAt, Line, Review, Stood, Tier, Tiers,
    TruthSource,
};
use aura_agent_gate::evidence::{flags, Evidence, Flag};
use aura_agent_gate::followup::Why;
use aura_agent_gate::turn_judge::{self, Answer, ReviewVariant, Standing};
use aura_core_types::{AgentId, Difficulty};
use aura_model::model_catalog::{estimate_cost_usd, infer_tier, route_label, ModelTier};
use aura_model::{Message, ModelProvider, Usage};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::event_loop::gate_followup::{checks_shown, commands_run, final_reply_in};
use crate::event_loop::gate_log::sent_text;
use crate::event_loop::processes::now_ms;
use crate::event_loop::tasks::TASKS_DIR;
use crate::isolation::changes;
use crate::session_checkpoint::Checkpoints;

/// Commands shown to the reviewer, latest last.
const SHOWN_COMMANDS: usize = 40;

/// How long one review may take.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(420);

/// Characters per token assumed when estimating what the reviews cost.
const CHARS_PER_TOKEN: f64 = 3.2;

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
    for stood in attempt_log::stood(lines) {
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
        }
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
        commands: commands_run(messages, SHOWN_COMMANDS),
        checks: checks_shown(messages),
        reply: final_reply_in(messages),
        why,
        signs,
    })
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
            let chars = turn_judge::REVIEW_SYSTEM_PROMPT.len()
                + turn_judge::review_prompt(&prepared.standing(&why)).len();
            #[allow(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss
            )]
            let input = (chars as f64 / CHARS_PER_TOKEN) as u64;
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
    let request = turn_judge::review_request(&judge, &prepared.standing(&why), variant, auth_token);
    let answer = match request {
        Err(e) => Err(format!("could not build the request: {e}")),
        Ok(request) => match tokio::time::timeout(REVIEW_TIMEOUT, provider.complete(request)).await
        {
            Err(_) => Err("the reviewer took too long".to_string()),
            Ok(Err(e)) => Err(format!("the reviewer's call failed: {e}")),
            Ok(Ok(response)) => {
                let cost_usd = estimate_cost_usd(&judge, &response.usage).unwrap_or(0.0);
                turn_judge::parse_answer(&response.message.text_content())
                    .map(|(answer, findings)| (answer, findings, cost_usd))
                    .ok_or_else(|| NO_VERDICT.to_string())
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
            variant: variant.name().to_string(),
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
    Asked {
        answer,
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
            let mut line = format!("{head}  {said:10}  ${cost_usd:.3}");
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
    let cost: f64 = answered.iter().map(|(_, _, cost_usd)| cost_usd).sum();
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
/// to review with is named or found in the log.
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
    let done: HashSet<(&str, &str)> = read
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Review(review) if review.replayed && review.variant == variant.name() => {
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
    let judge = options
        .judge
        .clone()
        .or_else(|| strongest_in(&read.lines))
        .context("no model to review with: name one with --judge")?;

    let tiers = attempt_log::tiers(&read.lines);
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
        "asking {} ({}) about {} attempts, {wrong} of them wrong{}",
        route_label(&judge),
        variant.name(),
        prepared.len(),
        estimate(&judge, &prepared, variant).map_or_else(String::new, |usd| format!(
            ": about ${usd:.2} at list price"
        )),
    );
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
            if problem != NO_VERDICT {
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
    println!("`aura gate eval` scores every review in the log.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_agent_gate::attempt_log::{Terms, Truth};
    use aura_core_types::{Hash, OutcomePayload, OutcomeSignal, PromptCategory};
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
}
