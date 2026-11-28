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
        !commands
            .iter()
            .any(|command| matches!(command, UiCommand::SetStatus(status) if status == "Ready")),
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

/// The events of a turn that says hello, runs a command that prints a
/// line and succeeds, then says done and ends.
async fn finished_turn_with_a_tool() -> mpsc::Receiver<RemoteEvent> {
    let (tx, events) = mpsc::channel(8);
    tx.send(RemoteEvent::Text("hello".into())).await.unwrap();
    tx.send(RemoteEvent::ToolStart {
        id: "t1".into(),
        name: "run_command".into(),
    })
    .await
    .unwrap();
    tx.send(RemoteEvent::ToolOutput {
        id: "t1".into(),
        chunk: "building\n".into(),
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
    events
}

#[tokio::test]
async fn a_finished_cloud_turn_completes_the_chat() {
    let (ui, mut rx) = chat();
    let mut events = finished_turn_with_a_tool().await;

    let mut extras = TurnExtras {
        said: String::new(),
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
    assert!(sent.iter().any(|command| matches!(
        command,
        UiCommand::ToolOutput { id, chunk }
            if id == "t1" && chunk == "building\n"
    )));
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
        said: String::new(),
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
async fn each_model_call_writes_a_reply_of_its_own() {
    let (ui, mut rx) = chat();
    let (tx, mut events) = mpsc::channel(8);
    for event in [
        RemoteEvent::Text("Built it.".into()),
        RemoteEvent::ModelCallEnd,
        RemoteEvent::Text("Nothing is blocked.".into()),
    ] {
        tx.send(event).await.unwrap();
    }
    drop(tx);
    let cancel = CancellationToken::new();
    forward(&ui, &mut events, &mut TurnExtras::default(), &cancel).await;

    let sent = commands(&mut rx);
    let starts = sent
        .iter()
        .filter(|command| matches!(command, UiCommand::StartStreaming))
        .count();
    assert_eq!(starts, 2, "a reply per model call: {sent:?}");
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
            said: String::new(),
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
