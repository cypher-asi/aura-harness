//! An agent the saved placements put on a cloud machine, while the cloud
//! hub is down: its work never runs here; it fails in red and says why.

use super::inbox::{assign_footer, channel_with_member, code};
use super::*;
use aura_exec_tools::todo::task_report_line;

/// The placements a hub saved with `agent` on a machine, in `home`.
fn save_placement(home: &std::path::Path, agent: AgentId) {
    let saved = serde_json::json!({
        "agents": {
            agent.to_hex(): {
                "zns": "0://reviewer",
                "name": "reviewer",
                "project": "p1",
                "project_dir": "/tmp/p1",
                "machine": "aura-re01-0101",
            }
        }
    });
    std::fs::write(home.join("placements.json"), saved.to_string()).unwrap();
}

/// The errors `agent`'s chat was shown.
fn errors_shown(commands: &mut mpsc::Receiver<UiCommand>, agent: AgentId) -> Vec<String> {
    let hex = agent.to_hex();
    drain(commands)
        .into_iter()
        .filter(|(to, _)| to.as_deref() == Some(hex.as_str()))
        .filter_map(|(_, command)| match command {
            UiCommand::ShowError(text) => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_cloud_agents_work_while_the_hub_is_down_fails_instead_of_running_here() {
    let (fixture, mut commands) = fixture();
    let coordinator = channel_with_member(&fixture).await;
    let member = fixture.second;
    let home = tempfile::TempDir::new().unwrap();
    save_placement(home.path(), member);
    let mut notices = fixture.hub.take_notices();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.cloud.read_saved_from(home.path());

    // The fixture has no boxd key: the hub can't come up.
    assign_footer(&fixture, coordinator, &code(&fixture, member)).await;
    notices::handle_notice(&mut state, notices.try_recv().unwrap()).await;
    assert!(
        !state.hub.is_working(member),
        "nothing runs on this computer"
    );
    assert_eq!(state.hub.inbox.owed_by(member), 0);
    let report = notices.try_recv().unwrap();
    assert_eq!(report.to, coordinator);
    assert!(
        report.prompt.contains("the cloud can't be reached"),
        "{}",
        report.prompt
    );
    assert!(report.prompt.ends_with(&task_report_line("3", false)));
    let failure = state.hub.status.failure(member).unwrap();
    assert!(failure.contains("runs on a cloud machine"), "{failure}");
    assert!(errors_shown(&mut commands, member)
        .iter()
        .any(|error| error.contains("did not run on this computer")));

    // A message typed into its chat doesn't run here either.
    handle_ui_event(&mut state, message(member, "hello there", true), &links).await;
    assert!(!state.hub.is_working(member));
}

#[tokio::test]
async fn an_agent_the_saved_placements_dont_name_still_works_here() {
    let (fixture, _commands) = fixture();
    let home = tempfile::TempDir::new().unwrap();
    save_placement(home.path(), AgentId::generate());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.cloud.read_saved_from(home.path());

    handle_ui_event(
        &mut state,
        message(fixture.second, "hello there", true),
        &links,
    )
    .await;
    assert!(
        state.hub.is_working(fixture.second),
        "it runs here as before"
    );
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(state.hub.status.failure(fixture.second), None);
}
