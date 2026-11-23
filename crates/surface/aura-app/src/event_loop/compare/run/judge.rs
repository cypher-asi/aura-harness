//! The blind judge of a comparison's runs: each run of arm A against arm
//! B's run of the same number, shown in an order seeded by the
//! comparison, and rated against what the user kept when the work is a
//! replayed session.

use super::context::{Context, Ran};
use crate::event_loop::agents::AgentSession;
use aura_core_types::CompareJudgment;
use aura_gate::compare::{judge_pairs, judge_task, list_price, seed, EndedRun};
use aura_gate::judge::blind;
use futures_util::stream::{self, StreamExt};
use std::borrow::Cow;
use std::time::Duration;
use tracing::warn;

/// Longest a judge call may take.
const JUDGE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Judge reasoning kept in the record, in characters.
const MAX_REASONING: usize = 2_000;

/// Have the judge compare each run of arm A with arm B's run of the same
/// number, through the comparison's own kernel, and rate each against
/// what the user kept when the work is a replayed session. As many pairs
/// are judged at once as runs may work at once, and the judgments keep
/// the pairs' order.
pub(super) async fn judge_all(
    ctx: &Context,
    session: &AgentSession,
    ran: &[Ran],
) -> Vec<CompareJudgment> {
    let ended: Vec<EndedRun> = ran
        .iter()
        .map(|r| EndedRun {
            arm: r.run.arm,
            n: r.order.0,
            errored: r.run.error.is_some(),
        })
        .collect();
    let pairs = judge_pairs(&ended, !ctx.plan.examples.is_empty());
    if pairs.is_empty() || ctx.plan.judge.is_empty() {
        return Vec::new();
    }
    let judging = PairJudging {
        ctx,
        session,
        auth_token: ctx.hub.turn(None, &ctx.plan.workspace).0.auth_token,
        seed: seed(ctx.host.agent_id()),
        shared_task: judge_task(&ctx.plan.prompts),
        reference: ctx.plan.source.reference().map(|r| blind::Attempt {
            diff: r.diff.clone(),
            reply: r.reply.clone(),
        }),
    };
    stream::iter(pairs.into_iter().enumerate())
        .map(|(i, (a, b))| judging.judge(i, (&ran[a], &ran[b])))
        .buffered(ctx.plan.concurrency.max(1))
        .filter_map(std::future::ready)
        .collect()
        .await
}

/// What the judge is given for every pair of a comparison.
struct PairJudging<'a> {
    ctx: &'a Context,
    session: &'a AgentSession,
    auth_token: Option<String>,
    seed: u64,
    /// The task of the runs that are not of an example task
    shared_task: String,
    /// What the user kept, when the work is a replayed session
    reference: Option<blind::Attempt>,
}

impl PairJudging<'_> {
    /// The judge's verdict on the `i`th pair, `(a, b)`; `None` when the
    /// comparison was cancelled before the judge was asked.
    async fn judge(&self, i: usize, (a, b): (&Ran, &Ran)) -> Option<CompareJudgment> {
        if self.ctx.cancel.is_cancelled() {
            return None;
        }
        let a_first = blind::a_first(self.seed, i);
        let mut judgment = CompareJudgment {
            a_run: a.run.name.clone(),
            b_run: b.run.name.clone(),
            a_first,
            winner: None,
            parsed: false,
            a_vs_reference: None,
            b_vs_reference: None,
            model: self.ctx.plan.judge.clone(),
            cost_usd: 0.0,
            reasoning: String::new(),
        };
        match self.ask((a, b), a_first).await {
            Ok((text, cost_usd)) => {
                judgment.cost_usd = cost_usd;
                if let Some(verdict) = blind::parse(&text) {
                    judgment.parsed = true;
                    judgment.winner = blind::winner(verdict, a_first);
                }
                if self.reference.is_some() {
                    (judgment.a_vs_reference, judgment.b_vs_reference) =
                        blind::reference_verdicts(&text, a_first);
                }
                let named = blind::unblind(&text, a_first, (&a.run.name, &b.run.name));
                judgment.reasoning = named.chars().take(MAX_REASONING).collect();
            }
            Err(problem) => {
                warn!(%problem, "Judge gave no verdict");
                judgment.reasoning = problem;
            }
        }
        Some(judgment)
    }

    /// The judge's reply on `(a, b)`, shown `a` first when `a_first`, and
    /// what it cost; else why there is none.
    async fn ask(&self, (a, b): (&Ran, &Ran), a_first: bool) -> Result<(String, f64), String> {
        let plan = &self.ctx.plan;
        let task = plan
            .example(a.order.0)
            .map_or(Cow::Borrowed(self.shared_task.as_str()), |task| {
                Cow::Owned(judge_task(&task.prompts))
            });
        let request = blind::request(
            &plan.judge,
            &task,
            (&attempt(a), &attempt(b)),
            a_first,
            self.reference.as_ref(),
            self.auth_token.clone(),
        )
        .map_err(|e| format!("judge request: {e}"))?;
        match tokio::time::timeout(JUDGE_TIMEOUT, self.session.kernel.reason(request)).await {
            Ok(Ok(result)) => Ok((
                blind::reply_text(&result.response),
                list_price(&plan.judge, &result.response.usage).unwrap_or(0.0),
            )),
            Ok(Err(e)) => Err(format!("judge call failed: {e}")),
            Err(_) => Err("judge call timed out".to_string()),
        }
    }
}

/// Run `r` as the judge is shown it.
fn attempt(r: &Ran) -> blind::Attempt {
    blind::Attempt {
        diff: r.patch.clone(),
        reply: r.reply.clone(),
    }
}
