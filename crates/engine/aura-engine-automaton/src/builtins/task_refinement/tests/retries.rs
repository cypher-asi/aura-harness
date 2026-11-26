use super::*;

#[tokio::test]
async fn works_without_event_channel() {
    // event_tx = None must not panic and must still run the
    // provider + update_task path.
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("Refined body.");
    let spec = sample_spec();
    let task = sample_task("Rotate tokens.");

    let out = refine_task_description(&domain, &provider, "test-model", &spec, &task, None)
        .await
        .expect("None event_tx must work");

    assert_eq!(provider.calls(), 1);
    assert_eq!(domain.updates().len(), 1);
    assert!(out.description.starts_with(REFINED_MARKER));
}

#[test]
fn assemble_refined_body_quotes_multiline_descriptions() {
    let task = sample_task("Line one.\nLine two.\nLine three.");
    let body = assemble_refined_body("Refined body.", &task);
    assert!(body.starts_with(REFINED_MARKER));
    assert!(body.contains("## Refined Description\nRefined body."));
    assert!(body.contains("> Line one."));
    assert!(body.contains("> Line two."));
    assert!(body.contains("> Line three."));
}

/// Phase 7 contract: after Phase 7 dropped the dev-loop
/// `build_retry_note.is_none()` outer gate, the
/// [`REFINED_MARKER`] short-circuit is the single authoritative
/// idempotency check. This regression test simulates the
/// build-retry second pass directly — call once, take the
/// persisted refined description, call again with that body —
/// and asserts the provider is invoked exactly once.
///
/// If a future patch reintroduces the redundant outer gate or
/// removes the marker, this test will fail loudly because the
/// second call will issue a second provider request.
#[tokio::test]
async fn refinement_runs_at_most_once_per_task_across_retries() {
    let domain = RecordingDomain::default();
    let provider = CountingProvider::with_text("Refined: rotate every 15m.");
    let spec = sample_spec();
    let first = sample_task("Rotate tokens.");

    let after_first =
        refine_task_description(&domain, &provider, "test-model", &spec, &first, None)
            .await
            .expect("first refinement must succeed");

    assert_eq!(provider.calls(), 1, "first pass must call the provider");
    assert!(
        after_first.description.starts_with(REFINED_MARKER),
        "first pass must persist the marker"
    );

    // Second pass emulates a build-retry: re-enter
    // `refine_task_description` with the now-marker-prefixed
    // description (this is what the dev-loop would do when
    // re-claiming the same task with a build_retry_note).
    let after_second =
        refine_task_description(&domain, &provider, "test-model", &spec, &after_first, None)
            .await
            .expect("second refinement must succeed via short-circuit");

    assert_eq!(
        provider.calls(),
        1,
        "second pass must NOT call the provider — the marker is the only gate, \
             so re-entry on a refined body short-circuits silently"
    );
    assert_eq!(
        after_second.description, after_first.description,
        "marker short-circuit must return the input description unchanged"
    );
    assert_eq!(
        domain.updates().len(),
        1,
        "update_task must have been called only on the first pass"
    );
}
