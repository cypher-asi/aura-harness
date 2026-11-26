
use super::*;
use crate::fake::FakeProvider;
use crate::fleet::MemoryLedger;

#[test]
fn a_build_step_says_what_it_waits_on() {
    assert_eq!(
            step_waits_on(
                "command -v cloud-init >/dev/null 2>&1 && cloud-init status --wait >/dev/null 2>&1; true",
                "b1"
            ),
            "waiting for b1's first boot (cloud-init)"
        );
    assert_eq!(
        step_waits_on("set -e;\n  cargo   fetch", "b1"),
        "`set -e; cargo fetch` on b1"
    );
    let long = step_waits_on(&"x".repeat(200), "b1");
    assert!(long.contains('…') && long.chars().count() < 100, "{long}");
}

#[test]
fn an_upload_says_how_far_it_has_got() {
    assert_eq!(
        uploading("/tmp/aura", 40 << 20, 140 << 20, "b1"),
        "uploading /tmp/aura (40 MiB of 140 MiB) to b1"
    );
    assert!(uploading("/tmp/aura", 140 << 20, 140 << 20, "b1").contains("sent, waiting"));
}
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git")
        .success());
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.email", "t@t"]);
    git(dir.path(), &["config", "user.name", "t"]);
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname='x'\n").expect("write");
    std::fs::write(dir.path().join("Cargo.lock"), "v1").expect("write");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn builder(fake: &FakeProvider, home: &Path) -> (SnapshotBuilder, Arc<MemoryLedger>) {
    builder_with(fake, home, AuraBinary::Installed)
}

fn builder_with(
    fake: &FakeProvider,
    home: &Path,
    binary: AuraBinary,
) -> (SnapshotBuilder, Arc<MemoryLedger>) {
    let ledger = Arc::new(MemoryLedger::default());
    let builder = SnapshotBuilder::new(
        Arc::new(fake.clone()),
        ledger.clone(),
        "inst-1",
        MachineSize::S4x16,
        true,
        "0.1.0",
        binary,
        home.join("cloud"),
    );
    (builder, ledger)
}

/// A stand-in binary whose string table needs glibc `2.<minor>`.
fn fake_binary(dir: &Path, minor: u32) -> PathBuf {
    let path = dir.join("aura");
    let strtab = format!("\x7fELF\0libc.so.6\0GLIBC_2.2.5\0GLIBC_2.{minor}\0");
    std::fs::write(&path, strtab).expect("write binary");
    path
}

#[tokio::test]
async fn a_binary_needing_a_newer_glibc_is_refused_before_any_machine() {
    let home = tempfile::tempdir().expect("home");
    let binary = fake_binary(home.path(), 43);
    let fake = FakeProvider::new();
    let (builder, _) = builder_with(&fake, home.path(), AuraBinary::Upload(binary.clone()));

    let err = builder.ensure_base().await.expect_err("refused");
    assert!(
        matches!(
            &err,
            CloudError::IncompatibleBinary { needs, has, .. }
                if *needs == GlibcVersion::new(2, 43) && *has == MACHINE_GLIBC
        ),
        "{err:?}"
    );
    let message = err.to_string();
    for part in [
        binary.display().to_string(),
        "glibc 2.43".into(),
        "have 2.39".into(),
        "scripts/build-cloud-binary.sh".into(),
        cloud_binary_path(&home.path().join("cloud"))
            .display()
            .to_string(),
        "AURA_CLOUD_BINARY".into(),
    ] {
        assert!(message.contains(&part), "{part} missing from: {message}");
    }
    assert!(
        fake.calls_matching("Create(").is_empty(),
        "{:?}",
        fake.calls()
    );
}

#[tokio::test]
async fn an_uploaded_binary_is_started_once_installed() {
    let home = tempfile::tempdir().expect("home");
    let binary = fake_binary(home.path(), 34);
    let fake = FakeProvider::new();
    let (builder, _) = builder_with(&fake, home.path(), AuraBinary::Upload(binary));

    builder.ensure_base().await.expect("build");
    let calls = fake.calls();
    let position = |needle: &str| {
        calls
            .iter()
            .position(|call| call.contains(needle))
            .unwrap_or_else(|| panic!("{needle} not run: {calls:?}"))
    };
    assert!(position("install -m 0755 /tmp/aura") < position(CHECK_BINARY));
    assert!(position(CHECK_BINARY) < position("nohup aura run"));
}

