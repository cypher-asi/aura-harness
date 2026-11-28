//! Turns, the queue, attachments and titles.

use super::*;

#[tokio::test]
async fn another_agent_works_while_the_first_is_busy() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    assert!(state.hub.is_running(first));

    handle_ui_event(&mut state, UiEvent::SelectAgent(second.to_hex()), &links).await;
    assert_eq!(state.active, second);
    handle_ui_event(&mut state, message(second, "quick question", true), &links).await;

    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, second);
    handle_work_done(&mut state, finished).await;
    assert!(
        state.hub.is_running(first),
        "the first agent is still working"
    );
    assert_eq!(state.idle[&second].messages.len(), 2);

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, first);
    handle_work_done(&mut state, finished).await;
    assert!(!state.hub.is_running(first));
    assert_eq!(state.idle[&first].messages.len(), 2);
    assert_eq!(state.in_flight, 0);

    let commands = drain(&mut commands);
    assert_eq!(completed(&commands, first), 1);
    assert_eq!(completed(&commands, second), 1);
    let replies_to_second = commands.iter().any(|(to, c)| {
        to.as_deref() == Some(second.to_hex().as_str())
            && matches!(c, UiCommand::AppendText(_) | UiCommand::ShowMessage(_))
    });
    assert!(
        replies_to_second,
        "the second agent's reply goes to its chat"
    );
}

#[tokio::test]
async fn message_to_a_busy_agent_waits_in_the_queue() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, message(first, "follow-up", false), &links).await;
    assert_eq!(state.queued.len(first), 1, "the message waits its turn");

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    assert!(finished.unseen.is_empty());
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    assert_eq!(user_texts(&state, first), ["slow start", "follow-up"]);
    let shown = drain(&mut commands)
        .into_iter()
        .any(|(_, c)| matches!(c, UiCommand::TurnStarted { prompt: Some(p) } if p == "follow-up"));
    assert!(shown, "the chat shows the message when its turn starts");
}

#[tokio::test]
async fn send_queued_now_cancels_the_turn_and_runs_the_message_next() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, message(first, "urgent", false), &links).await;
    handle_ui_event(&mut state, message(first, "later", false), &links).await;
    assert_eq!(state.queued.len(first), 2);

    // The slow turn is never released: only the cancellation ends it. The
    // cancelled turn leaves "slow start" as the last user message and the
    // next prompt joins it, so the mock holds the later turns too; these
    // permits let them through.
    handle_ui_event(&mut state, UiEvent::SendQueuedNow(first.to_hex()), &links).await;
    let finished = next_done(&mut done).await;
    fixture.gate.add_permits(2);
    handle_work_done(&mut state, finished).await;

    assert!(state.hub.is_running(first), "the queued message started");
    assert_eq!(state.queued.len(first), 1, "only the first one ran");
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let texts = user_texts(&state, first);
    let position = |text: &str| texts.iter().position(|t| t == text);
    assert!(position("urgent") < position("later"), "{texts:?}");
    assert!(state.queued.is_empty());
}

/// Holds a run's write-up until the test drops `release`; tells
/// `written` once one is done.
struct HeldWriteUp {
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    written: std::sync::mpsc::Sender<()>,
}

impl aura_agent_trace::TraceSink for HeldWriteUp {
    fn meta(&self, _: &aura_agent_trace::RunMeta) {}
    fn steps(&self, _: &str, _: &[aura_core_types::trace::Step]) {}
    fn rewards(&self, _: &str, _: &[aura_core_types::trace::Reward]) {}
    fn records(&self, _: &str, _: &[aura_agent_trace::RecordRow]) {}
    fn summary(&self, _: &str, _: &aura_agent_trace::RunScorecard) {
        let release = self.release.lock().unwrap();
        let _ = release.recv_timeout(Duration::from_secs(10));
        let _ = self.written.send(());
    }
}

/// After each turn the agent's run is written up: its scorecard, and its
/// log rebuilt from the whole run and the runs it spawned, which takes
/// seconds for a long run. A message sent now doesn't wait for that.
#[tokio::test]
async fn send_queued_now_starts_the_message_before_the_run_is_written_up() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, message(first, "urgent", false), &links).await;
    let (release, held) = std::sync::mpsc::channel();
    let (written, write_up) = std::sync::mpsc::channel();
    let run = state.hub.traces.recorder(first).expect("the turn's run");
    run.add_sink(Arc::new(HeldWriteUp {
        release: std::sync::Mutex::new(held),
        written,
    }));

    // The permit is for the queued message's turn; see
    // `send_queued_now_cancels_the_turn_and_runs_the_message_next`.
    handle_ui_event(&mut state, UiEvent::SendQueuedNow(first.to_hex()), &links).await;
    let finished = next_done(&mut done).await;
    fixture.gate.add_permits(1);
    handle_work_done(&mut state, finished).await;
    assert!(state.hub.is_running(first), "the queued message started");
    assert!(
        write_up.try_recv().is_err(),
        "before the cancelled turn was written up"
    );

    drop(release);
    write_up.recv_timeout(Duration::from_secs(10)).unwrap();
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
}

#[tokio::test]
async fn send_queued_now_with_nothing_queued_leaves_the_turn_alone() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    handle_ui_event(&mut state, UiEvent::SendQueuedNow(first.to_hex()), &links).await;

    assert!(
        state.hub.is_running(first),
        "nothing to send, nothing cancelled"
    );
    fixture.gate.add_permits(1);
}

