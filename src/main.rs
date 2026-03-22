//! Aura OS entry point.
//!
//! By default, starts the simple IRC-style terminal UI. Use `--ui none` to run
//! in headless/swarm mode.

use aura_agent::{AgentLoop, AgentLoopConfig, KernelToolExecutor};
use aura_core::{AgentId, Identity, Transaction};
use aura_executor::ExecutorRouter;
use aura_kernel::{ProcessManager, ProcessManagerConfig, TurnConfig};
use aura_reasoner::{AnthropicProvider, Message, MockProvider, ModelProvider, ToolDefinition};
use aura_store::RocksStore;
use aura_terminal::{events::AgentSummary, App, Terminal, Theme, UiCommand, UiEvent};
use aura_tools::{DefaultToolRegistry, ToolExecutor, ToolRegistry};
use axum::{routing::get, Json, Router};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

// ============================================================================
// CLI Arguments
// ============================================================================

#[derive(Parser)]
#[command(
    name = "aura",
    about = "AURA OS - Autonomous Universal Reasoning Architecture"
)]
struct Args {
    /// UI mode (terminal or none)
    #[arg(long, default_value = "terminal")]
    ui: UiMode,

    /// Theme (cyber, matrix, synthwave, minimal)
    #[arg(long, default_value = "cyber")]
    theme: String,

    /// Working directory
    #[arg(short, long)]
    dir: Option<PathBuf>,

    /// Model provider (anthropic or mock)
    #[arg(long, default_value = "anthropic")]
    provider: String,

    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum UiMode {
    /// Full terminal UI (default)
    Terminal,
    /// No UI, run as swarm server
    None,
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let args = Args::parse();

    match args.ui {
        UiMode::Terminal => run_terminal(args).await,
        UiMode::None => {
            let filter = if args.verbose {
                EnvFilter::from_default_env().add_directive("aura=debug".parse()?)
            } else {
                EnvFilter::from_default_env().add_directive("aura=info".parse()?)
            };

            tracing_subscriber::registry()
                .with(fmt::layer().with_target(false))
                .with(filter)
                .init();

            run_headless(args).await
        }
    }
}

// ============================================================================
// Terminal Mode
// ============================================================================

async fn run_terminal(args: Args) -> anyhow::Result<()> {
    let theme = Theme::by_name(&args.theme);

    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(200);

    let mut app = App::new()
        .with_event_sender(ui_tx.clone())
        .with_command_receiver(cmd_rx);

    if args.verbose {
        app.set_verbose(true);
    }

    let mut terminal = Terminal::new(theme)?;

    let data_dir = args
        .dir
        .clone()
        .or_else(|| std::env::var("AURA_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./aura_data"));

    let workspace_root = data_dir.join("workspaces");
    tokio::fs::create_dir_all(&data_dir).await?;
    tokio::fs::create_dir_all(&workspace_root).await?;

    let identity_file = data_dir.join("agent_identity.txt");
    let zns_id = if identity_file.exists() {
        tokio::fs::read_to_string(&identity_file).await?
    } else {
        let new_id = format!("0://terminal/{}", uuid::Uuid::new_v4());
        tokio::fs::write(&identity_file, &new_id).await?;
        new_id
    };
    let identity = Identity::new(&zns_id, "Terminal Agent");

    let store_path = data_dir.join("store");
    let store = Arc::new(RocksStore::open(&store_path, false)?);

    load_existing_records(&store, identity.agent_id, &cmd_tx);
    send_initial_agent(&identity, &store, &cmd_tx);
    start_api_server(cmd_tx.clone()).await;

    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(std::sync::Arc::new(ToolExecutor::with_defaults()));

    let tool_registry = DefaultToolRegistry::new();
    let tools = tool_registry.list();

    let agent_workspace = workspace_root.join(identity.agent_id.to_hex());
    let kernel_executor =
        KernelToolExecutor::new(executor_router, identity.agent_id, agent_workspace);

    let config = AgentLoopConfig {
        system_prompt: TurnConfig::default().system_prompt,
        ..AgentLoopConfig::default()
    };
    let agent_loop = AgentLoop::new(config);

    let (process_tx, mut process_rx_opt) = {
        let (tx, rx) = mpsc::channel::<Transaction>(100);
        (tx, Some(rx))
    };
    let process_manager = Arc::new(ProcessManager::new(
        process_tx,
        ProcessManagerConfig::default(),
    ));

    macro_rules! take_process_rx {
        () => {
            process_rx_opt.take().expect("process_rx already taken")
        };
    }

    let provider: Arc<dyn ModelProvider> = match args.provider.as_str() {
        "mock" => {
            let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));
            Arc::new(MockProvider::simple_response(
                "Mock mode: Set ANTHROPIC_API_KEY environment variable to enable real AI responses.",
            ))
        }
        _ => match AnthropicProvider::from_env() {
            Ok(p) => Arc::new(p),
            Err(_e) => {
                let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));
                Arc::new(MockProvider::simple_response(
                    "Mock mode: Set ANTHROPIC_API_KEY environment variable to enable real AI responses.",
                ))
            }
        },
    };

    let process_rx = take_process_rx!();
    let cmd_tx_clone = cmd_tx.clone();
    let store_clone = store.clone();
    let agent_id = identity.agent_id;
    let process_manager_clone = Arc::clone(&process_manager);

    let processor_handle = tokio::spawn(async move {
        run_event_loop(
            &mut ui_rx,
            process_rx,
            cmd_tx_clone,
            &agent_loop,
            provider.as_ref(),
            &kernel_executor,
            &tools,
            store_clone,
            agent_id,
            process_manager_clone,
        )
        .await
    });

    terminal.run(&mut app)?;

    processor_handle.abort();

    Ok(())
}

