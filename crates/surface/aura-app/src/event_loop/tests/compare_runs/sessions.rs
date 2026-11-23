//! A session replayed prompt by prompt from the BENCH tab: listed, set up,
//! run from the files as they were when it started, judged against what
//! the user kept, and shown in the tree of the agent that ran it.

use super::*;
use aura_core_types::{BenchSource, RefVerdict};
use std::path::PathBuf;

#[tokio::test]
async fn two_models_replay_a_session_from_its_start_prompt_by_prompt() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let notes = run_a_session(&fixture, &mut state).await;
    assert_the_session_is_listed(&mut state, &links, &mut commands).await;

    prepare_the_replay(&mut state, &links, &mut commands).await;
    bench(&mut state, &links, BenchRequest::Start).await;
    tokio::time::timeout(Duration::from_secs(60), state.compare.finished())
        .await
        .expect("the benchmark did not end");
    let payload = latest(&state);

    assert_the_replay(&payload);
    for run in &payload.runs {
        assert_replayed_run(&state, run);
    }
    assert_eq!(
        std::fs::read_to_string(&notes).unwrap(),
        "edited by the session\n",
        "the user's folder is untouched"
    );
    assert_the_judge_rated_against_the_reference(&payload);
    let updated_root = assert_the_bench_tab_followed(&mut commands);

    bench(&mut state, &links, BenchRequest::List).await;
    let (listed, root) = bench_list(&mut commands).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(root, first.to_hex());
    assert_eq!(listed[0].root, root);
    assert_eq!(updated_root, root, "updates name the tree too");

    state.active = fixture.second;
    bench(&mut state, &links, BenchRequest::List).await;
    let (elsewhere, _) = bench_list(&mut commands).unwrap();
    assert!(elsewhere.is_empty(), "another agent's tree has none");
    state.active = first;

    let id = listed[0].id.clone();
    bench(&mut state, &links, BenchRequest::Dismiss { id }).await;
    assert_eq!(latest(&state).status, CompareStatus::Dismissed);
}

/// Runs a session of two prompts in the first agent's chat, the first of
/// which edits `notes.txt`; returns that file's path.
async fn run_a_session(fixture: &Fixture, state: &mut LoopState<'_>) -> PathBuf {
    let notes = fixture.workspace.join("notes.txt");
    std::fs::write(&notes, "original\n").unwrap();
    checkpointed_turn(
        fixture,
        state,
        fixture.first,
        "write the answer",
        &[("notes.txt", Some("edited by the session\n"))],
    )
    .await;
    checkpointed_turn(fixture, state, fixture.first, "run failing check", &[]).await;
    notes
}

/// Checks that the BENCH tab lists the session as the current one, titled
/// after its first prompt.
async fn assert_the_session_is_listed(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
) {
    bench(state, links, BenchRequest::Sessions).await;
    let (sessions, _) = bench_sessions(commands).expect("the sessions are listed");
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].current);
    assert_eq!(sessions[0].prompts, 2);
    assert_eq!(sessions[0].title, "write the answer");
}

/// Sets up the replay of the current session on Sonnet against Opus from
/// the BENCH tab, and checks its estimate.
async fn prepare_the_replay(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
) {
    let spec = BenchSpec {
        work: BenchWork::CurrentSession,
        a: "sonnet".to_string(),
        b: "opus".to_string(),
        repeats: 1,
        check: None,
        budget_usd: None,
        classifier: Some("sonnet".to_string()),
    };
    bench(state, links, BenchRequest::Prepare(spec)).await;
    let estimate = bench_estimate(commands)
        .expect("an estimate")
        .expect("the benchmark is set up");
    assert!(
        estimate
            .starts_with("Benchmark: replay the session \"write the answer\", 2 prompts in order"),
        "{estimate}"
    );
    assert!(
        estimate.contains(
            "\n  A             aura-claude-sonnet-4-6 (cloud)\n  B             aura-claude-opus-4-7 (cloud)\n  Classifier    aura-claude-sonnet-4-6 (cloud)\n"
        ),
        "{estimate}"
    );
    assert!(estimate.contains("not undone"), "{estimate}");
}

/// Checks the replay's record: its arms and classifier, the session's
/// prompts in order, what the user kept of the session, and its runs.
fn assert_the_replay(payload: &ComparePayload) {
    assert_eq!(payload.status, CompareStatus::Done);
    assert_eq!(payload.classifier, SONNET, "the classifier the form named");
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
}

/// Checks that `run` started from the files as they were when the session
/// started, and replayed its prompts in order on its arm's model.
fn assert_replayed_run(state: &LoopState<'_>, run: &CompareRun) {
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
    let prompts: Vec<String> = entries(state, run.agent_id)
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .map(|p| p.text)
        .collect();
    assert_eq!(prompts, ["write the answer", "run failing check"]);
    let classifiers: Vec<String> = entries(state, run.agent_id)
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .map(|p| p.classifier)
        .collect();
    assert_eq!(classifiers, [SONNET, SONNET], "{} classified", run.name);
}

/// Checks that the judge preferred the attempt it was shown first, rating
/// it as good as what the user kept and the other worse.
fn assert_the_judge_rated_against_the_reference(payload: &ComparePayload) {
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
}

/// Checks what the BENCH tab and the chat were shown of the replay, once
/// `commands` are drained; returns the root of the tree the BENCH tab's
/// last update names.
fn assert_the_bench_tab_followed(commands: &mut mpsc::Receiver<UiCommand>) -> String {
    let shown = drain(commands);
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
    assert_eq!(last.classifier, SONNET);
    assert_eq!(last.arms[0].turns[1].failed, 1);
    assert!(last.has_reference);
    let text = system_texts(&shown);
    assert!(text.contains("Per prompt"), "{text}");
    assert!(text.contains("vs yours"), "{text}");
    last.root.clone()
}
