# AURA Interactive Coding Runtime (Rust-only) — Spec 02

**Status**: Implementation-ready  
**Builds on**: spec-01-swarm-mvp.md  
**Goal**: Claude Code–like agentic loop, fully in Rust

---

## 1) Goals

Build a "Claude Code–like" agent runtime on top of AURA's deterministic architecture:

* **Interactive loop**: model proposes tool calls, system executes them, results go back to the model, repeat until end_turn
* **AURA correctness**:
  * deterministic Kernel controls execution and authorization
  * all inputs/outputs are **recorded** in the per-agent Record
  * replay never calls model/tools; it reuses recorded blocks/results
* **Extensible**:
  * provider-agnostic Reasoner interface (Anthropic now, others later)
  * tool abstraction independent of any provider tool schema
* **Rust-only**:
  * deprecate TypeScript gateway (`aura-gateway-ts`)
  * use `anthropic-sdk-rust` for Anthropic provider integration

---

## 2) Architecture Changes from Spec-01

### What stays the same

* `aura-core`: IDs, schemas, hashing (no changes)
* `aura-store`: RocksDB storage (no changes)
* `aura-executor`: Executor trait + router (no changes)
* `aura-tools`: ToolExecutor + sandbox (extend with more tools)
* `aura-swarm`: Router, scheduler, workers (minor updates)

### What changes

| Component | Spec-01 | Spec-02 |
|-----------|---------|---------|
| `aura-reasoner` | HTTP client to TS gateway | Provider-agnostic trait + Anthropic impl |
| `aura-gateway-ts` | Claude SDK wrapper | **Deprecated** |
| `aura-kernel` | Single-step processing | **Turn Processor** (multi-step loop) |
| New: `aura-cli` | N/A | Interactive CLI with approvals |

### Updated Architecture Diagram

```
CLI/UI ─────────────────────────────────────────────────────────────────┐
                                                                        │
                                                                        ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                              aura-swarm                                 │
│   ┌──────────┐    ┌─────────────┐    ┌──────────────────────────────┐  │
│   │  Router  │───►│  Scheduler  │───►│   Worker (per agent)         │  │
│   │ (HTTP)   │    │             │    │   - Lock                     │  │
│   └──────────┘    └─────────────┘    │   - Dequeue tx               │  │
│                                       │   - Run Turn Processor       │  │
│                                       │   - Commit entries           │  │
│                                       └──────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
                                        │
                                        ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        aura-kernel (Deterministic)                      │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                    Turn Processor (NEW)                           │  │
│  │                                                                   │  │
│  │   loop {                                                          │  │
│  │     1. Build context (deterministic)                              │  │
│  │     2. Call ModelProvider.complete()                              │  │
│  │     3. Record assistant response                                  │  │
│  │     4. If tool_use: authorize → execute → inject tool_result      │  │
│  │     5. If end_turn: finalize                                      │  │
│  │   }                                                               │  │
│  │                                                                   │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                           │                    │                        │
│                           ▼                    ▼                        │
│               ┌───────────────────┐  ┌──────────────────────┐          │
│               │      Policy       │  │   Context Builder    │          │
│               │  (existing)       │  │   (existing)         │          │
│               └───────────────────┘  └──────────────────────┘          │
└─────────────────────────────────────────────────────────────────────────┘
         │                                              │
         ▼                                              ▼
┌─────────────────────┐                    ┌─────────────────────────────┐
│   aura-reasoner     │                    │       aura-executor         │
│  ┌───────────────┐  │                    │  ┌───────────────────────┐  │
│  │ ModelProvider │  │                    │  │   ExecutorRouter      │  │
│  │    trait      │  │                    │  └───────────┬───────────┘  │
│  └───────┬───────┘  │                    │              │              │
│          │          │                    │              ▼              │
│  ┌───────▼───────┐  │                    │  ┌───────────────────────┐  │
│  │  Anthropic    │  │                    │  │    aura-tools         │  │
│  │  Provider     │  │                    │  │  (ToolExecutor)       │  │
│  │ (sdk-rust)    │  │                    │  └───────────────────────┘  │
│  └───────────────┘  │                    │                             │
└─────────────────────┘                    └─────────────────────────────┘
```

---

## 3) Crate Layout (Updated)

