//! The copies the runs work in: each made from the user's workspace or an
//! example task's files, what each changed since it was made, and their
//! removal once the comparison is applied or dismissed.

use super::setup::FIXTURES;
use crate::event_loop::bench_source::{self, StartFile};
use crate::event_loop::tasks::TASKS_DIR;
use crate::isolation::changes::{self, BUILD_OUTPUT};
use crate::isolation::{
    CopyIsolation, IsolatedWorkspace, Isolation, IsolationStrategy, WorktreeIsolation,
};
use aura_core_types::{AgentId, CompareCopy, ComparePayload};
use aura_exec_tools::RunScratch;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Make the copy run `name` works in: a detached worktree of the user's
/// repository with their uncommitted work carried over, else a copy of
/// their folder without build output, with `start` put in it. Returns it
/// with a note when the uncommitted work could not be carried.
pub(super) fn prepare_copy(
    dir: &Path,
    workspace: &Path,
    repo: Option<&Path>,
    plan_file: Option<&str>,
    start: &[StartFile],
    name: &str,
) -> Result<(CompareCopy, Option<String>), String> {
    let problem = |e: crate::isolation::IsolationError| format!("preparing the copy: {e}");
    let (copy, note) = if let Some(repo) = repo {
        let ws = WorktreeIsolation::new(dir)
            .provision(repo, name)
            .map_err(problem)?;
        let note = changes::carry_changes(repo, &ws.root, BUILD_OUTPUT)
            .err()
            .map(|e| {
                format!("{name} ran on HEAD: your uncommitted changes did not carry over ({e})")
            });
        let inside = workspace
            .strip_prefix(repo)
            .unwrap_or_else(|_| Path::new(""));
        let workdir = ws.root.join(inside);
        std::fs::create_dir_all(&workdir).map_err(|e| format!("preparing the copy: {e}"))?;
        let copy = CompareCopy {
            root: ws.root,
            workdir,
            git_dir: None,
            baseline: String::new(),
        };
        (copy, note)
    } else {
        let ws = CopyIsolation::new(dir)
            .skipping(BUILD_OUTPUT.iter().copied())
            .provision(workspace, name)
            .map_err(problem)?;
        let copy = CompareCopy {
            workdir: ws.root.clone(),
            root: ws.root,
            git_dir: Some(dir.join(format!("{name}.git"))),
            baseline: String::new(),
        };
        (copy, None)
    };
    if let Some(plan_file) = plan_file {
        let target = copy.workdir.join(plan_file);
        if !target.exists() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("copying the plan: {e}"))?;
            }
            std::fs::copy(workspace.join(plan_file), &target)
                .map_err(|e| format!("copying the plan: {e}"))?;
        }
    }
    bench_source::restore(&copy.workdir, start)?;
    let baseline = changes::baseline(&copy.root, copy.git_dir.as_deref()).map_err(problem)?;
    Ok((CompareCopy { baseline, ..copy }, note))
}

/// The scratch folder of run `name`'s commands (see
/// [`aura_exec_tools::RunScratch`]): beside its copy in `dir`, not in it,
/// so nothing the run's tools write there shows in its changes.
pub(super) fn scratch_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.tmp"))
}

/// What the run of `agent_id` changed in `copy` since it was made, as a
/// patch, read off the async runtime; empty, with the problem logged,
/// when it can't be read.
pub(super) async fn patch_of(copy: &CompareCopy, agent_id: AgentId) -> String {
    let (root, git_dir, baseline) = (
        copy.root.clone(),
        copy.git_dir.clone(),
        copy.baseline.clone(),
    );
    tokio::task::spawn_blocking(move || {
        changes::diff_since(&root, git_dir.as_deref(), &baseline, &[TASKS_DIR])
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|diff| diff.map_err(|e| e.to_string()))
    .unwrap_or_else(|problem| {
        warn!(%agent_id, %problem, "Could not diff a compare run");
        String::new()
    })
}

/// Remove the copies of `payload`'s runs and the folder they were in.
/// Returns what could not be removed.
pub(crate) fn remove_copies(payload: &ComparePayload) -> Vec<String> {
    let mut problems = Vec::new();
    let mut parents: Vec<PathBuf> = Vec::new();
    for run in &payload.runs {
        let copy = &run.copy;
        if copy.root.as_os_str().is_empty() {
            continue;
        }
        let Some(parent) = copy.root.parent().map(Path::to_path_buf) else {
            continue;
        };
        remove_copy(&run.name, copy, &parent, &mut problems);
        if !parents.contains(&parent) {
            parents.push(parent);
        }
    }
    for parent in parents {
        let fixtures = parent.join(FIXTURES);
        if !payload.source.examples().is_empty() && fixtures.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&fixtures) {
                problems.push(format!("{}: {e}", fixtures.display()));
            }
        }
        // Only once it is empty; anything else in it is not ours.
        let _ = std::fs::remove_dir(parent);
    }
    problems
}

/// Remove run `name`'s `copy` in `parent`, its git folder and its scratch
/// folder, adding to `problems` what could not be removed.
fn remove_copy(name: &str, copy: &CompareCopy, parent: &Path, problems: &mut Vec<String>) {
    let workspace = IsolatedWorkspace {
        root: copy.root.clone(),
        strategy: if copy.git_dir.is_some() {
            IsolationStrategy::Copy
        } else {
            IsolationStrategy::Worktree
        },
    };
    let removed = if copy.git_dir.is_some() {
        CopyIsolation::new(parent).teardown(&workspace)
    } else {
        WorktreeIsolation::new(parent).teardown(&workspace)
    };
    if let Err(e) = removed {
        problems.push(format!("{name}: {e}"));
    }
    if let Some(git_dir) = copy.git_dir.as_deref().filter(|dir| dir.exists()) {
        if let Err(e) = std::fs::remove_dir_all(git_dir) {
            problems.push(format!("{}: {e}", git_dir.display()));
        }
    }
    if copy.root.exists() {
        problems.push(format!("{} is still there", copy.root.display()));
    }
    let scratch = copy
        .root
        .file_name()
        .map(|dir_name| RunScratch::new(scratch_dir(parent, &dir_name.to_string_lossy())));
    if let Some(scratch) = scratch.filter(|scratch| scratch.dir().exists()) {
        if let Err(e) = scratch.remove() {
            problems.push(format!("{}: {e}", scratch.dir().display()));
        }
    }
}

/// What each run of `payload` changed since its copy was made, by run
/// name; runs whose copy is gone or can't be read are left out.
pub(crate) fn patches(payload: &ComparePayload) -> HashMap<String, Result<String, String>> {
    payload
        .runs
        .iter()
        .filter(|run| !run.copy.root.as_os_str().is_empty())
        .map(|run| {
            let copy = &run.copy;
            let patch = if copy.root.exists() {
                changes::diff_since(
                    &copy.root,
                    copy.git_dir.as_deref(),
                    &copy.baseline,
                    &[TASKS_DIR],
                )
                .map_err(|e| e.to_string())
            } else {
                Err(format!("the copy of {} was removed", run.name))
            };
            (run.name.clone(), patch)
        })
        .collect()
}
