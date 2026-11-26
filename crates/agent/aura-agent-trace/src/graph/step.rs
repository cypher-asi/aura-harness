//! Applying trace steps: one handler per [`StepAction`] variant.

use aura_core_types::trace::{
    Actor, FileTouch, HarnessAction, ModelAction, Observation, Step, StepAction, ToolIntent,
};
use serde_json::{json, Map, Value};

use aura_model::model_catalog::{call_tokens, estimate_cost_usd};
use aura_model::Usage;

use super::{
    args_summary, decision_detail, first_line, intent_mix, secs, tokens, EdgeKind, GraphNode,
    NodeKind, NodeStatus, RunGraph,
};
use crate::recorder::excerpt;

/// Token and cost figures of one model call.
struct CallFigures {
    usage: aura_core_types::trace::TokenUsage,
    prompt_tokens: u64,
    cost: Option<f64>,
    truncated: bool,
    intents: Vec<(String, usize)>,
}

impl RunGraph {
    /// Add one step.
    pub fn apply(&mut self, step: &Step) {
        self.apply_action(step);
        if step.links.record_seqs.is_empty() {
            return;
        }
        if let Some(id) = self.by_seq.get(&step.seq).cloned() {
            if let Some(node) = self.node_mut(&id) {
                node.meta
                    .insert("record_seqs".into(), json!(step.links.record_seqs));
            }
        }
    }

    fn apply_action(&mut self, step: &Step) {
        match &step.action {
            StepAction::UserPrompt { text } if matches!(step.actor, Actor::Harness { .. }) => {
                self.on_follow_up(step, text);
            }
            StepAction::UserPrompt { text } => self.on_user_prompt(step, text),
            StepAction::Model(action) => self.on_model(step, action),
            StepAction::ToolResult { .. } => self.on_tool_result(step),
            StepAction::Harness {
                name,
                action,
                alternatives,
            } => self.on_harness(step, name, action, alternatives),
            StepAction::TurnEnd { .. } => self.on_turn_end(step),
        }
    }

    fn on_follow_up(&mut self, step: &Step, text: &str) {
        let parent = self.turn_parent(step);
        let mut meta = Map::new();
        meta.insert("seq".into(), json!(step.seq));
        meta.insert("at_ms".into(), json!(step.at_ms));
        meta.insert("prompt".into(), json!(text));
        self.insert(GraphNode {
            id: format!("s{}", step.seq),
            kind: NodeKind::Decision,
            parent: Some(parent),
            label: "follow_up".into(),
            detail: excerpt(first_line(text), 60),
            status: NodeStatus::Warn,
            seq: Some(step.seq),
            meta,
            children: Vec::new(),
        });
    }

    fn on_user_prompt(&mut self, step: &Step, text: &str) {
        let id = format!("t{}", step.turn);
        let mut meta = Map::new();
        meta.insert("prompt".into(), json!(text));
        meta.insert("at_ms".into(), json!(step.at_ms));
        self.insert(GraphNode {
            id: id.clone(),
            kind: NodeKind::Turn,
            parent: Some("run".into()),
            label: format!("T{} \"{}\"", step.turn, excerpt(first_line(text), 60)),
            detail: String::new(),
            status: NodeStatus::Pending,
            seq: Some(step.seq),
            meta,
            children: Vec::new(),
        });
        self.current_turn = Some(id);
        self.last_iteration = None;
    }

