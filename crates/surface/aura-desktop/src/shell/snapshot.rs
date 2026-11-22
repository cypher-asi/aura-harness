//! Whole screens of the window, drawn offscreen by the CPU rasterizer,
//! so the terminal UI, the painter and the input path are exercised
//! together without a display or a GPU; and the scenes the benchmarks
//! in [`super::bench`] time.
//!
//! Set `AURA_DESKTOP_SNAPSHOTS=<dir>` to also write each screen as a PNG
//! with its text beside it.
//!
//! Set `AURA_DESKTOP_GPU=1` on a machine with a graphics adapter to draw
//! each screen with [`crate::gpu`] as well, and check it against the CPU
//! rasterizer's picture of the same quads.

use super::keys::Press;
use super::tests::{stage, Stage};
use super::TICK;
use crate::chrome::Divider;
use crate::demo::{DIFF, LISTING, REPLY, TEST_OUTPUT};
use crate::geom::{rgba, Point};
use crate::gpu::Offscreen;
use crate::router::testing::pick_move;
use crate::tab_bar::Hit;
use crate::{raster, text};
use aura_ui::events::{
    AgentDefinitionSummary, AgentSummary, CallUsage, FlowKind, FlowLane, FlowNode, FlowUsage,
    MessageData, MessageRole, ModelOptionView, ProcessKind, ProcessStatus, ProcessSummary,
    SourceUsage, TodoState, TodoView, ToolData, TraceNodeStatus, UiCommand,
};
use aura_ui_terminal::{AgentsTab, ChatTab, Pane, PaneTab, RecordsTab};
use winit::event::MouseButton;
use winit::keyboard::{ModifiersState, NamedKey};

/// Channel levels the GPU may be off the CPU rasterizer by: both blend
/// the same quads, and round differently.
const GPU_CLOSE: u8 = 1;

