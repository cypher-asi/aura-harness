# AURA Remote Machine Wiring - Spec 06

**Status**: Design-ready  
**Builds on**: spec-02-interactive-runtime.md, spec-05-keystore.md  
**Goal**: Wire agents to remote machines via SSH with unified tool API

---

## 1) Purpose

Enable agents to execute filesystem and command operations on remote machines via SSH. This extends the existing tool system with a `target` parameter rather than creating separate remote tools.

### Why This Matters

1. **Cloud GPU Access** - Run ML workloads on remote GPU servers
2. **Multi-Environment Deployment** - Deploy to staging/production servers
3. **Distributed Workflows** - Orchestrate tasks across multiple machines
4. **Secure Access** - SSH keys managed by keystore with session-based unlock

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Protocol | SSH | No agent needed on targets, mature security model |
| Machine Scope | Per-Swarm | Shared like keys, simplifies management |
| Tool API | `target` parameter | Unified API, no tool proliferation |
| Connection Model | Session-based pooling | Avoid reconnection overhead within turn |

---

## 2) Architecture

### 2.1 Crate Layout

```
aura-remote/                    # Connection infrastructure only
├── src/
│   ├── lib.rs                  # Public exports
│   ├── types.rs                # MachineId, MachineConfig
│   ├── session.rs              # SshSession, SshSessionManager
│   └── error.rs

aura-tools/                     # All tool implementations
├── src/
│   ├── fs_tools.rs             # Local filesystem (existing)
│   ├── ssh_tools.rs            # Remote operations via SshSession (NEW)
│   ├── executor.rs             # Routes based on target parameter
│   └── ...
```

**Dependencies:**
- `aura-tools` depends on `aura-remote` for `SshSession`
- `aura-remote` depends on `aura-keystore` for SSH key retrieval

### 2.2 Component Diagram

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Terminal UI                                   │
│  ┌─────────────────────────────────────────────────────────────────┐ │
│  │ > user input...                                                  │ │
│  ├─────────────────────────────────────────────────────────────────┤ │
│  │ Model: claude-sonnet-4-2025  │  Machine: prod-gpu-01 ●          │ │
│  └─────────────────────────────────────────────────────────────────┘ │
│         │ Ctrl+M cycles machines                                      │
│         ▼                                                             │
└──────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────────────┐
│                        ToolExecutor                                   │
│   fs_read { target: "prod-gpu-01", path: "/app/log" }                │
│         │                                                             │
│         ├── target = "local" or None ──► fs_tools (std::fs)          │
│         │                                                             │
│         └── target = machine_id ──┬──► SwarmConfig.machines          │
│                                   │         │                         │
│                                   │         ▼                         │
│                                   │    MachineConfig                  │
│                                   │    └── ssh_key_id ───────────┐   │
│                                   │                               │   │
│                                   ▼                               ▼   │
│                          ┌─────────────┐               ┌───────────┐ │
│                          │ ssh_tools   │               │ Keystore  │ │
│                          │ (aura-tools)│◄──────────────│ get_key() │ │
│                          └──────┬──────┘               └───────────┘ │
│                                 │                                     │
│                                 ▼                                     │
│                          ┌─────────────────┐                         │
│                          │ SshSession      │                         │
│                          │ (aura-remote)   │                         │
│                          │ • sftp_read()   │                         │
│                          │ • sftp_write()  │                         │
│                          │ • exec()        │                         │
│                          └─────────────────┘                         │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 3) Data Model

### 3.1 Per-Swarm Machine Configuration

Machines are defined at the **Swarm level** (like keys). All agents in a swarm can access any configured machine.

