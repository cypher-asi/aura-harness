use super::support::*;
use super::*;

async fn row_once_named(fixture: &mut Fixture, child: AgentId) -> AgentSummary {
    let child = child.to_hex();
    let wait = async {
        loop {
            match fixture.commands.recv().await {
                Some(UiCommand::SetAgents(rows)) => {
                    if let Some(row) = rows
                        .into_iter()
                        .find(|r| r.id == child && r.subtitle.is_some())
                    {
                        return row;
                    }
                }
                Some(_) => {}
                None => panic!("the hub went away before the subtask was named"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("the subtask was never named")
}

#[tokio::test]
async fn a_spawned_task_is_named_after_its_subtask_by_the_title_model() {
    let mut fixture = titled_fixture("Fix parser escapes", "mock-title");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(
            parent,
            "reviewer",
            "The parser mangles backslash escapes in strings; fix it",
        ))
        .await
        .unwrap();
    let child = result.child_agent_id.unwrap();
    let row = row_once_named(&mut fixture, child).await;

    let (name, code) = {
        let registry = lock(&fixture.hub.registry);
        let agent = registry.find(child).unwrap();
        assert_eq!(agent.subtitle.as_deref(), Some("Fix parser escapes"));
        (agent.name.clone(), agent.code.clone().unwrap())
    };
    assert_eq!(name, "reviewer #1", "the name stays the agent's identity");
    assert_eq!(row.subtitle.as_deref(), Some("Fix parser escapes"));
    assert_eq!(row.name, format!("{code}: Fix parser escapes"));
    let listed = fixture.hub.agent_summaries();
    let listed = listed.iter().find(|r| r.id == child.to_hex()).unwrap();
    assert_eq!(listed.name, row.name);
}

/// The commands `fixture` sent the UI so far, without waiting.
fn sent_commands(fixture: &mut Fixture) -> Vec<UiCommand> {
    std::iter::from_fn(|| fixture.commands.try_recv().ok()).collect()
}

fn thread_subjects(commands: &[UiCommand]) -> Vec<(String, String)> {
    commands
        .iter()
        .filter_map(|command| match command {
            UiCommand::SetThreadSubject { thread_id, subject } => {
                Some((thread_id.clone(), subject.clone()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_spawned_tasks_inbox_thread_gets_a_subject_from_the_title_model() {
    let mut fixture = titled_fixture("Parser escapes", "mock-title");
    let _mail = fixture.hub.open_mail();
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(
            parent,
            "reviewer",
            "Work ONLY in /tmp/x\nThe parser mangles backslash escapes; fix it",
        ))
        .await
        .unwrap();
    let thread = task_id(result.child_agent_id.unwrap());

    let wait = async {
        loop {
            match fixture.commands.recv().await {
                Some(UiCommand::SetThreadSubject { thread_id, subject }) => {
                    break (thread_id, subject)
                }
                Some(_) => {}
                None => panic!("the UI channel closed"),
            }
        }
    };
    let (thread_id, subject) = tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("the thread was never given a subject");
    assert_eq!(thread_id, thread);
    assert_eq!(subject, "Parser escapes");
}

#[tokio::test]
async fn task_threads_get_no_subject_call_in_a_benchmark_or_without_mail() {
    // Mail collected, but under a benchmark's run.
    let mut bench = titled_fixture("Model subject", "mock-title");
    let _mail = bench.hub.open_mail();
    let parent = lock(&bench.hub.registry).active().agent_id();
    bench
        .hub
        .traces
        .start(parent, RunMeta::new("mock-model", RunSource::Bench))
        .unwrap();
    bench
        .dispatcher
        .dispatch(request(parent, "reviewer", "check the build"))
        .await
        .unwrap();
    // No call was started, so none can still be on its way.
    assert!(thread_subjects(&sent_commands(&mut bench)).is_empty());

    // A model to ask, but no INBOX to show the subject (`aura -p`).
    let mut headless = titled_fixture("Model subject", "mock-title");
    let parent = lock(&headless.hub.registry).active().agent_id();
    headless
        .dispatcher
        .dispatch(request(parent, "reviewer", "check the build"))
        .await
        .unwrap();
    assert!(thread_subjects(&sent_commands(&mut headless)).is_empty());
}

#[tokio::test]
async fn a_thread_whose_sender_is_not_running_is_not_titled() {
    let mut fixture = titled_fixture("Model subject", "mock-title");
    let idle = lock(&fixture.hub.registry).active().agent_id();
    fixture
        .hub
        .title_thread(idle, "t1".into(), "Check the lexer".into());
    assert!(thread_subjects(&sent_commands(&mut fixture)).is_empty());
}

#[tokio::test]
async fn without_a_title_model_a_subtask_is_named_from_its_prompt() {
    let mut fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(
            parent,
            "reviewer",
            "review the diff\nthen the tests",
        ))
        .await
        .unwrap();
    let child = result.child_agent_id.unwrap();
    let row = row_once_named(&mut fixture, child).await;

    assert_eq!(row.subtitle.as_deref(), Some("review the diff"));
    assert!(row.name.ends_with(": review the diff"), "{}", row.name);
}

#[tokio::test]
async fn subtasks_of_a_benchmark_run_are_named_without_a_model_call() {
    let fixture = titled_fixture("Model subtitle", "mock-title");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    fixture
        .hub
        .traces
        .start(parent, RunMeta::new("mock-model", RunSource::Bench))
        .unwrap();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "check the build\nand report"))
        .await
        .unwrap();

    let child = result.child_agent_id.unwrap();
    let registry = lock(&fixture.hub.registry);
    let agent = registry.find(child).unwrap();
    assert_eq!(agent.subtitle.as_deref(), Some("check the build"));
}

#[tokio::test]
async fn subtasks_nested_under_a_benchmark_run_are_named_without_a_model_call() {
    let fixture = titled_fixture("Model subtitle", "mock-title");
    let (run, run_zns) = {
        let registry = lock(&fixture.hub.registry);
        let run = registry.active();
        (run.agent_id(), run.zns_id.clone())
    };
    fixture
        .hub
        .traces
        .start(run, RunMeta::new("mock-model", RunSource::Bench))
        .unwrap();
    // A subtask of the run (`REN1`), working on a trace of its own that is
    // no bench run, as a spawned instance's is.
    let middle = lock(&fixture.hub.registry)
        .create_instance("rust-engineer", Some(run_zns))
        .unwrap()
        .agent_id();
    fixture
        .hub
        .traces
        .start(middle, RunMeta::new("mock-model", RunSource::Tui))
        .unwrap();
    assert!(fixture.hub.in_bench(middle));

    let result = fixture
        .dispatcher
        .dispatch(request(middle, "reviewer", "check the build\nand report"))
        .await
        .unwrap();

    // Named at once from the prompt; a model call would have answered
    // "Model subtitle", and only later.
    let child = result.child_agent_id.unwrap();
    assert!(fixture.hub.in_bench(child));
    let registry = lock(&fixture.hub.registry);
    let agent = registry.find(child).unwrap();
    assert_eq!(
        agent.parent.as_deref(),
        registry.find(middle).map(|a| a.zns_id.as_str())
    );
    assert_eq!(agent.subtitle.as_deref(), Some("check the build"));
}

#[tokio::test]
async fn in_bench_needs_a_bench_run_above_and_survives_a_parent_cycle() {
    let fixture = fixture("unused");
    let root = lock(&fixture.hub.registry).active().clone();
    let child = lock(&fixture.hub.registry)
        .create_instance("reviewer", Some(root.zns_id.clone()))
        .unwrap();
    assert!(!fixture.hub.in_bench(root.agent_id()));
    assert!(!fixture.hub.in_bench(child.agent_id()));
    let unknown = AgentId::new([7; 32]);
    assert!(!fixture.hub.in_bench(unknown));

    // A hand-edited registry can make the root its child's child.
    {
        let mut registry = lock(&fixture.hub.registry);
        let store = registry.store().clone();
        let mut snapshot = store.load().unwrap();
        let mut row = snapshot
            .agents
            .drain(..)
            .find(|agent| agent.agent_id == root.agent_id())
            .unwrap();
        row.parent = Some(child.agent_id());
        store
            .apply(&aura_store_db::agents::RegistryDiff {
                agents_put: vec![row],
                ..Default::default()
            })
            .unwrap();
        *registry =
            AgentRegistry::open(store, fixture._data.path(), fixture.workspace.path()).unwrap();
    }
    assert!(!fixture.hub.in_bench(child.agent_id()));
    fixture
        .hub
        .traces
        .start(
            root.agent_id(),
            RunMeta::new("mock-model", RunSource::Bench),
        )
        .unwrap();
    assert!(fixture.hub.in_bench(child.agent_id()));
}

#[tokio::test]
async fn apply_subtitle_names_only_spawned_instances_once() {
    let fixture = fixture("unused");
    let parent = lock(&fixture.hub.registry).active().clone();
    let spawned = lock(&fixture.hub.registry)
        .create_instance("reviewer", Some(parent.zns_id.clone()))
        .unwrap();

    fixture
        .hub
        .apply_subtitle(parent.agent_id(), "Not a subtask")
        .await;
    fixture
        .hub
        .apply_subtitle(spawned.agent_id(), "Fix parser")
        .await;
    fixture
        .hub
        .apply_subtitle(spawned.agent_id(), "Renamed")
        .await;

    let rows = fixture.hub.agent_summaries();
    let row = |id: AgentId| rows.iter().find(|r| r.id == id.to_hex()).unwrap();
    assert_eq!(row(parent.agent_id()).subtitle, None);
    assert_eq!(row(parent.agent_id()).name, parent.display_name());
    let code = spawned.code.as_deref().unwrap();
    assert_eq!(
        row(spawned.agent_id()).subtitle.as_deref(),
        Some("Fix parser")
    );
    assert_eq!(row(spawned.agent_id()).name, format!("{code}: Fix parser"));
}
