//! Memory the UI keeps per agent: chats of many agents working, each
//! shown once, then left off screen. Live heap bytes come from dhat's
//! heap profiler, so the numbers are exact and the budgets below are
//! deterministic.
//!
//! The budget tests run by default with small counts. The table for the
//! docs runs with
//! `cargo test --release -p aura-ui-terminal --test ui_memory -- --ignored --nocapture`;
//! `AURA_MEMORY_AGENTS` lists the agent counts (default `10,50,200`) and
//! `AURA_MEMORY_TURNS` the turns each agent takes (default `20`).

use aura_ui_terminal::events::{
    ActivityData, AgentSummary, MessageData, MessageRole, RecordStatus, RecordSummary, ToolData,
};
use aura_ui_terminal::{render, App, Theme, UiCommand};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Mutex;

#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

/// One heap profiler may run at a time, so the tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

const WIDTH: u16 = 160;
const HEIGHT: u16 = 48;
/// Bytes of a tool call's arguments and of its result: a file written and
/// a file read, as agents do every turn.
const TOOL_BYTES: usize = 20 * 1024;
/// Bytes of an agent's reply each turn.
const REPLY_BYTES: usize = 2 * 1024;
/// Record panel rows the session sends with each switch.
const RECORDS: usize = 300;

fn live() -> usize {
    dhat::HeapStats::get().curr_bytes
}

/// Run `f` under the heap profiler, one test at a time.
fn profiled<T>(f: impl FnOnce() -> T) -> T {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _profiler = dhat::Profiler::builder().testing().build();
    f()
}

fn id(i: usize) -> String {
    format!("{i:032x}")
}

fn agents(count: usize) -> Vec<AgentSummary> {
    (0..count)
        .map(|i| AgentSummary {
            id: id(i),
            name: format!("Agent {i}"),
            is_active: i == 0,
            ..Default::default()
        })
        .collect()
}

fn to(i: usize, command: UiCommand) -> UiCommand {
    UiCommand::ForAgent {
        agent_id: id(i),
        command: Box::new(command),
    }
}

fn args(turn: usize) -> String {
    let body = format!("fn f{turn}() {{}} ").repeat(TOOL_BYTES / 12);
    format!("{{\"path\":\"src/f{turn}.rs\",\"content\":\"{body}\"}}")
}

fn result(turn: usize) -> String {
    format!("line {turn} of the file\n").repeat(TOOL_BYTES / 20)
}

fn reply(turn: usize) -> String {
    format!("Step {turn} of the work, and why it matters. ").repeat(REPLY_BYTES / 40)
}

/// One turn of agent `i`, live, as the session streams it.
fn turn(i: usize, turn: usize) -> Vec<UiCommand> {
    let tool = format!("tool-{i}-{turn}");
    [
        UiCommand::TurnStarted {
            prompt: Some(format!("Do step {turn}")),
        },
        UiCommand::StartStreaming,
        UiCommand::AppendText(reply(turn)),
        UiCommand::FinishStreaming,
        UiCommand::ShowTool(ToolData {
            id: tool.clone(),
            name: "write_file".to_string(),
            args: String::new(),
        }),
        UiCommand::UpdateToolArgs {
            id: tool.clone(),
            args: args(turn),
        },
        UiCommand::CompleteTool {
            id: tool,
            result: result(turn),
            success: true,
        },
        UiCommand::Complete,
    ]
    .into_iter()
    .map(|command| to(i, command))
    .collect()
}

/// Agent `i`'s history after `turns` turns, as the session loads it.
fn history(i: usize, turns: usize) -> Vec<MessageData> {
    (0..turns)
        .flat_map(|t| {
            let tool = MessageData {
                activity: Some(ActivityData::Tool {
                    id: format!("tool-{i}-{t}"),
                    name: "write_file".to_string(),
                    args: args(t),
                    result: Some(result(t)),
                    is_error: false,
                    duration_ms: Some(5),
                    policy: None,
                }),
                ..MessageData::new(MessageRole::Assistant, "")
            };
            [
                MessageData::new(MessageRole::User, format!("Do step {t}")),
                MessageData::new(MessageRole::Assistant, reply(t)),
                tool,
            ]
        })
        .collect()
}

fn records(i: usize) -> Vec<RecordSummary> {
    (0..RECORDS)
        .map(|n| RecordSummary {
            seq: u64::try_from(n).unwrap_or(u64::MAX) + 1,
            timestamp: "12:00:00".to_string(),
            full_hash: format!("{n:064x}"),
            hash_suffix: format!("{n:04x}"),
            tx_kind: "AgentMsg".to_string(),
            sender: "AGENT".to_string(),
            message: format!("record {n} of agent {i}, with a line of what it did"),
            action_count: 1,
            effect_status: "1 ok".to_string(),
            status: RecordStatus::Ok,
            info: String::new(),
            what: "reply".to_string(),
            group: String::new(),
            detail: String::new(),
            error_details: String::new(),
            tx_id: format!("{n:064x}"),
            agent_id: id(i),
            ts_ms: 0,
            policy: None,
        })
        .collect()
}

