use super::*;

impl AgentHub {
    /// What Gate routes the work of an agent running the permanent agent
    /// `definition` among: `None` when the user pinned the main model or
    /// the definition names its own.
    pub(crate) fn gate_candidates(&self, definition: Option<&str>) -> Option<Vec<Candidate>> {
        let candidates = lock(&self.base).gate.candidates.clone()?;
        let pinned = definition.is_some_and(|name| {
            lock(&self.definitions)
                .get(name)
                .is_some_and(|d| d.model.is_some())
        });
        (!pinned).then_some(candidates)
    }

    /// The model that judges `ended`, the outcome of work Gate routed for
    /// an agent running the permanent agent `definition` (see
    /// [`live::judge_for`]).
    pub(crate) fn judge_of(
        &self,
        definition: Option<&str>,
        ended: &OutcomePayload,
    ) -> Option<String> {
        let candidates = self.gate_candidates(definition)?;
        live::judge_for(&self.gate, &candidates, ended)
    }

    /// The model that classifies prompts; empty when nothing does.
    pub(crate) fn classifier(&self) -> String {
        lock(&self.base).gate.classifier.clone()
    }

    /// For pointing an agent loop at a model other than the selected one.
    pub(crate) fn model_limits(&self) -> ModelLimits {
        lock(&self.base).limits
    }

    /// Tokens and cost per model and purpose of every model call `agent`
    /// and the agents under it made, classifier calls included.
    pub(crate) fn tokens_by_model(&self, agent: AgentId) -> Vec<ModelTokens> {
        let mut calls = Vec::new();
        self.each_entry(agent, |entry| {
            calls.extend(crate::record_loader::model_call(&entry.tx));
        });
        tally(calls)
    }

    /// [`Self::tokens_by_model`] of `agent`'s own calls alone, made for
    /// `purpose`, leaving out the agents under it.
    pub(crate) fn own_tokens_for(
        &self,
        agent: AgentId,
        purpose: ModelCallPurpose,
    ) -> Vec<ModelTokens> {
        let mut calls = Vec::new();
        self.each_entry_of(vec![agent], |entry| {
            let call = crate::record_loader::model_call(&entry.tx);
            calls.extend(call.filter(|call| call.purpose == Some(purpose)));
        });
        tally(calls)
    }

    /// The plan usage every model call of `agent` and the agents under it
    /// reported, with when each call was recorded.
    pub(crate) fn plan_readings(&self, agent: AgentId) -> Vec<PlanReading> {
        let mut readings = Vec::new();
        self.each_entry(agent, |entry| {
            let recorded_ms = entry.tx.ts_ms;
            readings.extend(
                crate::record_loader::plan_usage(&entry.tx)
                    .into_iter()
                    .map(|usage| PlanReading { recorded_ms, usage }),
            );
        });
        readings
    }

    /// The models the prompts of `agent` and the agents under it were
    /// routed to, first used first.
    pub(crate) fn routed_models(&self, agent: AgentId) -> Vec<String> {
        let mut models: Vec<String> = Vec::new();
        self.each_entry(agent, |entry| {
            if let Some(prompt) = entry.tx.prompt_payload() {
                let model = prompt.route.model;
                if !model.is_empty() && !models.contains(&model) {
                    models.push(model);
                }
            }
        });
        models
    }

    /// List-price cost in USD of the model calls the agents under `agent`
    /// made from `since_ms` on: what the subtasks of its turn that
    /// started then cost.
    pub(crate) fn subtask_cost_usd(&self, agent: AgentId, since_ms: u64) -> f64 {
        let below = lock(&self.registry).descendants(agent);
        let mut cost_usd = 0.0;
        self.each_entry_of(below, |entry| {
            if entry.tx.ts_ms < since_ms {
                return;
            }
            if let Some(call) = crate::record_loader::model_call(&entry.tx) {
                cost_usd += call.list_price().unwrap_or(0.0);
            }
        });
        cost_usd
    }

    /// Estimated cost in USD, at API list prices as the agent's own calls
    /// are, of the calls Gate made for `agent`'s turns from `since_ms` on:
    /// classifying their prompts, writing down what the work is checked
    /// against, and judging and reviewing it. `None` when none of them
    /// had a price.
    pub(crate) fn gate_cost_usd(&self, agent: AgentId, since_ms: u64) -> Option<f64> {
        use aura_model::ModelCallPurpose::{Classify, Criteria, Judge};
        let mut cost_usd = None;
        self.each_entry_of(vec![agent], |entry| {
            let gates = matches!(
                crate::record_loader::call_purpose(&entry.tx),
                Some(Classify | Criteria | Judge)
            );
            if entry.tx.ts_ms < since_ms || !gates {
                return;
            }
            let priced = crate::record_loader::model_call(&entry.tx)
                .and_then(|call| call.estimate_cost_usd());
            if let Some(price) = priced {
                *cost_usd.get_or_insert(0.0) += price;
            }
        });
        cost_usd
    }

    /// Call `f` with every entry in the records of `agent` and the agents
    /// under it, `agent`'s first.
    pub(super) fn each_entry(&self, agent: AgentId, f: impl FnMut(&RecordEntry)) {
        let agents: Vec<AgentId> = std::iter::once(agent)
            .chain(lock(&self.registry).descendants(agent))
            .collect();
        self.each_entry_of(agents, f);
    }

    /// Call `f` with every entry in the records of `agents`, in order.
    pub(super) fn each_entry_of(&self, agents: Vec<AgentId>, mut f: impl FnMut(&RecordEntry)) {
        let store = self.factory.store.as_ref();
        for agent in agents {
            let mut from = 1;
            loop {
                let entries = match store.scan_record(agent, from, SCAN_BATCH) {
                    Ok(entries) => entries,
                    Err(e) => {
                        warn!(%agent, error = %e, "Could not read an agent's record");
                        break;
                    }
                };
                entries.iter().for_each(&mut f);
                match entries.last() {
                    Some(last) if entries.len() == SCAN_BATCH => from = last.seq + 1,
                    _ => break,
                }
            }
        }
    }
}
