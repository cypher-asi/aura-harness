//! The prompt-cache scope of a comparison's arms: every request one arm's
//! runs send, Gate's calls on them and their subtasks' included, carries
//! the arm's salt, which no other arm and no ordinary session shares. The
//! classifier's call, made once for both arms, is no arm's.

use super::*;
use crate::agent_registry::RegisteredAgent;
use std::collections::BTreeSet;

/// A check only Opus's answer passes, run after each run's turn.
const OPUS_CHECK: &str = "sh -c 'grep -q opus answer.txt'";

/// Run a comparison of Gate (its lead writing itself) against Opus on
/// "write the answer", `repeats` times per arm, checked by [`OPUS_CHECK`].
async fn compared(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
    repeats: u32,
) -> ComparePayload {
    let args = format!("write the answer --repeats {repeats} --check \"{OPUS_CHECK}\"");
    let payload = run_comparison(state, links, commands, &args).await;
    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    payload
}

/// The one cache salt every request in `sent` carries; panics, naming
/// `who`, when there are none, one has no salt, or they differ.
fn one_salt(sent: &[Sent], who: &str) -> String {
    assert!(!sent.is_empty(), "{who} sent no requests");
    let salts: BTreeSet<Option<&str>> = sent.iter().map(|s| s.cache_salt.as_deref()).collect();
    match salts.into_iter().collect::<Vec<_>>()[..] {
        [Some(salt)] => salt.to_string(),
        ref other => panic!("{who}'s requests carry salts {other:?}: {sent:#?}"),
    }
}

/// The salt of each arm's runs in `payload`, after checking that every
/// request of every run carries its arm's.
fn arm_salts(fixture: &Fixture, payload: &ComparePayload) -> [String; 2] {
    let mut salts: [BTreeSet<String>; 2] = Default::default();
    for run in &payload.runs {
        assert_eq!(run.error, None, "{}", run.name);
        let salt = one_salt(&fixture.sent_by(run.agent_id), &run.name);
        salts[run.arm.index()].insert(salt);
    }
    salts.map(|arm| {
        assert_eq!(arm.len(), 1, "the runs of one arm share a salt: {arm:?}");
        arm.into_iter().next().unwrap()
    })
}

#[tokio::test]
async fn every_request_of_an_arm_carries_its_own_cache_salt() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    let payload = compared(&mut state, &links, &mut commands, 2).await;

    let arms = |arm: CompareArm| payload.runs.iter().filter(move |r| r.arm == arm);
    assert_eq!(arms(CompareArm::A).count(), 2);
    assert_eq!(arms(CompareArm::B).count(), 2);
    let [a, b] = arm_salts(&fixture, &payload);
    assert_ne!(a, b, "the arms never share a cache scope");
    for run in &payload.runs {
        let systems: Vec<String> = (fixture.sent_by(run.agent_id).into_iter())
            .map(|sent| sent.system)
            .collect();
        if run.arm == CompareArm::A {
            assert!(
                systems
                    .iter()
                    .any(|s| s == aura_gate::judge::turn::UNVERIFIED_SYSTEM_PROMPT),
                "{}: so is Gate's judge of its unverified turn: {systems:#?}",
                run.name
            );
        }
    }
    let classified: Vec<AgentId> = (fixture.sent.lock().unwrap().iter())
        .filter(|sent| sent.system == aura_gate::judge::classify::SYSTEM_PROMPT)
        .map(|sent| sent.agent)
        .collect();
    assert_eq!(
        classified.len(),
        1,
        "the prompt is classified once for both arms"
    );
    assert!(
        payload.runs.iter().all(|run| run.agent_id != classified[0]),
        "on the comparison's own agent, in neither arm's cache scope or costs"
    );
}