```rust
// In aura-swarm or aura-core
pub struct SwarmConfig {
    pub swarm_id: SwarmId,
    pub machines: HashMap<MachineId, MachineConfig>,
    // ... existing fields ...
}

/// Unique identifier for a machine within a swarm.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MachineId(pub String);

/// Configuration for a remote machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    pub machine_id: MachineId,
    pub name: String,                     // Human-readable name for UI
    pub host: String,                     // Hostname or IP address
    pub port: u16,                        // SSH port (default: 22)
    pub username: String,                 // SSH username
    pub ssh_key_id: KeyId,                // Reference to key in keystore
    pub workspace_root: PathBuf,          // Sandbox root on remote machine
    pub connection_timeout_ms: u64,       // Connection timeout (default: 30000)
    pub idle_timeout_ms: u64,             // Idle disconnect timeout (default: 300000)
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            machine_id: MachineId("".into()),
            name: String::new(),
            host: String::new(),
            port: 22,
            username: String::new(),
            ssh_key_id: KeyId::default(),
            workspace_root: PathBuf::from("/home"),
            connection_timeout_ms: 30_000,
            idle_timeout_ms: 300_000,
        }
    }
}
```

### 3.2 SSH Session Types

```rust
// In aura-remote/src/session.rs

/// Handle to an established SSH connection.
pub struct SshSession {
    session: russh::client::Handle,
    machine_id: MachineId,
    connected_at: Instant,
    last_activity: Instant,
}

impl SshSession {
    /// Read a file via SFTP.
    pub async fn sftp_read(&self, path: &Path) -> Result<Vec<u8>, RemoteError>;
    
    /// Write a file via SFTP.
    pub async fn sftp_write(&self, path: &Path, content: &[u8]) -> Result<(), RemoteError>;
    
    /// List directory via SFTP.
    pub async fn sftp_list(&self, path: &Path) -> Result<Vec<DirEntry>, RemoteError>;
    
    /// Execute a command and return output.
    pub async fn exec(&self, command: &str) -> Result<CommandOutput, RemoteError>;
    
    /// Check if connection is still alive.
    pub async fn is_alive(&self) -> bool;
}

/// Manages SSH session lifecycle and pooling.
pub struct SshSessionManager {
    sessions: HashMap<MachineId, SshSession>,
    keystore: Arc<dyn KeyStore>,
}

impl SshSessionManager {
    /// Get or establish a session for the given machine.
    pub async fn get_session(
        &mut self,
        config: &MachineConfig,
    ) -> Result<&SshSession, RemoteError>;
    
    /// Close a session.
    pub async fn close_session(&mut self, machine_id: &MachineId);
    
    /// Close all idle sessions.
    pub async fn cleanup_idle(&mut self);
    
    /// Get connection status for a machine.
    pub fn status(&self, machine_id: &MachineId) -> ConnectionStatus;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}
```

---

## 4) Keystore Integration

SSH keys are stored in the keystore (spec-05) with `SessionUnlock` protection tier.

### 4.1 Connection Flow

```
1. Agent calls fs_read { target: "prod-gpu-01", path: "..." }
2. ToolExecutor looks up MachineConfig for "prod-gpu-01"
3. Get ssh_key_id from MachineConfig
4. Request key from keystore (requires session unlock)
   └── If session not unlocked, prompt user for password
5. SshSessionManager establishes or reuses connection
6. ssh_tools executes operation via SshSession
7. Result returned to agent
```

### 4.2 Key Protection Model

- SSH keys use **SessionUnlock** protection tier
- First remote tool use prompts for session unlock if not already unlocked
- Session stays unlocked for configured timeout (default: 4 hours)
- Key material never exposed to agents - only used internally for connections

---

## 5) Unified Tool API

### 5.1 Target Parameter

Instead of separate `remote.*` tools, extend existing tools with an optional `target` parameter:

```rust
// Local (default)
fs_read { path: "src/main.rs" }
fs_read { target: "local", path: "src/main.rs" }

// Remote
fs_read { target: "prod-gpu-01", path: "/app/logs/error.log" }
cmd_run { target: "staging", program: "docker", args: ["ps"] }
```

### 5.2 Tools with Target Support

| Tool | Description | Permission |
|------|-------------|------------|
| `fs_ls` | List directory contents | AlwaysAllow |
| `fs_read` | Read file contents | AlwaysAllow |
| `fs_write` | Write file contents | AskOnce |
| `fs_stat` | Get file metadata | AlwaysAllow |
| `fs_edit` | Edit file with search/replace | AskOnce |
| `cmd_run` | Execute shell command | AskOnce |

### 5.3 Tool Schema Example

