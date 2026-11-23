//! `/compare`: both arms in separate copies, what each run records,
//! picking and applying a run, and the budget.

use super::*;
use crate::event_loop::compare::view::{duration, money, render};
use aura_core_types::{
    ArmSpec, BenchSource, CompareArm, CompareCopy, ComparePayload, CompareRun, CompareStatus,
    Difficulty, RefVerdict, RunTurn,
};
use aura_ui_terminal::events::{BenchRequest, BenchSpec, BenchStatus, BenchWork, MessageRole};

const SONNET: &str = "aura-claude-sonnet-4-6";
const OPUS: &str = "aura-claude-opus-4-7";
const GPT: &str = "aura-gpt-5-4";

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
pub(super) async fn run_comparison(
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
pub(super) fn entries(
    state: &LoopState<'_>,
    agent_id: AgentId,
) -> Vec<aura_core_types::RecordEntry> {
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
    {
        let registry = subagents::lock(&state.hub.registry);
        let host = registry.compares().last().unwrap().zns_id.clone();
        let rows = registry.tree();
        let at = rows
            .iter()
            .position(|(agent, _)| agent.zns_id == host)
            .unwrap();
        let arm = rows[at].1 + 1;
        let tree: Vec<(&str, usize)> = rows[at + 1..]
            .iter()
            .take_while(|(_, depth)| *depth >= arm)
            .map(|(agent, depth)| (agent.name.as_str(), *depth))
            .collect();
        let (a, b) = (
            format!("A · {}", payload.arm_a.label()),
            format!("B · {OPUS}"),
        );
        assert_eq!(
            tree,
            [
                (a.as_str(), arm),
                ("a-1", arm + 1),
                ("a-2", arm + 1),
                (b.as_str(), arm),
                ("b-1", arm + 1),
                ("b-2", arm + 1),
            ],
            "each arm's runs are listed under an agent of its own"
        );
        for run in &payload.runs {
            assert!(registry.under_compare(run.agent_id), "{}", run.name);
        }
    }
    let mut workdirs = Vec::new();
    let mut bench_runs = Vec::new();
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
        let route = Route::turn(model, RouteReason::Compare);
        let route = if run.arm == CompareArm::A {
            route.at_effort("medium")
        } else {
            route
        };
        assert_eq!(prompts[0].route, route, "Gate's arm records its effort");
        assert_eq!(
            prompts[0].led,
            Some(false),
            "a run that does not lead works direct"
        );
        let checked = if run.arm == CompareArm::A {
            OutcomeSignal::ChecksFailed
        } else {
            OutcomeSignal::ChecksPassed
        };
        assert_eq!(
            signals_of(&state, run.agent_id),
            [OutcomeSignal::Unavailable, checked],
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

        let (trace, path) = state
            .hub
            .traces
            .run_log(run.agent_id, &run.name, &[])
            .expect("the run is traced");
        assert!(
            path.ends_with("log.json") && path.is_file(),
            "{}",
            path.display()
        );
        assert_eq!(trace.run.source, "bench", "{}", run.name);
        let arm = match run.arm {
            CompareArm::A => "A · ",
            CompareArm::B => "B · ",
        };
        assert!(
            trace.run.arm.as_deref().is_some_and(|a| a.starts_with(arm)),
            "{:?}",
            trace.run.arm
        );
        assert!(trace
            .run
            .title
            .ends_with(&format!("{}: write the answer", run.name)));
        assert_eq!(trace.scorecard.turns, 1, "one prompt, one turn");
        assert_eq!(
            trace.scorecard.outcome,
            Some(if run.check_passed == Some(true) {
                1.0
            } else {
                0.0
            }),
            "its check is its outcome"
        );
        bench_runs.push(trace.run.run_id.clone());
    }
    let (requester, also) = {
        let registry = subagents::lock(&state.hub.registry);
        let host = registry.compares().last().unwrap().agent_id();
        let requester = registry.ancestry(host)[1];
        assert_eq!(
            requester, state.active,
            "the comparison sits under the agent that asked"
        );
        let also = crate::event_loop::log_view::benchmark_runs(&registry, requester, None);
        let names: Vec<&str> = also
            .iter()
            .map(|&id| registry.find(id).unwrap().name.as_str())
            .collect();
        assert_eq!(
            names,
            ["a-1", "b-1", "a-2", "b-2"],
            "the runs, as they started"
        );
        (requester, also)
    };
    let (trace, path) = state
        .hub
        .traces
        .run_log(requester, "main", &also)
        .expect("the requester's log holds its benchmark's runs");
    assert!(path.is_file(), "{}", path.display());
    for run in &bench_runs {
        assert!(trace.find(run).is_some(), "{run} is in the requester's log");
    }
    let view = crate::event_loop::log_view::view(&requester.to_hex(), &trace, &path);
    let root = view.root().unwrap();
    let listed: Vec<&str> = root
        .runs
        .iter()
        .map(|&i| view.runs[i].run_id.as_str())
        .collect();
    assert_eq!(
        listed, bench_runs,
        "no turn started them: they follow its turns"
    );
    assert!(
        root.runs.iter().all(|&i| view.runs[i].depth == 1),
        "{:?}",
        view.runs
    );
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
async fn the_judge_compares_the_pairs_at_once() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "write the answer, judged together --repeats 2",
    )
    .await;

    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    let pairs: Vec<(&str, &str, bool)> = (payload.judgments.iter())
        .map(|j| (j.a_run.as_str(), j.b_run.as_str(), j.parsed))
        .collect();
    assert_eq!(
        pairs,
        [("a-1", "b-1", true), ("a-2", "b-2", true)],
        "both pairs are judged together, and kept in order: {:#?}",
        payload.judgments
    );
}