/// Dispatch a reviewer asked `prompt` from `chain[0]`, whose ancestors
/// are the rest of `chain`, as the `task` tool does; returns the
/// reviewer's id.
pub(super) async fn subtask(fixture: &Fixture, chain: &[AgentId], prompt: &str) -> AgentId {
    let request: aura_core_types::SubagentDispatchRequest =
        serde_json::from_value(serde_json::json!({
            "parent_agent_id": chain[0],
            "parent_chain": chain,
            "subagent_type": "reviewer",
            "prompt": prompt,
            "parent_permissions": aura_core_types::AgentPermissions::full_access(),
            "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
        }))
        .unwrap();
    (fixture.dispatcher.dispatch(request).await.unwrap())
        .child_agent_id
        .unwrap()
}

#[tokio::test]
async fn a_subtask_of_a_compare_run_inherits_its_arms_cache_salt() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let payload = compared(&mut state, &links, &mut commands, 1).await;
    let salts = arm_salts(&fixture, &payload);

    for run in &payload.runs {
        let salt = &salts[run.arm.index()];
        let child = subtask(&fixture, &[run.agent_id], "what changed?").await;
        let name = format!("{}'s subtask", run.name);
        assert_eq!(&one_salt(&fixture.sent_by(child), &name), salt);
        assert_eq!(
            fixture.hub.factory.cache_salt_of(child).as_ref(),
            Some(salt),
            "{name} keeps it when reopened from its record"
        );
        let nested = subtask(&fixture, &[child, run.agent_id], "what changed?").await;
        let name = format!("{}'s nested subtask", run.name);
        assert_eq!(&one_salt(&fixture.sent_by(nested), &name), salt);
    }
}

/// A new agent in `fixture`'s registry.
pub(super) fn new_agent(fixture: &Fixture, name: &str) -> RegisteredAgent {
    subagents::lock(&fixture.hub.registry)
        .create(Some(name.to_string()))
        .unwrap()
}

#[tokio::test]
async fn kernel_factory_open_takes_the_cache_salt_of_the_nearest_salted_ancestor() {
    let (fixture, _commands) = fixture();
    let factory = &fixture.hub.factory;
    let [root, mid, child, own, plain] =
        ["root", "mid", "child", "own", "plain"].map(|name| new_agent(&fixture, name));
    factory.scope_cache(root.agent_id(), "5a175a175a175a17".to_string());
    let salt_of = |agent: &RegisteredAgent, chain: Vec<AgentId>| {
        let session = factory.open(agent, chain, None).unwrap();
        session.kernel.cache_salt().map(str::to_string)
    };

    let chain = vec![mid.agent_id(), root.agent_id()];
    assert_eq!(
        salt_of(&child, chain.clone()).as_deref(),
        Some("5a175a175a175a17"),
        "an unsalted parent passes its own ancestor's salt on"
    );
    assert_eq!(
        salt_of(&child, Vec::new()).as_deref(),
        Some("5a175a175a175a17"),
        "reopened without its chain, the agent keeps the salt"
    );
    factory.scope_cache(own.agent_id(), "0000111122223333".to_string());
    assert_eq!(
        salt_of(&own, chain).as_deref(),
        Some("0000111122223333"),
        "an agent's own salt wins over its ancestors'"
    );
    assert_eq!(salt_of(&plain, vec![mid.agent_id()]), None);
    assert_eq!(factory.cache_salt_of(plain.agent_id()), None);
    assert_eq!(factory.cache_salt_of(mid.agent_id()), None);
}

#[tokio::test]
async fn requests_outside_a_compare_carry_no_cache_salt() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    turn(&mut state, &mut done, &links, first, "what does gate do?").await;
    let child = subtask(&fixture, &[first], "what changed?").await;

    for (agent, who) in [(first, "the session"), (child, "its subtask")] {
        let sent = fixture.sent_by(agent);
        assert!(!sent.is_empty(), "{who} sent no requests");
        assert!(
            sent.iter().all(|s| s.cache_salt.is_none()),
            "{who}: {sent:#?}"
        );
    }
    assert!(
        (fixture.sent_by(first).iter())
            .any(|s| s.system == aura_gate::judge::classify::SYSTEM_PROMPT),
        "Gate's classifier call goes out unsalted too"
    );
}
