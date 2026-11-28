//! A new machine never gets the hub's uncommitted work: a ship there
//! commits every change it finds, and would push that work too.

use super::*;

/// The last `changes` fact among `events`.
fn changes_fact(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<crate::boot::BootEvent>,
) -> Option<String> {
    let mut said = None;
    while let Ok(event) = events.try_recv() {
        if let crate::boot::BootEvent::Fact { key, value } = event {
            if key == "changes" {
                said = Some(value);
            }
        }
    }
    said
}

/// Clone `project` as the machine's workspace under `root`, as its
/// snapshot left it, and answer the fake's execs there.
fn machine_from(fake: &FakeProvider, root: &Path, project: &Path) -> PathBuf {
    std::fs::create_dir_all(root).expect("mkdir");
    let machine = root.join("workspace");
    test_git(
        root,
        &[
            "clone",
            "-q",
            &project.display().to_string(),
            &machine.display().to_string(),
        ],
    );
    run_scripts_in(fake, root, &machine);
    machine
}

#[tokio::test]
async fn a_new_machine_starts_from_commits_only_and_has_nothing_to_ship() {
    let (hub, fake, dir) = hub();
    let project = dir.path().join("project");
    project_repo(&project, None);
    let head = test_git(&project, &["rev-parse", "HEAD"]);
    // The hub's unfinished work: a changed file and a new one.
    std::fs::write(project.join("a.txt"), "half done\n").expect("write");
    std::fs::write(project.join("wip.txt"), "not ready\n").expect("write");

    // The machine's workspace, as its snapshot left it.
    let machine = machine_from(&fake, &dir.path().join("vm"), &project);
    let agent = AgentId::new([1; 32]);
    adopt_machine(&hub, &fake, agent, &project, None).await;
    let view = hub.fleet.view(&agent.to_hex()).expect("view");

    let (boot, mut events) = BootReporter::channel();
    hub.sync_files(
        &request_in(&project),
        &view,
        &head,
        "aura/scout-sc01",
        None,
        &boot,
    )
    .await
    .expect("sync");
    drop(boot);

    assert!(
        !machine.join("wip.txt").exists(),
        "the hub's new file stayed home"
    );
    // a.txt as committed ("first"), not the hub's half-done edit.
    assert_eq!(
        std::fs::read_to_string(machine.join("a.txt")).expect("read"),
        "first"
    );
    assert_eq!(
        test_git(&machine, &["status", "--porcelain"]),
        "",
        "nothing to ship"
    );
    assert_eq!(test_git(&machine, &["rev-parse", "HEAD"]), head);
    let said = changes_fact(&mut events).expect("a changes fact");
    assert!(
        said.contains("2 uncommitted file(s) stay on the hub"),
        "{said}"
    );
}
