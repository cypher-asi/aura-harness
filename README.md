<p align="center">
  <strong style="font-size: 2em;">AURA</strong>
</p>

---

<p align="center">
  <strong>Deterministic Multi-Agent Runtime</strong><br/>
  An append-only, pluggable-reasoning runtime for running many agents concurrently with sandboxed tool execution.
</p>

<p align="center">
  <a href="#overview">Overview</a> &nbsp;·&nbsp;
  <a href="#quick-start">Quick Start</a> &nbsp;·&nbsp;
  <a href="#binaries">Binaries</a> &nbsp;·&nbsp;
  <a href="#cli-reference">CLI</a> &nbsp;·&nbsp;
  <a href="#architecture">Architecture</a> &nbsp;·&nbsp;
  <a href="#http--websocket-api">API</a> &nbsp;·&nbsp;
  <a href="#configuration">Configuration</a> &nbsp;·&nbsp;
  <a href="#development">Development</a>
</p>

## Overview

Aura is a deterministic multi-agent runtime for running many agents concurrently. Every agent maintains an append-only record log, a deterministic kernel advances state by consuming transactions, and reasoning is delegated to a pluggable LLM provider (proxy-routed or direct Anthropic API). All side effects flow through authorized executors so the full history is replayable from the record alone.

The runtime supports interactive terminal sessions (TUI), headless server deployments, and long-running automaton workflows — all backed by the same kernel, storage, and reasoning stack.

> This repository (`aura-harness`) is the Cargo workspace that builds the Aura runtime (`aura`, `aura-node`). It is distinct from the sibling `aura-swarm` repository, which is a Firecracker/Kubernetes platform for hosting Aura agents.

Core ideas:

1. **The Record.** The fundamental unit of truth. Every agent has an append-only log of record entries, strictly ordered by sequence number. All state is derivable from the record; there is no hidden state.
2. **The Kernel.** A deterministic processor that builds context from the record, calls the reasoner, enforces policy, executes actions through the executor, and commits new entries. Given the same record, the kernel always produces the same output.
3. **Reasoning.** Probabilistic LLM calls are isolated behind a provider trait. The default path routes through a JWT-authenticated proxy (`aura-router`); alternatively, calls go directly to the Anthropic API. A mock provider is available for testing.
4. **Tools & Executors.** All side effects (filesystem, shell commands, domain APIs, automaton actions) are explicit. The executor router dispatches authorized actions and captures structured effects, keeping the kernel boundary clean.
5. **Memory & Skills.** Per-agent memory (facts, events, procedures) and `SKILL.md`-based skill packages extend an agent's abilities at runtime without widening the deterministic kernel.

## Principles

1. **Per-Agent Order** — Record entries are strictly ordered by sequence number; no reordering, no gaps.
2. **Atomic Commit** — Transaction processing is all-or-nothing via RocksDB batch writes.
3. **No Hidden State** — All state is replayable from the record. If it is not in the log, it did not happen.
4. **Deterministic Kernel** — The kernel advances only by consuming transactions. Same input, same output.
5. **Explicit Side Effects** — Every tool call flows through an authorized executor; effects are captured and recorded.
6. **Open Source** — MIT-licensed Rust workspace. Every layer is auditable and reusable.

## Prerequisites

`aura-harness` is a Cargo workspace that depends on a sibling repository, `aura-os`, for shared protocol types (`aura-protocol`). Both repositories must live next to each other:

```
<parent>/
  aura-harness/   # this repo
  aura-os/        # sibling repo (for aura-protocol and related crates)
```

The path dependency is declared in [`Cargo.toml`](Cargo.toml) and [`crates/aura-node/Cargo.toml`](crates/aura-node/Cargo.toml) as `../aura-os/crates/aura-protocol`. RocksDB builds require LLVM/Clang; see [`docs/PROGRESS.md`](docs/PROGRESS.md) for platform notes.

