//! The reviewer's calls, and the log's lines for its answers.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use aura_gate::attempts::rereview::{Asked, Prepared};
use aura_gate::attempts::AttemptLog;
use aura_gate::judge::turn::{self, ReviewVariant};
use aura_model::model_catalog::route_label;
use aura_model::ModelProvider;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::event_loop::processes::now_ms;

/// How long one review may take.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(420);

/// The model that reviews and how it is asked, and the log its answers
/// go to.
#[derive(Clone)]
pub(super) struct Reviewer {
    pub(super) provider: Arc<dyn ModelProvider + Send + Sync>,
    pub(super) auth_token: Option<String>,
    pub(super) judge: String,
    pub(super) variant: ReviewVariant,
    pub(super) log: Arc<AttemptLog>,
}

impl Reviewer {
    /// Ask about `prepared`, and append the answer to the log.
    pub(super) async fn ask(&self, prepared: Prepared) -> Asked {
        let why = prepared.why_told();
        let standing = prepared.standing(&why);
        let auth_token = self.auth_token.clone();
        let request = match turn::review_request(&self.judge, &standing, self.variant, auth_token) {
            Ok(request) => request,
            Err(e) => return Asked::failed(&prepared, format!("could not build the request: {e}")),
        };
        let asked =
            match tokio::time::timeout(REVIEW_TIMEOUT, self.provider.complete(request)).await {
                Err(_) => Asked::failed(&prepared, "the reviewer took too long".to_string()),
                Ok(Err(e)) => Asked::failed(&prepared, format!("the reviewer's call failed: {e}")),
                Ok(Ok(response)) => Asked::of(&prepared, &self.judge, &response),
            };
        if let Ok((verdict, findings, cost_usd)) = &asked.answer {
            let answered = (*verdict, findings.as_str(), *cost_usd);
            let line = prepared.review(&self.judge, self.variant, answered, now_ms());
            if let Err(e) = self.log.append(&line) {
                let problem = format!("could not write the attempt log: {e}");
                return Asked::failed(&prepared, problem);
            }
        }
        asked
    }

    /// Ask about each of `prepared`, `jobs` at a time, printing each
    /// answer as it comes. The first is asked alone: when the reviewer
    /// cannot be reached for it (not signed in, no network), the rest
    /// would only wait as long, so none of them is asked.
    ///
    /// # Errors
    /// When the reviewer could not be reached for the first, or a review
    /// stopped unexpectedly.
    pub(super) async fn ask_all(
        &self,
        prepared: Vec<Prepared>,
        jobs: usize,
    ) -> anyhow::Result<Vec<Asked>> {
        let mut prepared = prepared.into_iter();
        let mut all = Vec::new();
        if let Some(first) = prepared.next() {
            let asked = self.ask(first).await;
            println!("{}", asked.row());
            if let Some(problem) = asked.unreached() {
                anyhow::bail!(
                    "{} could not be asked ({problem}); no other attempt was asked about",
                    route_label(&self.judge)
                );
            }
            all.push(asked);
        }
        let jobs = Arc::new(Semaphore::new(jobs.max(1)));
        let mut asking = JoinSet::new();
        for ready in prepared {
            let (reviewer, jobs) = (self.clone(), Arc::clone(&jobs));
            asking.spawn(async move {
                let _permit = jobs.acquire_owned().await;
                reviewer.ask(ready).await
            });
        }
        while let Some(joined) = asking.join_next().await {
            let asked = joined.context("a review stopped unexpectedly")?;
            println!("{}", asked.row());
            all.push(asked);
        }
        Ok(all)
    }
}