```json
{
  "name": "fs_read",
  "description": "Read file contents from local or remote machine",
  "parameters": {
    "target": {
      "type": "string",
      "description": "Machine ID or 'local'. Defaults to 'local' if omitted.",
      "optional": true
    },
    "path": {
      "type": "string",
      "description": "Path to file (relative to workspace root)",
      "required": true
    },
    "max_bytes": {
      "type": "integer",
      "description": "Maximum bytes to read",
      "optional": true
    }
  }
}
```

### 5.4 ToolExecutor Routing

```rust
// In aura-tools/src/executor.rs

impl ToolExecutor {
    fn execute_tool(&self, ctx: &ExecuteContext, tool_call: &ToolCall) -> Result<ToolResult, ToolError> {
        // Extract target from tool args
        let target = tool_call.args.get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("local");
        
        if target == "local" {
            // Use existing fs_tools
            self.execute_local(ctx, tool_call)
        } else {
            // Get session and use ssh_tools
            let session = ctx.sessions
                .as_ref()
                .ok_or(ToolError::NoSessionProvider)?
                .get_session(&MachineId(target.into()))?;
            self.execute_remote(session, tool_call)
        }
    }
}
```

---

## 6) Terminal UI

### 6.1 Status Bar Below Input

Add a status bar below the chat input with model and machine selectors:

```
┌─────────────────────────────────────────────────────────────┐
│  AURA v0.1.0  │  Agent: coder-01                     Ready  │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Chat / Content Panels                                      │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│ > user input here...                                        │
├─────────────────────────────────────────────────────────────┤
│  Model: claude-sonnet-4-2025   │  Machine: prod-gpu-01 ●    │
└─────────────────────────────────────────────────────────────┘
```

### 6.2 Status Bar Elements

**Model selector** (mock for now):
- Displays current model: `Model: claude-sonnet-4-2025`
- Cycle with `Ctrl+O`

**Machine selector** with connection indicator:
- `Machine: local` - default, no indicator
- `Machine: prod-gpu-01 ●` - connected (green dot)
- `Machine: staging ◐` - connecting (yellow half-circle)
- `Machine: dev-box ○` - disconnected (gray circle)
- `Machine: broken ✗` - error (red x)

### 6.3 Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+M` | Cycle through available machines |
| `Ctrl+O` | Cycle through available models (mock) |
| `/machine <id>` | Switch to specific machine |
| `/machines` | List all machines with status |

### 6.4 App State Extensions

```rust
// In aura-terminal/src/app.rs

pub struct App {
    // ... existing fields ...
    
    // Model selection (mock for now)
    active_model_idx: usize,
    available_models: Vec<String>,
    
    // Machine selection
    active_machine: Option<MachineId>,
    machines: Vec<MachineSummary>,
    machine_status: HashMap<MachineId, ConnectionStatus>,
}

pub struct MachineSummary {
    pub id: String,
    pub name: String,
    pub host: String,
}
```

### 6.5 UiEvent/UiCommand Extensions

```rust
// In aura-terminal/src/events.rs

pub enum UiEvent {
    // ... existing events ...
    SelectMachine(Option<MachineId>),  // None = local
    ConnectMachine(MachineId),
    DisconnectMachine(MachineId),
    RefreshMachines,
}

pub enum UiCommand {
    // ... existing commands ...
    SetMachines(Vec<MachineSummary>),
    SetActiveMachine(Option<MachineId>),
    SetMachineStatus { id: MachineId, status: ConnectionStatus },
}
```

---

## 7) CLI Commands

### 7.1 Machine Management

```
/machine                    Show current active machine
/machine list               List all configured machines with status
/machine <id>               Switch active machine to <id>
/machine local              Switch back to local
/machine status             Show detailed connection status
/machine connect <id>       Establish connection to machine
/machine disconnect <id>    Close connection to machine
/machine add                Interactive wizard to add a machine
/machine remove <id>        Remove a machine configuration
```

### 7.2 Command Implementation

```rust
// In aura-cli/src/main.rs

enum Command {
    // ... existing commands ...
    Machine(MachineSubcommand),
}

enum MachineSubcommand {
    List,
    Select(String),
    Status,
    Connect(String),
    Disconnect(String),
    Add,
    Remove(String),
}
```

