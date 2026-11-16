//! A scripted session for `aura-desktop --demo`: answers with thinking,
//! tool calls and streamed markdown, and runs fake commands, so the UI can
//! be shown and profiled without a model or an agent store.

use crate::app::Handles;
use aura_ui_core::events::{
    AgentDefinitionSummary, AgentSummary, CallUsage, MessageData, MessageRole, ModelOptionView,
    ProcessKind, ProcessStatus, ProcessSummary, RecordStatus, RecordSummary, ToolData, UiCommand,
    UiEvent,
};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

const AGENT_ID: &str = "a1";

pub fn start(runtime: &tokio::runtime::Runtime) -> Handles {
    let (events_tx, events) = mpsc::channel(100);
    let (commands, commands_rx) = mpsc::channel(1024);
    runtime.spawn(serve(events, commands));
    Handles {
        events: events_tx,
        commands: commands_rx,
    }
}

struct Demo {
    out: mpsc::Sender<UiCommand>,
    seq: u64,
    processes: Vec<ProcessSummary>,
    model: String,
}

async fn serve(mut events: mpsc::Receiver<UiEvent>, out: mpsc::Sender<UiCommand>) {
    let mut demo = Demo {
        out,
        seq: 0,
        processes: Vec::new(),
        model: "local/qwen3-coder".to_string(),
    };
    demo.seed().await;
    while let Some(event) = events.recv().await {
        match event {
            UiEvent::UserMessage { text, .. } => demo.turn(&text).await,
            UiEvent::RunCommand { command, .. } => demo.command(&command).await,
            UiEvent::SelectModel(id) => {
                demo.model = id;
                demo.send(demo.models()).await;
            }
            UiEvent::Approve(_) | UiEvent::Deny(_) | UiEvent::Cancel(_) => {
                demo.send(UiCommand::Complete).await;
            }
            UiEvent::Quit => break,
            UiEvent::Clear | UiEvent::NewSession => {
                demo.send(UiCommand::ClearConversation).await;
            }
            other => {
                demo.send(UiCommand::ShowWarning(format!(
                    "The demo session ignores {other:?}"
                )))
                .await;
            }
        }
    }
}

impl Demo {
    async fn send(&self, cmd: UiCommand) {
        let _ = self.out.send(cmd).await;
    }

    fn models(&self) -> UiCommand {
        let option = |id: &str, label: &str, group: &str, provider: Option<&str>, available| {
            ModelOptionView {
                id: id.into(),
                label: label.into(),
                local: provider.is_none(),
                installed: true,
                group: group.into(),
                provider: provider.map(Into::into),
                available,
            }
        };
        UiCommand::SetModels {
            options: vec![
                option("local/qwen3-coder", "Qwen3 Coder", "Local", None, true),
                option("local/gpt-oss", "gpt-oss 20B", "Local", None, true),
                option(
                    "claude/opus",
                    "Claude Opus",
                    "Claude plan",
                    Some("claude"),
                    false,
                ),
                option(
                    "router/sonnet",
                    "Claude Sonnet",
                    "Cloud (aura-router)",
                    Some("zos"),
                    true,
                ),
            ],
            current: self.model.clone(),
            route: None,
            user: Some("demo@aura.dev".into()),
            plans: Vec::new(),
        }
    }