```
aura/
├─ aura-core              # IDs, schemas, hashing (existing)
├─ aura-store             # RocksDB storage (existing)
├─ aura-executor          # Executor trait + router (existing)
├─ aura-tools             # ToolExecutor + sandbox (existing, extend)
├─ aura-reasoner          # UPDATED: provider-agnostic + Anthropic impl
│  ├─ lib.rs              # ModelProvider trait + factory
│  ├─ types.rs            # Normalized message types (NEW)
│  ├─ anthropic.rs        # AnthropicProvider (NEW)
│  └─ mock.rs             # MockProvider (existing, update)
├─ aura-kernel            # UPDATED: add Turn Processor
│  ├─ lib.rs
│  ├─ kernel.rs           # Existing single-step (keep for compatibility)
│  ├─ turn_processor.rs   # NEW: Claude Code-like loop
│  ├─ policy.rs           # Existing
│  └─ context.rs          # Existing
├─ aura-swarm             # Router, scheduler, workers (existing)
├─ aura-cli               # NEW: Interactive CLI
│  ├─ main.rs
│  ├─ session.rs
│  └─ approval.rs
├─ aura-gateway-ts/       # DEPRECATED (keep for reference)
└─ src/main.rs            # Existing server entry point
```

---

## 4) Normalized Provider Interface

### 4.1 Normalized Conversation Types

These are **AURA canonical** (not Anthropic-specific). Every provider adapter maps to/from these.

```rust
// aura-reasoner/src/types.rs

/// Role in conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// Content block in a message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content
    Text { text: String },
    
    /// Model requesting tool use (assistant only)
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    
    /// Result of tool execution (user only, in response to tool_use)
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        is_error: bool,
    },
}

/// Content of a tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Json(serde_json::Value),
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn tool_results(results: Vec<(String, ToolResultContent, bool)>) -> Self {
        Self {
            role: Role::User,
            content: results
                .into_iter()
                .map(|(id, content, is_error)| ContentBlock::ToolResult {
                    tool_use_id: id,
                    content,
                    is_error,
                })
                .collect(),
        }
    }
}
```

### 4.2 Tool Definition (Provider-Independent)

```rust
/// Tool definition for the model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (e.g., "fs.read", "search.code")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input parameters
    pub input_schema: serde_json::Value,
}
```

### 4.3 Model Request/Response

```rust
/// How the model should choose tools
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Tool { name: String },
}

/// Request to the model
#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: ToolChoice,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}

/// Why the model stopped
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
}

/// Token usage information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Provider trace for debugging/logging
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderTrace {
    pub request_id: Option<String>,
    pub latency_ms: u64,
    pub model: String,
}

/// Response from the model
#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub stop_reason: StopReason,
    pub message: Message,
    pub usage: Usage,
    pub trace: ProviderTrace,
}
```

### 4.4 ModelProvider Trait

```rust
// aura-reasoner/src/lib.rs

use async_trait::async_trait;

/// Provider-agnostic interface for model completions
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Provider name (e.g., "anthropic", "openai")
    fn name(&self) -> &'static str;

    /// Complete a conversation, potentially with tool use
    async fn complete(&self, request: ModelRequest) -> anyhow::Result<ModelResponse>;

    /// Check if the provider is available
    async fn health_check(&self) -> bool;
}
```

---

## 5) Anthropic Provider Implementation

### 5.1 Dependency

Add to `aura-reasoner/Cargo.toml`:

```toml
[dependencies]
anthropic-sdk-rust = "0.1"  # or latest
```

### 5.2 Implementation

```rust
// aura-reasoner/src/anthropic.rs

use crate::{
    ContentBlock, Message, ModelProvider, ModelRequest, ModelResponse,
    ProviderTrace, Role, StopReason, ToolDefinition, ToolChoice, Usage,
};
use anthropic_sdk_rust::{Client, MessageRequest, Tool, Content};
use async_trait::async_trait;
use std::time::Instant;

pub struct AnthropicConfig {
    pub api_key: String,
    pub default_model: String,
    pub timeout_ms: u64,
}

pub struct AnthropicProvider {
    client: Client,
    config: AnthropicConfig,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> anyhow::Result<Self> {
        let client = Client::new(&config.api_key)?;
        Ok(Self { client, config })
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn complete(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        let start = Instant::now();

        // Convert AURA types to Anthropic types
        let anthropic_messages = convert_messages(&request.messages);
        let anthropic_tools = convert_tools(&request.tools);
        let anthropic_tool_choice = convert_tool_choice(&request.tool_choice);

        // Build request
        let api_request = MessageRequest::builder()
            .model(&request.model)
            .system(&request.system)
            .messages(anthropic_messages)
            .tools(anthropic_tools)
            .tool_choice(anthropic_tool_choice)
            .max_tokens(request.max_tokens)
            .temperature(request.temperature.unwrap_or(0.7))
            .build();

        // Call API
        let response = self.client.messages().create(api_request).await?;

        // Convert response back to AURA types
        let message = convert_response_content(&response.content);
        let stop_reason = match response.stop_reason.as_deref() {
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("stop_sequence") => StopReason::StopSequence,
            _ => StopReason::EndTurn,
        };

        Ok(ModelResponse {
            stop_reason,
            message,
            usage: Usage {
                input_tokens: response.usage.input_tokens as u32,
                output_tokens: response.usage.output_tokens as u32,
            },
            trace: ProviderTrace {
                request_id: response.id,
                latency_ms: start.elapsed().as_millis() as u64,
                model: response.model,
            },
        })
    }

    async fn health_check(&self) -> bool {
        // Simple check - could be improved
        true
    }
}

// Conversion helpers (implement based on anthropic-sdk-rust types)
fn convert_messages(messages: &[Message]) -> Vec<anthropic_sdk_rust::Message> {
    // ... map AURA Message to Anthropic Message
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<Tool> {
    // ... map AURA ToolDefinition to Anthropic Tool
}

fn convert_tool_choice(choice: &ToolChoice) -> anthropic_sdk_rust::ToolChoice {
    // ... map AURA ToolChoice to Anthropic ToolChoice
}

fn convert_response_content(content: &[Content]) -> Message {
    // ... map Anthropic response content to AURA Message
}
```

