//! A scripted session for `aura-desktop --demo`: answers with thinking,
//! tool calls and streamed markdown, and runs fake commands, so the UI can
//! be shown and profiled without a model or an agent store.

use crate::shell::Handles;
use aura_ui::events::{
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
    /// Agents as `(id, name, nesting depth, permanent agent)`.
    agents: Vec<(String, String, usize, Option<String>)>,
    /// The agent the window shows.
    active: String,
    /// The agent whose turn or command is running.
    turn_agent: String,
}

async fn serve(mut events: mpsc::Receiver<UiEvent>, out: mpsc::Sender<UiCommand>) {
    let mut demo = Demo {
        out,
        seq: 0,
        processes: Vec::new(),
        model: "local/qwen3-coder".to_string(),
        agents: vec![
            (AGENT_ID.into(), "Agent01".into(), 0, None),
            ("a2".into(), "Reviewer".into(), 1, Some("reviewer".into())),
        ],
        active: AGENT_ID.into(),
        turn_agent: AGENT_ID.into(),
    };
    demo.seed().await;
    while let Some(event) = events.recv().await {
        match event {
            UiEvent::UserMessage { agent_id, text, .. } => {
                demo.work_for(agent_id);
                demo.turn(&text).await;
            }
            UiEvent::RunCommand {
                agent_id, command, ..
            } => {
                demo.work_for(agent_id);
                demo.command(&command).await;
            }
            UiEvent::CreateAgent(name) => demo.create_agent(name).await,
            UiEvent::SelectAgent(id) => demo.show_agent(id, true).await,
            UiEvent::FocusAgent(id) => demo.show_agent(id, false).await,
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

    /// Send `cmd` to the chat of the agent at work, whichever tab shows it.
    async fn chat(&self, cmd: UiCommand) {
        self.send(UiCommand::ForAgent {
            agent_id: self.turn_agent.clone(),
            command: Box::new(cmd),
        })
        .await;
    }

    /// The next turn or command is for `agent_id` (the active agent when
    /// the UI named none).
    fn work_for(&mut self, agent_id: String) {
        self.turn_agent = if agent_id.is_empty() {
            self.active.clone()
        } else {
            agent_id
        };
    }

    fn name_of(&self, agent_id: &str) -> String {
        self.agents
            .iter()
            .find(|(id, ..)| id == agent_id)
            .map_or_else(|| "Agent".to_string(), |(_, name, ..)| name.clone())
    }

    fn agent_list(&self) -> UiCommand {
        let running = self.processes.iter().any(|p| p.status.is_live());
        UiCommand::SetAgents(
            self.agents
                .iter()
                .map(|(id, name, depth, definition)| AgentSummary {
                    id: id.clone(),
                    name: name.clone(),
                    zns_id: format!("0://{name}"),
                    is_active: *id == self.active,
                    record_count: 42,
                    last_active: "12:04:31".into(),
                    definition: definition.clone(),
                    depth: *depth,
                    running: running && *id == self.turn_agent,
                    project: (id == AGENT_ID).then(|| "aura-harness".to_string()),
                    ..AgentSummary::default()
                })
                .collect(),
        )
    }

    async fn create_agent(&mut self, name: Option<String>) {
        let number = self.agents.len() + 1;
        let name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Agent{number:02}"));
        let id = format!("a{number}");
        self.agents.push((id.clone(), name.clone(), 0, None));
        self.show_agent(id, true).await;
        self.send(UiCommand::ShowSuccess(format!("Created agent {name}")))
            .await;
    }

    /// Make `id` the active agent; `reload` sends its (empty) chat too.
    async fn show_agent(&mut self, id: String, reload: bool) {
        if !self.agents.iter().any(|(known, ..)| *known == id) {
            self.send(UiCommand::ShowError("Unknown agent".into()))
                .await;
            return;
        }
        self.active.clone_from(&id);
        self.send(UiCommand::SetActiveAgent(id.clone())).await;
        self.send(UiCommand::ClearRecords).await;
        if reload && id != AGENT_ID {
            self.send(UiCommand::ForAgent {
                agent_id: id.clone(),
                command: Box::new(UiCommand::LoadConversation(Vec::new())),
            })
            .await;
        }
        self.send(self.agent_list()).await;
        self.send(UiCommand::SetActiveAgent(id)).await;
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
                context_window: None,
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
            thinking: None,
        }
    }

    async fn seed(&mut self) {
        self.send(self.models()).await;
        self.send(self.agent_list()).await;
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
            owner_agent_id: self.turn_agent.clone(),
            owner_name: self.name_of(&self.turn_agent),
            started_ms: now - ago_ms,
            ended_ms: ended_ago_ms.map(|a| now - a),
            jump_agent_id: self.turn_agent.clone(),
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
            agent_id: self.turn_agent.clone(),
            ts_ms: now_ms(),
            policy: None,
        }))
        .await;
    }

    async fn turn(&mut self, prompt: &str) {
        let label = format!("{} turn", self.name_of(&self.turn_agent));
        let process = self
            .track(&label, ProcessKind::Turn, ProcessStatus::Running, 0, None)
            .await;
        self.chat(UiCommand::TurnStarted { prompt: None }).await;
        self.record("UserPrompt", "", prompt).await;

        self.chat(UiCommand::StartThinking).await;
        for word in "The user wants an overview. I'll list the crates, read the \
                     manifest, then summarise."
            .split_inclusive(' ')
        {
            self.chat(UiCommand::AppendThinking(word.into())).await;
            sleep(Duration::from_millis(25)).await;
        }
        self.chat(UiCommand::FinishThinking).await;

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

        self.chat(UiCommand::SetStatus("Writing...".into())).await;
        self.chat(UiCommand::StartStreaming).await;
        let reply = REPLY.replace("{prompt}", prompt.trim());
        for chunk in reply.split_inclusive(' ') {
            self.chat(UiCommand::AppendText(chunk.into())).await;
            sleep(Duration::from_millis(12)).await;
        }
        self.chat(UiCommand::FinishStreaming).await;
        self.chat(UiCommand::SetReplyModel {
            model: self.model.clone(),
            usage: Some(CallUsage {
                tokens: 4_812,
                cost_usd: None,
            }),
        })
        .await;
        self.finish(process, ProcessStatus::Exited).await;
        self.chat(UiCommand::Complete).await;
    }

    async fn tool(&mut self, id: &str, name: &str, args: &str, result: &str, ms: u64) {
        self.chat(UiCommand::SetStatus(format!("Running {name}...")))
            .await;
        self.chat(UiCommand::ShowTool(ToolData {
            id: id.into(),
            name: name.into(),
            args: args.into(),
        }))
        .await;
        self.chat(UiCommand::ToolPolicy {
            id: id.into(),
            tool: name.into(),
            decision: "allowed by mode.agent".into(),
        })
        .await;
        sleep(Duration::from_millis(ms)).await;
        self.record("ActionResult", name, result.lines().next().unwrap_or(""))
            .await;
        self.chat(UiCommand::CompleteTool {
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
        self.chat(UiCommand::CommandFinished { output, exit_code })
            .await;
        self.finish(process, status).await;
        self.chat(UiCommand::Complete).await;
    }
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

pub(crate) const LISTING: &str =
    "aura-agent/\naura-app/\naura-cli/\naura-desktop/\naura-ui/\naura-ui-terminal/";

pub(crate) const DIFF: &str = "--- a/README.md\n+++ b/README.md\n@@ -1,3 +1,3 @@\n # AURA\n\
-A fast agent harness.\n+A fast and native agent harness.\n Runs locally.";

pub(crate) const TEST_OUTPUT: &str = "\x1b[32mrunning 42 tests\x1b[0m\n\
..........................................\n\
test result: \x1b[1;32mok\x1b[0m. 42 passed; 0 failed; 0 ignored";

pub(crate) const REPLY: &str = "Here's what I found for **{prompt}**:\n\n\
## Layout\n\n\
| Crate | Role |\n|---|---|\n\
| `aura-ui` | event protocol shared by the frontends |\n\
| `aura-ui-terminal` | the ratatui terminal UI |\n\
| `aura-desktop` | this window, drawn on the GPU with wgpu |\n\n\
The tests pass, and I updated the README:\n\n\
```rust\nlet session = aura_app::session::start(&options, mode).await?;\n```\n\n\
- Both UIs share one event loop\n- Fonts are Geist and Geist Mono\n\n\
See [wgpu](https://wgpu.rs) for the renderer.";
