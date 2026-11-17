//! Renders whole screens offscreen with the CPU renderer, so the grid,
//! glyphs and input path are exercised without a display or GPU.
//!
//! Set `AURA_DESKTOP_SNAPSHOTS=<dir>` to also write each screen as a PNG.

use crate::app::{App, Handles, Message, Options};
use crate::chrome::Divider;
use crate::demo::{DIFF, LISTING, REPLY, TEST_OUTPUT};
use crate::grid::Palette;
use crate::tab_bar::Hit;
use crate::theme;
use aura_ui::events::{
    AgentDefinitionSummary, AgentSummary, CallUsage, MessageData, MessageRole, ModelOptionView,
    ProcessKind, ProcessStatus, ProcessSummary, ToolData, UiCommand, UiEvent,
};
use aura_ui_terminal::{ChatTab, Pane, PaneTab};
use iced::advanced::graphics::text::font_system;
use iced::advanced::renderer::{Headless, Style};
use iced::keyboard::{self, key, Key, Location, Modifiers};
use iced::{mouse, Pixels, Point, Size};
use iced_runtime::user_interface::{Cache, UserInterface};
use std::sync::Once;
use std::time::Instant;
use tokio::sync::mpsc;

const WIDTH: u16 = 1320;
const HEIGHT: u16 = 860;

/// The app, plus the session's end of its event channel.
struct Harness {
    app: App,
    events: mpsc::Receiver<UiEvent>,
}

fn harness() -> Harness {
    load_fonts();
    let (events, events_rx) = mpsc::channel(64);
    let (_, commands) = mpsc::channel(16);
    let options = Options {
        theme: "cyber".into(),
        verbose: false,
        tabs_file: None,
    };
    let mut app = App::boot(Handles { events, commands }, &options, Instant::now()).0;
    let _ = app.update(Message::Resized(Size::new(
        f32::from(WIDTH),
        f32::from(HEIGHT),
    )));
    Harness {
        app,
        events: events_rx,
    }
}

/// The keys held for the window's own shortcuts.
fn window_chord() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::LOGO
    } else {
        Modifiers::CTRL | Modifiers::SHIFT
    }
}

fn agent(id: &str, name: &str, depth: usize, project: Option<&str>) -> AgentSummary {
    AgentSummary {
        id: id.into(),
        name: name.into(),
        zns_id: format!("0://{name}"),
        record_count: 42,
        last_active: "12:04:31".into(),
        depth,
        running: depth > 0,
        project: project.map(Into::into),
        ..AgentSummary::default()
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
        self.chord(key, text, Modifiers::empty());
    }

    fn chord(&mut self, key: Key, text: Option<&str>, modifiers: Modifiers) {
        let _ = self.app.update(Message::Key(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers,
            text: text.map(Into::into),
            repeat: false,
        }));
    }

    /// Left-click a box of the tab bar.
    fn click(&mut self, hit: Hit) {
        let position = self.app.bar().spot(hit).expect("the box is drawn");
        for event in [
            mouse::Event::CursorMoved { position },
            mouse::Event::ButtonPressed(mouse::Button::Left),
            mouse::Event::ButtonReleased(mouse::Button::Left),
        ] {
            let _ = self.app.update(Message::Mouse(event));
        }
    }

    /// Press the left button at `from` on the grid's canvas and let go
    /// at `to`.
    fn drag(&mut self, from: Point, to: Point) {
        let (from, to) = (self.app.on_window(from), self.app.on_window(to));
        for event in [
            mouse::Event::CursorMoved { position: from },
            mouse::Event::ButtonPressed(mouse::Button::Left),
            mouse::Event::CursorMoved { position: to },
            mouse::Event::ButtonReleased(mouse::Button::Left),
        ] {
            let _ = self.app.update(Message::Mouse(event));
        }
    }

    /// What the window asked of the session since the last call.
    fn asked(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(format!("{event:?}"));
        }
        out
    }

    /// The session's answer to creating the agent `id`.
    fn created(&mut self, id: &str, name: &str) {
        self.apply([
            UiCommand::SetActiveAgent(id.into()),
            UiCommand::ClearRecords,
            UiCommand::ForAgent {
                agent_id: id.into(),
                command: Box::new(UiCommand::LoadConversation(Vec::new())),
            },
            UiCommand::SetAgents(vec![
                agent("a1", "Agent01", 0, Some("aura-harness")),
                agent("a2", "Reviewer", 1, None),
                agent(id, name, 0, None),
            ]),
            UiCommand::SetActiveAgent(id.into()),
        ]);
    }

    fn text(&self) -> String {
        self.app.screen().text()
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
            user: Some("demo@aura.dev".into()),
            plans: vec![],
        },
        UiCommand::SetAgents(vec![
            agent("a1", "Agent01", 0, Some("aura-harness")),
            agent("a2", "Reviewer", 1, None),
        ]),
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

/// Register the bundled fonts, before anything is drawn.
fn load_fonts() {
    static FONTS: Once = Once::new();
    FONTS.call_once(|| {
        let mut fonts = font_system().write().expect("font system lock");
        for font in theme::font_files() {
            fonts.load_font(font);
        }
    });
}

