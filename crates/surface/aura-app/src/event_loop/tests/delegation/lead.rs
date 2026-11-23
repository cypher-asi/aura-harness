//! Which turns are led under Gate, and the lead's tools and limits.

use super::*;
use aura_core_types::Difficulty;

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

/// Gate has seen leading succeed on `model` for changes across files of
/// medium difficulty, as the test classifier rates them, and the model
/// doing that work itself fail.
fn prove_leading(fixture: &Fixture, model: &str) {
    for (led, outcomes) in [(true, (10, true)), (false, (3, false))] {
        fixture.hub.gate.learn(
            RouteLevel::Turn,
            (PromptCategory::MultiFileChange, Some(Difficulty::Medium)),
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

/// The tools of the calls `agent` made that the lead's limits refused, as
/// its record has them.
fn refused_calls(fixture: &Fixture, agent: AgentId) -> Vec<String> {
    fixture
        .hub
        .factory
        .store
        .scan_record(agent, 1, 10_000)
        .unwrap()
        .iter()
        .filter(|e| e.tx.tx_type == aura_core_types::TransactionType::ToolProposal)
        .filter_map(|e| serde_json::from_slice::<aura_core_types::ToolProposal>(&e.tx.payload).ok())
        .filter(|p| p.source.as_deref() == Some("gate_lead"))
        .map(|p| p.tool)
        .collect()
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
    assert_eq!(
        refused_calls(&fixture, first),
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