---

## 6) Tool System (Extended from Spec-01)

### 6.1 Tool Registry

```rust
// aura-tools/src/registry.rs

pub trait ToolRegistry: Send + Sync {
    /// List all available tools
    fn list(&self) -> Vec<ToolDefinition>;
    
    /// Get a specific tool definition
    fn get(&self, name: &str) -> Option<ToolDefinition>;
}

pub struct DefaultToolRegistry {
    tools: HashMap<String, ToolDefinition>,
}

impl DefaultToolRegistry {
    pub fn new() -> Self {
        let mut tools = HashMap::new();
        
        // Register built-in tools
        tools.insert("fs.ls".into(), fs_ls_schema());
        tools.insert("fs.read".into(), fs_read_schema());
        tools.insert("fs.stat".into(), fs_stat_schema());
        tools.insert("fs.write".into(), fs_write_schema());
        tools.insert("fs.edit".into(), fs_edit_schema());
        tools.insert("search.code".into(), search_code_schema());
        tools.insert("cmd.run".into(), cmd_run_schema());
        
        Self { tools }
    }
}
```

### 6.2 MVP Tool Set

| Tool | Description | Permission Level |
|------|-------------|------------------|
| `fs.ls` | List directory contents | AlwaysAllow |
| `fs.read` | Read file contents | AlwaysAllow |
| `fs.stat` | Get file metadata | AlwaysAllow |
| `search.code` | Search code (ripgrep) | AlwaysAllow |
| `fs.write` | Write file | AskOnce |
| `fs.edit` | Edit existing file | AskOnce |
| `cmd.run` | Run shell command | AlwaysAsk / Deny |

### 6.3 Tool Schemas

```rust
fn fs_read_schema() -> ToolDefinition {
    ToolDefinition {
        name: "fs.read".into(),
        description: "Read the contents of a file".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative to workspace)"
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Maximum bytes to read (default: 1MB)"
                }
            },
            "required": ["path"]
        }),
    }
}

fn search_code_schema() -> ToolDefinition {
    ToolDefinition {
        name: "search.code".into(),
        description: "Search for patterns in code using ripgrep".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: workspace root)"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "File glob pattern (e.g., '*.rs')"
                }
            },
            "required": ["pattern"]
        }),
    }
}
```

---

## 7) Kernel Turn Processor

### 7.1 Configuration

```rust
// aura-kernel/src/turn_processor.rs

#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Maximum steps (model calls) per turn
    pub max_steps: u32,
    /// Maximum tool calls per step
    pub max_tool_calls_per_step: u32,
    /// Model timeout in milliseconds
    pub model_timeout_ms: u64,
    /// Tool execution timeout in milliseconds
    pub tool_timeout_ms: u64,
    /// Context window size (record entries)
    pub context_window: usize,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            max_steps: 25,
            max_tool_calls_per_step: 8,
            model_timeout_ms: 60_000,
            tool_timeout_ms: 30_000,
            context_window: 50,
        }
    }
}
```

### 7.2 Turn Processor Loop

