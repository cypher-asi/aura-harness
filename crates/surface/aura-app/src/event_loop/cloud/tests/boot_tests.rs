use super::*;

#[tokio::test]
async fn boot_events_become_console_updates_with_log_runs_joined() {
    let (boot, mut events) = BootReporter::channel();
    boot.fact("machine", "aura-1");
    let _ = boot
        .run(BootStage::Snapshot, async {
            boot.log(BootStage::Snapshot, false, "$ cargo build");
            boot.log(BootStage::Snapshot, true, "Compiling aura");
            Ok::<_, String>(())
        })
        .await;
    drop(boot);
    let mut batch = Vec::new();
    while events.recv_many(&mut batch, 64).await > 0 {}

    let updates = boot_updates(batch);
    assert_eq!(updates.len(), 4, "{updates:?}");
    assert!(matches!(
        &updates[0],
        BootUpdate::Fact { key, value } if key == "machine" && value == "aura-1"
    ));
    assert!(matches!(
        &updates[1],
        BootUpdate::Stage { key, state: BootStageState::Running, .. } if key == "snapshot"
    ));
    let BootUpdate::Log(lines) = &updates[2] else {
        panic!("expected a log run: {updates:?}");
    };
    assert_eq!(
        lines,
        &vec![
            BootLogLine {
                stderr: false,
                text: "$ cargo build".into()
            },
            BootLogLine {
                stderr: true,
                text: "Compiling aura".into()
            },
        ]
    );
    assert!(matches!(
        &updates[3],
        BootUpdate::Stage {
            state: BootStageState::Done { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn a_wait_becomes_a_waiting_line_then_its_clear() {
    let (boot, mut events) = BootReporter::channel();
    boot.wait(
        BootStage::Create,
        "boxd is creating aura-1",
        Some(Duration::from_secs(180)),
        async {},
    )
    .await;
    drop(boot);
    let mut batch = Vec::new();
    while events.recv_many(&mut batch, 64).await > 0 {}
    assert_eq!(
        boot_updates(batch),
        vec![
            BootUpdate::Waiting {
                key: "create".into(),
                what: Some("boxd is creating aura-1".into()),
                limit_ms: Some(180_000),
            },
            BootUpdate::Waiting {
                key: "create".into(),
                what: None,
                limit_ms: None,
            },
        ]
    );
}
