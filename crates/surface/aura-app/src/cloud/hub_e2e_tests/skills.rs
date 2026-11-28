//! An agent's skills on its cloud node: sent as folders before a turn,
//! only when they changed, so the node serves `load_skill` and the turn's
//! prompt carries none of their text; a node that can't take them gets
//! the text as before.

use super::*;
use aura_core_protocol::cloud::{SkillBundle, SkillFile, SkillFolder};
use aura_engine_cloud::hub::{SkillsDelivery, TurnAsk, TurnSkills};

/// The turn's prompt without skills, and with them.
const PROMPT: &str = "Be careful.";
const FULL: &str = "Be careful.\n\n<agent_skills>Tag, build, publish.</agent_skills>";

/// A bundle with a `deploy` skill whose instructions are `body`, and a
/// reference file.
fn bundle(body: &str) -> SkillBundle {
    SkillBundle::new(vec![SkillFolder {
        name: "deploy".into(),
        files: vec![
            SkillFile {
                path: "SKILL.md".into(),
                text: format!("---\nname: deploy\ndescription: Ship a release.\n---\n{body}\n"),
            },
            SkillFile {
                path: "references/api.md".into(),
                text: "GET /v1/releases".into(),
            },
        ],
    }])
}

fn ask(bundle: SkillBundle) -> TurnAsk {
    TurnAsk {
        prompt: "ship it".into(),
        model: Some("mock-model".into()),
        system_prompt: Some(PROMPT.into()),
        skills: Some(TurnSkills {
            bundle,
            full_system_prompt: FULL.into(),
        }),
        ..TurnAsk::default()
    }
}

/// A placed agent on a node that takes skills.
async fn skilled() -> Placed {
    let node_dir = tempfile::tempdir().expect("tempdir");
    let node = TestNode::serve_skilled(node_dir.path(), "done").await;
    let p = placed_on(
        CloudConfig::defaults(),
        TestNode::exec_handler,
        (node, node_dir),
    )
    .await;
    p.import().await;
    p
}

/// What `load_skill` on `p`'s node returns for `deploy`.
fn node_load(p: &Placed, file: Option<&str>) -> String {
    let skills = p.node.skills.as_ref().expect("a skilled node");
    let manager = skills.read().unwrap();
    manager
        .load_agent_skill(&p.agent.to_hex(), "deploy", file)
        .unwrap()
}

/// Deliver `ask`'s skills to `p`'s node; how it went and the prompt left.
async fn deliver(p: &Placed, mut ask: TurnAsk) -> (SkillsDelivery, Option<String>) {
    let node = hooks::node(&p.hub, &p.agent.to_hex()).expect("node");
    let delivery = p.hub.deliver_skills(&node, p.agent, &mut ask).await;
    (delivery, ask.system_prompt)
}

#[tokio::test]
async fn a_turn_sends_only_the_list_and_the_node_loads_the_body_and_a_reference() {
    let p = skilled().await;
    let (delivery, prompt) = deliver(&p, ask(bundle("Tag, build, publish."))).await;
    assert_eq!(delivery, SkillsDelivery::Sent);
    assert_eq!(prompt.as_deref(), Some(PROMPT), "no skill text in the turn");
    let whole = node_load(&p, None);
    assert!(whole.contains("Tag, build, publish."), "{whole}");
    assert!(
        whole.contains("references/api.md"),
        "it lists its references"
    );
    assert_eq!(node_load(&p, Some("references/api.md")), "GET /v1/releases");
}

#[tokio::test]
async fn unchanged_skills_are_not_sent_again_and_a_changed_file_is() {
    let p = skilled().await;
    assert_eq!(deliver(&p, ask(bundle("v1"))).await.0, SkillsDelivery::Sent);
    let (again, prompt) = deliver(&p, ask(bundle("v1"))).await;
    assert_eq!(again, SkillsDelivery::Current);
    assert_eq!(prompt.as_deref(), Some(PROMPT));
    assert_eq!(deliver(&p, ask(bundle("v2"))).await.0, SkillsDelivery::Sent);
    assert!(
        node_load(&p, None).contains("v2"),
        "the node has the new text"
    );
}

#[tokio::test]
async fn a_node_that_takes_no_skills_gets_their_text_in_the_turn() {
    let p = placed(CloudConfig::defaults()).await;
    let (delivery, prompt) = deliver(&p, ask(bundle("Tag, build, publish."))).await;
    assert_eq!(delivery, SkillsDelivery::Unsupported);
    assert_eq!(prompt.as_deref(), Some(FULL));
    // And again next turn: nothing was recorded as sent.
    assert_eq!(
        deliver(&p, ask(bundle("Tag, build, publish."))).await.0,
        SkillsDelivery::Unsupported
    );
}

#[tokio::test]
async fn a_skill_with_a_path_outside_its_folder_is_refused() {
    let p = skilled().await;
    let bad = SkillBundle::new(vec![SkillFolder {
        name: "deploy".into(),
        files: vec![SkillFile {
            path: "../../escape.md".into(),
            text: "x".into(),
        }],
    }]);
    let (delivery, prompt) = deliver(&p, ask(bad)).await;
    assert_eq!(
        delivery,
        SkillsDelivery::Unsupported,
        "the node answers 400"
    );
    assert_eq!(prompt.as_deref(), Some(FULL));
    assert!(!p.node_dir.path().join("escape.md").exists());
}

#[tokio::test]
async fn a_turn_with_skills_runs_to_its_end() {
    let p = skilled().await;
    let mut events = p.hub.run_turn(
        p.agent,
        ask(bundle("Tag, build, publish.")),
        CancellationToken::new(),
    );
    let end = tokio::time::timeout(Duration::from_secs(30), async {
        let mut last = None;
        while let Some(event) = events.recv().await {
            if matches!(event, RemoteEvent::Done(_) | RemoteEvent::Failed(_)) {
                last = Some(event);
            }
        }
        last
    })
    .await
    .expect("the turn ends");
    assert!(matches!(end, Some(RemoteEvent::Done(_))), "{end:?}");
    assert!(node_load(&p, None).contains("Tag, build, publish."));
}
