//! The example coding tasks the BENCH tab offers, built in: every task of
//! `evals/coding`, then `evals/gate_cost`, `evals/gate_hard`,
//! `evals/polyglot`, `evals/codebases` and `evals/swe_bench_pro_hard`, each
//! with its fixture, its prompts, its check command and the hidden checks
//! that command reads. A task set in an existing codebase names what its
//! fixture starts from ([`Source`]): a repository at a commit, or the
//! repository in a container image, fetched when it first runs. A
//! benchmark of `n` of them takes the first `n`.

use crate::headless::copy_dir;
use crate::repo_source::Source;
use aura_core_types::ExampleTaskInfo;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Files of a task, relative to its fixture, with what they hold.
type Files = &'static [(&'static str, &'static [u8])];

/// A task as `build.rs` embedded it.
struct PackEntry {
    suite: &'static str,
    slug: &'static str,
    toml: &'static str,
    workspace: Files,
    checks: Files,
}

const PACK: &[PackEntry] = include!(concat!(env!("OUT_DIR"), "/bench_pack.rs"));

#[derive(Deserialize)]
struct TaskToml {
    #[serde(default)]
    title: String,
    #[serde(default)]
    category: String,
    prompt: String,
    #[serde(default)]
    follow_ups: Vec<String>,
    verify: String,
    #[serde(default)]
    source: Option<Source>,
}

/// One built-in example task.
#[derive(Debug, Clone)]
pub struct ExampleTask {
    /// `<suite>/<task>`, as `coding/write-file`
    pub id: String,
    pub title: String,
    pub category: String,
    /// Its first prompt, then its follow-ups
    pub prompts: Vec<String>,
    /// Command run in the copy after the run; exit 0 passes
    pub verify: String,
    /// The repository its fixture starts from, for a task set in an
    /// existing codebase
    pub source: Option<Source>,
    workspace: Files,
    checks: Files,
}

/// Every built-in example task, in the order a benchmark takes them.
#[must_use]
pub fn tasks() -> &'static [ExampleTask] {
    static TASKS: OnceLock<Vec<ExampleTask>> = OnceLock::new();
    TASKS.get_or_init(|| {
        PACK.iter()
            .filter_map(|entry| {
                let id = format!("{}/{}", entry.suite, entry.slug);
                let parsed: TaskToml = match toml::from_str(entry.toml) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        tracing::warn!(%id, error = %e, "A built-in example task does not parse");
                        return None;
                    }
                };
                let mut prompts = vec![parsed.prompt];
                prompts.extend(parsed.follow_ups);
                Some(ExampleTask {
                    title: if parsed.title.is_empty() {
                        id.clone()
                    } else {
                        parsed.title
                    },
                    id,
                    category: parsed.category,
                    prompts,
                    verify: parsed.verify,
                    source: parsed.source,
                    workspace: entry.workspace,
                    checks: entry.checks,
                })
            })
            .collect()
    })
}

impl ExampleTask {
    /// The suite it is from, as `gate_hard`.
    #[must_use]
    pub fn suite(&self) -> &str {
        self.id.split_once('/').map_or("", |(suite, _)| suite)
    }
}

/// The suites of the built-in tasks, in the order a benchmark takes them.
#[must_use]
pub fn suites() -> Vec<&'static str> {
    let mut suites: Vec<&'static str> = Vec::new();
    for task in tasks() {
        if !suites.contains(&task.suite()) {
            suites.push(task.suite());
        }
    }
    suites
}

/// The task with id `id`.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static ExampleTask> {
    tasks().iter().find(|task| task.id == id)
}

/// Write `files` under `dir`, refusing paths that would leave it.
fn write(dir: &Path, files: Files) -> std::io::Result<()> {
    for (relative, bytes) in files {
        let relative = Path::new(relative);
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(std::io::Error::other(format!(
                "{} is not inside the task",
                relative.display()
            )));
        }
        let target = dir.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, bytes)?;
    }
    Ok(())
}

impl ExampleTask {
    /// What a comparison records of it.
    #[must_use]
    pub fn info(&self) -> ExampleTaskInfo {
        ExampleTaskInfo {
            id: self.id.clone(),
            title: self.title.clone(),
            prompts: self.prompts.clone(),
            check: self.verify.clone(),
        }
    }

    /// Whether its check reads hidden files put in only after the run.
    #[must_use]
    pub const fn has_hidden_checks(&self) -> bool {
        !self.checks.is_empty()
    }

    /// Write its fixture into `dir`, which may not exist yet: its
    /// repository's files when it names one (downloaded into `cache` the
    /// first time), then its own.
    ///
    /// # Errors
    /// A download that failed, or the first file that could not be written.
    pub async fn materialize(&self, dir: &Path, cache: &Path) -> std::io::Result<()> {
        let repository = match &self.source {
            Some(source) => Some(source.fetch(cache).await?),
            None => None,
        };
        let (dir, workspace) = (dir.to_path_buf(), self.workspace);
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            if let Some(repository) = repository {
                copy_dir(&repository, &dir)?;
            }
            write(&dir, workspace)
        })
        .await
        .map_err(std::io::Error::other)?
    }

    /// Put its hidden checks in `workdir`. Returns what was added there
    /// (the top-level files and folders that did not exist), for
    /// [`remove_checks`] to take out again.
    ///
    /// # Errors
    /// The first file that could not be written.
    pub fn place_checks(&self, workdir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut added: Vec<PathBuf> = Vec::new();
        for (relative, _) in self.checks {
            if let Some(Component::Normal(top)) = Path::new(relative).components().next() {
                let top = workdir.join(top);
                if !top.exists() && !added.contains(&top) {
                    added.push(top);
                }
            }
        }
        write(workdir, self.checks)?;
        Ok(added)
    }
}

