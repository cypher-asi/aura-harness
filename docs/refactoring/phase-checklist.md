# Refactoring Phase Checklist

> Generated: 2026-03-30
>
> This document preserves the refactor gate checklist and pre-refactoring API
> snapshots used during migration. The snapshots in section 3 are historical.
> For current crate shape, see section 2.1 below.

---

## 1. Global Acceptance Criteria

Every phase **must** pass all of the following before it is considered complete:

| # | Gate | Command / Check |
|---|------|-----------------|
| G1 | Workspace compiles | `cargo check --workspace --all-targets` |
| G2 | All tests green | `cargo test --workspace --all-features` |
| G3 | No clippy warnings | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| G4 | No new cyclic deps | Verify with `cargo metadata` — no new inter-crate cycles introduced |
| G5 | Spec-01 invariants | **I1 – I4** preserved (see below) |

### Spec-01 Invariants (must hold at all times)

| ID | Invariant |
|----|-----------|
| **I1** | **Per-Agent order** — For a given `agent_id`, Record entries are strictly ordered by `seq`. |
| **I2** | **Atomic commit** — A processed Transaction commits all-or-nothing: RecordEntry + head_seq + inbox dequeue. |
| **I3** | **No hidden state** — Derived state is either replayable from Record or stored as a derived artifact that is also recorded. |
| **I4** | **Deterministic kernel input** — The Kernel advances only by consuming Transactions and committing RecordEntries. |


## 1.1 Simplify Tools And Permissions Progress

Updated: 2026-04-24

| Phase | Status | Notes |
|---|---|---|
| A | Complete | Tri-state core types and resolver were already in place. |
| B | Complete | User default persistence and parallel policy wiring were already in place. |
| C | Complete | Live WebSocket tool approval prompt flow is implemented and committed. |
| D | Complete | Tool defaults/agent permissions HTTP APIs, spawn/session wiring, and effective catalog responses are implemented and committed. |
| E | Complete | Legacy approval registry, `PermissionLevel`, and old per-tool policy fields were removed and committed. |
| F | Complete | `ToolConfig` was slimmed to execution guardrails and env permission switches were removed; committed as `dbf5ed0`. |
| G | Complete | `DefaultToolRegistry`/`ToolRegistry` were removed; executor-backed bootstrap callers now use `ToolCatalog`; focused compile checks passed. |
| H | Complete | `docs/invariants.md` and stale policy docs/comments were updated; focused tri-state validation tests, compile, Clippy, and targeted tests passed. |

---

## 2. Focused Compile Checks

In addition to the full-workspace gates, run targeted checks on the most
sensitive crates after each phase:

```bash
cargo check -p aura-agent   --all-targets
cargo check -p aura-automaton --all-targets
cargo check -p aura-node     --all-targets
cargo check -p aura-kernel   --all-targets
```

```bash
cargo test -p aura-agent
cargo test -p aura-automaton
cargo test -p aura-node
cargo test -p aura-kernel
```

---

## 2.1 Current Workspace Shape (Post-Refactor)

The workspace currently contains the following crates (root `aura`
binary plus `aura-*` library crates; `aura-protocol` lives in the
sibling `aura-os` workspace and is consumed as a path dependency):

```text
aura              (root binary — canonical CLI entry; see README.md)
aura-core
aura-store
aura-tools
aura-reasoner
aura-kernel
aura-terminal
aura-agent
aura-memory
aura-skills
aura-auth
aura-automaton
aura-node
aura-protocol    (external, ../aura-os/crates/aura-protocol)
```

> **Historical (2026):** earlier drafts of this list named an `aura-cli`
> crate. That crate was never created; its intended surface is split
> between the root `aura` binary (interactive TUI) and `aura-node`
> (headless server). See `README.md` → "Binaries" for the canonical
> entry point.

Refactor outcomes reflected in codebase:

- `aura-executor` was dissolved into `aura-core` + `aura-kernel`.
- `aura-runtime`, `aura-agent-fileops`, and `aura-agent-verify` were merged into `aura-agent`.
- `aura-session` was dissolved into binary-local session helper modules.

---

## 3. Pre-Refactoring Public API Snapshots

Captured from each crate's `src/lib.rs` before any refactoring begins.
Any phase that removes or renames a public item must update this section
and justify the change in its PR description.

### 3.1 `aura-agent`

**Public modules:**

```text
pub mod blocking
pub mod build
pub mod compaction
pub mod events
pub mod git
pub mod parser
pub mod planning
pub mod policy
pub mod prompts
pub mod self_review
pub mod shell_parse
pub mod types
pub mod agent_runner
pub mod message_conversion
pub mod task_context
pub mod task_executor
```

**Re-exports:**

```text
pub use aura_agent_fileops as file_ops;
pub use aura_agent_verify as verify;
pub use agent_loop::{AgentLoop, AgentLoopConfig};
pub use aura_runtime::ModelCallDelegate;
pub use events::AgentLoopEvent;
pub use kernel_executor::KernelToolExecutor;
pub use types::{AgentLoopResult, AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo, ToolCallResult};
```

