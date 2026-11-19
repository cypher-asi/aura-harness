//! `/compare`: both arms in separate copies, what each run records,
//! picking and applying a run, and the budget.

use super::*;
use aura_core_types::{
    ArmSpec, BenchSource, CompareArm, ComparePayload, CompareStatus, RefVerdict,
};
use aura_ui_terminal::events::{BenchRequest, BenchSpec, BenchWork};

const SONNET: &str = "aura-claude-sonnet-4-6";
const OPUS: &str = "aura-claude-opus-4-7";

/// Send `/compare <args>` from the first agent's chat.
async fn compare(state: &mut LoopState<'_>, links: &mpsc::Sender<auth::LinkOutcome>, args: &str) {
    let event = UiEvent::Compare {
        agent_id: state.active.to_hex(),
        args: args.to_string(),
    };
    handle_ui_event(state, event, links).await;
}

/// Prepare and start the comparison `args` asks for, and wait for it to
/// end.
async fn run_comparison(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
    args: &str,
) -> ComparePayload {
    compare(state, links, args).await;
    let estimate = system_texts(&drain(commands));
    assert!(estimate.contains("/compare go starts it"), "{estimate}");
    compare(state, links, "go").await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the comparison did not end");
    latest(state)
}

/// The latest state of the latest comparison.
fn latest(state: &LoopState<'_>) -> ComparePayload {
    let host = subagents::lock(&state.hub.registry)
        .compares()
        .last()
        .map(|agent| agent.agent_id())
        .expect("a comparison is registered");
    entries(state, host)
        .iter()
        .rev()
        .find_map(|entry| entry.tx.compare_payload())
        .expect("the comparison is recorded")
}

/// A record entry scan that works for agents with no open kernel too.
fn entries(state: &LoopState<'_>, agent_id: AgentId) -> Vec<aura_core_types::RecordEntry> {
    state
        .hub
        .factory
        .store
        .scan_record(agent_id, 1, 10_000)
        .unwrap()
}

fn signals_of(state: &LoopState<'_>, agent_id: AgentId) -> Vec<OutcomeSignal> {
    entries(state, agent_id)
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .map(|o| o.signal)
        .collect()
}

/// The `/compare` check proposed in `entry`, if it is one.
fn check_proposal(entry: &aura_core_types::RecordEntry) -> Option<aura_core_types::ToolProposal> {
    if entry.tx.tx_type != aura_core_types::TransactionType::ToolProposal {
        return None;
    }
    serde_json::from_slice::<aura_core_types::ToolProposal>(&entry.tx.payload)
        .ok()
        .filter(|p| p.source.as_deref() == Some("compare_check") && p.tool == "run_command")
}

