//! Prompts and replies, and who sent them.

use super::*;

impl FlowBuilder {
    pub(super) fn prompt(&mut self, entry: &RecordEntry) {
        let agent = entry.tx.agent_id;
        let payload = entry.tx.prompt_payload();
        let text = payload
            .as_ref()
            .map_or_else(|| payload_text(&entry.tx), |prompt| prompt.text.clone());
        let mut node = new_node(entry, FlowKind::Prompt, &text);
        let mut subtask = false;
        if let Some(prompt) = &payload {
            subtask = prompt.route.level == RouteLevel::Subtask;
            node.detail = short(&prompt.route.model, MAX_DETAIL_CHARS);
            if let Some(category) = prompt.category {
                set_meta(&mut node, "category", category.to_string());
            }
            if !prompt.route.model.is_empty() {
                set_meta(
                    &mut node,
                    "route",
                    format!(
                        "{} · {} · {}",
                        prompt.route.level, prompt.route.model, prompt.route.reason
                    ),
                );
            }
            if prompt.route.explored {
                set_meta(
                    &mut node,
                    "explored",
                    format!("propensity {:.3}", prompt.route.propensity),
                );
            }
            if !prompt.classifier.is_empty() {
                set_meta(&mut node, "classifier", prompt.classifier.clone());
            }
            if let Some(kind) = &prompt.kind {
                set_meta(&mut node, "kind", kind.clone());
            }
        }
        set_meta(&mut node, "text", capped(&text));
        let id = node.id.clone();
        let source = self.prompt_source(agent, subtask, &text);
        self.insert(entry, node, None);
        self.open_prompts.entry(agent).or_default().push(id.clone());
        if let Some((from, kind)) = source {
            self.link(&from, kind, &id);
        }
    }

    /// The node the prompt `text` for `agent` came from, and how: the
    /// `task` call that gave it the work, the last thing the agent whose
    /// reply or message it carries did, or the `send_to_agent` call that
    /// sent it.
    pub(super) fn prompt_source(
        &mut self,
        agent: AgentId,
        subtask: bool,
        text: &str,
    ) -> Option<(String, &'static str)> {
        if let Some(call) = self.bound.remove(&agent) {
            return Some((call, LINK_SPAWN));
        }
        if subtask {
            let parent = self.lanes[*self.lane_of.get(&agent)?].parent;
            if let Some(call) = self.claim_task(parent, agent, text) {
                return Some((call, LINK_SPAWN));
            }
        }
        if let Some((sender, kind)) = self.sender_of(agent, text) {
            return self.last_node.get(&sender).map(|id| (id.clone(), kind));
        }
        self.sends.remove(&agent).map(|id| (id, LINK_MESSAGE))
    }

    /// The `task` call of `parent` that `child`'s prompt `text` belongs
    /// to: the latest one asking for exactly that, else the latest still
    /// waiting (a retry rewrites the prompt).
    pub(super) fn claim_task(
        &mut self,
        parent: Option<AgentId>,
        child: AgentId,
        text: &str,
    ) -> Option<String> {
        let parent = parent?;
        let wanted = text.trim();
        let calls = || {
            self.tasks
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, call)| call.agent == parent)
        };
        let index = calls()
            .find(|(_, call)| call.prompt.trim() == wanted)
            .or_else(|| calls().find(|(_, call)| !call.returned))
            .map(|(index, _)| index)?;
        let call = &mut self.tasks[index];
        call.children.push(child);
        Some(call.node.clone())
    }

    /// The agent whose reply or message the prompt `text` for `to`
    /// carries, going by how the harness and the agent host word them.
    pub(super) fn sender_of(&self, to: AgentId, text: &str) -> Option<(AgentId, &'static str)> {
        let text = text
            .strip_prefix(SteerSource::Harness.prefix())
            .unwrap_or(text);
        if let Some(rest) = text.strip_prefix("Agent ") {
            if let Some((who, _)) = rest.split_once(") reported back:") {
                return self.lane_shown_as(who).map(|agent| (agent, LINK_RETURN));
            }
        }
        if let Some(rest) = text.strip_prefix("Message from agent ") {
            if let Some((who, _)) = rest.split_once("):") {
                return self.lane_shown_as(who).map(|agent| (agent, LINK_MESSAGE));
            }
        }
        self.lanes
            .iter()
            .rev()
            .filter(|lane| lane.agent != to)
            .find(|lane| {
                text.strip_prefix(lane.name.as_str())
                    .is_some_and(|rest| TASK_NOTICE_ENDINGS.iter().any(|end| rest.starts_with(end)))
            })
            .map(|lane| (lane.agent, LINK_RETURN))
    }

    /// The lane of the agent written as `name (id`, by id, else by name.
    pub(super) fn lane_shown_as(&self, who: &str) -> Option<AgentId> {
        let (name, id) = who.rsplit_once(" (").unwrap_or((who, ""));
        self.lane_with_id(id).or_else(|| {
            self.lanes
                .iter()
                .rev()
                .find(|lane| lane.name == name)
                .map(|lane| lane.agent)
        })
    }

    /// The lane whose agent's hex id starts with `id`.
    pub(super) fn lane_with_id(&self, id: &str) -> Option<AgentId> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        self.lanes
            .iter()
            .find(|lane| lane.hex.starts_with(id))
            .map(|lane| lane.agent)
    }

    pub(super) fn reply(&mut self, entry: &RecordEntry) {
        let text = payload_text(&entry.tx);
        let mut node = new_node(entry, FlowKind::Reply, &text);
        node.status = TraceNodeStatus::Ok;
        set_meta(&mut node, "text", capped(&text));
        self.insert(entry, node, None);
        for id in self
            .open_prompts
            .remove(&entry.tx.agent_id)
            .unwrap_or_default()
        {
            self.update(&id, |prompt| {
                if prompt.status == TraceNodeStatus::Pending {
                    prompt.status = TraceNodeStatus::Ok;
                }
            });
        }
    }
}