#[tokio::test]
async fn apply_brings_the_chosen_run_home() {
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
    let registry = subagents::lock(&state.hub.registry);
    let runs: Vec<&str> = registry
        .agents()
        .iter()
        .filter(|a| registry.under_compare(a.agent_id()))
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(runs, ["a-1"], "the runs that did not start are dropped");
}

#[tokio::test]
async fn a_comparison_recorded_running_that_nothing_runs_is_stopped() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);
    let mut payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "--budget 0.001 write the answer",
    )
    .await;

    // As Aura leaves it when it quits mid-run: the last record says running.
    let host = subagents::lock(&state.hub.registry).compares()[0].clone();
    payload.status = CompareStatus::Running;
    let kernel = compare::kernel_of(&mut state, host.agent_id()).unwrap();
    let ids = (host.agent_id(), host.name.as_str());
    compare::run::save(&state.hub, &kernel, state.commands, ids, &payload).await;
    drain(&mut commands);

    bench(&mut state, &links, BenchRequest::List).await;
    let listed = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchList { benches, .. } => Some(benches),
            _ => None,
        })
        .unwrap();
    assert_eq!(listed[0].status, BenchStatus::Cancelled);
    assert!(
        listed[0]
            .notes
            .iter()
            .any(|n| n.contains("stopped before it ended")),
        "{:?}",
        listed[0].notes
    );

    compare(&mut state, &links, "dismiss").await;
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
}

#[tokio::test]
async fn a_comparison_asked_from_another_goes_beside_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);

    let args = "--budget 0.001 write the answer";
    run_comparison(&mut state, &links, &mut commands, args).await;
    let first = subagents::lock(&state.hub.registry).compares()[0].clone();
    state.active = first.agent_id();
    run_comparison(&mut state, &links, &mut commands, args).await;

    let registry = subagents::lock(&state.hub.registry);
    let compares = registry.compares();
    assert_eq!(compares.len(), 2);
    assert_eq!(compares[1].parent, first.parent);
}

