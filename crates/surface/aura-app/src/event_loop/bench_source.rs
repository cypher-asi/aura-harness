//! A session as a benchmark's work: its prompts in order, the files as
//! they were when it started, what it cost, and what the user kept from
//! it (the reference the judge rates each run against).

#![deny(clippy::too_many_lines)]

use super::LoopState;
use crate::file_checkpoints::{FileCheckpoints, TurnManifest};
use crate::session_checkpoint::{SessionCheckpoint, SessionUsage};
use aura_core_types::{AgentId, BenchSource, Reference, SessionBaseline};
use aura_gate::compare::{relative, title, SessionPick};
use aura_gate::judge::blind::MAX_DIFF;
use aura_model::{Message, Role};
use aura_ui_terminal::events::BenchSession;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Sessions the BENCH tab offers at most.
const MAX_SESSIONS: usize = 20;

/// A file to put in each copy before a replay, relative to the workspace:
/// what it held when the session started, or `None` when it did not exist.
pub(super) type StartFile = (PathBuf, Option<Vec<u8>>);

/// A session ready to replay.
#[derive(Debug, Clone)]
pub(super) struct Replay {
    pub prompts: Vec<String>,
    pub source: BenchSource,
    pub start: Vec<StartFile>,
    pub notes: Vec<String>,
}

/// One of the agent's sessions: borrowed when it is open, else loaded
/// from its checkpoint.
struct Session<'a> {
    started_ms: u64,
    messages: Cow<'a, [Message]>,
    usage: Cow<'a, SessionUsage>,
    current: bool,
}

impl Session<'_> {
    /// The session `checkpoint` holds.
    fn checkpointed(checkpoint: SessionCheckpoint, current: bool) -> Self {
        Self {
            started_ms: checkpoint.started_ms,
            messages: Cow::Owned(checkpoint.messages),
            usage: Cow::Owned(checkpoint.usage),
            current,
        }
    }
}

/// The agent's current session: its open one, else its latest checkpoint.
fn current<'a>(state: &'a LoopState<'_>, agent_id: AgentId) -> Option<Session<'a>> {
    if let Some(open) = state.idle.get(&agent_id).filter(|s| !s.messages.is_empty()) {
        return Some(Session {
            started_ms: open.started_ms,
            messages: Cow::Borrowed(open.messages.as_slice()),
            usage: Cow::Borrowed(&open.usage),
            current: true,
        });
    }
    let checkpoint = state
        .hub
        .factory
        .checkpoints
        .as_ref()?
        .load_current(agent_id)?;
    Some(Session::checkpointed(checkpoint, true))
}

fn session<'a>(
    state: &'a LoopState<'_>,
    agent_id: AgentId,
    pick: SessionPick,
) -> Option<Session<'a>> {
    let now = current(state, agent_id);
    match pick {
        SessionPick::Current => now,
        SessionPick::Started(ms) if now.as_ref().is_some_and(|s| s.started_ms == ms) => now,
        SessionPick::Started(ms) => {
            let checkpoint = state.hub.factory.checkpoints.as_ref()?.load(agent_id, ms)?;
            Some(Session::checkpointed(checkpoint, false))
        }
    }
}

/// The agent's sessions with something to replay, current first, then
/// newest first.
pub(super) fn sessions(state: &LoopState<'_>, agent_id: AgentId) -> Vec<BenchSession> {
    let files = state.hub.factory.file_checkpoints.as_ref();
    let listed = |s: &Session<'_>| {
        let manifests = manifests(files, agent_id, s.started_ms);
        let prompts = prompts(&s.messages, &manifests);
        (!prompts.is_empty()).then(|| BenchSession {
            started_ms: s.started_ms,
            title: title(&prompts[0]),
            prompts: prompts.len(),
            cost_usd: s.usage.priced.then_some(s.usage.cost_usd),
            unpriced_calls: s.usage.unpriced_calls,
            models: s.usage.models.clone(),
            current: s.current,
        })
    };
    let now = current(state, agent_id);
    let mut out: Vec<BenchSession> = now.as_ref().and_then(listed).into_iter().collect();
    let Some(checkpoints) = state.hub.factory.checkpoints.as_ref() else {
        return out;
    };
    let except = now.as_ref().map(|s| s.started_ms);
    for listing in checkpoints.list(agent_id, except) {
        if out.len() >= MAX_SESSIONS {
            break;
        }
        let Some(checkpoint) = checkpoints.load(agent_id, listing.started_ms) else {
            continue;
        };
        out.extend(listed(&Session::checkpointed(checkpoint, false)));
    }
    out
}

