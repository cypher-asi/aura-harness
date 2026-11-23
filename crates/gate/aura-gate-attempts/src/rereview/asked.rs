//! What a review came to, as a row, and how the reviews held up.

use super::Prepared;
use aura_gate_judge::criteria;
use aura_gate_judge::turn::{self, Answer};
use aura_model::model_catalog::{estimate_cost_usd, route_label};
use aura_model::{ModelResponse, StopReason};

/// What is said of a review whose reply names no verdict.
const NO_VERDICT: &str = "the reviewer gave no verdict";

/// Findings shown under the row of work found unfinished, at most.
const FINDINGS_SHOWN: usize = 6;

/// What one review came to.
#[derive(Debug)]
pub struct Asked {
    pub run: String,
    pub task_id: String,
    pub model: String,
    pub right: bool,
    /// The answer, what was found and what it cost (`None` when the
    /// reviewer's price is unknown); the problem when the reviewer gave
    /// none
    pub answer: Result<(Answer, String, Option<f64>), String>,
    /// What a reply without a verdict cost; 0 when its price is unknown
    pub wasted_usd: f64,
    /// How many of its criteria the reviewer found met, when it was held
    /// to any
    pub criteria: String,
}

impl Asked {
    /// The review of `prepared` that came to `answer`.
    fn new(prepared: &Prepared, answer: Result<(Answer, String, Option<f64>), String>) -> Self {
        Self {
            run: prepared.run.clone(),
            task_id: prepared.task_id.clone(),
            model: route_label(&prepared.attempt.model),
            right: prepared.right,
            answer,
            wasted_usd: 0.0,
            criteria: String::new(),
        }
    }

    /// The review of `prepared` that came to no answer, for `problem`.
    #[must_use]
    pub fn failed(prepared: &Prepared, problem: String) -> Self {
        Self::new(prepared, Err(problem))
    }

    /// `judge`'s review of `prepared`, read from its `response`.
    #[must_use]
    pub fn of(prepared: &Prepared, judge: &str, response: &ModelResponse) -> Self {
        let cost_usd = estimate_cost_usd(judge, &response.usage);
        let Some((answer, findings)) = turn::parse_answer(&response.message.text_content()) else {
            let cut = if response.stop_reason == StopReason::MaxTokens {
                "its reply was cut off at the token limit"
            } else {
                "its reply ended without one"
            };
            let problem = format!(
                "{NO_VERDICT}: {cut} after {} tokens, {}",
                response.usage.output_tokens,
                cost_text(cost_usd)
            );
            return Self {
                wasted_usd: cost_usd.unwrap_or(0.0),
                ..Self::failed(prepared, problem)
            };
        };
        let criteria = (prepared.criteria.as_deref())
            .map(|held| criteria::tally(&criteria::checked(held, &findings)))
            .unwrap_or_default();
        Self {
            criteria,
            ..Self::new(prepared, Ok((answer, findings, cost_usd)))
        }
    }

    /// Why the reviewer could not be reached for this review, when it
    /// could not: any problem but a reply that named no verdict.
    #[must_use]
    pub fn unreached(&self) -> Option<&str> {
        (self.answer.as_ref().err())
            .map(String::as_str)
            .filter(|problem| !problem.starts_with(NO_VERDICT))
    }

    /// One line saying how the review went, then what it found when it
    /// found the work unfinished.
    #[must_use]
    pub fn row(&self) -> String {
        let proved = if self.right { "right" } else { "wrong" };
        let head = format!(
            "  {:16} {:40} {:32} {proved:5}",
            self.run, self.task_id, self.model
        );
        let (answer, findings, cost_usd) = match &self.answer {
            Ok(answered) => answered,
            Err(problem) => return format!("{head}  no answer: {problem}"),
        };
        let said = match answer {
            Answer::Model => "unfinished",
            Answer::Ok => "done",
            Answer::Other => "held up",
        };
        let mut line = format!(
            "{head}  {said:10}  {}{}",
            cost_text(*cost_usd),
            self.criteria
        );
        if *answer == Answer::Model {
            let shown = findings.lines().filter(|l| !l.trim().is_empty());
            for finding in shown.take(FINDINGS_SHOWN) {
                line.push_str("\n      ");
                line.push_str(finding.trim_end());
            }
        }
        line
    }
}

/// `$0.012`, or `price unknown`.
fn cost_text(cost_usd: Option<f64>) -> String {
    cost_usd.map_or_else(|| "price unknown".to_string(), |usd| format!("${usd:.3}"))
}

/// How the reviews in `asked` held up, in a few lines.
#[must_use]
pub fn tally(asked: &[Asked]) -> String {
    let answered: Vec<(&Asked, Answer, Option<f64>)> = asked
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
        .filter_map(|(_, _, cost_usd)| *cost_usd)
        .sum::<f64>()
        + asked.iter().map(|asked| asked.wasted_usd).sum::<f64>();
    let unpriced = answered.iter().any(|(_, _, cost_usd)| cost_usd.is_none());
    let cost = if unpriced {
        format!("${cost:.2}+? at list price (some prices unknown)")
    } else {
        format!("${cost:.2} at list price")
    };
    format!(
        "{} reviews answered, {} not, {cost}\n\
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
