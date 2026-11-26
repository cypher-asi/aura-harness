use super::*;

#[test]
fn lines_split_across_chunks() {
    let mut splitter = LineSplitter::default();
    assert!(splitter.push(b"Compil").is_empty());
    assert_eq!(
        splitter.push(b"ing x\nDone\n\npart"),
        vec!["Compiling x", "Done"]
    );
    assert_eq!(splitter.finish().as_deref(), Some("part"));
    assert_eq!(splitter.finish(), None);
}

#[test]
fn escapes_and_redraws_are_cleaned() {
    assert_eq!(
        clean_line(b"\x1b[1;32m   Compiling\x1b[0m aura v0.1.0").as_deref(),
        Some("   Compiling aura v0.1.0")
    );
    assert_eq!(
        clean_line(b"  10%\r  50%\r 100% done\r").as_deref(),
        Some(" 100% done")
    );
    assert_eq!(
        clean_line(b"\x1b]0;title\x07after").as_deref(),
        Some("after")
    );
    assert_eq!(clean_line(b"   \t "), None);
    let long = "x".repeat(MAX_LINE_CHARS + 50);
    let cut = clean_line(long.as_bytes()).expect("line");
    assert_eq!(cut.chars().count(), MAX_LINE_CHARS);
    assert!(cut.ends_with('…'));
}

#[tokio::test]
async fn stages_report_start_then_done_or_failure() {
    let (boot, mut rx) = BootReporter::channel();
    let ok: Result<u8, String> = boot.run(BootStage::Create, async { Ok(1) }).await;
    assert_eq!(ok, Ok(1));
    let err: Result<u8, String> = boot
        .run(BootStage::Claim, async { Err("refused".to_string()) })
        .await;
    assert!(err.is_err());
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    assert_eq!(events.len(), 4);
    assert!(matches!(
        events[1],
        BootEvent::Stage {
            stage: BootStage::Create,
            state: StageState::Done { .. }
        }
    ));
    assert_eq!(
        events[3],
        BootEvent::Stage {
            stage: BootStage::Claim,
            state: StageState::Failed("refused".into())
        }
    );
}

#[test]
fn output_is_reported_per_stream() {
    let (boot, mut rx) = BootReporter::channel();
    let mut out = boot.output(BootStage::Snapshot);
    out.chunk(false, b"one\ntw");
    out.chunk(true, b"warn\n");
    out.chunk(false, b"o");
    out.finish();
    let mut lines = Vec::new();
    while let Ok(BootEvent::Log { stderr, line, .. }) = rx.try_recv() {
        lines.push((stderr, line));
    }
    assert_eq!(
        lines,
        vec![
            (false, "one".to_string()),
            (true, "warn".to_string()),
            (false, "two".to_string())
        ]
    );
}

#[test]
fn a_silent_reporter_drops_everything() {
    let boot = BootReporter::none();
    assert!(!boot.is_active());
    boot.fact("region", "nyc");
}

fn drain(rx: &mut mpsc::UnboundedReceiver<BootEvent>) -> Vec<BootEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

fn cleared(stage: BootStage) -> BootEvent {
    BootEvent::Waiting {
        stage,
        what: None,
        limit: None,
    }
}

#[tokio::test]
async fn a_wait_shows_what_it_waits_on_then_clears() {
    let (boot, mut rx) = BootReporter::channel();
    let limit = Some(Duration::from_secs(180));
    let got = boot
        .wait(BootStage::Create, "boxd is creating aura-1", limit, async {
            7
        })
        .await;
    assert_eq!(got, 7);
    assert_eq!(
        drain(&mut rx),
        vec![
            BootEvent::Waiting {
                stage: BootStage::Create,
                what: Some("boxd is creating aura-1".into()),
                limit,
            },
            cleared(BootStage::Create),
        ]
    );
}

#[tokio::test]
async fn a_failed_wait_clears_too() {
    let (boot, mut rx) = BootReporter::channel();
    let got: Result<(), String> = boot
        .wait(BootStage::Claim, "claim", None, async {
            Err("refused".to_string())
        })
        .await;
    assert!(got.is_err());
    assert_eq!(drain(&mut rx).last(), Some(&cleared(BootStage::Claim)));
}

#[tokio::test]
async fn a_dropped_wait_clears_too() {
    let (boot, mut rx) = BootReporter::channel();
    let timed_out = tokio::time::timeout(
        Duration::from_millis(1),
        boot.wait(BootStage::Wake, "wake", None, std::future::pending::<()>()),
    )
    .await;
    assert!(timed_out.is_err());
    let events = drain(&mut rx);
    assert_eq!(events.len(), 2);
    assert_eq!(events.last(), Some(&cleared(BootStage::Wake)));
}

#[tokio::test]
async fn a_silent_reporter_still_runs_the_work() {
    let got = BootReporter::none()
        .wait(BootStage::Quota, "quota", None, async { "ran" })
        .await;
    assert_eq!(got, "ran");
}

#[test]
fn connecting_is_a_stage_of_its_own() {
    assert_eq!(BootStage::Connect.key(), "connect");
    assert_eq!(BootStage::Connect.label(), "Connecting to boxd");
}

#[test]
fn sizes_read_naturally() {
    assert_eq!(human_bytes(80), "80 B");
    assert_eq!(human_bytes(16 << 30), "16 GiB");
    assert_eq!(human_bytes(1536 << 20), "1.5 GiB");
    assert_eq!(human_bytes(100 << 30), "100 GiB");
}
