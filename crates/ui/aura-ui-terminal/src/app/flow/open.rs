//! Opening what the GRAPH tab has selected: its node as JSON, its record,
//! its agent.

use super::layout::CardKind;
use super::FlowState;
use crate::app::file_view::FileView;
use crate::app::trace_json::{node_json, NodeLink};
use crate::app::{App, DetailView, NotificationType, PanelFocus, RecordsTab};
use crate::events::{FlowNode, TraceNode};

impl App {
    /// Show the selected node, with its metadata and links, as JSON over
    /// the chat panel. Closing it comes back to the GRAPH tab.
    pub(super) fn open_flow_node(&mut self) {
        let Some(node) = self.flow.selected_node() else {
            return;
        };
        let links = self.flow.node_links(node);
        let step = TraceNode {
            id: node.id.clone(),
            kind: node.kind.label().to_string(),
            parent: None,
            label: node.label.clone(),
            detail: node.detail.clone(),
            status: node.status,
            seq: Some(node.seq),
            meta: self.flow.node_meta(node),
            links: Vec::new(),
        };
        let title = format!("GRAPH {} {}", step.kind.to_uppercase(), step.label);
        let view = FileView::snapshot(&title, &node_json(&step, &links), "json", 1);
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Show the kernel record the selected node comes from in the Record
    /// panel, which lists the agent on screen only.
    pub(super) fn jump_to_flow_record(&mut self) {
        let Some(node) = self.flow.selected_node() else {
            return;
        };
        let (seq, agent_id) = (node.seq, node.agent_id.clone());
        let found = (agent_id == self.active_agent_id)
            .then(|| self.records.iter().position(|record| record.seq == seq))
            .flatten();
        if let Some(index) = found {
            self.selected_record = index;
            self.records_tab = RecordsTab::Kernel;
            self.focus = PanelFocus::Records;
            self.open_detail(DetailView::Record);
            return;
        }
        let message = if agent_id == self.active_agent_id {
            format!("Record #{seq} is no longer in the Record panel")
        } else {
            format!(
                "Record #{seq} is in the record of {}; press o to open that agent",
                self.flow.lane_name(&agent_id)
            )
        };
        self.notifications.current = Some((message, NotificationType::Warning));
    }

    /// Switch to the agent whose lane the selected node is in. It comes
    /// back on the chat panel's tab it was left on, CHAT the first time it
    /// is shown.
    pub(super) fn open_flow_agent(&mut self) {
        if let Some(agent_id) = self.flow.selected_node().map(|node| node.agent_id.clone()) {
            self.open_agent(agent_id);
        }
    }

    /// Switch to agent `agent_id`, when it has a chat to open.
    pub(super) fn open_agent(&mut self, agent_id: String) {
        let name = self.flow.lane_name(&agent_id).to_string();
        if agent_id == self.active_agent_id {
            self.notifications.current =
                Some((format!("{name} is already open"), NotificationType::Success));
        } else if self.agents.iter().any(|agent| agent.id == agent_id) {
            self.request_agent_switch(agent_id);
        } else {
            self.notifications.current = Some((
                format!("{name} is a background agent with no chat to open"),
                NotificationType::Warning,
            ));
        }
    }
}

impl FlowState {
    /// The links of `node`, each with the label of the node at its other end.
    fn node_links(&self, node: &FlowNode) -> Vec<NodeLink> {
        self.links_of(&node.id)
            .into_iter()
            .map(|(kind, other)| {
                let label = self
                    .index
                    .get(&other)
                    .map(|&at| self.nodes[at].label.clone());
                (kind, other, label)
            })
            .collect()
    }

    /// What the JSON of `node` says of it besides its own metadata: its
    /// agent, who spawned that, when it happened and a turn's totals.
    fn node_meta(&self, node: &FlowNode) -> Vec<(String, String)> {
        let mut meta = vec![
            (
                "agent".to_string(),
                self.lane_name(&node.agent_id).to_string(),
            ),
            ("agent_id".to_string(), node.agent_id.clone()),
        ];
        let parent = self
            .lane_index
            .get(&node.agent_id)
            .and_then(|&lane| self.lanes[lane].parent.as_deref());
        if let Some(parent) = parent {
            meta.push((
                "spawned by".to_string(),
                format!("{} ({parent})", self.lane_name(parent)),
            ));
        }
        if let Some(time) = local_time(node.at_ms) {
            meta.push(("time".to_string(), time));
        }
        if let Some(totals) = self.selected_turn_totals() {
            meta.push(("turn".to_string(), totals));
        }
        meta.extend(node.meta.iter().cloned());
        meta
    }

    /// The totals of the selected turn, as JSON, when its head is selected.
    fn selected_turn_totals(&self) -> Option<String> {
        let Some((card, None)) = self.selected_place() else {
            return None;
        };
        let placed = &self.layout().cards[card];
        let (CardKind::Turn, Some(usage)) = (placed.kind, placed.stats) else {
            return None;
        };
        let totals = serde_json::json!({
            "steps": placed.steps,
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cost_usd": usage.cost_usd,
            "duration_ms": usage.duration_ms,
        });
        Some(totals.to_string())
    }
}

/// `at_ms` as local time, `2024-05-01 12:34:56.789`.
fn local_time(at_ms: u64) -> Option<String> {
    let time = i64::try_from(at_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)?;
    let local = time.with_timezone(&chrono::Local);
    Some(local.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
}