#[tokio::test]
async fn a_binary_that_does_not_start_on_the_builder_says_why() {
    let home = tempfile::tempdir().expect("home");
    let binary = fake_binary(home.path(), 34);
    let fake = FakeProvider::new();
    fake.on_exec(Arc::new(|_, command, _| {
        let fails = command == CHECK_BINARY;
        crate::ExecOutput {
            exit_code: i32::from(fails),
            stdout: Vec::new(),
            stderr: if fails {
                b"aura: /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.43' not found".to_vec()
            } else {
                Vec::new()
            },
        }
    }));
    let (builder, ledger) = builder_with(&fake, home.path(), AuraBinary::Upload(binary));

    let err = builder.ensure_base().await.expect_err("check fails");
    assert!(
        matches!(err, CloudError::BinaryDoesNotRun { exit_code: 1, .. }),
        "{err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains("uploaded aura binary does not run on the cloud image"),
        "{message}"
    );
    assert!(message.contains("GLIBC_2.43' not found"), "{message}");
    assert!(fake.machine_names().is_empty(), "builder destroyed");
    assert_eq!(
        ledger.events("0://cloud/project/base"),
        vec![VmEvent::Error]
    );
}

#[test]
fn fingerprints_follow_lockfiles_and_the_build_command() {
    let dir = project();
    let a = project_fingerprint(dir.path(), "base1", "cargo build");
    assert_eq!(a, project_fingerprint(dir.path(), "base1", "cargo build"));
    assert_ne!(a, project_fingerprint(dir.path(), "base1", "cargo test"));
    assert_ne!(a, project_fingerprint(dir.path(), "base2", "cargo build"));
    std::fs::write(dir.path().join("Cargo.lock"), "v2").expect("write");
    assert_ne!(a, project_fingerprint(dir.path(), "base1", "cargo build"));
    assert_ne!(
        base_fingerprint(MachineSize::S4x16, "0.1.0"),
        base_fingerprint(MachineSize::S8x32, "0.1.0")
    );
    assert_eq!(
        default_build_command(dir.path()),
        "cargo fetch && cargo test --workspace --no-run"
    );
}

#[tokio::test]
async fn a_project_snapshot_is_built_once_and_reused() {
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let fake = FakeProvider::new();
    let (builder, ledger) = builder(&fake, home.path());

    let first = builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect("build");
    assert_eq!(first.name, "aura-proj-p1");
    assert!(first.commit.is_some());
    // Builders are gone, the base and project snapshots exist.
    assert!(
        fake.machine_names().is_empty(),
        "{:?}",
        fake.machine_names()
    );
    assert_eq!(fake.calls_matching("SaveSnapshot(").len(), 2);
    assert!(fake.calls().iter().any(|c| c.contains("cargo fetch")));

    let again = builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect("reuse");
    assert_eq!(again, first);
    assert_eq!(
        fake.calls_matching("SaveSnapshot(").len(),
        2,
        "nothing rebuilt"
    );

    std::fs::write(dir.path().join("Cargo.lock"), "v2").expect("write");
    builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect("rebuild");
    assert_eq!(
        fake.calls_matching("SaveSnapshot(").len(),
        3,
        "only the project rebuilt"
    );
    assert_eq!(
        ledger.events("0://cloud/project/p1"),
        vec![VmEvent::SnapshotSaved, VmEvent::SnapshotSaved]
    );

    // The index survives a restart.
    let (restarted, _) = super::tests::builder(&fake, home.path());
    let record = restarted
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect("reuse");
    assert_eq!(fake.calls_matching("SaveSnapshot(").len(), 3);
    assert_eq!(record.version, 2);
}