fn renderer() -> iced::Renderer {
    futures::executor::block_on(<iced::Renderer as Headless>::new(
        theme::mono(),
        Pixels(theme::DEFAULT_FONT_SIZE),
        Some("tiny-skia"),
    ))
    .expect("tiny-skia renderer")
}

/// Draw the app and return its RGBA pixels.
fn render(app: &App) -> Vec<u8> {
    let mut renderer = renderer();
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
    Palette::from_theme(&aura_ui_terminal::Theme::by_name("cyber"))
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
    // Two rows fewer than the window alone holds: the tab bar is above.
    assert_eq!((cols, rows), (155, 45));
}

#[test]
fn the_window_opens_with_one_tab() {
    let mut h = harness();
    seed(&mut h);
    assert_eq!(h.app.tabs().len(), 1);
    assert_eq!(h.app.bar().title(), Some("AURA-HARNESS / AGENT01"));
    assert!(h.app.bar().spot(Hit::Tab(1)).is_none());
    assert!(h.asked().is_empty());
}

#[test]
fn tabs_are_workspaces_with_their_own_projects() {
    let mut h = harness();
    session(&mut h);
    let _ = h.asked();

    h.click(Hit::New);
    assert_eq!(h.asked(), ["CreateAgent(None)"]);
    assert_eq!(h.app.tabs().len(), 1, "the tab opens with its agent");
    h.created("a3", "Agent03");
    assert_eq!(h.app.tabs().len(), 2);
    assert_eq!(h.app.bar().title(), Some("AGENT03"));
    assert!(!h.text().contains("Summarise the crates"));
    assert!(!h.text().contains("Reviewer"), "a project of the first tab");

    h.type_text("a draft for the new agent");
    assert!(h.text().contains("a draft for the new agent"));
    snapshot("tabs", &h);

    h.chord(Key::Named(key::Named::Tab), None, Modifiers::CTRL);
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    assert_eq!(h.app.tabs().front_index(), 0);
    assert!(h.text().contains("Summarise the crates"));
    assert!(h.text().contains("Reviewer"));
    assert!(!h.text().contains("Agent03"), "a project of the second tab");
    assert!(!h.text().contains("a draft for the new agent"));

    h.click(Hit::Tab(1));
    assert_eq!(h.asked(), [r#"FocusAgent("a3")"#]);
    assert!(h.text().contains("a draft for the new agent"));

    h.chord(Key::Character("1".into()), None, Modifiers::ALT);
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    assert_eq!(h.app.tabs().front_index(), 0);
    assert!(h.text().contains("Summarise the crates"));
}

#[test]
fn shortcuts_open_and_close_tabs() {
    let mut h = harness();
    seed(&mut h);
    let _ = h.asked();
    h.chord(Key::Character("T".into()), None, window_chord());
    assert_eq!(h.asked(), ["CreateAgent(None)"]);
    h.created("a3", "Agent03");
    assert_eq!(h.app.tabs().len(), 2);

    // Closing a tab hands its projects to the one before it.
    h.chord(Key::Character("W".into()), None, window_chord());
    assert_eq!(h.app.tabs().len(), 1);
    assert_eq!(h.app.tabs().front().agent(), "a1");
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    assert!(h.text().contains("Agent03"));
    assert!(h.text().contains("Reviewer"));

    h.chord(Key::Character("W".into()), None, window_chord());
    assert_eq!(h.app.tabs().len(), 1, "the last tab stays");
}

#[test]
fn prose_is_set_in_inter_and_code_stays_on_the_grid() {
    let mut h = harness();
    seed(&mut h);
    h.apply([
        UiCommand::StartStreaming,
        UiCommand::AppendText(
            "Hi! That last message looks like a typo. What would you like to do?\n\n\
             - **\"ls\":** I said I'd created a task, but I hadn't done anything. If you \
             want, I can list the files in the project folder now.\n\n\
             Here are some things I can help with in this repo (`aura-harness`): run \
             the build, tests, or `cargo clippy`.\n\n```rust\nfn main() {}\n```"
                .into(),
        ),
        UiCommand::FinishStreaming,
        UiCommand::Complete,
    ]);
    let text = h.app.screen().text();
    assert!(text.contains("the files in the project folder"), "{text}");
    assert!(text.contains("fn main() {}"), "{text}");
    let tui = &h.app.tabs().front().tui;
    assert!(!tui.prose_rows().is_empty(), "the reply's text is prose");
    snapshot("prose", &h);
}

#[test]
fn welcome_screen() {
    let mut h = harness();
    seed(&mut h);
    snapshot("welcome", &h);
}

#[test]
fn panel_tabs_and_dividers_belong_to_the_window() {
    let mut h = harness();
    session(&mut h);
    let chat = |h: &Harness| {
        let tui = &h.app.tabs().front().tui;
        let pane = tui.pane_chrome().iter().find(|c| c.pane == Pane::Chat);
        pane.expect("the chat has a bar").clone()
    };
    let text = h.text();
    assert!(
        !text.contains("TERMINAL"),
        "tab labels are drawn, not cells"
    );

    let terminal = PaneTab::Chat(ChatTab::Terminal);
    let tab = h.app.chrome().spot(terminal).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.app.tabs().front().tui.chat_tab(), ChatTab::Terminal);
    assert!(chat(&h).focused);
    assert_eq!(chat(&h).active, Some(terminal));

    let before = chat(&h).area;
    let band = h
        .app
        .chrome()
        .band(Divider::Agents)
        .expect("agents beside the chat");
    let cell = crate::grid::Metrics::new(theme::DEFAULT_FONT_SIZE)
        .cell
        .width;
    h.drag(band, Point::new(band.x + 10.0 * cell, band.y));
    let after = chat(&h).area;
    assert_eq!(after.right(), before.right());
    assert_eq!(after.x, before.x + 10, "{before:?} → {after:?}");
    snapshot("panes", &h);
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
/// while a long reply streams in small chunks and while typing, building
/// the grid's drawing after each streamed chunk, and full repaints on the
/// CPU renderer. Run with
/// `cargo test --release -p aura-desktop -- --ignored --nocapture replay`.
#[test]
#[ignore = "benchmark"]
fn replay_timings() {
    use iced::widget::canvas::Program;
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
    let renderer = renderer();
    let bounds = iced::Rectangle::with_size(Size::new(f32::from(WIDTH), f32::from(HEIGHT)));
    let mut streaming = Vec::new();
    let mut painting = Vec::new();
    let mut frames = Vec::new();
    let (mut changed_frames, mut changed_rows) = (0, 0);
    for (i, chunk) in text.chunks(12).enumerate() {
        let chunk: String = chunk.iter().collect();
        let before = h.text();
        let t = Instant::now();
        let _ = h
            .app
            .update(Message::Commands(vec![UiCommand::AppendText(chunk)]));
        streaming.push(t.elapsed());
        let after = h.text();
        let rows = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .count();
        changed_frames += usize::from(rows > 0);
        changed_rows += rows;
        let t = Instant::now();
        let _ = Program::<Message>::draw(
            &h.app.grid(),
            &(),
            &renderer,
            &iced::Theme::Dark,
            bounds,
            mouse::Cursor::Unavailable,
        );
        painting.push(t.elapsed());
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
    report("grid drawing per streamed chunk", &mut painting);
    let (_, rows) = h.app.screen().size();
    println!(
        "rows repainted: {changed_rows} over {changed_frames} changed frames of {rows} rows \
         (a whole-grid repaint would be {})",
        changed_frames * usize::from(rows)
    );
    report("full repaint (tiny-skia)", &mut frames);
}

/// One second of a swarm streaming at once: the agent in front writes a
/// reply while the others think, call tools and write theirs. Reports how
/// long the window spends applying it with one command per update (each
/// arriving on its own), about a frame's worth per update, and all at
/// once. Run with
/// `cargo test --release -p aura-desktop -- --ignored --nocapture swarm`.
#[test]
#[ignore = "benchmark"]
fn swarm_timings() {
    const AGENTS: usize = 30;
    const COMMANDS_PER_AGENT: usize = 100;
    let reply: Vec<char> = REPLY.replace("{prompt}", "the crates").chars().collect();
    let mut chunks = reply.chunks(12).cycle();
    let mut second = Vec::new();
    for step in 0..COMMANDS_PER_AGENT {
        second.push(UiCommand::AppendText(
            chunks.next().unwrap().iter().collect(),
        ));
        for i in 1..AGENTS {
            let command = match step % 10 {
                0 => UiCommand::ShowTool(ToolData {
                    id: format!("t{step}"),
                    name: "read_file".into(),
                    args: r#"{"path":"src/lib.rs"}"#.into(),
                }),
                1 => UiCommand::CompleteTool {
                    id: format!("t{}", step - 1),
                    result: LISTING.into(),
                    success: true,
                },
                2..=4 => UiCommand::AppendThinking("weighing the options ".into()),
                _ => UiCommand::AppendText(chunks.next().unwrap().iter().collect()),
            };
            second.push(UiCommand::ForAgent {
                agent_id: format!("b{i:02}"),
                command: Box::new(command),
            });
        }
    }

    let mut agents = vec![agent("a1", "Agent01", 0, Some("aura-harness"))];
    agents
        .extend((1..AGENTS).map(|i| agent(&format!("b{i:02}"), &format!("Agent{i:02}"), 0, None)));
    for per_update in [1, 50, 1024] {
        let mut h = harness();
        seed(&mut h);
        h.apply([UiCommand::SetAgents(agents.clone())]);
        steps(&mut h);
        h.apply([UiCommand::StartStreaming]);
        let mut updates = Vec::new();
        for batch in second.chunks(per_update) {
            let t = Instant::now();
            let _ = h.app.update(Message::Commands(batch.to_vec()));
            updates.push(t.elapsed());
        }
        let busy: std::time::Duration = updates.iter().sum();
        println!(
            "{} commands from {AGENTS} agents, {per_update} per update: busy {:.1} ms of the second",
            second.len(),
            busy.as_secs_f64() * 1e3
        );
        report("  update", &mut updates);
    }
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