/// Take out what [`ExampleTask::place_checks`] added.
pub fn remove_checks(added: &[PathBuf]) {
    for path in added {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(e) = removed {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "Could not remove a hidden check");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_suite_is_built_in_easiest_first() {
        let all = tasks();
        assert_eq!(all.len(), PACK.len(), "every task.toml parses");
        assert!(all.len() >= 30, "{} tasks", all.len());
        let ids: HashSet<&str> = all.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), all.len(), "ids are unique");
        let suite = |t: &ExampleTask| t.id.split('/').next().unwrap_or_default().to_string();
        let order: Vec<String> = all.iter().map(suite).collect();
        let first = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(first("coding") < first("gate_cost") && first("gate_cost") < first("gate_hard"));
        assert!(first("gate_hard") < first("polyglot") && first("polyglot") < first("codebases"));
        assert!(first("codebases") < first("swe_bench_pro_hard"));
        assert_eq!(
            suites(),
            [
                "coding",
                "gate_cost",
                "gate_hard",
                "polyglot",
                "codebases",
                "swe_bench_pro_hard"
            ]
        );
        for task in all {
            let expected = match task.suite() {
                "codebases" => Some("repo"),
                "swe_bench_pro_hard" => Some("image"),
                _ => None,
            };
            let source = task.source.as_ref().map(|source| match source {
                Source::Repo(_) => "repo",
                Source::Image(_) => "image",
            });
            assert_eq!(source, expected, "{}", task.id);
        }
        assert_eq!(
            all.iter()
                .filter(|t| t.suite() == "swe_bench_pro_hard")
                .count(),
            51
        );
        assert_eq!(
            by_id("gate_hard/semver-precedence").unwrap().suite(),
            "gate_hard"
        );
        for task in all {
            assert!(
                !task.prompts[0].trim().is_empty(),
                "{} has a prompt",
                task.id
            );
            assert!(!task.verify.trim().is_empty(), "{} has a check", task.id);
        }
        assert!(by_id("coding/write-file").is_some());
        assert_eq!(
            by_id("gate_cost/long-session-todo-cli").map(|t| t.prompts.len()),
            Some(3)
        );
    }

    #[test]
    fn hidden_checks_and_solutions_stay_out_of_the_fixture() {
        for task in tasks() {
            for (path, _) in task.workspace {
                assert!(!path.starts_with(".gate_checks"), "{}: {path}", task.id);
            }
        }
        let median = by_id("gate_cost/bugfix-py-median").unwrap();
        assert!(median.has_hidden_checks());
        assert!(median.workspace.iter().any(|(p, _)| *p == "stats.py"));
        assert!(!by_id("coding/write-file").unwrap().has_hidden_checks());
    }

    #[tokio::test]
    async fn a_fixture_is_written_and_its_checks_come_and_go() {
        let dir = tempfile::tempdir().unwrap();
        let task = by_id("gate_cost/bugfix-py-median").unwrap();
        task.materialize(dir.path(), Path::new("/nowhere"))
            .await
            .unwrap();
        assert!(dir.path().join("stats.py").is_file());
        let added = task.place_checks(dir.path()).unwrap();
        assert_eq!(added, vec![dir.path().join(".gate_checks")]);
        assert!(dir.path().join(".gate_checks/check.py").is_file());
        remove_checks(&added);
        assert!(!dir.path().join(".gate_checks").exists());
        assert!(dir.path().join("stats.py").is_file());
    }

    #[tokio::test]
    async fn a_codebase_task_starts_from_its_repository_then_its_own_files() {
        let cache = tempfile::tempdir().unwrap();
        let task = tasks()
            .iter()
            .find(|t| t.suite() == "codebases")
            .expect("a codebase task");
        let repository = task.source.as_ref().unwrap().dir(cache.path()).unwrap();
        std::fs::create_dir_all(repository.join("pkg")).unwrap();
        std::fs::write(repository.join("pkg/code.txt"), "before the fix").unwrap();
        let dir = tempfile::tempdir().unwrap();
        task.materialize(dir.path(), cache.path()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("pkg/code.txt")).unwrap(),
            "before the fix"
        );
        assert!(task.has_hidden_checks());
        assert!(!dir.path().join(".gate_checks").exists());
    }

    #[tokio::test]
    async fn a_swe_bench_pro_task_adds_its_container_helper_to_the_repository() {
        let cache = tempfile::tempdir().unwrap();
        let task = by_id("swe_bench_pro_hard/nodebb-8168c6c4").unwrap();
        let Some(Source::Image(image)) = &task.source else {
            panic!("{:?}", task.source);
        };
        let repository = task.source.as_ref().unwrap().dir(cache.path()).unwrap();
        std::fs::create_dir_all(&repository).unwrap();
        std::fs::write(repository.join("app.js"), "// NodeBB").unwrap();
        let dir = tempfile::tempdir().unwrap();
        task.materialize(dir.path(), cache.path()).await.unwrap();
        assert!(dir.path().join("app.js").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".swe/image"))
                .unwrap()
                .trim(),
            image.image
        );
        assert!(dir.path().join(".swe/run").is_file());
        assert!(task.prompts[0].contains("sh .swe/run"));
        assert_eq!(task.verify, "sh .gate_checks/grade.sh");
        let added = task.place_checks(dir.path()).unwrap();
        assert!(dir
            .path()
            .join(".gate_checks/tests/test_patch.patch")
            .is_file());
        remove_checks(&added);
    }
}
