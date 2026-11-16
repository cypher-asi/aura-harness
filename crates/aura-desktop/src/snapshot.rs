//! Renders whole screens offscreen with the CPU renderer, so the grid,
//! glyphs and input path are exercised without a display or GPU.
//!
//! Set `AURA_DESKTOP_SNAPSHOTS=<dir>` to also write each screen as a PNG.

use crate::app::{App, Handles, Message, Options};
use crate::demo::{DIFF, LISTING, REPLY, TEST_OUTPUT};
use crate::grid::Palette;
use crate::theme;
use aura_ui_core::events::{
    AgentDefinitionSummary, AgentSummary, CallUsage, MessageData, MessageRole, ModelOptionView,
    ProcessKind, ProcessStatus, ProcessSummary, ToolData, UiCommand, UiEvent,
};
use iced::advanced::graphics::text::font_system;
use iced::advanced::renderer::{Headless, Style};
use iced::keyboard::{self, key, Key, Location, Modifiers};
use iced::{mouse, Pixels, Size};
use iced_runtime::user_interface::{Cache, UserInterface};
use std::sync::Once;
use std::time::Instant;
use tokio::sync::mpsc;

const WIDTH: u16 = 1320;
const HEIGHT: u16 = 860;

/// The app, plus the end of its event channel so sends succeed.
struct Harness {
    app: App,
    _events: mpsc::Receiver<UiEvent>,
}

fn harness() -> Harness {
    let (events, events_rx) = mpsc::channel(64);
    let (_, commands) = mpsc::channel(16);
    let options = Options {
        theme: "cyber".into(),
        verbose: false,
    };
    let mut app = App::boot(Handles { events, commands }, &options, Instant::now()).0;
    let _ = app.update(Message::Resized(Size::new(
        f32::from(WIDTH),
        f32::from(HEIGHT),
    )));
    Harness {
        app,
        _events: events_rx,
    }
}

impl Harness {
    fn apply(&mut self, commands: impl IntoIterator<Item = UiCommand>) {
        let _ = self
            .app
            .update(Message::Commands(commands.into_iter().collect()));
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let typed = c.to_string();
            self.key(Key::Character(typed.as_str().into()), Some(&typed));
        }
    }

    fn press(&mut self, named: key::Named) {
        self.key(Key::Named(named), None);
    }

    fn key(&mut self, key: Key, text: Option<&str>) {
        let _ = self.app.update(Message::Key(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: text.map(Into::into),
            repeat: false,
        }));
    }

    fn submit(&mut self, text: &str) {
        self.type_text(text);
        self.press(key::Named::Enter);
    }
}

/// Models, agents, processes and earlier conversation.
fn seed(h: &mut Harness) {
    let model =
        |id: &str, label: &str, group: &str, provider: Option<&str>, available| ModelOptionView {
            id: id.into(),
            label: label.into(),
            local: provider.is_none(),
            installed: true,
            group: group.into(),
            provider: provider.map(Into::into),
            available,
        };
    let agent = |id: &str, name: &str, depth: usize| AgentSummary {
        id: id.into(),
        name: name.into(),
        zns_id: format!("0://{name}"),
        record_count: 42,
        last_active: "12:04:31".into(),
        depth,
        running: depth > 0,
        project: (depth == 0).then(|| "aura-harness".to_string()),
        ..AgentSummary::default()
    };
    let process = |id: &str, label: &str, status, kind, started_ms, ended_ms| ProcessSummary {
        id: id.into(),
        kind,
        label: label.into(),
        status,
        owner_name: "Agent01".into(),
        started_ms,
        ended_ms,
        jump_agent_id: "a1".into(),
        killable: true,
        ..ProcessSummary::default()
    };
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default();
    let mut greeting = MessageData::new(
        MessageRole::Assistant,
        "AURA records every step as a **transaction**.",
    );
    greeting.model = Some("local/qwen3-coder".into());
    h.apply([
        UiCommand::SetModels {
            options: vec![
                model("local/qwen3-coder", "Qwen3 Coder", "Local", None, true),
                model("local/gpt-oss", "gpt-oss 20B", "Local", None, true),
                model(
                    "claude/opus",
                    "Claude Opus",
                    "Claude plan",
                    Some("claude"),
                    false,
                ),
            ],
            current: "local/qwen3-coder".into(),
            route: None,
            user: Some("demo@aura.dev".into()),
            plans: vec![],
        },
        UiCommand::SetAgents(vec![agent("a1", "Agent01", 0), agent("a2", "Reviewer", 1)]),
        UiCommand::SetActiveAgent("a1".into()),
        UiCommand::SetAgentDefinitions(vec![AgentDefinitionSummary {
            name: "reviewer".into(),
            description: "Reviews a diff for bugs and missing tests.".into(),
        }]),
        UiCommand::SetApiStatus {
            url: Some("http://127.0.0.1:8787".into()),
            active: true,
        },
        UiCommand::SetProcesses(vec![
            process(
                "p0",
                "Agent01 turn",
                ProcessStatus::Exited,
                ProcessKind::Turn,
                now - 90_000,
                Some(now - 70_000),
            ),
            process(
                "p1",
                "cargo watch",
                ProcessStatus::Idle,
                ProcessKind::Shell,
                now - 50_000,
                None,
            ),
            process(
                "p2",
                "cargo test",
                ProcessStatus::Failed,
                ProcessKind::Shell,
                now - 30_000,
                Some(now - 20_000),
            ),
        ]),
        UiCommand::LoadConversation(vec![
            MessageData::new(MessageRole::User, "What does this project do?"),
            greeting,
        ]),
    ]);
}