fn turn_row(state: &LoopState<'_>, model: &str) -> Option<(u32, u32)> {
    state
        .hub
        .gate
        .stats()
        .row(RouteLevel::Turn, PromptCategory::Question, model)
        .map(|row| (row.tries, row.successes))
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test]
async fn both_arms_run_repeatedly_in_separate_copies_and_their_checks_feed_gate() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    std::fs::write(fixture.workspace.join("lib.txt"), "seed\n").unwrap();
    std::fs::create_dir(fixture.workspace.join("target")).unwrap();
    std::fs::write(fixture.workspace.join("target/build.out"), "big").unwrap();

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "write the answer --repeats 2 --check \"sh -c 'grep -q opus answer.txt'\"",
    )
    .await;

    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    assert_eq!(payload.against, OPUS);
    let names: Vec<&str> = payload.runs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["a-1", "b-1", "a-2", "b-2"]);
    let mut workdirs = Vec::new();
    for run in &payload.runs {
        assert_eq!(run.error, None, "{}", run.name);
        let model = match run.arm {
            CompareArm::A => SONNET,
            CompareArm::B => OPUS,
        };
        assert_eq!(run.model, model, "{}", run.name);
        let workdir = &run.copy.workdir;
        assert_ne!(workdir, &fixture.workspace);
        assert!(workdir.starts_with(fixture._dirs[0].path().join("compare")));
        assert_eq!(
            std::fs::read_to_string(workdir.join("answer.txt")).unwrap(),
            model,
            "each run wrote in its own copy"
        );
        assert_eq!(
            std::fs::read_to_string(workdir.join("lib.txt")).unwrap(),
            "seed\n"
        );
        assert!(
            !workdir.join("target/build.out").exists(),
            "build output is not copied"
        );
        assert!(
            run.copy.git_dir.is_some(),
            "a plain folder gets a plain copy"
        );
        assert_eq!(run.files_changed, 1);
        assert_eq!(run.check_passed, Some(run.arm == CompareArm::B));
        assert!(run.cost_usd > 0.0, "{}", run.name);
        assert_eq!(run.tokens[0].model, model);
        workdirs.push(workdir.clone());

        let prompts: Vec<PromptPayload> = entries(&state, run.agent_id)
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .collect();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].text, "write the answer");
        assert_eq!(prompts[0].route, Route::turn(model, RouteReason::Compare));
        let checked = if run.arm == CompareArm::A {
            OutcomeSignal::ChecksFailed
        } else {
            OutcomeSignal::ChecksPassed
        };
        assert_eq!(
            signals_of(&state, run.agent_id),
            [OutcomeSignal::None, checked],
            "the turn's outcome, then the check's"
        );
        let log = entries(&state, run.agent_id);
        let check_at = log
            .iter()
            .position(|e| check_proposal(e).is_some())
            .expect("the check is in the run's log");
        let proposal = check_proposal(&log[check_at]).unwrap();
        assert_eq!(
            proposal.args["shell_script"],
            "sh -c 'grep -q opus answer.txt'"
        );
        assert!(
            log[check_at..]
                .iter()
                .any(|e| e.tx.tx_type == aura_core_types::TransactionType::ToolExecution),
            "and so is its result"
        );
        let verdict_at = log
            .iter()
            .rposition(|e| e.tx.outcome_payload().is_some())
            .unwrap();
        assert!(check_at < verdict_at, "the check runs before its verdict");
    }
    workdirs.dedup();
    assert_eq!(workdirs.len(), 4);
    assert!(
        !fixture.workspace.join("answer.txt").exists(),
        "the user's folder is untouched"
    );

    assert_eq!(turn_row(&state, OPUS), Some((2, 2)));
    assert_eq!(turn_row(&state, SONNET), Some((2, 0)));

    assert_eq!(payload.judgments.len(), 2);
    for judgment in &payload.judgments {
        assert!(judgment.parsed);
        let expected = if judgment.a_first {
            CompareArm::A
        } else {
            CompareArm::B
        };
        assert_eq!(
            judgment.winner,
            Some(expected),
            "the judge picked attempt A"
        );
        assert_eq!(judgment.model, OPUS);
    }

    let shown = system_texts(&drain(&mut commands));
    assert!(shown.contains("Compare #1: a-1 done in"), "{shown}");
    let checks = shown
        .lines()
        .find(|line| line.trim_start().starts_with("check "))
        .expect(&shown);
    assert!(
        checks.contains("0/2 passed") && checks.contains("2/2 passed"),
        "{checks}"
    );
    let agents = state.hub.agent_summaries();
    for name in ["Compare #1", "a-1", "b-2"] {
        assert!(
            agents.iter().any(|a| a.name == name),
            "{name} is in the Agents panel"
        );
    }

    compare(&mut state, &links, "").await;
    let listed = system_texts(&drain(&mut commands));
    assert!(listed.contains("1. Compare #1"), "{listed}");

    compare(&mut state, &links, "dismiss").await;
    for workdir in &workdirs {
        assert!(!workdir.exists(), "{} was removed", workdir.display());
    }
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
}

