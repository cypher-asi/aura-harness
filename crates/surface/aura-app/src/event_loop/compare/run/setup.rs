//! The example tasks' files, put in place before their runs: each task's
//! files are written once, to a fixture in the comparison's folder that
//! its runs' copies are made from.

use super::context::Context;
use crate::bench_pack::ExampleTask;
use aura_ui_terminal::events::BenchTaskSetup;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Folder of a comparison's copies that holds the example tasks' files
/// the copies are made from.
pub(super) const FIXTURES: &str = "fixtures";

/// Example tasks whose files are fetched at once.
const FETCHES: usize = 3;

/// Where `task`'s files are kept in the comparison's folder `dir`.
pub(super) fn fixture(dir: &Path, task: &ExampleTask) -> PathBuf {
    dir.join(FIXTURES).join(task.id.replace('/', "__"))
}

/// Put each of `examples`' files in its fixture under `dir`, `FETCHES` at
/// a time in order, telling `states` where each stands, until `cancel`
/// stops it. Tasks with the same source take turns, as they would write
/// the same cache.
pub(super) async fn set_up(
    examples: Vec<&'static ExampleTask>,
    dir: PathBuf,
    states: watch::Sender<Vec<BenchTaskSetup>>,
    cancel: CancellationToken,
) {
    let cache = crate::repo_source::cache_dir();
    let states = Arc::new(states);
    let permits = Arc::new(Semaphore::new(FETCHES));
    let mut sources: HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>> = HashMap::new();
    let mut fetching = JoinSet::new();
    for (i, task) in examples.into_iter().enumerate() {
        let permit = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            permit = Arc::clone(&permits).acquire_owned() => permit,
        };
        let Ok(permit) = permit else { break };
        let same = task
            .source
            .as_ref()
            .and_then(|source| source.dir(&cache).ok())
            .map(|dir| Arc::clone(sources.entry(dir).or_default()));
        let (states, cache, target, cancel) = (
            Arc::clone(&states),
            cache.clone(),
            fixture(&dir, task),
            cancel.clone(),
        );
        fetching.spawn(async move {
            let _permit = permit;
            let _turn = match same {
                Some(same) => Some(same.lock_owned().await),
                None => None,
            };
            let tell = |state| states.send_modify(|s| s[i] = state);
            fetch(task, &target, &cache, tell, &cancel).await;
        });
    }
    while fetching.join_next().await.is_some() {}
}

/// Write `task`'s files to `target`, pulling its source into `cache`
/// first when it must, and `tell` where they stand, unless `cancel`
/// stops it first.
async fn fetch(
    task: &'static ExampleTask,
    target: &Path,
    cache: &Path,
    tell: impl Fn(BenchTaskSetup),
    cancel: &CancellationToken,
) {
    let pull = match &task.source {
        Some(source) => {
            let cache = cache.to_path_buf();
            tokio::task::spawn_blocking(move || source.pulls(&cache))
                .await
                .unwrap_or(false)
        }
        None => false,
    };
    tell(BenchTaskSetup::Fetching { pull });
    let written = tokio::select! {
        written = task.materialize(target, cache) => written,
        () = cancel.cancelled() => return,
    };
    tell(match written {
        Ok(()) => BenchTaskSetup::Ready,
        Err(e) => {
            warn!(task = %task.id, error = %e, "Could not write an example task's files");
            BenchTaskSetup::Failed(e.to_string())
        }
    });
}

/// Wait until `task`'s files are in place. Errors with why its runs can't
/// run, or `None` when the user stopped the comparison first.
pub(super) async fn files_of(ctx: &Context, task: &ExampleTask) -> Result<(), Option<String>> {
    let Some(i) = ctx.plan.examples.iter().position(|t| t.id == task.id) else {
        return Ok(());
    };
    let mut setup = ctx.setup.clone();
    let settled = |s: &Vec<BenchTaskSetup>| {
        !matches!(
            s.get(i),
            Some(BenchTaskSetup::Queued | BenchTaskSetup::Fetching { .. })
        )
    };
    let state = tokio::select! {
        state = setup.wait_for(settled) => state.ok().and_then(|s| s.get(i).cloned()),
        () = ctx.cancel.cancelled() => return Err(None),
    };
    match state {
        Some(BenchTaskSetup::Failed(why)) => {
            Err(Some(format!("writing the files of {}: {why}", task.id)))
        }
        Some(_) => Ok(()),
        None => Err(None),
    }
}