#[tokio::test]
async fn an_untitled_agent_that_runs_a_benchmark_is_named_after_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);
    let user = state.active;
    assert!(subagents::lock(&state.hub.registry)
        .find(user)
        .is_some_and(RegisteredAgent::needs_title));

    run_comparison(
        &mut state,
        &links,
        &mut commands,
        "--budget 0.001 write the answer",
    )
    .await;

    let registry = subagents::lock(&state.hub.registry);
    let agent = registry.find(user).unwrap();
    assert_eq!(agent.display_name(), "write the answer");
    assert!(!agent.needs_title());
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
        Route::subtask(SONNET, RouteReason::Default).at_effort("medium"),
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
async fn a_by_difficulty_arm_runs_each_turn_and_subtask_on_its_difficultys_model() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    compare(
        &mut state,
        &links,
        "what does gate do? (hard) --repeats 1 --a sonnet \
         --against difficulty:easy=haiku,medium=sonnet,hard=opus",
    )
    .await;
    let estimate = system_texts(&drain(&mut commands));
    for part in [
        format!("B             by difficulty: easy → {HAIKU} (cloud)"),
        format!("medium →\n                {SONNET} (cloud)"),
        format!("hard → {OPUS}"),
    ] {
        assert!(estimate.contains(&part), "{part:?} in {estimate}");
    }
    compare(&mut state, &links, "go").await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the comparison did not end");
    let payload = latest(&state);

    assert_eq!(
        payload.arm_b,
        ArmSpec::ByDifficulty {
            easy: HAIKU.to_string(),
            medium: SONNET.to_string(),
            hard: OPUS.to_string(),
        }
    );
    let run = payload.run("b-1").unwrap();
    assert_eq!(run.error, None);
    assert_eq!(run.model, OPUS, "hard work runs on the hard model");
    let prompt = entries(&state, run.agent_id)
        .iter()
        .find_map(|e| e.tx.prompt_payload())
        .unwrap();
    assert_eq!(prompt.route.model, OPUS);
    assert_eq!(prompt.route.reason, RouteReason::Compare);
    assert_eq!(prompt.difficulty, Some(Difficulty::Hard));
    assert_eq!(payload.run("a-1").unwrap().model, SONNET);

    // Its subtasks are classified and go to their difficulty's model too.
    let runnable = state.models.runnable_models();
    let routes = aura_gate::compare::DifficultyRoutes::of(&payload.arm_b, &runnable).unwrap();
    let first = fixture.first;
    fixture
        .hub
        .begin_compare_run(first, subagents::ArmRouting::ByDifficulty(routes));
    for (prompt, model) in [("what changed? (easy)", HAIKU), ("what changed?", SONNET)] {
        let request: aura_core_types::SubagentDispatchRequest =
            serde_json::from_value(serde_json::json!({
                "parent_agent_id": first,
                "subagent_type": "reviewer",
                "prompt": prompt,
                "parent_permissions": aura_core_types::AgentPermissions::full_access(),
                "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
            }))
            .unwrap();
        let child = fixture
            .dispatcher
            .dispatch(request)
            .await
            .unwrap()
            .child_agent_id
            .unwrap();
        let route = entries(&state, child)
            .iter()
            .find_map(|e| e.tx.prompt_payload())
            .unwrap()
            .route;
        assert_eq!(
            (route.model.as_str(), route.reason),
            (model, RouteReason::Compare),
            "{prompt}"
        );
    }
    fixture.hub.end_compare_run(first);
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
            "\n  A             gate (likely aura-claude-sonnet-4-6 (cloud))\n  B             aura-claude-sonnet-4-6 (cloud)\n"
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
    for (model, led, outcomes) in [
        (SONNET, false, (3, false)),
        (GPT, true, (10, true)),
        (GPT, false, (3, false)),
    ] {
        state.hub.gate.learn(
            RouteLevel::Turn,
            (PromptCategory::MultiFileChange, Some(Difficulty::Medium)),
            model,
            Some(led),
            outcomes,
        );
    }

    compare(
        &mut state,
        &links,
        "write the answer across files --repeats 1 --against gate:nodelegate",
    )
    .await;
    let estimate = system_texts(&drain(&mut commands));
    assert!(
        estimate.contains("\n  B             gate without enforced delegation (gate:nodelegate)"),
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
    // Sonnet failed at changes across several files itself, so this one
    // starts on the frontier, at gpt-5.4, and is led there, where leading
    // is proven.
    for run in [gate, other] {
        assert_eq!(run.error, None, "{}", run.name);
        assert_eq!(run.model, GPT, "both arms are routed by Gate");
        let prompts: Vec<PromptPayload> = entries(&state, run.agent_id)
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .collect();
        assert_eq!(
            prompts[0].route,
            Route::turn(GPT, RouteReason::Compare).at_effort("medium")
        );
    }
    assert_eq!(gate.files_changed, 0, "the enforced lead may not write");
    assert!(!gate.copy.workdir.join("answer.txt").exists());
    assert_eq!(other.files_changed, 1, "the other lead writes itself");
    assert_eq!(
        std::fs::read_to_string(other.copy.workdir.join("answer.txt")).unwrap(),
        GPT
    );
    let shown = system_texts(&drain(&mut commands));
    assert!(
        shown.contains("A gate vs B gate without enforced delegation"),
        "{shown}"
    );
}

