//! Example tasks run from the BENCH tab: picked by suite, each run in a
//! fixture of its own and checked by its own check, then shown, applied
//! and dismissed.

use super::*;

/// A benchmark of `tasks` example tasks of `suite` (of any suite when
/// `None`), Sonnet against Opus.
fn spec_for(suite: Option<&str>, tasks: usize) -> BenchSpec {
    BenchSpec {
        work: BenchWork::Examples {
            suite: suite.map(str::to_string),
            tasks,
        },
        a: "sonnet".to_string(),
        b: "opus".to_string(),
        repeats: 1,
        check: None,
        budget_usd: None,
        classifier: None,
    }
}

#[tokio::test]
async fn a_suites_example_tasks_can_be_run_on_their_own() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let hard: Vec<&str> = crate::bench_pack::tasks()
        .iter()
        .filter(|t| t.suite() == "gate_hard")
        .map(|t| t.id.as_str())
        .collect();

    let gate_hard = spec_for(Some("gate_hard"), 2);
    bench(&mut state, &links, BenchRequest::Prepare(gate_hard)).await;
    let shown = bench_estimate(&mut commands)
        .expect("an estimate")
        .expect("the benchmark is set up");
    assert!(
        shown.contains(&format!("Tasks         {}, {}\n", hard[0], hard[1])),
        "{shown}"
    );

    let nope = spec_for(Some("nope"), 1);
    bench(&mut state, &links, BenchRequest::Prepare(nope)).await;
    let problem = bench_estimate(&mut commands)
        .expect("an answer")
        .expect_err("no such suite");
    assert!(problem.contains("gate_hard, polyglot"), "{problem}");

    let unrunnable = BenchSpec {
        classifier: Some("llama".to_string()),
        ..spec_for(Some("gate_hard"), 1)
    };
    bench(&mut state, &links, BenchRequest::Prepare(unrunnable)).await;
    let problem = bench_estimate(&mut commands)
        .expect("an answer")
        .expect_err("no such model to classify with");
    assert!(
        problem.starts_with("Classifier: No available model matches `llama`; available: "),
        "{problem}"
    );
    assert!(problem.contains(SONNET), "{problem}");
}

#[tokio::test]
async fn example_tasks_run_in_their_own_fixtures_with_their_own_checks() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    prepare_two_examples(&mut state, &links, &mut commands).await;
    bench(&mut state, &links, BenchRequest::Start).await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the benchmark did not end");
    let payload = latest(&state);

    let ids = assert_each_task_ran_on_both_arms(&payload);
    for run in &payload.runs {
        assert_example_run(&fixture, &state, run);
    }
    assert_a_1_has_its_tasks_files(&fixture, &payload, ids[0]).await;
    let pairs: Vec<(&str, &str)> = payload
        .judgments
        .iter()
        .map(|j| (j.a_run.as_str(), j.b_run.as_str()))
        .collect();
    assert_eq!(pairs, [("a-1", "b-1"), ("a-2", "b-2")]);

    let id = assert_the_bench_tab_followed(&mut commands, ids[1]);
    assert_nothing_to_apply(&mut state, &links, &mut commands, &id).await;
    bench(&mut state, &links, BenchRequest::Dismiss { id }).await;
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
    assert!(
        !payload.runs[0].copy.root.exists(),
        "the copies are removed"
    );
}

/// Checks that the BENCH tab lists the example tasks, then sets up a
/// benchmark of the first two from it and checks its estimate.
async fn prepare_two_examples(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
) {
    let pack = crate::bench_pack::tasks();
    bench(state, links, BenchRequest::Sessions).await;
    let (_, examples) = bench_sessions(commands).expect("the example tasks are listed");
    assert_eq!(examples.len(), pack.len());
    assert_eq!(
        (examples[0].suite.as_str(), examples[0].title.as_str()),
        (pack[0].suite(), pack[0].title.as_str())
    );

    bench(state, links, BenchRequest::Prepare(spec_for(None, 2))).await;
    let estimate = bench_estimate(commands)
        .expect("an estimate")
        .expect("the benchmark is set up");
    assert!(
        estimate.starts_with(&format!(
            "Benchmark: 2 example coding tasks\n  Tasks         {}, {}\n",
            pack[0].id, pack[1].id
        )),
        "{estimate}"
    );
    assert!(estimate.contains("(2 per arm)"), "{estimate}");
}

