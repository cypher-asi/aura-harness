use super::*;

#[test]
fn gate_is_listed_first() {
    let (selection, _) = selection(None);
    let options = selection.options();
    assert_eq!(options[0].id, GATE_MODEL_ID);
    assert_eq!(options[0].label, "Gate");
    assert!(options[0].available);
}

#[test]
fn the_status_line_gets_gates_pick() {
    let claude = signed_in(None, &[ProviderKind::Claude]);
    let (gate, _) = selection_on(claude, GATE_MODEL_ID);
    let aura_ui::UiCommand::SetModels { current, route, .. } = gate.ui_command() else {
        panic!("expected SetModels");
    };
    assert_eq!(current, GATE_MODEL_ID);
    assert_eq!(route.as_deref(), Some("claude-sonnet-4-6 (claude plan)"));

    let (pinned, _) = selection(None);
    let aura_ui::UiCommand::SetModels { route, .. } = pinned.ui_command() else {
        panic!("expected SetModels");
    };
    assert_eq!(route, None);
}

#[test]
fn ready_installed_model_has_no_hint() {
    let (mut selection, _) = selection(None);
    assert_eq!(selection.local_hint(), None);
    selection.local_status = LocalServerStatus::Unreachable;
    assert!(selection
        .local_hint()
        .is_some_and(|hint| hint.contains("ollama pull qwen3:4b")));
}

#[test]
fn cloud_model_has_no_local_hint_when_ollama_is_down() {
    let (mut selection, mut config) = selection(Some("alice"));
    selection
        .select(&mut config, "aura-claude-sonnet-4-6")
        .expect("signed-in cloud model");
    selection.local_status = LocalServerStatus::Unreachable;
    assert_eq!(selection.local_hint(), None);
}

#[test]
fn options_include_unlisted_current_model() {
    let (mut selection, mut config) = selection(None);
    selection
        .select(&mut config, "local/phi4-mini")
        .expect("local models need no login");
    let options = selection.options();
    let entry = options
        .iter()
        .find(|o| o.id == "local/phi4-mini")
        .expect("current model listed");
    assert!(entry.local);
    assert!(!entry.installed);
}
