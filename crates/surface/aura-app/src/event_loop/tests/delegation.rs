//! Delegation under Gate: which turns are led, the lead's tools, a
//! task's check and compact result, read-only tasks side by side and
//! write tasks one at a time, the tier a frontier lead's subtasks stay
//! under, the one escalation of a failed subtask, and `/gate`'s
//! delegation figures.

use super::*;
use aura_core_types::{SubagentDispatchRequest, SubagentExit, SubagentResult};
use aura_model::{ContentBlock, ToolResultContent};
use aura_ui_terminal::events::{ProcessKind, ProcessStatus};
use std::path::{Path, PathBuf};

const SONNET: &str = "aura-claude-sonnet-4-6";
const GPT: &str = "aura-gpt-5-4";
const HAIKU: &str = "aura-claude-haiku-4-5";

fn task(parent: AgentId, kind: &str, prompt: &str, check: Option<&str>) -> SubagentDispatchRequest {
    serde_json::from_value(serde_json::json!({
        "parent_agent_id": parent,
        "subagent_type": kind,
        "prompt": prompt,
        "parent_permissions": aura_core_types::AgentPermissions::full_access(),
        "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
        "check": check,
    }))
    .unwrap()
}

async fn dispatch(fixture: &Fixture, request: SubagentDispatchRequest) -> SubagentResult {
    fixture.dispatcher.dispatch(request).await.unwrap()
}

fn select_gate(fixture: &Fixture, state: &mut LoopState<'_>) {
    state.models = gate_on_zos(fixture);
    state.sync_turn_base();
}

fn names(tools: &[ToolDefinition]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_str()).collect()
}

/// The text of every tool result in `agent_id`'s idle session.
fn tool_results(state: &LoopState<'_>, agent_id: AgentId) -> Vec<(String, bool)> {
    state.idle[&agent_id]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((
                match content {
                    ToolResultContent::Text(text) => text.clone(),
                    ToolResultContent::Json(value) => value.to_string(),
                },
                *is_error,
            )),
            _ => None,
        })
        .collect()
}

/// The prompts and outcomes in `agent_id`'s record.
fn record(fixture: &Fixture, agent_id: AgentId) -> (Vec<PromptPayload>, Vec<OutcomePayload>) {
    let entries = fixture
        .hub
        .factory
        .store
        .scan_record(agent_id, 1, 10_000)
        .unwrap();
    let prompts = entries.iter().filter_map(|e| e.tx.prompt_payload());
    let outcomes = entries.iter().filter_map(|e| e.tx.outcome_payload());
    (prompts.collect(), outcomes.collect())
}

/// Children of `parent` that ran `kind`, oldest first.
fn children(fixture: &Fixture, parent: AgentId, kind: &str) -> Vec<AgentId> {
    let registry = subagents::lock(&fixture.hub.registry);
    registry
        .descendants(parent)
        .into_iter()
        .filter(|agent| {
            registry
                .find(*agent)
                .is_some_and(|a| a.definition.as_deref() == Some(kind))
        })
        .collect()
}

#[tokio::test]
async fn under_gate_the_lead_has_no_file_tools_and_a_limited_run_command() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);

    let mut session = fixture
        .hub
        .factory
        .open(&lock_registry_agent(&fixture, first), Vec::new(), None)
        .unwrap();
    session.lead = true;
    let (mut config, mut tools, lead) = fixture.hub.user_turn(&mut session);
    assert!(!session.lead, "a turn starts without the lead's limits");
    let lead = lead.expect("the user's agent leads under Gate");
    assert!(lead.apply(true, &mut config, &mut tools));
    let names = names(&tools);
    for tool in ["write_file", "edit_file", "delete_file"] {
        assert!(!names.contains(&tool), "{tool} in {names:?}");
    }
    for tool in [
        "read_file",
        "search_code",
        "run_command",
        "task",
        "todo_write",
    ] {
        assert!(names.contains(&tool), "{tool} missing from {names:?}");
    }
    assert!(config.system_prompt.contains("<gate_lead>"));
    assert!(config
        .system_prompt
        .contains("you do not edit files yourself"));
    let task = tools.iter().find(|t| t.name == "task").unwrap();
    assert!(task.input_schema["properties"]["check"].is_object());
    assert!(task.input_schema["properties"]["subagent_type"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .any(|kind| kind == "implement"));

    let (instance, _) = fixture.hub.turn(Some("implement"), &fixture.workspace);
    assert!(
        !instance.system_prompt.contains("<gate_lead>"),
        "subagents are never leads"
    );
}