/// Run the event processing loop.
///
/// Handles user messages from the UI and process completion events.
#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    events: &mut mpsc::Receiver<UiEvent>,
    mut process_completions: mpsc::Receiver<Transaction>,
    commands: mpsc::Sender<UiCommand>,
    agent_loop: &AgentLoop,
    provider: &dyn ModelProvider,
    executor: &KernelToolExecutor,
    tools: &[ToolDefinition],
    store: Arc<RocksStore>,
    agent_id: AgentId,
    _process_manager: Arc<ProcessManager>,
) -> anyhow::Result<()> {
    use aura_store::Store;

    let mut seq = store.get_head_seq(agent_id).unwrap_or(0) + 1;
    let mut messages: Vec<Message> = Vec::new();

    loop {
        tokio::select! {
            Some(completion_tx) = process_completions.recv() => {
                info!(
                    hash = %completion_tx.hash,
                    tx_type = ?completion_tx.tx_type,
                    "Processing async process completion"
                );

                if let Err(e) = store.enqueue_tx(&completion_tx) {
                    error!(error = %e, "Failed to enqueue completion transaction");
                    continue;
                }

                if let Ok(Some((inbox_seq, tx))) = store.dequeue_tx(agent_id) {
                    let context_hash = compute_context_hash(seq, &tx);
                    let entry = aura_core::RecordEntry::builder(seq, tx.clone())
                        .context_hash(context_hash)
                        .build();

                    if let Err(e) = store.append_entry_atomic(agent_id, seq, &entry, inbox_seq) {
                        error!(error = %e, "Failed to persist completion record");
                    } else {
                        debug!(seq = seq, "Completion record persisted");
                        send_record_to_ui(&commands, seq, &tx, &entry).await;
                        seq += 1;

                        let _ = commands.send(UiCommand::SetStatus("Process completed".to_string())).await;
                    }
                }
            }

            Some(event) = events.recv() => {
                match event {
            UiEvent::UserMessage(text) => {
                info!(text = %text, seq = seq, "Processing user message");

                let _ = commands
                    .send(UiCommand::SetStatus("Thinking...".to_string()))
                    .await;

                // Drain stale inbox transactions
                let mut stale_count = 0;
                while let Ok(Some((stale_inbox_seq, stale_tx))) = store.dequeue_tx(agent_id) {
                    warn!(
                        stale_inbox_seq = stale_inbox_seq,
                        stale_tx_type = ?stale_tx.tx_type,
                        "Discarding stale inbox transaction"
                    );
                    let stale_entry = aura_core::RecordEntry::builder(seq, stale_tx.clone())
                        .context_hash(compute_context_hash(seq, &stale_tx))
                        .build();
                    if let Err(e) = store.append_entry_atomic(agent_id, seq, &stale_entry, stale_inbox_seq) {
                        error!(error = %e, "Failed to clear stale transaction");
                        break;
                    }
                    seq += 1;
                    stale_count += 1;
                    if stale_count > 10 {
                        error!("Too many stale transactions, aborting drain");
                        break;
                    }
                }

                // Record prompt transaction
                let tx = Transaction::user_prompt(agent_id, text.clone());
                if let Err(e) = store.enqueue_tx(&tx) {
                    error!(error = %e, "Failed to enqueue transaction");
                    let _ = commands
                        .send(UiCommand::ShowError(format!("Storage error: {e}")))
                        .await;
                    let _ = commands.send(UiCommand::Complete).await;
                    continue;
                }

                let (inbox_seq, dequeued_tx) = match store.dequeue_tx(agent_id) {
                    Ok(Some(item)) => item,
                    Ok(None) => {
                        error!("Transaction was enqueued but not found in inbox");
                        continue;
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to dequeue transaction");
                        continue;
                    }
                };

                if dequeued_tx.hash != tx.hash {
                    error!("Transaction mismatch after draining stale entries");
                }

                // Append user message and run agent loop
                messages.push(Message::user(text));

                let process_result = agent_loop
                    .run(provider, executor, messages.clone(), tools.to_vec())
                    .await;

                match process_result {
                    Ok(result) => {
                        // Persist prompt record
                        let prompt_context_hash = compute_context_hash(seq, &tx);
                        let prompt_entry = aura_core::RecordEntry::builder(seq, tx.clone())
                            .context_hash(prompt_context_hash)
                            .build();

                        if let Err(e) =
                            store.append_entry_atomic(agent_id, seq, &prompt_entry, inbox_seq)
                        {
                            error!(error = %e, "Failed to persist prompt record");
                            let _ = commands
                                .send(UiCommand::ShowWarning(format!(
                                    "Warning: Failed to persist to audit log: {e}"
                                )))
                                .await;
                        } else {
                            debug!(seq = seq, "Prompt record persisted");
                        }

                        send_record_to_ui(&commands, seq, &tx, &prompt_entry).await;
                        seq += 1;

                        // Store accumulated messages from result
                        messages = result.messages.clone();

                        // Record response transaction
                        let response_tx = create_response_transaction(agent_id, &result.total_text);

                        if let Err(e) = store.enqueue_tx(&response_tx) {
                            error!(error = %e, "Failed to enqueue response transaction");
                        } else if let Ok(Some((resp_inbox_seq, resp_tx))) = store.dequeue_tx(agent_id) {
                            let response_context_hash = compute_context_hash(seq, &resp_tx);
                            let response_entry = aura_core::RecordEntry::builder(seq, resp_tx.clone())
                                .context_hash(response_context_hash)
                                .build();

                            if let Err(e) = store.append_entry_atomic(
                                agent_id,
                                seq,
                                &response_entry,
                                resp_inbox_seq,
                            ) {
                                error!(error = %e, "Failed to persist response record");
                            } else {
                                debug!(seq = seq, "Response record persisted");
                            }

                            send_record_to_ui(&commands, seq, &resp_tx, &response_entry).await;
                            seq += 1;
                        }

                        // Show assistant response
                        if !result.total_text.is_empty() {
                            let preview: String = result.total_text.chars().take(100).collect();
                            info!(response_preview = %preview, "Model response received");

                            let _ = commands
                                .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                                    role: aura_terminal::events::MessageRole::Assistant,
                                    content: result.total_text.clone(),
                                    is_streaming: false,
                                }))
                                .await;
                        }

                        if let Some(ref err) = result.llm_error {
                            let _ = commands
                                .send(UiCommand::ShowWarning(format!("LLM error: {err}")))
                                .await;
                        }

                        if result.timed_out {
                            let _ = commands
                                .send(UiCommand::ShowWarning("Agent loop timed out".to_string()))
                                .await;
                        }

                        let _ = commands.send(UiCommand::Complete).await;
                    }
                    Err(e) => {
                        error!(error = %e, "Agent loop failed");
                        let _ = commands
                            .send(UiCommand::ShowError(format!("Error: {e}")))
                            .await;
                        let _ = commands.send(UiCommand::Complete).await;
                    }
                }
            }
            UiEvent::Approve(_id) => {
                debug!("Approval received");
            }
            UiEvent::Deny(_id) => {
                debug!("Denial received");
            }
            UiEvent::Quit => {
                debug!("Quit received");
                break;
            }
            UiEvent::Cancel => {
                debug!("Cancel received");
                let _ = commands
                    .send(UiCommand::SetStatus("Cancelled".to_string()))
                    .await;
            }
            UiEvent::ShowStatus => {
            }
            UiEvent::ShowHelp => {
            }
            UiEvent::ShowHistory(_) => {
            }
            UiEvent::Clear => {
                let _ = commands.send(UiCommand::ClearConversation).await;
            }
            UiEvent::NewSession => {
                debug!("New session requested, seq={}", seq);

                let session_tx = Transaction::session_start(agent_id);

                if let Err(e) = store.enqueue_tx(&session_tx) {
                    error!(error = %e, "Failed to enqueue session start");
                } else if let Ok(Some((inbox_seq, tx))) = store.dequeue_tx(agent_id) {
                    let context_hash = compute_context_hash(seq, &tx);
                    let entry = aura_core::RecordEntry::builder(seq, tx.clone())
                        .context_hash(context_hash)
                        .build();

                    if let Err(e) = store.append_entry_atomic(agent_id, seq, &entry, inbox_seq) {
                        error!(error = %e, "Failed to persist session start record");
                    } else {
                        debug!(seq = seq, "Session start record persisted");
                        send_record_to_ui(&commands, seq, &tx, &entry).await;
                        seq += 1;
                    }
                }

                messages.clear();

                let _ = commands
                    .send(UiCommand::SetStatus("Ready".to_string()))
                    .await;
            }
            UiEvent::SelectAgent(_agent_id) => {
                debug!("Agent selection not yet implemented");
            }
            UiEvent::RefreshAgents => {
                debug!("Agent refresh not yet implemented");
            }
        } // end match event
            } // end Some(event) arm
        } // end tokio::select!
    } // end loop

    #[allow(unreachable_code)]
    Ok(())
}

