<p align="center">
  <strong style="font-size: 2em;">AURA SWARM</strong>
</p>

---

<p align="center">
  <strong>Deterministic Multi-Agent Runtime</strong><br/>
  An append-only, pluggable-reasoning runtime for running many agents concurrently with sandboxed tool execution.
</p>

<p align="center">
  <a href="#overview">Overview</a> &nbsp;·&nbsp;
  <a href="#quick-start">Quick Start</a> &nbsp;·&nbsp;
  <a href="#principles">Principles</a> &nbsp;·&nbsp;
  <a href="#architecture">Architecture</a> &nbsp;·&nbsp;
  <a href="#configuration">Configuration</a>
</p>

## Overview

Aura Swarm is a system for running many deterministic agents concurrently. Every agent maintains an append-only record log, a deterministic kernel advances state by consuming transactions, and reasoning is delegated to a pluggable LLM provider (proxy-routed or direct Anthropic API). All side effects flow through authorized executors so the full history is replayable from the record alone.

The runtime supports interactive terminal sessions (TUI), headless server deployments, and long-running automaton workflows -- all backed by the same kernel, storage, and reasoning stack.

---

What are the core ideas behind Aura Swarm?

1. **The Record:** The fundamental unit of truth. Every agent has an append-only log of record entries, strictly ordered by sequence number. All state is derivable from the record; there is no hidden state.

2. **The Kernel:** A deterministic processor that builds context from the record, calls the reasoner, enforces policy, executes actions through the executor, and commits new entries. Given the same record, the kernel always produces the same output.

3. **Reasoning:** Probabilistic LLM calls are isolated behind a provider trait. The default path routes through a JWT-authenticated proxy (`aura-router`); alternatively, calls go directly to the Anthropic API. A mock provider is available for testing.

4. **Tools & Executors:** All side effects (filesystem, shell commands, domain APIs, automaton actions) are explicit. The executor router dispatches authorized actions and captures structured effects, keeping the kernel boundary clean.

Three binaries ship in this workspace:

- **aura** -- the primary binary: interactive TUI (default) or headless node (`aura run --ui none`).
- **aura-node** -- standalone headless server with HTTP + WebSocket API.
- **aura-cli** -- interactive REPL with slash commands (`/status`, `/login`, etc.).

### Quick Start

```sh
# Build
cargo build --release

# Run the TUI (proxy mode -- no API key needed)
cargo run

# Or run headless
cargo run -- run --ui none
```

To use direct Anthropic access instead of the proxy:

```sh
AURA_LLM_ROUTING=direct AURA_ANTHROPIC_API_KEY=sk-ant-... cargo run
```

#### Docker

```sh
docker build -t aura .
docker run -p 8080:8080 aura
```

#### Optional: TypeScript Gateway

The `aura-gateway-ts` Express service exposes a `/propose` endpoint for local LLM routing:

```sh
cd aura-gateway-ts && npm install && npm run build
ANTHROPIC_API_KEY=your-key npm start   # listens on :3000
```

## Principles

1. **Per-Agent Order** -- Record entries are strictly ordered by sequence number; no reordering, no gaps.
2. **Atomic Commit** -- Transaction processing is all-or-nothing via RocksDB batch writes.
3. **No Hidden State** -- All state is replayable from the record. If it is not in the log, it did not happen.
4. **Deterministic Kernel** -- The kernel advances only by consuming transactions. Same input, same output.
5. **Explicit Side Effects** -- Every tool call flows through an authorized executor; effects are captured and recorded.
6. **Open Source** -- MIT-licensed Rust workspace. Every layer is auditable and reusable.

## Architecture

