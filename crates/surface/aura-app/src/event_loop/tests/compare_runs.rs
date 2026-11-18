//! `/compare`: both arms in separate copies, what each run records,
//! picking and applying a run, and the budget.

use super::*;
use aura_core_types::{CompareArm, ComparePayload, CompareStatus};

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

    assert_eq!(payload.status, CompareStatus::Done);
    assert_eq!(payload.against, OPUS);
    let names: Vec<&str> = payload.runs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["gate-1", "other-1", "gate-2", "other-2"]);
    let mut workdirs = Vec::new();
    for run in &payload.runs {
        assert_eq!(run.error, None, "{}", run.name);
        let model = match run.arm {
            CompareArm::Gate => SONNET,
            CompareArm::Other => OPUS,
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
        assert_eq!(run.check_passed, Some(run.arm == CompareArm::Other));
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
        let checked = if run.arm == CompareArm::Gate {
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
        let expected = if judgment.gate_first {
            CompareArm::Gate
        } else {
            CompareArm::Other
        };
        assert_eq!(
            judgment.winner,
            Some(expected),
            "the judge picked attempt A"
        );
        assert_eq!(judgment.model, OPUS);
    }

    let shown = system_texts(&drain(&mut commands));
    assert!(shown.contains("Compare #1: gate-1 done in"), "{shown}");
    let checks = shown
        .lines()
        .find(|line| line.trim_start().starts_with("check "))
        .expect(&shown);
    assert!(
        checks.contains("0/2 passed") && checks.contains("2/2 passed"),
        "{checks}"
    );
    let agents = state.hub.agent_summaries();
    for name in ["Compare #1", "gate-1", "other-2"] {
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
    let gate = payload.run("gate-1").unwrap().clone();
    let other = payload.run("other-1").unwrap().clone();
    assert_eq!(gate.copy.git_dir, None, "a git repository gets a worktree");
    assert_eq!(
        std::fs::read_to_string(gate.copy.workdir.join("lib.txt")).unwrap(),
        "seed, edited\n",
        "uncommitted work is carried into the copy"
    );
    assert_eq!(gate.files_changed, 1, "only the run's own change counts");

    compare(&mut state, &links, "pick gate").await;
    assert!(
        system_texts(&drain(&mut commands)).contains("Rated 1 gate run good and 1 other run bad")
    );
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
    assert_eq!(
        latest(&state).status,
        CompareStatus::Picked(CompareArm::Gate)
    );

    compare(&mut state, &links, "diff gate-1").await;
    let diffs: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowDiff { files, .. } => Some(files[0].path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(diffs, ["answer.txt"]);

    compare(&mut state, &links, "apply other-1").await;
    let applied = system_texts(&drain(&mut commands));
    assert!(applied.contains("applied other-1 (1 file)"), "{applied}");
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
        CompareStatus::Applied("other-1".to_string())
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
    assert_eq!(names, ["gate-1"], "one run spent the budget");
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
            "gate (likely aura-claude-sonnet-4-6 (cloud)) vs aura-claude-sonnet-4-6 (cloud)"
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
        estimate.contains("vs gate without enforced delegation (gate:nodelegate), 1 run each"),
        "{estimate}"
    );
    compare(&mut state, &links, "go").await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the comparison did not end");
    let payload = latest(&state);

    assert_eq!(payload.against, "gate:nodelegate");
    let gate = payload.run("gate-1").unwrap();
    let other = payload.run("other-1").unwrap();
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
        shown.contains("gate vs gate without enforced delegation"),
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
