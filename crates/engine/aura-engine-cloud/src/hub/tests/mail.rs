use super::*;
use crate::mesh::HUB_NODE;

#[tokio::test]
async fn mail_for_a_local_agent_without_a_host_is_refused_and_resends_are_retried() {
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
    let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail).await else {
        panic!("not an ack");
    };
    assert!(!delivered);
    let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail).await else {
        panic!("not an ack");
    };
    assert!(
        !delivered,
        "a failed delivery is forgotten, so the resend is tried again"
    );
}

/// A desk that keeps what it is handed, or refuses it all with `refuse`.
#[derive(Default)]
struct Desk {
    filed: Mutex<Vec<RemoteMail>>,
    refuse: Option<String>,
}

#[async_trait::async_trait]
impl MailDesk for Desk {
    async fn file(&self, mail: RemoteMail) -> Result<(), String> {
        if let Some(why) = &self.refuse {
            return Err(why.clone());
        }
        self.filed.lock().push(mail);
        Ok(())
    }
}

fn reach(agent: AgentId, name: &str) -> Reach {
    Reach {
        agent_id: agent,
        name: name.into(),
    }
}

fn peers_of(frame: MeshFrame) -> Vec<aura_core_protocol::cloud::MeshPeer> {
    let MeshFrame::Directory { peers } = frame else {
        panic!("not a directory");
    };
    peers
}

#[test]
fn a_placed_agents_machine_lists_the_agents_it_reaches_as_held_by_the_hub() {
    let (hub, _fake, _dir) = hub();
    let member = AgentId::new([2; 32]);
    let other = AgentId::new([3; 32]);
    let coordinator = AgentId::new([4; 32]);
    place_cloud(&hub, member, "aura-a", &Owner::Local);
    place_cloud(&hub, other, "aura-b", &Owner::Local);
    hub.set_reach(
        member,
        vec![
            reach(coordinator, "COO1 (site coordinator)"),
            reach(other, "REN4 (rust-engineer #4)"),
        ],
    );

    let peers = peers_of(hub.directory_frame("aura-a", "p1"));
    let row = |id: AgentId| {
        let rows: Vec<_> = peers
            .iter()
            .filter(|peer| peer.agent_id == id.to_hex())
            .collect();
        assert_eq!(rows.len(), 1, "{peers:?}");
        rows[0].clone()
    };
    let coordinator_row = row(coordinator);
    assert_eq!(coordinator_row.name, "COO1 (site coordinator)");
    assert_eq!(coordinator_row.node_id, HUB_NODE);
    assert_eq!(coordinator_row.url, None);
    let other_row = row(other);
    assert_eq!(
        (other_row.node_id.as_str(), other_row.url),
        (HUB_NODE, None),
        "its mail goes through the hub, not straight to its machine"
    );

    // A machine whose agent reaches no one lists its project as before.
    assert_eq!(
        hub.directory_frame("aura-b", "p1"),
        hub.directory.frame_for("aura-b", "p1")
    );
}

#[tokio::test]
async fn requests_a_placed_agent_sends_whom_it_reaches_are_filed_at_the_desk() {
    let (hub, _fake, _dir) = hub();
    let member = AgentId::new([2; 32]);
    let coordinator = AgentId::new([4; 32]);
    let stranger = AgentId::new([5; 32]);
    place_cloud(&hub, member, "aura-a", &Owner::Local);
    let desk = Arc::new(Desk::default());
    hub.attach_desk(Arc::clone(&desk) as Arc<dyn MailDesk>);
    hub.set_reach(member, vec![reach(coordinator, "COO1")]);

    let ack = hub
        .take_mail(&mail_to(member, "aura-a", coordinator, 1))
        .await;
    assert_eq!(ack_error(ack), None);
    let filed = desk.filed.lock().clone();
    assert_eq!(filed.len(), 1);
    assert_eq!(
        (filed[0].from, filed[0].to, filed[0].kind),
        (member, coordinator, MessageKind::Request)
    );
    assert_eq!(filed[0].text, "hi");

    // A reply, and mail to an agent it doesn't reach, go on as before:
    // to the host, which this hub lacks.
    let mut reply = mail_to(member, "aura-a", coordinator, 2);
    reply.kind = MeshMailKind::Reply;
    let refused = ack_error(hub.take_mail(&reply).await);
    assert!(refused.is_some_and(|e| e.contains("no agents")));
    let refused = ack_error(hub.take_mail(&mail_to(member, "aura-a", stranger, 3)).await);
    assert!(refused.is_some_and(|e| e.contains("no agents")));
    assert_eq!(desk.filed.lock().len(), 1);
}

#[tokio::test]
async fn a_request_the_desk_refuses_is_acked_undelivered_and_may_come_again() {
    let (hub, _fake, _dir) = hub();
    let member = AgentId::new([2; 32]);
    let coordinator = AgentId::new([4; 32]);
    place_cloud(&hub, member, "aura-a", &Owner::Local);
    hub.attach_desk(Arc::new(Desk {
        refuse: Some("the thread is full".into()),
        ..Desk::default()
    }));
    hub.set_reach(member, vec![reach(coordinator, "COO1")]);

    let mail = mail_to(member, "aura-a", coordinator, 1);
    let refused = ack_error(hub.take_mail(&mail).await);
    assert_eq!(refused.as_deref(), Some("the thread is full"));
    let again = ack_error(hub.take_mail(&mail).await);
    assert_eq!(
        again.as_deref(),
        Some("the thread is full"),
        "forgotten, so a resend reaches the desk again"
    );
}