#[tokio::test]
async fn pick_rates_both_arms_and_apply_brings_the_chosen_run_home() {
    if !git_available() {
        return;
    }
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let repo = &fixture.workspace;
    git(repo, &["init", "--quiet", "-b", "main"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("lib.txt"), "seed\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "seed"]);
    std::fs::write(repo.join("lib.txt"), "seed, edited\n").unwrap();

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "write the answer --repeats 1",
    )
    .await;
    let gate = payload.run("a-1").unwrap().clone();
    let other = payload.run("b-1").unwrap().clone();
    assert_eq!(gate.copy.git_dir, None, "a git repository gets a worktree");
    assert_eq!(
        std::fs::read_to_string(gate.copy.workdir.join("lib.txt")).unwrap(),
        "seed, edited\n",
        "uncommitted work is carried into the copy"
    );
    assert_eq!(gate.files_changed, 1, "only the run's own change counts");

    compare(&mut state, &links, "pick gate").await;
    assert!(system_texts(&drain(&mut commands))
        .contains("picked A (gate:nodelegate). Rated 1 A run good and 1 B run bad"));
    assert_eq!(
        signals_of(&state, gate.agent_id).last(),
        Some(&OutcomeSignal::Good)
    );
    assert_eq!(
        signals_of(&state, other.agent_id).last(),
        Some(&OutcomeSignal::Bad)
    );
    assert_eq!(turn_row(&state, SONNET), Some((1, 1)));
    assert_eq!(turn_row(&state, OPUS), Some((1, 0)));
    assert_eq!(latest(&state).status, CompareStatus::Picked(CompareArm::A));

    compare(&mut state, &links, "diff a-1").await;
    let diffs: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowDiff { files, .. } => Some(files[0].path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(diffs, ["answer.txt"]);

    compare(&mut state, &links, "apply b-1").await;
    let applied = system_texts(&drain(&mut commands));
    assert!(applied.contains("applied b-1 (1 file)"), "{applied}");
    assert_eq!(
        std::fs::read_to_string(repo.join("answer.txt")).unwrap(),
        OPUS
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("lib.txt")).unwrap(),
        "seed, edited\n"
    );
    assert!(!gate.copy.root.exists() && !other.copy.root.exists());
    assert_eq!(
        latest(&state).status,
        CompareStatus::Applied("b-1".to_string())
    );

    compare(&mut state, &links, "dismiss").await;
    assert!(system_texts(&drain(&mut commands)).contains("already removed"));
}

#[tokio::test]
async fn the_budget_stops_new_runs_once_spent() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "--budget 0.001 write the answer",
    )
    .await;

    let names: Vec<&str> = payload.runs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["a-1"], "one run spent the budget");
    assert!(payload.runs[0].cost_usd >= 0.001);
    assert!(
        payload
            .notes
            .iter()
            .any(|n| n.contains("5 runs did not start")),
        "{:?}",
        payload.notes
    );
    assert!(payload.judgments.is_empty(), "nothing to pair it with");
}

#[tokio::test]
async fn subtasks_of_a_run_follow_its_arm_not_the_users_selection() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();
    let candidates = gate_on_zos(&fixture).gate_candidates();
    let dispatch = |parent: AgentId| {
        let request: aura_core_types::SubagentDispatchRequest =
            serde_json::from_value(serde_json::json!({
                "parent_agent_id": parent,
                "subagent_type": "reviewer",
                "prompt": "what changed?",
                "parent_permissions": aura_core_types::AgentPermissions::full_access(),
                "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
            }))
            .unwrap();
        let dispatcher = Arc::clone(&fixture.dispatcher);
        async move {
            dispatcher
                .dispatch(request)
                .await
                .unwrap()
                .child_agent_id
                .unwrap()
        }
    };
    let route_of = |child: AgentId| {
        entries(&state, child)
            .iter()
            .find_map(|e| e.tx.prompt_payload())
            .unwrap()
            .route
    };

    fixture
        .hub
        .begin_compare_run(first, subagents::ArmRouting::Pinned(OPUS.to_string()));
    let pinned = dispatch(first).await;
    assert_eq!(
        route_of(pinned),
        Route::subtask(OPUS, RouteReason::Pinned),
        "the other arm's subtasks run on its model"
    );

    fixture.hub.begin_compare_run(
        first,
        subagents::ArmRouting::Gate {
            candidates,
            delegate: true,
        },
    );
    let routed = dispatch(first).await;
    assert_eq!(
        route_of(routed),
        Route::subtask(SONNET, RouteReason::Default),
        "the Gate arm's subtasks are routed though the user pinned a model"
    );
    let nested = dispatch(routed).await;
    assert_eq!(route_of(nested).model, SONNET, "and so are theirs");

    fixture.hub.end_compare_run(first);
    let plain = dispatch(first).await;
    assert_eq!(
        route_of(plain),
        Route::subtask("mock-model", RouteReason::Pinned)
    );
}

#[tokio::test]
async fn a_comparison_needs_a_prompt_and_waits_to_be_confirmed() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    compare(&mut state, &links, "").await;
    assert!(system_texts(&drain(&mut commands)).contains("No comparisons yet"));
    compare(&mut state, &links, "--repeats 2").await;
    assert!(system_texts(&drain(&mut commands)).contains("No prompt to compare yet"));
    compare(&mut state, &links, "go").await;
    assert!(system_texts(&drain(&mut commands)).contains("Nothing to start"));

    compare(&mut state, &links, "write the answer --against sonnet").await;
    let estimate = system_texts(&drain(&mut commands));
    assert!(
        estimate.contains(
            "A gate (likely aura-claude-sonnet-4-6 (cloud)) vs B aura-claude-sonnet-4-6 (cloud)"
        ),
        "{estimate}"
    );
    compare(&mut state, &links, "cancel").await;
    assert!(system_texts(&drain(&mut commands)).contains("Dropped the comparison"));
    assert!(subagents::lock(&state.hub.registry).compares().is_empty());
}

