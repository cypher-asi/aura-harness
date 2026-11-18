//! What a frame of the chat costs with a long history and while a long
//! reply streams:
//! `cargo test --release -p aura-ui-terminal -- --ignored --nocapture timings`.

use super::text::Measure;
use crate::events::{ActivityData, MessageData, MessageRole, UiCommand};
use crate::{App, Theme};
use ratatui::{backend::TestBackend, Terminal};
use std::time::{Duration, Instant};

/// A window's worth of cells.
const SIZE: (u16, u16) = (155, 45);

const REPLY: &str = "Here's what I found for **the crates**:\n\n\
## Layout\n\n\
| Crate | Role |\n|---|---|\n\
| `aura-ui` | event protocol shared by the frontends |\n\
| `aura-ui-terminal` | the ratatui terminal UI |\n\
| `aura-desktop` | the window, drawn on the GPU |\n\n\
The tests pass, and I updated the README so that it says what each of \
the frontends is for and how a session is started from either of them:\n\n\
```rust\nlet session = aura_app::session::start(&options, mode).await?;\n```\n\n\
- Both UIs share one event loop\n- Fonts are Geist Mono and Inter\n\n";

/// A conversation of `messages` messages: a prompt, two tool calls and a
/// reply, over and over.
fn history(messages: usize) -> Vec<MessageData> {
    let tool = |turn: usize, name: &str| MessageData {
        activity: Some(ActivityData::Tool {
            id: format!("t{turn}-{name}"),
            name: name.into(),
            args: r#"{"path":"crates/ui/aura-ui-terminal/src/lib.rs"}"#.into(),
            result: Some("pub mod app;\npub mod renderer;\npub mod theme;\n".repeat(8)),
            is_error: false,
            duration_ms: Some(120),
            policy: None,
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    };
    let mut out = Vec::with_capacity(messages);
    for turn in 0.. {
        let prompt = format!("Summarise the crates and fix the README, pass {turn}");
        let mut reply = MessageData::new(MessageRole::Assistant, REPLY);
        reply.model = Some("local/qwen3-coder".into());
        out.extend([
            MessageData::new(MessageRole::User, prompt),
            tool(turn, "read_file"),
            tool(turn, "list_files"),
            reply,
        ]);
        if out.len() >= messages {
            break;
        }
    }
    out.truncate(messages);
    out
}

struct Bench {
    app: App,
    theme: Theme,
    terminal: Terminal<TestBackend>,
}

impl Bench {
    fn new() -> Self {
        Self {
            app: App::new(),
            theme: Theme::cyber(),
            terminal: Terminal::new(TestBackend::new(SIZE.0, SIZE.1)).unwrap(),
        }
    }

    /// Render a frame and say how long it took.
    fn frame(&mut self) -> Duration {
        let t = Instant::now();
        self.terminal
            .draw(|frame| super::render(frame, &mut self.app, &self.theme))
            .unwrap();
        t.elapsed()
    }

    /// Bring the chat's lines up to date, as a frame does first, and say
    /// how long that took.
    fn lines(&mut self) -> Duration {
        let spinner = self.app.spinner_char();
        let measure = Measure::prose(self.app.prose_measure(), &self.theme);
        let width = usize::from(self.app.chat_width());
        let (messages, cache, groups) = self.app.chat_line_cache();
        let t = Instant::now();
        cache.update(messages, (width, measure), &self.theme, spinner, &groups);
        t.elapsed()
    }
}

fn report(label: &str, samples: &mut [Duration]) {
    samples.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let total: Duration = samples.iter().sum();
    let n = u32::try_from(samples.len()).unwrap_or(u32::MAX).max(1);
    println!(
        "{label}: {} samples, mean {:.3} ms, max {:.3} ms",
        samples.len(),
        ms(total / n),
        ms(samples[samples.len() - 1]),
    );
}

/// A frame of a conversation at the most messages the UI keeps, and at a
/// twentieth of that: the first frame, which lays every block out, and
/// frames in which nothing changed.
#[test]
#[ignore = "benchmark"]
fn history_timings() {
    for messages in [100, 2000] {
        let mut bench = Bench::new();
        bench
            .app
            .process_command(UiCommand::LoadConversation(history(messages)));
        let first = bench.frame();
        println!(
            "{messages} messages: first frame {:.2} ms",
            first.as_secs_f64() * 1e3
        );
        let mut frames: Vec<Duration> = (0..200).map(|_| bench.frame()).collect();
        report("  frame, nothing changed", &mut frames);
        let mut lines: Vec<Duration> = (0..200).map(|_| bench.lines()).collect();
        report("    of which the chat's lines", &mut lines);
    }
}

/// A reply streaming in small chunks after a long conversation, with a
/// frame after each chunk: what a frame costs as the reply grows.
#[test]
#[ignore = "benchmark"]
fn streaming_timings() {
    for messages in [0, 2000] {
        let mut bench = Bench::new();
        if messages > 0 {
            bench
                .app
                .process_command(UiCommand::LoadConversation(history(messages)));
        }
        bench.app.process_command(UiCommand::StartStreaming);
        bench.frame();
        let text: Vec<char> = REPLY.repeat(160).chars().collect();
        let marks = [2_000, 10_000, 40_000, usize::MAX];
        let (mut mark, mut sent) = (0, 0);
        let mut frames = Vec::new();
        println!("a reply streaming after {messages} messages:");
        for chunk in text.chunks(12) {
            bench
                .app
                .process_command(UiCommand::AppendText(chunk.iter().collect()));
            sent += chunk.len();
            frames.push(bench.frame());
            if sent >= marks[mark] {
                report(
                    &format!("  frame, up to {} characters", marks[mark]),
                    &mut frames,
                );
                frames.clear();
                mark += 1;
            }
        }
        report(&format!("  frame, up to {sent} characters"), &mut frames);
    }
}