#[tokio::test]
async fn a_gate_run_that_fails_its_checks_carries_on_on_a_stronger_model() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "fix it (easy): run failing check --repeats 1 --against opus",
    )
    .await;

    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    let routes = |name: &str| -> Vec<Route> {
        entries(&state, payload.run(name).unwrap().agent_id)
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .map(|p| p.route)
            .collect()
    };
    let gate = payload.run("a-1").unwrap();
    assert_eq!(gate.error, None);
    assert_eq!(
        routes("a-1"),
        [
            Route::turn(HAIKU, RouteReason::Compare).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ]
    );
    assert_eq!(gate.model, SONNET, "the run ends on the model it moved to");
    assert_eq!(
        (
            gate.turns[0].category,
            gate.turns[0].led,
            gate.turns[0].escalations
        ),
        (Some(PromptCategory::Debugging), Some(false), 1),
        "the run records how its turn was routed"
    );
    assert_eq!(
        gate.turns[0].first_model.as_deref(),
        Some(HAIKU),
        "and the model it started on"
    );
    assert_eq!(
        (
            gate.turns[0].effort.as_deref(),
            gate.turns[0].first_effort.as_deref()
        ),
        (Some("medium"), None),
        "and the effort it ran at, which a move to a stronger model keeps"
    );
    assert_eq!(
        signals_of(&state, gate.agent_id)[0],
        OutcomeSignal::ChecksFailed,
        "the failed attempt is recorded on its own"
    );
    assert_eq!(routes("b-1").len(), 1, "a run on one model stays on it");
}

#[tokio::test]
async fn gate_arms_explore_only_in_learn_mode() {
    for (args, explored) in [("", false), (" --learn", true)] {
        let gate = GateLive::with_rng(
            aura_config::GateConfig {
                explore_rate: 1.0,
                ..aura_config::GateConfig::defaults()
            },
            aura_gate::route::SplitMix64::seeded(3),
        );
        let (fixture, mut commands) = fixture_with_gate(gate);
        let mut b = borrowed(&fixture);
        let links = b.links.clone();
        let mut state = loop_state(&fixture, &mut b);
        state.models = gate_on_zos(&fixture);
        state.sync_turn_base();

        let payload = run_comparison(
            &mut state,
            &links,
            &mut commands,
            &format!("what does gate do? --repeats 1 --against opus{args}"),
        )
        .await;

        assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
        let route = entries(&state, payload.run("a-1").unwrap().agent_id)
            .iter()
            .find_map(|e| e.tx.prompt_payload())
            .unwrap()
            .route;
        assert_eq!(
            (route.reason, route.explored),
            (RouteReason::Compare, explored),
            "{args}"
        );
        assert_eq!(
            payload.notes.iter().any(|n| n.starts_with("Learn mode")),
            explored
        );
    }
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
        estimate.contains(
            "\n  A             aura-claude-sonnet-4-6 (cloud)\n  B             aura-claude-opus-4-7 (cloud)\n"
        ),
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
    let (listed, root) = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchList { benches, root, .. } => Some((benches, root)),
            _ => None,
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(root, first.to_hex());
    assert_eq!(listed[0].root, root);
    assert_eq!(last.root, root, "updates name the tree too");

    state.active = fixture.second;
    bench(&mut state, &links, BenchRequest::List).await;
    let elsewhere = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchList { benches, .. } => Some(benches),
            _ => None,
        })
        .unwrap();
    assert!(elsewhere.is_empty(), "another agent's tree has none");
    state.active = first;

    let id = listed[0].id.clone();
    bench(&mut state, &links, BenchRequest::Dismiss { id }).await;
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
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

    let spec_for = |suite: &str, tasks: usize| BenchSpec {
        work: BenchWork::Examples {
            suite: Some(suite.to_string()),
            tasks,
        },
        a: "sonnet".to_string(),
        b: "opus".to_string(),
        repeats: 1,
        check: None,
        budget_usd: None,
    };
    bench(
        &mut state,
        &links,
        BenchRequest::Prepare(spec_for("gate_hard", 2)),
    )
    .await;
    let shown = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchEstimate { estimate, .. } => Some(estimate),
            _ => None,
        })
        .expect("an estimate")
        .expect("the benchmark is set up");
    assert!(
        shown.contains(&format!("Tasks         {}, {}\n", hard[0], hard[1])),
        "{shown}"
    );

    bench(
        &mut state,
        &links,
        BenchRequest::Prepare(spec_for("nope", 1)),
    )
    .await;
    let problem = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchEstimate { estimate, .. } => Some(estimate),
            _ => None,
        })
        .expect("an answer")
        .expect_err("no such suite");
    assert!(problem.contains("gate_hard, polyglot"), "{problem}");
}

