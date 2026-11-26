//! Model calls, tool calls, their decisions and results, and spawns.

use super::*;

impl FlowBuilder {
    pub(super) fn model_call(&mut self, entry: &RecordEntry) {
        let payload = json_payload(&entry.tx).unwrap_or(Value::Null);
        let text = |key: &str| payload.get(key).and_then(Value::as_str);
        let mut node = new_node(
            entry,
            model_call_kind(&payload),
            text("model").unwrap_or("model call"),
        );
        node.status = TraceNodeStatus::Ok;
        let duration_ms = payload.get("duration_ms").and_then(Value::as_u64);
        let reasoning = reasoning_tokens(&payload);
        let tokens = model_call_usage(&mut node, entry, duration_ms).or(reasoning);
        let mut facts = Vec::new();
        if let Some(tokens) = tokens {
            set_meta(&mut node, "tokens", tokens.clone());
            facts.push(tokens);
        }
        if let Some(ms) = duration_ms {
            set_meta(&mut node, "took", format_duration_ms(ms));
            facts.push(format_duration_ms(ms));
        }
        model_call_reasoning_meta(&mut node, &payload, &mut facts);
        node.detail = facts.join(" · ");
        self.insert(entry, node, None);
    }

    pub(super) fn tool_call(&mut self, entry: &RecordEntry) {
        let Ok(proposal) = serde_json::from_slice::<ToolProposal>(&entry.tx.payload) else {
            return self.event(entry, &payload_text(&entry.tx));
        };
        let agent = entry.tx.agent_id;
        let mut node = new_node(
            entry,
            FlowKind::Tool,
            &call_label(&proposal.tool, &proposal.args),
        );
        node.status = TraceNodeStatus::Pending;
        set_call_meta(&mut node, &proposal.tool, &proposal.args);
        if let Some(source) = &proposal.source {
            set_meta(&mut node, "source", source.clone());
        }
        let id = node.id.clone();
        self.insert(entry, node, Some((agent, proposal.tool_use_id)));
        self.set_target(&id, &proposal.tool, &proposal.args);
        match proposal.tool.as_str() {
            TASK_TOOL_NAME => {
                self.tasks.push_back(TaskCall {
                    agent,
                    node: id,
                    prompt: arg(&proposal.args, "prompt").to_string(),
                    children: Vec::new(),
                    returned: false,
                    detached: arg(&proposal.args, "spawn_mode") == "detached",
                });
                if self.tasks.len() > MAX_TASK_CALLS {
                    self.tasks.pop_front();
                }
            }
            SEND_TO_AGENT_TOOL_NAME => {
                if let Some(target) = self.lane_with_id(arg(&proposal.args, "agent_id")) {
                    self.sends.insert(target, id);
                }
            }
            _ => {}
        }
    }