#[tokio::test]
async fn gate_is_compared_with_itself_without_enforced_delegation() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    compare(
        &mut state,
        &links,
        "write the answer across files --repeats 1 --against gate:nodelegate",
    )
    .await;
    let estimate = system_texts(&drain(&mut commands));
    assert!(
        estimate.contains("vs B gate without enforced delegation (gate:nodelegate)"),
        "{estimate}"
    );
    compare(&mut state, &links, "go").await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the comparison did not end");
    let payload = latest(&state);

    assert_eq!(payload.against, "gate:nodelegate");
    let gate = payload.run("a-1").unwrap();
    let other = payload.run("b-1").unwrap();
    // A change across several files is led, and starts on the cheapest
    // standard model.
    for run in [gate, other] {
        assert_eq!(run.error, None, "{}", run.name);
        assert_eq!(run.model, SONNET, "both arms are routed by Gate");
        let prompts: Vec<PromptPayload> = entries(&state, run.agent_id)
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .collect();
        assert_eq!(prompts[0].route, Route::turn(SONNET, RouteReason::Compare));
    }
    assert_eq!(gate.files_changed, 0, "the enforced lead may not write");
    assert!(!gate.copy.workdir.join("answer.txt").exists());
    assert_eq!(other.files_changed, 1, "the other lead writes itself");
    assert_eq!(
        std::fs::read_to_string(other.copy.workdir.join("answer.txt")).unwrap(),
        SONNET
    );
    let shown = system_texts(&drain(&mut commands));
    assert!(
        shown.contains("A gate vs B gate without enforced delegation"),
        "{shown}"
    );
}

#[tokio::test]
async fn a_runs_arm_decides_whether_its_lead_delegates() {
    let (fixture, _commands) = fixture();
    let first = fixture.first;
    let candidates = gate_on_zos(&fixture).gate_candidates();
    assert_eq!(
        fixture.hub.leads(first, None),
        None,
        "the user pinned a model"
    );
    for (delegate, leads) in [(true, Some(true)), (false, Some(false))] {
        fixture.hub.begin_compare_run(
            first,
            subagents::ArmRouting::Gate {
                candidates: candidates.clone(),
                delegate,
            },
        );
        assert_eq!(fixture.hub.leads(first, None), leads);
        assert_eq!(
            fixture.hub.leads(first, Some("implement")),
            None,
            "an instance of a permanent agent never leads"
        );
    }
    fixture
        .hub
        .begin_compare_run(first, subagents::ArmRouting::Pinned(OPUS.to_string()));
    assert_eq!(fixture.hub.leads(first, None), None);
}

/// Send `request` from the first agent's BENCH tab.
async fn bench(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    request: BenchRequest,
) {
    let event = UiEvent::Bench {
        agent_id: state.active.to_hex(),
        request,
    };
    handle_ui_event(state, event, links).await;
}

