//! Aura OS entry point.
//!
//! By default, starts the simple IRC-style terminal UI. Use `--ui none` to run
//! in headless/swarm mode.

use aura_core::{AgentId, Identity, Transaction};
use aura_executor::ExecutorRouter;
use aura_kernel::{
    ProcessManager, ProcessManagerConfig, StreamCallback, StreamCallbackEvent, TurnConfig,
    TurnProcessor, TurnResult,
};
use aura_reasoner::{AnthropicProvider, MockProvider, ModelProvider};
use aura_store::RocksStore;
use aura_terminal::{events::AgentSummary, App, Terminal, Theme, UiCommand, UiEvent};
use aura_tools::{DefaultToolRegistry, ToolExecutor};
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
    // Load .env file if present (for development)
    let _ = dotenvy::dotenv();
    
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
    // cmd channel needs capacity > MAX_RECORDS (100) to avoid blocking during init
    let (ui_tx, mut ui_rx) = mpsc::channel::<UiEvent>(100);
    let (cmd_tx, cmd_rx) = mpsc::channel::<UiCommand>(200);

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
    load_existing_records(&store, identity.agent_id, &cmd_tx);

    // Send initial agent info to UI (there's always at least one agent - the current one)
    send_initial_agent(&identity, &store, &cmd_tx);

    // Start embedded API server
    start_api_server(cmd_tx.clone()).await;

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

    // Create ProcessManager for async command handling
    // The ProcessManager sends completion transactions through this channel
    let (process_tx, mut process_rx_opt) = {
        let (tx, rx) = mpsc::channel::<Transaction>(100);
        (tx, Some(rx))
    };
    let process_manager = Arc::new(ProcessManager::new(process_tx, ProcessManagerConfig::default()));

    // Helper macro to take the process_rx (can only be called once)
    macro_rules! take_process_rx {
        () => {
            process_rx_opt.take().expect("process_rx already taken")
        };
    }

    // Create provider-specific processor and run
    let provider_name = args.provider.as_str();
    match provider_name {
        "mock" => {
            let provider = Arc::new(MockProvider::simple_response(
                "Mock mode: Set ANTHROPIC_API_KEY environment variable to enable real AI responses.",
            ));
            let processor =
                TurnProcessor::new(provider, store.clone(), executor, tool_registry, turn_config);

            // Set initial status to Mock Mode
            let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));

            // Spawn event processing task
            let cmd_tx_clone = cmd_tx.clone();
            let store_clone = store.clone();
            let agent_id = identity.agent_id;
            let process_manager_clone = Arc::clone(&process_manager);
            let process_rx = take_process_rx!();
            let processor_handle = tokio::spawn(async move {
                run_event_loop(
                    &mut ui_rx,
                    process_rx,
                    cmd_tx_clone,
                    processor,
                    store_clone,
                    agent_id,
                    process_manager_clone,
                )
                .await
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
                    let process_manager_clone = Arc::clone(&process_manager);
                    let process_rx = take_process_rx!();
                    let processor_handle = tokio::spawn(async move {
                        run_event_loop(
                            &mut ui_rx,
                            process_rx,
                            cmd_tx_clone,
                            processor,
                            store_clone,
                            agent_id,
                            process_manager_clone,
                        )
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
                        "Mock mode: Set ANTHROPIC_API_KEY environment variable to enable real AI responses.",
                    ));
                    let processor = TurnProcessor::new(
                        provider,
                        store.clone(),
                        executor,
                        tool_registry,
                        turn_config,
                    );

                    // Set initial status to Mock Mode
                    let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));

                    // Spawn event processing task
                    let cmd_tx_clone = cmd_tx.clone();
                    let store_clone = store.clone();
                    let agent_id = identity.agent_id;
                    let process_manager_clone = Arc::clone(&process_manager);
                    let process_rx = take_process_rx!();
                    let processor_handle = tokio::spawn(async move {
                        run_event_loop(
                            &mut ui_rx,
                            process_rx,
                            cmd_tx_clone,
                            processor,
                            store_clone,
                            agent_id,
                            process_manager_clone,
                        )
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
///
/// This loop handles:
/// - User messages from the UI
/// - Process completion events from the ProcessManager (for async commands)
async fn run_event_loop<P>(
    events: &mut mpsc::Receiver<UiEvent>,
    mut process_completions: mpsc::Receiver<Transaction>,
    commands: mpsc::Sender<UiCommand>,
    mut processor: TurnProcessor<P, RocksStore, DefaultToolRegistry>,
    store: Arc<RocksStore>,
    agent_id: AgentId,
    _process_manager: Arc<ProcessManager>,
) -> anyhow::Result<()>
where
    P: ModelProvider + Send + Sync + 'static,
{
    use aura_store::Store;

    // Get the current head sequence from the store to continue from where we left off
    let mut seq = store.get_head_seq(agent_id).unwrap_or(0) + 1;

    // Create a streaming callback that sends text deltas to the UI
    let cmd_tx_for_stream = commands.clone();
    let thinking_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thinking_started_clone = thinking_started.clone();
    
    let stream_callback: StreamCallback = Box::new(move |event| {
        match event {
            StreamCallbackEvent::ThinkingDelta(thinking) => {
                // Start thinking block if not already started
                if !thinking_started_clone.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    let _ = cmd_tx_for_stream.try_send(UiCommand::StartThinking);
                }
                // Send thinking delta to UI
                let _ = cmd_tx_for_stream.try_send(UiCommand::AppendThinking(thinking));
            }
            StreamCallbackEvent::ThinkingComplete => {
                // Thinking complete, finish the thinking block
                let _ = cmd_tx_for_stream.try_send(UiCommand::FinishThinking);
                thinking_started_clone.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            StreamCallbackEvent::TextDelta(text) => {
                // Send text delta to UI in a fire-and-forget manner
                let _ = cmd_tx_for_stream.try_send(UiCommand::AppendText(text));
            }
            StreamCallbackEvent::ToolStart { name, .. } => {
                // Update status to show tool execution
                let _ = cmd_tx_for_stream
                    .try_send(UiCommand::SetStatus(format!("Running {}...", name)));
            }
            StreamCallbackEvent::ToolComplete { name, is_error, .. } => {
                // Update status after tool completion
                if is_error {
                    let _ = cmd_tx_for_stream
                        .try_send(UiCommand::SetStatus(format!("{} failed", name)));
                } else {
                    let _ = cmd_tx_for_stream
                        .try_send(UiCommand::SetStatus(format!("{} complete", name)));
                }
            }
            StreamCallbackEvent::StepComplete => {
                // Step complete, status will be updated by the main loop
                // Reset thinking state for next step
                thinking_started_clone.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            StreamCallbackEvent::Error { code, message, .. } => {
                let _ = cmd_tx_for_stream
                    .try_send(UiCommand::SetStatus(format!("[{code}] {message}")));
            }
        }
    });
    processor.set_stream_callback(Arc::new(stream_callback));

    loop {
        // Use select! to handle both UI events and process completions
        tokio::select! {
            // Handle process completion events (async command results)
            Some(completion_tx) = process_completions.recv() => {
                info!(
                    hash = %completion_tx.hash,
                    tx_type = ?completion_tx.tx_type,
                    "Processing async process completion"
                );
                
                // Process completion transactions are already created by ProcessManager.
                // They need to be enqueued, processed, and recorded.
                if let Err(e) = store.enqueue_tx(&completion_tx) {
                    error!(error = %e, "Failed to enqueue completion transaction");
                    continue;
                }

                // Dequeue and record the completion
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
                        
                        // Notify UI of completion
                        let _ = commands.send(UiCommand::SetStatus("Process completed".to_string())).await;
                    }
                }
            }
            
            // Handle UI events
            Some(event) = events.recv() => {
                match event {
            UiEvent::UserMessage(text) => {
                info!(text = %text, seq = seq, "Processing user message");

                // Update status and start streaming
                let _ = commands
                    .send(UiCommand::SetStatus("Thinking...".to_string()))
                    .await;
                let _ = commands.send(UiCommand::StartStreaming).await;

                // BUG FIX: Drain any stale transactions from the inbox before processing.
                // Stale transactions can accumulate if previous operations failed mid-way.
                // We discard them to ensure we process the fresh user message.
                let mut stale_count = 0;
                while let Ok(Some((stale_inbox_seq, stale_tx))) = store.dequeue_tx(agent_id) {
                    warn!(
                        stale_inbox_seq = stale_inbox_seq,
                        stale_tx_type = ?stale_tx.tx_type,
                        "Discarding stale inbox transaction"
                    );
                    // Create a dummy record entry to clear this from the inbox
                    // We use the current seq and increment it
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

                // Create and enqueue the user's transaction
                let tx = Transaction::user_prompt(agent_id, text.clone());
                if let Err(e) = store.enqueue_tx(&tx) {
                    error!(error = %e, "Failed to enqueue transaction");
                    let _ = commands
                        .send(UiCommand::ShowError(format!("Storage error: {e}")))
                        .await;
                    let _ = commands.send(UiCommand::Complete).await;
                    continue;
                }

                // Dequeue the transaction we just enqueued (inbox should be empty now)
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

                // Verify we got what we enqueued (sanity check after draining stale entries)
                if dequeued_tx.hash != tx.hash {
                    error!("Transaction mismatch after draining stale entries - this should not happen");
                }

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

                        // === Tool Proposal & Execution Transactions ===
                        // For each tool call, create TWO transactions:
                        // 1. ToolProposal - What the LLM suggested (before policy)
                        // 2. ToolExecution - What the Kernel decided and executed (after policy)
                        for entry in &result.entries {
                            for tool in &entry.executed_tools {
                                // === Transaction: Tool Proposal ===
                                // Records what the LLM suggested - the Kernel has not yet decided
                                let proposal = aura_core::ToolProposal::new(
                                    tool.tool_use_id.clone(),
                                    tool.tool_name.clone(),
                                    tool.tool_args.clone(),
                                );
                                let proposal_tx = Transaction::tool_proposal(agent_id, &proposal);

                                if let Err(e) = store.enqueue_tx(&proposal_tx) {
                                    error!(error = %e, "Failed to enqueue tool proposal transaction");
                                    continue;
                                }

                                if let Ok(Some((proposal_inbox_seq, dequeued_proposal_tx))) = store.dequeue_tx(agent_id) {
                                    let proposal_context_hash = compute_context_hash(seq, &dequeued_proposal_tx);
                                    
                                    // Proposal record has no actions/effects - just records the suggestion
                                    let proposal_entry = aura_core::RecordEntry::builder(seq, dequeued_proposal_tx.clone())
                                        .context_hash(proposal_context_hash)
                                        .build();

                                    if let Err(e) = store.append_entry_atomic(
                                        agent_id,
                                        seq,
                                        &proposal_entry,
                                        proposal_inbox_seq,
                                    ) {
                                        error!(error = %e, "Failed to persist tool proposal record");
                                    } else {
                                        debug!(seq = seq, tool = %tool.tool_name, "Tool proposal record persisted");
                                    }

                                    send_record_to_ui(&commands, seq, &dequeued_proposal_tx, &proposal_entry).await;
                                    seq += 1;
                                }

                                // === Transaction: Tool Execution ===
                                // Records the Kernel's decision and execution result
                                let result_text = match &tool.result {
                                    aura_kernel::ToolResultContent::Text(s) => s.clone(),
                                    aura_kernel::ToolResultContent::Json(v) => {
                                        serde_json::to_string(v).unwrap_or_default()
                                    }
                                };

                                let execution = aura_core::ToolExecution {
                                    tool_use_id: tool.tool_use_id.clone(),
                                    tool: tool.tool_name.clone(),
                                    args: tool.tool_args.clone(),
                                    decision: aura_core::ToolDecision::Approved, // Was executed, so approved
                                    reason: None,
                                    result: Some(result_text.clone()),
                                    is_error: tool.is_error,
                                };
                                let execution_tx = Transaction::tool_execution(agent_id, &execution);

                                if let Err(e) = store.enqueue_tx(&execution_tx) {
                                    error!(error = %e, "Failed to enqueue tool execution transaction");
                                    continue;
                                }

                                if let Ok(Some((exec_inbox_seq, dequeued_exec_tx))) = store.dequeue_tx(agent_id) {
                                    let exec_context_hash = compute_context_hash(seq, &dequeued_exec_tx);

                                    // Create action and effect for the execution
                                    let tool_call = aura_core::ToolCall::new(
                                        tool.tool_name.clone(),
                                        tool.tool_args.clone(),
                                    );
                                    let action = aura_core::Action::delegate_tool(&tool_call);
                                    let action_id = action.action_id;

                                    let effect_status = if tool.is_error {
                                        aura_core::EffectStatus::Failed
                                    } else {
                                        aura_core::EffectStatus::Committed
                                    };

                                    let effect = aura_core::Effect::new(
                                        action_id,
                                        aura_core::EffectKind::Agreement,
                                        effect_status,
                                        result_text.into_bytes(),
                                    );

                                    let mut decision = aura_core::Decision::new();
                                    decision.accept(action_id);

                                    let exec_entry = aura_core::RecordEntry::builder(seq, dequeued_exec_tx.clone())
                                        .context_hash(exec_context_hash)
                                        .decision(decision)
                                        .actions(vec![action])
                                        .effects(vec![effect])
                                        .build();

                                    if let Err(e) = store.append_entry_atomic(
                                        agent_id,
                                        seq,
                                        &exec_entry,
                                        exec_inbox_seq,
                                    ) {
                                        error!(error = %e, "Failed to persist tool execution record");
                                    } else {
                                        debug!(seq = seq, tool = %tool.tool_name, "Tool execution record persisted");
                                    }

                                    send_record_to_ui(&commands, seq, &dequeued_exec_tx, &exec_entry).await;
                                    seq += 1;
                                }
                            }
                        }

                        // === Agent Response Transaction ===
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
                        if let Some(msg) = &result.final_message {
                            let preview: String = msg.text_content().chars().take(100).collect();
                            info!(response_preview = %preview, "Model response received");
                        }
                        // Streaming is always enabled when we have a callback
                        show_turn_result(&commands, &result, true).await;
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
            UiEvent::NewSession => {
                debug!("New session requested, seq={}", seq);

                // Create a SessionStart transaction to mark the context boundary
                let session_tx = Transaction::session_start(agent_id);

                // Enqueue and persist the session start transaction
                if let Err(e) = store.enqueue_tx(&session_tx) {
                    error!(error = %e, "Failed to enqueue session start");
                } else if let Ok(Some((inbox_seq, tx))) = store.dequeue_tx(agent_id) {
                    // Create a minimal record entry for the session start
                    let context_hash = compute_context_hash(seq, &tx);
                    let entry = aura_core::RecordEntry::builder(seq, tx.clone())
                        .context_hash(context_hash)
                        .build();

                    if let Err(e) = store.append_entry_atomic(agent_id, seq, &entry, inbox_seq) {
                        error!(error = %e, "Failed to persist session start record");
                    } else {
                        debug!(seq = seq, "Session start record persisted");
                        // Send to UI
                        send_record_to_ui(&commands, seq, &tx, &entry).await;
                        seq += 1;
                    }
                }

                let _ = commands
                    .send(UiCommand::SetStatus("Ready".to_string()))
                    .await;
            }
            UiEvent::SelectAgent(_agent_id) => {
                // TODO: Implement agent switching
                debug!("Agent selection not yet implemented");
            }
            UiEvent::RefreshAgents => {
                // TODO: Implement agent list refresh
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

    // Extract tx_kind and sender from the transaction type
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

    // Extract message content from payload
    let message = String::from_utf8_lossy(&tx.payload).to_string();
    let message = if message.len() > 200 {
        format!("{}...", &message[..197])
    } else {
        message
    };

    // Count effects and determine status
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
        format!("{} ok", ok_count)
    } else {
        format!("{} ok, {} err", ok_count, err_count)
    };

    // Derive status from effects
    // No effects = successfully recorded (Ok), errors = Error, pending = Pending
    use aura_terminal::events::RecordStatus;
    let status = if err_count > 0 {
        RecordStatus::Error
    } else if pending_count > 0 {
        RecordStatus::Pending
    } else {
        RecordStatus::Ok
    };

    // Extract error details from failed effects
    let error_details: String = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, aura_core::EffectStatus::Failed))
        .filter_map(|e| {
            String::from_utf8(e.payload.to_vec()).ok()
        })
        .collect::<Vec<_>>()
        .join("; ");

    // Extract info (tool name for tool-related transactions)
    let info = extract_tool_info(tx);

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
        status,
        info,
        error_details,
        // Transaction fields
        tx_id: hex::encode(tx.hash.as_bytes()),
        agent_id: hex::encode(tx.agent_id.as_bytes()),
        ts_ms: tx.ts_ms,
    };

    let _ = commands.send(UiCommand::NewRecord(record_summary)).await;
}

/// Extract tool name or other info from a transaction payload.
/// For cmd_run tools, extracts the actual command that was executed.
fn extract_tool_info(tx: &Transaction) -> String {
    use aura_core::TransactionType;

    match tx.tx_type {
        TransactionType::ToolProposal => {
            // Try to parse as ToolProposal
            if let Ok(proposal) = serde_json::from_slice::<aura_core::ToolProposal>(&tx.payload) {
                // For cmd_run, extract the actual command
                if proposal.tool == "cmd_run" {
                    return extract_cmd_run_command(&proposal.args);
                }
                return proposal.tool;
            }
        }
        TransactionType::ToolExecution => {
            // Try to parse as ToolExecution
            if let Ok(execution) = serde_json::from_slice::<aura_core::ToolExecution>(&tx.payload) {
                // For cmd_run, extract the actual command
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
        format!("{} {}", program, cmd_args.join(" "))
    }
}

/// Show turn result (chat message and stats) without creating new records.
///
/// If streaming was used, the message is already displayed and just needs to be finalized.
/// If no streaming callback was set, we show the full message.
async fn show_turn_result(
    commands: &mpsc::Sender<UiCommand>,
    result: &TurnResult,
    was_streaming: bool,
) {
    if was_streaming {
        // Finalize the streaming message (it's already been displayed incrementally)
        let _ = commands.send(UiCommand::FinishStreaming).await;
    } else {
        // No streaming - show the full message now
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
    }

    // Only show warning if there were failures
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
    hasher.update(tx.hash.as_bytes());
    hasher.update(&tx.ts_ms.to_be_bytes());
    hasher.update(&tx.payload);
    *hasher.finalize().as_bytes()
}

/// Create a response transaction for the assistant's message.
fn create_response_transaction(agent_id: AgentId, response_text: &str) -> Transaction {
    use aura_core::TransactionType;

    Transaction::new_chained(agent_id, TransactionType::AgentMsg, response_text.as_bytes().to_vec(), None)
}

/// Load existing records from the store and send to UI.
fn load_existing_records(
    store: &Arc<RocksStore>,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    use aura_core::TransactionType;
    use aura_store::Store;

    // Get the head sequence to know how many records exist
    let head_seq = match store.get_head_seq(agent_id) {
        Ok(seq) => seq,
        Err(e) => {
            // Log error and notify UI
            eprintln!("Warning: Failed to get head sequence: {e}");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )));
            return;
        }
    };
    
    if head_seq == 0 {
        return; // No records yet
    }

    // Load the most recent 100 records (or all if < 100)
    // Records are 1-indexed, so if head_seq is 150, we want seq 51-150
    let from_seq = head_seq.saturating_sub(99).max(1);
    let records = match store.scan_record(agent_id, from_seq, 100) {
        Ok(entries) => entries,
        Err(e) => {
            // Log error and notify UI - this could be a deserialization issue
            eprintln!("Warning: Failed to load records (seq {from_seq}..{head_seq}): {e}");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load {head_seq} historical records: {e}"
            )));
            return;
        }
    };

    for entry in records {
        // Extract tx_kind and sender from the transaction type
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

        // Extract message content from payload
        let message = String::from_utf8_lossy(&entry.tx.payload).to_string();
        let message = if message.len() > 200 {
            format!("{}...", &message[..197])
        } else {
            message
        };

        // Count effects and determine status
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
            format!("{} ok", ok_count)
        } else {
            format!("{} ok, {} err", ok_count, err_count)
        };

        // Derive status from effects
        // No effects = successfully recorded (Ok), errors = Error, pending = Pending
        use aura_terminal::events::RecordStatus;
        let status = if err_count > 0 {
            RecordStatus::Error
        } else if pending_count > 0 {
            RecordStatus::Pending
        } else {
            RecordStatus::Ok
        };

        // Extract error details from failed effects
        let error_details: String = entry
            .effects
            .iter()
            .filter(|e| matches!(e.status, aura_core::EffectStatus::Failed))
            .filter_map(|e| {
                String::from_utf8(e.payload.to_vec()).ok()
            })
            .collect::<Vec<_>>()
            .join("; ");

        // Extract info (tool name for tool-related transactions)
        let info = extract_tool_info(&entry.tx);

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
            status,
            info,
            error_details,
            // Transaction fields
            tx_id: hex::encode(entry.tx.hash.as_bytes()),
            agent_id: hex::encode(entry.tx.agent_id.as_bytes()),
            ts_ms: entry.tx.ts_ms,
        };

        // Use try_send to avoid blocking - channel may be full during init
        let _ = commands.try_send(UiCommand::NewRecord(record_summary));
    }
}

