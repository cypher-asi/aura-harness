//! A cloud node served in-process for the hub's tests: the real router
//! over a store of its own, claimed the way a machine's node is. Its
//! workspace is `ws` in its folder.

use std::path::Path;
use std::sync::Arc;

use aura_config::BoxdToken;
use aura_core_protocol::cloud::{NodeClaim, NodeClaimed};
use aura_engine_cloud::fake::ExecHandler;
use aura_engine_cloud::ExecOutput;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::{MockProvider, ModelProvider};
use aura_node::test_support::{create_router, RouterState, RouterStateConfig, Scheduler};
use aura_node::NodeConfig;
use aura_store_db::{RocksStore, Store};

use super::node_client::NodeClient;

/// The node id every test node is claimed as.
pub(crate) const NODE_ID: &str = "aura-test-0001";

/// One cloud node, serving on [`Self::base`].
pub(crate) struct TestNode {
    state: RouterState,
    /// Where the node's kernel writes.
    pub(crate) store: Arc<dyn Store>,
    /// `http://127.0.0.1:<port>`.
    pub(crate) base: String,
}

impl TestNode {
    /// An unclaimed cloud node over a store in `dir`, its model always
    /// answering `reply`.
    pub(crate) async fn serve(dir: &Path, reply: &str) -> Self {
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.join("db"), false).expect("store"));
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response(reply));
        let scheduler = Arc::new(Scheduler::new(
            Arc::clone(&store),
            Arc::clone(&provider),
            Vec::new(),
            Vec::new(),
            dir.join("ws"),
            None,
        ));
        let state = RouterState::new(RouterStateConfig {
            store: Arc::clone(&store),
            scheduler,
            config: NodeConfig {
                cloud_node: true,
                data_dir: dir.to_path_buf(),
                workspace: Some(dir.join("ws")),
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let app = create_router(state.clone())
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { state, store, base }
    }

    /// Claim it for a hub holding `token`.
    pub(crate) fn claim_for(&self, token: &str) -> NodeClaimed {
        aura_node::test_support::claim(
            &self.state,
            NodeClaim {
                node_id: NODE_ID.into(),
                node_token: token.into(),
                mesh_token: "mesh-token-for-tests-0123".into(),
                seal: Vec::new(),
                owner: None,
                plans: Vec::new(),
                git: None,
            },
            0,
        )
    }

    /// A client of it holding `token`.
    pub(crate) fn client(&self, token: &str) -> NodeClient {
        NodeClient::new(NODE_ID, self.base.clone(), BoxdToken::new(token))
    }

    /// What fake boxd runs in the machine: `aura node claim --stdin`
    /// hands this node the claim on stdin, as its claim socket would;
    /// every other command succeeds and prints nothing.
    pub(crate) fn exec_handler(&self) -> ExecHandler {
        let state = self.state.clone();
        Arc::new(move |_machine, command, stdin| {
            let output = |exit_code, stdout: Vec<u8>| ExecOutput {
                exit_code,
                stdout,
                stderr: Vec::new(),
            };
            if !command.contains("node claim") {
                return output(0, Vec::new());
            }
            match stdin.and_then(|bytes| serde_json::from_slice::<NodeClaim>(bytes).ok()) {
                Some(claim) => {
                    let claimed = aura_node::test_support::claim(&state, claim, 0);
                    output(0, serde_json::to_vec(&claimed).unwrap_or_default())
                }
                None => output(2, Vec::new()),
            }
        })
    }

    /// [`Self::exec_handler`], its claim answered as a node from before
    /// the node protocol answers: without one.
    pub(crate) fn older_exec_handler(&self) -> ExecHandler {
        let current = self.exec_handler();
        Arc::new(move |machine, command, stdin| {
            let mut output = current(machine, command, stdin);
            if let Ok(serde_json::Value::Object(mut answer)) =
                serde_json::from_slice::<serde_json::Value>(&output.stdout)
            {
                answer.remove("protocol");
                output.stdout = serde_json::to_vec(&answer).unwrap_or_default();
            }
            output
        })
    }
}