    async fn seed(&mut self) {
        self.send(self.models()).await;
        let agent = |id: &str, name: &str, depth, definition: Option<&str>| AgentSummary {
            id: id.into(),
            name: name.into(),
            zns_id: format!("0://{name}"),
            is_active: id == AGENT_ID,
            record_count: 42,
            last_active: "12:04:31".into(),
            definition: definition.map(Into::into),
            depth,
            project: (depth == 0).then(|| "aura-harness".to_string()),
            ..AgentSummary::default()
        };
        self.send(UiCommand::SetAgents(vec![
            agent(AGENT_ID, "Agent01", 0, None),
            agent("a2", "Reviewer", 1, Some("reviewer")),
        ]))
        .await;
        self.send(UiCommand::SetActiveAgent(AGENT_ID.into())).await;
        self.send(UiCommand::SetAgentDefinitions(vec![
            AgentDefinitionSummary {
                name: "reviewer".into(),
                description: "Reviews a diff for bugs and missing tests.".into(),
            },
            AgentDefinitionSummary {
                name: "researcher".into(),
                description: "Reads docs and code to answer a question.".into(),
            },
        ]))
        .await;
        self.send(UiCommand::SetApiStatus {
            url: Some("http://127.0.0.1:8787".into()),
            active: true,
        })
        .await;
        if let Ok(dir) = std::env::current_dir() {
            self.send(UiCommand::SetSandboxRoot(dir)).await;
        }
        let mut history = vec![
            MessageData::new(MessageRole::User, "What does this project do?"),
            MessageData::new(
                MessageRole::Assistant,
                "AURA is an agent harness: a **kernel** records every step as a \
                 transaction, and frontends (the terminal and this window) drive it \
                 over a small event protocol.",
            ),
        ];
        history[1].model = Some(self.model.clone());
        self.send(UiCommand::LoadConversation(history)).await;
        self.track(
            "Agent01 turn",
            ProcessKind::Turn,
            ProcessStatus::Exited,
            95_000,
            Some(80_000),
        )
        .await;
        self.track(
            "cargo watch",
            ProcessKind::Shell,
            ProcessStatus::Idle,
            50_000,
            None,
        )
        .await;
    }

    async fn track(
        &mut self,
        label: &str,
        kind: ProcessKind,
        status: ProcessStatus,
        ago_ms: u64,
        ended_ago_ms: Option<u64>,
    ) -> usize {
        let now = now_ms();
        self.processes.push(ProcessSummary {
            id: format!("p{}", self.processes.len()),
            kind,
            label: label.into(),
            status,
            owner_agent_id: AGENT_ID.into(),
            owner_name: "Agent01".into(),
            started_ms: now - ago_ms,
            ended_ms: ended_ago_ms.map(|a| now - a),
            jump_agent_id: AGENT_ID.into(),
            killable: status.is_live(),
            ..ProcessSummary::default()
        });
        self.send(UiCommand::SetProcesses(self.processes.clone()))
            .await;
        self.processes.len() - 1
    }

    async fn finish(&mut self, index: usize, status: ProcessStatus) {
        if let Some(process) = self.processes.get_mut(index) {
            process.status = status;
            process.ended_ms = Some(now_ms());
            process.killable = false;
        }
        self.send(UiCommand::SetProcesses(self.processes.clone()))
            .await;
    }

    async fn record(&mut self, kind: &str, what: &str, message: &str) {
        self.seq += 1;
        let hash = format!("{:064x}", self.seq * 0x9e37_79b9_7f4a_7c15);
        self.send(UiCommand::NewRecord(RecordSummary {
            seq: self.seq,
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            hash_suffix: hash[60..].to_string(),
            full_hash: hash,
            tx_kind: kind.into(),
            sender: "user".into(),
            message: message.into(),
            action_count: 1,
            effect_status: "1 ok".into(),
            status: RecordStatus::Ok,
            info: what.into(),
            what: what.into(),
            group: String::new(),
            detail: String::new(),
            error_details: String::new(),
            tx_id: String::new(),
            agent_id: AGENT_ID.into(),
            ts_ms: now_ms(),
            policy: None,
        }))
        .await;
    }

