//! Handling of [`UiEvent`]s: [`domain_of`] routes each event to the
//! `handle` of its domain's submodule.

mod account;
mod chat;
mod permissions;
mod places;
mod queue;
mod roster;
mod session;
mod views;

use super::approvals::Answer;
use super::{auth, policies, LoopState};
use aura_ui::{UiCommand, UiEvent};
use tokio::sync::mpsc;
use tracing::debug;

/// The handler a [`UiEvent`] goes to.
#[derive(Clone, Copy)]
enum Domain {
    Quit,
    Chat,
    Queue,
    Permissions,
    Views,
    Session,
    Roster,
    Places,
    Account,
}

/// Returns `true` if the loop should break (quit).
pub(super) async fn handle_ui_event(
    state: &mut LoopState<'_>,
    event: UiEvent,
    link_outcomes: &mpsc::Sender<auth::LinkOutcome>,
) -> bool {
    match domain_of(&event) {
        Domain::Quit => {
            debug!("Quit received");
            return true;
        }
        Domain::Chat => chat::handle(state, event).await,
        Domain::Queue => queue::handle(state, event).await,
        Domain::Permissions => permissions::handle(state, event).await,
        Domain::Views => views::handle(state, event).await,
        Domain::Session => session::handle(state, event).await,
        Domain::Roster => roster::handle(state, event).await,
        Domain::Places => places::handle(state, event).await,
        Domain::Account => account::handle(state, event, link_outcomes).await,
    }
    false
}

/// The domain whose handler takes `event`. The match has no wildcard, so
/// a new event doesn't compile until it is routed here.
fn domain_of(event: &UiEvent) -> Domain {
    use UiEvent as E;
    match event {
        E::Quit => Domain::Quit,
        E::UserMessage { .. } | E::RunCommand { .. } | E::Cancel(_) => Domain::Chat,
        E::StopCommand(_) | E::Compact { .. } | E::BuildPlan { .. } => Domain::Chat,
        E::SendQueuedNow(_) | E::RemoveQueued { .. } | E::EditQueued { .. } => Domain::Queue,
        E::MoveQueued { .. } | E::SendQueuedItemNow { .. } => Domain::Queue,
        E::Approve(_) | E::ApproveAlways(_) | E::Deny(_) => Domain::Permissions,
        E::SetMode { .. } | E::SetApproval { .. } | E::CycleApproval(_) => Domain::Permissions,
        E::TogglePlan(_) | E::RequestPolicies(_) => Domain::Permissions,
        E::ShowStatus | E::ShowGate | E::OpenGraph(_) | E::OpenFlow(_) => Domain::Views,
        E::ListRuns | E::LabelStep { .. } | E::ShowHelp | E::ShowHistory(_) => Domain::Views,
        E::Clear | E::NewSession | E::SelectModel(_) | E::SetThinking(_) => Domain::Session,
        E::Resume { .. } | E::InitProject { .. } | E::Rewind { .. } => Domain::Session,
        E::Diff { .. } | E::Compare { .. } | E::Bench { .. } => Domain::Session,
        E::Log { .. } | E::KillProcess(_) | E::Repo { .. } => Domain::Session,
        E::SelectAgent(_) | E::FocusAgent(_) | E::CreateAgent(_) => Domain::Roster,
        E::CreateProject { .. } | E::AddAgentToProject(_) | E::ReorderProjects(_) => Domain::Roster,
        E::ReorderAgents { .. } | E::ArchiveAgent(_) | E::RestoreAgent(_) => Domain::Roster,
        E::RemoveProject(_) | E::RefreshAgents | E::InstantiateDefinition(_) => Domain::Roster,
        E::AddDefinitionToProject(_) | E::EditDefinition(_) => Domain::Roster,
        E::DeleteDefinition(_) | E::DefinitionEdited(_) => Domain::Roster,
        E::SetPlacement { .. } | E::HostAction { .. } | E::CloudSetup => Domain::Places,
        E::CreateChannel { .. } | E::OpenChannel(_) | E::CloseChannel => Domain::Places,
        E::UpdateChannel { .. } | E::ArchiveChannel(_) | E::PlanInChannel { .. } => Domain::Places,
        E::ListSandbox(_) | E::ReadSandboxFile(_) => Domain::Places,
        E::LoginCredentials { .. } | E::Logout | E::Whoami => Domain::Account,
        E::LinkSubscription { .. } | E::LinkCode(_) | E::Unlink(_) => Domain::Account,
    }
}

/// Hand the user's answer to the tool call waiting as `request_id`. An
/// "always allow" adds a project rule, so the calling agent's POLICIES
/// tab is sent again.
pub(super) async fn answer_approval(state: &LoopState<'_>, request_id: &str, answer: Answer) {
    debug!(request = %request_id, ?answer, "Approval answered");
    match state.approvals.answer(request_id, answer).await {
        Ok(agent_id) if answer == Answer::Always => {
            policies::send_policies_of(state, agent_id).await;
        }
        Ok(_) => {}
        Err(e) => {
            let _ = state
                .commands
                .send(UiCommand::ShowWarning(e.to_string()))
                .await;
        }
    }
}
