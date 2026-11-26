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
mod tests;
