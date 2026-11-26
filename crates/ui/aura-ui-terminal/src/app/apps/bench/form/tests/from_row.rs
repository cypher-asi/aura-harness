use super::*;

#[test]
fn the_from_row_starts_on_the_tree_on_screen_and_cycles_top_level_agents() {
    let mut app = app_with_trees();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.event_tx = Some(tx);
    app.open_bench_form();
    assert_eq!(app.bench.form.from.as_deref(), Some("a"), "kid's tree");
    assert_eq!(app.bench_from_label().as_deref(), Some("a · web"));
    let offered: Vec<&str> = app
        .bench_from_choices()
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(offered, ["a", "b"], "no child, run or archived agent");

    app.bench.sessions = vec![session(5)];
    app.bench.form.work = 3;
    assert!(matches!(app.bench.work(), WorkChoice::Session(_)));
    app.bench.form.field = FormField::From;
    app.change_form_value(true);
    assert_eq!(app.bench.form.from.as_deref(), Some("b"));
    assert!(app.bench.sessions.is_empty(), "a's sessions go");
    assert_eq!(app.bench.form.work, DEFAULT_WORK, "and so does the pick");
    app.change_form_value(true);
    assert_eq!(app.bench.form.from.as_deref(), Some("a"), "and wraps");
    assert_eq!(
        requests(&mut rx),
        ["a Sessions", "b Sessions", "a Sessions"]
    );

    app.cmd_bench_sessions("b", vec![session(7)], Vec::new());
    assert!(app.bench.sessions.is_empty(), "another agent's sessions");
    app.cmd_bench_sessions("a", vec![session(7)], Vec::new());
    assert_eq!(app.bench.sessions.len(), 1);
}

#[test]
fn the_benchmark_is_prepared_and_started_from_the_agent_on_the_from_row() {
    let mut app = app_with_trees();
    app.open_bench_form();
    app.bench.form.field = FormField::From;
    app.change_form_value(true);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.event_tx = Some(tx);
    app.bench.form.field = FormField::Go;

    app.submit_bench_form();
    app.cmd_bench_estimate("a", Ok("about $1".to_string()));
    assert!(app.bench.form.waiting, "another agent's estimate");
    app.cmd_bench_estimate("b", Ok("about $2".to_string()));
    assert!(app.bench.form.ready());
    app.submit_bench_form();
    let sent = requests(&mut rx);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].starts_with("b Prepare("), "{sent:?}");
    assert_eq!(sent[1], "b Start");
}