fn manifests(
    files: Option<&FileCheckpoints>,
    agent_id: AgentId,
    started_ms: u64,
) -> Vec<TurnManifest> {
    files.map_or_else(Vec::new, |files| {
        files.session_manifests(agent_id, started_ms)
    })
}

/// What the user typed in `messages`, in order: the prompts the session's
/// file checkpoints name when they name any, else every user message that
/// isn't tool results.
fn prompts(messages: &[Message], manifests: &[TurnManifest]) -> Vec<String> {
    aura_gate::compare::prompts(messages, |text| {
        manifests.iter().any(|m| !m.rewound && m.is_prompt(text))
    })
}

/// The session `pick` of `agent_id`, ready to replay in copies of
/// `workspace`.
///
/// # Errors
/// When there is no such session or it has no prompts.
pub(super) fn load(
    state: &LoopState<'_>,
    agent_id: AgentId,
    pick: SessionPick,
    workspace: &Path,
) -> Result<Replay, String> {
    let session = session(state, agent_id, pick).ok_or_else(|| match pick {
        SessionPick::Current => "This chat has no session to replay yet".to_string(),
        SessionPick::Started(ms) => format!("No session started at {ms}"),
    })?;
    let files = state.hub.factory.file_checkpoints.as_ref();
    let manifests = manifests(files, agent_id, session.started_ms);
    let prompts = prompts(&session.messages, &manifests);
    let first = prompts
        .first()
        .ok_or("That session has no prompts to replay")?;
    let mut notes = Vec::new();
    let start = start(files, agent_id, &manifests, workspace, &mut notes);
    let diff = files.map_or_else(String::new, |files| {
        reference_diff(files, agent_id, &manifests, workspace)
    });
    if !diff.is_empty() && !session.current {
        notes.push(
            "The reference diff is against the files as they are now, so later changes to \
             them show up in it"
                .to_string(),
        );
    }
    let reply = last_reply(&session.messages);
    let reference = (!diff.is_empty() || !reply.is_empty()).then_some(Reference { diff, reply });
    let usage = &session.usage;
    let source = BenchSource::Session {
        agent_id,
        started_ms: session.started_ms,
        title: title(first),
        baseline: baseline(usage),
        reference,
    };
    Ok(Replay {
        prompts,
        source,
        start,
        notes,
    })
}

/// What `usage` says the session cost, to set the replays against. Its
/// unpriced calls are kept so that a partly priced cost is not taken for
/// the whole.
fn baseline(usage: &SessionUsage) -> SessionBaseline {
    SessionBaseline {
        calls: usage.calls,
        cost_usd: usage.cost_usd,
        priced: usage.priced,
        unpriced_calls: usage.unpriced_calls,
        models: usage.models.clone(),
    }
}

/// What each copy starts from, with a note saying which: the files the
/// session changed as they were when it started, or the files as they are
/// now (nothing to put back) when its file checkpoints name none.
fn start(
    files: Option<&FileCheckpoints>,
    agent_id: AgentId,
    manifests: &[TurnManifest],
    workspace: &Path,
    notes: &mut Vec<String>,
) -> Vec<StartFile> {
    match files {
        Some(files) if !manifests.is_empty() => {
            notes.push(
                "Each copy starts from the files as they were when the session started; \
                 changes its shell commands made are not undone"
                    .to_string(),
            );
            start_files(files, agent_id, manifests, workspace, notes)
        }
        _ => {
            notes.push(
                "The session changed no checkpointed files, so each copy starts from the \
                 files as they are now"
                    .to_string(),
            );
            Vec::new()
        }
    }
}