/// A typed request and the agent's thinking and tool calls.
fn steps(h: &mut Harness) {
    h.submit("Summarise the crates and fix the README");
    let tool = |id: &str, name: &str, args: &str, result: &str| {
        [
            UiCommand::ShowTool(ToolData {
                id: id.into(),
                name: name.into(),
                args: args.into(),
            }),
            UiCommand::ToolPolicy {
                id: id.into(),
                tool: name.into(),
                decision: "allowed by mode.agent".into(),
            },
            UiCommand::CompleteTool {
                id: id.into(),
                result: result.into(),
                success: true,
            },
        ]
    };
    h.apply([
        UiCommand::TurnStarted { prompt: None },
        UiCommand::AppendThinking("List the crates, then read the manifest.".into()),
        UiCommand::FinishThinking,
    ]);
    h.apply(tool("t1", "list_files", r#"{"path":"crates"}"#, LISTING));
    h.apply(tool(
        "t2",
        "edit_file",
        r#"{"path":"README.md","old_str":"fast","new_str":"fast and native"}"#,
        DIFF,
    ));
    h.apply(tool(
        "t3",
        "run_command",
        r#"{"command":"cargo test -q"}"#,
        TEST_OUTPUT,
    ));
}

fn reply(h: &mut Harness) {
    h.apply([
        UiCommand::StartStreaming,
        UiCommand::AppendText(REPLY.replace("{prompt}", "the crates")),
        UiCommand::FinishStreaming,
        UiCommand::SetReplyModel {
            model: "local/qwen3-coder".into(),
            usage: Some(CallUsage {
                tokens: 4_812,
                cost_usd: None,
            }),
        },
        UiCommand::Complete,
    ]);
}

fn session(h: &mut Harness) {
    seed(h);
    steps(h);
    reply(h);
}

/// Draw the app and return its RGBA pixels.
fn render(app: &App) -> Vec<u8> {
    static FONTS: Once = Once::new();
    FONTS.call_once(|| {
        let mut fonts = font_system().write().expect("font system lock");
        for font in theme::font_files() {
            fonts.load_font(font);
        }
    });
    let mut renderer = futures::executor::block_on(<iced::Renderer as Headless>::new(
        theme::mono(),
        Pixels(theme::DEFAULT_FONT_SIZE),
        Some("tiny-skia"),
    ))
    .expect("tiny-skia renderer");
    let size = Size::new(f32::from(WIDTH), f32::from(HEIGHT));
    let palette = palette();
    let mut ui: UserInterface<'_, Message, iced::Theme, iced::Renderer> =
        UserInterface::build(app.view(), size, Cache::default(), &mut renderer);
    ui.draw(
        &mut renderer,
        &iced::Theme::Dark,
        &Style {
            text_color: palette.fg,
        },
        mouse::Cursor::Unavailable,
    );
    renderer.screenshot(
        Size::new(u32::from(WIDTH), u32::from(HEIGHT)),
        1.0,
        palette.bg,
    )
}

fn palette() -> Palette {
    Palette::from_theme(&aura_terminal::Theme::by_name("cyber"))
}

/// Render, check something besides the background was drawn, and save the
/// screen when snapshots are requested.
fn snapshot(name: &str, h: &Harness) {
    let pixels = render(&h.app);
    assert_eq!(pixels.len(), usize::from(WIDTH) * usize::from(HEIGHT) * 4);
    let bg = palette().bg.into_rgba8();
    let drawn = pixels.chunks(4).filter(|p| p[..3] != bg[..3]).count();
    assert!(drawn > 10_000, "{name}: only {drawn} pixels drawn");
    if let Some(dir) = std::env::var_os("AURA_DESKTOP_SNAPSHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("snapshot dir");
        let image = png(u32::from(WIDTH), u32::from(HEIGHT), &pixels);
        std::fs::write(dir.join(format!("{name}.png")), image).expect("write snapshot");
        std::fs::write(dir.join(format!("{name}.txt")), h.app.screen().text()).expect("write text");
    }
}

#[test]
fn the_grid_fills_the_window() {
    let h = harness();
    let (cols, rows) = h.app.screen().size();
    assert_eq!((cols, rows), (155, 47));
}

#[test]
fn welcome_screen() {
    let mut h = harness();
    seed(&mut h);
    snapshot("welcome", &h);
}

#[test]
fn conversation() {
    let mut h = harness();
    session(&mut h);
    assert!(h.app.screen().text().contains("Summarise the crates"));
    snapshot("conversation", &h);
}

#[test]
fn agent_working() {
    let mut h = harness();
    seed(&mut h);
    steps(&mut h);
    h.apply([UiCommand::AppendThinking("Checking the diff…".into())]);
    for _ in 0..3 {
        let _ = h.app.update(Message::Tick(Instant::now()));
    }
    snapshot("working", &h);
}

#[test]
fn slash_menu() {
    let mut h = harness();
    seed(&mut h);
    h.type_text("/");
    snapshot("slash", &h);
}

#[test]
fn model_picker() {
    let mut h = harness();
    session(&mut h);
    h.submit("/model");
    snapshot("models", &h);
}

#[test]
fn approval_request() {
    let mut h = harness();
    session(&mut h);
    h.apply([UiCommand::RequestApproval {
        id: "r1".into(),
        tool: "run_command".into(),
        description: "rm -rf target/".into(),
        always: "`rm` commands in this project".into(),
    }]);
    snapshot("approval", &h);
}

/// Replays a session the way a model streams it and reports timings:
/// boot to first grid, each update (terminal UI render plus grid diff)
/// while a long reply streams in small chunks and while typing, and full
/// repaints on the CPU renderer. Run with
/// `cargo test --release -p aura-desktop -- --ignored --nocapture replay`.
#[test]
#[ignore = "benchmark"]
fn replay_timings() {
    let started = Instant::now();
    let mut h = harness();
    let boot = started.elapsed();
    seed(&mut h);
    steps(&mut h);
    h.apply([UiCommand::StartStreaming]);

    let text: Vec<char> = REPLY
        .replace("{prompt}", "the crates")
        .repeat(8)
        .chars()
        .collect();
    let mut streaming = Vec::new();
    let mut frames = Vec::new();
    for (i, chunk) in text.chunks(12).enumerate() {
        let chunk: String = chunk.iter().collect();
        let t = Instant::now();
        let _ = h
            .app
            .update(Message::Commands(vec![UiCommand::AppendText(chunk)]));
        streaming.push(t.elapsed());
        if i % 25 == 0 {
            let t = Instant::now();
            let _ = render(&h.app);
            frames.push(t.elapsed());
        }
    }
    h.apply([UiCommand::FinishStreaming, UiCommand::Complete]);

    let mut typing = Vec::new();
    for c in "how are the crates tested?".chars() {
        let t = Instant::now();
        h.type_text(&c.to_string());
        typing.push(t.elapsed());
    }

    println!("boot to first grid: {:.2} ms", boot.as_secs_f64() * 1e3);
    report("update while streaming", &mut streaming);
    report("update per keystroke", &mut typing);
    report("full repaint (tiny-skia)", &mut frames);
}

fn report(label: &str, samples: &mut [std::time::Duration]) {
    samples.sort();
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
    let total: std::time::Duration = samples.iter().sum();
    let n = u32::try_from(samples.len()).unwrap_or(u32::MAX).max(1);
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!(
        "{label}: {} samples, mean {:.3} ms, p99 {:.3} ms, max {:.3} ms",
        samples.len(),
        ms(total / n),
        ms(p99),
        ms(samples[samples.len() - 1]),
    );
}

/// Uncompressed PNG, enough for snapshots without an image dependency.
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<_> = raw.chunks(0xffff).collect();
    for (i, block) in blocks.iter().enumerate() {
        let len = u16::try_from(block.len()).expect("block fits");
        zlib.push(u8::from(i + 1 == blocks.len()));
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(*b"IHDR", header), (*b"IDAT", zlib), (*b"IEND", Vec::new())] {
        out.extend_from_slice(&u32::try_from(data.len()).expect("chunk fits").to_be_bytes());
        let start = out.len();
        out.extend_from_slice(&kind);
        out.extend_from_slice(&data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
