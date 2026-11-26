//! The executor hook that checkpoints files and shows their diffs.

use super::*;

/// A file-changing call in flight: its tool use id and the file.
pub(super) type CallKey = (String, PathBuf);

/// Checkpoints files for one agent's executor and shows each change in
/// its tool block.
pub(super) struct CheckpointHook {
    pub(super) files: FileCheckpoints,
    pub(super) chain: Vec<AgentId>,
    pub(super) commands: Option<mpsc::Sender<UiCommand>>,
    /// Content before each call in flight, for its diff
    pub(super) before: Mutex<HashMap<CallKey, Option<Vec<u8>>>>,
}

#[async_trait]
impl FileChangeHook for CheckpointHook {
    async fn before_change(&self, path: &Path, tool_use_id: Option<&str>) {
        let files = self.files.clone();
        let chain = self.chain.clone();
        let target = path.to_path_buf();
        let read = tokio::task::spawn_blocking(move || {
            let content = read_optional(&target)?;
            files.note_before(&chain, &target, content.as_deref());
            Ok::<_, std::io::Error>(content)
        })
        .await;
        let content = match read {
            Ok(Ok(content)) => content,
            Ok(Err(e)) => {
                warn!(path = %path.display(), error = %e, "Could not read file before a change");
                return;
            }
            Err(e) => {
                warn!(error = %e, "File checkpoint task failed");
                return;
            }
        };
        if let (Some(_), Some(id)) = (&self.commands, tool_use_id) {
            lock(&self.before).insert((id.to_string(), path.to_path_buf()), content);
        }
    }

    async fn after_change(&self, path: &Path, tool_use_id: Option<&str>, succeeded: bool) {
        let (Some(commands), Some(id)) = (&self.commands, tool_use_id) else {
            return;
        };
        let Some(before) = lock(&self.before).remove(&(id.to_string(), path.to_path_buf())) else {
            return;
        };
        if !succeeded {
            return;
        }
        let target = path.to_path_buf();
        let diff = tokio::task::spawn_blocking(move || {
            let after = read_optional(&target).ok()?;
            unified_diff(before.as_deref(), after.as_deref())
        })
        .await
        .ok()
        .flatten()
        .filter(|diff| !diff.is_empty());
        let (Some(diff), Some(agent)) = (diff, self.chain.first()) else {
            return;
        };
        AgentUi::new(commands.clone(), *agent)
            .send(UiCommand::ToolDiff {
                id: id.to_string(),
                diff,
            })
            .await;
    }
}
