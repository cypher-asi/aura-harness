//! The acceptance criteria written for replays, kept beside the attempt
//! log so every reviewer is asked about the same list, and their
//! writer's calls.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use aura_core_types::Hash;
use aura_gate::attempts::rereview::Prepared;
use aura_gate::judge::criteria::{self, Criteria};
use aura_model::model_catalog::{estimate_cost_usd, route_label};
use aura_model::ModelProvider;

/// How long writing one request's criteria may take.
const CRITERIA_TIMEOUT: Duration = Duration::from_secs(120);

/// Where the criteria written for replays are kept, under the data
/// directory.
const CRITERIA_FILE: &str = "gate/replay_criteria.json";

/// What writing one request's criteria is assumed to cost when
/// estimating, in USD.
const ESTIMATED_CRITERIA_USD: f64 = 0.03;

/// The criteria written for replays, by writer and request.
#[derive(Debug, Default)]
pub(super) struct Written {
    path: PathBuf,
    kept: BTreeMap<String, Criteria>,
}

impl Written {
    /// Those kept under `data_dir`; none when the file is missing or
    /// unreadable.
    pub(super) fn in_data_dir(data_dir: &Path) -> Self {
        let path = data_dir.join(CRITERIA_FILE);
        let kept = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, kept }
    }

    fn key(writer: &str, task: &str) -> String {
        format!("{writer} {}", Hash::from_content(task.as_bytes()).to_hex())
    }

    pub(super) fn get(&self, writer: &str, task: &str) -> Option<Arc<Criteria>> {
        self.kept
            .get(&Self::key(writer, task))
            .map(|found| Arc::new(found.clone()))
    }

    fn keep(&mut self, writer: &str, task: &str, criteria: &Criteria) -> anyhow::Result<()> {
        self.kept.insert(Self::key(writer, task), criteria.clone());
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.kept)?)
            .with_context(|| format!("writing {}", self.path.display()))
    }

    /// Say how many of the requests of `prepared` `writer` is to write
    /// criteria for, and what that may cost.
    pub(super) fn announce(&self, writer: &str, prepared: &[Prepared]) {
        let requests: HashSet<&str> = prepared.iter().map(|p| p.task.as_str()).collect();
        let to_write = requests
            .iter()
            .filter(|task| !self.kept.contains_key(&Self::key(writer, task)))
            .count();
        #[allow(clippy::cast_precision_loss)]
        let about = to_write as f64 * ESTIMATED_CRITERIA_USD;
        println!(
            "held to the criteria of {} requests: {to_write} to write with {} (about ${about:.2}), \
             the rest kept in {}; reviews held to criteria cost somewhat more than estimated",
            requests.len(),
            route_label(writer),
            self.path.display(),
        );
    }
}

/// Ask `writer` through `provider` for the criteria of `task`, and what
/// the call cost.
///
/// # Errors
/// Says why when the call failed or its reply held none, with what the
/// reply cost.
async fn write_criteria(
    provider: &Arc<dyn ModelProvider + Send + Sync>,
    writer: &str,
    task: &str,
    auth_token: Option<String>,
) -> Result<(Criteria, f64), (String, f64)> {
    let request = criteria::request(writer, task, auth_token)
        .map_err(|e| (format!("could not build the request: {e}"), 0.0))?;
    let response = match tokio::time::timeout(CRITERIA_TIMEOUT, provider.complete(request)).await {
        Err(_) => return Err(("the writer took too long".to_string(), 0.0)),
        Ok(Err(e)) => return Err((format!("the writer's call failed: {e}"), 0.0)),
        Ok(Ok(response)) => response,
    };
    let cost_usd = estimate_cost_usd(writer, &response.usage).unwrap_or(0.0);
    match criteria::criteria_of(&response) {
        Some(found) => Ok((found, cost_usd)),
        None => Err((criteria::none_because(&response.message), cost_usd)),
    }
}

/// Hold each of `prepared` to the criteria of its request, written by
/// `writer` through `provider` unless `written` already holds them.
/// Attempts whose request got none are left out, with a line saying why.
/// Returns the attempts and what the writing cost.
pub(super) async fn hold_to_criteria(
    prepared: Vec<Prepared>,
    written: &mut Written,
    writer: &str,
    (provider, auth_token): (&Arc<dyn ModelProvider + Send + Sync>, Option<String>),
) -> anyhow::Result<(Vec<Prepared>, f64)> {
    let mut cost_usd = 0.0;
    let mut failed: BTreeMap<String, String> = BTreeMap::new();
    let mut held = Vec::with_capacity(prepared.len());
    for ready in prepared {
        if let Some(why) = failed.get(&ready.task) {
            println!("  left out {} {}: {why}", ready.run, ready.task_id);
            continue;
        }
        let found = match written.get(writer, &ready.task) {
            Some(found) => found,
            None => match write_criteria(provider, writer, &ready.task, auth_token.clone()).await {
                Ok((found, cost)) => {
                    cost_usd += cost;
                    written.keep(writer, &ready.task, &found)?;
                    println!(
                        "  {:40} {} criteria{} (${cost:.3})",
                        ready.task_id,
                        found.items.len(),
                        if found.strict { ", strict" } else { "" },
                    );
                    Arc::new(found)
                }
                Err((problem, cost)) => {
                    cost_usd += cost;
                    let why = format!(
                        "{} wrote no criteria for its request: {problem}",
                        route_label(writer)
                    );
                    println!("  left out {} {}: {why}", ready.run, ready.task_id);
                    failed.insert(ready.task.clone(), why);
                    continue;
                }
            },
        };
        held.push(ready.held_to(found));
    }
    Ok((held, cost_usd))
}