    pub(super) fn policy(&mut self, entry: &RecordEntry) {
        let Ok(decision) = serde_json::from_slice::<PolicyDecision>(&entry.tx.payload) else {
            return;
        };
        let key = (entry.tx.agent_id, decision.tool_use_id.clone());
        let Some(id) = self.tools.get(&key).cloned() else {
            return;
        };
        self.update(&id, |node| {
            match decision.verdict {
                ToolGateVerdict::Approved => {}
                ToolGateVerdict::Denied => {
                    node.status = TraceNodeStatus::Error;
                    node.detail = "denied".to_string();
                }
                ToolGateVerdict::PendingApproval => {
                    node.status = TraceNodeStatus::Warn;
                    node.detail = "awaiting approval".to_string();
                }
            }
            set_meta(node, "policy", summarize_policy_decision(&decision));
            if let Some(reason) = decision.reason.as_deref().filter(|r| !r.is_empty()) {
                set_meta(node, "policy reason", capped(reason));
            }
            if let Some(approval) = &decision.approval {
                set_meta(node, "approved by", approval_label(approval));
            }
            set_meta(node, "mode", decision.mode.clone());
        });
        if let Some(action) = decision.action_id {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.action = Some(action.clone());
            }
            self.actions.insert(action, id);
        }
    }

    pub(super) fn tool_result(&mut self, entry: &RecordEntry) {
        let Some(execution) = tool_execution(entry) else {
            return;
        };
        let agent = entry.tx.agent_id;
        let key = (agent, execution.tool_use_id.clone());
        let status = execution_status(&execution);
        let finish = |node: &mut FlowNode| {
            node.status = status;
            node.detail = execution_detail(&execution, status);
            set_meta(node, "outcome", capped(&tool_outcome(&execution)));
            if let Some(result) = execution.result.as_deref().filter(|r| !r.is_empty()) {
                set_meta(node, "result", capped(result));
            }
        };
        let call = self
            .tools
            .get(&key)
            .filter(|id| self.slots.contains_key(*id))
            .cloned();
        let Some(call) = call else {
            // The call itself is older than what was loaded.
            let mut node = new_node(
                entry,
                FlowKind::Tool,
                &call_label(&execution.tool, &execution.args),
            );
            set_call_meta(&mut node, &execution.tool, &execution.args);
            finish(&mut node);
            let id = node.id.clone();
            self.insert(entry, node, Some(key));
            return self.set_target(&id, &execution.tool, &execution.args);
        };
        self.update(&call, finish);
        if execution.tool == TASK_TOOL_NAME {
            self.task_returned(entry, &call, &execution, status);
        }
    }

    /// The `task` call `call` came back: a result node in the caller's
    /// lane, which the agents that did the work point to. A call no agent
    /// took yet (one running in the background) has nothing to show.
    pub(super) fn task_returned(
        &mut self,
        entry: &RecordEntry,
        call: &str,
        execution: &ToolExecution,
        status: TraceNodeStatus,
    ) {
        let Some(task) = self.tasks.iter_mut().find(|task| task.node == call) else {
            return;
        };
        task.returned = true;
        let children = task.children.clone();
        if children.is_empty() || task.detached {
            return;
        }
        let names: Vec<&str> = children
            .iter()
            .filter_map(|child| self.lane_of.get(child))
            .map(|lane| self.lanes[*lane].name.as_str())
            .collect();
        let verb = if status == TraceNodeStatus::Error {
            "failed"
        } else {
            "replied"
        };
        let mut node = new_node(
            entry,
            FlowKind::Result,
            &format!("{} {verb}", names.join(", ")),
        );
        node.status = status;
        node.detail = execution_detail(execution, status);
        if let Some(result) = execution.result.as_deref().filter(|r| !r.is_empty()) {
            set_meta(&mut node, "result", capped(result));
        }
        let id = node.id.clone();
        self.insert(entry, node, None);
        self.link(call, LINK_RESULT, &id);
        for child in children {
            if let Some(last) = self.last_node.get(&child).cloned() {
                self.link(&last, LINK_RETURN, &id);
            }
        }
    }

    pub(super) fn spawn(&mut self, entry: &RecordEntry) {
        let payload = json_payload(&entry.tx).unwrap_or(Value::Null);
        let child = spawned_child(entry);
        if let Some(child) = child {
            if self.bind_task(entry.tx.agent_id, child, &payload) {
                return;
            }
        }
        let name = child
            .and_then(|child| self.lane_of.get(&child))
            .map(|lane| self.lanes[*lane].name.clone())
            .or_else(|| child.map(|child| format!("agent {child}")))
            .unwrap_or_else(|| "agent".to_string());
        let mut node = new_node(entry, FlowKind::Spawn, &spawn_label(&name));
        node.status = TraceNodeStatus::Ok;
        if let Some(mode) = payload.get("spawn_mode").and_then(Value::as_str) {
            node.detail = mode.to_string();
        }
        if let Some(child) = child {
            set_meta(&mut node, "child", child.to_hex());
        }
        let id = node.id.clone();
        self.insert(entry, node, None);
        if let Some(child) = child {
            self.spawns.insert(child, id);
        }
    }

    /// Tie `child` to the `task` call of `parent` its spawn entry
    /// `payload` names by tool-use id: the call's row is what the child's
    /// work wires to, so the spawn needs no box of its own. Returns
    /// whether the call was found.
    pub(super) fn bind_task(&mut self, parent: AgentId, child: AgentId, payload: &Value) -> bool {
        let tool_use = arg(payload, "tool_use_id");
        if tool_use.is_empty() {
            return false;
        }
        let Some(call) = self.tools.get(&(parent, tool_use.to_string())).cloned() else {
            return false;
        };
        let Some(task) = self.tasks.iter_mut().find(|task| task.node == call) else {
            return false;
        };
        if !task.children.contains(&child) {
            task.children.push(child);
        }
        let agent = arg(payload, "child_name");
        let todo = arg(payload, "todo_id");
        self.update(&call, |node| {
            if !agent.is_empty() {
                set_meta(node, "agent", agent.to_string());
            }
            if !todo.is_empty() {
                set_meta(node, "todo", todo.to_string());
            }
            set_meta(node, "child", child.to_hex());
        });
        if !self.last_node.contains_key(&child) {
            self.bound.insert(child, call);
        }
        true
    }
}