#[tokio::test]
async fn example_tasks_run_in_their_own_fixtures_with_their_own_checks() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let pack = crate::bench_pack::tasks();

    bench(&mut state, &links, BenchRequest::Sessions).await;
    let examples = drain(&mut commands)
        .into_iter()
        .find_map(|(_, c)| match c {
            UiCommand::BenchSessions { examples, .. } => Some(examples),
            _ => None,
        })
        .expect("the example tasks are listed");
    assert_eq!(examples.len(), pack.len());
    assert_eq!(
        (examples[0].suite.as_str(), examples[0].title.as_str()),
        (pack[0].suite(), pack[0].title.as_str())
    );

    let spec = BenchSpec {
        work: BenchWork::Examples {
            suite: None,
            tasks: 2,
        },
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
        estimate.starts_with(&format!(
            "Benchmark: 2 example coding tasks\n  Tasks         {}, {}\n",
            pack[0].id, pack[1].id
        )),
        "{estimate}"
    );
    assert!(estimate.contains("(2 per arm)"), "{estimate}");

    bench(&mut state, &links, BenchRequest::Start).await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the benchmark did not end");
    let payload = latest(&state);

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
    for run in &payload.runs {
        let task = crate::bench_pack::by_id(run.task.as_deref().unwrap()).unwrap();
        assert_eq!(run.error, None, "{}", run.name);
        assert!(
            !run.copy.workdir.starts_with(&fixture.workspace),
            "{} works outside the user's folder",
            run.name
        );
        assert!(run.check_passed.is_some(), "{} was checked", run.name);
        assert!(!run.copy.workdir.join(".gate_checks").exists());
        let prompts: Vec<String> = entries(&state, run.agent_id)
            .iter()
            .filter_map(|e| e.tx.prompt_payload())
            .map(|p| p.text)
            .collect();
        assert_eq!(prompts, task.prompts, "{}", run.name);
        let checks: Vec<serde_json::Value> = entries(&state, run.agent_id)
            .iter()
            .filter_map(check_proposal)
            .map(|p| p.args["shell_script"].clone())
            .collect();
        assert_eq!(checks, [task.verify.as_str()], "{}", run.name);
    }
    let first = crate::bench_pack::by_id(ids[0]).unwrap();
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
    let pairs: Vec<(&str, &str)> = payload
        .judgments
        .iter()
        .map(|j| (j.a_run.as_str(), j.b_run.as_str()))
        .collect();
    assert_eq!(pairs, [("a-1", "b-1"), ("a-2", "b-2")]);

    let shown = drain(&mut commands);
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
    assert_eq!(last.runs[2].task.as_deref(), Some(ids[1]));

    let id = last.id.clone();
    bench(
        &mut state,
        &links,
        BenchRequest::Apply {
            id: id.clone(),
            run: "a-1".to_string(),
        },
    )
    .await;
    let notice = drain(&mut commands)
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
    bench(&mut state, &links, BenchRequest::Dismiss { id }).await;
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
    assert!(
        !payload.runs[0].copy.root.exists(),
        "the copies are removed"
    );
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

/// The check of [`pinned`]'s comparison, which only Opus's answer passes.
const OPUS_CHECK: &str = "sh -c 'grep -q opus answer.txt'";

/// A comparison whose every word and record is pinned: Gate, whose lead
/// does not have to delegate, against Opus on "write the answer", once
/// each and one run at a time, checked by [`OPUS_CHECK`].
struct Pinned {
    payload: ComparePayload,
    /// Everything sent from preparing it on
    shown: Vec<(Option<String>, UiCommand)>,
    /// The comparison's own agent
    host: RegisteredAgent,
}

/// Run [`Pinned`]'s comparison from the first agent's chat.
async fn pinned(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
) -> Pinned {
    state.models = gate_on_zos(fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);
    let args = format!("write the answer --repeats 1 --check \"{OPUS_CHECK}\"");
    compare(state, links, &args).await;
    compare(state, links, "go").await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the comparison did not end");
    let host = subagents::lock(&state.hub.registry).compares()[0].clone();
    Pinned {
        payload: latest(state),
        shown: drain(commands),
        host,
    }
}

