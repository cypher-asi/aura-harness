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
//! saved session, and the final reply. Which attempts those are, what
//! the reviewer is shown and what its answers come to are
//! [`aura_gate::attempts::rereview`]'s; this reads what the runs left and
//! asks the models.
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
//!
//! - `prepare`: what each attempt's run left, from its saved session and
//!   its kept copy
//! - `criteria`: the criteria written for replays, and their writer's
//!   calls
//! - `ask`: the reviewer's calls, and the log's lines for its answers

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use aura_gate::attempts::rereview::{
    asked_as, estimate, pick, replayed, strongest_in, tally, writer_in, Filter, Pick, Prepared,
};
use aura_gate::attempts::{AttemptLog, CopyAt, Line};
use aura_gate::evidence::Evidence;
use aura_gate::judge::turn::ReviewVariant;
use aura_model::model_catalog::route_label;
use aura_model::ModelProvider;

mod ask;
mod criteria;
mod prepare;
#[cfg(test)]
mod tests;

use ask::Reviewer;
use criteria::{hold_to_criteria, Written};

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

impl ReplayOptions {
    /// Which attempts are asked about.
    fn filter(&self) -> Filter<'_> {
        Filter {
            task: self.task.as_deref(),
            any_difficulty: self.any_difficulty,
            again: self.again,
            limit: self.limit,
        }
    }

    /// How the reviewer is asked.
    const fn variant(&self) -> ReviewVariant {
        if self.thinking {
            ReviewVariant::Thinking
        } else {
            ReviewVariant::Plain
        }
    }
}

/// Run `aura gate replay` as `options` say, printing what it does.
///
/// # Errors
/// When the attempt log or the sign-in state cannot be read, or no model
/// to review with (or to write the criteria with) is named or found in
/// the log.
pub async fn gate_replay(options: ReplayOptions) -> anyhow::Result<()> {
    let log = Arc::new(AttemptLog::in_data_dir(&options.data_dir));
    let read = log
        .read()
        .with_context(|| format!("reading {}", log.path().display()))?;
    let judge = options.judge.clone().or_else(|| strongest_in(&read.lines));
    // Asked the same way: by the same reviewer, held to criteria or not.
    let asked_as = asked_as(options.variant(), options.criteria);
    let done = replayed(&read.lines, &asked_as, judge.as_deref());
    let kept = |copy: &CopyAt| copy.root.is_dir();
    let (picks, left_out) = pick(&read.lines, &options.filter(), &done, kept);
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
    let prepared = prepare::all(&picks, &options.data_dir, &read.lines);
    let replay = Replay {
        options: &options,
        log,
        judge,
        writer,
    };
    replay.run(prepared, &picks).await
}

/// A replay under way: what it was asked to do, the log it appends to,
/// the model that reviews and the one that writes the criteria.
struct Replay<'a> {
    options: &'a ReplayOptions,
    log: Arc<AttemptLog>,
    judge: String,
    writer: Option<String>,
}

impl Replay<'_> {
    /// Say what `prepared`, the attempts of `picks` as the reviewer is
    /// shown them, are asked about and what it may cost. Unless this is a
    /// dry run, write their lines again where their runs show more, hold
    /// them to criteria, ask about each and tally the answers.
    async fn run(&self, prepared: Vec<Prepared>, picks: &[Pick<'_>]) -> anyhow::Result<()> {
        self.announce(&prepared);
        let mut written = Written::in_data_dir(&self.options.data_dir);
        if let Some(writer) = &self.writer {
            written.announce(writer, &prepared);
        }
        if self.options.dry_run {
            for ready in &prepared {
                println!("{}", ready.listed());
            }
            return Ok(());
        }
        rewrite(&self.log, &prepared, picks)?;
        let (provider, auth_token) = provider()?;
        let (prepared, criteria_usd) = match &self.writer {
            Some(writer) => {
                let by = (&provider, auth_token.clone());
                hold_to_criteria(prepared, &mut written, writer, by).await?
            }
            None => (prepared, 0.0),
        };
        let reviewer = Reviewer {
            provider,
            auth_token,
            judge: self.judge.clone(),
            variant: self.options.variant(),
            log: Arc::clone(&self.log),
        };
        let all = reviewer.ask_all(prepared, self.options.jobs).await?;
        println!("{}", tally(&all));
        if let Some(writer) = &self.writer {
            println!(
                "writing the criteria cost ${criteria_usd:.2} on {}",
                route_label(writer)
            );
        }
        println!("`aura gate eval` scores every review in the log.");
        Ok(())
    }

    /// Say who is asked about how many of `prepared`, and what it may cost.
    fn announce(&self, prepared: &[Prepared]) {
        let variant = self.options.variant();
        let wrong = prepared.iter().filter(|p| !p.right).count();
        println!(
            "asking {} ({}) about {} attempts, {wrong} of them wrong{}",
            route_label(&self.judge),
            asked_as(variant, self.options.criteria),
            prepared.len(),
            estimate(&self.judge, prepared, variant).map_or_else(String::new, |usd| format!(
                ": about ${usd:.2} at list price"
            )),
        );
    }
}

/// Append to `log` again the line of each of `prepared` whose evidence,
/// as its run shows it, differs from what its line in the log (`picks`)
/// holds.
fn rewrite(log: &AttemptLog, prepared: &[Prepared], picks: &[Pick<'_>]) -> anyhow::Result<()> {
    let logged: HashMap<(&str, &str), &Evidence> = picks
        .iter()
        .map(|pick| {
            let attempt = pick.stood.attempt;
            let key = (attempt.agent.as_str(), attempt.prompt_tx.as_str());
            (key, &attempt.evidence)
        })
        .collect();
    for attempt in prepared.iter().map(|ready| &ready.attempt) {
        let key = (attempt.agent.as_str(), attempt.prompt_tx.as_str());
        if logged.get(&key) != Some(&&attempt.evidence) {
            log.append(&Line::Attempt(attempt.clone()))
                .with_context(|| format!("writing {}", log.path().display()))?;
        }
    }
    Ok(())
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
