use super::*;
use std::collections::HashMap;

fn config(vars: &[(&str, &str)]) -> PostEditCheckConfig {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    PostEditCheckConfig::from_lookup(|name| vars.get(name).cloned())
}

#[test]
fn parses_a_check_line() {
    assert_eq!(
        parse_agents_md_check("# Project\n\nCheck: `cargo check --workspace`\n"),
        Some("cargo check --workspace".into())
    );
    assert_eq!(
        parse_agents_md_check("## Commands\n- check: npm run lint && npm test\n"),
        Some("npm run lint && npm test".into())
    );
}

#[test]
fn parses_a_check_block() {
    let md = "Intro\n\n```check\n# fast checks first\ncargo fmt --check\n\ncargo clippy\n```\n\ncheck: later\n";
    assert_eq!(
        parse_agents_md_check(md),
        Some("cargo fmt --check && cargo clippy".into())
    );
    assert_eq!(
        parse_agents_md_check("~~~~ check\nmake test\n~~~~\n"),
        Some("make test".into())
    );
}

#[test]
fn ignores_check_lines_inside_other_blocks_and_empty_values() {
    let md = "```yaml\ncheck: not-this\n```\nCheck:\ncheckout: no\n```check\n```\n";
    assert_eq!(parse_agents_md_check(md), None);
    assert_eq!(
        parse_agents_md_check("```yaml\ncheck: x\n```\ncheck: yes\n"),
        Some("yes".into())
    );
}

#[test]
fn env_command_beats_agents_md_and_off_disables_both() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "check: make check\n").unwrap();

    assert_eq!(
        config(&[]).resolve(dir.path()),
        Some(CheckCommand {
            command: "make check".into(),
            source: CheckSource::AgentsMd
        })
    );
    assert_eq!(
        config(&[(CHECK_COMMAND_ENV, " cargo test ")]).resolve(dir.path()),
        Some(CheckCommand {
            command: "cargo test".into(),
            source: CheckSource::Env
        })
    );
    assert_eq!(
        config(&[(CHECK_COMMAND_ENV, "off")]).resolve(dir.path()),
        None
    );
    assert_eq!(
        config(&[(CHECK_COMMAND_ENV, "")])
            .resolve(dir.path())
            .map(|c| c.source),
        Some(CheckSource::AgentsMd)
    );
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(config(&[]).resolve(empty.path()), None);
}

#[test]
fn reads_the_timeout() {
    assert_eq!(config(&[]).timeout, DEFAULT_TIMEOUT);
    assert_eq!(
        config(&[(CHECK_TIMEOUT_ENV, "45")]).timeout,
        Duration::from_secs(45)
    );
    assert_eq!(
        config(&[(CHECK_TIMEOUT_ENV, "5m")]).timeout,
        Duration::from_secs(300)
    );
    assert_eq!(
        config(&[(CHECK_TIMEOUT_ENV, "soon")]).timeout,
        DEFAULT_TIMEOUT
    );
    assert_eq!(config(&[(CHECK_TIMEOUT_ENV, "999m")]).timeout, MAX_TIMEOUT);
}

#[test]
fn tail_keeps_the_end() {
    let output: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    let kept = tail(&output);
    assert!(
        kept.starts_with("[... 120 earlier lines omitted]\nline 121\n"),
        "{kept}"
    );
    assert!(kept.ends_with("line 200"));

    let long: String = (1..=50)
        .map(|i| format!("{i}{}\n", "x".repeat(400)))
        .collect();
    let kept = tail(&long);
    assert!(kept.len() <= TAIL_BYTES + 40, "{}", kept.len());
    assert!(kept.ends_with(&format!("50{}", "x".repeat(400))));
    assert_eq!(tail(""), "(no output)");
}

#[test]
fn counts_errors_at_least_once() {
    assert_eq!(count_errors("error[E0308]: x\n  error: y\nwarning: z"), 2);
    assert_eq!(count_errors("FAIL src/a.test.ts"), 1);
}

