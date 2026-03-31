# Aura Swarm MVP - Implementation Progress

## Overview

Implementing the Aura Swarm as specified in:
- `specs/spec-01.md` - MVP Foundation (Complete)
- `specs/spec-02.md` - Interactive Coding Runtime (In Progress)

**Start Date:** 2026-01-08
**Last Updated:** 2026-01-08

---

## Build Requirements

### Windows
RocksDB requires LLVM/Clang to build. Install via:
```powershell
winget install LLVM.LLVM
# Set environment variable:
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
```

### All Platforms
- Rust 1.75+ (via rustup)
- rustfmt: `rustup component add rustfmt`
- clippy: `rustup component add clippy`

---

## Implementation Phases

### Phase 1: Core Foundation (`aura-core`) 
**Status:** 🟢 Complete

Core types, IDs, serialization, and error handling.

- [x] Workspace + Cargo.toml setup
- [x] `AgentId` newtype (`[u8; 32]`)
- [x] `TxId` newtype (`[u8; 32]`)
- [x] `ActionId` newtype (`[u8; 16]`)
- [x] `Transaction` struct + `TransactionKind` enum
- [x] `Action` struct + `ActionKind` enum
- [x] `Effect` struct + `EffectKind` + `EffectStatus` enums
- [x] `Proposal` + `ProposalSet` structs
- [x] `Decision` struct
- [x] `RecordEntry` struct
- [x] `Identity` struct
- [x] `ToolCall` + `ToolResult` structs
- [x] Error types with `thiserror`
- [x] Serde serialization (JSON)
- [x] Hashing utilities (blake3)
- [x] Unit tests for serialization round-trips

---

### Phase 2: Storage Layer (`aura-store`)
**Status:** 🟢 Complete (code written, requires LLVM to build)

RocksDB implementation with column families and atomic commits.

- [x] RocksDB dependency setup
- [x] Column family definitions (record, agent_meta, inbox)
- [x] Key encoding/decoding utilities
- [x] `Store` trait definition
- [x] `RocksStore` implementation
- [x] `enqueue_tx` - durable inbox write
- [x] `dequeue_tx` - peek + return inbox item
- [x] `get_head_seq` - read agent head
- [x] `append_entry_atomic` - WriteBatch commit
- [x] `scan_record` - range scan for record window
- [x] Agent metadata operations
- [x] Unit tests for atomicity
- [x] Unit tests for key ordering

---

### Phase 3: Executor Framework (`aura-executor`)
**Status:** 🟢 Complete

Executor trait and router for dispatching actions.

- [x] `Executor` trait definition
- [x] `ExecuteContext` struct
- [x] `ExecuteLimits` struct
- [x] `ExecutorRouter` implementation
- [x] Action dispatch by kind
- [x] `NoOpExecutor` stub
- [x] Unit tests

---

### Phase 4: Tools (`aura-tools`)
**Status:** 🟢 Complete (code written, requires LLVM to build)

Filesystem and command tools with sandbox.

- [x] `ToolCall` struct (in aura-core)
- [x] `ToolResult` struct (in aura-core)
- [x] `ToolExecutor` implementation
- [x] `fs.ls` - directory listing
- [x] `fs.read` - file read with limits
- [x] `fs.stat` - file metadata
- [x] Sandbox path validation
- [x] Path canonicalization + prefix check
- [x] `cmd.run` - command execution (disabled by default)
- [x] Timeout enforcement structure
- [x] Output size limits
- [x] Unit tests for path traversal prevention

---

### Phase 5: Reasoner Client (`aura-reasoner`)
**Status:** 🟢 Complete

HTTP client to TypeScript gateway.

- [x] `Reasoner` trait definition
- [x] `ProposeRequest` struct
- [x] `RecordSummary` struct
- [x] `ReasonerConfig` struct
- [x] HTTP client implementation (reqwest)
- [x] Timeout + retry logic
- [x] Error handling
- [x] `MockReasoner` for testing
- [x] Unit tests

---

### Phase 6: Kernel (`aura-kernel`)
**Status:** 🟢 Complete (code written, requires LLVM to build)

Deterministic kernel with context building and policy.

- [x] `Kernel` struct
- [x] `KernelConfig` struct
- [x] Context builder (record window)
- [x] `context_hash` computation
- [x] Policy engine (`Policy` struct)
- [x] Action kind allowlist
- [x] Tool allowlist
- [x] Proposal → Action conversion
- [x] Execution orchestration
- [x] `RecordEntry` construction
- [x] Replay mode (skip Reasoner/Tools)
- [x] Unit tests for determinism
- [x] Unit tests for policy enforcement

---

### Phase 7: Swarm Runtime (`aura-node`)
**Status:** 🟢 Complete (code written, requires LLVM to build)

HTTP router, scheduler, and worker management.

- [x] Axum HTTP router setup
- [x] `POST /tx` endpoint
- [x] `GET /agents/{id}/head` endpoint
- [x] `GET /agents/{id}/record` endpoint
- [x] `GET /health` endpoint
- [x] Per-agent lock table (DashMap + Mutex)
- [x] Scheduler (pick agents with inbox items)
- [x] Worker loop implementation
- [x] `NodeConfig` struct

---

### Phase 8: TypeScript Gateway (`aura-gateway-ts`)
**Status:** 🟢 Complete

Claude Code SDK integration.

- [x] Node.js project setup
- [x] Express server
- [x] `POST /propose` endpoint
- [x] Claude SDK integration (Anthropic)
- [x] Propose-only mode (no tool execution)
- [x] Request/response validation (Zod)
- [x] Error handling
- [x] Health endpoint
- [x] README with API documentation