**Public types / traits / functions:**

```text
pub enum AgentError {
    Model(String),
    ToolExecution(String),
    Timeout(String),
    BuildFailed(String),
    Internal(String),
}
```

### 3.2 `aura-node`

**Public modules:**

```text
pub mod automaton_bridge
pub mod domain
pub mod jwt_domain
pub mod protocol
pub mod router
pub mod scheduler
pub mod session
pub mod terminal
```

**Re-exports:**

```text
pub use config::NodeConfig;
pub use node::Node;
```

**Public types / traits / functions:**

```text
pub enum NodeError {
    Server(#[from] std::io::Error),
    Store(#[from] anyhow::Error),
    InvalidAddress(#[from] std::net::AddrParseError),
}
```

### 3.3 `aura-tools`

**Public modules:**

```text
pub mod automaton_tools
pub mod catalog
pub mod definitions
pub mod domain_tools
pub mod resolver
```

**Crate-internal modules (pub(crate)):**

```text
pub(crate) mod fs_tools
pub(crate) mod tool
```

**Re-exports:**

```text
pub use catalog::ToolCatalog;
pub use error::ToolError;
pub use executor::ToolExecutor;
pub use fs_tools::{cmd_run_with_threshold, cmd_spawn, output_to_tool_result, ThresholdResult};
pub use resolver::ToolResolver;
pub use sandbox::Sandbox;
pub use tool::{Tool, ToolContext};
```

**Public types / traits / functions:**

```text
pub struct ToolConfig {
    pub command: CommandPolicy,
    pub max_read_bytes: usize,
    pub sync_threshold_ms: u64,
    pub max_async_timeout_ms: u64,
}

impl Default for ToolConfig { .. }
```

### 3.4 `aura-runtime`

**Public modules:**

```text
pub mod process_manager
```

**Re-exports:**

```text
pub use process_manager::{ProcessManager, ProcessManagerConfig, ProcessOutput, RunningProcess};
pub use turn_processor::{
    ExecutedToolCall, ModelCallDelegate, StepConfig, StepResult, StreamCallback,
    StreamCallbackEvent, ToolCache, TurnConfig, TurnEntry, TurnProcessor, TurnResult,
};
```

**Public types / traits / functions:**

```text
pub enum RuntimeError {
    Model(String),
    ToolExecution(String),
    Timeout(String),
    Store(String),
    Internal(String),
}
```

---

## 4. Historical Phase-by-Phase Checklist

Archived execution checklist from migration period.

### Phase 0 — Baseline verified green

- [ ] `cargo check --workspace --all-targets` passes
- [ ] `cargo test --workspace --all-features` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [ ] Focused checks on `aura-agent`, `aura-automaton`, `aura-node`, `aura-kernel` pass
- [ ] API snapshots above match current state

### Phase 1 — AgentRunner boundary fix

- [ ] `AgentRunner` moved or re-bounded as designed
- [ ] G1–G5 pass
- [ ] Focused crate checks pass
- [ ] No public API removals without snapshot update

### Phase 2 — Tighten `aura-agent` API

- [ ] Non-essential `pub mod` items made `pub(crate)` or removed
- [ ] G1–G5 pass
- [ ] Focused crate checks pass
- [ ] Snapshot updated to reflect narrowed API

### Phase 3 — Extract `aura-agent-fileops` wiring

- [ ] File-ops integration verified end-to-end
- [ ] G1–G5 pass
- [ ] Focused crate checks pass

### Phase 4 — Wire `AgentLoop` into runtime

- [ ] `AgentLoop` is callable from `aura-runtime` turn processor
- [ ] G1–G5 pass
- [ ] Focused crate checks pass

### Phase 5 — Consolidate error types

- [ ] `AgentError` / `RuntimeError` unified or bridged cleanly
- [ ] G1–G5 pass
- [ ] Focused crate checks pass

### Phase 6 — Process manager integration

- [ ] Async processes tracked end-to-end through agent loop
- [ ] G1–G5 pass
- [ ] Focused crate checks pass

### Phase 7 — Session lifecycle cleanup

- [ ] `aura-node` session module simplified
- [ ] G1–G5 pass
- [ ] Focused crate checks pass

### Phase 8 — Automaton bridge stabilization

- [ ] `automaton_bridge` API finalized
- [ ] G1–G5 pass
- [ ] Focused crate checks pass

### Phase 9 — Tool resolver unification

- [ ] Single `ToolResolver` path for built-in + domain tools
- [ ] G1–G5 pass
- [ ] Focused crate checks pass

### Phase 10 — Final cleanup & documentation

- [ ] Dead code removed
- [ ] `#![allow(dead_code)]` annotations removed where possible
- [ ] G1–G5 pass
- [ ] All snapshots updated to reflect final public API
- [ ] PROGRESS.md updated
