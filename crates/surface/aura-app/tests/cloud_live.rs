//! End-to-end check of cloud agents against the real boxd API. Ignored
//! by default; it builds snapshots and runs a machine, which costs a few
//! cents. Run with
//!
//! ```sh
//! AURA_CLOUD_TEST_BINARY=/path/to/linux/aura \
//!   cargo test -p aura-app --test cloud_live -- --ignored --nocapture
//! ```
//!
//! `BOXD_API_KEY` must be set (a `.env` works). The binary goes into the
//! base snapshot; it must run on the machines' Ubuntu (glibc 2.39).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use aura_app::cloud::{CloudHub, PatchOutcome, PlaceRequest};
use aura_core_types::{AgentId, RecordEntry, Transaction};
use aura_engine_cloud::snapshots::AuraBinary;
use aura_store_db::{RocksStore, Store};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

fn project(root: &Path) -> PathBuf {
    let dir = root.join("hello-cloud");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("README.md"), "# hello cloud\n").expect("write");
    std::fs::write(dir.join(".env"), "SECRET=never-uploaded\n").expect("write");
    std::fs::write(dir.join(".gitignore"), ".env\n").expect("write");
    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "-A"]);
    git(
        &dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    // Uncommitted work the machine must receive as a delta.
    std::fs::write(dir.join("notes.txt"), "made locally\n").expect("write");
    dir
}

/// Where the live tests keep their cloud state (install id, snapshot
/// index): `AURA_CLOUD_TEST_HOME`, else `aura_data/cloud-live` in this
/// tree. Keeping it lets later runs reuse the snapshots.
fn test_home() -> PathBuf {
    std::env::var("AURA_CLOUD_TEST_HOME").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../aura_data/cloud-live"),
        PathBuf::from,
    )
}

#[tokio::test]
#[ignore = "builds boxd snapshots and runs a machine; needs BOXD_API_KEY"]
async fn an_agent_moves_to_the_cloud_works_there_and_comes_back() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,aura_engine_cloud=debug,aura_app=debug")
        .try_init();
    let _ = aura_config::env_file::load();
    let config = aura_config::CloudConfig::from_env().expect("cloud settings");
    assert!(
        config.is_configured(),
        "{}",
        aura_config::MISSING_CREDENTIAL_NOTICE
    );
    let binary =
        PathBuf::from(std::env::var("AURA_CLOUD_TEST_BINARY").expect("set AURA_CLOUD_TEST_BINARY"));
    assert!(binary.is_file(), "{}", binary.display());

    // Kept between runs so snapshots are reused.
    let home = test_home();
    std::fs::create_dir_all(&home).expect("home");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = project(scratch.path());
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(scratch.path().join("db"), false).expect("store"));

    let agent = AgentId::generate();
    let entry =
        RecordEntry::builder(1, Transaction::user_prompt(agent, "hello from local")).build();
    store
        .append_entry_direct(agent, 1, &entry)
        .expect("seed record");

    let provider = Arc::new(aura_engine_cloud::BoxdProvider::from_config(&config).expect("boxd"));
    let hub = CloudHub::new(
        config,
        Arc::clone(&store),
        home,
        provider,
        AuraBinary::Upload(binary),
    )
    .expect("hub");

    let started = Instant::now();
    let placed = hub
        .place(PlaceRequest {
            agent,
            agent_zns: format!("0://test/cloud-live/{}", &agent.to_hex()[..8]),
            name: "cloud live".into(),
            code: Some("CL1".into()),
            project_id: "cloud-live-hello".into(),
            project_dir: dir.clone(),
            build_command: None,
            fork_from: None,
            owner: aura_app::cloud::hub::Owner::Local,
            owner_name: None,
        })
        .await;
    let view = match placed {
        Ok(view) => view,
        Err(err) => panic!("place failed after {:?}: {err}", started.elapsed()),
    };
    println!(
        "placed on {} ({}) in {:?}",
        view.machine.name,
        view.machine.url(),
        started.elapsed()
    );

    let result = async {
        let out = hub
            .exec(
                agent,
                "ls -a; test ! -e .env && echo no-env-uploaded; cat notes.txt; \
                 echo 'made in the cloud' > cloud.txt; git status --short",
            )
            .await?;
        println!(
            "exec {}:\n{}{}",
            out.exit_code,
            out.stdout_text(),
            out.stderr_tail(500)
        );
        assert!(
            out.stdout_text().contains("no-env-uploaded"),
            "the .env stayed local"
        );
        assert!(
            out.stdout_text().contains("made locally"),
            "the delta arrived"
        );
        let mirrored = hub.mirror(agent).await?;
        println!(
            "mirrored {mirrored} entries; local head {}",
            store.get_head_seq(agent).unwrap_or(0)
        );
        let outcome = hub.recall(agent, "live test").await?;
        println!("recall: {}", outcome.describe());
        Ok::<_, aura_engine_cloud::CloudError>(outcome)
    }
    .await;
    if result.is_err() && hub.is_placed(agent) {
        let _ = hub
            .fleet()
            .destroy(&agent.to_hex(), "live test failed")
            .await;
    }
    let outcome = result.expect("cloud round trip");
    assert!(
        matches!(outcome, PatchOutcome::Applied { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("cloud.txt")).expect("cloud.txt applied"),
        "made in the cloud\n"
    );
    assert!(!hub.is_placed(agent));
    assert!(
        !hub.fleet().has(&agent.to_hex()),
        "the machine is destroyed"
    );
    println!("round trip done in {:?}", started.elapsed());
}