/// What `agent_id`'s chat was sent in `shown`, in order: its system
/// lines as they read, its warnings and errors, and where its turns
/// started and ended.
fn chat_of(shown: &[(Option<String>, UiCommand)], agent_id: AgentId) -> Vec<String> {
    let hex = agent_id.to_hex();
    (shown.iter())
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .filter_map(|(_, command)| match command {
            UiCommand::ShowMessage(m) if m.role == MessageRole::System => Some(m.content.clone()),
            UiCommand::ShowWarning(warning) => Some(format!("warning: {warning}")),
            UiCommand::ShowError(error) => Some(format!("error: {error}")),
            UiCommand::TurnStarted { prompt } => {
                Some(format!("turn: {}", prompt.as_deref().unwrap_or_default()))
            }
            UiCommand::Complete => Some("complete".to_string()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_users_chat_hears_a_comparison_byte_for_byte() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let Pinned { payload, shown, .. } = pinned(&fixture, &mut state, &links, &mut commands).await;
    let estimate = [
        "Compare: \"write the answer\"",
        "  A             gate without enforced delegation (gate:nodelegate) (likely",
        "                aura-claude-sonnet-4-6 (cloud))",
        "  B             aura-claude-opus-4-7 (cloud)",
        "  Runs          1 per arm, up to 1 at a time,",
        "                each in its own copy of the workspace",
        "  Check         `sh -c 'grep -q opus answer.txt'` after each run",
        "  Estimate      ~$1.51 at list prices",
        "                A ~$0.54 · B ~$0.90",
        "                judge ~$0.07 on aura-claude-opus-4-7 (cloud)",
        "                for turns of about 150k input and 6k output tokens",
        "  /compare go starts it · /compare cancel drops it",
    ];
    let done = |name: &str, check: &str| {
        let run = payload.run(name).unwrap();
        let (took, cost) = (duration(run.wall_ms), money(run.cost_usd));
        format!("Compare #1: {name} done in {took}, {cost}, check {check}")
    };
    let started = "Compare #1 started: 1 run per arm, up to 1 at a time; they show in the Agents \
                   panel. /compare cancel stops it.";
    assert_eq!(
        chat_of(&shown, state.active),
        [
            estimate.join("\n"),
            started.to_string(),
            done("a-1", "failed"),
            done("b-1", "passed"),
            "Compare #1: judging the runs".to_string(),
            render("Compare #1", &payload),
        ]
    );
}

#[tokio::test]
async fn each_runs_chat_is_pinned_byte_for_byte() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let Pinned { payload, shown, .. } = pinned(&fixture, &mut state, &links, &mut commands).await;
    let check = format!("Check `{OPUS_CHECK}`: ");
    let failed =
        format!("{check}failed with exit code 1. Last lines:\nexit_code: 1\nstdout:\n\nstderr:");
    let runs = [
        (
            "a-1",
            "gate:nodelegate → aura-claude-sonnet-4-6 (cloud)",
            failed,
        ),
        (
            "b-1",
            "aura-claude-opus-4-7 → aura-claude-opus-4-7 (cloud)",
            format!("{check}passed"),
        ),
    ];
    for (name, routed, checked) in runs {
        let run = payload.run(name).unwrap();
        assert_eq!(
            chat_of(&shown, run.agent_id),
            [
                "turn: write the answer".to_string(),
                format!("Compare #1 {name} · {routed} · question"),
                "complete".to_string(),
                checked,
            ],
            "{name}"
        );
    }
}

/// The git tree of an empty folder, which a copy of the tests' empty
/// workspace starts from.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Tokens and cost of `calls` model calls on `model` as the mock
/// provider bills them.
fn mock_tokens(model: &str, calls: u32, cost_usd: f64) -> aura_core_types::ModelTokens {
    aura_core_types::ModelTokens {
        model: model.to_string(),
        calls,
        input_tokens: u64::from(calls) * 100,
        output_tokens: u64::from(calls) * 50,
        cost_usd,
        ..aura_core_types::ModelTokens::default()
    }
}

/// [`Pinned`]'s run `name` on `model`, in its own copy under `dir`, as
/// it should be recorded but for its agent, prompt and times, taken from
/// `ran`.
fn pinned_run(ran: &CompareRun, dir: &std::path::Path, model: &str) -> CompareRun {
    let gate = ran.arm == CompareArm::A;
    let (cost_usd, mock_calls) = if gate { (0.0021, 2) } else { (0.0035, 1) };
    let root = dir.join(&ran.name);
    let turn = RunTurn {
        category: Some(PromptCategory::Question),
        difficulty: Some(Difficulty::Medium),
        led: Some(false),
        effort: gate.then(|| "medium".to_string()),
        ..RunTurn::new(
            model,
            cost_usd,
            ran.turns[0].wall_ms,
            OutcomeSignal::Unavailable,
        )
    };
    CompareRun {
        name: ran.name.clone(),
        model: model.to_string(),
        copy: CompareCopy {
            workdir: root.clone(),
            git_dir: Some(dir.join(format!("{}.git", ran.name))),
            root,
            baseline: EMPTY_TREE.to_string(),
        },
        cost_usd,
        tokens: vec![
            mock_tokens(model, 2, cost_usd),
            mock_tokens("mock-model", mock_calls, 0.0),
        ],
        turns: vec![turn],
        plan_usage: Vec::new(),
        check_passed: Some(!gate),
        files_changed: 1,
        error: None,
        task: None,
        ..ran.clone()
    }
}

#[tokio::test]
async fn what_a_comparison_records_is_pinned() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let Pinned { payload, host, .. } = pinned(&fixture, &mut state, &links, &mut commands).await;
    let folder = host.zns_id.rsplit('/').next().unwrap();
    let dir = fixture._dirs[0].path().join("compare").join(folder);
    let judged = &payload.judgments[0];
    let first = if judged.a_first {
        CompareArm::A
    } else {
        CompareArm::B
    };
    let expected = ComparePayload {
        task: "write the answer".to_string(),
        plan: None,
        arm_a: ArmSpec::Gate { delegate: false },
        arm_b: ArmSpec::Model {
            model: OPUS.to_string(),
        },
        against: OPUS.to_string(),
        prompts: vec!["write the answer".to_string()],
        source: BenchSource::Prompt,
        repeats: 1,
        check: Some(OPUS_CHECK.to_string()),
        budget_usd: None,
        requested_by: state.active,
        apply_root: fixture.workspace.clone(),
        runs: vec![
            pinned_run(&payload.runs[0], &dir, SONNET),
            pinned_run(&payload.runs[1], &dir, OPUS),
        ],
        judgments: vec![aura_core_types::CompareJudgment {
            a_run: "a-1".to_string(),
            b_run: "b-1".to_string(),
            a_first: judged.a_first,
            winner: Some(first),
            parsed: true,
            a_vs_reference: None,
            b_vs_reference: None,
            model: OPUS.to_string(),
            cost_usd: 0.00175,
            reasoning: "A is tidier.\nWINNER: A".to_string(),
        }],
        notes: Vec::new(),
        status: CompareStatus::Done,
        ..payload.clone()
    };
    assert_eq!(payload, expected);
    let saved: Vec<ComparePayload> = (entries(&state, host.agent_id()).iter())
        .filter_map(|entry| entry.tx.compare_payload())
        .collect();
    let running = |runs: usize| ComparePayload {
        runs: payload.runs[..runs].to_vec(),
        judgments: Vec::new(),
        status: CompareStatus::Running,
        ..payload.clone()
    };
    assert_eq!(saved, [running(0), running(1), running(2), payload]);
}

#[tokio::test]
async fn each_run_is_its_own_agent_and_trace() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let Pinned { payload, .. } = pinned(&fixture, &mut state, &links, &mut commands).await;
    // A passed check counts twice in the outcome total: once for the run,
    // and once credited to the step that wrote the answer.
    let runs = [
        ("a-1", "A · gate:nodelegate", "gate:nodelegate", 0.0),
        ("b-1", "B · aura-claude-opus-4-7", OPUS, 2.0),
    ];
    for (name, arm, model, outcome) in runs {
        let run = payload.run(name).unwrap();
        let prompt = entries(&state, run.agent_id)
            .into_iter()
            .find(|entry| entry.tx.prompt_payload().is_some())
            .unwrap();
        assert_eq!(run.prompt_tx, Some(prompt.tx.hash), "{name}");
        let (trace, _) = state.hub.traces.run_log(run.agent_id, name, &[]).unwrap();
        let meta = &trace.run;
        assert_eq!(meta.title, format!("Compare #1 {name}: write the answer"));
        assert_eq!(
            (meta.arm.as_deref(), meta.model.as_str()),
            (Some(arm), model)
        );
        assert_eq!(
            (meta.mode.as_str(), meta.source.as_str()),
            ("agent", "bench")
        );
        assert_eq!(meta.workspace, run.copy.workdir.display().to_string());
        assert_eq!(meta.agent_id, Some(run.agent_id.to_hex()));
        assert_eq!(trace.scorecard.reward_totals.get("outcome"), Some(&outcome));
    }
}

