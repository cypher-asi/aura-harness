//! A window holding many messages.

use super::{long_reply, report, seed, stage, type_question, Stage};
use crate::demo::{LISTING, REPLY};
use crate::geom::Size;
use aura_ui::events::{ActivityData, MessageData, MessageRole, UiCommand};
use std::time::{Duration, Instant};

/// A conversation of `messages` messages: a prompt, two tool calls and a
/// reply, over and over.
fn history(messages: usize) -> Vec<MessageData> {
    let tool = |turn: usize, name: &str| MessageData {
        activity: Some(ActivityData::Tool {
            id: format!("t{turn}-{name}"),
            name: name.into(),
            args: r#"{"path":"crates"}"#.into(),
            result: Some(LISTING.into()),
            is_error: false,
            duration_ms: Some(120),
            policy: None,
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    };
    let mut out = Vec::with_capacity(messages);
    while out.len() < messages {
        let turn = out.len() / 4;
        let prompt = format!("Summarise the crates and fix the README, pass {turn}");
        let mut reply = MessageData::new(
            MessageRole::Assistant,
            REPLY.replace("{prompt}", "the crates"),
        );
        reply.model = Some("local/qwen3-coder".into());
        out.extend([
            MessageData::new(MessageRole::User, prompt),
            tool(turn, "read_file"),
            tool(turn, "list_files"),
            reply,
        ]);
    }
    out.truncate(messages);
    out
}

/// A frame after each of ten narrower widths.
fn new_widths(stage: &mut Stage) -> Vec<Duration> {
    let mut widths = Vec::new();
    for step in 1..=10_u8 {
        let wide = 1320.0 - 12.0 * f32::from(step);
        let t = Instant::now();
        stage.shell.resized(Size::new(wide, 860.0), 1.0);
        stage.shell.frame(stage.now);
        widths.push(t.elapsed());
    }
    widths
}

/// A long reply streamed in small chunks with a frame after each,
/// reported as it grows past each mark.
fn stream_growing(stage: &mut Stage) {
    let text = long_reply(160);
    let marks = [2_000, 10_000, 40_000, usize::MAX];
    let (mut mark, mut sent) = (0, 0);
    let mut frames = Vec::new();
    for chunk in text.chunks(12) {
        let t = Instant::now();
        stage
            .shell
            .commands([UiCommand::AppendText(chunk.iter().collect())]);
        stage.shell.frame(stage.now);
        frames.push(t.elapsed());
        sent += chunk.len();
        if sent >= marks[mark] {
            let label = format!(
                "  streamed chunk and its frame, up to {} characters",
                marks[mark]
            );
            report(&label, &mut frames);
            frames.clear();
            mark += 1;
        }
    }
    let label = format!("  streamed chunk and its frame, up to {sent} characters");
    report(&label, &mut frames);
}

/// The window with the most messages the UI keeps, and with a twentieth
/// of that: the first frame, which lays every message out, a key and
/// its frame, a frame after the window changed width, which lays them
/// all out again, and a reply streaming in small chunks with a frame
/// after each, as it grows.
#[test]
#[ignore = "benchmark"]
fn history_timings() {
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    for messages in [100, 2000] {
        let mut stage = stage();
        seed(&mut stage);
        stage.shell.frame(stage.now);
        stage
            .shell
            .commands([UiCommand::LoadConversation(history(messages))]);
        let t = Instant::now();
        stage.shell.frame(stage.now);
        println!("{messages} messages: first frame {:.2} ms", ms(t.elapsed()));

        let mut typing = type_question(&mut stage);
        report("  key and its frame", &mut typing);
        let mut widths = new_widths(&mut stage);
        report("  frame at a new width", &mut widths);

        stage.apply([UiCommand::StartStreaming]);
        stream_growing(&mut stage);
    }
}
