//! TUI sign-in: zOS login and logout, linking and unlinking personal
//! plans (Claude, ChatGPT), and `/whoami`.
//!
//! Browser sign-ins wait for the provider's redirect in a background task
//! and report back through [`LinkOutcome`]; paste sign-ins keep the
//! attempt in [`LoopState::pending_link`] until the user enters the code.

use aura_auth::{
    sign_in_notice, AuthError, AuthManager, AuthSummary, LinkMode, LinkStatus, LinkedAccount,
    Owner, ProviderKind,
};
use aura_core_types::{SystemKind, Transaction, TransactionType};
use aura_terminal::UiCommand;
use tokio::sync::mpsc;
use tracing::warn;

use super::handlers::show_system_message;
use super::LoopState;
use crate::model_selection::ModelSelection;

/// A background browser sign-in that finished.
pub(crate) type LinkOutcome = (ProviderKind, Result<LinkedAccount, AuthError>);

/// What the active identity can use. A router JWT supplied through the
/// environment counts as a zOS sign-in.
pub(crate) fn auth_summary(auth: &AuthManager, router_token: Option<&str>) -> AuthSummary {
    let mut summary = auth.summary();
    if summary.zos_user.is_none() && router_token.is_some() {
        summary.zos_user = Some("AURA_ROUTER_JWT".to_string());
    }
    summary
}

async fn send(state: &LoopState<'_>, command: UiCommand) {
    let _ = state.commands.send(command).await;
}

async fn record_auth_change(state: &LoopState<'_>, payload: serde_json::Value) {
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        warn!("Failed to serialize auth change payload");
        return;
    };
    let tx = Transaction::new_chained(
        state.session.agent_id(),
        TransactionType::System,
        bytes,
        None,
    );
    if let Err(e) = state.session.kernel.process_direct(tx).await {
        warn!(error = %e, "Failed to record auth change via kernel");
    }
}

/// Re-gate models after a sign-in change, falling back to the local
/// default when the current model lost its sign-in.
async fn publish_auth(state: &mut LoopState<'_>) {
    let summary = auth_summary(
        &state.auth,
        state.agent_loop.config_mut().auth_token.as_deref(),
    );
    state.models.set_auth(summary);
    if state.models.ensure_usable(state.agent_loop.config_mut()) {
        show_system_message(
            state,
            format!(
                "Switched to local model {}.",
                ModelSelection::label(state.models.current())
            ),
        )
        .await;
    }
    send(state, state.models.ui_command()).await;
}

fn parse_plan(provider: &str) -> Option<ProviderKind> {
    provider
        .parse::<ProviderKind>()
        .ok()
        .filter(|kind| kind.is_subscription())
}

pub(super) async fn handle_login(state: &mut LoopState<'_>, email: &str, password: &str) {
    send(state, UiCommand::SetStatus("Authenticating...".to_string())).await;
    match state.auth.login_zos(email, password).await {
        Ok(session) => {
            state
                .agent_loop
                .set_auth_token(Some(session.access_token.clone()));
            record_auth_change(
                state,
                serde_json::json!({
                    "system_kind": SystemKind::AuthChange,
                    "action": "login",
                    "provider": ProviderKind::Zos.id(),
                    "display_name": session.display_name,
                    "zid": session.primary_zid,
                }),
            )
            .await;
            send(
                state,
                UiCommand::ShowSuccess(format!(
                    "Logged in as {} ({})",
                    session.display_name, session.primary_zid
                )),
            )
            .await;
            let mut note = "Cloud models unlocked — use /model to switch.".to_string();
            if !state.auth.vault().links_for(&Owner::Local).is_empty() {
                note.push_str(
                    " Plans you linked while signed out stay with this machine's local \
                     profile; run `aura auth move --to-zos` to attach them to this account.",
                );
            }
            show_system_message(state, note).await;
            publish_auth(state).await;
        }
        Err(e) => send(state, UiCommand::ShowError(format!("Login failed: {e}"))).await,
    }
    send(state, UiCommand::Complete).await;
}

pub(super) async fn handle_logout(state: &mut LoopState<'_>) {
    match state.auth.logout_zos().await {
        Ok(previous) => {
            state.agent_loop.set_auth_token(None);
            if let Some(session) = previous {
                record_auth_change(
                    state,
                    serde_json::json!({
                        "system_kind": SystemKind::AuthChange,
                        "action": "logout",
                        "provider": ProviderKind::Zos.id(),
                        "display_name": session.display_name,
                        "zid": session.primary_zid,
                    }),
                )
                .await;
            }
            send(state, UiCommand::ShowSuccess("Logged out".to_string())).await;
            publish_auth(state).await;
        }
        Err(e) => {
            send(
                state,
                UiCommand::ShowError(format!("Failed to clear credentials: {e}")),
            )
            .await;
        }
    }
}

pub(super) async fn handle_whoami(state: &LoopState<'_>) {
    let mut lines = Vec::new();
    match state.auth.vault().zos_session() {
        Some(session) => lines.push(format!(
            "Logged in as {} (zID: {}, User: {}, Since: {})",
            session.display_name,
            session.primary_zid,
            session.user_id,
            session.created_at.format("%Y-%m-%d %H:%M UTC"),
        )),
        None => lines.push(
            "Not logged in to zOS — local models only. Use /login to unlock cloud models."
                .to_string(),
        ),
    }
    for kind in state.auth.subscription_kinds() {
        lines.push(match state.auth.link(kind) {
            Some(link) => describe_link(&link),
            None => format!("{}: not linked — /login {}", kind.label(), kind.id()),
        });
    }
    show_system_message(state, lines.join("\n")).await;
}