/// Gate has seen leading succeed on `model` for changes across files,
/// and the model doing that work itself fail.
fn prove_leading(fixture: &Fixture, model: &str) {
    for (led, outcomes) in [(true, (10, true)), (false, (3, false))] {
        fixture.hub.gate.learn(
            RouteLevel::Turn,
            PromptCategory::MultiFileChange,
            model,
            Some(led),
            outcomes,
        );
    }
}

#[tokio::test]
async fn with_nothing_learned_no_turn_is_led_and_proven_leading_is() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    let mut session = fixture
        .hub
        .factory
        .open(&lock_registry_agent(&fixture, first), Vec::new(), None)
        .unwrap();

    for category in PromptCategory::CLASSES {
        let (mut config, mut tools, lead) = fixture.hub.user_turn(&mut session);
        let before = (config.system_prompt.clone(), tools.len());
        let led = fixture
            .hub
            .gate
            .way((category, None), &config.model, false)
            .led;
        assert!(
            !lead.unwrap().apply(led, &mut config, &mut tools),
            "{category}"
        );
        assert_eq!((config.system_prompt, tools.len()), before, "{category}");
        assert!(names(&tools).contains(&"write_file"), "{category}");
    }
    let (mut config, mut tools, lead) = fixture.hub.user_turn(&mut session);
    prove_leading(&fixture, &config.model);
    let category = PromptCategory::MultiFileChange;
    let led = fixture
        .hub
        .gate
        .way((category, None), &config.model, false)
        .led;
    assert!(lead.unwrap().apply(led, &mut config, &mut tools));
    assert!(!names(&tools).contains(&"write_file"));
}

#[tokio::test]
async fn a_small_turn_under_gate_does_its_own_work() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());

    turn(&mut state, &mut done, &links, first, "write a note").await;

    let results = tool_results(&state, first);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(!results[0].1, "nothing refused the write: {}", results[0].0);
    assert!(fixture
        .workspace
        .join(format!("notes/{SONNET}.txt"))
        .exists());
    assert!(!state.idle[&first].lead);
    assert!(
        children(&fixture, first, "implement").is_empty(),
        "nothing was delegated"
    );
}

#[tokio::test]
async fn the_lead_is_held_to_its_limits_when_it_tries_anyway() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    for model in [HAIKU, GPT, SONNET] {
        prove_leading(&fixture, model);
    }
    let big: String = (1..=300).map(|n| format!("line {n}\n")).collect();
    std::fs::write(fixture.workspace.join("big.txt"), big).unwrap();

    for prompt in [
        "write a note",
        "touch a file",
        "run ls",
        "read the big file",
    ] {
        let across_files = format!("across files: {prompt}");
        turn(&mut state, &mut done, &links, first, &across_files).await;
    }
    assert!(state.idle[&first].lead);

    let results = tool_results(&state, first);
    assert_eq!(results.len(), 4, "{results:?}");
    assert!(results[0].1 && results[0].0.contains("implement subtask"));
    assert!(
        !fixture.workspace.join("notes").exists(),
        "the lead wrote nothing"
    );
    assert!(results[1].1 && results[1].0.contains("Delegate `touch touched.txt`"));
    assert!(!fixture.workspace.join("touched.txt").exists());
    assert!(!results[2].1, "inspection commands run: {}", results[2].0);
    assert!(results[3].0.contains("line 200\n… [lines capped at 200"));
    assert!(!results[3].0.contains("line 201"));

    let refused: Vec<String> = fixture
        .hub
        .factory
        .store
        .scan_record(first, 1, 10_000)
        .unwrap()
        .iter()
        .filter(|e| e.tx.tx_type == aura_core_types::TransactionType::ToolProposal)
        .filter_map(|e| serde_json::from_slice::<aura_core_types::ToolProposal>(&e.tx.payload).ok())
        .filter(|p| p.source.as_deref() == Some("gate_lead"))
        .map(|p| p.tool)
        .collect();
    assert_eq!(
        refused,
        ["write_file", "run_command"],
        "refused calls are recorded"
    );
}

