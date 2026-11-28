//! What a turn's events show in its agent's chat.

use super::*;
use crate::event_loop::tasks::TaskBoard;
use std::sync::Arc;

/// The commands `events` send to the chat of the agent whose turn it is.
async fn chat_commands(events: Vec<AgentLoopEvent>) -> Vec<UiCommand> {
    let agent = AgentId::new([1; 32]);
    let (commands, mut shown) = mpsc::channel(64);
    let (events_tx, events_rx) = mpsc::channel(64);
    for event in events {
        events_tx.send(event).await.unwrap();
    }
    drop(events_tx);
    let plan = TurnPlan {
        agent,
        tasks: Arc::new(TaskBoard::default()),
        workspace: PathBuf::new(),
        building: None,
    };
    let ui = AgentUi::new(commands, agent);
    forward_agent_events(events_rx, ui, None, None, plan, None).await;
    let mut out = Vec::new();
    while let Ok(command) = shown.try_recv() {
        if let UiCommand::ForAgent { command, .. } = command {
            out.push(*command);
        }
    }
    out
}

fn model_call(iteration: usize) -> AgentLoopEvent {
    AgentLoopEvent::IterationComplete {
        iteration,
        input_tokens: 100,
        output_tokens: 10,
    }
}

#[tokio::test]
async fn each_model_call_writes_a_reply_of_its_own() {
    let commands = chat_commands(vec![
        AgentLoopEvent::TextDelta("FEN6 built it.".into()),
        model_call(0),
        AgentLoopEvent::TextDelta("Nothing is blocked.".into()),
        model_call(1),
    ])
    .await;

    let replies: Vec<&str> = commands
        .iter()
        .filter_map(|command| match command {
            UiCommand::StartStreaming => Some("start"),
            UiCommand::AppendText(text) => Some(text.as_str()),
            UiCommand::FinishStreaming => Some("finish"),
            _ => None,
        })
        .collect();
    assert_eq!(
        replies,
        [
            "start",
            "FEN6 built it.",
            "finish",
            "start",
            "Nothing is blocked.",
            "finish"
        ],
        "the second call's text must not run on from the first's"
    );
}