/// Checks that the benchmark ran the first two example tasks, each once on
/// either arm; returns their ids.
fn assert_each_task_ran_on_both_arms(payload: &ComparePayload) -> Vec<&str> {
    let pack = crate::bench_pack::tasks();
    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    assert!(payload.prompts.is_empty());
    let ids: Vec<&str> = payload
        .source
        .examples()
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(ids, [pack[0].id.as_str(), pack[1].id.as_str()]);
    let runs: Vec<(&str, Option<&str>)> = payload
        .runs
        .iter()
        .map(|r| (r.name.as_str(), r.task.as_deref()))
        .collect();
    assert_eq!(
        runs,
        [
            ("a-1", Some(ids[0])),
            ("b-1", Some(ids[0])),
            ("a-2", Some(ids[1])),
            ("b-2", Some(ids[1])),
        ]
    );
    ids
}

/// Checks that `run` worked outside the user's folder on its task's
/// prompts, and was checked by its task's own check.
fn assert_example_run(fixture: &Fixture, state: &LoopState<'_>, run: &CompareRun) {
    let task = crate::bench_pack::by_id(run.task.as_deref().unwrap()).unwrap();
    assert_eq!(run.error, None, "{}", run.name);
    assert!(
        !run.copy.workdir.starts_with(&fixture.workspace),
        "{} works outside the user's folder",
        run.name
    );
    assert!(run.check_passed.is_some(), "{} was checked", run.name);
    assert!(!run.copy.workdir.join(".gate_checks").exists());
    let prompts: Vec<String> = entries(state, run.agent_id)
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .map(|p| p.text)
        .collect();
    assert_eq!(prompts, task.prompts, "{}", run.name);
    let checks: Vec<serde_json::Value> = entries(state, run.agent_id)
        .iter()
        .filter_map(check_proposal)
        .map(|p| p.args["shell_script"].clone())
        .collect();
    assert_eq!(checks, [task.verify.as_str()], "{}", run.name);
}

/// Checks that a-1's copy holds every file of task `id`'s fixture, and the
/// user's folder none of them.
async fn assert_a_1_has_its_tasks_files(fixture: &Fixture, payload: &ComparePayload, id: &str) {
    let first = crate::bench_pack::by_id(id).unwrap();
    let dir = tempfile::tempdir().unwrap();
    first
        .materialize(dir.path(), std::path::Path::new("/nowhere"))
        .await
        .unwrap();
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(
            payload.runs[0].copy.workdir.join(&name).exists(),
            "a-1 has {name:?} from its task"
        );
        assert!(
            !fixture.workspace.join(&name).exists(),
            "the user's folder is untouched"
        );
    }
}

/// Checks what the BENCH tab was shown of the benchmark while and after it
/// ran, once `commands` are drained, `second` being the second task's id;
/// returns the benchmark's id.
fn assert_the_bench_tab_followed(commands: &mut mpsc::Receiver<UiCommand>, second: &str) -> String {
    let shown = drain(commands);
    let last = shown
        .iter()
        .rev()
        .find_map(|(_, c)| match c {
            UiCommand::BenchUpsert(view) => Some(view),
            _ => None,
        })
        .expect("the BENCH tab follows the benchmark");
    assert!(
        shown.iter().any(|(_, c)| matches!(
            c,
            UiCommand::BenchUpsert(view) if view.live.iter().any(|r| r.wall_ms.is_some())
        )),
        "the BENCH tab shows the runs under way"
    );
    assert!(last.live.is_empty());
    assert_eq!(last.source, "2 example tasks");
    assert_eq!(last.tasks.len(), 2);
    assert!(last
        .tasks
        .iter()
        .all(|t| t.arms[0].runs == 1 && t.arms[1].runs == 1));
    assert_eq!(last.runs[2].task.as_deref(), Some(second));
    last.id.clone()
}

/// Asks the BENCH tab to apply a-1 of benchmark `id`, and checks that it
/// answers that there is nothing to apply.
async fn assert_nothing_to_apply(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
    id: &str,
) {
    bench(
        state,
        links,
        BenchRequest::Apply {
            id: id.to_string(),
            run: "a-1".to_string(),
        },
    )
    .await;
    let notice = drain(commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchNotice(notice) => Some(notice),
            _ => None,
        })
        .unwrap();
    assert!(
        notice
            .as_ref()
            .is_err_and(|e| e.contains("nothing to apply")),
        "{notice:?}"
    );
}
