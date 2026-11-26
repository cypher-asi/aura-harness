//! Writes one admitted candidate as a fact, event or procedure.

use super::candidate::{CandidateWrite, ProcedureSteps};
use super::{MemoryWritePipeline, WriteReport};
use crate::error::MemoryError;
use crate::types::{Fact, MemoryStatus, Procedure};
use aura_core_types::FactId;
use chrono::{DateTime, Utc};

impl MemoryWritePipeline {
    pub(super) fn write_fact(
        &self,
        write: &CandidateWrite<'_>,
        report: &mut WriteReport,
    ) -> Result<(), MemoryError> {
        let now = Utc::now();

        let existing = if write.is_active_write() {
            self.store
                .get_fact_by_key(write.storage_id, &write.candidate.key)
                .ok()
                .flatten()
        } else {
            None
        };
        match existing {
            Some(existing) if existing.value == write.candidate.value => {
                self.refresh_fact(existing, write, now, report)
            }
            Some(existing) => self.replace_fact(existing, write, now, report),
            None => {
                let fact = write.fact(FactId::generate(), write.origin.initial_status, now);
                self.store.put_fact(&fact)?;
                report.facts_written += 1;
                Ok(())
            }
        }
    }

    /// Same value again: refresh confidence and importance in place.
    fn refresh_fact(
        &self,
        mut existing: Fact,
        write: &CandidateWrite<'_>,
        now: DateTime<Utc>,
        report: &mut WriteReport,
    ) -> Result<(), MemoryError> {
        existing.confidence = write.candidate.confidence;
        existing.importance = write.candidate.importance;
        existing.updated_at = now;
        self.store.put_fact(&existing)?;
        report.facts_updated += 1;
        Ok(())
    }

    /// New value: supersede the existing fact, or park the replacement as
    /// pending when another agent owns the shared fact.
    fn replace_fact(
        &self,
        mut existing: Fact,
        write: &CandidateWrite<'_>,
        now: DateTime<Utc>,
        report: &mut WriteReport,
    ) -> Result<(), MemoryError> {
        let replacement_id = FactId::generate();
        let replacement_status = if write.is_from_other_contributor(&existing.continuity) {
            report.conflicts_pending += 1;
            MemoryStatus::Pending
        } else {
            existing.continuity.status = MemoryStatus::Superseded;
            existing.continuity.superseded_by = Some(replacement_id.to_hex());
            existing.updated_at = now;
            self.store.put_fact(&existing)?;
            write.origin.initial_status
        };

        let replacement = write.fact(replacement_id, replacement_status, now);
        self.store.put_fact(&replacement)?;
        report.facts_updated += 1;
        report.facts_written += 1;
        Ok(())
    }

    pub(super) fn write_event(
        &self,
        write: &CandidateWrite<'_>,
        report: &mut WriteReport,
    ) -> Result<(), MemoryError> {
        let event = write.event(Utc::now());
        self.store.put_event(&event)?;
        report.events_written += 1;

        Ok(())
    }

    pub(super) fn write_procedure(
        &self,
        write: &CandidateWrite<'_>,
        report: &mut WriteReport,
    ) -> Result<(), MemoryError> {
        let now = Utc::now();
        let steps = ProcedureSteps::from_candidate(write.candidate);

        // Check for an existing procedure with the same name and update it.
        let mut status = write.origin.initial_status;
        if let Some(mut proc) = self.active_procedure_named(write)? {
            if write.is_from_other_contributor(&proc.continuity) && steps.differs_from(&proc) {
                report.conflicts_pending += 1;
                status = MemoryStatus::Pending;
            } else {
                proc.trigger = steps.trigger;
                proc.steps = steps.steps;
                proc.skill_name = write.candidate.skill_name.clone();
                proc.updated_at = now;
                self.store.put_procedure(&proc)?;
                report.procedures_written += 1;
                return Ok(());
            }
        }

        let procedure = write.procedure(steps, status, now);
        self.store.put_procedure(&procedure)?;
        report.procedures_written += 1;

        Ok(())
    }

    /// The active procedure with the candidate's name, considered only for
    /// active writes.
    fn active_procedure_named(
        &self,
        write: &CandidateWrite<'_>,
    ) -> Result<Option<Procedure>, MemoryError> {
        let existing = self.store.list_procedures(write.storage_id)?;
        if !write.is_active_write() {
            return Ok(None);
        }
        Ok(existing
            .into_iter()
            .find(|p| p.name == write.candidate.key && p.continuity.status == MemoryStatus::Active))
    }
}
