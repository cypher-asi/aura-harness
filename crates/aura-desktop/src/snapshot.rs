//! Renders whole screens offscreen with the CPU renderer, so layout and
//! drawing are exercised without a display or GPU.
//!
//! Set `AURA_DESKTOP_SNAPSHOTS=<dir>` to also write each screen as a PNG.

use crate::app::{App, Handles, Message, Overlay, SideTab};
use crate::demo::{DIFF, LISTING, REPLY, TEST_OUTPUT};
use crate::{theme, view};
use aura_ui_core::events::{
    AgentDefinitionSummary, AgentSummary, CallUsage, MessageData, MessageRole, ModelOptionView,
    ProcessKind, ProcessStatus, ProcessSummary, ToolData, UiCommand,
};
use iced::advanced::graphics::text::font_system;
use iced::advanced::renderer::{Headless, Style};
use iced::{mouse, Pixels, Size};
use iced_runtime::user_interface::{Cache, UserInterface};
use std::sync::Once;
use std::time::Instant;
use tokio::sync::mpsc;

const WIDTH: u16 = 1320;
const HEIGHT: u16 = 860;

fn app() -> App {
    let (events, _) = mpsc::channel(16);
    let (_, commands) = mpsc::channel(16);
    App::boot(Handles { events, commands }, Instant::now()).0
}

fn apply(app: &mut App, commands: impl IntoIterator<Item = UiCommand>) {
    for command in commands {
        app.model.apply(command);
    }
}

/// A whole exchange: history, agent steps, the reply and a shell command.
fn session(app: &mut App) {
    seed(app);
    steps(app);
    reply(app);
}

/// Models, agents, processes and earlier conversation.
fn seed(app: &mut App) {
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
    apply(
        app,
        [
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
        ],
    );
}

fn reply(app: &mut App) {
    apply(
        app,
        [
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
        ],
    );
    app.model.send_command("git status --short");
    apply(
        app,
        [
            UiCommand::CommandFinished {
                output: " \x1b[32mM\x1b[0m README.md\n\x1b[31m??\x1b[0m crates/aura-desktop/"
                    .into(),
                exit_code: None,
            },
            UiCommand::Complete,
        ],
    );
}

/// A user message and the agent's expanded thinking and tool calls.
fn steps(app: &mut App) {
    app.model
        .send_user("Summarise the crates and fix the README");
    app.model.expand_activity = true;
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
    apply(
        app,
        [
            UiCommand::TurnStarted,
            UiCommand::AppendThinking("List the crates, then read the manifest.".into()),
            UiCommand::FinishThinking,
        ],
    );
    apply(
        app,
        tool("t1", "list_files", r#"{"path":"crates"}"#, LISTING),
    );
    apply(
        app,
        tool(
            "t2",
            "edit_file",
            r#"{"path":"README.md","old_str":"fast","new_str":"fast and native"}"#,
            DIFF,
        ),
    );
    apply(
        app,
        tool(
            "t3",
            "run_command",
            r#"{"command":"cargo test -q"}"#,
            TEST_OUTPUT,
        ),
    );
}

/// Draw `app` and return its RGBA pixels.
fn render(app: &App) -> Vec<u8> {
    static FONTS: Once = Once::new();
    FONTS.call_once(|| {
        let mut fonts = font_system().write().expect("font system lock");
        for font in theme::font_files() {
            fonts.load_font(font);
        }
    });
    let mut renderer = futures::executor::block_on(<iced::Renderer as Headless>::new(
        theme::ui(),
        Pixels(theme::TEXT_SIZE),
        Some("tiny-skia"),
    ))
    .expect("tiny-skia renderer");
    let size = Size::new(f32::from(WIDTH), f32::from(HEIGHT));
    let mut ui: UserInterface<'_, Message, iced::Theme, iced::Renderer> =
        UserInterface::build(view::view(app), size, Cache::default(), &mut renderer);
    ui.draw(
        &mut renderer,
        &theme::theme(),
        &Style {
            text_color: theme::TEXT,
        },
        mouse::Cursor::Unavailable,
    );
    renderer.screenshot(
        Size::new(u32::from(WIDTH), u32::from(HEIGHT)),
        1.0,
        theme::BG,
    )
}

/// Render `app`, check something besides the background was drawn, and
/// save it when snapshots are requested.
fn snapshot(name: &str, app: &App) {
    let pixels = render(app);
    assert_eq!(pixels.len(), usize::from(WIDTH) * usize::from(HEIGHT) * 4);
    let bg = theme::BG.into_rgba8();
    let drawn = pixels.chunks(4).filter(|p| p[..3] != bg[..3]).count();
    assert!(drawn > 10_000, "{name}: only {drawn} pixels drawn");
    if let Some(dir) = std::env::var_os("AURA_DESKTOP_SNAPSHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("snapshot dir");
        let image = png(u32::from(WIDTH), u32::from(HEIGHT), &pixels);
        std::fs::write(dir.join(format!("{name}.png")), image).expect("write snapshot");
    }
}

#[test]
fn welcome_screen() {
    snapshot("welcome", &app());
}

#[test]
fn conversation_with_agents() {
    let mut app = app();
    session(&mut app);
    app.visible = 4;
    snapshot("conversation", &app);
}

#[test]
fn agent_steps() {
    let mut app = app();
    seed(&mut app);
    steps(&mut app);
    app.model
        .apply(UiCommand::AppendThinking("Checking the diff…".into()));
    app.side = None;
    app.input = iced::widget::text_editor::Content::with_text("/mo");
    snapshot("steps", &app);
}

#[test]
fn help_screen() {
    let mut app = app();
    app.overlay = Some(Overlay::Help);
    snapshot("help", &app);
}

#[test]
fn processes_panel() {
    let mut app = app();
    session(&mut app);
    app.side = Some(SideTab::Processes);
    snapshot("processes", &app);
}

#[test]
fn model_picker() {
    let mut app = app();
    session(&mut app);
    app.overlay = Some(Overlay::Models);
    snapshot("models", &app);
}

#[test]
fn approval_request() {
    let mut app = app();
    session(&mut app);
    app.model.apply(UiCommand::RequestApproval {
        id: "r1".into(),
        tool: "run_command".into(),
        description: "rm -rf target/".into(),
    });
    snapshot("approval", &app);
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