---

## 8) REST API

### 8.1 Machine CRUD

```
GET    /api/v1/machines              List all machines
GET    /api/v1/machines/{id}         Get machine config
POST   /api/v1/machines              Add machine
PUT    /api/v1/machines/{id}         Update machine
DELETE /api/v1/machines/{id}         Remove machine
```

### 8.2 Connection Management

```
POST   /api/v1/machines/{id}/connect      Establish connection
POST   /api/v1/machines/{id}/disconnect   Close connection
GET    /api/v1/machines/{id}/status       Connection status + health
```

### 8.3 Response Examples

**GET /api/v1/machines**
```json
{
  "machines": [
    {
      "machine_id": "prod-gpu-01",
      "name": "Production GPU Server",
      "host": "gpu01.example.com",
      "port": 22,
      "username": "aura",
      "status": "connected"
    }
  ]
}
```

**GET /api/v1/machines/prod-gpu-01/status**
```json
{
  "machine_id": "prod-gpu-01",
  "status": "connected",
  "connected_at": "2026-01-09T10:30:00Z",
  "last_activity": "2026-01-09T10:45:00Z",
  "latency_ms": 45,
  "session_count": 1
}
```

---

## 9) Security Considerations

- **Host Allowlist**: Only pre-configured machines in SwarmConfig can be accessed
- **Path Sandboxing**: Remote paths constrained to `workspace_root`
- **Command Validation**: Optional command allowlist per machine
- **Audit Logging**: All remote operations logged to record
- **Key Protection**: SSH keys use SessionUnlock tier (requires user password)
- **No Key Exposure**: Key material never exposed to agents

---

## 10) Error Handling

### 10.1 Error Types

```rust
// In aura-remote/src/error.rs

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("Machine not found: {0}")]
    MachineNotFound(MachineId),
    
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    
    #[error("Connection timeout")]
    ConnectionTimeout,
    
    #[error("Authentication failed")]
    AuthenticationFailed,
    
    #[error("Session not unlocked")]
    SessionNotUnlocked,
    
    #[error("Path outside workspace: {0}")]
    PathOutsideWorkspace(PathBuf),
    
    #[error("SFTP error: {0}")]
    SftpError(String),
    
    #[error("Command execution failed: {0}")]
    CommandFailed(String),
}
```

### 10.2 Timeout Handling

- Connection attempts timeout after `connection_timeout_ms`
- Idle sessions closed after `idle_timeout_ms`
- Long-running commands follow existing cmd_run timeout behavior

---

## 11) Implementation Checklist

### Phase 1: Core Infrastructure
- [ ] Create `aura-remote` crate with `MachineId`, `MachineConfig` types
- [ ] Implement `SshSession` with SFTP and exec operations
- [ ] Implement `SshSessionManager` with connection pooling
- [ ] Add keystore integration for SSH key retrieval
- [ ] Add `machines` field to `SwarmConfig`

### Phase 2: Tool Integration
- [ ] Add `ssh_tools.rs` to `aura-tools`
- [ ] Add `target` parameter to fs_ls, fs_read, fs_write, fs_stat, fs_edit, cmd_run
- [ ] Update `ToolExecutor` to route based on target
- [ ] Add `SessionProvider` to `ExecuteContext`
- [ ] Update tool schemas in registry

### Phase 3: Terminal UI
- [ ] Add status bar below input in renderer
- [ ] Add model selector (mock)
- [ ] Add machine selector with connection indicators
- [ ] Implement `Ctrl+M` and `Ctrl+O` shortcuts
- [ ] Add `/machine` command parsing

### Phase 4: CLI and API
- [ ] Add `/machine` subcommands to CLI
- [ ] Implement interactive machine add wizard
- [ ] Add REST endpoints for machine CRUD
- [ ] Add REST endpoints for connection management

### Phase 5: Polish
- [ ] Add audit logging for remote operations
- [ ] Add health checks and reconnection logic
- [ ] Add machine configuration validation
- [ ] Write integration tests
- [ ] Update documentation