## Quick Start

```sh
# Build the full workspace (release)
cargo build --release

# Run the TUI (proxy mode — no API key needed)
cargo run

# Run the same binary headless
cargo run -- run --ui none

# Run the standalone node server
cargo run -p aura-node
```

Direct Anthropic access:

```sh
AURA_LLM_ROUTING=direct AURA_ANTHROPIC_API_KEY=sk-ant-... cargo run
```

### Docker

The Dockerfile builds from the **parent directory** that contains both `aura-harness/` and `aura-os/`, so the `aura-protocol` path dependency resolves in the image. Run from the parent:

```sh
# in <parent>/ (contains aura-harness/ and aura-os/)
docker build -f aura-harness/Dockerfile -t aura .
docker run -p 8080:8080 aura
```

The image runs `aura run --ui none` as a non-root user, exposes `:8080`, and defaults `AURA_DATA_DIR=/data`. See [`Dockerfile`](Dockerfile) for the full recipe.

### Optional: TypeScript Gateway

[`aura-gateway-ts/`](aura-gateway-ts/) is an optional Express service that exposes a `/propose` endpoint for local LLM routing:

```sh
cd aura-gateway-ts && npm install && npm run build
ANTHROPIC_API_KEY=your-key npm start   # listens on :3000
```

## Binaries

This workspace ships two binaries:

| Binary | Entry point | Purpose |
|--------|-------------|---------|
| `aura` | [`src/main.rs`](src/main.rs) | Primary binary — TUI by default, headless node with `run --ui none`. |
| `aura-node` | [`crates/aura-node/src/main.rs`](crates/aura-node/src/main.rs) | Standalone headless server (HTTP + WebSocket API). |

> The earlier `aura-cli` REPL crate was retired in Wave 4 of the refactor.
> Its surface is covered by `aura` (TUI + `run --ui none`). See
> `docs/architecture.md` for history.

## CLI Reference

Defined in [`src/cli.rs`](src/cli.rs):

| Command | Description |
|---------|-------------|
| `aura run` (default) | Run the agent. Flags below. |
| `aura login` | Authenticate with zOS and store a JWT for proxy mode. |
| `aura logout` | Clear stored credentials. |
| `aura whoami` | Show current authentication status. |
| `aura hello` | Print `Hello, World!` and exit (Spec 01 smoke test). |

Flags for `aura run`:

| Flag | Default | Description |
|------|---------|-------------|
| `--ui <terminal\|none>` | `terminal` | Terminal UI (ratatui) or headless node. |
| `--theme <name>` | `cyber` | One of `cyber`, `matrix`, `synthwave`, `minimal`. |
| `-d, --dir <path>` | -- | Override working / data directory. |
| `--provider <anthropic\|mock>` | `anthropic` | Model provider for the current session. |
| `-v, --verbose` | off | Enable verbose tracing output. |

## Architecture

### Workspace crates

All members are declared in [`Cargo.toml`](Cargo.toml) under `[workspace].members`.

