//! Pre-implementation task description refinement.
//!
//! Both the dev-loop and single-task automatons converge on this
//! helper after they have loaded the [`TaskDescriptor`] and the
//! [`SpecDescriptor`] but before they call
//! `AgentRunner::execute_task_tracked`. The helper asks the configured
//! [`ModelProvider`] for a description that more precisely matches the
//! spec's intent, persists it via [`DomainApi::update_task`], and
//! returns the updated descriptor so the caller can continue with the
//! refined text in `AgenticTaskParams`.
//!
//! Behaviour contract (locked in by the unit tests in this module):
//!
//! 1. If the incoming `task.description` already starts with
//!    [`REFINED_MARKER`] we return immediately with `task.clone()` —
//!    no provider call, no `update_task`, no events. This is the
//!    single authoritative idempotency guard — Phase 7 dropped the
//!    redundant dev-loop `build_retry_note.is_none()` outer gate
//!    because the marker is already persisted on the description
//!    after pass one, so the build-retry second pass and any
//!    ambient re-claim both short-circuit here uniformly.
//! 2. We emit [`AutomatonEvent::TaskDescriptionRefining`] on a
//!    best-effort basis (`event_tx` may be `None`, send failures are
//!    swallowed; observability is nice-to-have, never blocking).
//! 3. On provider/`update_task` failure we emit a `LogLine` and fall
//!    through to `Ok(task.clone())`. Refinement is purely a
//!    pre-implementation aid; if it fails we still deliver the task
//!    with the operator-authored description so the agent loop keeps
//!    making forward progress.
//! 4. On success we wrap the model output in a stable, traceable shape
//!    that preserves the original task block verbatim and persists it
//!    via `domain.update_task`. We then emit
//!    [`AutomatonEvent::TaskDescriptionRefined`] and return the
//!    updated descriptor returned by the domain API.

use aura_exec_tools::domain_tools::{DomainApi, SpecDescriptor, TaskDescriptor, TaskUpdate};
use aura_model::ModelProvider;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::common::{run_auxiliary_model_call, AuxiliaryModelCall};
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;

/// Idempotency marker prepended to every persisted refined
/// description. The leading-trim check in
/// [`refine_task_description`] uses this string verbatim so a future
/// schema bump can introduce `aura-refined:v2` without invalidating
/// existing rows (both versions will simply skip the second refinement
/// pass).
pub(crate) const REFINED_MARKER: &str = "<!-- aura-refined:v1 -->";

/// System prompt for the refinement call. Kept short on purpose —
/// the user message carries the spec / task body and a single
/// formatting instruction, so the system prompt only needs to set the
/// role.
const REFINEMENT_SYSTEM_PROMPT: &str =
    "You refine software engineering task descriptions before implementation. \
Rewrite the task description so it precisely matches the spec's intent and acceptance criteria. \
Keep it concise and actionable.";

/// Refine the description of `task` against `spec` via `provider`,
/// persist it through `domain`, and return the updated descriptor.
///
/// See module-level docs for the full behaviour contract. The
/// `model` parameter is the model identifier the underlying
/// `ModelRequest` will carry; both automatons already track their
/// configured model on the [`AgentRunner`] / dispatch JSON, so this
/// is plumbed in from the caller rather than re-derived inside the
/// helper.
///
/// Any failure (provider error, `update_task` error) is logged via
/// [`AutomatonEvent::LogLine`] and the original `task` is returned
/// unchanged. The function is therefore guaranteed not to error out
/// the upstream tick on refinement issues — refinement is best-effort.
pub(crate) async fn refine_task_description(
    domain: &dyn DomainApi,
    provider: &dyn ModelProvider,
    model: &str,
    spec: &SpecDescriptor,
    task: &TaskDescriptor,
    event_tx: Option<&mpsc::Sender<AutomatonEvent>>,
) -> Result<TaskDescriptor, AutomatonError> {
    // Idempotency: a previous run already persisted a refined body.
    // Skip silently — no provider call, no events. This is the
    // single authoritative gate now that Phase 7 removed the
    // dev-loop `build_retry_note.is_none()` outer check; it covers
    // both the build-retry second pass and ambient re-claims after
    // a process restart.
    if task.description.trim_start().starts_with(REFINED_MARKER) {
        debug!(
            task_id = %task.id,
            "task description already carries refinement marker; skipping"
        );
        return Ok(task.clone());
    }

    emit_best_effort(
        event_tx,
        AutomatonEvent::TaskDescriptionRefining {
            task_id: task.id.clone(),
        },
    );

    let Some(refined) = request_refinement(provider, model, spec, task, event_tx).await else {
        return Ok(task.clone());
    };
    let refined_body = assemble_refined_body(&refined, task);

    let Some(updated) = persist_refinement(domain, task, refined_body, event_tx).await else {
        return Ok(task.clone());
    };

    emit_best_effort(
        event_tx,
        AutomatonEvent::TaskDescriptionRefined {
            task_id: task.id.clone(),
        },
    );

    Ok(updated)
}

