//! What a frame of the chat costs with a long history, while a long
//! reply streams, and what a typed key costs (the key, then the frame it
//! asks for, as the event loop does) in a long chat, with a long input,
//! during a long turn and with many agents listed:
//! `cargo test --release -p aura-ui-terminal --lib -- --ignored --nocapture timings`.

use super::text::Measure;
use crate::events::{
    ActivityData, AgentSummary, MessageData, MessageRole, ProjectSummary, RecordStatus,
    RecordSummary, ToolData, UiCommand,
};
use crate::{App, Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
        "{label}: {} samples, median {:.3} ms, mean {:.3} ms, max {:.3} ms",
        samples.len(),
        ms(samples[samples.len() / 2]),
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

impl Bench {
    /// Type `c` into the input, then draw the frame the key asks for, as
    /// the event loop does, and say how long both took.
    fn key(&mut self, c: char) -> Duration {
        let t = Instant::now();
        self.app
            .handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        self.terminal
            .draw(|frame| super::render(frame, &mut self.app, &self.theme))
            .unwrap();
        t.elapsed()
    }

    /// Type `count` keys and report what each cost.
    fn typing(&mut self, label: &str, count: usize) {
        self.frame();
        let text: Vec<char> = "the quick brown fox jumps over the lazy dog "
            .chars()
            .cycle()
            .take(count)
            .collect();
        let mut keys: Vec<Duration> = text.into_iter().map(|c| self.key(c)).collect();
        report(label, &mut keys);
    }
}

/// `write_file` arguments carrying `bytes` of file content.
fn write_args(turn: usize, bytes: usize) -> String {
    let content = "fn main() { println!(\"hello\"); }\n".repeat(bytes / 34 + 1);
    serde_json::json!({ "path": format!("src/gen_{turn}.rs"), "content": content }).to_string()
}

/// `edit_file` arguments replacing `bytes` of text with a slightly
/// different copy.
fn edit_args(turn: usize, bytes: usize) -> String {
    let old = "let value = compute(input);\n".repeat(bytes / 28 + 1);
    let new = old.replacen("compute", "compute_fast", 40);
    serde_json::json!({ "path": format!("src/edit_{turn}.rs"), "old_text": old, "new_text": new })
        .to_string()
}

/// A turn in flight whose one group of steps has `steps` tool calls, a
/// few of them large writes and edits, the last two still running, the
/// last of those a large write.
fn live_turn(bench: &mut Bench, steps: usize) {
    bench.app.process_command(UiCommand::TurnStarted {
        prompt: Some("Refactor the renderer and write the new modules".into()),
    });
    for i in 0..steps {
        let id = format!("live-{i}");
        let (name, args) = match i % 10 {
            _ if i + 1 == steps => ("write_file", write_args(i, 120_000)),
            3 => ("write_file", write_args(i, 120_000)),
            7 => ("edit_file", edit_args(i, 60_000)),
            5 => (
                "search_code",
                r#"{"pattern":"fn render","path":"crates"}"#.to_string(),
            ),
            _ => (
                "read_file",
                format!(r#"{{"path":"crates/ui/aura-ui-terminal/src/file_{i}.rs"}}"#),
            ),
        };
        bench.app.process_command(UiCommand::ShowTool(ToolData {
            id: id.clone(),
            name: name.into(),
            args,
        }));
        if i + 2 < steps {
            bench.app.process_command(UiCommand::CompleteTool {
                id,
                result: "pub mod app;\npub mod renderer;\n".repeat(20),
                success: true,
            });
        }
    }
}

/// What a typed key costs, the key and the frame after it: in a long
/// chat, with a long input already typed, during a turn with a long group
/// of steps, with many agents listed, and in a session that has all of
/// that and full side panels; then what a frame that only turns the
/// spinners costs there.
#[test]
#[ignore = "benchmark"]
fn typing_timings() {
    let mut bench = Bench::new();
    bench
        .app
        .process_command(UiCommand::LoadConversation(history(2000)));
    bench.typing("2000 messages, idle: key + frame", 200);

    let mut bench = Bench::new();
    bench
        .app
        .process_command(UiCommand::LoadConversation(history(100)));
    let paste = "Here is the log from the failing run, line by line:\n".repeat(300);
    bench.app.handle_paste(&paste);
    bench.typing(
        &format!("a {} character input: key + frame", paste.len()),
        200,
    );

    for steps in [50, 300] {
        let mut bench = Bench::new();
        bench
            .app
            .process_command(UiCommand::LoadConversation(history(500)));
        live_turn(&mut bench, steps);
        bench.typing(&format!("a live turn of {steps} steps: key + frame"), 100);
    }

    let mut bench = Bench::new();
    bench
        .app
        .process_command(UiCommand::LoadConversation(history(100)));
    let agents: Vec<AgentSummary> = (0..300)
        .map(|i| AgentSummary {
            id: format!("{i:064x}"),
            name: format!("Agent {i}"),
            ..AgentSummary::default()
        })
        .collect();
    bench.app.process_command(UiCommand::SetAgents(agents));
    bench.typing("300 agents listed: key + frame", 200);

    let mut bench = Bench::new();
    full_session(&mut bench);
    bench.typing("a full session: key + frame", 200);
    live_turn(&mut bench, 100);
    bench.typing("a full session in a turn: key + frame", 200);
    // While anything runs, a frame is drawn every tick to turn the
    // spinners; a key typed meanwhile waits for the frame being drawn.
    let mut frames: Vec<Duration> = (0..100)
        .map(|_| {
            bench.app.tick();
            bench.frame()
        })
        .collect();
    report("a full session in a turn: spinner frame", &mut frames);
}

/// A record of the kind the Record panel lists, its payload `bytes` long.
fn record(seq: u64, bytes: usize) -> RecordSummary {
    RecordSummary {
        seq,
        timestamp: "12:34:56".into(),
        full_hash: format!("{seq:064x}"),
        hash_suffix: format!("{:04x}", seq % 0xffff),
        tx_kind: if seq % 2 == 0 {
            "ActionResult"
        } else {
            "ToolProposal"
        }
        .into(),
        sender: "agent".into(),
        message: "{\n  \"path\": \"crates/ui/aura-ui-terminal/src/lib.rs\"\n}\n".repeat(bytes / 50),
        action_count: 1,
        effect_status: "1 ok".into(),
        status: RecordStatus::Ok,
        info: "read_file".into(),
        what: "read_file".into(),
        group: "fs".into(),
        detail: "crates/ui/aura-ui-terminal/src/lib.rs".into(),
        error_details: String::new(),
        tx_id: format!("{seq:064x}"),
        agent_id: format!("{:064x}", 1),
        ts_ms: 1_790_000_000_000 + seq * 1000,
        policy: None,
    }
}

/// What a real session holds besides its chat: a full Record panel, a
/// sandbox on this repository, many agents across projects, and an
/// archive.
fn full_session(bench: &mut Bench) {
    let app = &mut bench.app;
    app.process_command(UiCommand::LoadConversation(history(2000)));
    app.process_command(UiCommand::SetRecords(
        (0..10_000).map(|seq| record(seq, 2_000)).collect(),
    ));
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    app.process_command(UiCommand::SetSandboxRoot(repo));
    let projects: Vec<ProjectSummary> = (0..12)
        .map(|i| ProjectSummary {
            id: format!("p{i}"),
            name: format!("project-{i}"),
            folder: format!("/home/user/code/project-{i}"),
            folder_exists: true,
            created: "2026-01-01".into(),
            listed_agents: 25,
            archived_agents: 40,
        })
        .collect();
    app.process_command(UiCommand::SetProjects(projects));
    let agent = |i: usize, archived: bool| AgentSummary {
        id: format!("{:064x}", i + usize::from(archived) * 100_000),
        name: format!("Agent {i}"),
        project_id: Some(format!("p{}", i % 12)),
        record_count: 500,
        last_active: "12:00:00".into(),
        last_active_ms: Some(1_790_000_000_000),
        archived_at_ms: archived.then_some(1_790_000_000_000 - (i as u64) * 3_600_000),
        running: i % 50 == 0,
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(
        (0..300).map(|i| agent(i, false)).collect(),
    ));
    app.process_command(UiCommand::SetArchivedAgents(
        (0..500).map(|i| agent(i, true)).collect(),
    ));
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
