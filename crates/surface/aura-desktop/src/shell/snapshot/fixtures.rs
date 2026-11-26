//! The session the screens and benchmarks start from.

use crate::demo::{DIFF, LISTING, REPLY, TEST_OUTPUT};
use crate::shell::keys::Press;
use crate::shell::tests::Stage;
use aura_ui::events::{
    AgentDefinitionSummary, AgentSummary, CallUsage, MessageData, MessageRole, ModelOptionView,
    ProcessKind, ProcessStatus, ProcessSummary, SourceUsage, ToolData, UiCommand,
};
use winit::keyboard::NamedKey;

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
    pub(super) fn joined(&mut self, id: &str, name: &str) {
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
    pub(super) fn screen(&mut self) -> String {
        self.shell.frame(self.now);
        self.shell.text()
    }
}

fn model(
    id: &str,
    label: &str,
    group: &str,
    provider: Option<&str>,
    available: bool,
) -> ModelOptionView {
    ModelOptionView {
        id: id.into(),
        label: label.into(),
        local: provider.is_none(),
        installed: true,
        group: group.into(),
        provider: provider.map(Into::into),
        available,
        context_window: None,
    }
}

fn process(
    id: &str,
    label: &str,
    status: ProcessStatus,
    kind: ProcessKind,
    started_ms: u64,
    ended_ms: Option<u64>,
) -> ProcessSummary {
    ProcessSummary {
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
    }
}

fn models() -> UiCommand {
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
    }
}

fn processes(now: u64) -> UiCommand {
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
    ])
}

fn earlier_conversation() -> UiCommand {
    let mut greeting = MessageData::new(
        MessageRole::Assistant,
        "AURA records every step as a **transaction**.",
    );
    greeting.model = Some("local/qwen3-coder".into());
    UiCommand::LoadConversation(vec![
        MessageData::new(MessageRole::User, "What does this project do?"),
        greeting,
    ])
}

/// Models, agents, processes and earlier conversation.
pub fn seed(h: &mut Stage) {
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default();
    h.apply([
        models(),
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
        processes(now),
        earlier_conversation(),
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
                unpriced_calls: 0,
            }),
            sources: vec![SourceUsage {
                label: "local".into(),
                tokens: 4_812,
                cost_usd: None,
                unpriced_calls: 0,
            }],
        },
        UiCommand::Complete,
    ]);
}

pub(super) fn session(h: &mut Stage) {
    seed(h);
    steps(h);
    reply(h);
}
