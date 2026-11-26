//! The hub's HTTP client for one cloud node.
//!
//! Every call carries the node token from the fleet's claim and its own
//! timeout. The node's address is its boxd HTTPS domain.

use std::time::Duration;

use aura_agent::checkpoint::SessionCheckpoint;
use aura_config::BoxdToken;
use aura_core_protocol::cloud::{
    HostStats, MeshMail, SealResponse, SessionSaved, SessionStarted, SyncImport, SyncImported,
    WorkspacePatch,
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
    use aura_agent::checkpoint::SessionUsage;
    use aura_core_protocol::cloud::NodeClaim;
    use aura_core_types::Transaction;
    use aura_exec_tools::{ToolCatalog, ToolConfig};
    use aura_model::{Message, MockProvider, ModelProvider};
    use aura_node::test_support::{
        claim, create_router, RouterState, RouterStateConfig, Scheduler,
    };
    use aura_node::NodeConfig;
    use aura_store_db::{RocksStore, Store};
    use std::sync::Arc;

    const NODE_TOKEN: &str = "node-token-for-tests-0123";

    /// A cloud node served in-process, claimed for a hub holding
    /// [`NODE_TOKEN`], and a client for it.
    async fn serve_node(dir: &std::path::Path) -> NodeClient {
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.join("db"), false).expect("store"));
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("ok"));
        let scheduler = Arc::new(Scheduler::new(
            store.clone(),
            provider.clone(),
            Vec::new(),
            Vec::new(),
            dir.join("ws"),
            None,
        ));
        let state = RouterState::new(RouterStateConfig {
            store,
            scheduler,
            config: NodeConfig {
                cloud_node: true,
                data_dir: dir.to_path_buf(),
                ..NodeConfig::default()
            },
            provider,
            tool_config: ToolConfig::default(),
            catalog: Arc::new(ToolCatalog::new()),
            domain_api: None,
            automaton_controller: None,
            automaton_bridge: None,
            memory_manager: None,
            skill_manager: None,
            secrets_vault: None,
            process_store: None,
            trigger_registrar: None,
            router_url: None,
        });
        claim(
            &state,
            NodeClaim {
                node_id: "aura-test-0001".into(),
                node_token: NODE_TOKEN.into(),
                mesh_token: "mesh-token-for-tests-0123".into(),
                seal: Vec::new(),
                owner: None,
                plans: Vec::new(),
                git: None,
            },
            0,
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let app =
            create_router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        NodeClient::new("aura-test-0001", base, BoxdToken::new(NODE_TOKEN))
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