/// Send initial agent info to the UI.
/// There's always at least one agent - the current terminal agent.
fn send_initial_agent(
    identity: &Identity,
    store: &Arc<RocksStore>,
    commands: &mpsc::Sender<UiCommand>,
) {
    use aura_store::Store;

    // Get record count for this agent
    let record_count = store.get_head_seq(identity.agent_id).unwrap_or(0);

    // Get last activity timestamp
    let last_active = chrono::Local::now().format("%H:%M:%S").to_string();

    // Create agent summary
    let agent = AgentSummary {
        id: hex::encode(identity.agent_id.as_bytes()),
        name: identity.name.clone(),
        zns_id: identity.zns_id.clone(),
        is_active: true,
        record_count,
        last_active,
    };

    // Send to UI - use try_send to avoid blocking during init (channel may be full)
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
/// Tries the default port first, then fallback ports if busy.
/// Returns the address it's listening on.
async fn start_api_server(cmd_tx: mpsc::Sender<UiCommand>) -> Option<String> {
    // Create a simple health check router
    let app = Router::new()
        .route("/health", get(api_health_handler))
        .layer(TraceLayer::new_for_http());

    // Try default port first, then fallbacks
    let ports_to_try = std::iter::once(API_PORT).chain(FALLBACK_PORTS.iter().copied());

    for port in ports_to_try {
        let addr = format!("127.0.0.1:{port}");
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                let url = format!("http://{addr}");
                info!(%url, "API server listening");

                // Show info if using fallback port
                if port != API_PORT {
                    let _ = cmd_tx.try_send(UiCommand::ShowWarning(format!(
                        "Port {API_PORT} busy, API server using port {port}"
                    )));
                }

                // Update UI with active status
                let _ = cmd_tx.try_send(UiCommand::SetApiStatus {
                    url: Some(url.clone()),
                    active: true,
                });

                // Spawn the server (need to clone app for the move)
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

    // All ports failed
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

    // Load config from environment
    let config = aura_swarm::SwarmConfig::from_env();

    // Run the swarm
    aura_swarm::Swarm::new(config).run().await
}