#[tokio::test]
async fn a_failed_step_destroys_the_builder_and_records_the_error() {
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let fake = FakeProvider::new();
    fake.on_exec(Arc::new(|_, command, _| crate::ExecOutput {
        exit_code: i32::from(command.contains("cargo fetch")),
        stdout: Vec::new(),
        stderr: b"error: failed to fetch".to_vec(),
    }));
    let (builder, ledger) = builder(&fake, home.path());
    let err = builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect_err("build fails");
    assert!(err.to_string().contains("failed to fetch"), "{err}");
    assert!(fake.machine_names().is_empty());
    assert_eq!(ledger.events("0://cloud/project/p1"), vec![VmEvent::Error]);
}

#[tokio::test]
async fn a_build_shows_its_commands_and_their_output() {
    use crate::boot::BootEvent;
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let fake = FakeProvider::new();
    fake.on_exec(Arc::new(|_, command, _| crate::ExecOutput {
        exit_code: 0,
        stdout: if command.contains("cargo fetch") {
            b"\x1b[32m   Compiling\x1b[0m x v0.1.0\n    Finished\n".to_vec()
        } else {
            Vec::new()
        },
        stderr: Vec::new(),
    }));
    let (builder, _) = builder(&fake, home.path());
    let (boot, mut rx) = BootReporter::channel();
    builder
        .ensure_project_reported("p1", dir.path(), None, false, &boot)
        .await
        .expect("build");
    let mut lines = Vec::new();
    let mut facts = Vec::new();
    let mut waits = Vec::new();
    let mut progressed = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            BootEvent::Log { line, .. } => lines.push(line),
            BootEvent::Fact { key, value } => facts.push((key, value)),
            BootEvent::Progress { .. } => progressed = true,
            BootEvent::Waiting { what, .. } => waits.push(what),
            BootEvent::Stage { .. } | BootEvent::Plan(_) => {}
        }
    }
    assert!(progressed);
    let said = |text: &str| waits.iter().flatten().any(|what| what.contains(text));
    assert!(said("first boot (cloud-init)"), "{waits:?}");
    assert!(said("boxd is creating builder machine"), "{waits:?}");
    assert!(said("cargo fetch"), "{waits:?}");
    assert!(said("ready"), "{waits:?}");
    assert_eq!(waits.last(), Some(&None), "the last wait is cleared");
    assert!(lines
        .iter()
        .any(|l| l.starts_with("$ ") && l.contains("cargo fetch")));
    assert!(
        lines.iter().any(|l| l == "   Compiling x v0.1.0"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l == "saving snapshot aura-proj-p1"));
    assert!(facts
        .iter()
        .any(|(k, v)| k == "snapshot" && v.contains("first build")));
    assert!(facts
        .iter()
        .any(|(k, v)| k == "snapshot" && v.ends_with("(built)")));

    let (boot, mut rx) = BootReporter::channel();
    builder
        .ensure_project_reported("p1", dir.path(), None, false, &boot)
        .await
        .expect("reuse");
    let mut cached = false;
    while let Ok(event) = rx.try_recv() {
        if let BootEvent::Fact { key, value } = event {
            cached |= key == "snapshot" && value.ends_with("(cached)");
        }
    }
    assert!(cached);
}

#[tokio::test]
async fn builders_delete_themselves_eventually() {
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let fake = FakeProvider::new();
    let (builder, _) = builder(&fake, home.path());
    builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect("build");
    let creates = fake.calls_matching("Create(");
    assert_eq!(creates.len(), 2, "{creates:?}");
    for create in &creates {
        assert!(create.contains("-build-"), "{create}");
        assert!(
            create.ends_with(&format!("ttl=Some({BUILDER_TTL_SECS}))")),
            "{create}"
        );
    }
    assert!(
        creates[1].contains("Snapshot(\"aura-base\")"),
        "{}",
        creates[1]
    );
}

#[test]
fn binary_digests_change_with_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("aura");
    std::fs::write(&path, b"build one").expect("write");
    let one = binary_digest(&path).expect("digest");
    std::fs::write(&path, b"build two").expect("write");
    assert_ne!(binary_digest(&path).expect("digest"), one);
    assert_eq!(one.len(), 16);
}