/// Send a record summary to the UI (matching the stored format).
async fn send_record_to_ui(
    commands: &mpsc::Sender<UiCommand>,
    seq: u64,
    tx: &Transaction,
    entry: &aura_core::RecordEntry,
) {
    use aura_core::TransactionType;

    let (tx_kind, sender) = match tx.tx_type {
        TransactionType::UserPrompt => ("Prompt".to_string(), "USER".to_string()),
        TransactionType::ActionResult => ("Action".to_string(), "SYSTEM".to_string()),
        TransactionType::System => ("System".to_string(), "SYSTEM".to_string()),
        TransactionType::AgentMsg => ("Response".to_string(), "AURA".to_string()),
        TransactionType::Trigger => ("Trigger".to_string(), "SYSTEM".to_string()),
        TransactionType::SessionStart => ("Session".to_string(), "SYSTEM".to_string()),
        TransactionType::ToolProposal => ("Propose".to_string(), "LLM".to_string()),
        TransactionType::ToolExecution => ("Execute".to_string(), "KERNEL".to_string()),
        TransactionType::ProcessComplete => ("Complete".to_string(), "SYSTEM".to_string()),
    };

    let message = String::from_utf8_lossy(&tx.payload).to_string();
    let message = if message.len() > 200 {
        format!("{}...", &message[..197])
    } else {
        message
    };

    let effect_count = entry.effects.len();
    let ok_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, aura_core::EffectStatus::Committed))
        .count();
    let pending_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, aura_core::EffectStatus::Pending))
        .count();
    let err_count = effect_count - ok_count - pending_count;

    let effect_status = if effect_count == 0 {
        "-".to_string()
    } else if err_count == 0 {
        format!("{ok_count} ok")
    } else {
        format!("{ok_count} ok, {err_count} err")
    };

    use aura_terminal::events::RecordStatus;
    let status = if err_count > 0 {
        RecordStatus::Error
    } else if pending_count > 0 {
        RecordStatus::Pending
    } else {
        RecordStatus::Ok
    };

    let error_details: String = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, aura_core::EffectStatus::Failed))
        .filter_map(|e| String::from_utf8(e.payload.to_vec()).ok())
        .collect::<Vec<_>>()
        .join("; ");

    let info = extract_tool_info(tx);

    let full_hash = hex::encode(entry.context_hash);
    let hash_suffix = full_hash[full_hash.len() - 4..].to_string();

    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();

    let record_summary = aura_terminal::events::RecordSummary {
        seq,
        timestamp,
        full_hash,
        hash_suffix,
        tx_kind,
        sender,
        message,
        action_count: entry.actions.len(),
        effect_status,
        status,
        info,
        error_details,
        tx_id: hex::encode(tx.hash.as_bytes()),
        agent_id: hex::encode(tx.agent_id.as_bytes()),
        ts_ms: tx.ts_ms,
    };

    let _ = commands.send(UiCommand::NewRecord(record_summary)).await;
}

