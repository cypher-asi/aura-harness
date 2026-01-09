//! Aura OS entry point.
//!
//! By default, starts the simple IRC-style terminal UI. Use `--ui none` to run
//! in headless/swarm mode.

use aura_core::{AgentId, Identity, Transaction};
use aura_executor::ExecutorRouter;
use aura_kernel::{TurnConfig, TurnProcessor, TurnResult};
use aura_reasoner::{AnthropicProvider, MockProvider, ModelProvider};
use aura_store::RocksStore;
use aura_terminal::{App, Terminal, Theme, UiCommand, UiEvent};
use aura_tools::{DefaultToolRegistry, ToolExecutor};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info};
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
    let args = Args::parse();

    match args.ui {
        UiMode::Terminal => run_terminal(args).await,
        UiMode::None => {
            // Initialize tracing only for headless mode (terminal mode handles its own output)
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
    // Load theme
    let theme = Theme::by_name(&args.theme);

    // Create communication channels
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(100);

    // Create terminal app
    let mut app = App::new()
        .with_event_sender(ui_tx.clone())
        .with_command_receiver(cmd_rx);

    if args.verbose {
        app.set_verbose(true);
    }

    // Create terminal
    let mut terminal = Terminal::new(theme)?;

    // Initialize session components
    let data_dir = args
        .dir
        .clone()
        .or_else(|| std::env::var("AURA_DATA_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./aura_data"));

    let workspace_root = data_dir.join("workspaces");
    tokio::fs::create_dir_all(&data_dir).await?;
    tokio::fs::create_dir_all(&workspace_root).await?;

    // Load or create persistent agent identity
    let identity_file = data_dir.join("agent_identity.txt");
    let zns_id = if identity_file.exists() {
        tokio::fs::read_to_string(&identity_file).await?
    } else {
        let new_id = format!("0://terminal/{}", uuid::Uuid::new_v4());
        tokio::fs::write(&identity_file, &new_id).await?;
        new_id
    };
    let identity = Identity::new(&zns_id, "Terminal Agent");

    // Open store
    let store_path = data_dir.join("store");
    let store = Arc::new(RocksStore::open(&store_path, false)?);

    // Load existing records from the store and send to UI
    load_existing_records(&store, identity.agent_id, &cmd_tx).await;

    // Create executor with tool support
    let mut executor = ExecutorRouter::new();
    executor.add_executor(std::sync::Arc::new(ToolExecutor::with_defaults()));

    // Create tool registry
    let tool_registry = Arc::new(DefaultToolRegistry::new());

    // Turn config
    let turn_config = TurnConfig {
        workspace_base: workspace_root,
        ..TurnConfig::default()
    };

    // Create provider-specific processor and run
    let provider_name = args.provider.as_str();
    match provider_name {
        "mock" => {
            let provider = Arc::new(MockProvider::simple_response(
                "I'm a mock assistant. Set ANTHROPIC_API_KEY for real model integration.",
            ));
            let processor =
                TurnProcessor::new(provider, store.clone(), executor, tool_registry, turn_config);

            // Spawn event processing task
            let cmd_tx_clone = cmd_tx.clone();
            let store_clone = store.clone();
            let agent_id = identity.agent_id;
            let processor_handle = tokio::spawn(async move {
                run_event_loop(&mut ui_rx, cmd_tx_clone, processor, store_clone, agent_id).await
            });

            // Run terminal UI (blocking)
            terminal.run(&mut app)?;

            // Cleanup
            processor_handle.abort();
        }
        _ => {
            // Try to create Anthropic provider, fall back to mock
            match AnthropicProvider::from_env() {
                Ok(provider) => {
                    let provider = Arc::new(provider);
                    let processor = TurnProcessor::new(
                        provider,
                        store.clone(),
                        executor,
                        tool_registry,
                        turn_config,
                    );

                    // Spawn event processing task
                    let cmd_tx_clone = cmd_tx.clone();
                    let store_clone = store.clone();
                    let agent_id = identity.agent_id;
                    let processor_handle = tokio::spawn(async move {
                        run_event_loop(&mut ui_rx, cmd_tx_clone, processor, store_clone, agent_id)
                            .await
                    });

                    // Run terminal UI (blocking)
                    terminal.run(&mut app)?;

                    // Cleanup
                    processor_handle.abort();
                }
                Err(_e) => {
                    // Fall back to mock provider if Anthropic key not available
                    let provider = Arc::new(MockProvider::simple_response(
                        "Mock mode: Set ANTHROPIC_API_KEY to use real model.",
                    ));
                    let processor = TurnProcessor::new(
                        provider,
                        store.clone(),
                        executor,
                        tool_registry,
                        turn_config,
                    );

                    // Spawn event processing task
                    let cmd_tx_clone = cmd_tx.clone();
                    let store_clone = store.clone();
                    let agent_id = identity.agent_id;
                    let processor_handle = tokio::spawn(async move {
                        run_event_loop(&mut ui_rx, cmd_tx_clone, processor, store_clone, agent_id)
                            .await
                    });

                    // Run terminal UI (blocking)
                    terminal.run(&mut app)?;

                    // Cleanup
                    processor_handle.abort();
                }
            }
        }
    }

    Ok(())
}

/// Run the event processing loop.
async fn run_event_loop<P>(
    events: &mut mpsc::Receiver<UiEvent>,
    commands: mpsc::Sender<UiCommand>,
    processor: TurnProcessor<P, RocksStore, DefaultToolRegistry>,
    store: Arc<RocksStore>,
    agent_id: AgentId,
) -> anyhow::Result<()>
where
    P: ModelProvider + Send + Sync + 'static,
{
    use aura_store::Store;

    // Get the current head sequence from the store to continue from where we left off
    let mut seq = store.get_head_seq(agent_id).unwrap_or(0) + 1;

    while let Some(event) = events.recv().await {
        match event {
            UiEvent::UserMessage(text) => {
                debug!(text = %text, "Processing user message");

                // Update status
                let _ = commands
                    .send(UiCommand::SetStatus("Thinking...".to_string()))
                    .await;

                // Create and enqueue the transaction
                let tx = Transaction::user_prompt(agent_id, text);
                if let Err(e) = store.enqueue_tx(&tx) {
                    error!(error = %e, "Failed to enqueue transaction");
                    let _ = commands
                        .send(UiCommand::ShowError(format!("Storage error: {e}")))
                        .await;
                    let _ = commands.send(UiCommand::Complete).await;
                    continue;
                }

                // Dequeue the transaction (for the atomic commit protocol)
                let (inbox_seq, tx) = match store.dequeue_tx(agent_id) {
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

                // === Transaction 1: User Prompt ===
                // The prompt transaction is already enqueued and dequeued above.
                // Now process it through the kernel (calls model, executes tools).
                let process_result = processor.process_turn(agent_id, tx.clone(), seq).await;

                match process_result {
                    Ok(result) => {
                        // Commit the prompt transaction to the record
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

                        // Send prompt record to UI
                        send_record_to_ui(&commands, seq, &tx, &prompt_entry).await;
                        seq += 1;

                        // === Transaction 2: Agent Response ===
                        // Create a new transaction for the agent's response.
                        let response_text = result
                            .final_message
                            .as_ref()
                            .map(|m| m.text_content())
                            .unwrap_or_default();

                        let response_tx = create_response_transaction(agent_id, &response_text);

                        // Enqueue the response transaction
                        if let Err(e) = store.enqueue_tx(&response_tx) {
                            error!(error = %e, "Failed to enqueue response transaction");
                        } else if let Ok(Some((resp_inbox_seq, resp_tx))) = store.dequeue_tx(agent_id)
                        {
                            // Response transactions don't need model processing -
                            // they're recordings of what the agent said/did.
                            // Just commit the record with the actions/effects from the turn.
                            let response_context_hash = compute_context_hash(seq, &resp_tx);
                            let response_entry = processor.to_record_entry(
                                seq,
                                resp_tx.clone(),
                                &result,
                                response_context_hash,
                            );

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

                            // Send response record to UI
                            send_record_to_ui(&commands, seq, &resp_tx, &response_entry).await;
                            seq += 1;
                        }

                        // Show the assistant's response in chat
                        show_turn_result(&commands, &result).await;
                    }
                    Err(e) => {
                        error!(error = %e, "Turn processing failed");
                        let _ = commands
                            .send(UiCommand::ShowError(format!("Error: {e}")))
                            .await;
                        let _ = commands.send(UiCommand::Complete).await;
                    }
                }
            }
            UiEvent::Approve(_id) => {
                // TODO: Implement approval handling
                debug!("Approval received");
            }
            UiEvent::Deny(_id) => {
                // TODO: Implement denial handling
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
                // Status is shown automatically in the UI
            }
            UiEvent::ShowHelp => {
                // Help is shown automatically in the UI
            }
            UiEvent::ShowHistory(_) => {
                // History browsing is automatic
            }
            UiEvent::Clear => {
                let _ = commands.send(UiCommand::ClearConversation).await;
            }
        }
    }

    Ok(())
}

/// Send a record summary to the UI (matching the stored format).
async fn send_record_to_ui(
    commands: &mpsc::Sender<UiCommand>,
    seq: u64,
    tx: &Transaction,
    entry: &aura_core::RecordEntry,
) {
    use aura_core::TransactionKind;

    // Extract tx_kind and sender from the transaction type
    let (tx_kind, sender) = match tx.kind {
        TransactionKind::UserPrompt => ("Prompt".to_string(), "USER".to_string()),
        TransactionKind::ActionResult => ("Action".to_string(), "SYSTEM".to_string()),
        TransactionKind::System => ("System".to_string(), "SYSTEM".to_string()),
        TransactionKind::AgentMsg => ("Response".to_string(), "AURA".to_string()),
        TransactionKind::Trigger => ("Trigger".to_string(), "SYSTEM".to_string()),
    };

    // Extract message content from payload
    let message = String::from_utf8_lossy(&tx.payload).to_string();
    let message = if message.len() > 200 {
        format!("{}...", &message[..197])
    } else {
        message
    };

    // Count effects
    let effect_count = entry.effects.len();
    let ok_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, aura_core::EffectStatus::Committed))
        .count();
    let err_count = effect_count - ok_count;

    let effect_status = if effect_count == 0 {
        "-".to_string()
    } else if err_count == 0 {
        format!("{} ok", ok_count)
    } else {
        format!("{} ok, {} err", ok_count, err_count)
    };

    // Get full hash and suffix from context_hash
    let full_hash = hex::encode(entry.context_hash);
    let hash_suffix = full_hash[full_hash.len() - 4..].to_string();

    // Get timestamp
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
    };

    let _ = commands.send(UiCommand::NewRecord(record_summary)).await;
}

