//! What an attempt added that no test in its workspace names, so the
//! tests that passed could not have checked it.

use crate::isolation::changes;
use aura_gate::evidence::{coverage, is_test_path};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::warn;

/// Files of a workspace [`untested`] reads at most; past them, what the
/// attempt added counts as named.
const MAX_SEARCHED: usize = 20_000;

/// Test files bigger than this are not read.
const MAX_TEST_BYTES: u64 = 1024 * 1024;

/// How long looking through a workspace's tests may take.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);

/// `names` when no test file under `workspace` names any of them, else
/// none. Hidden folders (Aura's task records among them, which quote the
/// prompt) and build output are not searched; a search that fails or
/// takes too long finds them named.
pub(super) async fn untested(workspace: PathBuf, names: Vec<String>) -> Vec<String> {
    let found = tokio::task::spawn_blocking(move || {
        let named = named_by_a_test(&workspace, &names);
        (!named).then_some(names)
    });
    match tokio::time::timeout(SEARCH_TIMEOUT, found).await {
        Ok(Ok(Some(names))) => names,
        Ok(_) => Vec::new(),
        Err(_) => {
            warn!("Looking for tests of what the attempt added took too long");
            Vec::new()
        }
    }
}

/// Whether a test file under `workspace` names any of `names`; also when
/// there are more files than are searched.
fn named_by_a_test(workspace: &Path, names: &[String]) -> bool {
    let searched = walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|entry| {
            let skipped = entry.depth() > 0
                && entry.file_type().is_dir()
                && entry.file_name().to_str().is_none_or(|name| {
                    name.starts_with('.') || changes::BUILD_OUTPUT.contains(&name)
                });
            !skipped
        });
    for (seen, entry) in searched.filter_map(Result::ok).enumerate() {
        if seen >= MAX_SEARCHED {
            return true;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(workspace).unwrap_or(entry.path());
        if !is_test_path(&relative.to_string_lossy())
            || entry.metadata().map_or(true, |m| m.len() > MAX_TEST_BYTES)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if names.iter().any(|name| coverage::mentions(&text, name)) {
            return true;
        }
    }
    false
}