/// Extract tool name or other info from a transaction payload.
fn extract_tool_info(tx: &Transaction) -> String {
    use aura_core::TransactionType;

    match tx.tx_type {
        TransactionType::ToolProposal => {
            if let Ok(proposal) = serde_json::from_slice::<aura_core::ToolProposal>(&tx.payload) {
                if proposal.tool == "cmd_run" {
                    return extract_cmd_run_command(&proposal.args);
                }
                return proposal.tool;
            }
        }
        TransactionType::ToolExecution => {
            if let Ok(execution) = serde_json::from_slice::<aura_core::ToolExecution>(&tx.payload) {
                if execution.tool == "cmd_run" {
                    return extract_cmd_run_command(&execution.args);
                }
                return execution.tool;
            }
        }
        _ => {}
    }

    String::new()
}

/// Extract the command string from cmd_run tool arguments.
fn extract_cmd_run_command(args: &serde_json::Value) -> String {
    let program = args["program"].as_str().unwrap_or("");
    let cmd_args: Vec<&str> = args["args"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    if cmd_args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", cmd_args.join(" "))
    }
}

/// Compute a context hash for a record entry.
fn compute_context_hash(seq: u64, tx: &Transaction) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seq.to_be_bytes());
    hasher.update(tx.hash.as_bytes());
    hasher.update(&tx.ts_ms.to_be_bytes());
    hasher.update(&tx.payload);
    *hasher.finalize().as_bytes()
}

