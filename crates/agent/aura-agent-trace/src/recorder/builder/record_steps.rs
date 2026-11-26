//! Steps built from kernel record rows: Gate calls, routes and
//! approval waits.

use aura_core_types::trace::{
    Actor, DecisionSource, HarnessAction, HarnessDecision, ModelAction, PolicyKind, PolicyRef,
    Step, StepAction,
};
use serde_json::json;

use super::StepBuilder;
use crate::health::{duration, APPROVAL_GUESS_MS, DEFAULT_SLOW_MS};
use crate::records::{CallRow, RecordRow, RouteRow};

impl StepBuilder {
    /// Translate one kernel record row. A call Gate made becomes a model
    /// step of its own, and where it sent a prompt's work a `route`
    /// decision. A policy decision that waited for an approval becomes an
    /// `approval_wait` step: allowed, flagged when the wait was long, or
    /// blocked when the call was denied.
    pub fn on_record(&mut self, row: &RecordRow) -> Option<Step> {
        if let Some(call) = &row.call {
            return GATE_CALLS
                .contains(&call.purpose.as_str())
                .then(|| self.gate_call(row.seq, call, row.at_ms));
        }
        if let Some(route) = &row.route {
            return Some(self.route(row.seq, route, row.at_ms));
        }
        let id = row.tool_use_id.as_deref()?;
        match row.tx_type.as_str() {
            "tool_proposal" => {
                self.proposed.insert(id.to_string(), row.at_ms);
                None
            }
            "policy_decision" => {
                let wait_ms = row.at_ms.saturating_sub(self.proposed.remove(id)?);
                if row.approver.is_none() && wait_ms < APPROVAL_GUESS_MS {
                    return None;
                }
                let tool = row.tool.clone().unwrap_or_default();
                let waited = duration(wait_ms);
                let action = if row.verdict.as_deref() == Some("denied") {
                    HarnessAction::Block {
                        tool: tool.clone(),
                        tool_use_id: Some(id.to_string()),
                        reason: format!("denied after waiting {waited} for approval"),
                    }
                } else if wait_ms >= DEFAULT_SLOW_MS {
                    HarnessAction::Warn {
                        message: format!("{tool} waited {waited} for approval"),
                    }
                } else {
                    HarnessAction::Allow
                };
                let decision =
                    HarnessDecision::new(DecisionSource::Policy, "approval_wait", action)
                        .with_tool_use(id)
                        .with_trigger(json!({
                            "tool": tool,
                            "wait_ms": wait_ms,
                            "approver": row.approver,
                            "verdict": row.verdict,
                        }));
                Some(self.harness(&decision, row.at_ms))
            }
            _ => None,
        }
    }

    fn gate_call(&mut self, record_seq: u64, call: &CallRow, at_ms: u64) -> Step {
        let mut step = self.step(
            Actor::Harness {
                source: DecisionSource::Gate,
            },
            StepAction::Model(ModelAction {
                stop_reason: call.stop_reason.clone(),
                purpose: Some(call.purpose.clone()),
                error: call.error.clone(),
                ..ModelAction::default()
            }),
            at_ms,
        );
        step.iteration = None;
        step.policy = PolicyRef {
            kind: PolicyKind::Model,
            id: call.model.clone(),
            ..PolicyRef::default()
        };
        step.links.record_seqs.push(record_seq);
        step.usage = call.usage;
        step.duration_ms = call.duration_ms;
        step
    }

    fn route(&mut self, record_seq: u64, route: &RouteRow, at_ms: u64) -> Step {
        let decision = HarnessDecision::new(
            DecisionSource::Gate,
            "route",
            HarnessAction::Route {
                model: route.model.clone(),
                effort: route.effort.clone(),
            },
        )
        .with_features(json!({
            "category": route.category,
            "difficulty": route.difficulty,
        }))
        .with_trigger(json!({
            "reason": route.reason,
            "explored": route.explored,
        }));
        let mut step = self.harness(&decision, at_ms);
        step.iteration = None;
        step.links.record_seqs.push(record_seq);
        step
    }
}

/// Purposes of the calls Gate makes: labelling a prompt, writing down
/// what its work will be checked against, and reviewing or judging it.
const GATE_CALLS: &[&str] = &["classify", "criteria", "judge"];
