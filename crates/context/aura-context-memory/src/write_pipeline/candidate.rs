//! Per-candidate write context, filters and record builders.

use crate::safety::unsafe_for_prompt;
use crate::types::{
    AgentEvent, Fact, FactSource, MemoryAccessContext, MemoryContinuity, MemoryProvenance,
    MemoryScope, MemorySensitivity, MemoryStatus, Procedure, RefinedCandidate,
};
use aura_core_types::{AgentEventId, AgentId, FactId, ProcedureId};
use chrono::{DateTime, Utc};

/// Where a batch of refined candidates comes from; shared by every write of
/// one ingest call.
#[derive(Clone, Copy)]
pub(super) struct WriteOrigin<'a> {
    pub(super) contributor_agent_id: AgentId,
    pub(super) source_session_id: Option<&'a str>,
    pub(super) initial_status: MemoryStatus,
    pub(super) memory_context: &'a MemoryAccessContext,
    pub(super) extractor_model: &'a str,
}

impl<'a> WriteOrigin<'a> {
    /// Resolves the candidate's effective scope and storage partition.
    pub(super) fn candidate_write(self, candidate: &'a RefinedCandidate) -> CandidateWrite<'a> {
        let scope = effective_scope(candidate.scope, self.memory_context);
        CandidateWrite {
            storage_id: self
                .memory_context
                .storage_id(self.contributor_agent_id, scope),
            scope,
            candidate,
            origin: self,
        }
    }
}

/// One refined candidate on its way to storage.
pub(super) struct CandidateWrite<'a> {
    pub(super) storage_id: AgentId,
    pub(super) scope: MemoryScope,
    pub(super) candidate: &'a RefinedCandidate,
    pub(super) origin: WriteOrigin<'a>,
}

impl CandidateWrite<'_> {
    pub(super) fn is_active_write(&self) -> bool {
        self.origin.initial_status == MemoryStatus::Active
    }

    /// True when a shared-scope record was contributed by another agent.
    pub(super) fn is_from_other_contributor(&self, existing: &MemoryContinuity) -> bool {
        let contributor_hex = self.origin.contributor_agent_id.to_hex();
        is_shared_scope(self.scope)
            && existing.provenance.contributor_agent_id.as_deref() != Some(contributor_hex.as_str())
    }

    pub(super) fn continuity(
        &self,
        excerpt: Option<String>,
        status: MemoryStatus,
    ) -> MemoryContinuity {
        let origin = &self.origin;
        MemoryContinuity {
            scope: self.scope,
            status,
            sensitivity: if candidate_is_sensitive(self.candidate) {
                MemorySensitivity::Sensitive
            } else {
                MemorySensitivity::Normal
            },
            provenance: MemoryProvenance {
                session_id: origin.source_session_id.map(str::to_string),
                excerpt,
                extractor_model: Some(origin.extractor_model.to_string()),
                project_id: origin.memory_context.project_id.clone(),
                user_id: origin.memory_context.user_id.clone(),
                contributor_agent_id: Some(origin.contributor_agent_id.to_hex()),
            },
            ..MemoryContinuity::default()
        }
    }

    pub(super) fn fact(&self, fact_id: FactId, status: MemoryStatus, now: DateTime<Utc>) -> Fact {
        let candidate = self.candidate;
        Fact {
            fact_id,
            agent_id: self.storage_id,
            key: candidate.key.clone(),
            value: candidate.value.clone(),
            confidence: candidate.confidence,
            source: FactSource::Extracted,
            importance: candidate.importance,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            updated_at: now,
            continuity: self.continuity(turn_excerpt(candidate), status),
        }
    }

    pub(super) fn event(&self, now: DateTime<Utc>) -> AgentEvent {
        let candidate = self.candidate;
        AgentEvent {
            event_id: AgentEventId::generate(),
            agent_id: self.storage_id,
            event_type: "task_run".to_string(),
            summary: candidate.summary.clone().unwrap_or_default(),
            metadata: candidate.value.clone(),
            importance: candidate.importance,
            access_count: 0,
            last_accessed: now,
            timestamp: now,
            continuity: self.continuity(None, self.origin.initial_status),
        }
    }

    pub(super) fn procedure(
        &self,
        steps: ProcedureSteps,
        status: MemoryStatus,
        now: DateTime<Utc>,
    ) -> Procedure {
        let candidate = self.candidate;
        Procedure {
            procedure_id: ProcedureId::generate(),
            agent_id: self.storage_id,
            name: candidate.key.clone(),
            trigger: steps.trigger,
            steps: steps.steps,
            context_constraints: serde_json::Value::Null,
            success_rate: 1.0,
            execution_count: 0,
            last_used: now,
            created_at: now,
            updated_at: now,
            skill_name: candidate.skill_name.clone(),
            skill_relevance: candidate.skill_name.as_ref().map(|_| 0.8),
            continuity: self.continuity(None, status),
        }
    }
}

/// Trigger and steps a procedure candidate proposes.
pub(super) struct ProcedureSteps {
    pub(super) trigger: String,
    pub(super) steps: Vec<String>,
}

impl ProcedureSteps {
    pub(super) fn from_candidate(candidate: &RefinedCandidate) -> Self {
        Self {
            trigger: candidate
                .trigger
                .clone()
                .unwrap_or_else(|| candidate.summary.clone().unwrap_or_default()),
            steps: candidate.steps.clone().unwrap_or_default(),
        }
    }

    pub(super) fn differs_from(&self, procedure: &Procedure) -> bool {
        procedure.trigger != self.trigger || procedure.steps != self.steps
    }
}

fn effective_scope(scope: MemoryScope, context: &MemoryAccessContext) -> MemoryScope {
    match scope {
        MemoryScope::Project if context.project_id.is_none() => MemoryScope::Agent,
        MemoryScope::User if context.user_id.is_none() => MemoryScope::Agent,
        other => other,
    }
}

fn is_shared_scope(scope: MemoryScope) -> bool {
    matches!(scope, MemoryScope::Project | MemoryScope::User)
}

fn turn_excerpt(candidate: &RefinedCandidate) -> Option<String> {
    let raw = candidate
        .summary
        .as_deref()
        .or_else(|| candidate.value.as_str())?;
    let excerpt: String = raw.chars().take(240).collect();
    (!excerpt.trim().is_empty()).then_some(excerpt)
}

pub(super) fn candidate_is_sensitive(candidate: &RefinedCandidate) -> bool {
    let haystack = format!("{} {}", candidate.key, candidate.value).to_ascii_lowercase();
    [
        "password",
        "passcode",
        "api_key",
        "api key",
        "secret",
        "private_key",
        "private key",
        "access_token",
        "access token",
        "credit card",
        "social security",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

pub(super) fn candidate_is_unsafe(candidate: &RefinedCandidate) -> bool {
    unsafe_for_prompt(&format!(
        "{} {} {}",
        candidate.key,
        candidate.value,
        candidate.summary.as_deref().unwrap_or_default()
    ))
}