#[tokio::test]
async fn with_delegation_off_or_a_pinned_model_the_lead_keeps_its_tools() {
    let (off, _commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&off);
    let mut state = loop_state(&off, &mut b);
    select_gate(&off, &mut state);
    let mut session = off
        .hub
        .factory
        .open(&lock_registry_agent(&off, off.first), Vec::new(), None)
        .unwrap();
    let (mut config, mut tools, lead) = off.hub.user_turn(&mut session);
    let lead = lead.expect("it still leads, without the limits");
    assert!(!lead.apply(true, &mut config, &mut tools));
    assert!(names(&tools).contains(&"write_file"));
    assert!(config.system_prompt.contains("Delegating is recommended"));

    let (pinned, _commands) = fixture();
    let mut b = borrowed(&pinned);
    let mut state = loop_state(&pinned, &mut b);
    state.sync_turn_base();
    let mut session = pinned
        .hub
        .factory
        .open(
            &lock_registry_agent(&pinned, pinned.first),
            Vec::new(),
            None,
        )
        .unwrap();
    let (config, tools, lead) = pinned.hub.user_turn(&mut session);
    assert_eq!(lead, None);
    assert!(!session.lead);
    assert!(names(&tools).contains(&"write_file"));
    assert!(
        !config.system_prompt.contains("<gate_lead>"),
        "a pinned model gets no lead guidance"
    );
}

#[tokio::test]
async fn a_check_decides_the_subtask_and_comes_back_in_a_compact_result() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();
    fixture.hub.set_permissions(first, yolo());

    let passed = dispatch(
        &fixture,
        task(first, "implement", "write a note", Some("ls notes")),
    )
    .await;
    assert!(matches!(passed.exit, SubagentExit::Completed));
    assert_eq!(passed.files_changed, ["notes/mock-model.txt"]);
    assert_eq!(
        passed.final_message,
        "Done.\n\nFiles changed: notes/mock-model.txt\n\nCheck `ls notes`: passed"
    );
    let child = passed.child_agent_id.unwrap();
    let (_, outcomes) = record(&fixture, child);
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksPassed);

    let failed = dispatch(
        &fixture,
        task(
            first,
            "implement",
            "run passing check",
            Some("ls no-such-dir"),
        ),
    )
    .await;
    let child = failed.child_agent_id.unwrap();
    let (_, outcomes) = record(&fixture, child);
    assert_eq!(
        outcomes[0].signal,
        OutcomeSignal::ChecksFailed,
        "the task's check overrides the subagent's own passing check"
    );
    assert!(
        failed
            .final_message
            .contains("Check `ls no-such-dir`: failed with exit code"),
        "{}",
        failed.final_message
    );
    assert!(
        !failed.final_message.contains("Retried"),
        "a pinned subtask is never escalated"
    );
    assert_eq!(children(&fixture, first, "implement").len(), 2);
    let shown = system_texts(&drain(&mut commands));
    assert!(shown.contains("Check `ls notes`: passed"), "{shown}");
}

#[tokio::test]
async fn a_check_runs_without_the_task_records_that_quote_the_prompts() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();
    fixture.hub.set_permissions(first, yolo());

    let done = dispatch(
        &fixture,
        task(
            first,
            "implement",
            "rename calc_total",
            Some("test ! -e .aura/tasks"),
        ),
    )
    .await;
    let (_, outcomes) = record(&fixture, done.child_agent_id.unwrap());
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksPassed);
    let kept = super::super::tasks::load_all(&fixture.workspace);
    assert_eq!(kept.len(), 1, "the records are back after the check");
    assert_eq!(kept[0].view.title, "rename calc_total");
}

/// Wait until `n` task rows of the Processes tab are running.
async fn running_tasks(fixture: &Fixture, n: usize) {
    let running = || {
        fixture
            .hub
            .processes
            .snapshot()
            .iter()
            .filter(|row| row.kind == ProcessKind::Task && row.status == ProcessStatus::Running)
            .count()
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while running() != n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} tasks running, not {n}", running()));
}

fn count_running(fixture: &Fixture) -> usize {
    fixture
        .hub
        .processes
        .snapshot()
        .iter()
        .filter(|row| row.kind == ProcessKind::Task && row.status == ProcessStatus::Running)
        .count()
}

