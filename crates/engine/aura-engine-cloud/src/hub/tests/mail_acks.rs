//! Mail the hub sends on to another machine: a refusal there comes back
//! as an ack, and the request's sender is told, as a refusal here is.

use super::mail::Desk;
use super::*;

/// A hub where `member` on `aura-a` mails `other` on `aura-b`, whose link
/// is `frames`; the desk keeps what its senders are told.
struct Forwarding {
    hub: Arc<CloudHub>,
    desk: Arc<Desk>,
    frames: mpsc::UnboundedReceiver<MeshFrame>,
    member: AgentId,
    other: AgentId,
    _dir: tempfile::TempDir,
}

fn forwarding() -> Forwarding {
    let (hub, _fake, dir) = hub();
    let (member, other) = (AgentId::new([2; 32]), AgentId::new([3; 32]));
    place_cloud(&hub, member, "aura-a", &Owner::Local);
    place_cloud(&hub, other, "aura-b", &Owner::Local);
    let desk = Arc::new(Desk::default());
    hub.attach_desk(Arc::clone(&desk) as Arc<dyn MailDesk>);
    let (tx, frames) = mpsc::unbounded_channel();
    hub.links.lock().insert(
        other.to_hex(),
        Link {
            frames: tx,
            stop: CancellationToken::new(),
        },
    );
    Forwarding {
        hub,
        desk,
        frames,
        member,
        other,
        _dir: dir,
    }
}

#[tokio::test]
async fn a_request_the_other_machine_refuses_is_told_to_its_sender() {
    let mut f = forwarding();
    let mail = mail_to(f.member, "aura-a", f.other, 7);
    assert_eq!(ack_error(f.hub.take_mail(&mail).await), None, "sent on");
    assert!(matches!(f.frames.try_recv(), Ok(MeshFrame::Mail(sent)) if sent.id == 7));
    assert!(f.desk.refused.lock().is_empty(), "nothing to tell yet");

    let why = format!("agent {} is sealed on this node", f.other.to_hex());
    f.hub.mail_acked("aura-a", 7, false, Some(&why));
    let refused = f.desk.refused.lock().clone();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!((refused[0].0.from, refused[0].0.to), (f.member, f.other));
    assert_eq!(refused[0].1, why);

    // The same ack again, or one for mail never sent on, tells no one.
    f.hub.mail_acked("aura-a", 7, false, Some(&why));
    f.hub.mail_acked("aura-a", 99, false, None);
    assert_eq!(f.desk.refused.lock().len(), 1);
}

#[tokio::test]
async fn a_request_the_other_machine_took_tells_no_one() {
    let mut f = forwarding();
    let mail = mail_to(f.member, "aura-a", f.other, 8);
    f.hub.take_mail(&mail).await;
    let _ = f.frames.try_recv();
    f.hub.mail_acked("aura-a", 8, true, None);
    f.hub.mail_acked("aura-a", 8, false, Some("late"));
    assert!(f.desk.refused.lock().is_empty());
}

#[tokio::test]
async fn a_refusal_with_no_reason_still_says_its_machine_refused_it() {
    let mut f = forwarding();
    let mut reply = mail_to(f.member, "aura-a", f.other, 9);
    reply.kind = MeshMailKind::Reply;
    f.hub.take_mail(&reply).await;
    let mail = mail_to(f.member, "aura-a", f.other, 10);
    f.hub.take_mail(&mail).await;
    while f.frames.try_recv().is_ok() {}
    f.hub.mail_acked("aura-a", 9, false, None);
    f.hub.mail_acked("aura-a", 10, false, None);
    let refused = f.desk.refused.lock().clone();
    assert_eq!(refused.len(), 1, "a reply is never told: {refused:?}");
    assert_eq!(refused[0].1, "its machine refused it");
}

#[test]
fn saved_placements_name_cloud_agents_without_a_hub() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (placed, child) = (AgentId::new([4; 32]), AgentId::new([5; 32]));
    let saved = serde_json::json!({
        "agents": {
            placed.to_hex(): {
                "zns": "0://scout",
                "name": "scout",
                "project": "p1",
                "project_dir": "/tmp/p1",
                "machine": "aura-sc01-0101",
                "children": [child.to_hex(), "not-an-id"],
            }
        }
    });
    std::fs::write(dir.path().join("placements.json"), saved.to_string()).expect("write");
    let agents = saved_cloud_agents(dir.path());
    assert_eq!(agents, [placed, child].into_iter().collect());
    let saved = saved_placements(dir.path());
    assert_eq!(
        saved[&child].root, placed,
        "a spawn runs on its placer's machine"
    );
    assert_eq!(saved[&placed].root, placed);
    assert_eq!(
        saved[&child].owner,
        Owner::Local,
        "files from before owners"
    );
    let empty = tempfile::tempdir().expect("tempdir");
    assert!(saved_cloud_agents(empty.path()).is_empty());
}
