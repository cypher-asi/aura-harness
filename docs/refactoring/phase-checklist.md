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
cargo check -p aura-runtime  --all-targets
cargo check -p aura-kernel   --all-targets
```

```bash
cargo test -p aura-agent
cargo test -p aura-automaton
cargo test -p aura-runtime
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
aura-runtime
aura-protocol    (external, ../aura-os/crates/aura-protocol)
```

> **Historical (2026):** earlier drafts of this list named an `aura-cli`
> crate. That crate was never created; its intended surface is split
> between the root `aura` binary (interactive TUI) and `aura-runtime`
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

### 3.2 `aura-runtime`

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

Archived execution checklist from the original migration period (Phases 0–10).
All items below were closed out in the 2026-Q1 refactor and are kept here for
historical reference. The newer **System-Audit Refactor (2026-04-24)** has its
own checklist in §5.

### Phase 0 — Baseline verified green

- [x] `cargo check --workspace --all-targets` passes
- [x] `cargo test --workspace --all-features` passes
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [x] Focused checks on `aura-agent`, `aura-automaton`, `aura-runtime`, `aura-kernel` pass
- [x] API snapshots above match current state

### Phase 1 — AgentRunner boundary fix

- [x] `AgentRunner` moved or re-bounded as designed
- [x] G1–G5 pass
- [x] Focused crate checks pass
- [x] No public API removals without snapshot update

### Phase 2 — Tighten `aura-agent` API

- [x] Non-essential `pub mod` items made `pub(crate)` or removed
- [x] G1–G5 pass
- [x] Focused crate checks pass
- [x] Snapshot updated to reflect narrowed API

### Phase 3 — Extract `aura-agent-fileops` wiring

- [x] File-ops integration verified end-to-end
- [x] G1–G5 pass
- [x] Focused crate checks pass

### Phase 4 — Wire `AgentLoop` into runtime

- [x] `AgentLoop` is callable from `aura-runtime` turn processor
- [x] G1–G5 pass
- [x] Focused crate checks pass

### Phase 5 — Consolidate error types

- [x] `AgentError` / `RuntimeError` unified or bridged cleanly
- [x] G1–G5 pass
- [x] Focused crate checks pass

### Phase 6 — Process manager integration

- [x] Async processes tracked end-to-end through agent loop
- [x] G1–G5 pass
- [x] Focused crate checks pass

### Phase 7 — Session lifecycle cleanup

- [x] `aura-runtime` session module simplified
- [x] G1–G5 pass
- [x] Focused crate checks pass

### Phase 8 — Automaton bridge stabilization

- [x] `automaton_bridge` API finalized
- [x] G1–G5 pass
- [x] Focused crate checks pass

### Phase 9 — Tool resolver unification

- [x] Single `ToolResolver` path for built-in + domain tools
- [x] G1–G5 pass
- [x] Focused crate checks pass

### Phase 10 — Final cleanup & documentation

- [x] Dead code removed
- [x] `#![allow(dead_code)]` annotations removed where possible
- [x] G1–G5 pass
- [x] All snapshots updated to reflect final public API
- [x] PROGRESS.md updated

---

## 5. System-Audit Refactor (2026-04-24)

A second, narrower refactor pass covering invariant drift, god-modules, and
type-system tightening. See `C:\Users\n3o\.cursor\plans\system-audit-refactor_c3234749.plan.md`
for the full plan. Status as of Phase 6 close-out:

### Phase 0 — Invariant gating + `aura-node` rename

- [x] HTTP `tool_permissions` permission writes routed under the per-agent
      scheduler lock (Phase 0 fix; pinned by the §2 allowlist in
      `scripts/check_invariants.sh`).
- [x] `aura-node` crate renamed to `aura-runtime`; binary name kept as
      `aura-node` to avoid operator churn.
- [x] `scripts/check_invariants.sh` enabled in CI via
      `.github/workflows/invariants.yml`.

### Phase 1 — Sole external gateway hardening

- [x] `KernelDomainGateway` introduced; every automaton/agent domain
      mutation routes through `Kernel::process_direct` and produces a
      `System/DomainMutation` `RecordEntry`.
- [x] `AutomatonBridge::record_lifecycle_event` `.await`s
      `scheduler.schedule_agent` so lifecycle entries always commit.
- [x] Sync + handshake reasoning failure paths now record a `Reasoning`
      `RecordEntry` (Invariant §3).

### Phase 2a — God-module splits in `aura-core` / `aura-kernel`

- [x] `aura_core::types::tool` split into the `tool/` directory.
- [x] `aura_kernel::policy::check` split into the `check/` directory
      (`delegate_gate`, `agent_permissions`, `integration_gate`,
      `scope`, `verdict`, `tests`).