/// Show turn result (chat message and stats) without creating new records.
async fn show_turn_result(commands: &mpsc::Sender<UiCommand>, result: &TurnResult) {
    // Show the final message in chat
    if let Some(message) = &result.final_message {
        let content = message.text_content();
        if !content.is_empty() {
            let _ = commands
                .send(UiCommand::ShowMessage(aura_terminal::events::MessageData {
                    role: aura_terminal::events::MessageRole::Assistant,
                    content,
                    is_streaming: false,
                }))
                .await;
        }
    }

    // Show stats as success message
    let stats = format!(
        "Steps: {}, Input: {}k, Output: {}k tokens",
        result.steps,
        result.total_input_tokens / 1000,
        result.total_output_tokens / 1000
    );
    let _ = commands.send(UiCommand::ShowSuccess(stats)).await;

    if result.had_failures {
        let _ = commands
            .send(UiCommand::ShowWarning("Some tool calls failed".to_string()))
            .await;
    }

    // Mark complete
    let _ = commands.send(UiCommand::Complete).await;
}

/// Compute a context hash for a record entry.
fn compute_context_hash(seq: u64, tx: &Transaction) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seq.to_be_bytes());
    hasher.update(tx.tx_id.as_bytes());
    hasher.update(&tx.ts_ms.to_be_bytes());
    hasher.update(&tx.payload);
    *hasher.finalize().as_bytes()
}