```rust
pub struct TurnProcessor<P: ModelProvider, S: Store> {
    provider: Arc<P>,
    store: Arc<S>,
    executor: ExecutorRouter,
    policy: Policy,
    tool_registry: Arc<dyn ToolRegistry>,
    config: TurnConfig,
}

impl<P: ModelProvider, S: Store> TurnProcessor<P, S> {
    /// Process a user transaction through the full turn loop
    pub async fn process_turn(
        &self,
        agent_id: AgentId,
        tx: Transaction,
        next_seq: u64,
    ) -> anyhow::Result<Vec<RecordEntry>> {
        let mut entries = Vec::new();
        let mut messages = self.build_initial_messages(&tx)?;
        let system = self.build_system_prompt();
        let tools = self.tool_registry.list();
        
        for step in 0..self.config.max_steps {
            // 1. Build model request
            let request = ModelRequest {
                model: self.config.model.clone(),
                system: system.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
                tool_choice: ToolChoice::Auto,
                max_tokens: 4096,
                temperature: Some(0.7),
            };
            
            // 2. Call model (skip in replay mode)
            let response = if self.replay_mode {
                self.load_recorded_response(agent_id, next_seq + step as u64)?
            } else {
                self.provider.complete(request).await?
            };
            
            // 3. Record assistant response
            let entry = self.record_step(agent_id, next_seq + step as u64, &tx, &response)?;
            entries.push(entry);
            
            // 4. Add assistant message to conversation
            messages.push(response.message.clone());
            
            // 5. Check stop reason
            match response.stop_reason {
                StopReason::EndTurn => {
                    // Turn complete
                    break;
                }
                StopReason::ToolUse => {
                    // Extract and execute tool calls
                    let tool_results = self.execute_tool_calls(&response.message, agent_id).await?;
                    
                    // Add tool results to conversation
                    messages.push(Message::tool_results(tool_results));
                }
                StopReason::MaxTokens => {
                    // Could continue or stop based on config
                    break;
                }
                _ => break,
            }
        }
        
        Ok(entries)
    }
    
    async fn execute_tool_calls(
        &self,
        message: &Message,
        agent_id: AgentId,
    ) -> anyhow::Result<Vec<(String, ToolResultContent, bool)>> {
        let mut results = Vec::new();
        
        for block in &message.content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                // 1. Check policy
                let allowed = self.policy.check_tool(name, input)?;
                
                if !allowed {
                    results.push((
                        id.clone(),
                        ToolResultContent::Text(format!("Tool '{}' denied by policy", name)),
                        true,
                    ));
                    continue;
                }
                
                // 2. Execute tool
                let tool_call = ToolCall {
                    tool: name.clone(),
                    args: input.clone(),
                };
                
                match self.executor.execute_tool(&tool_call, agent_id).await {
                    Ok(result) => {
                        results.push((
                            id.clone(),
                            ToolResultContent::Json(serde_json::to_value(&result)?),
                            !result.ok,
                        ));
                    }
                    Err(e) => {
                        results.push((
                            id.clone(),
                            ToolResultContent::Text(format!("Tool error: {}", e)),
                            true,
                        ));
                    }
                }
            }
        }
        
        Ok(results)
    }
}
```

---

## 8) Record Updates

### 8.1 Extended RecordEntry

Add fields to support multi-step turns:

```rust
pub struct RecordEntry {
    // Existing fields from spec-01
    pub seq: u64,
    pub tx: Transaction,
    pub context_hash: [u8; 32],
    pub proposals: ProposalSet,
    pub decision: Decision,
    pub actions: Vec<Action>,
    pub effects: Vec<Effect>,
    
    // NEW: Turn processor fields
    pub turn_step: Option<u32>,           // Step within turn (0, 1, 2...)
    pub model_response: Option<Message>,  // Recorded model output
    pub tool_results: Vec<ToolResult>,    // Recorded tool outputs
    pub stop_reason: Option<StopReason>,  // Why this step ended
}
```

### 8.2 Replay Rule

During replay:
- Do NOT call `ModelProvider.complete()`
- Do NOT execute tools
- Load `model_response` and `tool_results` from recorded `RecordEntry`
- Reconstruct conversation state from recorded data

---

## 9) Permission System

