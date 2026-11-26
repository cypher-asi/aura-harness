//! [`StepBuilder`]: agent-loop events to [`Step`]s.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aura_agent::{AgentLoopEvent, AgentLoopResult, DebugEvent};
use aura_config::HarnessParams;
use aura_core_types::trace::{
    Actor, DecisionSource, HarnessAction, HarnessDecision, ModelExchange, Observation, PolicyKind,
    PolicyRef, Step, StepAction, StepLinks,
};
use aura_store_snapshot::FsSnapshotStore;
use serde_json::{json, Value};

use super::touches::file_touches;
use aura_core_types::text::truncate_with_ellipsis;

mod record_steps;

/// Characters of tool output kept inline on a step.
pub const EXCERPT_CHARS: usize = 400;

/// Builds [`Step`]s from agent-loop events.
pub struct StepBuilder {
    run_id: String,
    seq: u64,
    turn: u32,
    iteration: Option<u32>,
    harness_params: Value,
    /// tool_use_id → (model step seq, tool name, input)
    calls: HashMap<String, (u64, String, Value)>,
    /// tool_use_id → unix ms the tool started
    started: HashMap<String, u64>,
    /// tool_use_id → unix ms the kernel recorded its proposal, until the
    /// policy decides it
    proposed: HashMap<String, u64>,
    /// Harness steps since the last model step; they shaped its input.
    pending_causes: Vec<u64>,
    cache_hits: HashSet<String>,
    blocked: HashSet<String>,
    last_model_seq: Option<u64>,
    blobs: Option<Arc<FsSnapshotStore>>,
}

