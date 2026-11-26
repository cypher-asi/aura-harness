//! System entries, processes, outcomes and plain events.

use super::*;

impl FlowBuilder {
    pub(super) fn system(&mut self, entry: &RecordEntry) {
        if let Some(payload) = entry.tx.compare_payload() {
            self.bench(entry.tx.agent_id, &payload);
        }
        let Some(payload) = json_payload(&entry.tx) else {
            return self.event(entry, &payload_text(&entry.tx));
        };
        if payload.get("kind").and_then(Value::as_str) != Some("agent_message") {
            return self.event(entry, &summarize_system(&payload));
        }
        // A message the agent could not take live (see the agent host).
        let from = arg(&payload, "from_name");
        let text = arg(&payload, "text");
        let mut node = new_node(entry, FlowKind::Message, &format!("{from}: {text}"));
        node.status = TraceNodeStatus::Ok;
        set_meta(&mut node, "from", from.to_string());
        set_meta(&mut node, "text", capped(text));
        let id = node.id.clone();
        self.insert(entry, node, None);
        let reply = payload.get("reply").and_then(Value::as_bool) == Some(true);
        let sender = self
            .lane_with_id(arg(&payload, "from_agent_id"))
            .or_else(|| self.lane_shown_as(from));
        if let Some(last) = sender.and_then(|sender| self.last_node.get(&sender).cloned()) {
            self.link(&last, if reply { LINK_RETURN } else { LINK_MESSAGE }, &id);
        }
    }

    pub(super) fn process(&mut self, entry: &RecordEntry) {
        let result = serde_json::from_slice::<ActionResultPayload>(&entry.tx.payload).ok();
        let label = result
            .as_ref()
            .map_or_else(|| payload_text(&entry.tx), summarize_action_result);
        // The action id names the call that started the process; the
        // reference is only the agent's latest entry when it started,
        // which in a batch of calls can be another call's.
        let started_by = result
            .as_ref()
            .and_then(|result| self.actions.get(&result.action_id.to_hex()))
            .or_else(|| {
                let hash = entry.tx.reference_tx_hash?;
                self.by_tx.get(&hash)
            })
            .cloned();
        let command = started_by
            .as_ref()
            .and_then(|id| self.slots.get(id))
            .and_then(|slot| slot.target.clone());
        let label = match &command {
            Some(command) => format!("{command} · {label}"),
            None => label,
        };
        let mut node = new_node(entry, FlowKind::Process, &label);
        node.status = match &result {
            Some(result) if !result.success => TraceNodeStatus::Error,
            _ => TraceNodeStatus::Ok,
        };
        if let Some(command) = command {
            set_meta(&mut node, "command", capped(&command));
        }
        if let Some(result) = &result {
            node.detail = format_duration_ms(result.duration_ms);
            set_meta(
                &mut node,
                "exit",
                result
                    .exit_code
                    .map_or_else(|| "none".to_string(), |code| code.to_string()),
            );
            set_meta(&mut node, "process", result.process_id.to_hex());
            for (key, bytes) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
                let text = String::from_utf8_lossy(bytes);
                if !text.trim().is_empty() {
                    set_meta(&mut node, key, capped(&text));
                }
            }
        }
        let id = node.id.clone();
        self.insert(entry, node, None);
        if let Some(from) = started_by {
            self.link(&from, LINK_COMPLETED, &id);
        }
    }

    /// An outcome says how its prompt's turn ended; it has no box.
    pub(super) fn outcome(&mut self, entry: &RecordEntry) {
        let Some(outcome) = entry.tx.outcome_payload() else {
            return;
        };
        let Some(id) = self.by_tx.get(&outcome.prompt_tx).cloned() else {
            return;
        };
        if let Some(open) = self.open_prompts.get_mut(&entry.tx.agent_id) {
            open.retain(|prompt| *prompt != id);
        }
        self.update(&id, |node| {
            node.status = match outcome.success {
                Some(false) => TraceNodeStatus::Error,
                Some(true) => TraceNodeStatus::Ok,
                None if node.status == TraceNodeStatus::Pending => TraceNodeStatus::Ok,
                None => node.status,
            };
            if !outcome.model.is_empty() {
                node.detail = short(
                    &format!(
                        "{} · {}",
                        outcome.model,
                        format_duration_ms(outcome.latency_ms)
                    ),
                    MAX_DETAIL_CHARS,
                );
            }
            // Later signals (a rewind, the judge, the user's rating) say
            // more about the same work; the first one is not dropped.
            let signal = outcome.signal.to_string();
            let signals = match node.meta.iter().find(|(key, _)| key == "outcome") {
                Some((_, before)) if before.split(" → ").any(|s| s == signal) => before.clone(),
                Some((_, before)) => format!("{before} → {signal}"),
                None => signal,
            };
            set_meta(node, "outcome", signals);
            set_meta(node, "cost", format!("${:.4}", outcome.cost_usd));
            set_meta(node, "took", format_duration_ms(outcome.latency_ms));
        });
    }

    pub(super) fn event(&mut self, entry: &RecordEntry, label: &str) {
        let mut node = new_node(entry, FlowKind::Event, label);
        node.status = TraceNodeStatus::Ok;
        self.insert(entry, node, None);
    }
}