    fn on_model(&mut self, step: &Step, action: &ModelAction) {
        let parent = self.turn_parent(step);
        let id = format!("s{}", step.seq);
        let figures = call_figures(step, action);
        let meta = model_meta(step, action, &figures);
        let n = step.iteration.map_or(step.seq, |i| u64::from(i) + 1);
        self.insert(GraphNode {
            id: id.clone(),
            kind: NodeKind::Iteration,
            parent: Some(parent),
            label: action.purpose.as_ref().map_or_else(
                || format!("I{n} model"),
                |purpose| format!("gate {purpose}"),
            ),
            detail: model_detail(step, action, &figures),
            status: if action.error.is_some() {
                NodeStatus::Error
            } else if figures.truncated {
                NodeStatus::Warn
            } else {
                NodeStatus::Ok
            },
            seq: Some(step.seq),
            meta,
            children: Vec::new(),
        });
        if let Some(prev) = self.last_iteration.replace(id.clone()) {
            self.edge(&prev, &id, EdgeKind::Next);
        }
        for cause in &step.links.caused_by {
            if let Some(from) = self.by_seq.get(cause).cloned() {
                self.edge(&from, &id, EdgeKind::Triggered);
            }
        }
        self.add_tool_calls(step, action, &id);
    }

    fn add_tool_calls(&mut self, step: &Step, action: &ModelAction, id: &str) {
        for call in &action.tool_calls {
            let mut m = Map::new();
            m.insert("tool_use_id".into(), json!(call.tool_use_id));
            m.insert("tool".into(), json!(call.name));
            m.insert("input".into(), call.input.clone());
            m.insert("intent".into(), json!(call.intent));
            m.insert("at_ms".into(), json!(step.at_ms));
            let call_id = format!("tc:{}", call.tool_use_id);
            self.insert(GraphNode {
                id: call_id.clone(),
                kind: if call.intent == ToolIntent::Delegate {
                    NodeKind::Subagent
                } else {
                    NodeKind::ToolCall
                },
                parent: Some(id.to_string()),
                label: format!("{} {}", call.name, args_summary(&call.input)),
                detail: "running".into(),
                status: NodeStatus::Pending,
                seq: None,
                meta: m,
                children: Vec::new(),
            });
            if let Some(policy) = self.pending_policy.remove(&call_id) {
                self.set_policy(&call_id, policy);
            }
        }
    }

    fn on_tool_result(&mut self, step: &Step) {
        let StepAction::ToolResult {
            tool_use_id,
            tool,
            is_error,
            source,
            content_chars,
            excerpt: text,
            content,
            files,
        } = &step.action
        else {
            return;
        };
        let id = format!("tc:{tool_use_id}");
        self.ensure_tool_node(step, &id, tool);
        self.by_seq.insert(step.seq, id.clone());
        let flags = result_flags(step, *is_error, source, files);
        if let Some(node) = self.node_mut(&id) {
            node.seq = Some(step.seq);
            node.status = if *is_error {
                NodeStatus::Error
            } else if source == "blocked" {
                NodeStatus::Warn
            } else {
                NodeStatus::Ok
            };
            node.detail = flags.join("  ");
            node.meta.insert("result_seq".into(), json!(step.seq));
            node.meta.insert("ended_at_ms".into(), json!(step.at_ms));
            node.meta.insert("is_error".into(), json!(is_error));
            node.meta.insert("source".into(), json!(source));
            node.meta
                .insert("content_chars".into(), json!(content_chars));
            node.meta.insert("excerpt".into(), json!(text));
            node.meta.insert("files".into(), json!(files));
            node.meta
                .insert("duration_ms".into(), json!(step.duration_ms));
            if let Some(blob) = content {
                node.meta.insert("content_blob".into(), json!(blob));
            }
        }
        for f in files {
            if let Some(prev) = self.last_touch.insert(f.path.clone(), id.clone()) {
                if prev != id {
                    self.edge(&prev, &id, EdgeKind::Touched);
                }
            }
        }
    }

    /// A result whose call was never seen still gets a node.
    fn ensure_tool_node(&mut self, step: &Step, id: &str, tool: &str) {
        if self.node(id).is_some() {
            return;
        }
        let parent = self
            .last_iteration
            .clone()
            .unwrap_or_else(|| self.turn_parent(step));
        self.insert(GraphNode {
            id: id.to_string(),
            kind: NodeKind::ToolCall,
            parent: Some(parent),
            label: tool.to_string(),
            detail: String::new(),
            status: NodeStatus::Pending,
            seq: None,
            meta: Map::new(),
            children: Vec::new(),
        });
        if let Some(policy) = self.pending_policy.remove(id) {
            self.set_policy(id, policy);
        }
    }