/// Quitting cancels a running turn and waits for it to end, so it saves
/// the session as every turn does. A turn dropped at quit loses all it
/// did since the last checkpoint, and an agent still on its first turn
/// comes back with no session at all.
#[tokio::test]
async fn shutting_down_mid_turn_saves_the_running_turns_session() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let checkpoints = fixture.hub.factory.checkpoints.clone().unwrap();
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow job", true), &links).await;
    assert!(state.hub.is_running(first));
    assert!(
        checkpoints.load_current(first).is_none(),
        "nothing is saved before the first turn ends"
    );

    // The slow turn is never released: only the shutdown's cancel ends it.
    shut_down(&mut state, &mut done).await;

    assert_eq!(state.in_flight, 0, "the turn ended within the grace");
    let saved = checkpoints
        .load_current(first)
        .expect("the cancelled turn saved its session");
    let prompts: Vec<String> = saved
        .messages
        .iter()
        .filter(|m| m.role == aura_model::Role::User)
        .map(aura_model::Message::text_content)
        .collect();
    assert_eq!(prompts, ["slow job"]);
}

#[tokio::test]
async fn mentioned_files_reach_the_model_but_the_chat_keeps_the_typed_text() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let workspace = state.kernel_for(first).unwrap().agent_workspace();
    std::fs::write(workspace.join("notes.md"), "remember the milk\n").unwrap();

    let typed = "explain @notes.md and @missing.md";
    handle_ui_event(&mut state, message(first, typed, true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let prompts = user_texts(&state, first);
    assert_eq!(
        prompts[0],
        format!(
            "{typed}\n\n<attached_file path=\"notes.md\">\nremember the milk\n</attached_file>"
        )
    );
    let records: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::NewRecord(record) => Some(record.message),
            _ => None,
        })
        .collect();
    assert!(records.iter().any(|m| m.contains(typed)), "{records:?}");
    assert!(
        !records.iter().any(|m| m.contains("remember the milk")),
        "the record keeps the prompt as typed: {records:?}"
    );
}

#[tokio::test]
async fn a_pasted_image_goes_to_the_model_as_an_image_block() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    let event = message_with_image(first, "[image 1] what is this?", true);
    handle_ui_event(&mut state, event, &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    let prompt = state.idle[&first]
        .messages
        .iter()
        .find(|m| m.role == aura_model::Role::User)
        .unwrap();
    assert!(matches!(
        prompt.content.as_slice(),
        [
            aura_model::ContentBlock::Image { .. },
            aura_model::ContentBlock::Text { text },
        ] if text == "[image 1] what is this?"
    ));
    assert_eq!(
        user_images(&state, first),
        [("image/png".to_string(), "aGk=".to_string())]
    );
}

#[tokio::test]
async fn an_image_for_a_busy_agent_waits_for_the_next_turn() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow start", true), &links).await;
    let event = message_with_image(first, "[image 1] and this", false);
    handle_ui_event(&mut state, event, &links).await;
    assert_eq!(state.queued.len(first), 1, "a turn can't take images");

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;

    assert_eq!(
        user_texts(&state, first),
        ["slow start", "[image 1] and this"]
    );
    assert_eq!(user_images(&state, first).len(), 1);
    let shown = drain(&mut commands).into_iter().any(|(_, c)| {
        matches!(c, UiCommand::TurnStarted { prompt: Some(p) } if p == "[image 1] and this")
    });
    assert!(
        shown,
        "the chat shows the queued message when its turn starts"
    );
}

#[tokio::test]
async fn a_harness_notice_never_attaches_files() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let workspace = state.kernel_for(first).unwrap().agent_workspace();
    std::fs::write(workspace.join("notes.md"), "remember the milk\n").unwrap();

    deliver(
        &mut state,
        first,
        SteerSource::Harness,
        "job log mentions @notes.md".to_string(),
        false,
    )
    .await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(
        lock_registry_agent(&fixture, first).title,
        None,
        "only typed prompts title a chat"
    );

    let prompts = user_texts(&state, first);
    assert!(!prompts[0].contains("remember the milk"), "{prompts:?}");
}

#[tokio::test]
async fn the_first_turn_titles_a_placeholder_named_agent() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "fix the build", true), &links).await;
    // The title lands while the turn runs, not once it has ended.
    let title = wait_for_title(&fixture, first).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(
        lock_registry_agent(&fixture, first).title.as_deref(),
        Some(title.as_str())
    );
    assert_eq!(state.idle[&first].messages.len(), 2);
    let shown = state.hub.agent_summaries();
    let row = shown.iter().find(|a| a.id == first.to_hex()).unwrap();
    assert_eq!(row.name, title);

    handle_ui_event(&mut state, message(first, "and again", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(
        lock_registry_agent(&fixture, first).title.as_deref(),
        Some(title.as_str()),
        "an agent is titled once"
    );

    handle_ui_event(&mut state, message(second, "quick question", true), &links).await;
    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(lock_registry_agent(&fixture, second).title, None);
}

/// Wait for the background title job to name `agent_id`.
async fn wait_for_title(fixture: &Fixture, agent_id: AgentId) -> String {
    for _ in 0..500 {
        if let Some(title) = lock_registry_agent(fixture, agent_id).title {
            return title;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the agent was never titled");
}
