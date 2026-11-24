//! The scratch folder of a comparison's runs: each run's commands get
//! their own temp, cache and build folders beside its copy, which its
//! subtasks share, no ordinary session has, and dismissing removes.

use super::cache_salt::{new_agent, subtask};
use super::*;
use aura_exec_tools::RunScratch;
use std::collections::BTreeSet;

/// Run a comparison of Gate against Opus on "write the answer",
/// `repeats` times per arm.
async fn compared(
    state: &mut LoopState<'_>,
    links: &mpsc::Sender<auth::LinkOutcome>,
    commands: &mut mpsc::Receiver<UiCommand>,
    repeats: u32,
) -> ComparePayload {
    let args = format!("write the answer --repeats {repeats}");
    let payload = run_comparison(state, links, commands, &args).await;
    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    payload
}

/// The scratch folder of each run of `payload`, after checking it sits
/// in the comparison's folder beside the run's copy, not inside it.
fn run_scratches(fixture: &Fixture, payload: &ComparePayload) -> Vec<(CompareArm, RunScratch)> {
    payload
        .runs
        .iter()
        .map(|run| {
            assert_eq!(run.error, None, "{}", run.name);
            let scratch = (fixture.hub.factory.run_scratch_of(run.agent_id))
                .unwrap_or_else(|| panic!("{} has no scratch folder", run.name));
            let copy = &run.copy;
            assert_eq!(
                scratch.dir().parent(),
                copy.root.parent(),
                "{}: the scratch is in the comparison's folder",
                run.name
            );
            assert!(
                !scratch.dir().starts_with(&copy.root) && !scratch.dir().starts_with(&copy.workdir),
                "{}: {:?} is outside its copy {:?}",
                run.name,
                scratch.dir(),
                copy.root
            );
            assert!(scratch.dir().is_dir(), "{}: it is made", run.name);
            for (var, dir) in scratch.vars() {
                assert!(dir.is_dir(), "{}: {var} names a made folder", run.name);
            }
            (run.arm, scratch)
        })
        .collect()
}

#[tokio::test]
async fn each_compare_run_gets_its_own_scratch_beside_its_copy() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    let payload = compared(&mut state, &links, &mut commands, 2).await;

    let scratches = run_scratches(&fixture, &payload);
    assert_eq!(scratches.len(), 4);
    let dirs: BTreeSet<_> = scratches
        .iter()
        .map(|(_, s)| s.dir().to_path_buf())
        .collect();
    assert_eq!(dirs.len(), 4, "no two runs share a scratch: {dirs:#?}");
    let arm_dirs = |arm: CompareArm| -> BTreeSet<_> {
        (scratches.iter())
            .filter(|(a, _)| *a == arm)
            .map(|(_, s)| s.dir().to_path_buf())
            .collect()
    };
    assert!(
        arm_dirs(CompareArm::A).is_disjoint(&arm_dirs(CompareArm::B)),
        "the arms never share a scratch"
    );
    for (_, scratch) in &scratches {
        assert!(!scratch.dir().starts_with(&fixture.workspace));
    }
}

#[tokio::test]
async fn a_subtask_of_a_compare_run_inherits_its_runs_scratch() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let payload = compared(&mut state, &links, &mut commands, 1).await;
    run_scratches(&fixture, &payload);

    let factory = &fixture.hub.factory;
    for run in &payload.runs {
        let scratch = factory.run_scratch_of(run.agent_id);
        let child = subtask(&fixture, &[run.agent_id], "what changed?").await;
        assert_eq!(
            factory.run_scratch_of(child),
            scratch,
            "{}'s subtask works in its scratch",
            run.name
        );
        let nested = subtask(&fixture, &[child, run.agent_id], "what changed?").await;
        assert_eq!(
            factory.run_scratch_of(nested),
            scratch,
            "{}'s nested subtask too",
            run.name
        );
    }
}

#[tokio::test]
async fn kernel_factory_open_takes_the_scratch_of_the_nearest_ancestor_with_one() {
    let (fixture, _commands) = fixture();
    let factory = &fixture.hub.factory;
    let [root, mid, child, plain] =
        ["root", "mid", "child", "plain"].map(|name| new_agent(&fixture, name));
    let scratch = RunScratch::new(fixture.workspace.join("..").join("root.tmp"));
    factory.give_scratch(root.agent_id(), scratch.clone());
    let scratch_of = |agent: &RegisteredAgent, chain: Vec<AgentId>| {
        let session = factory.open(agent, chain, None).unwrap();
        let check = session.post_edit_check.as_ref().expect("a post-edit check");
        assert_eq!(
            check.run_scratch(),
            session.kernel.run_scratch(),
            "the post-edit check builds where the session's commands do"
        );
        session.kernel.run_scratch().cloned()
    };

    let chain = vec![mid.agent_id(), root.agent_id()];
    assert_eq!(scratch_of(&child, chain).as_ref(), Some(&scratch));
    assert_eq!(
        scratch_of(&child, Vec::new()).as_ref(),
        Some(&scratch),
        "reopened without its chain, the agent keeps the scratch"
    );
    assert_eq!(scratch_of(&plain, vec![mid.agent_id()]), None);
    assert_eq!(factory.run_scratch_of(mid.agent_id()), None);
}

#[tokio::test]
async fn a_session_outside_a_compare_has_no_scratch() {
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

    let factory = &fixture.hub.factory;
    assert_eq!(factory.run_scratch_of(first), None);
    assert_eq!(factory.run_scratch_of(child), None);
    let plain = new_agent(&fixture, "plain");
    let session = factory.open(&plain, vec![first], None).unwrap();
    assert_eq!(session.kernel.run_scratch(), None);
    let check = session.post_edit_check.as_ref().expect("a post-edit check");
    assert_eq!(check.run_scratch(), None);
}

#[tokio::test]
async fn dismissing_a_comparison_removes_its_runs_scratch() {
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let payload = compared(&mut state, &links, &mut commands, 1).await;
    let scratches = run_scratches(&fixture, &payload);
    for (_, scratch) in &scratches {
        std::fs::write(scratch.dir().join("tmp").join("left-behind"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Go leaves its module cache read-only.
            let module = scratch.dir().join("go/mod/example.com/m@v1.0.0");
            std::fs::create_dir_all(&module).unwrap();
            std::fs::write(module.join("go.mod"), "module m\n").unwrap();
            std::fs::set_permissions(&module, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
    }

    compare(&mut state, &links, "dismiss").await;

    let dismissed = system_texts(&drain(&mut commands));
    assert!(dismissed.contains("removed the copies"), "{dismissed}");
    for (_, scratch) in &scratches {
        assert!(!scratch.dir().exists(), "{:?} was removed", scratch.dir());
    }
    for run in &payload.runs {
        assert!(!run.copy.root.exists(), "{}", run.name);
        if let Some(dir) = run.copy.root.parent() {
            assert!(!dir.exists(), "the comparison's folder is gone too");
        }
    }
}
