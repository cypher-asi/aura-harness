# Aura Swarm

A deterministic multi-agent runtime with append-only record, pluggable reasoning, and sandboxed tool execution.

## Overview

Aura Swarm is a system for running many deterministic agents concurrently where:

- **All reality is the Record** - Append-only log per agent
- **Kernel is deterministic** - Processes transactions sequentially per agent
- **Reasoning is probabilistic** - Pluggable (default: Claude via TypeScript gateway)
- **Side effects are explicit** - All tool execution happens through authorized executors

## Core Guarantees

1. **Per-Agent Order** - Record entries are strictly ordered by sequence number
2. **Atomic Commit** - Transaction processing is all-or-nothing
3. **No Hidden State** - All state is replayable from the Record
4. **Deterministic Kernel** - Advances only by consuming transactions

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Aura Swarm                               │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐   ┌─────────────┐   ┌─────────────────────┐   │
│  │   Router    │──>│  Scheduler  │──>│   Worker (per agent)│   │
│  │  (HTTP API) │   │             │   │   - Lock            │   │
│  └─────────────┘   └─────────────┘   │   - Dequeue         │   │
│                                       │   - Process         │   │
│                                       │   - Commit          │   │
│                                       └─────────────────────┘   │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                     Kernel (Deterministic)               │   │
│  │  - Build context from record window                      │   │
│  │  - Call Reasoner (probabilistic)                         │   │
│  │  - Apply Policy (deterministic)                          │   │
│  │  - Execute Actions via Executor Router                   │   │
│  │  - Build RecordEntry                                     │   │
│  └─────────────────────────────────────────────────────────┘   │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │   Reasoner   │  │   Executor   │  │      Store           │  │
│  │   (Claude)   │  │   (Tools)    │  │    (RocksDB)         │  │
│  └──────────────┘  └──────────────┘  └──────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Crates

| Crate | Description |
|-------|-------------|
| `aura-core` | Core types, IDs, schemas, serialization |
| `aura-store` | RocksDB storage with atomic commits |
| `aura-executor` | Executor trait and router |
| `aura-tools` | Sandboxed filesystem and command tools |
| `aura-reasoner` | HTTP client to TypeScript gateway |
| `aura-kernel` | Deterministic kernel with policy engine |
| `aura-node` | HTTP router, scheduler, worker runtime |
| `aura-gateway-ts` | TypeScript gateway for Claude integration |

## Quick Start

### Prerequisites

- Rust 1.75+ (via rustup)
- Node.js 20+ (for gateway)
- LLVM/Clang (for RocksDB on Windows)

### Build

```bash
# Rust crates
cargo build --release

# TypeScript gateway
cd aura-gateway-ts
npm install
npm run build
```

### Run

1. Start the TypeScript gateway:
   ```bash
   cd aura-gateway-ts
   ANTHROPIC_API_KEY=your-key npm start
   ```

2. Start the Rust swarm:
   ```bash
   cargo run -p aura-node
   ```

### API

Submit a transaction:
```bash
curl -X POST http://localhost:8080/tx \
  -H "Content-Type: application/json" \
  -d '{
    "agent_id": "0000000000000000000000000000000000000000000000000000000000000001",
    "kind": "user_prompt",
    "payload": "SGVsbG8sIEFnZW50IQ=="
  }'
```

Check agent head:
```bash
curl http://localhost:8080/agents/0000000000000000000000000000000000000000000000000000000000000001/head
```

Scan record:
```bash
curl http://localhost:8080/agents/0000000000000000000000000000000000000000000000000000000000000001/record?from_seq=1&limit=10
```

## Configuration

### Swarm Config

Environment variables or config file:

| Variable | Default | Description |
|----------|---------|-------------|
| `DATA_DIR` | `./aura_data` | Data directory for DB and workspaces |
| `BIND_ADDR` | `127.0.0.1:8080` | HTTP server address |
| `REASONER_URL` | `http://localhost:3000` | Gateway URL |
| `SYNC_WRITES` | `false` | Sync writes for durability |

### Gateway Config

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3000` | Gateway port |
| `ANTHROPIC_API_KEY` | - | Required API key |
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
