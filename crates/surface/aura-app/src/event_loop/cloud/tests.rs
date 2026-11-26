use super::boot::boot_updates;
use super::submit::forward;
use super::*;

#[cfg(test)]
mod owner_tests {
    use super::*;
    use aura_core_auth::{Owner, ProviderKind};

    #[test]
    fn only_the_placing_user_manages_a_cloud_agent() {
        let alice = Owner::Zos {
            user_id: "alice".into(),
        };
        let bob = Owner::Zos {
            user_id: "bob".into(),
        };
        assert_eq!(refusal_for(&alice, Some("Alice".into()), &alice), None);
        assert_eq!(
            refusal_for(&alice, Some("Alice".into()), &bob).as_deref(),
            Some("Placed by Alice; sign in as Alice to manage it.")
        );
        assert!(refusal_for(&Owner::Local, None, &bob)
            .is_some_and(|note| note.contains("local profile")));
    }

    #[test]
    fn plan_models_are_recognized() {
        assert_eq!(
            plan_of_model("claude/claude-sonnet-4"),
            Some(ProviderKind::Claude)
        );
        assert_eq!(plan_of_model("chatgpt/gpt-5"), Some(ProviderKind::Chatgpt));
        assert_eq!(
            plan_of_model("openrouter/x/y"),
            Some(ProviderKind::OpenRouter)
        );
        assert_eq!(plan_of_model("aura-claude-sonnet"), None);
        assert_eq!(plan_of_model("local/qwen3:4b"), None);
        assert_eq!(plan_of_model("zos/x"), None);
    }
}

#[cfg(test)]
mod forward_tests {
    use super::*;
    use tokio::sync::mpsc;

    fn chat() -> (AgentUi, mpsc::Receiver<UiCommand>) {
        let (tx, rx) = mpsc::channel(32);
        (AgentUi::new(tx, AgentId::new([7; 32])), rx)
    }

    fn commands(rx: &mut mpsc::Receiver<UiCommand>) -> Vec<UiCommand> {
        let mut out = Vec::new();
        while let Ok(command) = rx.try_recv() {
            match command {
                UiCommand::ForAgent { command, .. } => out.push(*command),
                other => out.push(other),
            }
        }
        out
    }

    /// The working line clears only when the chat leaves Processing, which
    /// `Complete` does. A status of Ready does not.
    fn ends_the_turn(commands: &[UiCommand]) {
        assert!(
            matches!(commands.last(), Some(UiCommand::Complete)),
            "{commands:?}"
        );
        assert!(
            !commands.iter().any(
                |command| matches!(command, UiCommand::SetStatus(status) if status == "Ready")
            ),
            "{commands:?}"
        );
    }

    #[tokio::test]
    async fn a_finished_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, events) = mpsc::channel(8);
        tx.send(RemoteEvent::Text("hello".into())).await.unwrap();
        tx.send(RemoteEvent::ToolStart {
            id: "t1".into(),
            name: "run_command".into(),
        })
        .await
        .unwrap();
        tx.send(RemoteEvent::ToolResult {
            id: "t1".into(),
            result: "ok".into(),
            is_error: false,
        })
        .await
        .unwrap();
        tx.send(RemoteEvent::Text("done".into())).await.unwrap();
        tx.send(RemoteEvent::Done {
            model: "m".into(),
            input_tokens: 1,
            output_tokens: 2,
        })
        .await
        .unwrap();
        drop(tx);

        forward(ui, events).await;

        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::FinishStreaming)));
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::CompleteTool { success: true, .. })));
    }

    #[tokio::test]
    async fn a_failed_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, events) = mpsc::channel(8);
        tx.send(RemoteEvent::Failed("boom".into())).await.unwrap();
        drop(tx);

        forward(ui, events).await;

        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent.iter().any(|command| matches!(
            command,
            UiCommand::ShowError(message) if message.contains("boom")
        )));
    }

    #[tokio::test]
    async fn a_dropped_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, events) = mpsc::channel(8);
        tx.send(RemoteEvent::Thinking("hmm".into())).await.unwrap();
        drop(tx);

        forward(ui, events).await;

        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::FinishThinking)));
    }
}

#[cfg(test)]
mod boot_tests {
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
}
