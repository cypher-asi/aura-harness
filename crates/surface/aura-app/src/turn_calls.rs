//! The models a turn ran on, for the footer under its reply:
//! `claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5 · title: claude-haiku-5-5`,
//! with the tokens and cost of every call.
//!
//! Every model call is a `Reasoning` entry in the agent's record, so the
//! footer is read off the record the same way while the turn runs and
//! when the chat is loaded again. A turn starts at a prompt that records
//! where its work runs, and takes the classifier's call made just before
//! that prompt was recorded. A stronger model taking the turn over
//! carries it on, and so do messages sent into it while it runs, which
//! are recorded without a route before its reply. Calls Gate makes
//! (classifying, judging) and the chat's title are told apart from the
//! turn's own by the purpose their entry records.
//!
//! Every prompt is classified, so Gate learns how each model does on
//! each kind of work, but the classifier's call routes the turn only when
//! Gate picked its model. On a model the user or an agent definition
//! chose, the prompt's route is `pinned` and the call is marked
//! `classify:` instead of `gate:`, so the footer doesn't read as if Gate
//! had chosen the model.

use crate::event_loop::agent_events::call_usage;
use crate::record_loader::{json_payload, model_call};
use aura_core_types::{Route, RouteReason, Transaction, TransactionType};
use aura_model::model_catalog::upstream_model_name;
use aura_model::ModelCallPurpose;
use aura_ui_terminal::events::CallUsage;

/// What one entry did to the turn being tallied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    /// A prompt that starts a new turn
    NewTurn,
    /// A model call of the current turn
    Call,
    /// Anything else, or a call held for the next turn
    Other,
}

/// The model calls of an agent's current turn.
#[derive(Debug, Default)]
pub(crate) struct TurnCalls {
    current: Tally,
    /// Classifier calls made for the prompt about to be recorded
    next: Tally,
    /// Whether the current turn's reply is recorded
    replied: bool,
    /// Whether Gate picked the current turn's model, so its classifier's
    /// call routed the turn
    routed: bool,
}

impl TurnCalls {
    /// Take in the record entry `tx`.
    pub(crate) fn see(&mut self, tx: &Transaction) -> Seen {
        match tx.tx_type {
            TransactionType::UserPrompt => {
                let route = tx
                    .prompt_payload()
                    .map(|prompt| prompt.route)
                    .filter(|route| !route.model.is_empty());
                if !self.starts_turn(route.as_ref()) {
                    return Seen::Other;
                }
                self.current = std::mem::take(&mut self.next);
                self.replied = false;
                self.routed = route.is_some_and(|route| route.reason != RouteReason::Pinned);
                Seen::NewTurn
            }
            TransactionType::AgentMsg => {
                self.replied = true;
                Seen::Other
            }
            TransactionType::Reasoning => {
                let Some((model, usage)) = model_call(tx) else {
                    return Seen::Other;
                };
                let purpose = json_payload(tx)
                    .and_then(|payload| payload.get("purpose").cloned())
                    .and_then(|purpose| serde_json::from_value(purpose).ok());
                let call = call_usage(&model, &usage);
                if matches!(
                    purpose,
                    Some(ModelCallPurpose::Classify | ModelCallPurpose::Criteria)
                ) {
                    self.next.add(&model, purpose, call);
                    return Seen::Other;
                }
                self.current.add(&model, purpose, call);
                Seen::Call
            }
            _ => Seen::Other,
        }
    }

    /// The footer of the current turn: its models, then the tokens and
    /// cost of all its calls. `None` before its first call.
    pub(crate) fn footer(&self) -> Option<(String, Option<CallUsage>)> {
        self.current.footer(self.routed)
    }

    /// Whether a prompt recording `route` starts a turn: it records where
    /// its work runs and is not a stronger model taking a turn over, or it
    /// records no route and the current turn has replied or made no call
    /// yet.
    fn starts_turn(&self, route: Option<&Route>) -> bool {
        match route {
            Some(route) => route.reason != RouteReason::Escalate,
            None => self.replied || self.current.models.is_empty(),
        }
    }
}

#[derive(Debug, Default)]
struct Tally {
    /// Each model and what it ran for, with its number of calls, in the
    /// order they first ran
    models: Vec<(String, Option<ModelCallPurpose>, u32)>,
    tokens: u64,
    cost_usd: f64,
    priced: bool,
}

impl Tally {
    fn add(&mut self, model: &str, purpose: Option<ModelCallPurpose>, call: Option<CallUsage>) {
        let name = upstream_model_name(model);
        match self
            .models
            .iter_mut()
            .find(|(m, p, _)| m == name && *p == purpose)
        {
            Some((_, _, calls)) => *calls += 1,
            None => self.models.push((name.to_string(), purpose, 1)),
        }
        if let Some(call) = call {
            self.tokens += call.tokens;
            if let Some(cost) = call.cost_usd {
                self.cost_usd += cost;
                self.priced = true;
            }
        }
    }

