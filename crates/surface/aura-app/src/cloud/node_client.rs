//! The hub's HTTP client for one cloud node.
//!
//! Every call carries the node token from the fleet's claim and its own
//! timeout. The node's address is its boxd HTTPS domain.

use std::time::Duration;

use aura_agent::checkpoint::SessionCheckpoint;
use aura_config::BoxdToken;
use aura_core_protocol::cloud::{
    HostStats, MeshMail, SealResponse, SessionSaved, SessionStarted, SyncImport, SyncImported,
    TracePage, WorkspacePatch,
};
use aura_core_types::{AgentId, RecordEntry};
use aura_engine_cloud::{CloudError, CloudResult};
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

/// One entry of a directory listed on a node (`GET /api/files`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileEntry {
    /// File name
    pub name: String,
    /// Whether it is a directory
    pub is_dir: bool,
}

#[derive(Deserialize)]
struct Listing {
    entries: Vec<FileEntry>,
}

#[derive(Deserialize)]
struct FileText {
    content: String,
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

    /// Make `checkpoint` `agent`'s current session on the node, so its
    /// next run there continues it.
    ///
    /// # Errors
    ///
    /// When the node refuses it (sealed, or past the node's record).
    pub async fn put_session(
        &self,
        agent: AgentId,
        checkpoint: &SessionCheckpoint,
    ) -> CloudResult<SessionSaved> {
        self.read(
            "session",
            self.http
                .put(format!("{}/v1/sync/session/{}", self.base, agent.to_hex()))
                .json(checkpoint),
        )
        .await
    }

    /// `agent`'s current session on the node, if it has one.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn session(&self, agent: AgentId) -> CloudResult<Option<SessionCheckpoint>> {
        self.read(
            "session",
            self.http
                .get(format!("{}/v1/sync/session/{}", self.base, agent.to_hex())),
        )
        .await
    }

    /// `agent`'s newest trace runs on the node, whole.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn traces(&self, agent: AgentId) -> CloudResult<TracePage> {
        self.read(
            "traces",
            self.http
                .get(format!("{}/v1/sync/traces", self.base))
                .query(&[("agent", agent.to_hex())]),
        )
        .await
    }

    /// Start a new session for `agent` on the node.
    ///
    /// # Errors
    ///
    /// When the call fails.
    pub async fn new_session(&self, agent: AgentId) -> CloudResult<SessionStarted> {
        self.read(
            "new session",
            self.http.post(format!(
                "{}/v1/sync/session/{}/new",
                self.base,
                agent.to_hex()
            )),
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

    /// What directory `dir` holds on the node; nothing when it is gone.
    /// Dot entries and build output directories are left out.
    ///
    /// # Errors
    ///
    /// When the call fails or the node refuses the path.
    pub async fn list_files(&self, dir: &str) -> CloudResult<Vec<FileEntry>> {
        let request = self
            .http
            .get(format!("{}/api/files", self.base))
            .query(&[("path", dir), ("depth", "1")])
            .bearer_auth(self.token.expose());
        let response = request
            .send()
            .await
            .map_err(|err| self.fail("list files", err))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(self.fail(
                "list files",
                format!("{status}: {}", body.chars().take(300).collect::<String>()),
            ));
        }
        let listing: Listing = response
            .json()
            .await
            .map_err(|err| self.fail("list files", err))?;
        Ok(listing.entries)
    }

    /// The text of file `path` on the node.
    ///
    /// # Errors
    ///
    /// When the call fails, or the file is missing or too large.
    pub async fn read_file(&self, path: &str) -> CloudResult<String> {
        let text: FileText = self
            .read(
                "read file",
                self.http
                    .get(format!("{}/api/read-file", self.base))
                    .query(&[("path", path)]),
            )
            .await?;
        Ok(text.content)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::test_node::TestNode;
    use aura_agent::checkpoint::SessionUsage;
    use aura_core_types::Transaction;
    use aura_model::Message;

    const NODE_TOKEN: &str = "node-token-for-tests-0123";

    /// A cloud node served in-process, claimed for a hub holding
    /// [`NODE_TOKEN`], and a client for it.
    async fn serve_node(dir: &std::path::Path) -> NodeClient {
        let node = TestNode::serve(dir, "ok").await;
        node.claim_for(NODE_TOKEN);
        node.client(NODE_TOKEN)
    }

    fn checkpoint(record_seq: u64) -> SessionCheckpoint {
        SessionCheckpoint::new(
            1_000,
            record_seq,
            vec![Message::user("p1"), Message::assistant("a1")],
            Vec::new(),
            SessionUsage::default(),
        )
    }

    #[tokio::test]
    async fn a_session_goes_over_and_a_new_one_starts_on_the_node() {
        let dir = tempfile::tempdir().expect("tempdir");
        let node = serve_node(dir.path()).await;
        let agent = AgentId::new([4; 32]);
        let entries = (1..=2)
            .map(|seq| {
                RecordEntry::builder(seq, Transaction::user_prompt(agent, format!("p{seq}")))
                    .build()
            })
            .collect();
        node.import(&SyncImport {
            agent_id: agent.to_hex(),
            name: "re36".into(),
            entries,
            done: true,
        })
        .await
        .expect("import");

        assert!(
            node.put_session(agent, &checkpoint(3)).await.is_err(),
            "a session past the node's record is refused"
        );
        let saved = node
            .put_session(agent, &checkpoint(2))
            .await
            .expect("saved");
        assert_eq!(
            saved,
            SessionSaved {
                record_seq: 2,
                messages: 2
            }
        );

        let started = node.new_session(agent).await.expect("started");
        assert_eq!(started, SessionStarted { head_seq: 3 });
        assert_eq!(node.head(agent).await.expect("head"), 3);
    }
}
