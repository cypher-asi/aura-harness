use super::*;

// ========================================================================
// Session modes and plans
// ========================================================================

fn app_with_events() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(8);
    (App::new().with_event_sender(tx), rx)
}

fn sample_plan() -> crate::events::PlanView {
    use crate::events::{PlanView, TodoState, TodoView};
    PlanView {
        path: ".aura/plans/auth_0a1b2c3d.plan.md".to_string(),
        name: "Auth".to_string(),
        todos: vec![TodoView {
            content: "Add login".to_string(),
            state: TodoState::Pending,
            assignee: None,
            task: None,
        }],
        changed_ms: None,
    }
}

#[test]
fn test_mode_commands_request_a_switch() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));

    app.handle_command("/plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(agent_id)) if agent_id == "aa"));
    assert_eq!(app.hud().mode, None, "badge waits for the kernel");

    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    assert_eq!(app.hud().mode, Some(crate::PermissionMode::Plan));
    app.handle_command("/plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(agent_id)) if agent_id == "aa"));
    assert!(app.notification().is_none(), "plan mode toggles off");

    app.handle_command("/mode ask");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode {
            mode: crate::PermissionMode::ReadOnly,
            ..
        })
    ));

    app.handle_command("/mode nonsense");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((_, NotificationType::Warning))
    ));
}

#[test]
fn test_switching_to_the_current_mode_sends_nothing() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Agent));
    app.handle_command("/mode agent");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((text, NotificationType::Warning)) if text.contains("Already in agent mode")
    ));
}

#[test]
fn test_plan_works_mid_turn_but_build_waits() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetPlan(Some(sample_plan())));
    app.chat_state = AppState::Processing;

    app.handle_command("/build");
    assert!(rx.try_recv().is_err());
    app.handle_command("/plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(_))));
}

#[test]
fn test_plan_with_a_message_sends_it_in_plan_mode() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetPlan(Some(sample_plan())));

    // Even a build phrase after `/plan` is a request for a plan.
    submit(&mut app, "/plan implement auth");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode { agent_id, mode: crate::PermissionMode::Plan }) if agent_id == "aa"
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { agent_id, text, shown: true, .. })
            if agent_id == "aa" && text == "implement auth"
    ));
    assert_eq!(app.state(), AppState::Processing);
    assert_eq!(app.messages().back().unwrap().content(), "implement auth");

    // Already in plan mode, it stays there rather than toggling out.
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    app.chat_state = AppState::Idle;
    submit(&mut app, "/PLAN  add\nlogout ");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode {
            mode: crate::PermissionMode::Plan,
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "add\nlogout"
    ));
    assert!(app.notification().is_none());
}

#[test]
fn test_plan_message_parsing() {
    use super::super::slash::{plan_ranges, strip_plan};
    let ranges = |text: &str| -> Vec<(usize, usize)> {
        plan_ranges(text).iter().map(|r| (r.start, r.end)).collect()
    };
    assert_eq!(ranges("/plan"), [(0, 5)]);
    assert_eq!(ranges("/Plan do it"), [(0, 5)]);
    assert_eq!(ranges("fix it /plan"), [(7, 12)]);
    assert_eq!(ranges("a /plan b\n/PLAN"), [(2, 7), (10, 15)]);
    for plain in [
        "/planner",
        "/pla",
        "src/plan",
        "x /plan/y",
        "/plé",
        "/build a /plan",
    ] {
        assert!(plan_ranges(plain).is_empty(), "{plain}");
    }
    assert_eq!(strip_plan("/plan").as_deref(), Some(""));
    assert_eq!(strip_plan("  /plan   ").as_deref(), Some(""));
    assert_eq!(strip_plan("/plan fix it ").as_deref(), Some("fix it"));
    assert_eq!(strip_plan("fix /plan it").as_deref(), Some("fix it"));
    assert_eq!(strip_plan("fix it /plan").as_deref(), Some("fix it"));
    assert_eq!(strip_plan("fix it"), None);
    assert_eq!(strip_plan("/planfix"), None);
}

#[test]
fn test_plan_anywhere_in_a_message_sends_it_in_plan_mode() {
    let (mut app, mut rx) = app_with_events();
    submit(&mut app, "add a logout button /plan");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetMode {
            mode: crate::PermissionMode::Plan,
            ..
        })
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "add a logout button"
    ));
}

#[test]
fn test_bare_plan_still_toggles() {
    let (mut app, mut rx) = app_with_events();
    submit(&mut app, "/plan ");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(_))));
    assert!(rx.try_recv().is_err());
    submit(&mut app, " /plan");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::TogglePlan(_))));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_build_needs_a_plan() {
    let (mut app, mut rx) = app_with_events();
    app.handle_command("/build");
    assert!(rx.try_recv().is_err());
    assert!(matches!(
        app.notification(),
        Some((text, NotificationType::Warning)) if text.contains("No plan")
    ));

    app.handle_command("/build .aura/plans/x_00000000.plan.md");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::BuildPlan { path: Some(path), .. }) if path == ".aura/plans/x_00000000.plan.md"
    ));

    app.process_command(UiCommand::SetPlan(Some(sample_plan())));
    assert_eq!(app.active_plan().map(|p| p.name.as_str()), Some("Auth"));
    app.handle_command("/build");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::BuildPlan { path: None, .. })
    ));
}

