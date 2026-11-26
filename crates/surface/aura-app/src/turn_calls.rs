//! The models a turn ran on, for the footer under its reply:
//! `claude-sonnet-5-5 ×2 · high thinking · gate: claude-haiku-5-5`, with
//! the tokens and cost of every call. The chat title's model is left out
//! of the list, but its tokens and cost are counted.
//!
//! The thinking level is the one the turn's own calls recorded asking
//! for, shown as a range (`low–high thinking`) when they differed and
//! left out when none recorded one, as in older records. Gate's and the
//! title's calls think at a level of their own, so theirs is not shown.
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
use aura_model::model_catalog::{backend_for, upstream_model_name};
use aura_model::{ModelCallPurpose, ThinkingEffort};
use aura_ui::events::{CallUsage, SourceUsage};
use serde_json::Value;

/// A turn's footer: its models, what all its calls used, and that split
/// by the sign-in that paid for it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Footer {
    /// `claude-sonnet-5-5 ×2 · high thinking · gate: claude-haiku-5-5`
    pub(crate) models: String,
    /// Tokens and cost of all the turn's calls, when any reported usage
    pub(crate) usage: Option<CallUsage>,
    /// Those tokens per sign-in, in the order each was first used
    pub(crate) sources: Vec<SourceUsage>,
}

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
                let Some(model_call) = model_call(tx) else {
                    return Seen::Other;
                };
                let payload = json_payload(tx);
                let field = |key: &str| payload.as_ref().and_then(|payload| payload.get(key));
                let purpose = field("purpose")
                    .cloned()
                    .and_then(|purpose| serde_json::from_value(purpose).ok());
                let effort = field("thinking_effort")
                    .and_then(Value::as_str)
                    .and_then(ThinkingEffort::from_wire);
                // The requested id names the route, and so who paid;
                // records written before it was kept have only the
                // model the upstream echoed back.
                let route = model_call.endpoint();
                let call = Call {
                    model: &model_call.model,
                    route,
                    purpose,
                    effort,
                    usage: call_usage(&model_call),
                };
                if matches!(
                    purpose,
                    Some(ModelCallPurpose::Classify | ModelCallPurpose::Criteria)
                ) {
                    self.next.add(&call);
                    return Seen::Other;
                }
                self.current.add(&call);
                Seen::Call
            }
            _ => Seen::Other,
        }
    }

    /// The footer of the current turn: its models and the level they
    /// thought at, then the tokens and cost of all its calls. `None`
    /// before its first call.
    pub(crate) fn footer(&self) -> Option<Footer> {
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

/// One model call a `Reasoning` entry records.
struct Call<'a> {
    model: &'a str,
    /// The model id the call asked for, route prefix included; `model`
    /// for records that don't say
    route: &'a str,
    /// What the call was made for; `None` for the turn's own work
    purpose: Option<ModelCallPurpose>,
    /// The thinking level the call asked for, when its entry records one
    effort: Option<ThinkingEffort>,
    usage: Option<CallUsage>,
}

#[derive(Debug, Default)]
struct Tally {
    /// Each model and what it ran for, with its number of calls, in the
    /// order they first ran
    models: Vec<(String, Option<ModelCallPurpose>, u32)>,
    /// The lowest and highest thinking level of the turn's own calls.
    /// Gate's and the title's calls are left out: they run at a level of
    /// their own, not the one the turn was asked to think at.
    thinking: Option<(ThinkingEffort, ThinkingEffort)>,
    /// What all the calls used
    spent: Spent,
    /// What the calls used per sign-in, in the order each was first used
    sources: Vec<(&'static str, Spent)>,
}

/// Tokens and cost of some calls, and how many of them had no known
/// price, so that a partly priced total is not taken for a whole one.
#[derive(Debug, Default, Clone, Copy)]
struct Spent {
    tokens: u64,
    cost_usd: f64,
    /// Whether any call was priced
    priced: bool,
    /// Calls with no known price, which `cost_usd` leaves out
    unpriced_calls: u32,
}

impl Spent {
    fn add(&mut self, usage: CallUsage) {
        self.tokens += usage.tokens;
        if let Some(cost) = usage.cost_usd {
            self.cost_usd += cost;
            self.priced = true;
        }
        self.unpriced_calls = self.unpriced_calls.saturating_add(usage.unpriced_calls);
    }

