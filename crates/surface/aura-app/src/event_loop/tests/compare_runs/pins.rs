//! One comparison of Gate against Opus whose every word and record is
//! pinned byte for byte: what the user's chat and each run's chat hear,
//! what it records and traces, what the BENCH tab shows, and what
//! `/compare` answers once it ended.

use super::*;
use crate::event_loop::compare::view::{duration, money, render};
use aura_core_types::{BenchSource, CompareCopy, Difficulty, RunTurn};
use aura_ui_terminal::events::MessageRole;

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
        "  Classifier    aura-claude-haiku-4-5 (cloud) · Gate's choice of fast model;",
        "                --classifier <model> picks another",
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

/// What [`Pinned`]'s comparison should record once it ended, asked for by
/// `requested_by` in `workspace` and run in copies under `dir`, but for
/// its times and the order its judge was shown the runs in, taken from
/// `payload`.
fn pinned_payload(
    payload: &ComparePayload,
    dir: &std::path::Path,
    requested_by: AgentId,
    workspace: std::path::PathBuf,
) -> ComparePayload {
    let judged = &payload.judgments[0];
    let first = if judged.a_first {
        CompareArm::A
    } else {
        CompareArm::B
    };
    ComparePayload {
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
        classifier: HAIKU.to_string(),
        requested_by,
        apply_root: workspace,
        runs: vec![
            pinned_run(&payload.runs[0], dir, SONNET),
            pinned_run(&payload.runs[1], dir, OPUS),
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
    let expected = pinned_payload(&payload, &dir, state.active, fixture.workspace.clone());
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

/// What `/compare <args>` answers, in order, once [`Pinned`]'s comparison
/// ended as `payload`, which `/compare` lists as `listed`: the list, its
/// view and runs it does not have, then, once it is dismissed, what its
/// removed copies no longer allow.
fn answers_once_ended(
    payload: &ComparePayload,
    listed: String,
) -> Vec<(&'static str, Result<String, String>)> {
    let removed = "Compare #1's copies were already removed";
    let answers = [
        ("", Ok(listed)),
        ("show", Ok(render("Compare #1", payload))),
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
    (answers.into_iter())
        .map(|(args, answer)| (args, answer.map_err(str::to_string)))
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
    for (args, answer) in answers_once_ended(&payload, listed("done")) {
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