---

### Phase 9: Integration & Testing
**Status:** 🔴 Not Started

End-to-end tests and verification.

- [ ] Full pipeline test (tx → record)
- [ ] Determinism test (replay)
- [ ] Atomicity test (simulated crash)
- [ ] Concurrency test (parallel agents)
- [ ] Tool sandbox test (path traversal)
- [ ] Performance benchmarks (optional)

---

## Spec-02: Interactive Coding Runtime (Rust-only)

### Phase 10: Provider Abstraction (`aura-reasoner` refactor)
**Status:** 🔴 Not Started

Provider-agnostic model interface.

- [ ] Define normalized `Message`, `ContentBlock` types
- [ ] Define `ToolDefinition` struct (JSON Schema)
- [ ] Define `ModelRequest` / `ModelResponse` structs
- [ ] Define `ModelProvider` trait
- [ ] Update `MockReasoner` to implement `ModelProvider`
- [ ] Add `ProviderFactory` for provider selection

---

### Phase 11: Anthropic Provider
**Status:** 🔴 Not Started

Direct Anthropic API integration (no TypeScript gateway).

- [ ] Add `anthropic-sdk-rust` dependency
- [ ] Implement `AnthropicProvider`
- [ ] AURA → Anthropic type conversion
- [ ] Anthropic → AURA type conversion
- [ ] Tool schema conversion
- [ ] Unit tests with mock responses

---

### Phase 12: Tool Registry
**Status:** 🔴 Not Started

Centralized tool definitions with JSON Schema.

- [ ] Define `ToolRegistry` trait
- [ ] Implement `DefaultToolRegistry`
- [ ] JSON schemas for: fs.ls, fs.read, fs.stat, fs.write, fs.edit
- [ ] JSON schema for: search.code (ripgrep)
- [ ] JSON schema for: cmd.run (gated)
- [ ] Permission level mapping

---

### Phase 13: Turn Processor (`aura-kernel` extension)
**Status:** 🔴 Not Started

Claude Code-like multi-step conversation loop.

- [ ] Define `TurnConfig` struct
- [ ] Implement `TurnProcessor` struct
- [ ] Conversation loop (model → tool_use → tool_result → repeat)
- [ ] Tool call extraction from `tool_use` blocks
- [ ] Tool result injection as `tool_result` blocks
- [ ] Step recording for replay
- [ ] Budget enforcement (max steps, max tool calls)
- [ ] Timeout handling

---

### Phase 14: Permission System
**Status:** 🔴 Not Started

Approval flow for sensitive operations.

- [ ] Define `PermissionLevel` enum
- [ ] Default permission mapping per tool
- [ ] Approval request generation
- [ ] Approval response handling
- [ ] Session-level permission caching (AskOnce)

---

### Phase 15: CLI (`aura-cli`)
**Status:** 🔴 Not Started

Interactive command-line interface.

- [ ] Create `aura-cli` crate
- [ ] REPL loop with prompt
- [ ] Transaction submission
- [ ] Record streaming / tailing
- [ ] Slash commands (/status, /history, /approve, /deny)
- [ ] Approval prompts inline

---

### Phase 16: Gateway Deprecation
**Status:** 🔴 Not Started

Remove TypeScript gateway dependency.

- [ ] Add provider selection config
- [ ] Test Rust provider end-to-end
- [ ] Default to Rust provider
- [ ] Mark `aura-gateway-ts` as deprecated
- [ ] Update documentation

---

## Legend

- 🔴 Not Started
- 🟡 In Progress
- 🟢 Complete
- ⏸️ Blocked

---

## Crate Structure

```
aura_os/
├── Cargo.toml           # Workspace manifest
├── rust-toolchain.toml  # Toolchain pinning
├── src/main.rs          # Server entry point
├── .gitignore
├── PROGRESS.md          # This file
├── specs/
│   ├── spec-01.md       # MVP specification
│   └── spec-02.md       # Interactive runtime spec
├── .cursor/
│   └── rules.md         # Rust coding conventions
├── aura-core/           # Core types, IDs, errors
├── aura-store/          # RocksDB storage
├── aura-executor/       # Executor trait & router
├── aura-tools/          # Tool executor (fs, cmd)
├── aura-reasoner/       # Model provider abstraction + Anthropic
├── aura-kernel/         # Deterministic kernel + Turn Processor
├── aura-node/           # HTTP router, scheduler
├── aura-cli/            # Interactive CLI (planned)
└── aura-gateway-ts/     # TypeScript gateway (deprecated)
```

---

## Notes

### 2026-01-08: Initial Implementation

- Created full workspace structure with 7 Rust crates
- Implemented all core types with serialization
- Implemented RocksDB store with atomic WriteBatch commits
- Implemented executor framework with tool executor
- Implemented sandboxed filesystem tools (ls, read, stat)
- Implemented reasoner client with mock for testing
- Implemented deterministic kernel with policy engine
- Implemented swarm runtime with HTTP API and scheduler
- Build verified for non-native crates (aura-core, aura-executor, aura-reasoner)
- RocksDB crates require LLVM/Clang installation on Windows

### Key Design Decisions

1. **Atomic Commits**: All state changes use RocksDB WriteBatch for atomicity
2. **Per-Agent Locking**: DashMap with Mutex ensures single-writer per agent
3. **Replay Mode**: Kernel can skip reasoner/tools for deterministic replay
4. **Sandbox**: All tool paths are canonicalized and validated against workspace root
5. **Policy Engine**: Allowlists for action kinds and tools, applied deterministically
