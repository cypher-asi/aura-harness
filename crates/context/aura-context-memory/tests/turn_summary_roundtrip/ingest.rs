//! Heuristic-only ingest: candidate counts, outcome events, provenance and
//! supersession.

use aura_context_memory::{MemoryStatus, MemoryStoreApi, TurnSummary};
use aura_core_types::AgentId;

use crate::support::pipeline_with_silent_llm;

#[tokio::test]
async fn turn_summary_with_no_iterations_produces_empty_report() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    let summary = TurnSummary::default();

    let report = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");

    // No text, no iterations, no conversation turn → nothing to do.
    assert_eq!(report.candidates_extracted, 0);
    assert_eq!(report.candidates_refined, 0);
    assert_eq!(report.facts_written, 0);
    assert_eq!(report.events_written, 0);
    assert_eq!(store.list_facts(agent_id).expect("list facts").len(), 0);
    assert_eq!(
        store.list_events(agent_id, 100).expect("list events").len(),
        0
    );
}

#[tokio::test]
async fn turn_summary_with_outcome_event_produces_one_event() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    // Drives `HeuristicExtractor::extract_task_outcome`: a non-zero
    // iteration count is enough to produce one Event candidate. Pre-Phase-6c
    // the same shape came from `AgentLoopResult { iterations: 3, .. }`.
    let summary = TurnSummary {
        iterations: 3,
        total_input_tokens: 11,
        total_output_tokens: 22,
        ..TurnSummary::default()
    };

    let report = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");

    assert_eq!(
        report.candidates_extracted, 1,
        "exactly one task-outcome event"
    );
    assert_eq!(report.candidates_refined, 1);
    assert_eq!(report.events_written, 1);
    assert_eq!(report.facts_written, 0);
    let events = store.list_events(agent_id, 10).expect("list events");
    assert_eq!(events.len(), 1);
    assert!(
        events[0].summary.contains("completed"),
        "expected completed outcome label, got: {}",
        events[0].summary,
    );
}

#[tokio::test]
async fn approval_writes_pending_memory_with_provenance() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    let summary = TurnSummary {
        total_text: "the test command is cargo nextest run".to_string(),
        ..TurnSummary::default()
    };

    let report = pipeline
        .ingest_with_provenance(
            agent_id,
            &summary,
            None,
            None,
            &[],
            Some("session-approval-1"),
            MemoryStatus::Pending,
        )
        .await
        .expect("ingest pending memory");

    assert_eq!(report.facts_written, 1);
    let fact = store.list_facts(agent_id).expect("list facts").remove(0);
    assert_eq!(fact.continuity.status, MemoryStatus::Pending);
    assert_eq!(
        fact.continuity.provenance.session_id.as_deref(),
        Some("session-approval-1")
    );
    assert!(fact
        .continuity
        .provenance
        .extractor_model
        .as_deref()
        .is_some_and(|model| !model.is_empty()));
    assert_eq!(
        fact.continuity.provenance.excerpt.as_deref(),
        Some("cargo nextest run")
    );
}

#[tokio::test]
async fn corrected_fact_preserves_supersession_history() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();

    for value in ["React", "Vue"] {
        pipeline
            .ingest(
                agent_id,
                &TurnSummary {
                    total_text: format!("the project uses {value}"),
                    ..TurnSummary::default()
                },
                None,
            )
            .await
            .expect("ingest technology fact");
    }

    let facts = store.list_facts(agent_id).expect("list facts");
    assert_eq!(facts.len(), 2);
    let active = facts
        .iter()
        .find(|fact| fact.continuity.status == MemoryStatus::Active)
        .expect("active replacement");
    let superseded = facts
        .iter()
        .find(|fact| fact.continuity.status == MemoryStatus::Superseded)
        .expect("superseded history");
    assert_eq!(active.value, serde_json::json!("Vue"));
    assert_eq!(
        superseded.continuity.superseded_by,
        Some(active.fact_id.to_hex())
    );
}

#[tokio::test]
async fn turn_summary_with_fact_text_produces_fact_candidate() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    let summary = TurnSummary {
        total_text: "the project uses React".to_string(),
        iterations: 1,
        ..TurnSummary::default()
    };

    let report = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");

    // One fact pattern matched + one task-outcome event.
    assert_eq!(report.candidates_extracted, 2);
    assert_eq!(report.candidates_refined, 2);
    assert_eq!(report.facts_written, 1);
    assert_eq!(report.events_written, 1);
    let facts = store.list_facts(agent_id).expect("list facts");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].key, "project_technology");
}

#[tokio::test]
async fn turn_summary_with_timed_out_outcome_labels_event() {
    let (store, pipeline) = pipeline_with_silent_llm();
    let agent_id = AgentId::generate();
    let summary = TurnSummary {
        iterations: 5,
        timed_out: true,
        ..TurnSummary::default()
    };

    let _ = pipeline
        .ingest(agent_id, &summary, None)
        .await
        .expect("ingest");
    let events = store.list_events(agent_id, 10).expect("list events");
    assert_eq!(events.len(), 1);
    assert!(
        events[0].summary.contains("timed_out"),
        "expected timed_out label, got: {}",
        events[0].summary,
    );
}