/// Create a response transaction for the assistant's message.
fn create_response_transaction(agent_id: AgentId, response_text: &str) -> Transaction {
    use aura_core::TransactionType;

    Transaction::new_chained(
        agent_id,
        TransactionType::AgentMsg,
        response_text.as_bytes().to_vec(),
        None,
    )
}

/// Load existing records from the store and send to UI.
fn load_existing_records(
    store: &Arc<RocksStore>,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    use aura_core::TransactionType;
    use aura_store::Store;

    let head_seq = match store.get_head_seq(agent_id) {
        Ok(seq) => seq,
        Err(e) => {
            eprintln!("Warning: Failed to get head sequence: {e}");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )));
            return;
        }
    };

    if head_seq == 0 {
        return;
    }

    let from_seq = head_seq.saturating_sub(99).max(1);
    let records = match store.scan_record(agent_id, from_seq, 100) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Warning: Failed to load records (seq {from_seq}..{head_seq}): {e}");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load {head_seq} historical records: {e}"
            )));
            return;
        }
    };

    for entry in records {
        let (tx_kind, sender) = match entry.tx.tx_type {
            TransactionType::UserPrompt => ("Prompt".to_string(), "USER".to_string()),
            TransactionType::ActionResult => ("Action".to_string(), "SYSTEM".to_string()),
            TransactionType::System => ("System".to_string(), "SYSTEM".to_string()),
            TransactionType::AgentMsg => ("Response".to_string(), "AURA".to_string()),
            TransactionType::Trigger => ("Trigger".to_string(), "SYSTEM".to_string()),
            TransactionType::SessionStart => ("Session".to_string(), "SYSTEM".to_string()),
            TransactionType::ToolProposal => ("Propose".to_string(), "LLM".to_string()),
            TransactionType::ToolExecution => ("Execute".to_string(), "KERNEL".to_string()),
            TransactionType::ProcessComplete => ("Complete".to_string(), "SYSTEM".to_string()),
        };

        let message = String::from_utf8_lossy(&entry.tx.payload).to_string();
        let message = if message.len() > 200 {
            format!("{}...", &message[..197])
        } else {
            message
        };

        let effect_count = entry.effects.len();
        let ok_count = entry
            .effects
            .iter()
            .filter(|e| matches!(e.status, aura_core::EffectStatus::Committed))
            .count();
        let pending_count = entry
            .effects
            .iter()
            .filter(|e| matches!(e.status, aura_core::EffectStatus::Pending))
            .count();
        let err_count = effect_count - ok_count - pending_count;

        let effect_status = if effect_count == 0 {
            "-".to_string()
        } else if err_count == 0 {
            format!("{ok_count} ok")
        } else {
            format!("{ok_count} ok, {err_count} err")
        };

        use aura_terminal::events::RecordStatus;
        let status = if err_count > 0 {
            RecordStatus::Error
        } else if pending_count > 0 {
            RecordStatus::Pending
        } else {
            RecordStatus::Ok
        };

        let error_details: String = entry
            .effects
            .iter()
            .filter(|e| matches!(e.status, aura_core::EffectStatus::Failed))
            .filter_map(|e| String::from_utf8(e.payload.to_vec()).ok())
            .collect::<Vec<_>>()
            .join("; ");

        let info = extract_tool_info(&entry.tx);

        let full_hash = hex::encode(entry.context_hash);
        let hash_suffix = full_hash[full_hash.len() - 4..].to_string();

        let timestamp = chrono::DateTime::from_timestamp_millis(entry.tx.ts_ms as i64)
            .map(|dt| dt.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "??:??:??".to_string());

        let record_summary = aura_terminal::events::RecordSummary {
            seq: entry.seq,
            timestamp,
            full_hash,
            hash_suffix,
            tx_kind,
            sender,
            message,
            action_count: entry.actions.len(),
            effect_status,
            status,
            info,
            error_details,
            tx_id: hex::encode(entry.tx.hash.as_bytes()),
            agent_id: hex::encode(entry.tx.agent_id.as_bytes()),
            ts_ms: entry.tx.ts_ms,
        };

        let _ = commands.try_send(UiCommand::NewRecord(record_summary));
    }
}