pub fn agent(id: &str, name: &str, depth: usize, project: Option<&str>) -> AgentSummary {
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

impl Stage {
    /// Apply `commands` and draw the frame they lead to.
    pub fn apply(&mut self, commands: impl IntoIterator<Item = UiCommand>) {
        self.shell.commands(commands);
        self.shell.frame(self.now);
    }

    /// Type `text` and press Enter.
    pub fn submit(&mut self, text: &str) {
        self.type_text(text);
        self.shell
            .key_pressed(&Press::of(NamedKey::Enter), self.now);
        self.shell.frame(self.now);
    }

    /// The session's answer to creating the agent `id`, beside the ones
    /// [`seed`] lists.
    fn joined(&mut self, id: &str, name: &str) {
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

    /// The grid of the tab in front as last drawn, a line per row.
    fn screen(&mut self) -> String {
        self.shell.frame(self.now);
        self.shell.text()
    }
}

/// Models, agents, processes and earlier conversation.
pub fn seed(h: &mut Stage) {
    let model =
        |id: &str, label: &str, group: &str, provider: Option<&str>, available| ModelOptionView {
            id: id.into(),
            label: label.into(),
            local: provider.is_none(),
            installed: true,
            group: group.into(),
            provider: provider.map(Into::into),
            available,
            context_window: None,
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
                model(
                    "openrouter/anthropic/claude-sonnet-4.6",
                    "Claude Sonnet 4.6",
                    "OpenRouter · anthropic",
                    Some("openrouter"),
                    false,
                ),
            ],
            current: "local/qwen3-coder".into(),
            route: None,
            user: Some("demo@aura.dev".into()),
            plans: vec![],
            thinking: None,
        },
        UiCommand::SetAgents(vec![
            agent("a1", "Agent01", 0, Some("aura-harness")),
            agent("a2", "Reviewer", 1, None),
        ]),
        UiCommand::SetActiveAgent("a1".into()),
        UiCommand::SetAgentDefinitions(vec![AgentDefinitionSummary {
            name: "reviewer".into(),
            description: "Reviews a diff for bugs and missing tests.".into(),
            abbreviation: None,
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
pub fn steps(h: &mut Stage) {
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

fn reply(h: &mut Stage) {
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
            sources: vec![SourceUsage {
                label: "local".into(),
                tokens: 4_812,
                cost_usd: None,
            }],
        },
        UiCommand::Complete,
    ]);
}

fn session(h: &mut Stage) {
    seed(h);
    steps(h);
    reply(h);
}

/// Draw the window, check something besides the background was drawn,
/// and save the screen when snapshots are asked for.
fn snapshot(name: &str, stage: &mut Stage) {
    let scene = stage.shell.frame(stage.now).clone();
    let mut pixels = Vec::new();
    let on_gpu = std::env::var_os("AURA_DESKTOP_GPU").is_some();
    let drawn = text::with(|text| {
        raster::paint(&scene, text.atlas(), &mut pixels);
        on_gpu.then(|| {
            let mut gpu = Offscreen::new().expect("AURA_DESKTOP_GPU needs a graphics adapter");
            let close = GPU_CLOSE + u8::from(gpu.is_software());
            (gpu.read(&scene, text.atlas_mut()), close)
        })
    });
    assert_eq!(
        pixels.len(),
        scene.width as usize * scene.height as usize * 4
    );
    let bg = rgba(stage.shell.palette.bg);
    let inked = pixels.chunks(4).filter(|p| p[..3] != bg[..3]).count();
    assert!(inked > 10_000, "{name}: only {inked} pixels drawn");
    if let Some((drawn, close)) = drawn {
        assert_eq!(
            drawn.len(),
            pixels.len(),
            "{name}: the GPU drew another size"
        );
        let gaps = drawn.iter().zip(&pixels).map(|(a, b)| a.abs_diff(*b));
        let worst = gaps.max().unwrap_or(0);
        println!("{name}: the GPU is at most {worst} levels off the CPU");
        assert!(worst <= close, "{name}: the GPU is {worst} levels off");
    }
    if let Some(dir) = std::env::var_os("AURA_DESKTOP_SNAPSHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("snapshot dir");
        let image = raster::png(scene.width, scene.height, &pixels);
        std::fs::write(dir.join(format!("{name}.png")), image).expect("write snapshot");
        std::fs::write(dir.join(format!("{name}.txt")), stage.shell.text()).expect("write text");
    }
}

#[test]
fn the_window_opens_with_one_tab() {
    let mut h = stage();
    seed(&mut h);
    assert_eq!(h.shell.tabs.len(), 1);
    assert_eq!(h.shell.bar.title(), Some("AURA-HARNESS / AGENT01"));
    assert!(h.shell.bar.spot(Hit::Tab(1)).is_none());
    assert!(h.asked().is_empty());
}

#[test]
fn tabs_are_workspaces_with_their_own_projects() {
    let mut h = stage();
    session(&mut h);
    let _ = h.asked();

    h.click(Hit::New, MouseButton::Left);
    assert!(h.asked().is_empty(), "nothing until a folder is picked");
    h.pick_folder();
    assert_eq!(h.shell.tabs.len(), 1, "the tab opens with its project");
    h.joined("a3", "Agent03");
    assert_eq!(h.shell.tabs.len(), 2);
    assert_eq!(h.shell.bar.title(), Some("AGENT03"));
    assert!(!h.screen().contains("Here's what I found"));
    assert!(
        !h.screen().contains("Reviewer"),
        "a project of the first tab"
    );

    h.type_text("a draft for the new agent");
    assert!(h.screen().contains("a draft for the new agent"));
    snapshot("tabs", &mut h);

    h.key(ModifiersState::CONTROL, Press::of(NamedKey::Tab));
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    assert_eq!(h.shell.tabs.front_index(), 0);
    let first = h.screen();
    assert!(first.contains("Here's what I found"));
    assert!(first.contains("Reviewer"));
    assert!(!first.contains("Agent03"), "a project of the second tab");
    assert!(!first.contains("a draft for the new agent"));

    h.click(Hit::Tab(1), MouseButton::Left);
    assert_eq!(h.asked(), [r#"FocusAgent("a3")"#]);
    assert!(h.screen().contains("a draft for the new agent"));

    h.key(ModifiersState::ALT, Press::typing("1"));
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    assert_eq!(h.shell.tabs.front_index(), 0);
    assert!(h.screen().contains("Here's what I found"));
}

#[test]
fn tabs_drag_into_a_new_order() {
    let mut h = stage();
    seed(&mut h);
    h.click(Hit::New, MouseButton::Left);
    h.pick_folder();
    h.joined("a3", "Agent03");
    let _ = h.asked();
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a1"], vec!["a3"]]);

    h.drag_tab(Hit::Tab(1), Hit::Tab(0));
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a3"], vec!["a1"]]);
    assert_eq!(h.shell.tabs.front_index(), 0);
    assert_eq!(h.shell.bar.title(), Some("AGENT03"));
    assert!(h.asked().is_empty());

    // Numbers pick tabs by their new places.
    h.key(ModifiersState::ALT, Press::typing("2"));
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
}

#[test]
fn a_tab_dragged_while_a_move_closes_another_stays_the_one_dragged() {
    let mut h = stage();
    seed(&mut h);
    h.click(Hit::New, MouseButton::Left);
    h.pick_folder();
    h.joined("a3", "Agent03");
    h.click(Hit::New, MouseButton::Left);
    h.pick_folder();
    h.apply([
        UiCommand::SetActiveAgent("a4".into()),
        UiCommand::SetAgents(vec![
            agent("a1", "Agent01", 0, Some("aura-harness")),
            agent("a2", "Reviewer", 1, None),
            agent("a3", "Agent03", 0, None),
            agent("a4", "Agent04", 0, None),
        ]),
        UiCommand::SetActiveAgent("a4".into()),
    ]);
    let tabs = || [vec!["a1"], vec!["a3"], vec!["a4"]];
    assert_eq!(h.shell.tabs.arrangement(), tabs());

    // a1's tab asks to move its project to a4's: the next update does it
    // and closes a1's tab.
    h.shell.frame(h.now);
    let first = h.shell.tabs.id_at(0).expect("three tabs");
    let tab = h.shell.tabs.by_id_mut(first).expect("open");
    pick_move(&mut tab.tui, "Reviewer", "Workspace 3");
    assert_eq!(h.shell.tabs.arrangement(), tabs(), "not yet");

    // Pressing a3's tab is that update.
    let a3 = h.shell.bar.spot(Hit::Tab(1)).expect("drawn");
    h.shell.pointer_moved(a3);
    h.shell.button(MouseButton::Left, true, h.now);
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a3"], vec!["a4", "a1"]]);

    // Dragged past a4's, a3's tab goes after it.
    h.shell.frame(h.now);
    let last = h.shell.bar.spot(Hit::Tab(1)).expect("drawn");
    h.shell.pointer_moved(last);
    h.shell.button(MouseButton::Left, false, h.now);
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a4", "a1"], vec!["a3"]]);
    assert_eq!(h.shell.tabs.front().agent(), "a3");
}

#[test]
fn a_closed_tab_hands_its_projects_to_the_one_before_it() {
    let mut h = stage();
    seed(&mut h);
    let _ = h.asked();
    let held = ModifiersState::CONTROL | ModifiersState::SHIFT;
    h.key(held, Press::typing("T"));
    h.pick_folder();
    h.joined("a3", "Agent03");
    assert_eq!(h.shell.tabs.len(), 2);

    h.key(held, Press::typing("W"));
    assert_eq!(h.shell.tabs.len(), 1);
    assert_eq!(h.shell.tabs.front().agent(), "a1");
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    let all = h.screen();
    assert!(all.contains("Agent03"));
    assert!(all.contains("Reviewer"));
}

#[test]
fn prose_is_set_in_inter_and_code_stays_on_the_grid() {
    let mut h = stage();
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
    let text = h.screen();
    assert!(text.contains("files in the project folder"), "{text}");
    assert!(text.contains("fn main() {}"), "{text}");
    assert!(!h.shell.prose.is_empty(), "the reply's text is prose");
    snapshot("prose", &mut h);
}

#[test]
fn the_chat_slides_half_a_row_up() {
    use winit::dpi::PhysicalPosition;
    use winit::event::MouseScrollDelta;

    let mut h = stage();
    seed(&mut h);
    let conversation: Vec<MessageData> = (0..30)
        .flat_map(|turn| {
            [
                MessageData::new(
                    MessageRole::User,
                    format!("Question {turn}: what runs next?"),
                ),
                MessageData::new(
                    MessageRole::Assistant,
                    format!(
                        "Turn {turn}: the kernel records the step, checks it against the \
                         policy and runs it. Then `cargo test` passes.\n\n- one\n- two"
                    ),
                ),
            ]
        })
        .collect();
    h.apply([UiCommand::LoadConversation(conversation)]);
    let (width, height) = (h.shell.window.width, h.shell.window.height);
    h.shell.pointer_moved(Point::new(width / 2.0, height / 2.0));
    h.shell.frame(h.now);
    let row = f64::from(h.shell.metrics.cell.height / super::glide::TOUCHPAD_GAIN);
    for _ in 0..41 {
        h.shell.wheel(
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, row * 1.5)),
            h.now,
        );
        h.shell.frame(h.now);
    }
    assert!(h.shell.tabs.front().tui.scroll_offset() > 0);
    snapshot("chat-sliding", &mut h);
}

#[test]
fn welcome_screen() {
    let mut h = stage();
    seed(&mut h);
    snapshot("welcome", &mut h);
}

#[test]
fn panels_with_a_tab_picked_and_a_divider_moved() {
    let mut h = stage();
    session(&mut h);
    let terminal = PaneTab::Chat(ChatTab::Terminal);
    let tab = h.shell.chrome.spot(terminal).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.shell.tabs.front().tui.chat_tab(), ChatTab::Terminal);
    let band = h.shell.chrome.band(Divider::Agents);
    let band = band.expect("agents beside the chat");
    let cell = h.shell.metrics.cell.width;
    h.drag(band, Point::new(band.x + 10.0 * cell, band.y));
    snapshot("panes", &mut h);
}

#[test]
fn an_empty_archive_says_how_agents_get_there() {
    let mut h = stage();
    session(&mut h);
    let archive = PaneTab::Agents(AgentsTab::Archive);
    let tab = h.shell.chrome.spot(archive).expect("the tab is drawn");
    h.drag(tab, tab);
    let screen = h.screen();
    assert!(screen.contains("  No archived agents."), "{screen}");
    snapshot("archive", &mut h);
}

#[test]
fn the_kernel_bar_opens_the_members_of_the_run() {
    let mut h = stage();
    session(&mut h);
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default();
    h.apply([UiCommand::SetProcesses(vec![ProcessSummary {
        id: "task:a2".into(),
        kind: ProcessKind::Task,
        label: "Reviewer (reviewer)".into(),
        detail: "Review the README change".into(),
        status: ProcessStatus::Running,
        owner_agent_id: "a1".into(),
        owner_name: "Agent01".into(),
        started_ms: now - 12_000,
        jump_agent_id: "a2".into(),
        killable: true,
        ..ProcessSummary::default()
    }])]);
    let members = PaneTab::Records(RecordsTab::Members);
    let tab = h.shell.chrome.spot(members).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.shell.tabs.front().tui.records_tab(), RecordsTab::Members);
    let screen = h.screen();
    assert!(
        screen.contains("Agent01") && screen.contains("Reviewer"),
        "{screen}"
    );
    snapshot("members", &mut h);
}

#[test]
fn conversation() {
    let mut h = stage();
    session(&mut h);
    assert!(h.screen().contains("Here's what I found"));
    snapshot("conversation", &mut h);
}

#[test]
fn agent_working() {
    let mut h = stage();
    seed(&mut h);
    steps(&mut h);
    let todo = |content: &str, state| TodoView {
        content: content.into(),
        state,
        assignee: None,
        task: None,
    };
    h.apply([
        UiCommand::SetTodos(vec![
            todo("Read the failing test", TodoState::Completed),
            todo("Fix the parser", TodoState::InProgress),
            todo("Run the suite", TodoState::Pending),
        ]),
        UiCommand::AppendThinking("Checking the diff…".into()),
    ]);
    for _ in 0..3 {
        h.pass(TICK);
    }
    let screen = h.screen();
    assert!(screen.contains("Working on 3 to-dos"), "{screen}");
    snapshot("working", &mut h);
}

#[test]
fn slash_menu() {
    let mut h = stage();
    seed(&mut h);
    h.type_text("/");
    snapshot("slash", &mut h);
}

#[test]
fn model_picker() {
    let mut h = stage();
    session(&mut h);
    h.submit("/model");
    snapshot("models", &mut h);
}

/// What a lead did, and the coder and tester it gave work to.
pub fn flow(h: &mut Stage) {
    let node = |agent: &str, seq: u64, kind: FlowKind, label: &str, detail: &str| FlowNode {
        id: format!("{agent}:{seq}"),
        agent_id: agent.into(),
        kind,
        label: label.into(),
        detail: detail.into(),
        status: TraceNodeStatus::Ok,
        seq,
        at_ms: 1_000 * seq + if agent == "a1" { 0 } else { 3_500 },
        ..FlowNode::default()
    };
    let linked = |mut node: FlowNode, kind: &str, to: &str| {
        node.links.push((kind.into(), to.into()));
        node
    };
    let lane = |id: &str, name: &str, parent: Option<&str>| FlowLane {
        agent_id: id.into(),
        name: name.into(),
        parent: parent.map(Into::into),
        ..FlowLane::default()
    };
    let used = |mut node: FlowNode, input_tokens, output_tokens, cost_usd, ms| {
        node.usage = Some(FlowUsage {
            input_tokens,
            output_tokens,
            cost_usd,
            duration_ms: Some(ms),
        });
        node
    };
    let mut pending = node("a1", 9, FlowKind::Model, "qwen3-coder", "");
    pending.status = TraceNodeStatus::Pending;
    h.apply([UiCommand::FlowLoad {
        agent_id: "a1".into(),
        lanes: vec![
            lane("a1", "Agent01", None),
            lane("c1", "coder #1", Some("a1")),
            lane("t1", "tester #1", Some("a1")),
        ],
        nodes: vec![
            node("a1", 1, FlowKind::Prompt, "Summarise the crates", ""),
            used(
                node("a1", 2, FlowKind::Model, "qwen3-coder", "1.2k/300 · 4s"),
                48_200,
                300,
                Some(0.042),
                4_000,
            ),
            node("a1", 3, FlowKind::Tool, "list_files crates", "12ms"),
            linked(
                linked(
                    node("a1", 4, FlowKind::Tool, "task coder · fix README", "8.1s"),
                    "spawn",
                    "c1:1",
                ),
                "result",
                "a1:7",
            ),
            linked(
                linked(
                    node("a1", 5, FlowKind::Tool, "task tester · cargo test", "6.3s"),
                    "spawn",
                    "t1:1",
                ),
                "result",
                "a1:8",
            ),
            node("c1", 1, FlowKind::Prompt, "fix README", ""),
            used(
                node("c1", 2, FlowKind::Model, "qwen3-coder", "800/120 · 2s"),
                800,
                120,
                None,
                2_000,
            ),
            node("c1", 3, FlowKind::Tool, "edit_file README.md", "4ms"),
            linked(node("c1", 4, FlowKind::Reply, "done", ""), "return", "a1:7"),
            node("t1", 1, FlowKind::Prompt, "cargo test", ""),
            node("t1", 2, FlowKind::Tool, "run_command cargo test", "5.9s"),
            linked(
                node("t1", 3, FlowKind::Reply, "42 passed", ""),
                "return",
                "a1:8",
            ),
            node("a1", 7, FlowKind::Result, "coder #1 replied", ""),
            node("a1", 8, FlowKind::Result, "tester #1 replied", ""),
            pending,
        ],
    }]);
}

#[test]
fn graph_canvas() {
    let mut h = stage();
    seed(&mut h);
    let graph = PaneTab::Chat(ChatTab::Graph);
    let tab = h.shell.chrome.spot(graph).expect("the tab is drawn");
    h.drag(tab, tab);
    flow(&mut h);
    let screen = h.screen();
    assert!(
        !screen.contains("qwen3-coder"),
        "the window draws the cards\n{screen}"
    );
    let tui = &h.shell.tabs.front().tui;
    let canvas = tui.flow_canvas().expect("the canvas is on screen");
    assert_eq!(tui.flow_canvas_clip(), [canvas]);
    snapshot("graph", &mut h);

    // Expanded, the graph has the whole window.
    let expand = |h: &mut Stage| {
        let at = h.shell.chrome.expand_box(Pane::Chat).expect("drawn");
        let at = Point::new(at.center_x(), at.center_y());
        h.drag(at, at);
    };
    expand(&mut h);
    let tui = &h.shell.tabs.front().tui;
    assert!(tui.chat_expanded());
    let wide = tui.flow_canvas().expect("the canvas is on screen");
    assert!(wide.width > canvas.width, "{wide:?} over {canvas:?}");
    snapshot("graph-expanded", &mut h);
    expand(&mut h);
    assert!(!h.shell.tabs.front().tui.chat_expanded());

    h.shell.key_pressed(&Press::of(NamedKey::Home), h.now);
    for _ in 0..3 {
        h.shell.key_pressed(&Press::of(NamedKey::ArrowDown), h.now);
    }
    let tui = &h.shell.tabs.front().tui;
    assert_eq!(
        tui.flow().selected_node().map(|n| n.id.as_str()),
        Some("a1:4")
    );
    snapshot("graph-delegation", &mut h);

    // Zoomed out, the whole flow fits; further out, text is bars.
    h.shell.tabs.front_mut().tui.zoom_flow(0.5, None);
    snapshot("graph-zoomed-out", &mut h);
    h.shell.tabs.front_mut().tui.zoom_flow(0.5, None);
    h.shell.frame(h.now);
    h.shell.tabs.front_mut().tui.zoom_flow(4.0, None);

    // A menu over the canvas stays over it.
    h.type_text("/");
    h.shell.frame(h.now);
    let tui = &h.shell.tabs.front().tui;
    let clip = tui.flow_canvas_clip();
    let drawn: u32 = clip.iter().map(|rect| rect.area()).sum();
    assert!(drawn > 0 && drawn < canvas.area(), "{clip:?}");
    snapshot("graph-menu", &mut h);
}

/// A lead that ran a benchmark of six tasks on two arms, and kept working.
fn benchmark_flow(h: &mut Stage) {
    let node = |agent: &str, seq: u64, at_ms: u64, kind: FlowKind, label: &str| FlowNode {
        id: format!("{agent}:{seq}"),
        agent_id: agent.into(),
        kind,
        label: label.into(),
        status: TraceNodeStatus::Ok,
        seq,
        at_ms,
        ..FlowNode::default()
    };
    let lane = |id: &str, name: &str, parent: Option<&str>| FlowLane {
        agent_id: id.into(),
        name: name.into(),
        parent: parent.map(Into::into),
        ..FlowLane::default()
    };
    let mut lanes = vec![
        lane("a1", "Agent01", None),
        lane("h1", "Compare #1", Some("a1")),
    ];
    let mut nodes = Vec::new();
    for turn in 0..8 {
        let at = 1_000 * turn;
        nodes.push(node("a1", 3 * turn + 1, at, FlowKind::Prompt, "next step"));
        nodes.push(node(
            "a1",
            3 * turn + 2,
            at + 100,
            FlowKind::Model,
            "qwen3-coder",
        ));
        nodes.push(node("a1", 3 * turn + 3, at + 900, FlowKind::Reply, "done"));
    }
    for task in 0..6_u64 {
        for arm in 0..2_usize {
            let id = format!("r{task}{arm}");
            let letter = if arm == 0 { 'a' } else { 'b' };
            let check = !(arm == 1 && task % 3 == 0);
            lanes.push(FlowLane {
                bench: Some(aura_ui::events::FlowBench {
                    group: format!("coding/task-{task}"),
                    run: format!("{letter}-{}", task + 1),
                    arm,
                    arm_label: if arm == 0 { "gate" } else { "opus" }.into(),
                    check_passed: Some(check),
                    cost_usd: 0.12,
                    wall_ms: 40_000,
                    error: None,
                }),
                ..lane(&id, &format!("{letter}-{}", task + 1), Some("h1"))
            });
            for turn in 0..2 {
                let at = 1_000 * task + 400 * turn;
                nodes.push(node(&id, 3 * turn + 1, at, FlowKind::Prompt, "fix it"));
                let mut model = node(&id, 3 * turn + 2, at + 50, FlowKind::Model, "opus");
                if !check {
                    model.status = TraceNodeStatus::Error;
                }
                nodes.push(model);
                nodes.push(node(&id, 3 * turn + 3, at + 300, FlowKind::Reply, "fixed"));
            }
        }
    }
    h.apply([UiCommand::FlowLoad {
        agent_id: "a1".into(),
        lanes,
        nodes,
    }]);
}

#[test]
fn graph_of_a_benchmark() {
    let mut h = stage();
    seed(&mut h);
    let graph = PaneTab::Chat(ChatTab::Graph);
    let tab = h.shell.chrome.spot(graph).expect("the tab is drawn");
    h.drag(tab, tab);
    benchmark_flow(&mut h);
    h.shell.frame(h.now);
    let tui = &h.shell.tabs.front().tui;
    assert!(tui.flow().is_big());
    assert!(tui.flow().minimap().is_some(), "the map is drawn");
    snapshot("graph-benchmark", &mut h);

    let time = PaneTab::Chat(ChatTab::Time);
    let tab = h.shell.chrome.spot(time).expect("the tab is drawn");
    h.drag(tab, tab);
    assert_eq!(h.shell.tabs.front().tui.chat_tab(), ChatTab::Time);
    snapshot("time-benchmark", &mut h);
}

#[test]
fn approval_request() {
    let mut h = stage();
    session(&mut h);
    h.apply([UiCommand::RequestApproval {
        id: "r1".into(),
        tool: "run_command".into(),
        description: "rm -rf target/".into(),
        always: "`rm` commands in this project".into(),
    }]);
    snapshot("approval", &mut h);
}

pub fn report(label: &str, samples: &mut [std::time::Duration]) {
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