/// The text of the last reply in `messages` that has any; empty when none
/// has.
fn last_reply(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant)
        .map(Message::text_content)
        .find(|text| !text.trim().is_empty())
        .unwrap_or_default()
}

/// What each file the session changed held when it started, relative to
/// `workspace`. Files outside it or whose checkpoint can't be read are
/// left out, with a note.
fn start_files(
    files: &FileCheckpoints,
    agent_id: AgentId,
    manifests: &[TurnManifest],
    workspace: &Path,
    notes: &mut Vec<String>,
) -> Vec<StartFile> {
    let mut out = Vec::new();
    for (path, content) in files.before(agent_id, manifests) {
        let Some(relative) = relative(&path, workspace) else {
            notes.push(format!(
                "{} is outside the workspace; the copies keep it as it is",
                path.display()
            ));
            continue;
        };
        match content {
            Ok(content) => out.push((relative, content)),
            Err(problem) => notes.push(format!(
                "{} was not put back: {problem}",
                relative.display()
            )),
        }
    }
    out
}

/// The session's changes to the files as they are now, cut to what the
/// judge reads.
fn reference_diff(
    files: &FileCheckpoints,
    agent_id: AgentId,
    manifests: &[TurnManifest],
    workspace: &Path,
) -> String {
    let mut diff = String::new();
    for file in files.diff(agent_id, manifests, Some(workspace)) {
        let _ = write!(diff, "--- a/{0}\n+++ b/{0}\n{1}", file.path, file.diff);
        if !diff.ends_with('\n') {
            diff.push('\n');
        }
        if diff.len() > MAX_DIFF {
            break;
        }
    }
    if let Some((cut, _)) = diff.char_indices().nth(MAX_DIFF) {
        diff.truncate(cut);
    }
    diff
}

/// Put the session's starting files in the copy at `workdir`.
///
/// # Errors
/// The first file that could not be written or removed.
pub(super) fn restore(workdir: &Path, start: &[StartFile]) -> Result<(), String> {
    for (relative, content) in start {
        let target = workdir.join(relative);
        let written = match content {
            Some(bytes) => target
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&target, bytes)),
            None => match std::fs::remove_file(&target) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        written.map_err(|e| format!("putting back {}: {e}", relative.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partly_priced_session_keeps_its_unpriced_calls_in_the_baseline() {
        let usage = SessionUsage {
            calls: 5,
            cost_usd: 0.3,
            priced: true,
            unpriced_calls: 2,
            models: vec!["opus".to_string(), "glm-9".to_string()],
            ..SessionUsage::default()
        };
        let partly = baseline(&usage);
        assert_eq!(partly.unpriced_calls, 2);
        assert!(partly.priced, "the priced part is still shown");
        assert_eq!(partly.known_cost_usd(), None, "but not set against");
        let whole = baseline(&SessionUsage {
            unpriced_calls: 0,
            ..usage
        });
        assert_eq!(whole.known_cost_usd(), Some(0.3));
    }

    #[test]
    fn restore_writes_and_removes_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("added.txt"), "new").unwrap();
        std::fs::write(dir.path().join("kept.txt"), "edited").unwrap();
        let start = vec![
            (PathBuf::from("added.txt"), None),
            (PathBuf::from("kept.txt"), Some(b"original".to_vec())),
            (PathBuf::from("sub/gone.txt"), Some(b"was here".to_vec())),
        ];
        restore(dir.path(), &start).unwrap();
        assert!(!dir.path().join("added.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("kept.txt")).unwrap(),
            "original"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("sub/gone.txt")).unwrap(),
            "was here"
        );
    }
}