- [x] `aura_kernel::context` split into `context/{mod,tests}.rs`.
- [x] `aura_kernel::kernel::tools` split into
      `kernel/tools/{mod,single,batch,shared}.rs`.

### Phase 2b — God-module splits in `aura-tools` / `aura-reasoner`

- [x] `aura_tools::resolver::trusted` split into
      `resolver/trusted/{mod,http,transforms,guards,integrations/}`.
- [x] `aura_tools::git_tool` split into per-subcommand modules
      (`executor`, `sandbox`, `commit`, `push`, `commit_push`,
      `redact`, `tests`).
- [x] `aura_reasoner::anthropic::sse` split into
      `anthropic/sse/{mod,parse,event,state,tests}.rs`.

### Phase 2c — God-module splits in `aura-runtime` / `aura-agent`

- [x] `aura_runtime::automaton_bridge` split into the `automaton_bridge/`
      directory (`mod`, `build`, `event_channel`, `dispatch`, `tests`).
- [x] `aura_agent::kernel_domain_gateway` split into the
      `kernel_domain_gateway/` directory (`specs`, `project`, `storage`,
      `orbit`, `network`, `tasks`, `tests`).

### Phase 3 — Shared embedder bootstrap + event mapping

- [x] `aura_agent::session_bootstrap` consolidates store opening, auth
      token loading, default config, and executor-router construction;
      `src/session_helpers.rs` is now a thin re-export layer.
- [x] `aura_agent::events::TurnEventSink` + `map_agent_loop_event`
      shared by the TUI `UiCommandSink` and the node `OutboundMessageSink`.
- [x] `aura_runtime::files_api` shared by both the node `/api/files` /
      `/api/read-file` handlers and the TUI-embedded `src/api_server.rs`.

### Phase 4 — Type-state seal + mid-loop refactor

- [x] `aura_agent::RecordingModelProvider` sealed marker trait introduced;
      automatons take `P: RecordingModelProvider` instead of
      `Arc<dyn ModelProvider>`. Locks Invariant §1 into the type system.
- [x] `aura_kernel`-internal `ToolDecision` renamed to
      `ToolGateVerdict` to disambiguate from the `aura_core`
      audit-log enum.
- [x] `agent_loop::iteration` split into the `iteration/` directory
      (`truncation`, `counters`, `response`, `reasoning`,
      `scheduling`); `IterCounters` and `ThinkingBudget` introduced.
- [x] `agent_loop::tool_processing` renamed to `tool_pipeline`.
- [x] `agent_loop::tool_result_cache::ToolResultCache` extracted.
- [x] `aura_agent::events` split into
      `events/{mod,types,wire,mapper,tests}.rs`.
- [x] `aura_runtime::router::memory` split into
      `router/memory/{mod,handlers,wire,tests}.rs`.
- [x] `dev_loop` split into
      `builtins/dev_loop/{mod,aggregate,forward_event,validation}.rs`.

### Phase 5 — Test-only reachability cleanup

- [x] Test-only constructors gated behind `#[cfg(test)]` consistently.
- [x] Zero unused-import / dead-code warnings under
      `--all-features --all-targets`.

### Phase 6 — Finish & document (this phase)

- [x] **Bullet 1:** `thinking_budget` wired into
      `AgentLoopConfig::thinking_budget` and seeded into
      `LoopState::thinking.budget` (capped at `max_tokens` so
      truncation-recovery still restores to the per-request ceiling).
      Test: `agent_runner::tests::configure_loop_config_seeds_thinking_budget`.
- [x] **Bullet 2:** Router executor-ambiguity in
      `aura_kernel::router::ExecutorRouter::execute` now `error!`-logs,
      panics under `debug_assert!` in debug/test builds, and returns
      `Effect::Failed("ambiguous executor routing")` in release.
      Tests: `ambiguous_routing_panics_in_debug_builds`,
      `single_match_dispatches_normally`.
- [x] **Bullet 3:** `scripts/check_invariants.sh` §2 and §10 allowlists
      refreshed for the Phase 2c module layout (`automaton_bridge/`,
      `router/state.rs`, `kernel_domain_gateway/`) and the Phase 0
      `tool_permissions.rs` direct-append site. `docs/invariants.md`
      §10 prose tracks the new allowlist verbatim.
- [x] **Bullet 4:** `docs/architecture.md`, `docs/invariants.md`,
      `docs/PROGRESS.md`, `docs/refactoring/phase-checklist.md`,
      and `README.md` refreshed for the new module layout, type
      renames, and the `RecordingModelProvider` seal.
- [x] G1–G3 + invariants script all green (workspace `cargo check`
      passes; clippy-clean on the touched bands; `rg` bands re-run
      manually since `bash` is unavailable on the Windows host).
