//! What each attempt's run left: its saved session and its kept copy.

use std::path::Path;

use aura_core_types::AgentId;
use aura_gate::attempts::rereview::{Pick, Prepared};
use aura_gate::attempts::{self, Line, Tiers};
use aura_gate::live::sent_text;
use aura_model::Message;

use crate::event_loop::tasks::TASKS_DIR;
use crate::isolation::changes;
use crate::session_checkpoint::Checkpoints;

/// What each of `picks` left, read from the sessions saved under
/// `data_dir` and the runs' kept copies, with the tiers the signs in
/// `lines` have earned. An attempt whose run left too little is left out,
/// with a line saying why.
pub(super) fn all(picks: &[Pick<'_>], data_dir: &Path, lines: &[Line]) -> Vec<Prepared> {
    let tiers = attempts::tiers(lines);
    let sessions = Checkpoints::new(data_dir);
    picks
        .iter()
        .filter_map(|pick| match prepare(pick, &sessions, &tiers) {
            Ok(ready) => Some(ready),
            Err(problem) => {
                let truth = pick.stood.truth;
                println!(
                    "  left out {} {}: {problem}",
                    truth.run.as_deref().unwrap_or_default(),
                    truth.task.as_deref().unwrap_or_default()
                );
                None
            }
        })
        .collect()
}

/// Read what the attempt of `pick` left: its saved session in `sessions`
/// and what its run changed in the kept copy.
pub(super) fn prepare(
    pick: &Pick<'_>,
    sessions: &Checkpoints,
    tiers: &Tiers,
) -> Result<Prepared, String> {
    let attempt = pick.stood.attempt;
    let agent = AgentId::from_hex(&attempt.agent).map_err(|e| format!("its agent id: {e}"))?;
    let saved = sessions
        .load_current(agent)
        .ok_or("its session is not saved")?;
    let start = saved
        .messages
        .iter()
        .position(|message| sent_text(message).is_some())
        .ok_or("its saved session holds no prompt")?;
    let messages: &[Message] = &saved.messages[start..];
    let task = sent_text(&messages[0]).unwrap_or_default();
    let copy = pick.copy;
    let diff = changes::readable_diff_since(
        &copy.root,
        copy.git_dir.as_deref(),
        &copy.baseline,
        &[TASKS_DIR],
    )
    .map_err(|e| format!("its copy could not be compared with its baseline: {e}"))?;
    let changed = changes::paths_in(&diff);
    Ok(Prepared::of(pick, (task, messages), diff, &changed, tiers))
}
