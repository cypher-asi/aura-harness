//! A new member of a cloud channel starting on a machine of its own: its
//! peers see how far it got ([`super::super::standing`]), and the agents
//! waiting on its work hear when the start is slow or fails.

use super::*;
use crate::event_loop::inbox;
use crate::event_loop::standing::Standing;

/// How long a new member may take to start in the cloud before the
/// agents waiting on its work hear about it.
pub(super) const SLOW_START: Duration = Duration::from_secs(15 * 60);

/// A watch noting on `hub` the boot stage `agent` reached and what it
/// waits on, for its peers.
pub(super) fn boot_watch(hub: &Arc<AgentHub>, agent: AgentId) -> boot::BootWatch {
    let hub = Arc::clone(hub);
    Arc::new(move |event| match event {
        BootEvent::Stage {
            stage,
            state: StageState::Started,
        } => hub.standing.note_boot(agent, Some(stage.label()), None),
        BootEvent::Waiting { what, .. } => {
            hub.standing
                .note_boot(agent, None, Some(what.as_deref().unwrap_or_default()));
        }
        _ => {}
    })
}

/// Whether `agent` is starting in the cloud.
pub(super) fn is_starting(hub: &AgentHub, agent: AgentId) -> bool {
    matches!(
        hub.standing.get(agent),
        Some((Standing::Starting { .. }, _))
    )
}

/// After `after`, tell the agents `agent` owes answers when it is still
/// starting in the cloud, with how far it got. Told once.
pub(super) async fn report_slow_start(hub: Arc<AgentHub>, agent: AgentId, after: Duration) {
    tokio::time::sleep(after).await;
    if !is_starting(&hub, agent) {
        return;
    }
    let minutes = after.as_secs() / 60;
    let at = hub
        .standing
        .view(agent, false)
        .why
        .unwrap_or_else(|| "connecting".to_string());
    let note = format!(
        "it has been starting on a cloud machine for {minutes} minutes and has not begun the work \
         yet (now: {at}). The first start after a new build makes the machine images again, which \
         can take a while; it starts the work as soon as its machine is up."
    );
    warn!(%agent, %at, "A new cloud member is slow to start");
    inbox::tell_waiting(&hub, agent, &note);
}

/// `agent`'s start in the cloud ended, `ok` or not, as `note` says: its
/// peers no longer see it starting, and when it failed, the agents it
/// owes answers hear that it works on this computer instead. Nothing for
/// an agent that wasn't starting, such as one the user moved.
pub(super) fn start_ended(hub: &AgentHub, agent: AgentId, ok: bool, note: &str) {
    if !is_starting(hub, agent) {
        return;
    }
    hub.standing.clear(agent);
    if !ok {
        inbox::tell_waiting(hub, agent, note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::inbox::{assign, AssignArgs};
    use crate::event_loop::notices::Notice;
    use crate::event_loop::subagents::tests::fixture;

    /// `#site`'s coordinator, a new member it assigned a task, and the
    /// notices sent once the task was.
    async fn assigned() -> (
        crate::event_loop::subagents::tests::Fixture,
        AgentId,
        AgentId,
        tokio::sync::mpsc::UnboundedReceiver<Notice>,
    ) {
        let mut fixture = fixture("unused");
        let mut notices = fixture.hub.take_notices();
        let channel = crate::event_loop::channels::create_channel(
            &fixture.hub,
            aura_store_db::NewChannel {
                slug: "site".into(),
                title: "Site".into(),
                ..aura_store_db::NewChannel::default()
            },
        )
        .await
        .unwrap();
        let brief = AssignArgs {
            to: None,
            agent: Some("reviewer".into()),
            subject: "Footer".into(),
            prompt: "Review the footer.".into(),
            todo_id: None,
        };
        assign(&fixture.hub, channel.coordinator, brief)
            .await
            .unwrap();
        fixture.drain_commands();
        let assignment = notices.try_recv().unwrap();
        let starting = Standing::Starting {
            stage: Some("Project snapshot".into()),
            waiting: None,
        };
        fixture.hub.standing.set(assignment.to, starting);
        (fixture, channel.coordinator, assignment.to, notices)
    }

    #[tokio::test]
    async fn a_slow_start_tells_the_assigner_once_and_how_far_it_got() {
        let (fixture, coordinator, member, mut notices) = assigned().await;
        report_slow_start(Arc::clone(&fixture.hub), member, Duration::ZERO).await;
        let told = notices.try_recv().unwrap();
        assert_eq!(told.to, coordinator);
        assert!(
            told.prompt.contains("owes you a report on \"Footer\""),
            "{}",
            told.prompt
        );
        assert!(
            told.prompt.contains("(now: Project snapshot)"),
            "{}",
            told.prompt
        );

        // Once it started, nothing.
        start_ended(&fixture.hub, member, true, "Now running in the cloud.");
        report_slow_start(Arc::clone(&fixture.hub), member, Duration::ZERO).await;
        assert!(notices.try_recv().is_err());
        assert_eq!(fixture.hub.standing.view(member, false).state, "idle");
    }

    #[tokio::test]
    async fn a_failed_start_tells_the_assigner_it_works_here_instead() {
        let (fixture, coordinator, member, mut notices) = assigned().await;
        start_ended(
            &fixture.hub,
            member,
            false,
            "No boxd key — it works on this computer instead.",
        );
        let told = notices.try_recv().unwrap();
        assert_eq!(told.to, coordinator);
        assert!(told
            .prompt
            .ends_with("No boxd key — it works on this computer instead."));
        assert!(!is_starting(&fixture.hub, member));
        assert_eq!(
            fixture.hub.inbox.owed_by(member),
            1,
            "it still owes the work"
        );

        // A move the user asked for tells no one.
        start_ended(&fixture.hub, member, false, "Could not move.");
        assert!(notices.try_recv().is_err());
    }
}
