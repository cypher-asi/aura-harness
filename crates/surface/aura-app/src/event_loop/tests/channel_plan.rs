//! `/plan` in a channel's chat: a new member of the channel plans, in plan
//! mode, in a chat of its own the session puts on screen, and the
//! coordinator never enters plan mode.

use super::*;
use aura_ui::events::{ChannelSummary, MessageRole};
use aura_ui::host::PlacementView;

/// Channel `#site`, made and opened through the UI, whose agents run at
/// `placement`. Returns its coordinator, which is on screen.
async fn open_site(
    fixture: &Fixture,
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    placement: PlacementView,
) -> AgentId {
    let create = UiEvent::CreateChannel {
        slug: "site".to_string(),
        title: "Website relaunch".to_string(),
        description: String::new(),
        project_id: None,
        placement,
    };
    handle_ui_event(state, create, links).await;
    let channel = (fixture.hub.channels.as_ref().unwrap().by_slug("site"))
        .unwrap()
        .unwrap();
    assert_eq!(state.active, channel.coordinator);
    channel.coordinator
}

fn plan_in(coordinator: AgentId, text: &str) -> UiEvent {
    UiEvent::PlanInChannel {
        agent_id: coordinator.to_hex(),
        text: text.to_string(),
        images: Vec::new(),
    }
}

/// The planner `/plan` started: the agent on screen, a new member of the
/// channel, in plan mode while the coordinator is not.
fn planner_on_screen(fixture: &Fixture, state: &LoopState<'_>, coordinator: AgentId) -> AgentId {
    let planner = state.active;
    assert_ne!(planner, coordinator);
    let member = lock_registry_agent(fixture, planner);
    assert_eq!(
        member.definition.as_deref(),
        Some(channels::PLANNER_DEFINITION)
    );
    assert!(subagents::lock(&state.hub.channel_members).contains(&planner));
    assert_eq!(state.hub.mode_of(planner), PermissionMode::Plan);
    assert_eq!(state.hub.mode_of(coordinator), PermissionMode::Agent);
    planner
}

fn lists_member(channels: &[ChannelSummary], hex: &str) -> bool {
    channels
        .iter()
        .any(|channel| channel.members.iter().any(|member| member.id == hex))
}

#[tokio::test]
async fn plan_in_a_channel_plans_in_a_new_member_whose_chat_comes_on_screen() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = open_site(&fixture, &mut state, &links, PlacementView::Local).await;
    let _ = drain(&mut commands);

    let asked = "Design the login flow";
    handle_ui_event(&mut state, plan_in(coordinator, asked), &links).await;
    let planner = planner_on_screen(&fixture, &state, coordinator);
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, planner);
    handle_work_done(&mut state, finished).await;

    assert_eq!(user_texts(&state, planner), [asked]);
    assert!(
        fixture.sent_by(coordinator).is_empty(),
        "no coordinator turn"
    );
    let sent = drain(&mut commands);
    let hex = planner.to_hex();
    let listed = sent.iter().position(|(_, command)| {
        matches!(command, UiCommand::SetChannels(channels) if lists_member(channels, &hex))
    });
    let shown = sent
        .iter()
        .position(|(_, command)| matches!(command, UiCommand::SetActiveAgent(id) if *id == hex));
    assert!(
        listed.expect("the planner listed") < shown.expect("the planner shown"),
        "the UI keeps the channel open only for its members"
    );
    let prompted = sent.iter().any(|(to, command)| {
        to.as_deref() == Some(hex.as_str())
            && matches!(command, UiCommand::TurnStarted { prompt: Some(text) } if text == asked)
    });
    assert!(prompted, "the planner's chat shows the request");
}

