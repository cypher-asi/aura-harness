//! What can be done with a comparison that ended while its copies are
//! kept: apply a run's changes to the user's workspace, show the runs'
//! diffs, or dismiss them; applying and dismissing remove the copies.

use super::kernel_of;
use super::record::comparisons;
use super::view::{plural, split_patch};
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::compare_run::{patches, remove_copies, save};
use crate::event_loop::handlers::system_message;
use crate::event_loop::LoopState;
use crate::isolation::changes;
use aura_core_types::{AgentId, CompareArm, ComparePayload, CompareStatus};
use aura_ui_terminal::UiCommand;

/// The comparison `host` holds, else the latest, once it has ended, for
/// apply, diff and dismiss.
fn ended(
    state: &LoopState<'_>,
    host: Option<AgentId>,
) -> Result<(AgentId, String, ComparePayload), String> {
    let all = comparisons(state).into_iter().rev();
    let (host, name, payload) = match host {
        Some(wanted) => all
            .filter(|(id, _, _)| *id == wanted)
            .find_map(|(id, name, payload)| Some((id, name, payload?)))
            .ok_or_else(|| "No such benchmark".to_string())?,
        None => all
            .filter_map(|(id, name, payload)| Some((id, name, payload?)))
            .next()
            .ok_or_else(|| "No comparison yet; /compare [prompt] runs one".to_string())?,
    };
    if state.compare.running().is_some_and(|r| r.host == host)
        || payload.status == CompareStatus::Running
    {
        return Err(format!("{name} is still running; wait for it to end"));
    }
    Ok((host, name, payload))
}

/// `/compare apply <run>`: put the run's changes in the user's
/// workspace, then remove the copies.
pub(crate) async fn apply(
    state: &mut LoopState<'_>,
    host: Option<AgentId>,
    name: &str,
) -> Result<String, String> {
    let (host, compare, payload) = ended(state, host)?;
    if !payload.source.examples().is_empty() {
        return Err(format!(
            "{compare} ran example tasks in their own files; there is nothing to apply to your \
             workspace"
        ));
    }
    if !payload.status.has_copies() {
        return Err(format!("{compare}'s copies were already removed"));
    }
    if payload.run(name).is_none() {
        return Err(format!(
            "No run {name} in {compare}; runs are named like a-1"
        ));
    }
    let (mut payload, files, problems) = apply_run(payload, name.to_string()).await?;
    payload.status = CompareStatus::Applied(name.to_string());
    save_state(state, host, &compare, &payload).await?;
    Ok(format!(
        "{compare}: applied {name} ({files} file{}) to {}{}",
        plural(files),
        payload.apply_root.display(),
        removed_line(&problems),
    ))
}

/// Put the changes of `payload`'s run `name` in its workspace, then
/// remove its copies, off the event loop: `payload` back, with how many
/// files the run changed and what could not be removed.
async fn apply_run(
    payload: ComparePayload,
    name: String,
) -> Result<(ComparePayload, usize, Vec<String>), String> {
    tokio::task::spawn_blocking(move || {
        let patch = patches(&payload)
            .remove(&name)
            .unwrap_or_else(|| Err(format!("{name} has no copy")))?;
        let files = changes::files_in(&patch);
        changes::apply_patch(&payload.apply_root, &patch).map_err(|e| {
            format!(
                "{name} did not apply to {}: {e}",
                payload.apply_root.display()
            )
        })?;
        let problems = remove_copies(&payload);
        Ok((payload, files, problems))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What removing the copies came to, after the comparison's own line.
fn removed_line(problems: &[String]) -> String {
    if problems.is_empty() {
        "; removed the copies".to_string()
    } else {
        format!("; some copies were not removed: {}", problems.join("; "))
    }
}

/// `/compare dismiss`: remove the copies without applying anything.
pub(crate) async fn dismiss(
    state: &mut LoopState<'_>,
    host: Option<AgentId>,
) -> Result<String, String> {
    if host.is_none() && state.compare.pending.take().is_some() {
        return Ok("Dropped the comparison".to_string());
    }
    let (host, compare, payload) = ended(state, host)?;
    if !payload.status.has_copies() {
        return Err(format!("{compare}'s copies were already removed"));
    }
    let (mut payload, problems) = tokio::task::spawn_blocking(move || {
        let problems = remove_copies(&payload);
        (payload, problems)
    })
    .await
    .map_err(|e| e.to_string())?;
    payload.status = CompareStatus::Dismissed;
    save_state(state, host, &compare, &payload).await?;
    Ok(format!("{compare}: dismissed{}", removed_line(&problems)))
}

/// Record `payload` as the latest state of the comparison `compare` that
/// `host` holds.
async fn save_state(
    state: &mut LoopState<'_>,
    host: AgentId,
    compare: &str,
    payload: &ComparePayload,
) -> Result<(), String> {
    let kernel = kernel_of(state, host)?;
    save(
        &state.hub,
        &kernel,
        state.commands,
        (host, compare),
        payload,
    )
    .await;
    Ok(())
}

/// `/compare diff [run]`: the run's changes, or the first run of each
/// arm one after the other.
pub(crate) async fn diff(
    state: &LoopState<'_>,
    ui: &AgentUi,
    host: Option<AgentId>,
    run: Option<&str>,
) -> Result<String, String> {
    let (_, compare, payload) = ended(state, host)?;
    if !payload.status.has_copies() {
        return Err(format!(
            "{compare}'s copies were removed, and their diffs with them"
        ));
    }
    let names: Vec<String> = match run {
        Some(name) => {
            payload
                .run(name)
                .ok_or_else(|| format!("No run {name} in {compare}"))?;
            vec![name.to_string()]
        }
        None => CompareArm::BOTH
            .iter()
            .filter_map(|arm| payload.arm_runs(*arm).next().map(|r| r.name.clone()))
            .collect(),
    };
    let (payload, mut all) = tokio::task::spawn_blocking(move || {
        let all = patches(&payload);
        (payload, all)
    })
    .await
    .map_err(|e| e.to_string())?;
    for name in names {
        let model = payload.run(&name).map_or("", |r| r.model.as_str());
        let title = format!("{compare} {name} ({model})");
        show_patch(ui, title, all.remove(&name)).await;
    }
    Ok(String::new())
}

/// Show the run titled `title` its `patch`: its diff, that it changed
/// nothing, or why it has none.
async fn show_patch(ui: &AgentUi, title: String, patch: Option<Result<String, String>>) {
    let shown = match patch {
        Some(Ok(patch)) if patch.trim().is_empty() => {
            system_message(format!("{title}: no changes"))
        }
        Some(Ok(patch)) => UiCommand::ShowDiff {
            files: split_patch(&patch),
            title,
        },
        Some(Err(problem)) => UiCommand::ShowWarning(format!("{title}: {problem}")),
        None => UiCommand::ShowWarning(format!("{title}: it has no copy")),
    };
    ui.send(shown).await;
}