/// Create a response transaction for the assistant's message.
fn create_response_transaction(agent_id: AgentId, response_text: &str) -> Transaction {
    use aura_core::{TransactionKind, TxId};

    let payload = response_text.as_bytes().to_vec();
    let tx_id = TxId::from_content(&payload);
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);

    Transaction::new(tx_id, agent_id, ts_ms, TransactionKind::AgentMsg, payload)
}

/// Load existing records from the store and send to UI.
async fn load_existing_records(
    store: &Arc<RocksStore>,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    use aura_core::TransactionKind;
    use aura_store::Store;

    // Scan for existing records starting from seq 1 (records are 1-indexed)
    let records = match store.scan_record(agent_id, 1, 100) {
        Ok(entries) => entries,
        Err(_) => return, // Silently skip if no records
    };

    for entry in records {
        // Extract tx_kind and sender from the transaction type
        let (tx_kind, sender) = match entry.tx.kind {
            TransactionKind::UserPrompt => ("Prompt".to_string(), "USER".to_string()),
            TransactionKind::ActionResult => ("Action".to_string(), "SYSTEM".to_string()),
            TransactionKind::System => ("System".to_string(), "SYSTEM".to_string()),
            TransactionKind::AgentMsg => ("Response".to_string(), "AURA".to_string()),
            TransactionKind::Trigger => ("Trigger".to_string(), "SYSTEM".to_string()),
        };

        // Extract message content from payload
        let message = String::from_utf8_lossy(&entry.tx.payload).to_string();
        let message = if message.len() > 200 {
            format!("{}...", &message[..197])
        } else {
            message
        };

        // Count effects
        let effect_count = entry.effects.len();
        let ok_count = entry
            .effects
            .iter()
            .filter(|e| matches!(e.status, aura_core::EffectStatus::Committed))
            .count();
        let err_count = effect_count - ok_count;

        let effect_status = if effect_count == 0 {
            "-".to_string()
        } else if err_count == 0 {
            format!("{} ok", ok_count)
        } else {
            format!("{} ok, {} err", ok_count, err_count)
        };

        // Get full hash and suffix from context_hash
        let full_hash = hex::encode(entry.context_hash);
        let hash_suffix = full_hash[full_hash.len() - 4..].to_string();

        // Get timestamp from transaction
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
        };

        let _ = commands.send(UiCommand::NewRecord(record_summary)).await;
    }
}

// ============================================================================
// Headless Mode (Swarm)
// ============================================================================

async fn run_headless(_args: Args) -> anyhow::Result<()> {
    info!("Starting AURA OS in headless mode (swarm server)");

    // Load config from environment
    let config = aura_swarm::SwarmConfig::from_env();

    // Run the swarm
    aura_swarm::Swarm::new(config).run().await
}
