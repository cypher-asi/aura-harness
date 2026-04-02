//! Core memory types.

use aura_core::{AgentEventId, AgentId, FactId, ProcedureId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub fact_id: FactId,
    pub agent_id: AgentId,
    pub key: String,
    pub value: serde_json::Value,
    pub confidence: f32,
    pub source: FactSource,
    pub importance: f32,
    pub access_count: u32,
    pub last_accessed: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FactSource {
    Extracted,
    UserProvided,
    Consolidated,
}

impl std::fmt::Display for FactSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extracted => write!(f, "extracted"),
            Self::UserProvided => write!(f, "user_provided"),
            Self::Consolidated => write!(f, "consolidated"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub event_id: AgentEventId,
    pub agent_id: AgentId,
    pub event_type: String,
    pub summary: String,
    pub metadata: serde_json::Value,
    pub importance: f32,
    pub access_count: u32,
    pub last_accessed: DateTime<Utc>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Procedure {
    pub procedure_id: ProcedureId,
    pub agent_id: AgentId,
    pub name: String,
    pub trigger: String,
    pub steps: Vec<String>,
    pub context_constraints: serde_json::Value,
    pub success_rate: f32,
    pub execution_count: u32,
    pub last_used: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryPacket {
    pub facts: Vec<Fact>,
    pub events: Vec<AgentEvent>,
    pub procedures: Vec<Procedure>,
}

impl MemoryPacket {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.events.is_empty() && self.procedures.is_empty()
    }

    #[must_use]
    pub fn format_for_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut out = String::from("\n<agent_memory>\n");

        if !self.facts.is_empty() {
            out.push_str("<facts>\n");
            for fact in &self.facts {
                let val = match &fact.value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let _ = writeln!(out, "- {}: {} (confidence: {:.2})", fact.key, val, fact.confidence);
            }
            out.push_str("</facts>\n");
        }

        if !self.events.is_empty() {
            out.push_str("<recent_events>\n");
            for event in &self.events {
                let _ = writeln!(
                    out,
                    "- [{}] {}: {}",
                    event.timestamp.format("%Y-%m-%d"),
                    event.event_type,
                    event.summary
                );
            }
            out.push_str("</recent_events>\n");
        }

        if !self.procedures.is_empty() {
            out.push_str("<procedures>\n");
            for proc in &self.procedures {
                let steps = proc.steps.join(" -> ");
                let _ = writeln!(
                    out,
                    "- \"{}\": {} (success: {:.0}%)",
                    proc.name,
                    steps,
                    proc.success_rate * 100.0
                );
            }
            out.push_str("</procedures>\n");
        }

        out.push_str("</agent_memory>");
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CandidateType {
    Fact,
    Event,
    Procedure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCandidate {
    pub candidate_type: CandidateType,
    pub key: Option<String>,
    pub value: serde_json::Value,
    pub summary: Option<String>,
    pub source_hint: String,
    pub preliminary_confidence: f32,
    pub preliminary_importance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefinedCandidate {
    pub candidate_type: CandidateType,
    pub key: String,
    pub value: serde_json::Value,
    pub summary: Option<String>,
    pub confidence: f32,
    pub importance: f32,
    pub keep: bool,
}