| Crate | Description |
|---|---|
| `aura-core` | Shared domain types, strongly-typed IDs, hashing, serialization, and model identifiers |
| `aura-store` | RocksDB persistence: record log, agent metadata, and inbox with atomic batch commits |
| `aura-executor` | `Executor` trait and `ExecutorRouter` bridging deterministic kernel logic from side effects |
| `aura-tools` | Tool registry, sandboxed filesystem and command execution, domain tool wiring |
| `aura-reasoner` | Provider-agnostic `ModelProvider` trait: Anthropic HTTP, proxy routing, mock, retries |
| `aura-kernel` | Deterministic single-step kernel: context, reasoning, policy, execution, record commit |
| `aura-runtime` | Multi-step `TurnProcessor` agentic loop and `ProcessManager` for long-running commands |
| `aura-terminal` | Ratatui-based terminal UI library: themes, components, input handling, layout |
| `aura-cli` | Interactive REPL with slash commands, wired to session and agent handling |
| `aura-agent` | `AgentLoop` orchestration: blocking detection, compaction, budgets, sanitization, task runners |
| `aura-agent-fileops` | Structured file operations, path validation, workspace mapping, edit parsing |
| `aura-agent-verify` | Build/test verification with optional LLM-driven fix loops, snapshots, and rollback |
| `aura-auth` | zOS login client and JWT credential store (`~/.aura/credentials.json`) for proxy mode |
| `aura-automaton` | Automaton lifecycle, scheduling, runtime, state, and built-in automatons (chat, dev loop, etc.) |
| `aura-node` | HTTP router, scheduler, and per-agent worker loops with single-writer guarantee |
| `aura-protocol` | Serde types for the `/stream` WebSocket API (session init, messages, events, approvals) |
| `aura-session` | Shared bootstrap for CLI/TUI: identity setup, auth loading, provider selection |
| `aura-gateway-ts` | Optional Express gateway for local `/propose` LLM routing (TypeScript) |

## Project Structure

```
aura-harness/
  Cargo.toml                # workspace root + `aura` binary
  Dockerfile                # multi-stage build, headless on :8080
  .env.example              # environment variable template
  src/
    main.rs                 # CLI entry: TUI, headless, login/logout/whoami
    cli.rs                  # clap command definitions
    event_loop.rs           # terminal event loop
    api_server.rs           # embedded /health endpoint for TUI mode
    record_loader.rs        # record loading utilities
  crates/
    aura-core/              # shared types, IDs, hashing
    aura-store/             # RocksDB storage backend
    aura-executor/          # executor trait and router
    aura-tools/             # tool registry, sandboxed FS/cmd
    aura-reasoner/          # LLM provider abstraction
    aura-kernel/            # deterministic kernel
    aura-runtime/           # turn processor, process manager
    aura-terminal/          # ratatui TUI library
    aura-cli/               # interactive REPL (separate binary)
    aura-agent/             # agent loop orchestration
    aura-agent-fileops/     # structured file operations
    aura-agent-verify/      # build/test verification + fix loops
    aura-auth/              # zOS login, credential store
    aura-automaton/         # automaton lifecycle and built-ins
    aura-node/              # HTTP server, scheduler, workers
    aura-protocol/          # WebSocket stream API types
    aura-session/           # shared CLI/TUI bootstrap
  aura-gateway-ts/          # optional TypeScript gateway
    src/index.ts
    src/reasoner.ts
  tests/                    # integration, e2e, proptest
  specs/                    # design specifications
  docs/                     # supplementary documentation
```

## System Diagram