| Crate | Description |
|-------|-------------|
| [`aura-core`](crates/aura-core) | Shared domain types, strongly-typed IDs, hashing, time, serialization, and error types. |
| [`aura-store`](crates/aura-store) | RocksDB persistence: record log, agent metadata, inbox queues, memory CFs, skill installs. Atomic batch commits. |
| [`aura-reasoner`](crates/aura-reasoner) | Provider-agnostic `ModelProvider` trait: Anthropic HTTP, proxy routing, mock, streaming, retries. |
| [`aura-kernel`](crates/aura-kernel) | Deterministic kernel: context building, reasoning, policy, execution routing, record commit. |
| [`aura-tools`](crates/aura-tools) | Tool registry, sandboxed filesystem and command execution, domain tool wiring, automaton tools. |
| [`aura-agent`](crates/aura-agent) | Multi-step orchestration (`AgentLoop`), tool gateways, task runner, budgets, compaction, and session bootstrap. |
| [`aura-memory`](crates/aura-memory) | Per-agent memory: facts, events, procedures. Two-stage write pipeline (heuristic + LLM refiner), retrieval, consolidation. |
| [`aura-skills`](crates/aura-skills) | `SKILL.md`-compatible skill packages. Parser, multi-location loader, registry, activation, and per-agent install store. |
| [`aura-auth`](crates/aura-auth) | zOS login client and JWT credential store (`~/.aura/credentials.json`) for proxy mode. |
| [`aura-terminal`](crates/aura-terminal) | Ratatui-based terminal UI library: themes, components, input handling, layout. |
| [`aura-automaton`](crates/aura-automaton) | Automaton lifecycle, scheduling, runtime, state, and built-in automatons (chat, dev loop, spec gen, task run). |
| [`aura-node`](crates/aura-node) | HTTP router, WebSocket sessions, scheduler, and per-agent worker loops with single-writer guarantee. |

### External dependencies

- [`aura-protocol`](../aura-os/crates/aura-protocol) lives in the sibling `aura-os` workspace and defines the serde types for the `/stream` WebSocket API (session init, messages, events, approvals). The harness consumes it as a relative path dependency.
- [`aura-gateway-ts/`](aura-gateway-ts/) is an optional TypeScript gateway for local `/propose` LLM routing.

### Project structure

```
aura-harness/
  Cargo.toml                # workspace root + `aura` binary
  Dockerfile                # multi-stage build, headless on :8080
  .env.example              # environment variable template
  index.html                # landing page
  src/                      # `aura` binary
    main.rs                 #   entry: TUI, headless, login/logout/whoami/hello
    cli.rs                  #   clap command definitions
    event_loop/             #   terminal event loop
    api_server.rs           #   embedded /health endpoint for TUI mode
    session_helpers.rs      #   session bootstrap re-exports + defaults
    record_loader.rs        #   record loading utilities
  crates/
    aura-core/              # shared types, IDs, hashing, time
    aura-store/             # RocksDB storage backend
    aura-reasoner/          # LLM provider abstraction + Anthropic
    aura-kernel/            # deterministic kernel + policy + executor
    aura-tools/             # tool registry, sandboxed FS/cmd, domain tools
    aura-agent/             # agent loop + runtime + compaction + session bootstrap
    aura-memory/            # facts/events/procedures + write pipeline + retrieval
    aura-skills/            # SKILL.md parser, loader, registry, install store
    aura-auth/              # zOS login, credential store
    aura-terminal/          # ratatui TUI library
    aura-automaton/         # automaton lifecycle and built-ins
    aura-node/              # HTTP server, scheduler, workers
  aura-gateway-ts/          # optional TypeScript gateway (Express + /propose)
  tests/                    # integration, e2e, proptest, pipeline
  docs/                     # supplementary documentation
    architecture.md         #   full architecture reference
    PROGRESS.md             #   implementation status / build notes
    specs/                  #   design specifications (v0.1.0, v0.1.1)
    refactoring/            #   refactoring checklists
```

### System diagram

```
                             ┌──────────────────────────────────┐
                             │           Entry Points           │
                             │  aura (TUI)  ·  aura --ui none  │
                             │  aura-node                       │
                             └──────────────┬───────────────────┘
                                            │
                             ┌──────────────▼───────────────────┐
                             │         HTTP / WebSocket         │
                             │      Router (Axum on :8080)      │
                             │  (routes listed below)           │
                             └──────────────┬───────────────────┘
                                            │
                    ┌───────────────────────▼──────────────────────────┐
                    │                  Scheduler                       │
                    │   per-agent tokio::Mutex  ·  DashMap registry   │
                    └───┬──────────────┬──────────────┬───────────────┘
                        │              │              │
                   ┌────▼────┐   ┌─────▼────┐   ┌────▼────┐
                   │ Worker  │   │  Worker  │   │ Worker  │  (one per agent)
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
                    │              └──────────────┘  │  memory_*      │
                    │                                │  agent_skills  │
                    │                                └────────────────┘
      ┌─────────────┼──────────────────────────────┐
      │             │                              │
 ┌────▼──────┐ ┌────▼──────────┐  ┌───────────────▼───────────────┐
 │ Aura      │ │  Anthropic   │  │     Domain Services           │
 │ Router    │ │  API         │  │  Orbit · Aura Storage         │
 │ (proxy)   │ │  (direct)    │  │  Aura Network                 │
 └───────────┘ └──────────────┘  └───────────────────────────────┘
```