impl StepBuilder {
    /// Builder for `run_id`. `blobs` stores full tool output when set.
    #[must_use]
    pub fn new(
        run_id: impl Into<String>,
        params: &HarnessParams,
        blobs: Option<Arc<FsSnapshotStore>>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            seq: 0,
            turn: 0,
            iteration: None,
            harness_params: serde_json::to_value(params).unwrap_or(Value::Null),
            calls: HashMap::new(),
            started: HashMap::new(),
            proposed: HashMap::new(),
            pending_causes: Vec::new(),
            cache_hits: HashSet::new(),
            blocked: HashSet::new(),
            last_model_seq: None,
            blobs,
        }
    }

    /// Steps emitted so far.
    #[must_use]
    pub const fn last_seq(&self) -> u64 {
        self.seq
    }

    /// Current 1-based turn.
    #[must_use]
    pub const fn turn(&self) -> u32 {
        self.turn
    }

    /// Model step of the latest model call, if any.
    #[must_use]
    pub const fn last_model_seq(&self) -> Option<u64> {
        self.last_model_seq
    }

    fn step(&mut self, actor: Actor, action: StepAction, at_ms: u64) -> Step {
        self.seq += 1;
        Step {
            run_id: self.run_id.clone(),
            seq: self.seq,
            parent: None,
            turn: self.turn.max(1),
            iteration: self.iteration,
            at_ms,
            actor,
            observation: Observation::None,
            action,
            policy: PolicyRef::default(),
            links: StepLinks::default(),
            usage: None,
            duration_ms: None,
        }
    }

    /// A user prompt starts a new turn.
    pub fn user_prompt(&mut self, text: &str, at_ms: u64) -> Step {
        self.turn += 1;
        self.iteration = None;
        self.pending_causes.clear();
        let mut step = self.step(
            Actor::User,
            StepAction::UserPrompt {
                text: text.to_string(),
            },
            at_ms,
        );
        step.policy.kind = PolicyKind::Human;
        step.policy.id = "user".into();
        step
    }

    /// A prompt Gate sent within the turn under way: a follow-up on an
    /// attempt, or a stronger model's takeover.
    pub fn follow_up(&mut self, text: &str, at_ms: u64) -> Step {
        self.iteration = None;
        self.pending_causes.clear();
        let mut step = self.step(
            Actor::Harness {
                source: DecisionSource::Gate,
            },
            StepAction::UserPrompt {
                text: text.to_string(),
            },
            at_ms,
        );
        step.policy.kind = PolicyKind::Harness;
        step.policy.id = DecisionSource::Gate.as_str().into();
        step
    }

    /// Translate one loop event. Most UI events produce nothing.
    pub fn on_event(&mut self, event: &AgentLoopEvent, at_ms: u64) -> Option<Step> {
        match event {
            AgentLoopEvent::ToolStart { id, .. } => {
                self.started.insert(id.clone(), at_ms);
                None
            }
            AgentLoopEvent::ToolResult {
                tool_use_id,
                tool_name,
                content,
                is_error,
                ..
            } => Some(self.tool_result(tool_use_id, tool_name, content, *is_error, at_ms)),
            AgentLoopEvent::StreamReset { reason, .. } => Some(self.harness(
                &HarnessDecision::new(
                    DecisionSource::Retry,
                    "stream_reset",
                    HarnessAction::Retry {
                        attempt: 0,
                        wait_ms: 0,
                        reason: reason.clone(),
                    },
                ),
                at_ms,
            )),
            AgentLoopEvent::TestSuiteWarning {
                passed,
                summary,
                failed_tests,
            } => Some(
                self.harness(
                    &HarnessDecision::new(
                        DecisionSource::SelfReview,
                        "test_suite",
                        HarnessAction::Build {
                            success: *passed,
                            error_count: failed_tests.len(),
                        },
                    )
                    .with_trigger(json!({ "summary": summary, "failed_tests": failed_tests })),
                    at_ms,
                ),
            ),
            AgentLoopEvent::Debug(debug) => self.on_debug(debug, at_ms),
            _ => None,
        }
    }

    fn on_debug(&mut self, event: &DebugEvent, at_ms: u64) -> Option<Step> {
        match event {
            DebugEvent::ModelExchange { exchange, .. } => Some(self.model(exchange, at_ms)),
            DebugEvent::Decision { decision, .. } => Some(self.harness(decision, at_ms)),
            DebugEvent::Retry {
                reason,
                attempt,
                wait_ms,
                ..
            } => Some(self.harness(
                &HarnessDecision::new(
                    DecisionSource::Retry,
                    "provider_retry",
                    HarnessAction::Retry {
                        attempt: *attempt,
                        wait_ms: *wait_ms,
                        reason: reason.clone(),
                    },
                ),
                at_ms,
            )),
            DebugEvent::Blocker {
                kind,
                path,
                message,
                ..
            } => {
                let mut decision = HarnessDecision::new(
                    DecisionSource::Policy,
                    kind.clone(),
                    HarnessAction::Block {
                        tool: String::new(),
                        tool_use_id: None,
                        reason: message.clone(),
                    },
                );
                if let Some(path) = path {
                    decision = decision.with_file(path.clone());
                }
                Some(self.harness(&decision, at_ms))
            }
            DebugEvent::LlmCall { .. } | DebugEvent::Iteration { .. } => None,
        }
    }

    fn model(&mut self, exchange: &ModelExchange, at_ms: u64) -> Step {
        self.iteration = Some(exchange.iteration);
        let mut step = self.step(
            Actor::Model,
            StepAction::Model(exchange.action.clone()),
            at_ms,
        );
        let seq = step.seq;
        for call in &exchange.action.tool_calls {
            self.calls.insert(
                call.tool_use_id.clone(),
                (seq, call.name.clone(), call.input.clone()),
            );
            step.links.tool_use_ids.push(call.tool_use_id.clone());
        }
        step.links.caused_by = std::mem::take(&mut self.pending_causes);
        step.policy = PolicyRef {
            kind: PolicyKind::Model,
            id: exchange.model.clone(),
            version: String::new(),
            params: exchange
                .observation
                .as_ref()
                .map_or(Value::Null, |o| o.params.clone()),
            propensity: None,
        };
        if let Some(obs) = &exchange.observation {
            step.observation = Observation::Model(obs.clone());
        }
        step.usage = Some(exchange.usage);
        step.duration_ms = Some(exchange.latency_ms);
        self.last_model_seq = Some(seq);
        step
    }

    fn harness(&mut self, decision: &HarnessDecision, at_ms: u64) -> Step {
        if decision.iteration.is_some() {
            self.iteration = decision.iteration;
        }
        let mut step = self.step(
            Actor::Harness {
                source: decision.source,
            },
            StepAction::Harness {
                name: decision.name.clone(),
                action: decision.action.clone(),
                alternatives: decision.alternatives.clone(),
            },
            at_ms,
        );
        step.observation = Observation::Harness {
            features: decision.features.clone(),
            trigger: decision.trigger.clone(),
        };
        step.policy = PolicyRef {
            kind: PolicyKind::Harness,
            id: decision.name.clone(),
            version: decision.version.to_string(),
            params: self.harness_params.clone(),
            propensity: None,
        };
        step.links = decision.links.clone();
        match &decision.action {
            HarnessAction::CacheHit { tool_use_id, .. } => {
                self.cache_hits.insert(tool_use_id.clone());
            }
            HarnessAction::Block {
                tool_use_id: Some(id),
                ..
            } => {
                self.blocked.insert(id.clone());
            }
            _ => {}
        }
        step.parent = step
            .links
            .tool_use_ids
            .first()
            .and_then(|id| self.calls.get(id))
            .map(|(seq, _, _)| *seq);
        if step.parent.is_none() {
            self.pending_causes.push(step.seq);
        }
        step
    }

    fn tool_result(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        content: &str,
        is_error: bool,
        at_ms: u64,
    ) -> Step {
        let (parent, input) = match self.calls.get(tool_use_id) {
            Some((seq, _, input)) => (Some(*seq), input.clone()),
            None => (None, Value::Null),
        };
        let source = if self.blocked.contains(tool_use_id) {
            "blocked"
        } else if self.cache_hits.contains(tool_use_id) {
            "cache"
        } else {
            "executor"
        };
        let files = file_touches(tool_name, &input, is_error);
        let content_blob = self.blobs.as_ref().map(|store| {
            store
                .put_bytes(content.as_bytes())
                .unwrap_or_else(|_| aura_core_types::trace::blob_id(content.as_bytes()))
        });
        let mut step = self.step(
            Actor::Environment,
            StepAction::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                tool: tool_name.to_string(),
                is_error,
                source: source.to_string(),
                content_chars: content.chars().count(),
                excerpt: truncate_with_ellipsis(content, EXCERPT_CHARS),
                content: content_blob,
                files: files.clone(),
            },
            at_ms,
        );
        step.parent = parent;
        step.policy.id = tool_name.to_string();
        step.links.tool_use_ids.push(tool_use_id.to_string());
        step.links.files = files.into_iter().map(|f| f.path).collect();
        step.duration_ms = self
            .started
            .remove(tool_use_id)
            .map(|start| at_ms.saturating_sub(start));
        step
    }

    /// The loop finished a turn.
    pub fn turn_end(&mut self, result: &AgentLoopResult, at_ms: u64) -> Step {
        let mut step = self.step(
            Actor::Environment,
            StepAction::TurnEnd {
                stop_reason: result.stop_reason().to_string(),
                iterations: result.iterations,
                input_tokens: result.total_input_tokens,
                output_tokens: result.total_output_tokens,
                files_changed: result.file_changes.len(),
                error: result.llm_error.clone(),
            },
            at_ms,
        );
        step.links.files = result.file_changes.iter().map(|c| c.path.clone()).collect();
        step.policy.id = "agent_loop".into();
        self.calls.clear();
        self.started.clear();
        self.proposed.clear();
        self.pending_causes.clear();
        step
    }
}
