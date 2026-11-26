//! Suite loading: the tasks of a suite directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use aura_app::repo_source::Source;
use serde::Deserialize;

use super::{DEFAULT_CATEGORY, TASK_FILE};

/// One eval task.
#[derive(Debug, Clone, Deserialize)]
pub struct EvalTask {
    /// Directory name; filled in by [`load_suite`].
    #[serde(skip)]
    pub id: String,
    /// Task directory; filled in by [`load_suite`].
    #[serde(skip)]
    pub dir: PathBuf,
    /// Short title.
    #[serde(default)]
    pub title: String,
    /// Category reports group by (default [`DEFAULT_CATEGORY`]).
    #[serde(default)]
    pub category: String,
    /// Free-form labels.
    #[serde(default)]
    pub tags: Vec<String>,
    /// First user prompt.
    pub prompt: String,
    /// Follow-up prompts, one per extra turn.
    #[serde(default)]
    pub follow_ups: Vec<String>,
    /// Shell command run in the workspace; exit status 0 passes.
    pub verify: String,
    /// Iteration cap for each turn.
    #[serde(default)]
    pub max_iterations: Option<usize>,
    /// Repository the workspace starts from, before `workspace/`.
    #[serde(default)]
    pub source: Option<Source>,
}

/// Load every task under `suite`, sorted by id.
///
/// # Errors
///
/// Fails when the directory can't be read or a `task.toml` is invalid.
pub fn load_suite(suite: &Path) -> anyhow::Result<Vec<EvalTask>> {
    let mut tasks = Vec::new();
    let entries =
        std::fs::read_dir(suite).with_context(|| format!("reading suite {}", suite.display()))?;
    for entry in entries {
        let dir = entry?.path();
        let file = dir.join(TASK_FILE);
        if !file.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&file)?;
        let mut task: EvalTask =
            toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
        task.id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if task.title.is_empty() {
            task.title.clone_from(&task.id);
        }
        if task.category.is_empty() {
            task.category = DEFAULT_CATEGORY.to_string();
        }
        task.dir = dir;
        tasks.push(task);
    }
    if tasks.is_empty() {
        bail!("no tasks (`*/{TASK_FILE}`) under {}", suite.display());
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tasks)
}

/// Suite name recorded in each run's `task_id` (`<suite>/<task>`).
#[must_use]
pub fn suite_name(suite: &Path) -> String {
    suite
        .file_name()
        .map_or_else(|| "suite".into(), |n| n.to_string_lossy().into_owned())
}

/// Category of each task id (`<suite>/<task>`) in `tasks`.
#[must_use]
pub fn categories(suite: &str, tasks: &[EvalTask]) -> BTreeMap<String, String> {
    tasks
        .iter()
        .map(|t| (format!("{suite}/{}", t.id), t.category.clone()))
        .collect()
}