/// The kind of node a model call is, from the purpose it was made for.
fn model_call_kind(payload: &Value) -> FlowKind {
    let purpose = payload
        .get("purpose")
        .cloned()
        .and_then(|purpose| serde_json::from_value(purpose).ok());
    match purpose {
        Some(ModelCallPurpose::Classify | ModelCallPurpose::Criteria) => FlowKind::Gate,
        Some(ModelCallPurpose::Judge) => FlowKind::Judge,
        Some(ModelCallPurpose::Title) => FlowKind::Title,
        None => FlowKind::Model,
    }
}

/// Record what the call in `entry` used on `node`; the token line when
/// the entry has a model call.
fn model_call_usage(
    node: &mut FlowNode,
    entry: &RecordEntry,
    duration_ms: Option<u64>,
) -> Option<String> {
    let call = model_call(&entry.tx)?;
    let billed = call.billed_tokens();
    let flow_usage = FlowUsage {
        input_tokens: billed.uncached_input + billed.cache_write + billed.cache_read,
        output_tokens: billed.output,
        cost_usd: call.estimate_cost_usd(),
        duration_ms,
    };
    // The whole prompt, as the stats line counts it: Anthropic
    // reports cached tokens beside `input_tokens`, not in it.
    let tokens = format!(
        "{}/{} tok",
        format_tokens(flow_usage.input_tokens),
        format_tokens(flow_usage.output_tokens)
    );
    if billed.cache_read > 0 || billed.cache_write > 0 {
        set_meta(
            node,
            "cache",
            format!(
                "{} read · {} written",
                format_tokens(billed.cache_read),
                format_tokens(billed.cache_write)
            ),
        );
    }
    if let Some(cost) = flow_usage.cost_usd {
        set_meta(node, "cost", format!("${cost:.4}"));
    }
    node.usage = Some(flow_usage);
    Some(tokens)
}

/// Stop reason, calls, thinking and error of a model call; an error
/// replaces `facts` with "failed".
fn model_call_reasoning_meta(node: &mut FlowNode, payload: &Value, facts: &mut Vec<String>) {
    let text = |key: &str| payload.get(key).and_then(Value::as_str);
    if let Some(stop) = text("stop_reason") {
        set_meta(node, "stop", stop.to_string());
    }
    if let Some(tools) = reasoning_tools(payload) {
        set_meta(node, "calls", tools);
    }
    if let Some(thinking) = text("thinking").filter(|thinking| !thinking.is_empty()) {
        set_meta(node, "thinking", capped(thinking));
        if let Some(ms) = payload.get("thinking_ms").and_then(Value::as_u64) {
            set_meta(node, "thought for", format_duration_ms(ms));
        }
    }
    if let Some(error) = reasoning_error(payload) {
        node.status = TraceNodeStatus::Error;
        *facts = vec!["failed".to_string()];
        set_meta(node, "error", capped(error));
        if let Some(stage) = text("stage") {
            set_meta(node, "stage", stage.to_string());
        }
    }
}