#[cfg(unix)]
mod running {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn check_in(dir: &Path, command: &str) -> (PostEditCheck, Arc<Mutex<Vec<CheckReport>>>) {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&reports);
        let check = PostEditCheck::new(dir, config(&[(CHECK_COMMAND_ENV, command)])).with_notifier(
            Arc::new(move |r: &CheckReport| {
                sink.lock().unwrap().push(r.clone());
            }),
        );
        (check, reports)
    }

    #[tokio::test]
    async fn failure_is_reported_once_then_shortened_then_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("broken");
        std::fs::write(&flag, "").unwrap();
        let command = "test ! -f broken || { echo 'error: it broke' >&2; exit 3; }";
        let (check, reports) = check_in(dir.path(), command);

        let first = check.run().await.unwrap();
        assert!(!first.success);
        assert_eq!(first.error_count, 1);
        assert!(
            first.output.contains("failed with exit code 3"),
            "{}",
            first.output
        );
        assert!(first.output.contains("set by AURA_CHECK_COMMAND"));
        assert!(first.output.contains("error: it broke"));

        let second = check.run().await.unwrap();
        assert!(!second.success);
        assert!(
            second.output.contains("still fails with the same output"),
            "{}",
            second.output
        );

        std::fs::remove_file(&flag).unwrap();
        let third = check.run().await.unwrap();
        assert!(third.success);
        assert!(third.output.is_empty());

        let outcomes: Vec<CheckOutcome> = reports
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.outcome.clone())
            .collect();
        assert_eq!(
            outcomes,
            vec![
                CheckOutcome::Failed {
                    exit_code: Some(3),
                    timed_out: false
                },
                CheckOutcome::StillFailing,
                CheckOutcome::Passed { recovered: true },
            ]
        );
    }

    #[tokio::test]
    async fn slow_checks_time_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(&[(CHECK_COMMAND_ENV, "echo started; sleep 30")]);
        cfg.timeout = Duration::from_millis(300);
        let check = PostEditCheck::new(dir.path(), cfg);
        let started = std::time::Instant::now();
        let result = check.run().await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(15));
        assert!(!result.success);
        assert!(
            result.output.contains("timed out after 0s"),
            "{}",
            result.output
        );
    }

    #[tokio::test]
    async fn unconfigured_check_defers_to_the_inner_executor() {
        let dir = tempfile::tempdir().unwrap();
        let check = PostEditCheck::new(dir.path(), config(&[]));
        assert!(check.run().await.is_none());
    }

    #[tokio::test]
    async fn a_check_with_a_run_scratch_builds_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = RunScratch::new(dir.path().join("a-1.tmp"));
        let target = scratch.dir().join("target");
        let command = format!(
                "test \"$CARGO_TARGET_DIR\" = '{}' || {{ echo \"error: target $CARGO_TARGET_DIR\"; exit 1; }}",
                target.display()
            );
        let (check, _) = check_in(dir.path(), &command);
        let without = check.run().await.unwrap();
        assert!(!without.success, "the harness's own target dir is used");

        let (check, _) = check_in(dir.path(), &command);
        let check = check.with_run_scratch(scratch.clone());
        assert_eq!(check.run_scratch(), Some(&scratch));
        let with = check.run().await.unwrap();
        assert!(with.success, "{}", with.output);
    }

    /// Counts tool calls; every call succeeds.
    struct Writes(AtomicUsize);

    #[async_trait]
    impl AgentToolExecutor for Writes {
        async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
            self.0.fetch_add(tool_calls.len(), Ordering::SeqCst);
            tool_calls
                .iter()
                .map(|tc| ToolCallResult::success(&tc.id, "ok"))
                .collect()
        }
    }

    /// One model step writing `a.rs`, `b.rs` and `c.rs`, then a final text reply.
    fn three_writes_then_done() -> aura_model::MockProvider {
        use aura_model::{ContentBlock, MockProvider, MockResponse, StopReason, Usage};
        let write = |id: &str, path: &str| {
            ContentBlock::tool_use(
                id,
                "write_file",
                serde_json::json!({"path": path, "content": "x"}),
            )
        };
        MockProvider::new()
            .with_response(MockResponse {
                stop_reason: StopReason::ToolUse,
                content: vec![
                    write("w1", "a.rs"),
                    write("w2", "b.rs"),
                    write("w3", "c.rs"),
                ],
                usage: Usage::new(100, 50),
            })
            .with_response(MockResponse::text("done"))
    }

    #[tokio::test]
    async fn a_step_with_several_edits_runs_the_check_once_and_feeds_back_failures() {
        use aura_model::{ContentBlock, Message};

        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("runs");
        let command = format!(
            "echo run >> {} ; echo 'error: bad edit' ; exit 1",
            runs.display()
        );
        let (check, _) = check_in(dir.path(), &command);

        let provider = three_writes_then_done();
        let inner = Writes(AtomicUsize::new(0));
        let executor = WithPostEditCheck::new(&inner, Some(&check));
        let agent = crate::AgentLoop::new(crate::AgentLoopConfig {
            system_prompt: "test".into(),
            ..crate::AgentLoopConfig::for_agent("claude-test-model")
        });
        let tools = vec![aura_model::ToolDefinition::new(
            "write_file",
            "Write a file",
            serde_json::json!({"type": "object"}),
        )];

        let result = agent
            .run(&provider, &executor, vec![Message::user("edit")], tools)
            .await
            .unwrap();

        assert_eq!(inner.0.load(Ordering::SeqCst), 3);
        assert_eq!(std::fs::read_to_string(&runs).unwrap().lines().count(), 1);
        let fed_back = result
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .any(|block| {
                matches!(block, ContentBlock::Text { text }
                    if text.contains("Post-edit check") && text.contains("error: bad edit"))
            });
        assert!(
            fed_back,
            "the failure must reach the model: {:#?}",
            result.messages
        );
    }
}