/// Asks the model for a refined description. Returns its trimmed text,
/// or `None` (after logging) when the call fails or the text is empty.
async fn request_refinement(
    provider: &dyn ModelProvider,
    model: &str,
    spec: &SpecDescriptor,
    task: &TaskDescriptor,
    event_tx: Option<&mpsc::Sender<AutomatonEvent>>,
) -> Option<String> {
    let call = AuxiliaryModelCall {
        model,
        system_prompt: REFINEMENT_SYSTEM_PROMPT,
        user_body: build_refinement_user_body(spec, task),
        task_scope: Some(task.id.clone()),
    };
    let response = match run_auxiliary_model_call(provider, call).await {
        Ok(r) => r,
        Err(e) => {
            warn!(task_id = %task.id, error = %e, "auxiliary model call rejected refinement");
            emit_refinement_failed(event_tx, task, &e);
            return None;
        }
    };

    let refined_text = response.message.text_content();
    let refined_trimmed = refined_text.trim();
    if refined_trimmed.is_empty() {
        // An empty rewrite is functionally no different from a
        // failure — the original description is strictly more
        // informative. Fall through with a LogLine instead of
        // persisting an empty body.
        emit_best_effort(
            event_tx,
            AutomatonEvent::LogLine {
                message: format!(
                    "task description refinement returned empty body for {}",
                    task.id
                ),
            },
        );
        return None;
    }
    Some(refined_trimmed.to_string())
}

/// Persists the refined body. Returns the updated descriptor, or `None`
/// (after logging) when `update_task` fails.
async fn persist_refinement(
    domain: &dyn DomainApi,
    task: &TaskDescriptor,
    refined_body: String,
    event_tx: Option<&mpsc::Sender<AutomatonEvent>>,
) -> Option<TaskDescriptor> {
    let update = TaskUpdate {
        description: Some(refined_body),
        ..Default::default()
    };
    match domain.update_task(&task.id, update, None).await {
        Ok(t) => Some(t),
        Err(e) => {
            warn!(task_id = %task.id, error = %e, "update_task rejected refined description");
            emit_refinement_failed(event_tx, task, &e);
            None
        }
    }
}

/// Reports a failed refinement step on the event channel.
fn emit_refinement_failed(
    event_tx: Option<&mpsc::Sender<AutomatonEvent>>,
    task: &TaskDescriptor,
    error: &dyn std::fmt::Display,
) {
    emit_best_effort(
        event_tx,
        AutomatonEvent::LogLine {
            message: format!(
                "task description refinement failed for {}: {error}",
                task.id
            ),
        },
    );
}

/// Build the user-message body for the refinement call. Mirrors the
/// shape used by `aura_engine_automaton::builtins::spec_gen` so the
/// auxiliary-call template in
/// [`super::common::run_auxiliary_model_call`] stays the single seam
/// where the `ModelRequest::builder(...)` knobs (`tool_choice`,
/// etc.) are picked.
fn build_refinement_user_body(spec: &SpecDescriptor, task: &TaskDescriptor) -> String {
    format!(
        "# Spec: {spec_title}\n\n{spec_content}\n\n\
         # Current Task: {task_title}\n\n{task_desc}\n\n\
         # Output\nReturn only the refined task description in markdown, no preamble.",
        spec_title = spec.title,
        spec_content = spec.content,
        task_title = task.title,
        task_desc = task.description,
    )
}

/// Wrap the model output and the original task block in the stable
/// shape persisted by [`refine_task_description`]. Exposed for tests
/// so the format can be asserted directly without round-tripping the
/// whole helper.
fn assemble_refined_body(refined: &str, task: &TaskDescriptor) -> String {
    let mut out = String::with_capacity(refined.len() + task.description.len() + 256);
    out.push_str(REFINED_MARKER);
    out.push('\n');
    out.push_str("## Refined Description\n");
    out.push_str(refined);
    out.push_str("\n\n## Original Task\n");
    out.push_str("> ");
    out.push_str(&task.title);
    out.push('\n');
    // Preserve a blank-quoted line between title and body so the
    // rendered markdown shows a paragraph break inside the blockquote.
    out.push_str(">\n");
    if task.description.is_empty() {
        out.push_str(">\n");
    } else {
        for line in task.description.lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Send an event without failing the caller. The two callers expect
/// `mpsc::Sender::try_send` semantics, but we use the async `send`
/// here for resilience to a transiently full channel. A closed
/// channel is a no-op — observability is best-effort.
fn emit_best_effort(event_tx: Option<&mpsc::Sender<AutomatonEvent>>, event: AutomatonEvent) {
    let Some(tx) = event_tx else { return };
    if let Err(e) = tx.try_send(event) {
        debug!(error = ?e, "task_refinement event drop (channel full or closed)");
    }
}

#[cfg(test)]
mod tests;
