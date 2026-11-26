//! The hub's HTTP client for one cloud node.
//!
//! Every call carries the node token from the fleet's claim and its own
//! timeout. The node's address is its boxd HTTPS domain.

use std::time::Duration;

use crate::{CloudError, CloudResult};
use aura_config::BoxdToken;
use aura_core_protocol::cloud::{
    HostStats, MeshMail, SealResponse, SyncImport, SyncImported, WorkspacePatch,
};
use aura_core_types::{AgentId, RecordEntry};
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// Limit on ordinary node calls.
pub const NODE_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest record page asked for.
pub const PAGE: usize = 500;

/// A client for one node.
#[derive(Clone)]
pub struct NodeClient {
    node: String,
    base: String,
    token: BoxdToken,
    http: reqwest::Client,
}

impl std::fmt::Debug for NodeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeClient")
            .field("node", &self.node)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Head {
    head_seq: u64,
}

impl NodeClient {
    /// A client for node `node` at `base` (`https://<name>.boxd.sh`).
    #[must_use]
    pub fn new(node: impl Into<String>, base: impl Into<String>, token: BoxdToken) -> Self {
        Self {
            node: node.into(),
            base: base.into().trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::builder()
                .timeout(NODE_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    /// The node id.
    #[must_use]
    pub fn node(&self) -> &str {
        &self.node
    }

    /// The node's base URL.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The node token, for the run stream.
    #[must_use]
    pub const fn token(&self) -> &BoxdToken {
        &self.token
    }

    fn fail(&self, op: &'static str, message: impl std::fmt::Display) -> CloudError {
        CloudError::Node {
            node: self.node.clone(),
            op,
            message: message.to_string(),
        }
    }

    async fn read<T: DeserializeOwned>(
        &self,
        op: &'static str,
        request: reqwest::RequestBuilder,
    ) -> CloudResult<T> {
        let response = request
            .bearer_auth(self.token.expose())
            .send()
            .await
            .map_err(|err| self.fail(op, err))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(self.fail(
                op,
                format!("{status}: {}", body.chars().take(300).collect::<String>()),
            ));
        }
        response.json::<T>().await.map_err(|err| self.fail(op, err))
    }

    /// Whether the node answers `/health`.
    pub async fn healthy(&self) -> bool {
        self.http
            .get(format!("{}/health", self.base))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
    }

    /// The node's head for `agent`.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn head(&self, agent: AgentId) -> CloudResult<u64> {
        let head: Head = self
            .read(
                "head",
                self.http
                    .get(format!("{}/agents/{}/head", self.base, agent.to_hex())),
            )
            .await?;
        Ok(head.head_seq)
    }

    /// Up to [`PAGE`] entries of `agent` from `from_seq`.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn record(&self, agent: AgentId, from_seq: u64) -> CloudResult<Vec<RecordEntry>> {
        self.read(
            "record",
            self.http
                .get(format!("{}/agents/{}/record", self.base, agent.to_hex()))
                .query(&[
                    ("from_seq", from_seq.to_string()),
                    ("limit", PAGE.to_string()),
                ]),
        )
        .await
    }

    /// Seed the node with entries.
    ///
    /// # Errors
    ///
    /// When the node refuses the page.
    pub async fn import(&self, import: &SyncImport) -> CloudResult<SyncImported> {
        self.read(
            "import",
            self.http
                .post(format!("{}/v1/sync/import", self.base))
                .json(import),
        )
        .await
    }

    /// Seal `agent` on the node.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn seal(&self, agent: AgentId) -> CloudResult<SealResponse> {
        self.read(
            "seal",
            self.http
                .post(format!("{}/v1/sync/seal/{}", self.base, agent.to_hex())),
        )
        .await
    }

    /// The node's host stats.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn host(&self) -> CloudResult<HostStats> {
        self.read("host", self.http.get(format!("{}/v1/host", self.base)))
            .await
    }

    /// The agent's work as a patch.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn patch(&self) -> CloudResult<WorkspacePatch> {
        self.read(
            "patch",
            self.http
                .get(format!("{}/v1/workspace/patch", self.base))
                .timeout(Duration::from_secs(120)),
        )
        .await
    }

    /// Hand one message to an agent on the node.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn mail(&self, mail: &MeshMail) -> CloudResult<serde_json::Value> {
        self.read(
            "mail",
            self.http
                .post(format!("{}/v1/mesh/mail", self.base))
                .json(mail),
        )
        .await
    }

    /// Start a run on the node (`POST /v1/run`); returns its JSON answer
    /// (`run_id`, `event_stream_url`).
    ///
    /// # Errors
    ///
    /// When the node refuses the run.
    pub async fn start_run(
        &self,
        request: &aura_core_protocol::RuntimeRequest,
    ) -> CloudResult<serde_json::Value> {
        self.read(
            "run",
            self.http
                .post(format!("{}/v1/run", self.base))
                .json(request),
        )
        .await
    }
}
