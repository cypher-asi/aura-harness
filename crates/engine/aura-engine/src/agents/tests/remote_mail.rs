use super::*;

#[tokio::test]
async fn mail_for_an_agent_on_another_node_goes_through_the_router() {
    let h = harness(AgentHostConfig::default());
    let remote = AgentId::generate();
    let router = Arc::new(RecordingRouter {
        remote,
        sent: parking_lot::Mutex::new(Vec::new()),
    });
    h.host.set_remote_router(router.clone());

    h.hooks
        .deliver_message_in_thread(
            AgentMessage::new(&remote.to_hex(), "please review the parser"),
            Some("Parser review"),
        )
        .await
        .expect("routed");
    let sent = router.sent.lock().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from, h.root);
    assert_eq!(sent[0].to, remote);
    assert_eq!(sent[0].kind, MessageKind::Request);
    assert_eq!(sent[0].subject, "Parser review");

    let listed = h.hooks.list_agents(None).await.expect("list");
    assert!(listed.to_string().contains("cloud-scout"), "{listed}");
    let state = h.hooks.snapshot(&remote.to_hex()).await.expect("state");
    assert_eq!(state["name"], "cloud-scout", "{state}");
    assert_eq!(state["node"], "aura-x", "{state}");
}

#[tokio::test]
async fn mail_from_another_node_reaches_a_root_in_its_thread() {
    let mut h = harness(AgentHostConfig::default());
    let remote = AgentId::generate();
    h.host
        .deliver_remote(RemoteMail {
            from: remote,
            from_name: "cloud-scout".into(),
            to: h.root,
            kind: MessageKind::Reply,
            thread: ThreadId("aura-x:t2".into()),
            subject: "Parser review".into(),
            text: "Found two issues".into(),
        })
        .expect("delivered");
    let message = next_message(&mut h.inbox).await;
    assert_eq!(message.from, remote);
    assert_eq!(message.from_name, "cloud-scout");
    assert_eq!(message.kind, MessageKind::Reply);
    assert_eq!(message.thread, ThreadId("aura-x:t2".into()));
}

#[tokio::test]
async fn an_agent_no_node_holds_is_still_unknown() {
    let h = harness(AgentHostConfig::default());
    h.host.set_remote_router(Arc::new(RecordingRouter {
        remote: AgentId::generate(),
        sent: parking_lot::Mutex::new(Vec::new()),
    }));
    let err = h
        .hooks
        .deliver_message(AgentMessage::new(&AgentId::generate().to_hex(), "hi"))
        .await
        .expect_err("unknown");
    assert!(err.contains("unknown agent"), "{err}");
}

/// A router holding channel peers the hub names `CODE (name)`, as a
/// cloud member's directory lists them.
struct ChannelRouter {
    peers: Vec<(AgentId, &'static str)>,
    sent: parking_lot::Mutex<Vec<RemoteMail>>,
}

impl ChannelRouter {
    fn new(peers: Vec<(AgentId, &'static str)>) -> Arc<Self> {
        Arc::new(Self {
            peers,
            sent: parking_lot::Mutex::new(Vec::new()),
        })
    }
}

impl RemoteRouter for ChannelRouter {
    fn name_of(&self, agent: AgentId) -> Option<String> {
        self.peers
            .iter()
            .find(|(id, _)| *id == agent)
            .map(|(_, name)| (*name).to_string())
    }

    fn route(&self, mail: RemoteMail) -> Result<(), String> {
        self.sent.lock().push(mail);
        Ok(())
    }

    fn peers(&self, _root: AgentId) -> Vec<serde_json::Value> {
        self.peers
            .iter()
            .map(|(id, name)| {
                serde_json::json!({"agent_id": id.to_hex(), "name": name, "node": "hub", "remote": true})
            })
            .collect()
    }
}

#[tokio::test]
async fn a_cloud_member_mails_a_channel_peer_by_its_code() {
    let h = harness(AgentHostConfig::default());
    let (peer, coordinator) = (AgentId::generate(), AgentId::generate());
    let router = ChannelRouter::new(vec![
        (peer, "RUS198 (rust-engineer #198)"),
        (coordinator, "CO2 (coordinator #2)"),
    ]);
    h.host.set_remote_router(router.clone());

    for target in ["RUS198", "rus198", "@RUS198"] {
        h.hooks
            .deliver_message_in_thread(
                AgentMessage::new(target, "the tasks drawer?"),
                Some("Drawer"),
            )
            .await
            .unwrap_or_else(|e| panic!("{target}: {e}"));
    }
    let sent = router.sent.lock().clone();
    assert_eq!(sent.len(), 3);
    assert!(sent
        .iter()
        .all(|mail| mail.to == peer && mail.from == h.root));

    let state = h.hooks.snapshot("rus198").await.expect("state by code");
    assert_eq!(state["agent_id"], peer.to_hex(), "{state}");
}

#[tokio::test]
async fn an_unknown_code_lists_the_agents_a_cloud_member_reaches() {
    let h = harness(AgentHostConfig::default());
    let router = ChannelRouter::new(vec![
        (AgentId::generate(), "RUS198 (rust-engineer #198)"),
        (AgentId::generate(), "CO2 (coordinator #2)"),
    ]);
    h.host.set_remote_router(router.clone());
    let err = h
        .hooks
        .deliver_message(AgentMessage::new("RUS999", "hi"))
        .await
        .expect_err("unknown");
    assert!(err.contains("unknown agent 'RUS999'"), "{err}");
    assert!(err.contains("CO2") && err.contains("RUS198"), "{err}");
    assert!(router.sent.lock().is_empty());
}

#[tokio::test]
async fn a_code_two_peers_share_is_refused_as_ambiguous() {
    let h = harness(AgentHostConfig::default());
    let router = ChannelRouter::new(vec![
        (AgentId::generate(), "DUP1 (rust-engineer #1)"),
        (AgentId::generate(), "DUP1 (reviewer #1)"),
    ]);
    h.host.set_remote_router(router.clone());
    let err = h
        .hooks
        .deliver_message(AgentMessage::new("dup1", "hi"))
        .await
        .expect_err("ambiguous");
    assert!(err.contains("matches several agents"), "{err}");
    assert!(router.sent.lock().is_empty());
}
