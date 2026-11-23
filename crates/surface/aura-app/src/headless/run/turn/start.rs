//! What a turn runs on and is asked: the model Gate routes it to, or that
//! of the turn whose model it asks to check its change; whether it is
//! led; and its prompt, recorded and given to the model.

use super::Carry;
use crate::event_loop::processes::now_ms;
use crate::event_loop::turns::record_prompt;
use crate::headless::run::drive::Driving;
use crate::headless::run::route::Prompted;
use aura_agent::{AgentLoop, AgentLoopConfig};
use aura_core_types::{Hash, OutcomePayload, PromptCategory, PromptPayload, RouteReason};
use aura_gate::attempts::Terms;
use aura_model::{Message, ToolDefinition};

/// A turn about to run, and what its `Outcome` needs.
pub(super) struct Started {
    pub(super) agent_loop: AgentLoop,
    /// What its work asks for, when Gate routed it
    pub(super) category: Option<PromptCategory>,
    /// The recorded prompt it answers
    pub(super) prompt_tx: Hash,
    /// The `Outcome` of the turn whose model it asks to check its change,
    /// whose place it takes
    pub(super) nudged: Option<OutcomePayload>,
    pub(super) terms: Terms,
    /// Where its own messages start in the session
    pub(super) turn_start: usize,
    /// When its prompt was recorded, in Unix milliseconds
    pub(super) started_ms: u64,
}

impl Driving<'_> {
    /// Set up the turn sending `text`, carrying on as `carry` says: its
    /// model (see [`Self::prompted`]), whether it is led, its prompt
    /// recorded and given to the model. Returns it with the tools it may
    /// use.
    ///
    /// # Errors
    /// Says why when Gate could not classify the prompt, or the prompt
    /// could not be recorded.
    pub(super) async fn start_turn(
        &mut self,
        text: String,
        carry: Option<Carry>,
    ) -> Result<(Started, Vec<ToolDefinition>), String> {
        let (mut config, mut tools, lead) = self.headless.hub.user_turn(self.session);
        if let Some(max) = self.run.limits.max_iterations {
            config.max_iterations = max;
        }
        let carried = carry.is_some();
        let earlier = carry.as_ref().map(Carry::earlier);
        let (nudged, (mut payload, category, told)) =
            self.prompted(&mut config, text, carry).await?;
        let difficulty = payload
            .difficulty
            .or(earlier.and_then(|terms| terms.difficulty));
        if let (Some(lead), Some(category)) = (lead, category) {
            let explore = payload.route.reason != RouteReason::Escalate && nudged.is_none();
            let kept = earlier
                .filter(|_| nudged.is_some())
                .and_then(|terms| terms.led);
            let led = kept.unwrap_or_else(|| {
                let gate = &self.headless.hub.gate;
                gate.way((category, difficulty), &config.model, explore).led
            });
            self.session.lead = lead.apply(led, &mut config, &mut tools);
            payload.led = Some(led);
        }
        let terms = Terms {
            difficulty,
            led: payload.led,
            routed: category.is_some(),
        };
        let started_ms = now_ms();
        let (prompt, _) = record_prompt(self.session, &payload).await?;
        let turn_start = self.session.messages.len();
        let agent_loop = self.give(config, payload.text, told, carried);
        let started = Started {
            agent_loop,
            category,
            prompt_tx: prompt.hash,
            nudged,
            terms,
            turn_start,
            started_ms,
        };
        Ok((started, self.headless.tool_filter.filter(tools)))
    }

    /// The prompt of the turn sending `text`, carrying on as `carry` says:
    /// a turn whose model is asked to check its change stays on that
    /// model, with what its work asks for, and also returns the `Outcome`
    /// of the turn it takes the place of; any other is routed (see
    /// [`Self::route_turn`]).
    async fn prompted(
        &mut self,
        config: &mut AgentLoopConfig,
        text: String,
        carry: Option<Carry>,
    ) -> Result<(Option<OutcomePayload>, Prompted), String> {
        let escalation = match carry {
            Some(Carry::Nudge(ended, _)) => {
                if config.model != ended.model {
                    self.headless
                        .hub
                        .model_limits()
                        .configure(config, &ended.model);
                }
                let prompted = (PromptPayload::new(text), Some(ended.category), None);
                return Ok((Some(ended), prompted));
            }
            Some(Carry::Escalate(decision, category, _)) => Some((decision, category)),
            None => None,
        };
        Ok((None, self.route_turn(config, text, escalation).await?))
    }

    /// Give the model `text`, the turn's prompt, with `told` after it, and
    /// return the agent loop the turn runs on: `config`, traced when the
    /// run is, starting from the session's todo list and, for a turn that
    /// carries on from the one before (`carried`), at the effort that
    /// turn's requests were held at.
    fn give(
        &mut self,
        mut config: AgentLoopConfig,
        mut text: String,
        told: Option<String>,
        carried: bool,
    ) -> AgentLoop {
        if let Some(trace) = &self.headless.trace {
            config.trace_blobs = trace.blob_store();
            trace.begin_turn(&text);
        }
        if let Some(told) = told {
            text.push_str(&told);
        }
        self.session.messages.push(Message::user(text));
        let (agent, name) = (self.session.agent_id(), &self.session.identity.name);
        let workspace = self.session.kernel.agent_workspace();
        self.headless
            .hub
            .register_root(agent, name, workspace, &config);
        config.initial_todos.clone_from(&self.session.todos);
        if carried {
            config.initial_thinking_effort = self.headless.held_effort;
        }
        self.headless.held_effort = config.held_thinking_effort();
        AgentLoop::new(config)
    }
}