#[tokio::test]
async fn read_only_tasks_run_side_by_side_and_write_tasks_one_at_a_time() {
    let (fixture, _commands) = fixture();
    let first = fixture.first;
    fixture.hub.set_permissions(first, yolo());
    let both = |kind: &'static str| {
        let fixture = &fixture;
        async move {
            tokio::join!(
                dispatch(fixture, task(first, kind, "slow look at a", None)),
                dispatch(fixture, task(first, kind, "slow look at b", None)),
            )
        }
    };

    let reads = both("explorer");
    let watch = async {
        running_tasks(&fixture, 2).await;
        fixture.gate.add_permits(2);
    };
    let ((a, b), ()) = tokio::join!(reads, watch);
    assert!(matches!(a.exit, SubagentExit::Completed));
    assert!(matches!(b.exit, SubagentExit::Completed));

    let writes = both("implement");
    let watch = async {
        running_tasks(&fixture, 1).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(count_running(&fixture), 1, "the second edit waits its turn");
        fixture.gate.add_permits(1);
        running_tasks(&fixture, 1).await;
        fixture.gate.add_permits(1);
    };
    let ((a, b), ()) = tokio::join!(writes, watch);
    assert!(matches!(a.exit, SubagentExit::Completed));
    assert!(matches!(b.exit, SubagentExit::Completed));
}

#[tokio::test]
async fn a_failed_check_undoes_the_attempt_and_retries_once_on_a_stronger_model() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    let lead = state.idle.get(&first).unwrap();
    let mut turn_prompt = PromptPayload::new("make the notes");
    turn_prompt.route = Route::turn(SONNET, RouteReason::Default);
    turns::record_prompt(lead, &turn_prompt).await.unwrap();

    let check = format!("ls notes/{GPT}.txt");
    let result = dispatch(
        &fixture,
        task(first, "implement", "write a note", Some(&check)),
    )
    .await;

    assert!(matches!(result.exit, SubagentExit::Completed));
    let attempts = children(&fixture, first, "implement");
    assert_eq!(attempts.len(), 2, "one retry");
    let notes: Vec<String> = std::fs::read_dir(fixture.workspace.join("notes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        notes,
        [format!("{GPT}.txt")],
        "the first attempt's note was removed before the retry"
    );

    let (prompts, outcomes) = record(&fixture, attempts[0]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(SONNET, RouteReason::Default)
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksFailed);
    let (prompts, outcomes) = record(&fixture, attempts[1]);
    assert_eq!(prompts[0].route, Route::subtask(GPT, RouteReason::Escalate));
    assert_eq!(prompts[0].category, Some(PromptCategory::Question));
    assert!(prompts[0]
        .text
        .starts_with("write a note\n\n---\nA first attempt"));
    assert!(
        prompts[0]
            .text
            .contains(&format!("Check `{check}`: failed")),
        "the retry sees the failure: {}",
        prompts[0].text
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksPassed);
    assert_eq!(outcomes[0].model, GPT);

    let stats = fixture.hub.gate.stats();
    let row = |model| {
        stats
            .row(RouteLevel::Subtask, PromptCategory::Question, model)
            .map(|r| (r.tries, r.successes))
    };
    assert_eq!(row(SONNET), Some((1, 0)), "the first attempt counts");
    assert_eq!(row(GPT), None, "the retry does not");
    assert!(
        result.final_message.contains(&format!(
            "Retried once: the first attempt on {SONNET} failed"
        )),
        "{}",
        result.final_message
    );
    assert!(result
        .final_message
        .contains("Check `ls notes/aura-gpt-5-4.txt`: passed"));
    assert_eq!(result.child_agent_id, Some(attempts[1]));
    assert!(route_lines(&mut commands)
        .iter()
        .any(|line| line.ends_with("subtask, question, escalate")));

    let delegation = gate_view::delegation_of(&fixture.hub, first);
    assert_eq!(delegation.turns.len(), 1);
    let overall = delegation.overall;
    assert_eq!((overall.subtasks, overall.escalations), (1, 1));
    assert!(overall.subtask_calls >= 4, "{overall:?}");
    assert_eq!(
        overall.cost_share(),
        Some(1.0),
        "the lead itself made no calls"
    );
    assert_eq!(overall.escalation_rate(), Some(1.0));
    drain(&mut commands);
    handle_ui_event(&mut state, UiEvent::ShowGate, &links).await;
    let text = system_texts(&drain(&mut commands));
    assert!(
        text.contains(
            "Delegation in this agent's 1 turn: 1.0 subtasks per turn, escalation rate 100% (1 of 1)"
        ),
        "{text}"
    );
}