/// A small model per plan, for one cheap turn.
fn plan_model(kind: aura_core_auth::ProviderKind) -> &'static str {
    match kind {
        aura_core_auth::ProviderKind::Claude => "claude/claude-haiku-4-5",
        aura_core_auth::ProviderKind::Chatgpt => "chatgpt/gpt-5-mini",
        _ => "openrouter/openai/gpt-4o-mini",
    }
}

/// One real turn on each plan linked here, run on a cloud machine that
/// holds only boxd placeholders for the tokens. Ignored by default; it
/// needs `BOXD_API_KEY`, at least one plan linked here (`aura login
/// claude`), and `AURA_CLOUD_TEST_BINARY` built from this tree.
#[tokio::test]
#[ignore = "runs real plan turns on a boxd machine; needs BOXD_API_KEY and a linked plan"]
async fn a_cloud_agent_answers_on_each_linked_plan() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,aura_engine_cloud=debug,aura_app=debug")
        .try_init();
    let _ = aura_config::env_file::load();
    let config = aura_config::CloudConfig::from_env().expect("cloud settings");
    assert!(
        config.is_configured(),
        "{}",
        aura_config::MISSING_CREDENTIAL_NOTICE
    );
    let binary =
        PathBuf::from(std::env::var("AURA_CLOUD_TEST_BINARY").expect("set AURA_CLOUD_TEST_BINARY"));
    let auth = Arc::new(aura_auth::AuthManager::new().expect("auth"));
    let owner = auth.owner();
    let linked = auth.linked_plans(&owner);
    if linked.is_empty() {
        println!("no plan linked here; link one with `aura login claude` to run this test");
        return;
    }

    let home = test_home();
    std::fs::create_dir_all(&home).expect("home");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = project(scratch.path());
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(scratch.path().join("db"), false).expect("store"));
    let agent = AgentId::generate();
    let entry =
        RecordEntry::builder(1, Transaction::user_prompt(agent, "hello from local")).build();
    aura_store_db::WriteStore::append_entry_direct(store.as_ref(), agent, 1, &entry)
        .expect("seed record");
    let provider = Arc::new(aura_engine_cloud::BoxdProvider::from_config(&config).expect("boxd"));
    let hub = CloudHub::new(
        config,
        Arc::clone(&store),
        home,
        provider,
        AuraBinary::Upload(binary),
    )
    .expect("hub");
    hub.set_current_owner(owner.clone());
    hub.set_plan_credentials(Arc::new(aura_app::cloud::plans::VaultPlans(Arc::clone(
        &auth,
    ))));
    let view = hub
        .place(PlaceRequest {
            agent,
            agent_zns: format!("0://test/cloud-plans/{}", &agent.to_hex()[..8]),
            name: "cloud plans".into(),
            code: Some("CP1".into()),
            project_id: "cloud-live-hello".into(),
            project_dir: dir,
            build_command: None,
            fork_from: None,
            owner: owner.clone(),
            owner_name: None,
        })
        .await
        .expect("place");
    println!("placed on {}", view.machine.name);

    let result = async {
        // The machine sees only placeholders for its owner's plan secrets.
        let env = hub
            .exec(
                agent,
                "env | grep '^AURA_.*_\\(CLAUDE\\|CHATGPT\\|OPENROUTER\\)=' || true",
            )
            .await?;
        let text = env.stdout_text();
        println!("plan secrets on the machine: {}", text.lines().count());
        for line in text.lines() {
            let value = line.split_once('=').map_or("", |(_, v)| v);
            assert!(
                value.starts_with("bxds_"),
                "a real value reached the machine: {}",
                line.split('=').next().unwrap_or("")
            );
        }
        let mut answers = Vec::new();
        for kind in &linked {
            let mut rx = hub.run_turn(
                agent,
                "Reply with the single word: pong".into(),
                Some(plan_model(*kind).into()),
                None,
                tokio_util::sync::CancellationToken::new(),
            );
            let mut reply = String::new();
            let mut failed = None;
            while let Some(event) = rx.recv().await {
                match event {
                    aura_app::cloud::RemoteEvent::Text(text) => reply.push_str(&text),
                    aura_app::cloud::RemoteEvent::Failed(err) => failed = Some(err),
                    aura_app::cloud::RemoteEvent::Done { .. } => break,
                    _ => {}
                }
            }
            println!("{}: {reply:?} {failed:?}", kind.id());
            answers.push((*kind, reply, failed));
        }
        Ok::<_, aura_engine_cloud::CloudError>(answers)
    }
    .await;
    let _ = hub.fleet().destroy(&agent.to_hex(), "live test done").await;
    for (kind, reply, failed) in result.expect("cloud turns") {
        assert!(failed.is_none(), "{} turn failed: {failed:?}", kind.id());
        assert!(
            reply.to_ascii_lowercase().contains("pong"),
            "{}: {reply:?}",
            kind.id()
        );
    }
}

