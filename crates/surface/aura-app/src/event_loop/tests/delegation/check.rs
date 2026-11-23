//! A task's check: it decides the subtask, comes back in the compact
//! result, and runs without the task records that quote the prompts.

use super::*;
use aura_core_types::SubagentExit;

#[tokio::test]
async fn a_check_decides_the_subtask_and_comes_back_in_a_compact_result() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();
    fixture.hub.set_permissions(first, yolo());

    let passed = dispatch(
        &fixture,
        task(first, "implement", "write a note", Some("ls notes")),
    )
    .await;
    assert!(matches!(passed.exit, SubagentExit::Completed));
    assert_eq!(passed.files_changed, ["notes/mock-model.txt"]);
    assert_eq!(
        passed.final_message,
        "Done.\n\nFiles changed: notes/mock-model.txt\n\nCheck `ls notes`: passed"
    );
    let (_, outcomes) = record(&fixture, passed.child_agent_id.unwrap());
    assert!(
        outcomes.is_empty(),
        "Gate learns nothing from a pinned subtask: {outcomes:?}"
    );

    let failed = dispatch(
        &fixture,
        task(
            first,
            "implement",
            "run passing check",
            Some("ls no-such-dir"),
        ),
    )
    .await;
    assert!(
        failed
            .final_message
            .contains("Check `ls no-such-dir`: failed with exit code"),
        "the task's check, not the subagent's own passing one, is the verdict: {}",
        failed.final_message
    );
    assert!(
        !failed.final_message.contains("Retried"),
        "a pinned subtask is never escalated"
    );
    assert_eq!(children(&fixture, first, "implement").len(), 2);
    let shown = system_texts(&drain(&mut commands));
    assert!(shown.contains("Check `ls notes`: passed"), "{shown}");
}

#[tokio::test]
async fn a_check_runs_without_the_task_records_that_quote_the_prompts() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();
    fixture.hub.set_permissions(first, yolo());

    let done = dispatch(
        &fixture,
        task(
            first,
            "implement",
            "rename calc_total",
            Some("test ! -e .aura/tasks"),
        ),
    )
    .await;
    assert!(
        done.final_message
            .contains("Check `test ! -e .aura/tasks`: passed"),
        "{}",
        done.final_message
    );
    let kept = crate::event_loop::tasks::load_all(&fixture.workspace);
    assert_eq!(kept.len(), 1, "the records are back after the check");
    assert_eq!(kept[0].view.title, "rename calc_total");
}