#[test]
fn saying_implement_in_plan_mode_builds_the_plan() {
    let (mut app, mut rx) = app_with_events();
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));

    // No plan yet: the agent gets the message.
    submit(&mut app, "implement");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UserMessage { text, .. }) if text == "implement"
    ));
    app.chat_state = AppState::Idle;

    app.process_command(UiCommand::SetPlan(Some(sample_plan())));
    submit(&mut app, "  Implement it! ");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::BuildPlan { path: None, .. })
    ));

    // Longer messages are feedback on the plan.
    submit(&mut app, "implement it with a trait instead");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::UserMessage { .. })));
    app.chat_state = AppState::Idle;

    // Outside plan mode "implement" is an ordinary message.
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Agent));
    submit(&mut app, "implement");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::UserMessage { .. })));
}

#[test]
fn a_plan_for_an_agent_off_screen_leaves_the_active_plan_alone() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("a1".to_string()));
    app.process_command(UiCommand::SetPlan(Some(sample_plan())));

    app.process_command(for_agent("a2", UiCommand::SetPlan(None)));
    assert_eq!(app.active_plan().map(|p| p.name.as_str()), Some("Auth"));

    app.process_command(for_agent("a1", UiCommand::SetPlan(None)));
    assert!(app.active_plan().is_none());
}

/// Whether the Agents panel marks agent `id` as in plan mode.
fn listed_in_plan_mode(app: &App, id: &str) -> bool {
    app.agents()
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("{id} is not listed"))
        .plan_mode
}

#[test]
fn set_mode_plan_marks_an_off_screen_agent_in_plan_mode() {
    let (mut app, _rx) = app_with_agents();
    assert!(!listed_in_plan_mode(&app, "aa") && !listed_in_plan_mode(&app, "bb"));

    app.process_command(for_agent(
        "bb",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(listed_in_plan_mode(&app, "bb"));
    assert!(!listed_in_plan_mode(&app, "aa"), "the active agent is not");
    assert_eq!(app.hud().mode, None, "the chat on screen keeps its mode");

    app.process_command(for_agent(
        "bb",
        UiCommand::SetMode(crate::PermissionMode::Agent),
    ));
    assert!(!listed_in_plan_mode(&app, "bb"));
}

#[test]
fn set_mode_plan_marks_the_on_screen_agent_in_plan_mode() {
    let (mut app, _rx) = app_with_agents();

    app.process_command(for_agent(
        "aa",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(listed_in_plan_mode(&app, "aa"));
    assert!(!listed_in_plan_mode(&app, "bb"));
    assert_eq!(app.hud().mode, Some(crate::PermissionMode::Plan));

    app.process_command(for_agent(
        "aa",
        UiCommand::SetMode(crate::PermissionMode::Agent),
    ));
    assert!(!listed_in_plan_mode(&app, "aa"));
    assert_eq!(app.hud().mode, Some(crate::PermissionMode::Agent));
}

#[test]
fn any_mode_but_plan_clears_the_plan_mode_mark() {
    use crate::PermissionMode;
    let (mut app, _rx) = app_with_agents();
    for mode in [PermissionMode::ReadOnly, PermissionMode::Debug] {
        app.process_command(for_agent("bb", UiCommand::SetMode(PermissionMode::Plan)));
        assert!(listed_in_plan_mode(&app, "bb"));
        app.process_command(for_agent("bb", UiCommand::SetMode(mode)));
        assert!(!listed_in_plan_mode(&app, "bb"), "{mode:?}");
    }
}

#[test]
fn set_mode_plan_for_an_unlisted_agent_marks_no_one_in_plan_mode() {
    let (mut app, _rx) = app_with_agents();
    app.process_command(for_agent(
        "zz",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(app.agents().iter().all(|a| !a.plan_mode));
}

#[test]
fn a_new_agent_list_replaces_the_plan_mode_mark() {
    let (mut app, _rx) = app_with_agents();
    app.process_command(for_agent(
        "bb",
        UiCommand::SetMode(crate::PermissionMode::Plan),
    ));
    assert!(listed_in_plan_mode(&app, "bb"));

    app.process_command(UiCommand::SetAgents(vec![
        agent("aa", "Terminal Agent"),
        agent("bb", "Researcher"),
    ]));
    assert!(!listed_in_plan_mode(&app, "bb"), "the list says it is not");

    app.process_command(UiCommand::SetAgents(vec![
        crate::events::AgentSummary {
            plan_mode: true,
            ..agent("aa", "Terminal Agent")
        },
        agent("bb", "Researcher"),
    ]));
    assert!(listed_in_plan_mode(&app, "aa"), "the list says it is");
    assert!(!listed_in_plan_mode(&app, "bb"));
}
