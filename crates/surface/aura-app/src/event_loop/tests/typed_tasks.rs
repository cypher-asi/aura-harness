//! A task the user types into a channel member's chat plans first, as one
//! its coordinator assigns does, and the member's chat says what the
//! software factory does with it, on this computer or in the cloud.

use super::*;
use crate::event_loop::factory::flow::Stage;
use crate::event_loop::factory::hooks::{self, BUILDING_PLAN, PLANNING_FIRST};
use crate::event_loop::turns::{Prompt, Request, Work};
use aura_context_prompts::system::sections::plan_mode::{render, Build};
use aura_exec_tools::plan_tools::CREATE_PLAN_TOOL_NAME;
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use aura_store_db::NewChannel;
use aura_ui::events::MessageRole;

/// A channel `#site` with the fixture's second agent as a member, as a
/// member joins. Returns the coordinator.
async fn channel_with_member(fixture: &Fixture) -> AgentId {
    let channel = channels::create_channel(
        &fixture.hub,
        NewChannel {
            slug: "site".to_string(),
            title: "Website relaunch".to_string(),
            ..NewChannel::default()
        },
    )
    .await
    .unwrap();
    let member = lock_registry_agent(fixture, fixture.second);
    channels::join_parent_channel(&fixture.hub, channel.coordinator, &member).unwrap();
    channel.coordinator
}

/// Finish the turn `agent` runs.
async fn finish(state: &mut LoopState<'_>, done: &mut mpsc::Receiver<WorkDone>, agent: AgentId) {
    let finished = next_done(done).await;
    assert_eq!(finished.agent_id, agent);
    handle_work_done(state, finished).await;
}

/// The commands sent to `agent`'s chat since last asked.
fn sent_to(commands: &mut mpsc::Receiver<UiCommand>, agent: AgentId) -> Vec<UiCommand> {
    let hex = agent.to_hex();
    drain(commands)
        .into_iter()
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .map(|(_, command)| command)
        .collect()
}

/// The lines `sent` shows from the harness.
fn notes(sent: &[UiCommand]) -> Vec<&str> {
    sent.iter()
        .filter_map(|command| match command {
            UiCommand::ShowMessage(message) if message.role == MessageRole::System => {
                Some(message.content.as_str())
            }
            _ => None,
        })
        .collect()
}

/// The modes `sent` shows.
fn modes(sent: &[UiCommand]) -> Vec<PermissionMode> {
    sent.iter()
        .filter_map(|command| match command {
            UiCommand::SetMode(mode) => Some(*mode),
            _ => None,
        })
        .collect()
}

/// The system prompt of `agent`'s latest model call in plan mode.
fn planning_prompt(fixture: &Fixture, agent: AgentId) -> String {
    (fixture.sent_by(agent).into_iter().rev())
        .find(|sent| sent.system.contains("<plan_mode>"))
        .expect("a model call in plan mode")
        .system
}

#[tokio::test]
async fn a_task_typed_into_a_members_chat_plans_first_and_its_chat_says_so() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    channel_with_member(&fixture).await;
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let task = message(member, "Collapse the members' subagents", true);
    handle_ui_event(&mut state, task, &links).await;
    assert_eq!(state.hub.mode_of(member), PermissionMode::Plan);
    finish(&mut state, &mut done, member).await;

    // The model answered without writing a plan, as when it asks a
    // question: the task is still being planned.
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Planning));
    assert_eq!(state.hub.mode_of(member), PermissionMode::Plan);
    let sent = sent_to(&mut commands, member);
    assert!(modes(&sent).contains(&PermissionMode::Plan), "{sent:?}");
    assert_eq!(notes(&sent), [PLANNING_FIRST], "not what /plan says");
    let prompt = planning_prompt(&fixture, member);
    assert!(prompt.contains(&render(Build::RightAway)), "{prompt}");
}

#[tokio::test]
async fn a_member_asked_to_build_or_in_plan_mode_the_user_chose_takes_no_task() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    channel_with_member(&fixture).await;
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(member, "implement", true), &links).await;
    finish(&mut state, &mut done, member).await;
    assert_eq!(state.hub.factory_flows.stage(member), None);
    assert_eq!(state.hub.mode_of(member), PermissionMode::Agent);

    let to_plan = UiEvent::SetMode {
        agent_id: member.to_hex(),
        mode: PermissionMode::Plan,
    };
    handle_ui_event(&mut state, to_plan, &links).await;
    let task = message(member, "Design the login flow", true);
    handle_ui_event(&mut state, task, &links).await;
    finish(&mut state, &mut done, member).await;
    assert_eq!(
        state.hub.factory_flows.stage(member),
        None,
        "it waits for implement"
    );
    let sent = sent_to(&mut commands, member);
    assert!(!notes(&sent).contains(&PLANNING_FIRST), "{sent:?}");
    let prompt = planning_prompt(&fixture, member);
    assert!(prompt.contains(&render(Build::WhenAsked)), "{prompt}");
}