```
                             ┌──────────────────────────────────┐
                             │           Entry Points           │
                             │  aura (TUI)  ·  aura --ui none  │
                             │  aura-node   ·  aura-cli        │
                             └──────────────┬───────────────────┘
                                            │
                             ┌──────────────▼───────────────────┐
                             │         HTTP / WebSocket         │
                             │     Router  (Axum on :8080)      │
                             │  /health /tx /agents /stream     │
                             │  /automaton/*                    │
                             └──────────────┬───────────────────┘
                                            │
                    ┌───────────────────────▼──────────────────────────┐
                    │                  Scheduler                       │
                    │   per-agent tokio::Mutex  ·  DashMap registry   │
                    └───┬──────────────┬──────────────┬───────────────┘
                        │              │              │
                   ┌────▼────┐   ┌─────▼────┐   ┌────▼────┐
                   │ Worker  │   │  Worker  │   │ Worker  │  (one per agent)
                   │  Lock   │   │  Lock    │   │  Lock   │
                   │ Dequeue │   │ Dequeue  │   │ Dequeue │
                   │ Process │   │ Process  │   │ Process │
                   │ Commit  │   │ Commit   │   │ Commit  │
                   └────┬────┘   └────┬─────┘   └────┬────┘
                        └─────────────┼──────────────┘
                                      │
                    ┌─────────────────▼───────────────────────────────┐
                    │              Kernel (Deterministic)              │
                    │  Build context  ·  Call Reasoner  ·  Policy     │
                    │  Execute actions  ·  Build RecordEntry          │
                    └─────┬──────────────────┬──────────────┬────────┘
                          │                  │              │
             ┌────────────▼─────┐  ┌─────────▼────┐  ┌─────▼──────────┐
             │     Reasoner     │  │   Executor   │  │     Store      │
             │                  │  │   (Tools)    │  │   (RocksDB)    │
             │  proxy ──► Router│  │  FS · Cmd    │  │  record        │
             │  direct ► Claude │  │  Domain      │  │  agent_meta    │
             └──────┬───────────┘  │  Automaton   │  │  inbox         │
                    │              └──────────────┘  └────────────────┘
                    │
      ┌─────────────┼──────────────────────────────┐
      │             │                              │
 ┌────▼──────┐ ┌────▼──────────┐  ┌───────────────▼───────────────┐
 │ Aura      │ │  Anthropic   │  │     Domain Services           │
 │ Router    │ │  API         │  │  Orbit · Aura Storage         │
 │ (proxy)   │ │  (direct)    │  │  Aura Network                 │
 └───────────┘ └──────────────┘  └───────────────────────────────┘

      Optional:
 ┌─────────────────────┐        ┌──────────────┐
 │  aura-gateway-ts    │        │    zOS API   │
 │  Express on :3000   │        │  (CLI auth)  │
 │  /health  /propose  │        └──────────────┘
 └─────────────────────┘
```

## Configuration

### Node

Environment variables (see [`.env.example`](.env.example)):

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_LLM_ROUTING` | `proxy` | `proxy` (via aura-router with JWT) or `direct` (Anthropic API) |
| `AURA_ROUTER_URL` | `https://aura-router.onrender.com` | Proxy router endpoint |
| `AURA_ROUTER_JWT` | -- | JWT for terminal/CLI sessions (WebSocket clients provide their own) |
| `AURA_ANTHROPIC_API_KEY` | -- | Required when `AURA_LLM_ROUTING=direct` |
| `AURA_ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Anthropic API base URL override |
| `AURA_ANTHROPIC_MODEL` | `claude-opus-4-6` | Model identifier |
| `AURA_MODEL_TIMEOUT_MS` | `60000` | LLM request timeout |
| `AURA_DATA_DIR` | `./aura_data` | Data directory for RocksDB and workspaces |
| `BIND_ADDR` | `127.0.0.1:8080` | HTTP server bind address |

### Gateway (optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3000` | Gateway listen port |
| `ANTHROPIC_API_KEY` | -- | Required API key |
| `CLAUDE_MODEL` | `claude-opus-4-6` | Model to use |
| `MAX_TOKENS` | `4096` | Max response tokens |

## Development

```bash
# Format
cargo fmt --all

# Lint
cargo clippy --all-targets --all-features -- -D warnings

# Test
cargo test --all --all-features

# Check non-RocksDB crates (no LLVM required)
cargo check -p aura-core -p aura-executor -p aura-reasoner
```

## License

MIT
