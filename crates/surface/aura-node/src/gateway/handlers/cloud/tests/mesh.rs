//! Mesh mail from peers, and how it maps to the host's mail.

use super::super::remote_mail;
use super::support::{call, claim, cloud_state};
use aura_core_protocol::cloud::{MeshMail, MeshMailKind};
use aura_core_types::AgentId;
use aura_engine::agents::{MessageKind, ThreadId};
use axum::http::StatusCode;

#[tokio::test]
async fn mail_from_a_peer_reaches_nobody_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = cloud_state(dir.path());
    claim(&state);
    let mail = serde_json::json!({
        "id": 1, "from_node": "aura-ex2-1111", "thread": "aura-ex2-1111:t1", "subject": "s",
        "from": AgentId::new([1; 32]).to_hex(), "from_name": "peer", "to": AgentId::new([2; 32]).to_hex(),
        "kind": "request", "text": "hi", "sent_ms": 0
    });
    let (status, body) = call(
        &state,
        "POST",
        "/v1/mesh/mail",
        Some("mesh-token-for-tests-0123"),
        Some(mail),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"], "mail_ack");
    assert_eq!(body["delivered"], false);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown agent"),
        "{body}"
    );
}

#[test]
fn mesh_mail_maps_to_host_mail() {
    let mail = MeshMail {
        id: 3,
        from_node: "hub".into(),
        thread: "hub:t1".into(),
        subject: "Review".into(),
        from: AgentId::new([1; 32]).to_hex(),
        from_name: "lead".into(),
        to: AgentId::new([2; 32]).to_hex(),
        kind: MeshMailKind::Reply,
        text: "done".into(),
        sent_ms: 0,
    };
    let remote = remote_mail(&mail).expect("maps");
    assert_eq!(remote.kind, MessageKind::Reply);
    assert_eq!(remote.thread, ThreadId("hub:t1".into()));
    assert!(remote_mail(&MeshMail {
        from: "zz".into(),
        ..mail
    })
    .is_err());
}