fn describe_link(link: &LinkedAccount) -> String {
    let account = if link.label.is_empty() {
        String::new()
    } else {
        format!(" as {}", link.label)
    };
    let status = match &link.status {
        LinkStatus::Active if link.plan_enabled => "active".to_string(),
        LinkStatus::Active => "signed in, but the plan is not enabled for this app".to_string(),
        LinkStatus::ReauthRequired { reason } => {
            format!(
                "sign in again with /login {} ({reason})",
                link.provider.id()
            )
        }
    };
    format!("{}: linked{account} — {status}", link.provider.label())
}

pub(super) async fn handle_link(
    state: &mut LoopState<'_>,
    provider: &str,
    paste: bool,
    outcomes: &mpsc::Sender<LinkOutcome>,
) {
    let Some(kind) = parse_plan(provider) else {
        send(
            state,
            UiCommand::ShowWarning(format!("{provider} is not a plan provider")),
        )
        .await;
        send(state, UiCommand::EndLink).await;
        return;
    };
    if let Some(notice) = sign_in_notice(kind) {
        show_system_message(state, notice.to_string()).await;
    }
    let mode = if paste {
        LinkMode::Paste
    } else {
        LinkMode::Browser
    };
    let pending = match state.auth.begin_link(kind, mode).await {
        Ok(pending) => pending,
        Err(e) => {
            send(
                state,
                UiCommand::ShowError(format!("Could not start {} sign-in: {e}", kind.label())),
            )
            .await;
            send(state, UiCommand::EndLink).await;
            return;
        }
    };
    let url = pending.url().to_string();
    let opened = aura_auth::open_browser(&url);
    let how = if opened {
        "Opened in your browser"
    } else {
        "Open this URL"
    };
    if pending.needs_paste() {
        show_system_message(
            state,
            format!(
                "{how} to sign in to {}, then paste the code it shows:\n{url}",
                kind.label()
            ),
        )
        .await;
        state.pending_link = Some(pending);
        send(
            state,
            UiCommand::PromptLinkCode {
                provider: kind.label().to_string(),
            },
        )
        .await;
    } else {
        show_system_message(
            state,
            format!(
                "{how} to sign in to {}:\n{url}\nWaiting for the browser to finish \
                 (on another machine? use /login {} paste).",
                kind.label(),
                kind.id()
            ),
        )
        .await;
        let auth = state.auth.clone();
        let outcomes = outcomes.clone();
        tokio::spawn(async move {
            let result = auth.finish_link(pending, None).await;
            let _ = outcomes.send((kind, result)).await;
        });
    }
}

pub(super) async fn handle_link_code(state: &mut LoopState<'_>, code: &str) {
    let Some(pending) = state.pending_link.take() else {
        send(
            state,
            UiCommand::ShowWarning("No sign-in is waiting for a code".to_string()),
        )
        .await;
        send(state, UiCommand::EndLink).await;
        return;
    };
    let kind = pending.provider();
    let result = state.auth.finish_link(pending, Some(code)).await;
    handle_link_outcome(state, (kind, result)).await;
}

pub(super) async fn handle_link_outcome(state: &mut LoopState<'_>, outcome: LinkOutcome) {
    let (kind, result) = outcome;
    match result {
        Ok(link) => {
            record_auth_change(
                state,
                serde_json::json!({
                    "system_kind": SystemKind::AuthChange,
                    "action": "link",
                    "provider": kind.id(),
                    "account": link.label,
                    "owner": link.owner.to_string(),
                }),
            )
            .await;
            send(
                state,
                UiCommand::ShowSuccess(format!("{} linked", kind.label())),
            )
            .await;
            let mut note = describe_link(&link);
            if link.plan_enabled {
                note.push_str(&format!(
                    "\n{} plan models are now in /model; they bill your plan, not Aura.",
                    kind.label()
                ));
            }
            show_system_message(state, note).await;
            publish_auth(state).await;
        }
        Err(e) => {
            send(
                state,
                UiCommand::ShowError(format!("{} sign-in failed: {e}", kind.label())),
            )
            .await;
        }
    }
    send(state, UiCommand::EndLink).await;
}

pub(super) async fn handle_unlink(state: &mut LoopState<'_>, provider: &str) {
    let Some(kind) = parse_plan(provider) else {
        send(
            state,
            UiCommand::ShowWarning(format!("{provider} is not a plan provider")),
        )
        .await;
        return;
    };
    match state.auth.unlink(kind).await {
        Ok(Some(link)) => {
            record_auth_change(
                state,
                serde_json::json!({
                    "system_kind": SystemKind::AuthChange,
                    "action": "unlink",
                    "provider": kind.id(),
                    "account": link.label,
                    "owner": link.owner.to_string(),
                }),
            )
            .await;
            send(
                state,
                UiCommand::ShowSuccess(format!("{} unlinked", kind.label())),
            )
            .await;
            publish_auth(state).await;
        }
        Ok(None) => {
            send(
                state,
                UiCommand::ShowWarning(format!("No {} plan is linked", kind.label())),
            )
            .await;
        }
        Err(e) => {
            send(
                state,
                UiCommand::ShowError(format!("Failed to unlink {}: {e}", kind.label())),
            )
            .await;
        }
    }
}
