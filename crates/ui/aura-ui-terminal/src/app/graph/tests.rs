use super::*;
use crate::events::UiCommand;

fn node(id: &str, parent: Option<&str>, status: TraceNodeStatus) -> TraceNode {
    TraceNode {
        id: id.into(),
        kind: "step".into(),
        parent: parent.map(str::to_string),
        label: id.into(),
        status,
        ..TraceNode::default()
    }
}

fn loaded() -> App {
    let mut app = App::new();
    let mut decision = node("s2", Some("t1"), TraceNodeStatus::Warn);
    decision.links = vec![("triggered".into(), "s3".into())];
    app.process_command(UiCommand::GraphLoad {
        run_id: "r".into(),
        nodes: vec![
            node("run", None, TraceNodeStatus::Ok),
            node("t1", Some("run"), TraceNodeStatus::Ok),
            decision,
            node("s3", Some("t1"), TraceNodeStatus::Ok),
        ],
    });
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

#[test]
fn load_opens_view_and_selects_latest() {
    let app = loaded();
    assert_eq!(app.detail_view(), Some(&DetailView::Graph));
    assert_eq!(app.graph().selected_node().unwrap().id, "s3");
    let rows = app.graph().rows();
    assert_eq!(rows.len(), 4);
    assert!(rows[3].last);
    assert_eq!(rows[2].guides, vec![false, false]);
}

#[test]
fn links_are_bidirectional_and_tab_follows_them() {
    let mut app = loaded();
    assert!(app.graph().linked_to_selected().contains("s2"));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.graph().selected_node().unwrap().id, "s2");
}

#[test]
fn collapse_expand_and_issue_filter() {
    let mut app = loaded();
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.graph().selected_node().unwrap().id, "t1");
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.graph().rows().len(), 2);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.graph().rows().len(), 4);
    app.handle_key(key(KeyCode::Char('e')));
    let ids: Vec<&str> = app
        .graph()
        .rows()
        .iter()
        .map(|r| app.graph().nodes()[r.node].id.as_str())
        .collect();
    assert_eq!(ids, vec!["run", "t1", "s2"]);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.detail_view().is_none());
}

#[test]
fn labels_are_sent_and_shown() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut app = App::new().with_event_sender(tx);
    let mut step = node("s3", Some("run"), TraceNodeStatus::Ok);
    step.seq = Some(3);
    app.process_command(UiCommand::GraphLoad {
        run_id: "r".into(),
        nodes: vec![node("run", None, TraceNodeStatus::Ok), step],
    });
    app.handle_key(key(KeyCode::Char('-')));
    match rx.try_recv().unwrap() {
        UiEvent::LabelStep { run_id, seq, value } => {
            assert_eq!((run_id.as_str(), seq, value), ("r", Some(3), -1.0));
        }
        other => panic!("unexpected {other:?}"),
    }
    let shown = meta(app.graph().node("s3").unwrap(), "human");
    assert_eq!(shown.as_deref(), Some("-1"));
}

#[test]
fn o_opens_the_run_a_subagent_call_started_and_the_one_that_spawned_this() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let mut app = App::new().with_event_sender(tx);
    let mut run = node("run", None, TraceNodeStatus::Ok);
    run.kind = "run".into();
    run.meta = vec![("spawned_by".into(), "parent-run".into())];
    let mut call = node("tc:a", Some("run"), TraceNodeStatus::Ok);
    call.meta = vec![("child_run".into(), "child-run".into())];
    app.process_command(UiCommand::GraphLoad {
        run_id: "r".into(),
        nodes: vec![run, call],
    });
    app.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenGraph(Some(id))) if id == "child-run"));
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::OpenGraph(Some(id))) if id == "parent-run"));
}

#[test]
fn inspected_nodes_say_when_they_happened_to_the_millisecond() {
    let mut step = node("s3", None, TraceNodeStatus::Ok);
    step.meta = vec![("at_ms".into(), "1700000000123".into())];
    let shown = with_time(&step);
    let (key, time) = &shown.meta[0];
    assert_eq!(key, "time");
    assert!(time.ends_with(".123"), "{time}");
}

#[test]
fn r_jumps_to_the_linked_record_row() {
    let mut app = App::new();
    app.records.push_back(crate::app::detail_tests::record(4));
    app.records.push_back(crate::app::detail_tests::record(7));
    let mut step = node("tc:a", Some("run"), TraceNodeStatus::Ok);
    step.meta = vec![("record_seqs".into(), "[7,8]".into())];
    app.process_command(UiCommand::GraphLoad {
        run_id: "r".into(),
        nodes: vec![node("run", None, TraceNodeStatus::Ok), step],
    });
    app.handle_key(key(KeyCode::Char('r')));
    assert_eq!(app.detail_view(), Some(&DetailView::Record));
    assert_eq!(app.selected_record(), 1);
}

#[test]
fn feed_entries_open_as_json_and_g_opens_the_graph() {
    let mut app = loaded();
    app.process_command(UiCommand::GraphUpsert {
        run_id: "r".into(),
        nodes: vec![TraceNode {
            kind: "decision".into(),
            ..node("s2", Some("t1"), TraceNodeStatus::Warn)
        }],
    });
    assert_eq!(app.trace_feed().entries().len(), 1);
    app.close_detail();
    app.open_selected_trace_entry();
    let Some(DetailView::File(view)) = app.detail_view() else {
        panic!("the step opens as a file view");
    };
    assert!(view.title.starts_with("TRACE DECISION"), "{}", view.title);
    assert!(matches!(&view.body, crate::app::file_view::FileBody::Text(t) if t.contains("\"s2\"")));

    app.close_detail();
    app.open_selected_trace_in_graph();
    assert_eq!(app.detail_view(), Some(&DetailView::Graph));
    assert_eq!(app.graph().selected_node().unwrap().id, "s2");
}

#[test]
fn upserts_for_other_runs_are_ignored() {
    let mut app = loaded();
    app.process_command(UiCommand::GraphUpsert {
        run_id: "other".into(),
        nodes: vec![node("s9", Some("t1"), TraceNodeStatus::Ok)],
    });
    assert!(app.graph().node("s9").is_none());
    let mut updated = node("s3", Some("t1"), TraceNodeStatus::Error);
    updated.detail = "failed".into();
    app.process_command(UiCommand::GraphUpsert {
        run_id: "r".into(),
        nodes: vec![updated, node("s4", Some("t1"), TraceNodeStatus::Pending)],
    });
    assert_eq!(
        app.graph().node("s3").unwrap().status,
        TraceNodeStatus::Error
    );
    assert_eq!(app.graph().rows().len(), 5);
}