### 9.1 Permission Levels

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionLevel {
    /// Always allowed without asking
    AlwaysAllow,
    /// Ask once per session, then remember
    AskOnce,
    /// Always ask before each use
    AlwaysAsk,
    /// Never allowed
    Deny,
}
```

### 9.2 Default Tool Permissions

```rust
fn default_tool_permission(tool: &str) -> PermissionLevel {
    match tool {
        // Safe read-only operations
        "fs.ls" | "fs.read" | "fs.stat" | "search.code" => PermissionLevel::AlwaysAllow,
        
        // Write operations need confirmation
        "fs.write" | "fs.edit" => PermissionLevel::AskOnce,
        
        // Commands are risky
        "cmd.run" => PermissionLevel::AlwaysAsk,
        
        // Unknown tools are denied
        _ => PermissionLevel::Deny,
    }
}
```

### 9.3 Approval Flow

When a tool requires approval:

1. Turn Processor emits `ApprovalRequest` effect
2. CLI/UI displays request to user
3. User approves/denies via `Transaction(kind=System, payload=ApprovalResponse)`
4. Turn Processor continues with recorded decision

---

## 10) CLI Integration

### 10.1 Basic Structure

```rust
// aura-cli/src/main.rs

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = CliConfig::from_env();
    let session = Session::new(config).await?;
    
    // REPL loop
    loop {
        let input = prompt("> ")?;
        
        match parse_command(&input) {
            Command::Prompt(text) => {
                session.submit_prompt(&text).await?;
                session.stream_until_complete().await?;
            }
            Command::Approve => session.approve_pending().await?,
            Command::Deny => session.deny_pending().await?,
            Command::Status => session.print_status()?,
            Command::History(n) => session.print_history(n)?,
            Command::Quit => break,
        }
    }
    
    Ok(())
}
```

### 10.2 Slash Commands

| Command | Description |
|---------|-------------|
| `/status` | Show agent status and pending approvals |
| `/history [n]` | Show last n record entries |
| `/approve` | Approve pending tool request |
| `/deny` | Deny pending tool request |
| `/diff` | Show pending file changes |
| `/quit` | Exit CLI |

---

## 11) Configuration

### 11.1 Environment Variables

```bash
# Provider selection
AURA_MODEL_PROVIDER=anthropic

# Anthropic settings
AURA_ANTHROPIC_API_KEY=sk-ant-...
AURA_ANTHROPIC_MODEL=claude-sonnet-4-20250514

# Turn processor settings
AURA_MAX_STEPS_PER_TURN=25
AURA_MAX_TOOL_CALLS_PER_STEP=8
AURA_MODEL_TIMEOUT_MS=60000
AURA_TOOL_TIMEOUT_MS=30000

# Storage
AURA_DATA_DIR=./aura_data
AURA_WORKSPACE_ROOT=./aura_data/workspaces
```

---

## 12) Implementation Checklist

### Phase 1: Provider Abstraction
- [ ] Define normalized types in `aura-reasoner/src/types.rs`
- [ ] Define `ModelProvider` trait
- [ ] Update `MockReasoner` to implement `ModelProvider`

### Phase 2: Anthropic Provider
- [ ] Add `anthropic-sdk-rust` dependency
- [ ] Implement `AnthropicProvider`
- [ ] Add conversion functions (AURA ↔ Anthropic types)
- [ ] Test with simple completions

### Phase 3: Tool Registry
- [ ] Create `ToolRegistry` trait
- [ ] Define JSON schemas for all MVP tools
- [ ] Implement `DefaultToolRegistry`

### Phase 4: Turn Processor
- [ ] Create `TurnProcessor` struct
- [ ] Implement conversation loop
- [ ] Add tool execution integration
- [ ] Add recording for replay

### Phase 5: Permission System
- [ ] Define permission levels
- [ ] Implement policy checks for tools
- [ ] Add approval request/response flow

### Phase 6: CLI
- [ ] Create `aura-cli` crate
- [ ] Implement REPL loop
- [ ] Add slash commands
- [ ] Add approval prompts

### Phase 7: Integration
- [ ] Wire Turn Processor into worker loop
- [ ] Update swarm to use new provider system
- [ ] Deprecate TS gateway usage

---

## 13) Acceptance Criteria

### Must Work
- [ ] CLI prompt triggers multi-step conversation
- [ ] Model can read files via `fs.read`
- [ ] Model can search code via `search.code`
- [ ] File writes require user approval
- [ ] All model outputs + tool results are recorded
- [ ] Replay reconstructs state without calling model/tools

### Provider Independence
- [ ] Switching providers only changes `aura-reasoner` config
- [ ] Kernel code unchanged when swapping providers
- [ ] Tool definitions work with any provider

---

## 14) Migration Path

### Deprecating TypeScript Gateway

1. Add Rust provider alongside existing TS gateway
2. Add config flag to select provider: `AURA_USE_RUST_PROVIDER=true`
3. Test thoroughly with both paths
4. Default to Rust provider
5. Remove TS gateway code and dependencies

### Backwards Compatibility

- Existing `Reasoner` trait can wrap `ModelProvider` for compatibility
- Existing `ProposeRequest` maps to `ModelRequest`
- Existing `ProposalSet` maps from `ModelResponse` tool_use blocks