#[tokio::test]
async fn the_coordinator_and_messages_from_its_chat_take_no_task() {
    let (fixture, _commands) = fixture();
    let member = fixture.second;
    let coordinator = channel_with_member(&fixture).await;
    let code = lock_registry_agent(&fixture, member).code.unwrap();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let task = message(coordinator, "Ship the new footer", true);
    handle_ui_event(&mut state, task, &links).await;
    finish(&mut state, &mut done, coordinator).await;
    let mention = format!("@{code} collapse the members' subagents");
    handle_ui_event(&mut state, message(coordinator, &mention, true), &links).await;
    finish(&mut state, &mut done, member).await;

    for agent in [coordinator, member] {
        assert_eq!(state.hub.factory_flows.stage(agent), None);
        assert_eq!(state.hub.mode_of(agent), PermissionMode::Agent);
    }
}

#[tokio::test]
async fn switching_a_member_out_of_plan_mode_while_it_plans_takes_its_task_over() {
    let (fixture, _commands) = fixture();
    let member = fixture.second;
    channel_with_member(&fixture).await;
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let task = message(member, "Collapse the members' subagents", true);
    handle_ui_event(&mut state, task, &links).await;
    finish(&mut state, &mut done, member).await;
    assert_eq!(state.hub.factory_flows.stage(member), Some(Stage::Planning));

    let to_agent = UiEvent::SetMode {
        agent_id: member.to_hex(),
        mode: PermissionMode::Agent,
    };
    handle_ui_event(&mut state, to_agent, &links).await;
    handle_ui_event(&mut state, message(member, "Just do it", true), &links).await;
    assert_eq!(state.hub.mode_of(member), PermissionMode::Agent);
    finish(&mut state, &mut done, member).await;
    assert_eq!(state.hub.factory_flows.stage(member), None);
    assert_eq!(state.hub.mode_of(member), PermissionMode::Agent);
}

/// A conversation whose last turn, started with `prompt`, wrote the plan
/// at `path` with `create_plan`.
fn planned(prompt: &str, path: &str) -> Vec<Message> {
    let call = ContentBlock::ToolUse {
        id: "1".to_string(),
        name: CREATE_PLAN_TOOL_NAME.to_string(),
        input: serde_json::json!({}),
    };
    let result = ContentBlock::ToolResult {
        tool_use_id: "1".to_string(),
        content: ToolResultContent::Text(format!("Plan saved: {path}")),
        is_error: false,
    };
    vec![
        Message::user(prompt),
        Message {
            role: Role::Assistant,
            content: vec![call],
        },
        Message {
            role: Role::User,
            content: vec![result],
        },
    ]
}

#[tokio::test]
async fn a_cloud_members_chat_shows_its_task_planning_then_the_build() {
    let (fixture, mut commands) = fixture();
    let member = fixture.second;
    channel_with_member(&fixture).await;
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);

    // The turn its machine runs starts as `cloud::submit` starts it.
    let text = "Collapse the members' subagents";
    let prompt = Prompt::typed(text.to_string(), Vec::new());
    hooks::plan_first_in_cloud(&state, member, &prompt).await;
    state.hub.factory_flows.note_prompt(member, text);
    assert_eq!(state.hub.mode_of(member), PermissionMode::Plan);
    let sent = sent_to(&mut commands, member);
    assert_eq!(modes(&sent), [PermissionMode::Plan]);
    assert_eq!(notes(&sent), [PLANNING_FIRST]);
    let machine = state.hub.cloud_turn(member).system_prompt.unwrap();
    assert!(machine.contains(&render(Build::RightAway)), "{machine}");

    let plan = ".aura/plans/collapse.plan.md";
    let how = hooks::TurnState {
        ok: true,
        interrupted: false,
    };
    let waits = hooks::after_cloud_turn(&mut state, member, how, &planned(text, plan)).await;
    assert!(waits, "its answer waits for the build");
    assert_eq!(state.hub.mode_of(member), PermissionMode::Agent);
    let sent = sent_to(&mut commands, member);
    assert_eq!(modes(&sent), [PermissionMode::Agent]);
    assert_eq!(notes(&sent), [BUILDING_PLAN]);
    let Some(Work::Prompt { prompt, .. }) = state.queued.pop_front(member) else {
        panic!("the build is not queued");
    };
    assert_eq!(prompt.request, Request::Build);
    assert!(prompt.text.contains(plan), "{}", prompt.text);
}