    fn on_harness(
        &mut self,
        step: &Step,
        name: &str,
        action: &HarnessAction,
        alternatives: &[String],
    ) {
        let parent = step
            .parent
            .and_then(|p| self.by_seq.get(&p).cloned())
            .or_else(|| self.current_turn.clone())
            .unwrap_or_else(|| self.turn_parent(step));
        let id = format!("s{}", step.seq);
        self.insert(GraphNode {
            id: id.clone(),
            kind: NodeKind::Decision,
            parent: Some(parent),
            label: name.to_string(),
            detail: decision_detail(action),
            status: harness_status(action),
            seq: Some(step.seq),
            meta: harness_meta(step, name, action, alternatives),
            children: Vec::new(),
        });
        for tool_use_id in &step.links.tool_use_ids {
            let target = format!("tc:{tool_use_id}");
            if self.node(&target).is_some() {
                self.edge(&id, &target, EdgeKind::Triggered);
            }
        }
    }

    fn on_turn_end(&mut self, step: &Step) {
        let StepAction::TurnEnd {
            stop_reason,
            iterations,
            input_tokens,
            output_tokens,
            files_changed,
            error,
        } = &step.action
        else {
            return;
        };
        let turn = self.turn_parent(step);
        self.by_seq.insert(step.seq, turn.clone());
        if let Some(error) = error {
            self.failed_call(&turn, step, error);
        }
        let cost = self.cost_of(|n| n.parent.as_deref() == Some(turn.as_str()));
        if let Some(node) = self.node_mut(&turn) {
            node.status = if matches!(stop_reason.as_str(), "end_turn" | "task_done") {
                NodeStatus::Ok
            } else {
                NodeStatus::Error
            };
            node.detail = format!(
                "{stop_reason}  {}  {iterations} calls  {files_changed} files{}",
                tokens(input_tokens + output_tokens),
                cost.map(|c| format!("  ${c:.4}")).unwrap_or_default()
            );
            let started = node.meta.get("at_ms").and_then(Value::as_u64);
            node.meta.insert("stop_reason".into(), json!(stop_reason));
            node.meta.insert("error".into(), json!(error));
            node.meta.insert("iterations".into(), json!(iterations));
            node.meta.insert("input_tokens".into(), json!(input_tokens));
            node.meta
                .insert("output_tokens".into(), json!(output_tokens));
            node.meta.insert("cost_usd".into(), json!(cost));
            node.meta.insert("ended_at_ms".into(), json!(step.at_ms));
            node.meta.insert(
                "duration_ms".into(),
                json!(started.map(|s| step.at_ms.saturating_sub(s))),
            );
            node.meta.insert("files".into(), json!(step.links.files));
        }
        let run_cost = self.cost_of(|_| true);
        if let Some(run) = self.node_mut("run") {
            run.meta.insert("cost_usd".into(), json!(run_cost));
        }
        self.current_turn = None;
        self.last_iteration = None;
    }
}

fn call_figures(step: &Step, action: &ModelAction) -> CallFigures {
    let usage = step.usage.unwrap_or_default();
    let billed = Usage {
        input_tokens: usage.input,
        output_tokens: usage.output,
        cache_creation_input_tokens: Some(usage.cache_write),
        cache_read_input_tokens: Some(usage.cache_read),
    };
    let total = call_tokens(&step.policy.id, &billed);
    CallFigures {
        usage,
        prompt_tokens: total.saturating_sub(usage.output),
        cost: estimate_cost_usd(&step.policy.id, &billed),
        truncated: action.stop_reason == "max_tokens",
        intents: intent_mix(action.tool_calls.iter().map(|c| c.intent)),
    }
}