/// Dispatch a subtask asking `prompt` whose check fails on its first
/// attempt (on Sonnet), and return its attempts, oldest first.
async fn failing_first_attempt(fixture: &Fixture, prompt: &str) -> (SubagentResult, Vec<AgentId>) {
    let first = fixture.first;
    let check = format!("ls notes/{GPT}.txt");
    let result = dispatch(fixture, task(first, "implement", prompt, Some(&check))).await;
    (result, children(fixture, first, "implement"))
}

#[tokio::test]
async fn a_failed_subtask_the_judge_excuses_is_not_retried_or_counted() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());

    let (result, attempts) = failing_first_attempt(&fixture, "write a note (judge: excuse)").await;

    assert!(matches!(result.exit, SubagentExit::Completed));
    assert_eq!(attempts.len(), 1, "no retry on a stronger model");
    let (_, outcomes) = record(&fixture, attempts[0]);
    let signals: Vec<OutcomeSignal> = outcomes.iter().map(|o| o.signal).collect();
    assert_eq!(
        signals,
        [OutcomeSignal::ChecksFailed, OutcomeSignal::JudgedExcused]
    );
    assert_eq!(
        fixture
            .hub
            .gate
            .stats()
            .row(RouteLevel::Subtask, PromptCategory::Question, SONNET),
        None,
        "an excused failure counts for nothing"
    );
}

#[tokio::test]
async fn a_failed_subtask_the_judge_blames_is_retried_with_what_the_judge_found() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());

    let (_, attempts) = failing_first_attempt(&fixture, "write a note (judge: blame)").await;

    assert_eq!(attempts.len(), 2, "one retry");
    let (_, outcomes) = record(&fixture, attempts[0]);
    let signals: Vec<OutcomeSignal> = outcomes.iter().map(|o| o.signal).collect();
    assert_eq!(
        signals,
        [OutcomeSignal::ChecksFailed, OutcomeSignal::JudgedFail]
    );
    let (prompts, _) = record(&fixture, attempts[1]);
    assert_eq!(prompts[0].route, Route::subtask(GPT, RouteReason::Escalate));
    assert!(
        prompts[0]
            .text
            .contains("A reviewer of that attempt found: The change is wrong."),
        "{}",
        prompts[0].text
    );
    let row = fixture
        .hub
        .gate
        .stats()
        .row(RouteLevel::Subtask, PromptCategory::Question, SONNET)
        .map(|r| (r.tries, r.successes));
    assert_eq!(row, Some((1, 0)), "the failure counts once");
}

#[tokio::test]
async fn a_frontier_leads_subtasks_start_below_its_tier_and_may_escalate_to_it() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    // Sonnet has failed at planning, so on its own this subtask would run
    // on the strongest model, the lead's.
    sonnet_failed_at_planning(&fixture);
    fixture.hub.gate.note_model(first, GPT);

    let check = format!("ls notes/{GPT}.txt");
    let result = dispatch(
        &fixture,
        task(first, "implement", "plan: write a note", Some(&check)),
    )
    .await;

    let attempts = children(&fixture, first, "implement");
    assert_eq!(attempts.len(), 2, "{}", result.final_message);
    let (prompts, outcomes) = record(&fixture, attempts[0]);
    assert_eq!(prompts[0].category, Some(PromptCategory::Planning));
    assert_eq!(
        prompts[0].route,
        Route::subtask(SONNET, RouteReason::Default),
        "the strongest model below the lead's tier"
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksFailed);
    let (prompts, _) = record(&fixture, attempts[1]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(GPT, RouteReason::Escalate),
        "the retry may go up to the lead's tier"
    );
    assert!(result
        .final_message
        .contains(&format!("Check `{check}`: passed")));

    fixture.hub.stop_running(first);
    assert_eq!(
        fixture.hub.gate.model_of(first),
        None,
        "forgotten once the lead's work ends"
    );
    dispatch(
        &fixture,
        task(first, "implement", "plan: write a note", None),
    )
    .await;
    let attempts = children(&fixture, first, "implement");
    let (prompts, _) = record(&fixture, attempts[2]);
    assert_eq!(prompts[0].route, Route::subtask(GPT, RouteReason::Default));
}

/// Gate has seen two planning subtasks fail on Sonnet, which rules it
/// out of planning subtasks.
fn sonnet_failed_at_planning(fixture: &Fixture) {
    fixture.hub.gate.learn(
        RouteLevel::Subtask,
        PromptCategory::Planning,
        SONNET,
        None,
        (2, false),
    );
}