    /// `None` when the calls used no tokens.
    fn usage(self) -> Option<CallUsage> {
        (self.tokens > 0).then_some(CallUsage {
            tokens: self.tokens,
            cost_usd: self.priced.then_some(self.cost_usd),
            unpriced_calls: self.unpriced_calls,
        })
    }
}

impl Tally {
    fn add(&mut self, call: &Call<'_>) {
        let name = upstream_model_name(call.model);
        match self
            .models
            .iter_mut()
            .find(|(m, p, _)| m == name && *p == call.purpose)
        {
            Some((_, _, calls)) => *calls += 1,
            None => self.models.push((name.to_string(), call.purpose, 1)),
        }
        if let (None, Some(effort)) = (call.purpose, call.effort) {
            self.thinking = Some(match self.thinking {
                Some((lowest, highest)) => (lowest.min(effort), highest.max(effort)),
                None => (effort, effort),
            });
        }
        if let Some(usage) = call.usage {
            self.spent.add(usage);
            let label = backend_for(call.route).source_label();
            match self.sources.iter_mut().find(|(l, _)| *l == label) {
                Some((_, source)) => source.add(usage),
                None => {
                    let mut source = Spent::default();
                    source.add(usage);
                    self.sources.push((label, source));
                }
            }
        }
    }

    /// `high thinking` when the turn's own calls all thought at one
    /// level, `low–high thinking` when they differed, `no thinking` when
    /// every call had it off. `None` when none of them recorded a level,
    /// as in records written before levels were.
    fn thinking_label(&self) -> Option<String> {
        let (lowest, highest) = self.thinking?;
        if highest == ThinkingEffort::Off {
            return Some("no thinking".to_string());
        }
        let (lowest, highest) = (lowest.as_str(), highest.as_str());
        Some(if lowest == highest {
            format!("{lowest} thinking")
        } else {
            format!("{lowest}–{highest} thinking")
        })
    }

    /// The tally's footer. `routed` says whether Gate picked the turn's
    /// model: only then is the classifier's call Gate's, otherwise it
    /// just labelled the prompt for Gate to learn from.
    fn footer(&self, routed: bool) -> Option<Footer> {
        if self.models.is_empty() {
            return None;
        }
        let classified = if routed { "gate: " } else { "classify: " };
        let groups = [
            (None, ""),
            (Some(ModelCallPurpose::Classify), classified),
            (Some(ModelCallPurpose::Criteria), "gate: "),
            (Some(ModelCallPurpose::Judge), "gate: "),
        ];
        let mut parts: Vec<String> = Vec::new();
        for (purpose, label) in groups {
            for (model, _, calls) in self.models.iter().filter(|(_, p, _)| *p == purpose) {
                parts.push(match calls {
                    1 => format!("{label}{model}"),
                    n => format!("{label}{model} ×{n}"),
                });
            }
            // The level the turn thought at goes right after its own
            // models, before Gate's.
            if purpose.is_none() {
                parts.extend(self.thinking_label());
            }
        }
        // Only the chat's title has run so far: there is no model of the
        // turn's own to show yet.
        if parts.is_empty() {
            return None;
        }
        let usage = self.spent.usage();
        let sources = self
            .sources
            .iter()
            .filter_map(|&(label, spent)| {
                let usage = spent.usage()?;
                Some(SourceUsage {
                    label: label.to_string(),
                    tokens: usage.tokens,
                    cost_usd: usage.cost_usd,
                    unpriced_calls: usage.unpriced_calls,
                })
            })
            .collect();
        Some(Footer {
            models: parts.join(" · "),
            usage,
            sources,
        })
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
        let Footer { models, usage, .. } = calls.footer().unwrap();
        assert_eq!(
            models, "claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5",
            "the earlier turn's Opus call is not this turn's, and the title's model is not shown"
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
            calls.footer().unwrap().models,
            "claude-sonnet-5-5 · claude-opus-5-5 · gate: claude-opus-5-5"
        );

        calls.see(&prompt(
            agent,
            Route::turn("local/qwen3", RouteReason::Pinned),
        ));
        assert_eq!(calls.footer(), None, "the next turn starts empty");
        calls.see(&call(agent, "local/qwen3", None, 10));
        let Footer { models, usage, .. } = calls.footer().unwrap();
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
            calls.footer().unwrap().models,
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
            calls.footer().unwrap().models,
            "claude-opus-5-5 · gate: claude-haiku-5-5",
            "the next turn is routed by Gate"
        );
    }

