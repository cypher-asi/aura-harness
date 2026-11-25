//! Live checks against the real boxd API. Ignored by default; run with
//!
//! ```sh
//! cargo test -p aura-engine-cloud --test live_boxd -- --ignored --nocapture
//! ```
//!
//! with `BOXD_API_KEY` set (a `.env` works). `read_only_calls_succeed`
//! creates nothing and costs nothing.

use aura_engine_cloud::{BoxdProvider, VmProvider};

fn provider() -> BoxdProvider {
    let _ = aura_config::env_file::load();
    let config = aura_config::CloudConfig::from_env().expect("cloud settings");
    assert!(
        config.is_configured(),
        "{}",
        aura_config::MISSING_CREDENTIAL_NOTICE
    );
    BoxdProvider::from_config(&config).expect("provider")
}

#[tokio::test]
#[ignore = "talks to the real boxd API; needs BOXD_API_KEY"]
async fn read_only_calls_succeed() {
    let boxd = provider();
    let limit = boxd.machine_limit().await.expect("machine limit");
    println!("machines: {} of {}", limit.count, limit.max);
    let machines = boxd.list(&[]).await.expect("list machines");
    println!("listed {} machines", machines.len());
    for machine in &machines {
        println!(
            "  {} {} {}",
            machine.name,
            machine.status.as_str(),
            machine.region
        );
    }
    match boxd.balance().await {
        Ok(balance) => println!("balance: {balance:?}"),
        Err(err) => println!("balance unavailable (fine for API keys): {err}"),
    }
    let snapshot = boxd.snapshot("aura-base").await.expect("snapshot lookup");
    println!("aura-base snapshot: {snapshot:?}");
}

/// Creates one small fresh machine, runs a few commands, uploads and
/// reads back a file, then destroys it. Costs a fraction of a cent; the
/// machine also deletes itself after 30 minutes if the test dies.
#[tokio::test]
#[ignore = "creates a real boxd machine; needs BOXD_API_KEY"]
async fn a_machine_runs_commands_and_is_destroyed() {
    use aura_engine_cloud::{MachineOrigin, NewMachine};
    use std::time::Duration;

    let boxd = provider();
    let name = format!(
        "aura-probe-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    );
    let mut spec = NewMachine::named(&name);
    spec.size = aura_config::MachineSize::S1x4;
    spec.delete_after_secs = Some(30 * 60);
    spec.labels.insert("aura.role".into(), "probe".into());
    let started = std::time::Instant::now();
    let machine = boxd
        .create(&spec, &MachineOrigin::Fresh)
        .await
        .expect("create");
    println!(
        "created {} ({}) in {:?}: {:?}",
        machine.name,
        machine.id,
        started.elapsed(),
        machine.status
    );

    let result = async {
        let out = boxd
            .exec(
                &machine.name,
                "cat /etc/os-release | head -2; ldd --version | head -1; uname -m; whoami; \
                 sudo -n true && echo sudo-ok; nproc; free -g | head -2; df -h / | tail -1; \
                 command -v curl git cc || true",
                None,
                Duration::from_secs(120),
            )
            .await?;
        println!(
            "exit {}\n{}\n{}",
            out.exit_code,
            out.stdout_text(),
            out.stderr_tail(2000)
        );
        let written = boxd
            .upload(&machine.name, "/tmp/aura-probe.txt", b"hello boxd".to_vec())
            .await?;
        let back = boxd
            .download(&machine.name, "/tmp/aura-probe.txt", 1024)
            .await?;
        println!(
            "uploaded {written} bytes, read back {:?}",
            String::from_utf8_lossy(&back)
        );
        let stdin = boxd
            .exec(
                &machine.name,
                "cat",
                Some(b"piped stdin".to_vec()),
                Duration::from_secs(30),
            )
            .await?;
        println!("stdin echo: {:?}", stdin.stdout_text());
        let info = boxd.get(&machine.name).await?;
        println!("machine: {info:?}");
        Ok::<_, aura_engine_cloud::CloudError>(())
    }
    .await;
    boxd.destroy(&machine.name).await.expect("destroy");
    let gone = boxd.get(&machine.name).await.expect("get after destroy");
    println!("after destroy: {:?}", gone.map(|m| m.status));
    result.expect("probe commands");
}

/// Which `org` value boxd's `SetVar` accepts for an unattached secret.
#[tokio::test]
#[ignore = "sets and removes a throwaway boxd secret; needs BOXD_API_KEY"]
async fn which_org_value_secrets_take() {
    let boxd = provider();
    let (orgs, default) = boxd.orgs().await.expect("orgs");
    println!("default org id: {default}");
    let mut candidates = vec![String::new()];
    for (id, slug, name) in &orgs {
        println!("org: id={id} slug={slug} name={name}");
        candidates.extend([id.clone(), slug.clone(), name.clone()]);
    }
    let _ = aura_config::env_file::load();
    for org in candidates {
        let mut config = aura_config::CloudConfig::from_env().expect("cloud settings");
        config.org = (!org.is_empty()).then(|| org.clone());
        let probe = BoxdProvider::from_config(&config).expect("provider");
        let secret = aura_engine_cloud::HostSecret {
            name: "AURA_PROBE_SECRET_ORG".into(),
            value: zeroize::Zeroizing::new("unlinked".into()),
            domains: vec!["example.com".into()],
            replace_in: vec!["headers".into()],
        };
        let set = probe.set_secret(&secret).await;
        println!("org {org:?}: set {set:?}");
        if set.is_ok() {
            println!("  remove {:?}", probe.remove_secret(&secret.name).await);
        }
    }
}
