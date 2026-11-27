use super::boot::boot_updates;
use super::forward::{forward, TitleSink, Titles, TurnEnd, TurnExtras, CLOSED_EARLY};
use super::machine::machine_rows;
use super::submit::runs_on_machine;
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

    /// A prompt from the channel or the queue finds the chat idle, and an
    /// idle chat folds each group of steps as soon as none of them runs.
    #[tokio::test]
    async fn a_cloud_turn_puts_its_chat_to_work_until_it_ends() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Thinking("hmm".into())).await.unwrap();
        tx.send(done(10)).await.unwrap();
        drop(tx);

        let end = forward(
            &ui,
            &mut events,
            &mut TurnExtras::default(),
            &CancellationToken::new(),
        )
        .await;

        assert_eq!(end, TurnEnd::Done);
        let sent = commands(&mut rx);
        assert!(
            matches!(sent.first(), Some(UiCommand::TurnStarted { prompt: None })),
            "{sent:?}"
        );
        ends_the_turn(&sent);
    }

    #[tokio::test]
    async fn a_finished_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
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
        tx.send(done(100)).await.unwrap();
        drop(tx);

        let mut extras = TurnExtras {
            context_limit: Some(1_000),
            ..TurnExtras::default()
        };
        let end = forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

        assert_eq!(end, TurnEnd::Done);
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::FinishStreaming)));
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::CompleteTool { success: true, .. })));
        assert!(
            sent.iter().any(|command| matches!(
                command,
                UiCommand::SetContext(view)
                    if view.total_tokens == 100 && view.limit_tokens == Some(1_000)
            )),
            "the status bar shows the turn's context: {sent:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Failed("boom".into())).await.unwrap();
        drop(tx);

        let end = forward(
            &ui,
            &mut events,
            &mut TurnExtras::default(),
            &CancellationToken::new(),
        )
        .await;

        assert_eq!(end, TurnEnd::Failed("boom".into()));
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent.iter().any(|command| matches!(
            command,
            UiCommand::ShowError(message) if message.contains("boom")
        )));
    }

    #[tokio::test]
    async fn a_stopped_cloud_turn_says_so_without_an_error() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Text("half a reply".into()))
            .await
            .unwrap();
        tx.send(RemoteEvent::Failed(
            "machine aura-1 is cancelled; cannot finish the turn".into(),
        ))
        .await
        .unwrap();
        drop(tx);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let end = forward(&ui, &mut events, &mut TurnExtras::default(), &cancel).await;

        assert_eq!(end.status(cancel.is_cancelled()), ProcessStatus::Killed);
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(
            sent.iter().any(
                |command| matches!(command, UiCommand::ShowWarning(message) if message == "Turn cancelled")
            ),
            "{sent:?}"
        );
        assert!(
            !sent
                .iter()
                .any(|command| matches!(command, UiCommand::ShowError(_))),
            "{sent:?}"
        );
    }

    #[test]
    fn a_cloud_turn_s_row_ends_as_the_turn_did() {
        assert_eq!(TurnEnd::Done.status(false), ProcessStatus::Exited);
        assert_eq!(
            TurnEnd::Failed("boom".into()).status(false),
            ProcessStatus::Failed
        );
        assert_eq!(TurnEnd::Done.status(true), ProcessStatus::Killed);
    }

    #[tokio::test]
    async fn a_dropped_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Thinking("hmm".into())).await.unwrap();
        drop(tx);

        let end = forward(
            &ui,
            &mut events,
            &mut TurnExtras::default(),
            &CancellationToken::new(),
        )
        .await;

        assert_eq!(
            end,
            TurnEnd::Failed(CLOSED_EARLY.into()),
            "closed before the reply ended"
        );
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::FinishThinking)));
    }

    fn done(context_tokens: u64) -> RemoteEvent {
        RemoteEvent::Done(Box::new(aura_core_protocol::SessionUsage {
            model: "m".into(),
            input_tokens: 1,
            output_tokens: 2,
            estimated_context_tokens: context_tokens,
            ..Default::default()
        }))
    }

    /// Titles that remember what they were given.
    fn titles(prompt: &str) -> (Titles, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let sink: TitleSink = Box::new(move |title| {
            let kept = Arc::clone(&kept);
            Box::pin(async move { kept.lock().push(title) })
        });
        (Titles::new(sink, prompt), seen)
    }

    #[tokio::test]
    async fn the_machine_s_title_names_the_chat_without_breaking_the_reply() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Text("hel".into())).await.unwrap();
        tx.send(RemoteEvent::Title("Build fix".into()))
            .await
            .unwrap();
        tx.send(RemoteEvent::Text("lo".into())).await.unwrap();
        tx.send(done(10)).await.unwrap();
        drop(tx);
        let (titles, seen) = titles("fix the build please");
        let mut extras = TurnExtras {
            titles: Some(titles),
            ..TurnExtras::default()
        };

        forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

        assert_eq!(*seen.lock(), ["Build fix"], "and no title from the prompt");
        let sent = commands(&mut rx);
        let starts = sent
            .iter()
            .filter(|command| matches!(command, UiCommand::StartStreaming))
            .count();
        assert_eq!(starts, 1, "one reply: {sent:?}");
    }

    #[tokio::test]
    async fn without_a_title_from_the_machine_the_prompt_names_the_chat() {
        for last in [done(10), RemoteEvent::Failed("boom".into())] {
            let (ui, _rx) = chat();
            let (tx, mut events) = mpsc::channel(8);
            tx.send(RemoteEvent::Text("ok".into())).await.unwrap();
            tx.send(last).await.unwrap();
            drop(tx);
            let (titles, seen) = titles("\n  fix   the build\nthen run tests");
            let mut extras = TurnExtras {
                titles: Some(titles),
                ..TurnExtras::default()
            };

            forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

            assert_eq!(*seen.lock(), ["fix the build"]);
        }
    }

    #[test]
    fn titles_are_written_on_the_machine_only_by_models_it_can_run() {
        use aura_core_auth::ProviderKind;
        let linked = [ProviderKind::Claude];
        assert!(runs_on_machine("aura-claude-haiku-4-5", &linked));
        assert!(runs_on_machine("claude/claude-haiku-4-5", &linked));
        assert!(
            !runs_on_machine("chatgpt/gpt-5-mini", &linked),
            "plan not linked"
        );
        assert!(!runs_on_machine("local/qwen3:4b", &linked));
        assert!(!runs_on_machine("gate", &linked));
        assert!(!runs_on_machine("", &linked));
    }
}

