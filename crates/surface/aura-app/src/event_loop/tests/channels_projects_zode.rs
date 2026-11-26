//! Channel `#zode`: given a project after it was made without one, by an
//! idle or a busy coordinator, and keeping the project it works in from
//! being removed.

use super::channels_projects::{
    errors, placements, record, sent_channel_tasks, sent_sandbox_root, sent_sandbox_root_at, update,
};
use super::support::open_project;
use super::*;

/// Make channel `#zode` without a project through the UI and open it;
/// returns its id and coordinator, the active agent now.
async fn open_zode(
    state: &mut LoopState<'_>,
    commands: &mut mpsc::Receiver<UiCommand>,
    links: &mpsc::Sender<auth::LinkOutcome>,
) -> (String, AgentId) {
    let create = UiEvent::CreateChannel {
        slug: "zode".to_string(),
        title: "Zode".to_string(),
        description: String::new(),
        project_id: None,
    };
    handle_ui_event(state, create, links).await;
    let _ = drain(commands);
    let channel = state
        .hub
        .channels
        .as_ref()
        .unwrap()
        .by_slug("zode")
        .unwrap()
        .unwrap();
    assert_eq!(state.active, channel.coordinator);
    assert_eq!(state.open_channel.as_deref(), Some(channel.id.as_str()));
    (channel.id, channel.coordinator)
}

/// The other direction from `channels_projects`: a channel without a project
/// is refused one, and a rename that keeps no project goes through, which
/// clears to the channel's folder rather than a project's.
#[tokio::test]
async fn a_busy_coordinator_refuses_a_project_for_a_channel_without_one() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let dir = TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());
    let (channel_id, coordinator) = open_zode(&mut state, &mut commands, &links).await;
    let kernel = Arc::clone(&state.idle[&coordinator].kernel);
    state.hub.start_running(
        coordinator,
        Arc::clone(&kernel),
        CancellationToken::new(),
        None,
    );
    let channel = record(&fixture, &channel_id);
    let placed = placements(&fixture, coordinator);

    let change = update(&channel_id, "Zode", "", Some(&project_id));
    handle_ui_event(&mut state, change, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(
        errors(&sent),
        ["#zode's coordinator is busy; change the project when it is idle"]
    );
    assert!(!sent_sandbox_root(&sent), "{sent:?}");
    assert_eq!(record(&fixture, &channel_id), channel);
    assert_eq!(channel.project_id, None);
    assert_eq!(placements(&fixture, coordinator), placed);
    assert!(
        Arc::ptr_eq(&state.idle[&coordinator].kernel, &kernel),
        "the session is not reopened"
    );

    let renamed = update(&channel_id, "Renamed", "", None);
    handle_ui_event(&mut state, renamed, &links).await;
    let sent = drain(&mut commands);
    assert_eq!(errors(&sent), Vec::<String>::new());
    let renamed = record(&fixture, &channel_id);
    assert_eq!(
        (renamed.title.as_str(), renamed.project_id),
        ("Renamed", None)
    );
    state.hub.stop_running(coordinator);
}

#[tokio::test]
async fn a_project_change_reopens_the_coordinator_in_the_new_folder() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let dir = TempDir::new().unwrap();
    let (project_id, project_folder) = open_project(&fixture.hub, dir.path());
    let (channel_id, coordinator) = open_zode(&mut state, &mut commands, &links).await;
    let before = state.idle[&coordinator].kernel.agent_workspace();
    assert!(!before.starts_with(&project_folder), "{}", before.display());

    let change = update(&channel_id, "Zode", "", Some(&project_id));
    handle_ui_event(&mut state, change, &links).await;
    let sent = drain(&mut commands);
    assert!(errors(&sent).is_empty(), "{sent:?}");
    // The next turn runs in the project, not in the session from before.
    let after = state.idle[&coordinator].kernel.agent_workspace();
    assert!(after.starts_with(&project_folder), "{}", after.display());
    assert!(sent_sandbox_root_at(&sent, &after), "{sent:?}");
    // The open channel's tasks are read again from where it works now.
    assert!(sent_channel_tasks(&sent, &channel_id), "{sent:?}");
}

#[tokio::test]
async fn a_project_a_channel_works_in_cannot_be_removed() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let dir = TempDir::new().unwrap();
    let (project_id, _) = open_project(&fixture.hub, dir.path());

    let create = UiEvent::CreateChannel {
        slug: "zode".to_string(),
        title: "Zode".to_string(),
        description: String::new(),
        project_id: Some(project_id.clone()),
    };
    handle_ui_event(&mut state, create, &links).await;
    let _ = drain(&mut commands);

    handle_ui_event(
        &mut state,
        UiEvent::RemoveProject(project_id.clone()),
        &links,
    )
    .await;
    let sent = drain(&mut commands);
    assert!(
        sent.iter().any(|(_, c)| matches!(
            c,
            UiCommand::ShowError(problem)
                if problem.contains("#zode works in this project; edit the channel")
        )),
        "{sent:?}"
    );
    let registry = subagents::lock(&fixture.hub.registry);
    assert!(registry.project(&project_id).is_some_and(|p| !p.removed));
    assert!(registry
        .agents()
        .iter()
        .any(|a| a.project_id.as_deref() == Some(project_id.as_str())));
}
