use super::*;

#[tokio::test]
async fn skips_when_marker_present() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("should never run");
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("<!-- aura-refined:v1 -->\n## Refined Description\nalready done");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("marker short-circuit must succeed");

    assert_eq!(
        out.description, task.description,
        "marker short-circuit must return the input description unchanged"
    );
    assert_eq!(
        provider.calls(),
        0,
        "marker short-circuit must not call the provider"
    );
    assert!(
        domain.updates().is_empty(),
        "marker short-circuit must not call update_task"
    );
    assert!(
        drain(&mut rx).is_empty(),
        "marker short-circuit must emit no events"
    );
}

#[tokio::test]
async fn falls_back_on_provider_error() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_error();
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Make the tokens rotate every 15 minutes.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("provider failure must not error the caller");

    assert_eq!(
        out.description, task.description,
        "provider failure must return the original description"
    );
    assert!(
        domain.updates().is_empty(),
        "provider failure must not persist anything"
    );

    let events = drain(&mut rx);
    let mut saw_refining = false;
    let mut saw_log = false;
    let mut saw_refined = false;
    for e in &events {
        match e {
            AutomatonEvent::TaskDescriptionRefining { task_id } => {
                assert_eq!(task_id, "task-1");
                saw_refining = true;
            }
            AutomatonEvent::LogLine { message } => {
                assert!(
                    message.contains("task-1") && message.contains("refinement failed"),
                    "LogLine must carry the failure context, got: {message}"
                );
                saw_log = true;
            }
            AutomatonEvent::TaskDescriptionRefined { .. } => {
                saw_refined = true;
            }
            other => panic!("unexpected event on provider-failure path: {other:?}"),
        }
    }
    assert!(
        saw_refining,
        "refining event must still fire before the call"
    );
    assert!(saw_log, "exactly one LogLine must capture the failure");
    assert!(
        !saw_refined,
        "the success event must NOT fire when the provider rejected the call"
    );
}

#[tokio::test]
async fn persists_refined_description() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text(
        "Rotate refresh tokens every 15 minutes and revoke prior keys.",
    );
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("happy path must succeed");

    assert_refined_description(&out.description);

    let updates = domain.updates();
    assert_eq!(updates.len(), 1, "update_task must be called exactly once");
    let (id, update) = &updates[0];
    assert_eq!(id, "task-1");
    let persisted = update
        .description
        .as_deref()
        .expect("update payload must carry the new description");
    assert!(persisted.starts_with(REFINED_MARKER));
    assert!(persisted.contains("## Original Task"));

    assert_happy_path_events(&drain(&mut rx));
}

/// The returned description carries the marker, the quoted original and
/// the model output.
fn assert_refined_description(description: &str) {
    assert!(
        description.starts_with(REFINED_MARKER),
        "returned description must carry the marker, got: {}",
        description
    );
    assert!(
        description.contains("## Original Task"),
        "returned description must preserve the original task block"
    );
    assert!(
        description.contains("> Rotate tokens."),
        "original description must be quoted line-by-line"
    );
    assert!(
        description.contains("Rotate refresh tokens every 15 minutes"),
        "refined model output must be embedded in the body"
    );
}

/// The happy path emits `TaskDescriptionRefining` then
/// `TaskDescriptionRefined` for `task-1`, and nothing else.
fn assert_happy_path_events(events: &[AutomatonEvent]) {
    let mut saw_refining = false;
    let mut saw_refined = false;
    for e in events {
        match e {
            AutomatonEvent::TaskDescriptionRefining { task_id } => {
                assert_eq!(task_id, "task-1");
                saw_refining = true;
            }
            AutomatonEvent::TaskDescriptionRefined { task_id } => {
                assert_eq!(task_id, "task-1");
                saw_refined = true;
            }
            AutomatonEvent::LogLine { message } => {
                panic!("happy path must not emit a LogLine, got: {message}")
            }
            other => panic!("unexpected event on happy path: {other:?}"),
        }
    }
    assert!(saw_refining, "TaskDescriptionRefining must fire");
    assert!(saw_refined, "TaskDescriptionRefined must fire on success");
}

#[tokio::test]
async fn empty_provider_output_falls_back_to_original() {
    // A whitespace-only model response is functionally indistinguishable
    // from a failure — the original description is strictly more
    // informative. Lock in the fall-through.
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("   \n\t ");
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("empty provider output must not error the caller");

    assert_eq!(out.description, task.description);
    assert!(
        domain.updates().is_empty(),
        "empty refinement must not persist anything"
    );
    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::LogLine { .. })),
        "empty refinement must emit a LogLine"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::TaskDescriptionRefined { .. })),
        "empty refinement must NOT emit the success event"
    );
}

#[tokio::test]
async fn falls_back_on_update_task_error() {
    let domain = RecordingDomain {
        update_should_fail: true,
        ..Default::default()
    };
    let provider = CountingProvider::with_text("Refined body.");
    let (tx, mut rx) = mpsc::channel::<AutomatonEvent>(8);
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, Some(&tx))
        .await
        .expect("update_task failure must not error the caller");

    assert_eq!(out.description, task.description);
    assert_eq!(
        provider.calls(),
        1,
        "provider must still have been called once"
    );
    assert_eq!(
        domain.updates().len(),
        1,
        "update_task is still attempted exactly once"
    );

    let events = drain(&mut rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::LogLine { .. })),
        "update_task failure must emit a LogLine"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AutomatonEvent::TaskDescriptionRefined { .. })),
        "update_task failure must NOT emit the success event"
    );
}