#[tokio::test]
async fn two_models_replay_a_session_from_its_start_prompt_by_prompt() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let notes = fixture.workspace.join("notes.txt");
    std::fs::write(&notes, "original\n").unwrap();
    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "write the answer",
        &[("notes.txt", Some("edited by the session\n"))],
    )
    .await;
    checkpointed_turn(&fixture, &mut state, first, "run failing check", &[]).await;

    bench(&mut state, &links, BenchRequest::Sessions).await;
    let sessions = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchSessions { sessions, .. } => Some(sessions),
            _ => None,
        })
        .expect("the sessions are listed");
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].current);
    assert_eq!(sessions[0].prompts, 2);
    assert_eq!(sessions[0].title, "write the answer");

    let spec = BenchSpec {
        work: BenchWork::CurrentSession,
        a: "sonnet".to_string(),
        b: "opus".to_string(),
        repeats: 1,
        check: None,
        budget_usd: None,
    };
    bench(&mut state, &links, BenchRequest::Prepare(spec)).await;
    let estimate = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchEstimate { estimate, .. } => Some(estimate),
            _ => None,
        })
        .expect("an estimate")
        .expect("the benchmark is set up");
    assert!(
        estimate
            .starts_with("Benchmark: replay the session \"write the answer\", 2 prompts in order"),
        "{estimate}"
    );
    assert!(
        estimate.contains("A aura-claude-sonnet-4-6 (cloud) vs B aura-claude-opus-4-7 (cloud)"),
        "{estimate}"
    );
    assert!(estimate.contains("not undone"), "{estimate}");

    bench(&mut state, &links, BenchRequest::Start).await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the benchmark did not end");
    let payload = latest(&state);

    assert_eq!(payload.status, CompareStatus::Done);
    assert_eq!(
        (&payload.arm_a, &payload.arm_b),
        (
            &ArmSpec::Model {
                model: SONNET.to_string()
            },
            &ArmSpec::Model {
                model: OPUS.to_string()
            }
        )
    );
    assert_eq!(payload.prompts, ["write the answer", "run failing check"]);
    let BenchSource::Session {
        baseline,
        reference,
        ..
    } = &payload.source
    else {
        panic!("the work is a session: {:?}", payload.source);
    };
    assert_eq!(baseline.calls, 0, "the session's turns were scripted");
    let reference = reference.as_ref().expect("what the user kept");
    assert!(reference.diff.contains("notes.txt"), "{}", reference.diff);
    assert!(reference.diff.contains("+edited by the session"));
    assert_eq!(reference.reply, "did run failing check");

    let names: Vec<&str> = payload.runs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["a-1", "b-1"]);
    for run in &payload.runs {
        assert_eq!(run.error, None, "{}", run.name);
        let model = if run.arm == CompareArm::A {
            SONNET
        } else {
            OPUS
        };
        assert_eq!(run.model, model);
        assert_eq!(
            std::fs::read_to_string(run.copy.workdir.join("notes.txt")).unwrap(),
            "original\n",
            "{} started from the files as they were when the session started",
            run.name
        );
        assert_eq!(
            std::fs::read_to_string(run.copy.workdir.join("answer.txt")).unwrap(),
            model
        );
        assert_eq!(run.turns.len(), 2, "{}", run.name);
        assert!(run.turns.iter().all(|t| t.model == model));
        assert!(run.turns[0].cost_usd > 0.0);
        assert_eq!(run.turns[1].signal, OutcomeSignal::ChecksFailed);
        let prompts: Vec<String> = entries(&state, run.agent_id)
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .map(|p| p.text)
            .collect();
        assert_eq!(prompts, ["write the answer", "run failing check"]);
    }
    assert_eq!(
        std::fs::read_to_string(&notes).unwrap(),
        "edited by the session\n",
        "the user's folder is untouched"
    );

    assert_eq!(payload.judgments.len(), 1);
    let judgment = &payload.judgments[0];
    let (shown_first, shown_second) = if judgment.a_first {
        (CompareArm::A, CompareArm::B)
    } else {
        (CompareArm::B, CompareArm::A)
    };
    assert_eq!(judgment.winner, Some(shown_first));
    assert_eq!(judgment.vs_reference(shown_first), Some(RefVerdict::AsGood));
    assert_eq!(judgment.vs_reference(shown_second), Some(RefVerdict::Worse));

    let shown = drain(&mut commands);
    let views: Vec<_> = shown
        .iter()
        .filter_map(|(_, c)| match c {
            UiCommand::BenchUpsert(view) => Some(view),
            _ => None,
        })
        .collect();
    let last = views.last().expect("the BENCH tab follows the benchmark");
    assert_eq!(last.name, "Compare #1");
    assert_eq!(last.prompts.len(), 2);
    assert_eq!(last.arms[0].turns[1].failed, 1);
    assert!(last.has_reference);
    let text = system_texts(&shown);
    assert!(text.contains("Per prompt"), "{text}");
    assert!(text.contains("vs yours"), "{text}");

    bench(&mut state, &links, BenchRequest::List).await;
    let listed = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchList(list) => Some(list),
            _ => None,
        })
        .unwrap();
    assert_eq!(listed.len(), 1);

    let id = listed[0].id.clone();
    bench(
        &mut state,
        &links,
        BenchRequest::Pick {
            id: id.clone(),
            arm: 1,
        },
    )
    .await;
    let notice = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchNotice(notice) => Some(notice),
            _ => None,
        })
        .unwrap()
        .unwrap();
    assert!(notice.contains(&format!("picked B ({OPUS})")), "{notice}");
    assert_eq!(latest(&state).status, CompareStatus::Picked(CompareArm::B));
    bench(&mut state, &links, BenchRequest::Dismiss { id }).await;
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
}

#[tokio::test]
async fn both_arms_must_differ_and_an_unknown_session_is_refused() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);

    compare(&mut state, &links, "write the answer --a opus --b opus").await;
    assert!(
        system_texts(&drain(&mut commands)).contains("Both arms run on"),
        "same arm twice"
    );
    compare(&mut state, &links, "--session 42").await;
    assert!(system_texts(&drain(&mut commands)).contains("No session started at 42"));
    compare(&mut state, &links, "--session current").await;
    assert!(system_texts(&drain(&mut commands)).contains("no session to replay yet"));
}
