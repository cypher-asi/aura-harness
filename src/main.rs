//! Aura OS entry point.
//!
//! By default, starts the simple IRC-style terminal UI. Use `--ui none` to run
//! in headless/swarm mode.

mod api_server;
mod cli;
mod event_loop;
mod record_loader;

use cli::{Args, UiMode};

use aura_agent::{AgentLoop, AgentLoopConfig, KernelToolExecutor};
use aura_core::{Identity, Transaction};
use aura_executor::ExecutorRouter;
use aura_kernel::{ProcessManager, ProcessManagerConfig, TurnConfig};
use aura_reasoner::{AnthropicProvider, MockProvider, ModelProvider};
use aura_store::RocksStore;
use aura_terminal::{App, Terminal, Theme, UiCommand, UiEvent};
use aura_tools::{DefaultToolRegistry, ToolExecutor, ToolRegistry};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

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

    record_loader::load_existing_records(&store, identity.agent_id, &cmd_tx);
    record_loader::send_initial_agent(&identity, &store, &cmd_tx);
    api_server::start_api_server(cmd_tx.clone()).await;

    let mut executor_router = ExecutorRouter::new();
    executor_router.add_executor(std::sync::Arc::new(ToolExecutor::with_defaults()));

    let tool_registry = DefaultToolRegistry::new();
    let tools = tool_registry.list();

    let agent_workspace = workspace_root.join(identity.agent_id.to_hex());
    let kernel_executor =
        KernelToolExecutor::new(executor_router, identity.agent_id, agent_workspace);

    let auth_token = std::env::var("AURA_ROUTER_JWT").ok();

    let config = AgentLoopConfig {
        system_prompt: TurnConfig::default().system_prompt,
        auth_token: auth_token.clone(),
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
                "Mock mode: Set AURA_LLM_ROUTING and required credentials to enable real AI responses.",
            ))
        }
        _ => match AnthropicProvider::from_env() {
            Ok(p) => Arc::new(p),
            Err(e) => {
                warn!(error = %e, "LLM provider not configured, using mock");
                let _ = cmd_tx.try_send(UiCommand::SetStatus("Mock Mode".to_string()));
                Arc::new(MockProvider::simple_response(
                    "Mock mode: Set AURA_LLM_ROUTING and required credentials to enable real AI responses.",
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
        event_loop::run_event_loop(
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

// ============================================================================
// Headless Mode (Swarm)
// ============================================================================

async fn run_headless(_args: Args) -> anyhow::Result<()> {
    info!("Starting AURA OS in headless mode (swarm server)");

    let config = aura_swarm::SwarmConfig::from_env();

    aura_swarm::Swarm::new(config).run().await
}
