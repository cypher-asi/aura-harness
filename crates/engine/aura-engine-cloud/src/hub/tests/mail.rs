use super::*;

#[test]
fn mail_for_a_local_agent_without_a_host_is_refused_and_resends_are_retried() {
    let (hub, _fake, _dir) = hub();
    place_cloud(&hub, AgentId::new([2; 32]), "aura-x", &Owner::Local);
    let mail = MeshMail {
        id: 1,
        from_node: "aura-x".into(),
        thread: "aura-x:t1".into(),
        subject: "s".into(),
        from: AgentId::new([2; 32]).to_hex(),
        from_name: "scout".into(),
        to: AgentId::new([3; 32]).to_hex(),
        kind: MeshMailKind::Request,
        text: "hi".into(),
        sent_ms: 0,
    };
    let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail) else {
        panic!("not an ack");
    };
    assert!(!delivered);
    let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail) else {
        panic!("not an ack");
    };
    assert!(
        !delivered,
        "a failed delivery is forgotten, so the resend is tried again"
    );
}
