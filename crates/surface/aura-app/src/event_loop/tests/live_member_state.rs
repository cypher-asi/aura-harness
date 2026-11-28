//! A member's row says what it does now, as its peers hear it: a new
//! start replaces an old failure at once, with the step it is at; a new
//! failure shows its own reason; a start that ends well goes to work.

use super::inbox::channel_with_member;
use super::*;
use crate::event_loop::standing::{state_of, WAITING_STATE};
use aura_engine_cloud::boot::{BootEvent, BootStage, StageState};
use aura_ui::events::AgentSummary;

/// `agent`'s row in the channel's MEMBERS, as the lists send it.
fn member_row(hub: &AgentHub, agent: AgentId) -> AgentSummary {
    hub.member_rows().remove(&agent).expect("a member row")
}

/// The state `row` shows, as peers are told one.
fn row_state(row: &AgentSummary) -> &'static str {
    if row.error.is_some() {
        "failed"
    } else if row.starting {
        "starting in the cloud"
    } else if row.waiting {
        WAITING_STATE
    } else if row.reconnecting {
        "working"
    } else {
        "idle or working"
    }
}

/// Check `agent`'s row and what its peers hear agree, and return the row.
fn agreeing(hub: &AgentHub, agent: AgentId) -> AgentSummary {
    let row = member_row(hub, agent);
    let told = state_of(hub, agent);
    match row_state(&row) {
        "idle or working" => assert!(
            matches!(told.state, "idle" | "working" | "queued"),
            "row {row:?} but told {}",
            told.state
        ),
        shown => assert_eq!(shown, told.state, "row {row:?}, told {:?}", told.why),
    }
    if let Some(why) = &row.error {
        assert_eq!(told.why.as_ref(), Some(why), "the same reason");
    }
    row
}

/// The stages a member's start runs, as its start panel shows them.
const PLAN: [BootStage; 9] = [
    BootStage::Connect,
    BootStage::Plans,
    BootStage::Repo,
    BootStage::Snapshot,
    BootStage::Quota,
    BootStage::Create,
    BootStage::Configure,
    BootStage::Claim,
    BootStage::SyncFiles,
];

fn started(stage: BootStage) -> BootEvent {
    BootEvent::Stage {
        stage,
        state: StageState::Started,
    }
}

#[tokio::test]
async fn a_failed_member_starting_again_shows_its_step_not_the_old_failure() {
    let (fixture, _commands) = fixture();
    channel_with_member(&fixture).await;
    let (hub, member) = (&fixture.hub, fixture.second);
    hub.status
        .fail(member, "Could not start on a cloud machine: the old reason");
    assert!(agreeing(hub, member).error.is_some());

    hub.status.begin_start(member);
    let row = agreeing(hub, member);
    assert_eq!((row.error, row.starting), (None, true), "starting at once");
    cloud::show_start_events(
        hub,
        member,
        vec![BootEvent::Plan(PLAN.to_vec()), started(BootStage::Connect)],
    )
    .await;
    cloud::show_start_events(hub, member, vec![started(BootStage::Snapshot)]).await;

    let row = agreeing(hub, member);
    let step = row.start_step.expect("the step");
    assert_eq!(
        (step.label.as_str(), step.index, step.total),
        (BootStage::Snapshot.label(), 4, 9)
    );
    let told = state_of(hub, member).why.unwrap();
    assert!(told.contains(BootStage::Snapshot.label()), "{told}");
}

#[tokio::test]
async fn a_second_failure_shows_its_own_reason() {
    let (fixture, _commands) = fixture();
    channel_with_member(&fixture).await;
    let (hub, member) = (&fixture.hub, fixture.second);
    hub.status
        .fail(member, "Could not start on a cloud machine: the old reason");
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);

    // The fixture has no boxd key: the new start fails on its own.
    assert!(!cloud::start_in_cloud(&mut state, member, cloud::StartCause::Work).await);
    let row = agreeing(&state.hub, member);
    let why = row.error.expect("failed again");
    assert!(!why.contains("the old reason"), "{why}");
    assert!(why.contains("Could not start on a cloud machine"), "{why}");
    assert_eq!(row.start_step, None);
}

#[tokio::test]
async fn a_start_that_ends_well_goes_to_work() {
    let (fixture, _commands) = fixture();
    channel_with_member(&fixture).await;
    let (hub, member) = (&fixture.hub, fixture.second);
    hub.status.fail(member, "Its turn failed: earlier");
    hub.status.begin_start(member);
    cloud::show_start_events(hub, member, vec![started(BootStage::SyncFiles)]).await;
    assert!(agreeing(hub, member).starting);

    // Its machine is up, and its work starts.
    hub.status.set_starting(member, false);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    handle_ui_event(&mut state, message(member, "slow work", true), &links).await;
    let row = agreeing(&state.hub, member);
    assert_eq!(
        (row.error, row.starting, row.start_step),
        (None, false, None)
    );
    assert_eq!(state_of(&state.hub, member).state, "working");
    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(state_of(&state.hub, member).state, "idle");
    agreeing(&state.hub, member);
}