fn model_meta(step: &Step, action: &ModelAction, figures: &CallFigures) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("seq".into(), json!(step.seq));
    meta.insert("at_ms".into(), json!(step.at_ms));
    meta.insert("iteration".into(), json!(step.iteration));
    meta.insert("model".into(), json!(step.policy.id));
    meta.insert("params".into(), step.policy.params.clone());
    meta.insert("usage".into(), json!(figures.usage));
    meta.insert("prompt_tokens".into(), json!(figures.prompt_tokens));
    meta.insert("cost_usd".into(), json!(figures.cost));
    meta.insert("latency_ms".into(), json!(step.duration_ms));
    meta.insert("stop_reason".into(), json!(action.stop_reason));
    meta.insert("thinking_chars".into(), json!(action.thinking_chars));
    if !action.thinking_text.is_empty() {
        meta.insert(
            "thinking".into(),
            json!(excerpt(&action.thinking_text, 4000)),
        );
    }
    meta.insert("text".into(), json!(excerpt(&action.text, 2000)));
    meta.insert("intents".into(), json!(figures.intents));
    if let Observation::Model(obs) = &step.observation {
        meta.insert("observation".into(), json!(obs));
    }
    if let Some(blob) = &action.message {
        meta.insert("message_blob".into(), json!(blob));
    }
    if let Some(purpose) = &action.purpose {
        meta.insert("purpose".into(), json!(purpose));
    }
    if let Some(error) = &action.error {
        meta.insert("error".into(), json!(error));
    }
    meta
}

fn model_detail(step: &Step, action: &ModelAction, figures: &CallFigures) -> String {
    match &action.error {
        Some(error) => excerpt(first_line(error), 60),
        None => format!(
            "{}/{}  {}  {}{}",
            tokens(figures.prompt_tokens),
            tokens(figures.usage.output),
            secs(step.duration_ms.unwrap_or_default()),
            figures
                .intents
                .first()
                .map_or("answer", |(i, _)| i.as_str()),
            if figures.truncated {
                "  max tokens"
            } else {
                ""
            }
        ),
    }
}

fn result_flags(step: &Step, is_error: bool, source: &str, files: &[FileTouch]) -> Vec<String> {
    let mut flags = vec![if is_error { "fail" } else { "ok" }.to_string()];
    if source != "executor" {
        flags.push(source.to_string());
    }
    for f in files {
        if f.kind != "read" && (f.added > 0 || f.removed > 0) {
            flags.push(format!("+{} -{}", f.added, f.removed));
        }
    }
    if let Some(ms) = step.duration_ms {
        flags.push(secs(ms));
    }
    flags
}

fn harness_meta(
    step: &Step,
    name: &str,
    action: &HarnessAction,
    alternatives: &[String],
) -> Map<String, Value> {
    let source = match step.actor {
        Actor::Harness { source } => source.as_str(),
        _ => "harness",
    };
    let mut meta = Map::new();
    meta.insert("seq".into(), json!(step.seq));
    meta.insert("at_ms".into(), json!(step.at_ms));
    meta.insert("source".into(), json!(source));
    meta.insert("name".into(), json!(name));
    meta.insert("action".into(), json!(action));
    meta.insert("alternatives".into(), json!(alternatives));
    meta.insert("policy".into(), json!(step.policy));
    if let Observation::Harness { features, trigger } = &step.observation {
        meta.insert("features".into(), features.clone());
        meta.insert("trigger".into(), trigger.clone());
    }
    meta.insert("links".into(), json!(step.links));
    meta
}

fn harness_status(action: &HarnessAction) -> NodeStatus {
    match action {
        HarnessAction::Allow | HarnessAction::CacheHit { .. } | HarnessAction::Route { .. } => {
            NodeStatus::Ok
        }
        HarnessAction::Build { success: true, .. } => NodeStatus::Ok,
        HarnessAction::Build { .. } | HarnessAction::Stop { .. } => NodeStatus::Error,
        _ => NodeStatus::Warn,
    }
}