/// Send initial agent info to the UI.
fn send_initial_agent(
    identity: &Identity,
    store: &Arc<RocksStore>,
    commands: &mpsc::Sender<UiCommand>,
) {
    use aura_store::Store;

    let record_count = store.get_head_seq(identity.agent_id).unwrap_or(0);
    let last_active = chrono::Local::now().format("%H:%M:%S").to_string();

    let agent = AgentSummary {
        id: hex::encode(identity.agent_id.as_bytes()),
        name: identity.name.clone(),
        zns_id: identity.zns_id.clone(),
        is_active: true,
        record_count,
        last_active,
    };

    let _ = commands.try_send(UiCommand::SetAgents(vec![agent]));
    let _ = commands.try_send(UiCommand::SetActiveAgent(hex::encode(
        identity.agent_id.as_bytes(),
    )));
}

// ============================================================================
// Embedded API Server
// ============================================================================

/// Default API server port
const API_PORT: u16 = 8080;

/// Fallback ports to try if the default is busy
const FALLBACK_PORTS: &[u16] = &[8081, 8082, 8090, 3000];

/// Start the embedded API server.
async fn start_api_server(cmd_tx: mpsc::Sender<UiCommand>) -> Option<String> {
    let app = Router::new()
        .route("/health", get(api_health_handler))
        .layer(TraceLayer::new_for_http());

    let ports_to_try = std::iter::once(API_PORT).chain(FALLBACK_PORTS.iter().copied());

    for port in ports_to_try {
        let addr = format!("127.0.0.1:{port}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                let url = format!("http://{addr}");
                info!(%url, "API server listening");

                if port != API_PORT {
                    let _ = cmd_tx.try_send(UiCommand::ShowWarning(format!(
                        "Port {API_PORT} busy, API server using port {port}"
                    )));
                }

                let _ = cmd_tx.try_send(UiCommand::SetApiStatus {
                    url: Some(url.clone()),
                    active: true,
                });

                tokio::spawn(async move {
                    if let Err(e) = axum::serve(listener, app).await {
                        error!(error = %e, "API server error");
                    }
                });

                return Some(url);
            }
            Err(e) => {
                debug!(port = port, error = %e, "Port unavailable, trying next");
            }
        }
    }

    warn!("Failed to start API server on any port");
    let _ = cmd_tx.try_send(UiCommand::SetApiStatus {
        url: None,
        active: false,
    });
    let _ = cmd_tx.try_send(UiCommand::ShowError(
        "API server failed to start - all ports busy".to_string(),
    ));
    None
}

/// Health check endpoint handler.
async fn api_health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

// ============================================================================
// Headless Mode (Swarm)
// ============================================================================

async fn run_headless(_args: Args) -> anyhow::Result<()> {
    info!("Starting AURA OS in headless mode (swarm server)");

    let config = aura_swarm::SwarmConfig::from_env();

    aura_swarm::Swarm::new(config).run().await
}
