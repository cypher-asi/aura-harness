//! Many agents working at once through the real kernel and record store,
//! with a mock model: how long their turns take, how fast records are
//! committed, and what it costs in CPU and memory. Ignored by default; run
//! with
//! `cargo test --release -p aura-agent --test many_agents_load -- --ignored --nocapture`.
//!
//! `AURA_LOAD_AGENTS` lists the agent counts to try (default `1,8,32,64`),
//! `AURA_LOAD_STEPS` the tool steps in each agent's turn (default `20`) and
//! `AURA_LOAD_RESULT_BYTES` the size of each tool result (default `2000`).

use async_trait::async_trait;
use aura_agent::{AgentLoop, AgentLoopConfig, KernelModelGateway, KernelToolGateway};
use aura_agent_kernel::{
    ExecuteContext, Executor, ExecutorError, ExecutorRouter, Kernel, KernelConfig,
};
use aura_core_types::{Action, ActionKind, AgentId, Effect, ToolCall, ToolResult};
use aura_model::{
    ContentBlock, Message, MockProvider, MockResponse, StopReason, ToolDefinition, Usage,
};
use aura_store_db::{RocksStore, Store};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Answers every tool call with `bytes` of text, as a file read would.
struct FixedExecutor {
    bytes: usize,
}

#[async_trait]
impl Executor for FixedExecutor {
    async fn execute(
        &self,
        _ctx: &ExecuteContext,
        action: &Action,
    ) -> Result<Effect, ExecutorError> {
        let call: ToolCall = serde_json::from_slice(&action.payload)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        let text = "x".repeat(self.bytes);
        let result = ToolResult::success(&call.tool, text);
        let payload = serde_json::to_vec(&result)
            .map_err(|e| ExecutorError::ExecutionFailed(e.to_string()))?;
        Ok(Effect::committed_agreement(action.action_id, payload))
    }

    fn can_handle(&self, action: &Action) -> bool {
        action.kind == ActionKind::Delegate
    }

    fn name(&self) -> &'static str {
        "fixed"
    }
}

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name).ok().map_or_else(
        || default.to_vec(),
        |v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect(),
    )
}

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// A turn of `steps` tool calls, each reading a file, then a reply.
fn provider(steps: usize) -> MockProvider {
    let mut responses: Vec<MockResponse> = (0..steps)
        .map(|i| MockResponse {
            stop_reason: StopReason::ToolUse,
            content: vec![
                ContentBlock::text(format!("Reading part {i} of the code.")),
                ContentBlock::tool_use(
                    format!("t{i}"),
                    "read_file",
                    serde_json::json!({ "path": format!("src/file_{i}.rs") }),
                ),
            ],
            usage: Usage::new(1000, 50),
        })
        .collect();
    responses.push(MockResponse::text("done"));
    MockProvider::new().with_responses(responses)
}

/// User and system CPU time of this process, from `/proc/self/stat`.
fn cpu_time() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(Duration::from_millis(ticks * 10))
}

/// Resident memory of this process in MiB, from `/proc/self/statm`.
fn rss_mib() -> Option<f64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: f64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096.0 / (1024.0 * 1024.0))
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index]
}

async fn run_agents(store: &Arc<dyn Store>, agents: usize, steps: usize, bytes: usize) {
    let workspace = tempfile::tempdir().unwrap();
    let tools = vec![ToolDefinition::new(
        "read_file",
        "Read a file",
        serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    )];
    let cpu_before = cpu_time();
    let started = Instant::now();
    let mut turns = Vec::with_capacity(agents);
    for _ in 0..agents {
        let router = ExecutorRouter::with_executors(vec![Arc::new(FixedExecutor { bytes })]);
        let config = KernelConfig {
            workspace_base: workspace.path().to_path_buf(),
            ..KernelConfig::default()
        };
        let kernel = Arc::new(
            Kernel::new(
                Arc::clone(store),
                Arc::new(provider(steps)),
                router,
                config,
                AgentId::generate(),
            )
            .unwrap(),
        );
        let tools = tools.clone();
        turns.push(tokio::spawn(async move {
            let model = KernelModelGateway::new(Arc::clone(&kernel));
            let executor = KernelToolGateway::new(kernel);
            let agent = AgentLoop::new(AgentLoopConfig {
                system_prompt: "You are a coding agent.".repeat(200),
                max_iterations: steps + 5,
                ..AgentLoopConfig::for_agent("claude-test-model")
            });
            let start = Instant::now();
            let result = agent
                .run(&model, &executor, vec![Message::user("go")], tools)
                .await
                .unwrap();
            (start.elapsed(), result.iterations)
        }));
    }
    let mut times = Vec::with_capacity(agents);
    let mut iterations = 0;
    for turn in turns {
        let (took, ran) = turn.await.unwrap();
        times.push(took);
        iterations += ran;
    }
    let wall = started.elapsed();
    times.sort();
    let cpu = cpu_before.zip(cpu_time()).map(|(a, b)| b.saturating_sub(a));
    #[allow(clippy::cast_precision_loss)]
    let per_second = iterations as f64 / wall.as_secs_f64();
    println!(
        "{agents:>6} {:>10.1} {:>10.1} {:>10.1} {:>12.0} {:>10} {:>10}",
        wall.as_secs_f64() * 1000.0,
        percentile(&times, 0.5).as_secs_f64() * 1000.0,
        percentile(&times, 0.99).as_secs_f64() * 1000.0,
        per_second,
        cpu.map_or_else(|| "-".to_string(), |c| format!("{}", c.as_millis())),
        rss_mib().map_or_else(|| "-".to_string(), |r| format!("{r:.0}")),
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "load test; run with --ignored --nocapture"]
async fn many_agents_load() {
    let counts = env_list("AURA_LOAD_AGENTS", &[1, 8, 32, 64]);
    let steps = env_num("AURA_LOAD_STEPS", 20);
    let bytes = env_num("AURA_LOAD_RESULT_BYTES", 2000);
    let db = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db.path(), false).unwrap());
    println!("{steps} tool steps per turn, {bytes}-byte results");
    println!(
        "{:>6} {:>10} {:>10} {:>10} {:>12} {:>10} {:>10}",
        "agents", "wall ms", "turn p50", "turn p99", "iter/s", "cpu ms", "rss MiB"
    );
    for agents in counts {
        run_agents(&store, agents, steps, bytes).await;
    }
}
