//! A session as a benchmark's work: its prompts in order, the files as
//! they were when it started, what it cost, and what the user kept from
//! it (the reference the judge rates each run against).

use super::LoopState;
use crate::file_checkpoints::{FileCheckpoints, TurnManifest};
use crate::session_checkpoint::SessionUsage;
use aura_agent_gate::compare::SessionPick;
use aura_agent_gate::judge::MAX_DIFF;
use aura_core_types::{AgentId, BenchSource, Reference, SessionBaseline};
use aura_model::{ContentBlock, Message, Role};
use aura_ui_terminal::events::BenchSession;
use std::path::{Path, PathBuf};

/// Sessions the BENCH tab offers at most.
const MAX_SESSIONS: usize = 20;

/// Longest session title, in characters.
const TITLE_CHARS: usize = 60;

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

/// One of the agent's sessions.
struct Session {
    started_ms: u64,
    messages: Vec<Message>,
    usage: SessionUsage,
    current: bool,
}

/// The agent's current session: its open one, else its latest checkpoint.
fn current(state: &LoopState<'_>, agent_id: AgentId) -> Option<Session> {
    if let Some(session) = state.idle.get(&agent_id) {
        if !session.messages.is_empty() {
            return Some(Session {
                started_ms: session.started_ms,
                messages: session.messages.clone(),
                usage: session.usage.clone(),
                current: true,
            });
        }
    }
    let checkpoint = state
        .hub
        .factory
        .checkpoints
        .as_ref()?
        .load_current(agent_id)?;
    Some(Session {
        started_ms: checkpoint.started_ms,
        messages: checkpoint.messages,
        usage: checkpoint.usage,
        current: true,
    })
}

fn session(state: &LoopState<'_>, agent_id: AgentId, pick: SessionPick) -> Option<Session> {
    let now = current(state, agent_id);
    match pick {
        SessionPick::Current => now,
        SessionPick::Started(ms) if now.as_ref().is_some_and(|s| s.started_ms == ms) => now,
        SessionPick::Started(ms) => {
            let checkpoint = state.hub.factory.checkpoints.as_ref()?.load(agent_id, ms)?;
            Some(Session {
                started_ms: checkpoint.started_ms,
                messages: checkpoint.messages,
                usage: checkpoint.usage,
                current: false,
            })
        }
    }
}

/// The agent's sessions with something to replay, current first, then
/// newest first.
pub(super) fn sessions(state: &LoopState<'_>, agent_id: AgentId) -> Vec<BenchSession> {
    let files = state.hub.factory.file_checkpoints.as_ref();
    let listed = |s: &Session| {
        let manifests = manifests(files, agent_id, s.started_ms);
        let prompts = prompts(&s.messages, &manifests);
        (!prompts.is_empty()).then(|| BenchSession {
            started_ms: s.started_ms,
            title: title(&prompts[0]),
            prompts: prompts.len(),
            cost_usd: s.usage.priced.then_some(s.usage.cost_usd),
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
        let past = Session {
            started_ms: checkpoint.started_ms,
            messages: checkpoint.messages,
            usage: checkpoint.usage,
            current: false,
        };
        out.extend(listed(&past));
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
    let typed: Vec<String> = messages
        .iter()
        .filter(|m| m.role == Role::User)
        .filter(|m| {
            !m.content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        })
        .map(Message::text_content)
        .filter(|text| !text.trim().is_empty())
        .collect();
    let kept: Vec<&TurnManifest> = manifests.iter().filter(|m| !m.rewound).collect();
    let named: Vec<String> = typed
        .iter()
        .filter(|text| kept.iter().any(|m| m.is_prompt(text)))
        .cloned()
        .collect();
    if named.is_empty() {
        typed
    } else {
        named
    }
}

fn title(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default().trim();
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().count() > TITLE_CHARS {
        title.push('…');
    }
    title
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
    let start = match files {
        Some(files) if !manifests.is_empty() => {
            notes.push(
                "Each copy starts from the files as they were when the session started; \
                 changes its shell commands made are not undone"
                    .to_string(),
            );
            start_files(files, agent_id, &manifests, workspace, &mut notes)
        }
        _ => {
            notes.push(
                "The session changed no checkpointed files, so each copy starts from the \
                 files as they are now"
                    .to_string(),
            );
            Vec::new()
        }
    };
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
    let reply = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && !m.text_content().trim().is_empty())
        .map(Message::text_content)
        .unwrap_or_default();
    let reference = (!diff.is_empty() || !reply.is_empty()).then_some(Reference { diff, reply });
    let usage = &session.usage;
    let source = BenchSource::Session {
        agent_id,
        started_ms: session.started_ms,
        title: title(first),
        baseline: SessionBaseline {
            calls: usage.calls,
            cost_usd: usage.cost_usd,
            priced: usage.priced,
            models: usage.models.clone(),
        },
        reference,
    };
    Ok(Replay {
        prompts,
        source,
        start,
        notes,
    })
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

/// `path` relative to `workspace`, when inside it.
fn relative(path: &Path, workspace: &Path) -> Option<PathBuf> {
    if path.is_relative() {
        return Some(path.to_path_buf());
    }
    if let Ok(inside) = path.strip_prefix(workspace) {
        return Some(inside.to_path_buf());
    }
    let canonical = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(path.file_name())
        .map(|(parent, name)| parent.join(name))?;
    canonical
        .strip_prefix(workspace)
        .ok()
        .map(Path::to_path_buf)
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
        diff.push_str(&format!("--- a/{0}\n+++ b/{0}\n{1}", file.path, file.diff));
        if !diff.ends_with('\n') {
            diff.push('\n');
        }
        if diff.len() > MAX_DIFF {
            break;
        }
    }
    diff.chars().take(MAX_DIFF).collect()
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
    fn prompts_skip_tool_results_and_empty_messages() {
        let tool_result = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t".to_string(),
                content: "ok".into(),
                is_error: false,
            }],
        };
        let messages = vec![
            Message::user("fix the test"),
            Message::assistant("on it"),
            tool_result,
            Message::user("  "),
            Message::user("now the docs"),
        ];
        assert_eq!(
            prompts(&messages, &[]),
            vec!["fix the test".to_string(), "now the docs".to_string()]
        );
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

    #[test]
    fn paths_are_made_relative_to_the_workspace() {
        let ws = Path::new("/work/project");
        assert_eq!(
            relative(Path::new("/work/project/src/lib.rs"), ws),
            Some(PathBuf::from("src/lib.rs"))
        );
        assert_eq!(relative(Path::new("/elsewhere/x"), ws), None);
    }
}
