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