/// Run git with `args` in `dir` and return its output.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Seed `workspace` with the files "change files by shell" works on and
/// an ignored `target/out`.
fn seed_shell_files(workspace: &Path) {
    std::fs::write(workspace.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(workspace.join("tracked.txt"), "base\n").unwrap();
    std::fs::write(workspace.join("gone.txt"), "keep\n").unwrap();
    std::fs::write(workspace.join("wip.txt"), "wip\n").unwrap();
    std::fs::create_dir(workspace.join("target")).unwrap();
    std::fs::write(workspace.join("target/out"), "build\n").unwrap();
}

#[tokio::test]
async fn in_a_git_workspace_a_failed_attempts_shell_changes_are_undone_before_the_retry() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    let ws = fixture.workspace.clone();
    seed_shell_files(&ws);
    git(&ws, &["init", "--quiet", "-b", "main"]);
    git(&ws, &["config", "user.email", "test@example.com"]);
    git(&ws, &["config", "user.name", "Test"]);
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "--quiet", "-m", "seed"]);
    std::fs::write(ws.join("wip.txt"), "stashed\n").unwrap();
    git(&ws, &["stash", "--quiet"]);
    std::fs::write(ws.join("wip.txt"), "wip, not committed\n").unwrap();
    let repository = || {
        (
            git(&ws, &["rev-parse", "HEAD"]),
            git(&ws, &["for-each-ref"]),
            git(&ws, &["stash", "list"]),
            std::fs::read(ws.join(".git/index")).unwrap(),
        )
    };
    let before = repository();

    let check = format!("ls shell-{GPT}.txt");
    let result = dispatch(
        &fixture,
        task(first, "implement", "change files by shell", Some(&check)),
    )
    .await;

    assert!(
        matches!(result.exit, SubagentExit::Completed),
        "{}",
        result.final_message
    );
    assert_eq!(children(&fixture, first, "implement").len(), 2, "one retry");
    assert!(result
        .final_message
        .contains(&format!("Check `{check}`: passed")));
    assert!(
        !ws.join(format!("shell-{SONNET}.txt")).exists(),
        "the first attempt's new file was removed"
    );
    assert!(ws.join(format!("shell-{GPT}.txt")).exists());
    assert_eq!(
        read(ws.join("tracked.txt")),
        format!("base\n{GPT}\n"),
        "the first attempt's edit was undone"
    );
    assert_eq!(read(ws.join("gone.txt")), "keep\n", "and its delete");
    assert_eq!(read(ws.join("wip.txt")), "wip, not committed\n");
    assert_eq!(
        read(ws.join("target/out")),
        format!("build\n{SONNET}\n{GPT}\n"),
        "ignored files are left alone"
    );
    assert_eq!(
        repository(),
        before,
        "index, HEAD, refs and stash are untouched"
    );
    assert!(before.2.contains("stash@{0}"));
}

#[tokio::test]
async fn outside_git_only_file_tool_changes_are_undone_before_the_retry() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    let ws = fixture.workspace.clone();
    seed_shell_files(&ws);

    let check = format!("ls shell-{GPT}.txt");
    let result = dispatch(
        &fixture,
        task(first, "implement", "change files by shell", Some(&check)),
    )
    .await;

    assert!(result
        .final_message
        .contains(&format!("Check `{check}`: passed")));
    assert!(
        ws.join(format!("shell-{SONNET}.txt")).exists(),
        "a shell command's changes stay outside git"
    );
    assert_eq!(
        read(ws.join("tracked.txt")),
        format!("base\n{SONNET}\n{GPT}\n")
    );
    assert!(!ws.join("gone.txt").exists());
}

#[tokio::test]
async fn nothing_is_retried_without_a_stronger_model() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    sonnet_failed_at_planning(&fixture);

    let result = dispatch(
        &fixture,
        task(
            first,
            "implement",
            "plan: write a note",
            Some("ls no-such-dir"),
        ),
    )
    .await;

    let attempts = children(&fixture, first, "implement");
    assert_eq!(attempts.len(), 1);
    let (prompts, outcomes) = record(&fixture, attempts[0]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(GPT, RouteReason::Default),
        "with Sonnet ruled out, planning goes to the strongest model"
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksFailed);
    assert!(!result.final_message.contains("Retried"));
    assert!(
        fixture.workspace.join(format!("notes/{GPT}.txt")).exists(),
        "a final failure is left for the lead to see"
    );
}