    /// A call recording the thinking level `effort` it asked for.
    fn thinking_call(
        agent: AgentId,
        model: &str,
        purpose: Option<&str>,
        effort: &str,
    ) -> Transaction {
        let mut payload = json!({
            "model": model,
            "input_tokens": 1_000,
            "output_tokens": 10,
            "thinking_effort": effort,
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

    fn footer_of(txs: &[Transaction]) -> String {
        let mut calls = TurnCalls::default();
        for tx in txs {
            calls.see(tx);
        }
        calls.footer().unwrap().models
    }

    #[test]
    fn the_footer_names_the_level_the_turn_thought_at() {
        let agent = AgentId::generate();
        let sonnet = "claude/claude-sonnet-5-5";
        let haiku = "claude/claude-haiku-5-5";
        let routed = || prompt(agent, Route::turn(sonnet, RouteReason::Default));
        let cases = [
            (
                "one level",
                vec![
                    thinking_call(agent, haiku, Some("classify"), "low"),
                    routed(),
                    thinking_call(agent, sonnet, None, "high"),
                    thinking_call(agent, sonnet, None, "high"),
                ],
                "claude-sonnet-5-5 ×2 · high thinking · gate: claude-haiku-5-5",
            ),
            (
                "levels that differ show as a range, lowest first",
                vec![
                    routed(),
                    thinking_call(agent, sonnet, None, "max"),
                    thinking_call(agent, sonnet, None, "minimal"),
                    thinking_call(agent, sonnet, None, "medium"),
                ],
                "claude-sonnet-5-5 ×3 · minimal–max thinking",
            ),
            (
                "Gate's and the title's levels are not the turn's",
                vec![
                    thinking_call(agent, haiku, Some("classify"), "off"),
                    routed(),
                    thinking_call(agent, sonnet, None, "medium"),
                    thinking_call(agent, "claude/claude-opus-5-5", Some("judge"), "max"),
                    thinking_call(agent, haiku, Some("title"), "off"),
                ],
                "claude-sonnet-5-5 · medium thinking · gate: claude-haiku-5-5 \
                 · gate: claude-opus-5-5",
            ),
            (
                "a call that recorded no level leaves the range alone",
                vec![
                    routed(),
                    call(agent, sonnet, None, 10),
                    thinking_call(agent, sonnet, None, "xhigh"),
                ],
                "claude-sonnet-5-5 ×2 · xhigh thinking",
            ),
            (
                "only Gate recorded a level",
                vec![
                    thinking_call(agent, haiku, Some("classify"), "low"),
                    routed(),
                    call(agent, sonnet, None, 10),
                ],
                "claude-sonnet-5-5 · gate: claude-haiku-5-5",
            ),
            (
                "thinking off throughout",
                vec![
                    routed(),
                    thinking_call(agent, sonnet, None, "off"),
                    thinking_call(agent, sonnet, None, "off"),
                ],
                "claude-sonnet-5-5 ×2 · no thinking",
            ),
        ];
        for (case, txs, expected) in cases {
            assert_eq!(footer_of(&txs), expected, "{case}");
        }
    }

    fn routed_call(agent: AgentId, model: &str, route: &str, output: u64) -> Transaction {
        let payload = json!({
            "model": model,
            "route": route,
            "input_tokens": 1_000,
            "output_tokens": output,
        });
        Transaction::new_chained(
            agent,
            TransactionType::Reasoning,
            serde_json::to_vec(&payload).unwrap(),
            None,
        )
    }

    #[test]
    fn a_turn_splits_its_tokens_by_the_sign_in_that_paid() {
        let agent = AgentId::generate();
        let mut calls = TurnCalls::default();
        for tx in [
            prompt(
                agent,
                Route::turn("claude/claude-sonnet-4-6", RouteReason::Pinned),
            ),
            routed_call(agent, "claude-sonnet-4-6", "claude/claude-sonnet-4-6", 100),
            routed_call(
                agent,
                "anthropic/claude-4.5-haiku-20251001",
                "openrouter/anthropic/claude-haiku-4.5",
                10,
            ),
            routed_call(agent, "claude-sonnet-4-6", "claude/claude-sonnet-4-6", 50),
        ] {
            calls.see(&tx);
        }
        let footer = calls.footer().unwrap();
        let labels: Vec<&str> = footer.sources.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Claude plan", "OpenRouter"]);
        assert_eq!(footer.sources[0].tokens, 2_000 + 150);
        assert_eq!(footer.sources[1].tokens, 1_010);
        assert!(
            footer.sources[1].cost_usd.is_some_and(|cost| cost > 0.0),
            "the OpenRouter route is priced by the model it names"
        );
        assert_eq!(
            footer.usage.unwrap().tokens,
            footer.sources.iter().map(|s| s.tokens).sum::<u64>()
        );
    }

    #[test]
    fn a_turn_with_an_unpriced_call_is_partly_priced() {
        let agent = AgentId::generate();
        let mut calls = TurnCalls::default();
        for tx in [
            prompt(
                agent,
                Route::turn("claude/claude-sonnet-4-6", RouteReason::Pinned),
            ),
            routed_call(agent, "claude-sonnet-4-6", "claude/claude-sonnet-4-6", 100),
            routed_call(agent, "mystery/model-x", "openrouter/mystery/model-x", 10),
            call(agent, "local/qwen3:4b", None, 10),
        ] {
            calls.see(&tx);
        }
        let footer = calls.footer().unwrap();
        let usage = footer.usage.unwrap();
        assert!(usage.cost_usd.is_some_and(|cost| cost > 0.0));
        assert_eq!(
            usage.unpriced_calls, 1,
            "the unknown model is left out of the cost; the local one is free"
        );
        let unpriced: Vec<(&str, u32)> = (footer.sources.iter())
            .map(|s| (s.label.as_str(), s.unpriced_calls))
            .collect();
        assert_eq!(
            unpriced,
            [("Claude plan", 0), ("OpenRouter", 1), ("local", 0)]
        );
        assert_eq!(footer.sources[1].cost_usd, None, "nothing it ran is priced");
    }

    #[test]
    fn records_without_a_route_are_put_down_to_the_model_they_name() {
        let agent = AgentId::generate();
        let mut calls = TurnCalls::default();
        for tx in [
            prompt(
                agent,
                Route::turn("aura-claude-sonnet-4-6", RouteReason::Pinned),
            ),
            call(agent, "aura-claude-sonnet-4-6", None, 10),
            call(agent, "local/qwen3:4b", None, 10),
        ] {
            calls.see(&tx);
        }
        let sources = calls.footer().unwrap().sources;
        let labels: Vec<&str> = sources.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["aura-router", "local"]);
        assert_eq!(sources[1].cost_usd, None, "local calls are not priced");
    }
}
