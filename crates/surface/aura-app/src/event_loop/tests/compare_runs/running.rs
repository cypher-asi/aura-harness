//! Both arms run repeatedly in copies of their own: what each run records,
//! traces and shows, and what Gate learns from their checks.

use super::*;
use std::path::PathBuf;

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
    assert_each_arm_lists_its_runs(&state, &payload);
    let (mut workdirs, bench_runs) = assert_runs(&fixture, &state, &payload);
    assert_requesters_log_holds(&state, &bench_runs);
    workdirs.dedup();
    assert_eq!(workdirs.len(), 4);
    assert!(
        !fixture.workspace.join("answer.txt").exists(),
        "the user's folder is untouched"
    );

    assert_eq!(turn_row(&state, OPUS), Some((2, 2)));
    assert_eq!(turn_row(&state, SONNET), Some((2, 0)));
    assert_the_judge_picked_the_first_shown(&payload);
    assert_shown(&state, &mut commands);

    compare(&mut state, &links, "").await;
    let listed = system_texts(&drain(&mut commands));
    assert!(listed.contains("1. Compare #1"), "{listed}");

    compare(&mut state, &links, "dismiss").await;
    for workdir in &workdirs {
        assert!(!workdir.exists(), "{} was removed", workdir.display());
    }
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
}

/// The model `arm` runs on in this file's comparison.
fn arm_model(arm: CompareArm) -> &'static str {
    match arm {
        CompareArm::A => SONNET,
        CompareArm::B => OPUS,
    }
}

fn turn_row(state: &LoopState<'_>, model: &str) -> Option<(u32, u32)> {
    state
        .hub
        .gate
        .stats()
        .row(RouteLevel::Turn, PromptCategory::Question, model)
        .map(|row| (row.tries, row.successes))
}

/// Checks that each arm's runs are listed under an agent of its own, under
/// the comparison.
fn assert_each_arm_lists_its_runs(state: &LoopState<'_>, payload: &ComparePayload) {
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

/// Checks each run of `payload`: its copy, its prompt, its check and its
/// trace. Returns the runs' copies, and their ids in their traces.
fn assert_runs(
    fixture: &Fixture,
    state: &LoopState<'_>,
    payload: &ComparePayload,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut workdirs = Vec::new();
    let mut bench_runs = Vec::new();
    for run in &payload.runs {
        workdirs.push(assert_run_in_its_own_copy(fixture, run));
        assert_run_prompted(state, run);
        assert_run_checked(state, run);
        bench_runs.push(assert_run_traced(state, run));
    }
    (workdirs, bench_runs)
}

/// Checks that `run` wrote in a copy of its own, which holds the user's
/// files but not their build output; returns the copy's folder.
fn assert_run_in_its_own_copy(fixture: &Fixture, run: &CompareRun) -> PathBuf {
    assert_eq!(run.error, None, "{}", run.name);
    let model = arm_model(run.arm);
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
    workdir.clone()
}

/// Checks the one prompt `run` recorded: its text, its route on the arm's
/// model, and that it did not lead.
fn assert_run_prompted(state: &LoopState<'_>, run: &CompareRun) {
    let prompts: Vec<PromptPayload> = entries(state, run.agent_id)
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .collect();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].text, "write the answer");
    let route = Route::turn(arm_model(run.arm), RouteReason::Compare);
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
}

/// Checks that `run`'s check and its result are in its log, before the
/// check's outcome, which follows the turn's.
fn assert_run_checked(state: &LoopState<'_>, run: &CompareRun) {
    let checked = if run.arm == CompareArm::A {
        OutcomeSignal::ChecksFailed
    } else {
        OutcomeSignal::ChecksPassed
    };
    assert_eq!(
        signals_of(state, run.agent_id),
        [OutcomeSignal::Unavailable, checked],
        "the turn's outcome, then the check's"
    );
    let log = entries(state, run.agent_id);
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

/// Checks `run`'s trace: a benchmark run of its arm, of one turn, whose
/// outcome is its check's. Returns the run's id in it.
fn assert_run_traced(state: &LoopState<'_>, run: &CompareRun) -> String {
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
    trace.run.run_id.clone()
}

/// The agent that asked for the latest comparison, which it sits under,
/// and the benchmark runs under that agent, as they started.
fn requester_and_its_runs(state: &LoopState<'_>) -> (AgentId, Vec<AgentId>) {
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
}

/// Checks that the log of the agent that asked holds `bench_runs`, after
/// its own turns.
fn assert_requesters_log_holds(state: &LoopState<'_>, bench_runs: &[String]) {
    let (requester, also) = requester_and_its_runs(state);
    let (trace, path) = state
        .hub
        .traces
        .run_log(requester, "main", &also)
        .expect("the requester's log holds its benchmark's runs");
    assert!(path.is_file(), "{}", path.display());
    for run in bench_runs {
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
}

/// Checks that each pair's verdict was parsed, came from Opus, and went to
/// the attempt the judge was shown first.
fn assert_the_judge_picked_the_first_shown(payload: &ComparePayload) {
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
}

/// Checks what the chat was shown of the comparison and its checks, and
/// that the comparison and its runs are in the Agents panel.
fn assert_shown(state: &LoopState<'_>, commands: &mut mpsc::Receiver<UiCommand>) {
    let shown = system_texts(&drain(commands));
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
}