## HTTP / WebSocket API

All routes are defined in `crates/aura-node/src/router/mod.rs` (`create_router`). Names use Axum path-parameter syntax.

### Health & workspace

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Liveness probe. |
| GET | `/api/files` | List files in the configured workspace root. |
| GET | `/api/read-file` | Read a file from the workspace root. |
| GET | `/workspace/resolve` | Resolve a project/workspace slug to a filesystem path. |

### Transactions & records

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/tx` | Submit a transaction for an agent. |
| GET  | `/tx/status/:agent_id/:tx_id` | Status of a submitted transaction. |
| GET  | `/agents/:agent_id/head` | Latest record sequence for an agent. |
| GET  | `/agents/:agent_id/record` | Paginated record scan. |

### Automaton

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/automaton/start` | Start an automaton. |
| GET  | `/automaton/list` | List automatons. |
| GET  | `/automaton/:automaton_id/status` | Status for one automaton. |
| POST | `/automaton/:automaton_id/pause` | Pause an automaton. |
| POST | `/automaton/:automaton_id/stop` | Stop an automaton. |

### Memory

Canonical routes are mounted under `/memory/...`; compatibility aliases are mounted under `/api/agents/:agent_id/memory/...`. Both surfaces cover:

- Facts: list / create / update / delete, fetch by key.
- Events: list / create / delete, bulk-delete.
- Procedures: list / create / update / delete.
- `GET /memory/:agent_id/snapshot` — full memory snapshot.
- `POST /memory/:agent_id/wipe` — clear all memory for an agent.
- `GET /memory/:agent_id/stats` — counts, token budgets.
- `POST /memory/:agent_id/consolidate` — trigger consolidation.

### Skills

| Method | Path | Purpose |
|--------|------|---------|
| GET, POST | `/api/skills` | List available skills / install a skill. |
| GET | `/api/skills/:name` | Skill details. |
| POST | `/api/skills/:name/activate` | Activate a skill. |
| GET, POST | `/api/agents/:agent_id/skills` | Per-agent install list / install. |
| DELETE | `/api/agents/:agent_id/skills/:name` | Uninstall a skill from an agent. |

Legacy harness aliases for skill list/install/uninstall are mounted under `/api/harness/agents/:agent_id/skills...` for backward compatibility.

### WebSocket

| Path | Purpose |
|------|---------|
| `/ws/terminal` | Terminal bridge used by the TUI. |
| `/stream` | Primary session stream (session init, messages, events, approvals). |
| `/stream/automaton/:automaton_id` | Stream events for a specific automaton. |

## Memory

`aura-memory` adds per-agent long-term memory backed by RocksDB column families:

- **Facts** — durable key/value claims (`MEMORY_FACTS`).
- **Events** — episodic events with time index (`MEMORY_EVENTS`, `MEMORY_EVENT_INDEX`).
- **Procedures** — repeated step sequences detected over time (`MEMORY_PROCEDURES`).

Writes flow through a two-stage pipeline (heuristic extractor + optional LLM refiner, see [`crates/aura-memory/src/write_pipeline.rs`](crates/aura-memory/src/write_pipeline.rs) and [`crates/aura-memory/src/refinement.rs`](crates/aura-memory/src/refinement.rs)). `MemoryRetriever` injects a size-budgeted slice of memory into the kernel context on each turn.