#[tokio::test]
async fn the_bench_tab_follows_a_comparison_run_by_run() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let pinned = pinned(&fixture, &mut state, &links, &mut commands).await;
    let views: Vec<&aura_ui_terminal::events::BenchView> = (pinned.shown.iter())
        .filter_map(|(_, command)| match command {
            UiCommand::BenchUpsert(view) => Some(view.as_ref()),
            _ => None,
        })
        .collect();
    let finished: Vec<Vec<&str>> = (views.iter())
        .filter(|view| view.live.is_empty())
        .map(|view| view.runs.iter().map(|run| run.name.as_str()).collect())
        .collect();
    assert_eq!(
        finished,
        [vec![], vec!["a-1"], vec!["a-1", "b-1"], vec!["a-1", "b-1"]],
        "the start, each run as it ends, and the end"
    );
    let id = pinned.host.agent_id();
    let root = subagents::lock(&state.hub.registry).root(id);
    let last = super::bench::view(id, root, "Compare #1", &pinned.payload);
    assert_eq!(views.last(), Some(&&last));
    for view in &views {
        for live in &view.live {
            let run = pinned.payload.run(&live.name).unwrap();
            assert_eq!(live.arm, run.arm.index());
            assert!(
                live.wall_ms.is_some() == live.agent_id.is_some(),
                "{live:?}"
            );
            assert!(live
                .agent_id
                .as_ref()
                .is_none_or(|id| *id == run.agent_id.to_hex()));
        }
    }
}