/// A UI after `count` agents each took `turns` turns and the user looked
/// at every one of them, then came back to the first.
struct Session {
    app: App,
    terminal: Terminal<TestBackend>,
    theme: Theme,
}

impl Session {
    fn new(count: usize) -> Self {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(agents(count)));
        app.process_command(UiCommand::SetActiveAgent(id(0)));
        Self {
            app,
            terminal: Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap(),
            theme: Theme::cyber(),
        }
    }

    fn draw(&mut self) {
        let Self {
            app,
            terminal,
            theme,
        } = self;
        terminal.draw(|f| render(f, app, theme)).unwrap();
    }

    /// Put agent `i` on screen as the session does: its history and its
    /// records follow the switch.
    fn show(&mut self, i: usize, turns: usize) {
        self.app.process_command(UiCommand::SetActiveAgent(id(i)));
        self.app
            .process_command(to(i, UiCommand::LoadConversation(history(i, turns))));
        self.app.process_command(UiCommand::SetRecords(records(i)));
        self.app.tick();
        self.draw();
    }

    fn work(&mut self, count: usize, turns: std::ops::Range<usize>) {
        for t in turns {
            for i in 0..count {
                for command in turn(i, t) {
                    self.app.process_command(command);
                }
            }
            self.app.tick();
            self.draw();
        }
    }
}

/// Live bytes the UI holds after `count` agents took `turns` turns each
/// and were each shown once, over an empty UI's.
fn measure(count: usize, turns: usize) -> usize {
    let before = live();
    let mut session = Session::new(count);
    session.work(count, 0..turns);
    for i in 0..count {
        session.show(i, turns);
    }
    session.show(0, turns);
    let held = live().saturating_sub(before);
    drop(session);
    held
}

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).ok().map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect(),
    )
}

fn mib(bytes: usize) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let bytes = bytes as f64;
    bytes / (1024.0 * 1024.0)
}

/// Most a UI may hold per agent it showed once and left idle off screen,
/// beyond the few kept whole: what the agent row, the view's scroll,
/// tab, draft and HUD take, with room to spare.
const IDLE_AGENT_BUDGET: usize = 64 * 1024;

#[test]
fn an_idle_agent_off_screen_holds_little_however_long_its_chat() {
    let (few, many) = profiled(|| (measure(8, 6), measure(24, 6)));
    let per_agent = many.saturating_sub(few) / 16;
    assert!(
        per_agent < IDLE_AGENT_BUDGET,
        "each idle agent off screen holds {per_agent} bytes; budget {IDLE_AGENT_BUDGET}"
    );
}

#[test]
fn a_long_session_grows_with_the_chats_kept_not_with_the_agents() {
    let (few, many) = profiled(|| (long_session(12), long_session(36)));
    // Every round each agent takes five turns; the chats kept whole grow
    // by as much whatever the count, the 24 agents more only by what an
    // idle one holds.
    let more = many[5].saturating_sub(few[5]);
    assert!(
        more < 24 * IDLE_AGENT_BUDGET,
        "36 agents hold {more} bytes more than 12 after six rounds: {few:?} {many:?}"
    );
}

/// Live bytes after each of six rounds of `count` agents taking five
/// turns, with one of them shown and the first shown again after each.
fn long_session(count: usize) -> Vec<usize> {
    let before = live();
    let mut session = Session::new(count);
    let mut samples = Vec::new();
    for round in 0..6 {
        session.work(count, round * 5..round * 5 + 5);
        session.show(round % count, round * 5 + 5);
        session.show(0, round * 5 + 5);
        samples.push(live().saturating_sub(before));
    }
    drop(session);
    samples
}

#[test]
#[ignore = "memory table; run with --release --ignored --nocapture"]
#[allow(clippy::cast_precision_loss)]
fn ui_memory() {
    let counts = env_list("AURA_MEMORY_AGENTS", &[10, 50, 200]);
    let turns = env_list("AURA_MEMORY_TURNS", &[20])[0];
    for count in [12, 36] {
        let rounds: Vec<String> = profiled(|| long_session(count))
            .iter()
            .map(|b| format!("{:.1}", mib(*b)))
            .collect();
        println!("long session, {count} agents, MiB after each of 6 rounds: {rounds:?}");
    }
    println!("{turns} turns per agent, each shown once, {TOOL_BYTES} B tool args and result");
    println!("{:>6} {:>12} {:>14}", "agents", "held MiB", "per agent KiB");
    for count in counts {
        let held = profiled(|| measure(count, turns));
        println!(
            "{count:>6} {:>12.1} {:>14.1}",
            mib(held),
            mib(held) * 1024.0 / count as f64
        );
    }
}