## Skills

`aura-skills` loads `SKILL.md` packages from (in precedence order):

1. Workspace — `{workspace}/skills/`
2. Agent-personal — `~/.aura/agents/{id}/skills/`
3. Personal — `~/.aura/skills/`
4. Extra directories from config
5. Bundled skills shipped with the runtime

`SkillManager` exposes activation and prompt injection; `SkillInstallStore` persists per-agent installs in the `AGENT_SKILLS` column family. See [`crates/aura-skills/src/lib.rs`](crates/aura-skills/src/lib.rs).

## Configuration

The node reads configuration from environment variables via `NodeConfig::from_env()` in [`crates/aura-node/src/config/mod.rs`](crates/aura-node/src/config/mod.rs). Copy [`.env.example`](.env.example) as a starting point.

### LLM routing

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_LLM_ROUTING` | `proxy` | `proxy` (via aura-router with JWT) or `direct` (Anthropic API). |
| `AURA_ROUTER_URL` | `https://aura-router.onrender.com` | Proxy router endpoint. |
| `AURA_ROUTER_JWT` | — | JWT for terminal/CLI sessions. WebSocket clients supply their own. |
| `AURA_ANTHROPIC_API_KEY` | — | Required when `AURA_LLM_ROUTING=direct`. |
| `AURA_ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Anthropic API base URL override. |
| `AURA_ANTHROPIC_MODEL` | `claude-opus-4-6` | Model identifier. |
| `AURA_MODEL_TIMEOUT_MS` | `60000` | LLM request timeout. |

### Node runtime

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_DATA_DIR` (alias `DATA_DIR`) | OS local app data `aura/node`; `./aura_data` fallback | Data directory for RocksDB and workspaces. Set explicitly to share state or keep repo-local data. |
| `AURA_LISTEN_ADDR` (alias `BIND_ADDR`) | `127.0.0.1:8080` | HTTP server bind address. |
| `SYNC_WRITES` | `false` | Enable sync writes (`true`/`1` to enable) to RocksDB. |
| `RECORD_WINDOW_SIZE` | `50` | Kernel context record window. |
| `ENABLE_FS_TOOLS` | `true` | Disable with `false`/`0`. |
| `ENABLE_CMD_TOOLS` | `false` | Enable shell command tools with `true`/`1`. |
| `ALLOWED_COMMANDS` | — | Comma-separated command allowlist when `ENABLE_CMD_TOOLS` is on. |
| `AURA_PROJECT_BASE` | — | Remaps incoming project paths to `{base}/{slug}` (remote VM mode). |
| `ORBIT_URL` | `https://orbit-sfvu.onrender.com` | Orbit service URL. |
| `AURA_STORAGE_URL` | `https://aura-storage.onrender.com` | Aura Storage service URL. |
| `AURA_NETWORK_URL` | `https://aura-network.onrender.com` | Aura Network service URL. |

### Gateway (optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3000` | Gateway listen port. |
| `ANTHROPIC_API_KEY` | — | Required API key. |
| `CLAUDE_MODEL` | `claude-opus-4-6` | Model to use. |
| `MAX_TOKENS` | `4096` | Max response tokens. |

## Development

```bash
# Format
cargo fmt --all

# Lint
cargo clippy --all-targets --all-features -- -D warnings

# Test everything
cargo test --all --all-features

# Fast smoke test: node config
cargo test -p aura-node config::

# Check non-RocksDB crates (no LLVM required)
cargo check -p aura-core -p aura-kernel -p aura-reasoner
```

Further reading:

- [`docs/architecture.md`](docs/architecture.md) — full architecture reference.
- [`docs/PROGRESS.md`](docs/PROGRESS.md) — implementation status and platform notes.
- [`docs/specs/`](docs/specs) — design specifications.

## License

MIT
