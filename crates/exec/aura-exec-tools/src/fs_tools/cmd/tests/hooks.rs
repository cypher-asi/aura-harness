use super::*;

#[cfg(unix)]
#[tokio::test]
async fn test_background_command_returns_at_once_and_logs_output() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["echo".to_string(), "true".to_string()],
        ..Default::default()
    });
    let (mut ctx, dir) = tool_ctx(config);
    let hook = std::sync::Arc::new(KeepProcesses {
        dir: dir.path().join("logs"),
        adopted: std::sync::Mutex::new(Vec::new()),
    });
    ctx.background_processes = Some(hook.clone());
    ctx.caller_agent_id = Some(aura_core_types::AgentId::generate());
    ctx.current_tool_use_id = Some("toolu_9".into());

    let started = std::time::Instant::now();
    let result = CmdRunTool
        .execute(
            &ctx,
            serde_json::json!({
                "shell_script": "echo out; echo err >&2; sleep 2",
                "background": true
            }),
        )
        .await
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(result.ok);
    assert_eq!(
        result.metadata.get("background").map(String::as_str),
        Some("true")
    );

    let mut process = hook.adopted.lock().unwrap().pop().unwrap();
    assert_eq!(process.tool_use_id.as_deref(), Some("toolu_9"));
    assert_eq!(
        result.metadata.get("log_path").map(String::as_str),
        Some(process.log_path.to_str().unwrap())
    );
    let mut log = String::new();
    for _ in 0..100 {
        log = std::fs::read_to_string(&process.log_path).unwrap();
        if log.contains("out") && log.contains("err") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    process.child.kill().unwrap();
    process.child.wait().unwrap();
    assert!(log.contains("out") && log.contains("err"), "{log}");
}

#[tokio::test]
async fn test_background_command_needs_a_hook() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["echo".to_string(), "true".to_string()],
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    let err = CmdRunTool
        .execute(
            &ctx,
            serde_json::json!({ "shell_script": "true", "background": true }),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(msg) if msg.contains("background")));
}

#[cfg(unix)]
#[tokio::test]
async fn test_commands_carry_the_calling_agent_in_their_environment() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["echo".to_string()],
        ..Default::default()
    });
    let (mut ctx, _dir) = tool_ctx(config);
    let agent = aura_core_types::AgentId::generate();
    ctx.caller_agent_id = Some(agent);
    ctx.current_tool_use_id = Some("toolu_7".into());

    let result = CmdRunTool
        .execute(
            &ctx,
            serde_json::json!({
                "shell_script": "echo $AURA_AGENT_ID $AURA_TOOL_USE_ID $AURA_HARNESS_PID"
            }),
        )
        .await
        .unwrap();
    let out = String::from_utf8_lossy(&result.stdout).to_string();
    assert_eq!(
        out.trim(),
        format!("{} toolu_7 {}", agent.to_hex(), std::process::id())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_foreground_output_reaches_the_hook_while_the_command_runs() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["echo".to_string(), "sleep".to_string()],
        ..Default::default()
    });
    for tty in [false, true] {
        let (mut ctx, _dir) = tool_ctx(config.clone());
        let hook = std::sync::Arc::new(RecordOutput::default());
        ctx.command_output = Some(hook.clone());
        ctx.current_tool_use_id = Some("toolu_live".into());
        let mut args = serde_json::json!({ "shell_script": "echo a; sleep 0.5; echo b" });
        if tty {
            args["tty_cols"] = serde_json::json!(80);
        }

        let result = CmdRunTool.execute(&ctx, args).await.unwrap();
        let returned = std::time::Instant::now();

        assert!(result.ok, "{result:?}");
        assert_eq!(String::from_utf8_lossy(&result.stdout), "a\nb\n");
        let chunks = hook.0.lock().unwrap();
        let (first_at, id, first) = chunks.first().expect("output reached the hook");
        assert_eq!(id, "toolu_live");
        assert!(String::from_utf8_lossy(first).starts_with('a'), "{first:?}");
        assert!(
            returned.duration_since(*first_at) >= std::time::Duration::from_millis(300),
            "tty={tty}: the first line came only as the command ended"
        );
        let all: Vec<u8> = chunks.iter().flat_map(|(_, _, c)| c.clone()).collect();
        assert_eq!(
            String::from_utf8_lossy(&all).replace("\r\n", "\n"),
            "a\nb\n"
        );
    }
}

/// A context whose commands may write only inside `workspace`.
#[cfg(target_os = "linux")]
fn confined_ctx(workspace: &std::path::Path) -> ToolContext {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["sh".to_string(), "touch".to_string()],
        ..Default::default()
    });
    let mut ctx = ToolContext::new(Sandbox::new(workspace).unwrap(), config);
    ctx.confine_writes = true;
    ctx
}

#[cfg(target_os = "linux")]
async fn assert_run_command_succeeds(ctx: &ToolContext, args: serde_json::Value) {
    let result = CmdRunTool.execute(ctx, args).await.unwrap();
    assert!(result.ok, "{result:?}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_confined_run_command_cannot_write_above_the_workspace() {
    if !crate::confine::supported() {
        return;
    }
    // Not under /tmp or a cache folder, which confined commands may write to.
    let Some(parent) = crate::confine_test::unwritable_parent() else {
        eprintln!("no folder outside the writable roots here; skipping");
        return;
    };
    let workspace = parent.path().join("ws");
    std::fs::create_dir(&workspace).unwrap();
    let mut ctx = confined_ctx(&workspace);

    for args in [
        serde_json::json!({ "shell_script": "touch inside" }),
        serde_json::json!({ "program": "touch", "args": ["inside2"] }),
    ] {
        assert_run_command_succeeds(&ctx, args).await;
    }
    assert!(workspace.join("inside").exists());
    assert!(workspace.join("inside2").exists());

    for args in [
        serde_json::json!({ "shell_script": "touch ../outside" }),
        serde_json::json!({ "program": "touch", "args": ["../outside"] }),
        serde_json::json!({ "program": "touch", "args": ["../outside"], "tty_cols": 80 }),
    ] {
        let result = CmdRunTool.execute(&ctx, args.clone()).await.unwrap();
        assert!(!result.ok, "{args} should fail: {result:?}");
    }
    assert!(!parent.path().join("outside").exists());

    // The confinement, not the binary allowlist, bounds confined commands.
    let unlisted = serde_json::json!({ "program": "mkdir", "args": ["unlisted"] });
    assert_run_command_succeeds(&ctx, unlisted).await;
    assert!(workspace.join("unlisted").is_dir());

    ctx.confine_writes = false;
    let err = CmdRunTool
        .execute(
            &ctx,
            serde_json::json!({ "program": "mkdir", "args": ["x"] }),
        )
        .await
        .expect_err("unconfined commands keep the binary allowlist");
    assert!(matches!(err, ToolError::Forbidden(_)), "{err:?}");

    let outside = serde_json::json!({ "program": "touch", "args": ["../outside"] });
    assert_run_command_succeeds(&ctx, outside).await;
}