#[cfg(test)]
mod machine_tests {
    use super::*;
    use aura_core_protocol::cloud::MESH_PROCESS_AGENT;

    fn row(id: &str, agent: &str, status: &str, parent: Option<&str>) -> MeshProcess {
        MeshProcess {
            id: id.into(),
            agent_id: agent.into(),
            label: id.into(),
            status: status.into(),
            started_ms: 5,
            kind: parent.map(|_| MESH_PROCESS_AGENT.into()),
            run_id: None,
            parent: parent.map(str::to_string),
            name: parent.map(|_| format!("name of {agent}")),
        }
    }

    #[test]
    fn a_machine_s_work_becomes_rows_members_can_list() {
        let rows = machine_rows(
            "aa",
            &[
                row("turn:r1:5", "aa", "running", None),
                row("agent:bb", "bb", "running", Some("aa")),
                row("agent:cc", "cc", "idle", Some("bb")),
            ],
        );

        assert_eq!(rows[0].id, "machine:turn:r1:5");
        assert_eq!(rows[0].kind, ProcessKind::Turn);
        assert_eq!(rows[0].owner_agent_id, "aa");

        assert_eq!(rows[1].id, "agent:bb");
        assert_eq!(rows[1].kind, ProcessKind::Agent);
        assert_eq!(rows[1].owner_agent_id, "aa");
        assert!(rows[1].parent_chain.is_empty());
        assert_eq!(rows[1].status, ProcessStatus::Running);

        assert_eq!(rows[2].owner_agent_id, "bb");
        assert_eq!(rows[2].owner_name, "name of bb");
        assert_eq!(rows[2].parent_chain, ["aa"]);
        assert_eq!(rows[2].status, ProcessStatus::Idle);

        assert!(rows.iter().all(|row| row.jump_agent_id == "aa"));
        assert!(rows.iter().all(|row| !row.killable));
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