    async fn turn(&mut self, prompt: &str) {
        let process = self
            .track(
                "Agent01 turn",
                ProcessKind::Turn,
                ProcessStatus::Running,
                0,
                None,
            )
            .await;
        self.send(UiCommand::TurnStarted { prompt: None }).await;
        self.record("UserPrompt", "", prompt).await;

        self.send(UiCommand::StartThinking).await;
        for word in "The user wants an overview. I'll list the crates, read the \
                     manifest, then summarise."
            .split_inclusive(' ')
        {
            self.send(UiCommand::AppendThinking(word.into())).await;
            sleep(Duration::from_millis(25)).await;
        }
        self.send(UiCommand::FinishThinking).await;

        self.tool("t1", "list_files", r#"{"path":"crates"}"#, LISTING, 300)
            .await;
        self.tool(
            "t2",
            "edit_file",
            r#"{"path":"README.md","old_str":"fast","new_str":"fast and native"}"#,
            DIFF,
            200,
        )
        .await;
        self.tool(
            "t3",
            "run_command",
            r#"{"command":"cargo test -q"}"#,
            TEST_OUTPUT,
            600,
        )
        .await;

        self.send(UiCommand::SetStatus("Writing...".into())).await;
        self.send(UiCommand::StartStreaming).await;
        let reply = REPLY.replace("{prompt}", prompt.trim());
        for chunk in reply.split_inclusive(' ') {
            self.send(UiCommand::AppendText(chunk.into())).await;
            sleep(Duration::from_millis(12)).await;
        }
        self.send(UiCommand::FinishStreaming).await;
        self.send(UiCommand::SetReplyModel {
            model: self.model.clone(),
            usage: Some(CallUsage {
                tokens: 4_812,
                cost_usd: None,
            }),
        })
        .await;
        self.finish(process, ProcessStatus::Exited).await;
        self.send(UiCommand::Complete).await;
    }

    async fn tool(&mut self, id: &str, name: &str, args: &str, result: &str, ms: u64) {
        self.send(UiCommand::SetStatus(format!("Running {name}...")))
            .await;
        self.send(UiCommand::ShowTool(ToolData {
            id: id.into(),
            name: name.into(),
            args: args.into(),
        }))
        .await;
        self.send(UiCommand::ToolPolicy {
            id: id.into(),
            tool: name.into(),
            decision: "allowed by mode.agent".into(),
        })
        .await;
        sleep(Duration::from_millis(ms)).await;
        self.record("ActionResult", name, result.lines().next().unwrap_or(""))
            .await;
        self.send(UiCommand::CompleteTool {
            id: id.into(),
            result: result.into(),
            success: true,
        })
        .await;
    }

    async fn command(&mut self, command: &str) {
        let process = self
            .track(command, ProcessKind::Shell, ProcessStatus::Running, 0, None)
            .await;
        sleep(Duration::from_millis(250)).await;
        let (output, exit_code, status) = if command.starts_with("false") {
            (
                "\x1b[31merror\x1b[0m: command failed".to_string(),
                Some(1),
                ProcessStatus::Failed,
            )
        } else {
            (
                format!("\x1b[1;36m$\x1b[0m {command}\n{TEST_OUTPUT}"),
                None,
                ProcessStatus::Exited,
            )
        };
        self.send(UiCommand::CommandFinished { output, exit_code })
            .await;
        self.finish(process, status).await;
        self.send(UiCommand::Complete).await;
    }
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

pub(crate) const LISTING: &str =
    "aura-agent/\naura-desktop/\naura-terminal/\naura-ui-core/\naura-surface-cli/";

pub(crate) const DIFF: &str = "--- a/README.md\n+++ b/README.md\n@@ -1,3 +1,3 @@\n # AURA\n\
-A fast agent harness.\n+A fast and native agent harness.\n Runs locally.";

pub(crate) const TEST_OUTPUT: &str = "\x1b[32mrunning 42 tests\x1b[0m\n\
..........................................\n\
test result: \x1b[1;32mok\x1b[0m. 42 passed; 0 failed; 0 ignored";

pub(crate) const REPLY: &str = "Here's what I found for **{prompt}**:\n\n\
## Layout\n\n\
| Crate | Role |\n|---|---|\n\
| `aura-ui-core` | event protocol shared by the frontends |\n\
| `aura-terminal` | the ratatui terminal UI |\n\
| `aura-desktop` | this window, drawn on the GPU with iced |\n\n\
The tests pass, and I updated the README:\n\n\
```rust\nlet session = aura_surface_cli::session::start(&args, mode).await?;\n```\n\n\
- Both UIs share one event loop\n- Fonts are Geist and Geist Mono\n\n\
See [iced](https://iced.rs) for the toolkit.";
