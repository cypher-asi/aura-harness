//! Delegation under Gate: the lead's tools, a task's check and compact
//! result, read-only tasks side by side and write tasks one at a time,
//! the one escalation of a failed subtask, and `/gate`'s delegation
//! figures.

use super::*;
use aura_core_types::{SubagentDispatchRequest, SubagentExit, SubagentResult};
use aura_model_reasoner::{ContentBlock, ToolResultContent};
use aura_terminal::events::{ProcessKind, ProcessStatus};

const SONNET: &str = "aura-claude-sonnet-4-6";
const GPT: &str = "aura-gpt-5-4";

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
    let (config, tools) = fixture.hub.lead_turn(&mut session);
    assert!(session.lead);
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

#[tokio::test]
async fn the_lead_is_held_to_its_limits_when_it_tries_anyway() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_mode(first, PermissionMode::Unrestricted);
    let big: String = (1..=300).map(|n| format!("line {n}\n")).collect();
    std::fs::write(fixture.workspace.join("big.txt"), big).unwrap();

    turn(&mut state, &mut done, &links, first, "write a note").await;
    turn(&mut state, &mut done, &links, first, "touch a file").await;
    turn(&mut state, &mut done, &links, first, "run ls").await;
    turn(&mut state, &mut done, &links, first, "read the big file").await;

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
    let (config, tools) = off.hub.lead_turn(&mut session);
    assert!(!session.lead);
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
    let (config, tools) = pinned.hub.lead_turn(&mut session);
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
    fixture.hub.set_mode(first, PermissionMode::Unrestricted);

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
    fixture.hub.set_mode(first, PermissionMode::Unrestricted);
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
    fixture.hub.set_mode(first, PermissionMode::Unrestricted);
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
    assert_eq!(prompts[0].category, PromptCategory::Question);
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

#[tokio::test]
async fn nothing_is_retried_without_a_stronger_model() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_mode(first, PermissionMode::Unrestricted);

    let result = dispatch(
        &fixture,
        task(
            first,
            "implement",
            "fix: write a note",
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
        "debugging goes to the strongest model"
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksFailed);
    assert!(!result.final_message.contains("Retried"));
    assert!(
        fixture.workspace.join(format!("notes/{GPT}.txt")).exists(),
        "a final failure is left for the lead to see"
    );
}
