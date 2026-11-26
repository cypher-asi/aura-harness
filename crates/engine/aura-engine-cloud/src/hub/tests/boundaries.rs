use super::*;
use crate::hub::links::HubRouter;
use crate::hub::turn::chat_request;

#[test]
fn mail_never_crosses_users() {
    let (hub, _fake, _dir) = hub();
    hub.set_current_owner(alice());
    let local = AgentId::new([1; 32]);
    hub.note_local(local, "local", "p1");
    let alice_cloud = AgentId::new([2; 32]);
    let bob_cloud = AgentId::new([3; 32]);
    place_cloud(&hub, alice_cloud, "aura-alice", &alice());
    place_cloud(&hub, bob_cloud, "aura-bob", &bob());

    // Bob's machine can't mail Alice's local agent or her cloud agent.
    let err = ack_error(hub.take_mail(&mail_to(bob_cloud, "aura-bob", local, 1)));
    assert!(err.is_some_and(|e| e.contains("another user")));
    let err = ack_error(hub.take_mail(&mail_to(bob_cloud, "aura-bob", alice_cloud, 2)));
    assert!(err.is_some_and(|e| e.contains("another user")));
    // Alice's machine reaches her local agent (no host here: refused
    // for that reason, not for ownership).
    let err = ack_error(hub.take_mail(&mail_to(alice_cloud, "aura-alice", local, 3)));
    assert!(err.is_some_and(|e| !e.contains("another user")));

    // Local agents act as Alice: Bob's cloud agent is unknown to them.
    let router = HubRouter(Arc::downgrade(&hub));
    assert_eq!(router.name_of(bob_cloud), None);
    assert!(router.name_of(alice_cloud).is_some());
    let err = router
        .route(RemoteMail {
            from: local,
            from_name: "local".into(),
            to: bob_cloud,
            kind: MessageKind::Request,
            thread: ThreadId("t1".into()),
            subject: "s".into(),
            text: "hi".into(),
        })
        .expect_err("cross-user");
    assert!(err.contains("another user"), "{err}");

    // Each node's directory holds only its owner's agents.
    let MeshFrame::Directory { peers } = hub.directory.frame_for("aura-bob", "p1") else {
        panic!("not a directory");
    };
    assert!(peers.is_empty(), "{peers:?}");
}

#[tokio::test]
async fn an_agent_that_was_never_placed_recalls_to_nothing() {
    let (hub, _fake, _dir) = hub();
    let agent = AgentId::new([5; 32]);
    assert!(!hub.is_placed(agent));
    assert_eq!(
        hub.recall(agent, "test", &BootReporter::none())
            .await
            .expect("recall"),
        PullOutcome::patch_only(PatchOutcome::Empty)
    );
    assert_eq!(hub.placement_of(agent), Placement::Local);
}

#[test]
fn patch_outcomes_describe_themselves() {
    assert_eq!(PatchOutcome::Empty.describe(), "no changes to pull");
    assert_eq!(
        PatchOutcome::Applied {
            files: vec!["a".into()]
        }
        .describe(),
        "pulled 1 changed file(s)"
    );
    assert_eq!(
        PullOutcome {
            patch: PatchOutcome::Empty,
            commits: CommitsOutcome::Fetched {
                branch: "aura/scout-a1b2".into(),
                commits: 2,
            },
        }
        .describe(),
        "no changes to pull; 2 commit(s) on branch aura/scout-a1b2"
    );
    assert_eq!(
        PullOutcome::patch_only(PatchOutcome::Empty).describe(),
        "no changes to pull"
    );
}

#[test]
fn cloud_turns_never_offer_computer_use() {
    let request = chat_request(AgentId::new([1; 32]), None, None);
    let json = serde_json::to_value(&request.agent_permissions).expect("json");
    let text = json.to_string();
    assert!(!text.contains("computer"), "{text}");
    assert!(
        text.contains("spawnAgent") || text.contains("spawn_agent"),
        "{text}"
    );
}