    /// The tally's footer. `routed` says whether Gate picked the turn's
    /// model: only then is the classifier's call Gate's, otherwise it
    /// just labelled the prompt for Gate to learn from.
    fn footer(&self, routed: bool) -> Option<(String, Option<CallUsage>)> {
        if self.models.is_empty() {
            return None;
        }
        let classified = if routed { "gate: " } else { "classify: " };
        let groups = [
            (None, ""),
            (Some(ModelCallPurpose::Classify), classified),
            (Some(ModelCallPurpose::Criteria), "gate: "),
            (Some(ModelCallPurpose::Judge), "gate: "),
            (Some(ModelCallPurpose::Title), "title: "),
        ];
        let mut parts: Vec<String> = Vec::new();
        for (purpose, label) in groups {
            for (model, _, calls) in self.models.iter().filter(|(_, p, _)| *p == purpose) {
                parts.push(match calls {
                    1 => format!("{label}{model}"),
                    n => format!("{label}{model} ×{n}"),
                });
            }
        }
        let usage = (self.tokens > 0).then(|| CallUsage {
            tokens: self.tokens,
            cost_usd: self.priced.then_some(self.cost_usd),
        });
        Some((parts.join(" · "), usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_types::{AgentId, PromptPayload};
    use serde_json::json;

    fn call(agent: AgentId, model: &str, purpose: Option<&str>, output: u64) -> Transaction {
        let mut payload = json!({
            "model": model,
            "input_tokens": 1_000,
            "output_tokens": output,
        });
        if let Some(purpose) = purpose {
            payload["purpose"] = json!(purpose);
        }
        Transaction::new_chained(
            agent,
            TransactionType::Reasoning,
            serde_json::to_vec(&payload).unwrap(),
            None,
        )
    }

    fn prompt(agent: AgentId, route: Route) -> Transaction {
        Transaction::user_prompt_with(
            agent,
            &PromptPayload {
                route,
                ..PromptPayload::new("fix it")
            },
        )
    }

    #[test]
    fn a_turn_counts_its_classifier_its_calls_and_its_title() {
        let agent = AgentId::generate();
        let sonnet = "claude/claude-sonnet-5-5";
        let haiku = "claude/claude-haiku-5-5";
        let mut calls = TurnCalls::default();
        let seen: Vec<Seen> = [
            call(agent, "claude/claude-opus-5-5", None, 10),
            call(agent, haiku, Some("classify"), 1),
            prompt(agent, Route::turn(sonnet, RouteReason::Default)),
            call(agent, sonnet, None, 100),
            call(agent, sonnet, None, 200),
            Transaction::user_prompt_with(agent, &PromptPayload::new("also add a test")),
            Transaction::new_chained(agent, TransactionType::AgentMsg, b"Done".to_vec(), None),
            call(agent, haiku, Some("title"), 5),
        ]
        .iter()
        .map(|tx| calls.see(tx))
        .collect();
        assert_eq!(
            seen,
            [
                Seen::Call,
                Seen::Other,
                Seen::NewTurn,
                Seen::Call,
                Seen::Call,
                Seen::Other,
                Seen::Other,
                Seen::Call
            ]
        );
        let (models, usage) = calls.footer().unwrap();
        assert_eq!(
            models, "claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5 · title: claude-haiku-5-5",
            "the earlier turn's Opus call is not this turn's"
        );
        let usage = usage.unwrap();
        assert_eq!(usage.tokens, 4_000 + 1 + 100 + 200 + 5);
        assert!(usage.cost_usd.is_some_and(|cost| cost > 0.0));

        let unrouted = Transaction::user_prompt_with(agent, &PromptPayload::new("next"));
        assert_eq!(
            calls.see(&unrouted),
            Seen::NewTurn,
            "a prompt without a route after the reply starts a turn"
        );
        assert_eq!(calls.footer(), None);
    }

    #[test]
    fn a_stronger_model_taking_over_carries_the_turn_on() {
        let agent = AgentId::generate();
        let mut calls = TurnCalls::default();
        for tx in [
            prompt(
                agent,
                Route::turn("claude/claude-sonnet-5-5", RouteReason::Default),
            ),
            call(agent, "claude/claude-sonnet-5-5", None, 10),
            call(agent, "claude/claude-opus-5-5", Some("judge"), 10),
            prompt(
                agent,
                Route::turn("claude/claude-opus-5-5", RouteReason::Escalate),
            ),
            call(agent, "claude/claude-opus-5-5", None, 10),
        ] {
            calls.see(&tx);
        }
        assert_eq!(
            calls.footer().unwrap().0,
            "claude-sonnet-5-5 · claude-opus-5-5 · gate: claude-opus-5-5"
        );

        calls.see(&prompt(
            agent,
            Route::turn("local/qwen3", RouteReason::Pinned),
        ));
        assert_eq!(calls.footer(), None, "the next turn starts empty");
        calls.see(&call(agent, "local/qwen3", None, 10));
        let (models, usage) = calls.footer().unwrap();
        assert_eq!(models, "qwen3");
        assert_eq!(usage.unwrap().cost_usd, None, "local calls are not priced");
    }

    #[test]
    fn a_turn_on_a_chosen_model_marks_its_classifier_as_classifying() {
        let agent = AgentId::generate();
        let opus = "claude/claude-opus-5-5";
        let haiku = "claude/claude-haiku-5-5";
        let mut calls = TurnCalls::default();
        for tx in [
            call(agent, haiku, Some("classify"), 1),
            prompt(agent, Route::turn(opus, RouteReason::Pinned)),
            call(agent, opus, None, 100),
            Transaction::user_prompt_with(agent, &PromptPayload::new("also add a test")),
            call(agent, opus, None, 100),
        ] {
            calls.see(&tx);
        }
        assert_eq!(
            calls.footer().unwrap().0,
            "claude-opus-5-5 ×2 · classify: claude-haiku-5-5",
            "Gate didn't pick the model, so its classifier didn't route the turn"
        );

        for tx in [
            Transaction::new_chained(agent, TransactionType::AgentMsg, b"Done".to_vec(), None),
            call(agent, haiku, Some("classify"), 1),
            prompt(agent, Route::turn(opus, RouteReason::Proven)),
            call(agent, opus, None, 100),
        ] {
            calls.see(&tx);
        }
        assert_eq!(
            calls.footer().unwrap().0,
            "claude-opus-5-5 · gate: claude-haiku-5-5",
            "the next turn is routed by Gate"
        );
    }
}