#[tokio::test]
async fn a_bare_plan_leaves_the_planner_waiting_in_plan_mode() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = open_site(&fixture, &mut state, &links, PlacementView::Local).await;
    let _ = drain(&mut commands);

    handle_ui_event(&mut state, plan_in(coordinator, " "), &links).await;
    let planner = planner_on_screen(&fixture, &state, coordinator);

    assert!(!state.hub.is_running(planner));
    assert!(state.idle.contains_key(&planner));
    assert!(user_texts(&state, planner).is_empty());
    assert!(site_posts(&fixture).is_empty(), "no request, no plan post");
    let hex = planner.to_hex();
    let told_plan = drain(&mut commands).iter().any(|(to, command)| {
        to.as_deref() == Some(hex.as_str())
            && matches!(command, UiCommand::SetMode(PermissionMode::Plan))
    });
    assert!(told_plan, "the planner's chat shows plan mode");
}

#[tokio::test]
async fn switching_a_coordinator_to_plan_mode_starts_a_planner_instead() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = open_site(&fixture, &mut state, &links, PlacementView::Local).await;
    let _ = drain(&mut commands);

    let toggle = UiEvent::TogglePlan(coordinator.to_hex());
    handle_ui_event(&mut state, toggle, &links).await;
    let first = planner_on_screen(&fixture, &state, coordinator);
    let set_plan = UiEvent::SetMode {
        agent_id: coordinator.to_hex(),
        mode: PermissionMode::Plan,
    };
    handle_ui_event(&mut state, set_plan, &links).await;
    let second = planner_on_screen(&fixture, &state, coordinator);

    assert_ne!(first, second);
    assert!(!state.hub.is_running(first) && !state.hub.is_running(second));
}

#[tokio::test]
async fn a_planner_in_a_cloud_channel_plans_here_and_says_why() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = open_site(&fixture, &mut state, &links, PlacementView::Cloud).await;
    let _ = drain(&mut commands);

    let asked = plan_in(coordinator, "Design the login flow");
    handle_ui_event(&mut state, asked, &links).await;
    let planner = planner_on_screen(&fixture, &state, coordinator);
    assert!(subagents::lock(&state.hub.cloud_starts).is_empty());
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, planner);
    handle_work_done(&mut state, finished).await;

    assert!(!state.cloud.is_cloud(planner) && !state.cloud.is_moving(planner));
    let hex = planner.to_hex();
    let notes: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .filter_map(|(_, command)| match command {
            UiCommand::ShowMessage(message) if message.role == MessageRole::System => {
                Some(message.content)
            }
            _ => None,
        })
        .collect();
    assert!(
        notes.iter().any(|note| note == channels::PLANS_HERE),
        "{notes:?}"
    );
}

/// Every post of `#site`.
fn site_posts(fixture: &Fixture) -> Vec<aura_store_db::ChannelPost> {
    let store = fixture.hub.channels.as_ref().unwrap();
    let channel = store.by_slug("site").unwrap().unwrap();
    store.posts(&channel.id, None).unwrap()
}

#[tokio::test]
async fn a_plan_request_is_kept_for_history_but_left_out_of_the_digest() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let coordinator = open_site(&fixture, &mut state, &links, PlacementView::Local).await;
    let _ = drain(&mut commands);

    let asked = "Design the login flow";
    handle_ui_event(&mut state, plan_in(coordinator, asked), &links).await;
    let planner = planner_on_screen(&fixture, &state, coordinator);

    let posts = site_posts(&fixture);
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert_eq!(posts[0].author, aura_store_db::PostAuthor::User);
    assert_eq!(posts[0].text, asked);
    assert_eq!(posts[0].planner, Some(planner));
    assert_eq!(posts[0].mentions, [planner]);
    let hex = planner.to_hex();
    let sent = drain(&mut commands).into_iter().any(|(to, command)| {
        to == Some(coordinator.to_hex())
            && matches!(command, UiCommand::ChannelPost { post, .. }
                if post.planner.as_deref() == Some(hex.as_str()) && post.text == asked)
    });
    assert!(sent, "the coordinator's UI gets it, naming its planner");
    assert_eq!(
        channels::take_digest(&state.hub, coordinator).await,
        None,
        "the coordinator's turns never read it"
    );
}