/// Several agents of one project move to their own machines at once,
/// all restored from the project snapshot; each reports its host, then
/// all come back and their machines are destroyed. Set
/// `AURA_CLOUD_TEST_AGENTS` (default 3, at most 10).
#[tokio::test]
#[ignore = "launches several boxd machines in parallel; needs BOXD_API_KEY"]
async fn several_agents_launch_in_parallel() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,aura_engine_cloud=info,aura_app=info")
        .try_init();
    let _ = aura_config::env_file::load();
    let config = aura_config::CloudConfig::from_env().expect("cloud settings");
    assert!(
        config.is_configured(),
        "{}",
        aura_config::MISSING_CREDENTIAL_NOTICE
    );
    let binary =
        PathBuf::from(std::env::var("AURA_CLOUD_TEST_BINARY").expect("set AURA_CLOUD_TEST_BINARY"));
    let count: usize = std::env::var("AURA_CLOUD_TEST_AGENTS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3)
        .clamp(1, 10);

    let home = test_home();
    std::fs::create_dir_all(&home).expect("home");
    let scratch = tempfile::tempdir().expect("tempdir");
    let dir = project(scratch.path());
    let store: Arc<dyn Store> =
        Arc::new(RocksStore::open(scratch.path().join("db"), false).expect("store"));
    let provider = Arc::new(aura_engine_cloud::BoxdProvider::from_config(&config).expect("boxd"));
    let hub = CloudHub::new(
        config,
        Arc::clone(&store),
        home,
        provider,
        AuraBinary::Upload(binary),
    )
    .expect("hub");

    let agents: Vec<AgentId> = (0..count).map(|_| AgentId::generate()).collect();
    for agent in &agents {
        let entry =
            RecordEntry::builder(1, Transaction::user_prompt(*agent, "hello from local")).build();
        aura_store_db::WriteStore::append_entry_direct(store.as_ref(), *agent, 1, &entry)
            .expect("seed record");
    }
    // Build (or reuse) the project snapshot once, before the race.
    let started = Instant::now();
    let places = agents.iter().enumerate().map(|(i, agent)| {
        let hub = Arc::clone(&hub);
        let dir = dir.clone();
        let agent = *agent;
        async move {
            let t = Instant::now();
            let placed = hub
                .place(PlaceRequest {
                    agent,
                    agent_zns: format!("0://test/cloud-par/{}", &agent.to_hex()[..8]),
                    name: format!("parallel {i}"),
                    code: Some(format!("PAR{i}")),
                    project_id: "cloud-live-hello".into(),
                    project_dir: dir,
                    build_command: None,
                    fork_from: None,
                    owner: aura_app::cloud::hub::Owner::Local,
                    owner_name: None,
                })
                .await;
            (agent, placed.map(|view| (view.machine.name, t.elapsed())))
        }
    });
    let placed = futures_util::future::join_all(places).await;
    println!("{count} placements done in {:?}", started.elapsed());

    let mut failures = Vec::new();
    for (agent, outcome) in &placed {
        match outcome {
            Ok((machine, took)) => {
                let out = hub
                    .exec(*agent, "echo $(hostname) $(nproc)cpu $(free -g | awk '/Mem/{print $2}')GiB; cat notes.txt")
                    .await;
                match out {
                    Ok(out) => println!(
                        "  {machine}: placed in {took:?}; {}",
                        out.stdout_text().replace('\n', " | ")
                    ),
                    Err(err) => failures.push(format!("{machine}: exec failed: {err}")),
                }
            }
            Err(err) => failures.push(format!("place failed: {err}")),
        }
    }
    let listed = hub.fleet().views().len();
    println!("fleet holds {listed} machines");
    if std::env::var("AURA_CLOUD_TEST_KEEP").is_ok() {
        println!("AURA_CLOUD_TEST_KEEP set: leaving the machines running");
        return;
    }
    for (agent, outcome) in &placed {
        if outcome.is_ok() {
            match hub.recall(*agent, "parallel test done").await {
                Ok(done) => println!("  recalled {}: {}", &agent.to_hex()[..8], done.describe()),
                Err(err) => {
                    let _ = hub.fleet().destroy(&agent.to_hex(), "parallel test").await;
                    failures.push(format!("recall failed: {err}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(hub.fleet().views().is_empty(), "every machine destroyed");
}