/// What `/compare <args>` showed in the first agent's chat: `Ok` for a
/// system line, `Err` for a warning.
async fn answered(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
    args: &str,
) -> Vec<Result<String, String>> {
    compare(state, links, args).await;
    (drain(commands).into_iter())
        .filter_map(|(_, command)| match command {
            UiCommand::ShowMessage(m) if m.role == MessageRole::System => Some(Ok(m.content)),
            UiCommand::ShowWarning(warning) => Some(Err(warning)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn what_compare_answers_after_a_comparison_is_pinned_byte_for_byte() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let Pinned { payload, .. } = pinned(&fixture, &mut state, &links, &mut commands).await;
    let at = crate::record_loader::format_local_time(payload.started_ms);
    let listed = |status: &str| {
        format!(
            "Comparisons (/compare show <n> for one):\n  1. Compare #1 · {at} · \"write the answer\" · \
             gate without enforced delegation (gate:nodelegate) vs aura-claude-opus-4-7 (cloud) · \
             {status} · A $0.0021 median, 0/1 pass / B $0.0035 median, 1/1 pass"
        )
    };
    let removed = "Compare #1's copies were already removed";
    let answers = [
        ("", Ok(listed("done"))),
        ("show", Ok(render("Compare #1", &payload))),
        ("show 2", Err("No such comparison; /compare lists them")),
        ("diff c-9", Err("No run c-9 in Compare #1")),
        (
            "apply c-9",
            Err("No run c-9 in Compare #1; runs are named like a-1"),
        ),
        (
            "dismiss",
            Ok("Compare #1: dismissed; removed the copies".to_string()),
        ),
        ("dismiss", Err(removed)),
        ("apply a-1", Err(removed)),
        (
            "diff",
            Err("Compare #1's copies were removed, and their diffs with them"),
        ),
        ("cancel", Ok("No comparison to cancel".to_string())),
        (
            "go",
            Err("Nothing to start: /compare [prompt] prepares a comparison first"),
        ),
    ];
    for (args, answer) in answers {
        let answer = answer.map_err(str::to_string);
        assert_eq!(
            answered(&mut state, &links, &mut commands, args).await,
            [answer],
            "{args}"
        );
    }
    answered(&mut state, &links, &mut commands, "write the answer").await;
    let waiting = format!(
        "{}\nA comparison is waiting: /compare go starts it.",
        listed("dismissed")
    );
    assert_eq!(
        answered(&mut state, &links, &mut commands, "").await,
        [Ok(waiting)]
    );
    let dropped = answered(&mut state, &links, &mut commands, "cancel").await;
    assert_eq!(dropped, [Ok("Dropped the comparison".to_string())]);
}

#[tokio::test]
async fn compare_diff_shows_the_first_run_of_each_arm_file_by_file() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    pinned(&fixture, &mut state, &links, &mut commands).await;
    compare(&mut state, &links, "diff").await;
    let diffs: Vec<(String, Vec<(String, String)>)> = (drain(&mut commands).into_iter())
        .filter_map(|(_, command)| match command {
            UiCommand::ShowDiff { title, files } => {
                let files = files.into_iter().map(|f| (f.path, f.diff)).collect();
                Some((title, files))
            }
            _ => None,
        })
        .collect();
    let answer = |model: &str| {
        let diff = format!("@@ -0,0 +1 @@\n+{model}\n\\ No newline at end of file\n");
        vec![("answer.txt".to_string(), diff)]
    };
    assert_eq!(
        diffs,
        [
            (format!("Compare #1 a-1 ({SONNET})"), answer(SONNET)),
            (format!("Compare #1 b-1 ({OPUS})"), answer(OPUS)),
        ]
    );
}
