//! Bringing an agent home brings every branch of its, not only the one
//! its machine is pinned to: the task branches `git_start_task` made
//! beside it come back too, and other agents' branches stay out.

use super::*;
use crate::hub::CommitsOutcome;

/// A project and its agent's machine pinned to `aura/first-task-rus207`,
/// with a second task branch of the same agent, one of another agent and
/// one of no agent. Each branch has commits the project lacks.
struct Branches {
    hub: Arc<CloudHub>,
    _dir: tempfile::TempDir,
    project: PathBuf,
    machine: PathBuf,
    agent: AgentId,
    placed: PlacedAgent,
}

const PIN: &str = "aura/first-task-rus207";
const SECOND: &str = "aura/second-task-rus207";
const OTHERS: &str = "aura/their-task-rus204";
const STRAY: &str = "aura/scratch";

async fn branches() -> Branches {
    let (hub, fake, dir) = hub();
    let project = dir.path().join("project");
    project_repo(&project, None);
    let root = dir.path().join("vm");
    std::fs::create_dir_all(&root).expect("mkdir");
    let machine = root.join("workspace");
    test_git(
        dir.path(),
        &[
            "clone",
            "-q",
            &project.display().to_string(),
            &machine.display().to_string(),
        ],
    );
    test_git(&machine, &["update-ref", workspace::HUB_HEAD_REF, "HEAD"]);
    for (branch, file) in [
        (OTHERS, "o.txt"),
        (STRAY, "s.txt"),
        (PIN, "a.txt"),
        (SECOND, "b.txt"),
    ] {
        test_git(
            &machine,
            &["checkout", "-q", "-B", branch, workspace::HUB_HEAD_REF],
        );
        test_commit(&machine, file, branch);
    }
    run_scripts_in(&fake, &root, &machine);
    let agent = AgentId::new([1; 32]);
    let placed = adopt_machine(&hub, &fake, agent, &project, Some(PIN)).await;
    Branches {
        hub,
        _dir: dir,
        project,
        machine,
        agent,
        placed,
    }
}

fn has_branch(project: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "rev-parse",
            "-q",
            "--verify",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(project)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[tokio::test]
async fn an_unshipped_task_branch_comes_home_with_the_pinned_one() {
    let Branches {
        hub,
        _dir,
        project,
        machine,
        agent,
        placed,
    } = branches().await;
    let outcome = hub.fetch_commits(&agent.to_hex(), &placed).await;
    assert_eq!(
        outcome,
        CommitsOutcome::Several(vec![
            CommitsOutcome::Fetched {
                branch: PIN.into(),
                commits: 1
            },
            CommitsOutcome::Fetched {
                branch: SECOND.into(),
                commits: 1
            },
        ])
    );
    for branch in [PIN, SECOND] {
        assert_eq!(
            test_git(&project, &["rev-parse", &format!("refs/heads/{branch}")]),
            test_git(&machine, &["rev-parse", &format!("refs/heads/{branch}")]),
            "{branch} came home whole"
        );
    }
    assert!(
        !has_branch(&project, OTHERS),
        "another agent's branch stays out"
    );
    assert!(
        !has_branch(&project, STRAY),
        "a branch of no agent stays out"
    );
    let said = outcome.describe().expect("a report");
    assert!(said.contains(PIN) && said.contains(SECOND), "{said}");
    assert!(!outcome.kept_any());
}

#[tokio::test]
async fn a_task_branch_brought_home_again_brings_only_its_new_commits() {
    let Branches {
        hub,
        _dir,
        project: _,
        machine,
        agent,
        placed,
    } = branches().await;
    let hex = agent.to_hex();
    hub.fetch_commits(&hex, &placed).await;
    let placed = hub.placed_agent(agent).expect("placed");
    // Only the task branch moved since.
    test_git(&machine, &["checkout", "-q", SECOND]);
    test_commit(&machine, "c.txt", "more on the second task");
    assert_eq!(
        hub.fetch_commits(&hex, &placed).await,
        CommitsOutcome::Fetched {
            branch: SECOND.into(),
            commits: 2,
        },
        "the task branch's commits past the hub's head; the pinned one has nothing new"
    );
}

#[tokio::test]
async fn a_task_branch_checked_out_here_stays_on_the_machine_and_says_so() {
    let Branches {
        hub,
        _dir,
        project,
        machine: _,
        agent,
        placed,
    } = branches().await;
    test_git(&project, &["checkout", "-q", "-b", SECOND]);
    let outcome = hub.fetch_commits(&agent.to_hex(), &placed).await;
    assert!(outcome.kept_any(), "{outcome:?}");
    let said = outcome.describe().expect("a report");
    assert!(
        said.contains(&format!("commits on {SECOND} not fetched")),
        "{said}"
    );
    assert!(
        said.contains(&format!("on branch {PIN}")),
        "the pinned one still came: {said}"
    );
}
