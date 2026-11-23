//! An attempt as the reviewer is shown it.

use super::{asked_as, Pick};
use crate::{Attempt, Line, Review, Tier, Tiers};
use aura_core_types::Difficulty;
use aura_gate_evidence::transcript::reply::final_reply;
use aura_gate_evidence::{flags, Evidence, Flag};
use aura_gate_followup::Why;
use aura_gate_judge::criteria::{self, Criteria};
use aura_gate_judge::turn::{self, shown_to_reviewer, Answer, ReviewVariant, Standing};
use aura_model::model_catalog::{estimate_cost_usd, route_label};
use aura_model::{Message, Usage};
use std::sync::Arc;

/// Why an attempt is reviewed when nothing else says: it is being
/// replayed.
const REPLAY_REASON: &str = "the work is reviewed before it is accepted";

/// Characters per token assumed when estimating what the reviews cost.
const ESTIMATE_CHARS_PER_TOKEN: f64 = 3.2;

/// Output tokens assumed per review when estimating: plain, thinking.
const ESTIMATED_OUTPUT: [u64; 2] = [600, 6_000];

/// An attempt as the reviewer is shown it.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The attempt's line, with the evidence as it is known now
    pub attempt: Attempt,
    /// `Compare #22 a-4`
    pub run: String,
    /// The benchmark task
    pub task_id: String,
    /// Whether the task's check passed
    pub right: bool,
    /// What the user asked
    pub task: String,
    /// Everything the run changed
    pub diff: String,
    pub commands: String,
    pub checks: String,
    pub reply: String,
    /// Why it is reviewed, as logged and as the reviewer is told
    pub why: Vec<(String, String)>,
    /// Every sign its evidence raises
    pub signs: Vec<Flag>,
    /// What the work is held to, when the replay holds it to criteria
    pub criteria: Option<Arc<Criteria>>,
}

impl Prepared {
    /// The attempt of `pick` as what its run left shows it: `task`, what
    /// the user asked, and its saved session's `messages` from that
    /// prompt on; `diff`, everything the run changed, in the files
    /// `changed`. Why it is reviewed follows from the signs it raises,
    /// with the tiers `tiers`.
    #[must_use]
    pub fn of(
        pick: &Pick<'_>,
        (task, messages): (String, &[Message]),
        diff: String,
        changed: &[String],
        tiers: &Tiers,
    ) -> Self {
        let (attempt, truth) = (pick.stood.attempt, pick.stood.truth);
        let mut evidence = Evidence::of(messages);
        evidence.hit_limit = attempt.evidence.hit_limit;
        evidence.note_changed(changed);
        let signs = flags(&evidence, attempt.category);
        let why = reasons(attempt, &signs, tiers);
        let run = [truth.compare.as_deref(), truth.run.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let (commands, checks) = shown_to_reviewer(messages);
        Self {
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
        }
    }

    /// What the reviewer is shown, told `why` it reviews.
    #[must_use]
    pub fn standing<'a>(&'a self, why: &'a [String]) -> Standing<'a> {
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
    #[must_use]
    pub fn held_to(mut self, criteria: Arc<Criteria>) -> Self {
        let why = Why::Criteria;
        self.why
            .retain(|(name, _)| name != "replay" && name != why.name());
        self.why
            .push((why.name().to_string(), why.describe().to_string()));
        self.criteria = Some(criteria);
        self
    }

    /// What the reviewer is told of why it reviews.
    #[must_use]
    pub fn why_told(&self) -> Vec<String> {
        self.why.iter().map(|(_, told)| told.clone()).collect()
    }

    /// The log's line, written at `at_ms`, for `judge`'s review of the
    /// attempt asked as `variant`, which answered `verdict` with
    /// `findings` for `cost_usd`.
    #[must_use]
    pub fn review(
        &self,
        judge: &str,
        variant: ReviewVariant,
        (verdict, findings, cost_usd): (Answer, &str, Option<f64>),
        at_ms: u64,
    ) -> Line {
        Line::Review(Review {
            at_ms,
            agent: self.attempt.agent.clone(),
            prompt_tx: self.attempt.prompt_tx.clone(),
            why: self.why.iter().map(|(name, _)| name.clone()).collect(),
            signs: self.signs.clone(),
            verdict,
            findings: findings.to_string(),
            judge: judge.to_string(),
            cost_usd,
            variant: asked_as(variant, self.criteria.is_some()),
            criteria: (self.criteria.as_deref())
                .map(|held| criteria::checked(held, findings))
                .unwrap_or_default(),
            acted: false,
            replayed: true,
        })
    }

    /// One line saying what the reviewer would be asked about, for a dry
    /// run.
    #[must_use]
    pub fn listed(&self) -> String {
        let signs: Vec<&str> = self.signs.iter().map(|sign| sign.name()).collect();
        format!(
            "  {:16} {:40} {:32} {:5}  {} files, {} chars of diff  {}",
            self.run,
            self.task_id,
            route_label(&self.attempt.model),
            if self.right { "right" } else { "wrong" },
            self.attempt.evidence.files_changed.unwrap_or_default(),
            self.diff.len(),
            signs.join(" ")
        )
    }
}

/// Why `attempt`, which raises `signs`, would be reviewed with the tiers
/// `tiers`: as logged, and as the reviewer is told.
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

/// What asking `judge` about `prepared` as `variant` may cost at list
/// price; `None` when the model has no list price.
#[must_use]
pub fn estimate(judge: &str, prepared: &[Prepared], variant: ReviewVariant) -> Option<f64> {
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
