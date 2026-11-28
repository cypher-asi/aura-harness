//! The Integrations panel's side in the app: each action runs off the
//! event loop, then the whole list goes back (`UiCommand::SetIntegrations`)
//! with what the action said.

use super::{read, write, Integrations};
use aura_engine_integrations::{AuthKind, IntegrationView as Model, Secret, Status};
use aura_ui::events::{IntegrationAction, IntegrationFieldView, IntegrationState, IntegrationView};
use aura_ui::UiCommand;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

/// Run `action` on integration `id` and send the list back. Does nothing
/// before the integrations are opened.
pub(crate) fn act(commands: mpsc::Sender<UiCommand>, id: String, action: IntegrationAction) {
    let Some(service) = super::get().map(Arc::clone) else {
        return;
    };
    tokio::spawn(async move {
        if let Some(note) = service.run(&id, action).await {
            write(&service.notes).insert(id, note);
        }
        service.refresh().await;
        // A closed UI takes no more views; nothing to do then.
        let _ = commands
            .send(UiCommand::SetIntegrations(service.panel().await))
            .await;
    });
}

impl Integrations {
    /// Run `action` on `id`: what it said, and whether it worked; `None`
    /// for a refresh.
    pub(crate) async fn run(&self, id: &str, action: IntegrationAction) -> Option<(bool, String)> {
        let registry = &self.registry;
        let said = match action {
            IntegrationAction::Refresh => return None,
            IntegrationAction::Connect { secret } => registry
                .connect(id, Secret::new(secret))
                .await
                .map(|report| format!("connected: {}", report.summary)),
            IntegrationAction::Test => registry.test(id).await.map(|report| report.summary),
            IntegrationAction::Disconnect => registry.disconnect(id).await.map(|removed| {
                if removed {
                    "the stored key was removed".to_string()
                } else {
                    "no stored key to remove".to_string()
                }
            }),
            IntegrationAction::Configure { key, value } => registry
                .configure(id, &key, &value)
                .await
                .map(|()| format!("{key} saved")),
        };
        info!(integration = %id, ok = said.is_ok(), "Integration action done");
        Some(match said {
            Ok(text) => (true, text),
            Err(error) => (false, error.to_string()),
        })
    }

    /// Every integration as the panel shows it.
    pub(crate) async fn panel(&self) -> Vec<IntegrationView> {
        let notes = read(&self.notes).clone();
        self.registry
            .views()
            .await
            .into_iter()
            .map(|model| {
                let note = notes.get(model.manifest.id.as_str()).cloned();
                view(&model, note)
            })
            .collect()
    }
}

/// `model` as the panel shows it, its last action's `note` on it; a
/// failed last action shows the row failed.
fn view(model: &Model, note: Option<(bool, String)>) -> IntegrationView {
    let manifest = &model.manifest;
    let (mut state, status) = match &model.status {
        Status::Connected { via } => (IntegrationState::Connected, via.clone()),
        Status::NotConnected { hint } => (IntegrationState::NotConnected, hint.clone()),
        Status::Misconfigured { why } => (IntegrationState::Failed, why.clone()),
    };
    if note.as_ref().is_some_and(|(ok, _)| !ok) {
        state = IntegrationState::Failed;
    }
    let login = match &manifest.auth {
        AuthKind::CliLogin { login, .. } => Some(login.clone()),
        _ => None,
    };
    IntegrationView {
        id: manifest.id.to_string(),
        name: manifest.name.clone(),
        icon: manifest.icon.clone(),
        description: manifest.description.clone(),
        provides: manifest.provides.clone(),
        state,
        status,
        takes_key: manifest.auth.key().map(|key| key.label.clone()),
        login,
        stored: model.stored,
        // An advanced field (an API URL override) shows once it is set
        // to something other than its default.
        config: model
            .config
            .iter()
            .filter(|(field, value)| !field.advanced || *value != field.default)
            .map(|(field, value)| IntegrationFieldView {
                key: field.key.clone(),
                label: field.label.clone(),
                value: value.clone(),
            })
            .collect(),
        tools: manifest
            .tools
            .iter()
            .map(|tool| aura_engine_integrations::tool_name(&manifest.id, &tool.name))
            .collect(),
        access: model.access.describe(),
        note: note.map(|(_, text)| text),
    }
}
