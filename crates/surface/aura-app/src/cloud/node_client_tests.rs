//! The node client against a cloud node served in-process.

use aura_agent::checkpoint::{SessionCheckpoint, SessionUsage};
use aura_core_protocol::cloud::{SessionSaved, SessionStarted, SyncImport};
use aura_core_types::{AgentId, RecordEntry, Transaction};
use aura_model::Message;

use super::node_client::NodeClient;
use super::test_node::TestNode;

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
            RecordEntry::builder(seq, Transaction::user_prompt(agent, format!("p{seq}"))).build()
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
