# Architectural Invariants

This document defines the invariants that the Aura system must uphold. Every code change should be validated against these rules. Violations are bugs.

## Enforcement Map

Each invariant below is guarded by one or more tests. The table below is
the living index of which suite enforces which invariant; it is kept in
sync with the `Enforcement:` lines under each section.

| # | Invariant | Enforcement |
|---|---|---|
| §1 | Sole External Gateway | CI-gated `rg` bands in `scripts/check_invariants.sh` + `.github/workflows/invariants.yml` (ModelProvider `.complete(`, `append_entry_*`, `Command::new("git")`, `aura_store` imports inside `aura-agent/agent_loop/`). Git-mutation surface covered by `crates/aura-tools/src/git_tool/tests.rs` (`commit_reports_sha_when_there_are_changes`, `commit_rejects_empty_message`, `commit_surfaces_nonzero_exit_from_add`, `spawn_git_enforces_subcommand_allowlist`, `tool_executes_commit_via_context`, `tool_rejects_workspace_escape_via_config`, `git_push_rejects_missing_fields`). Automaton `DomainApi` mediation covered by `crates/aura-agent/src/kernel_domain_gateway.rs` tests. |
| §2 | Every State Change Is a Transaction | `tests/pipeline_tests.rs`, `tests/kernel_integration.rs`, `crates/aura-kernel/src/kernel/tests.rs`, `crates/aura-node/src/automaton_bridge.rs::tests::start_then_stop_records_two_automaton_lifecycle_entries` (Phase 1 lifecycle path). |
| §3 | Every LLM Call Is Recorded | `crates/aura-agent/src/recording_stream.rs` tests (`streaming_natural_end_records_completed`, `streaming_error_records_failed`, `streaming_drop_records_failed`), `crates/aura-kernel/src/kernel/tests.rs::reason_sync_error_records_failed` + `reason_streaming_handshake_error_records_failed` (Phase 1 sync + handshake failure paths), `tests/automaton_reasoning_recording.rs` (automaton spec-gen / dev-loop calls). |
| §4 | Full Policy Enforcement | `crates/aura-kernel/tests/invariant_policy_matrix.rs` + `crates/aura-kernel/src/policy/tests.rs`. |
| §5 | Complete Audit Trail | `crates/aura-kernel/src/kernel/tests.rs` + §4 matrix asserts `decision`/`actions`/`context_hash`. |
| §6 | Deterministic Context | `crates/aura-kernel/tests/invariant_determinism.rs` (proptest). |
| §7 | Monotonic Sequencing | `crates/aura-store/tests/invariant_atomicity.rs` + `crates/aura-store/src/rocks_store/tests.rs`. |
| §8 | Gateway Transparency | `crates/aura-agent/src/agent_loop/parity_tests.rs`. |
| §9 | AgentLoop Isolation | Architectural / `rg` grep bands (see Untested Invariants) — now CI-gated via `scripts/check_invariants.sh` (aura_store import band scoped to `aura-agent/agent_loop/`). |
| §10 | Append-Only Record | `crates/aura-store/tests/invariant_atomicity.rs` (`static_assertions` sealed-trait check + atomic-commit fault injection) + `crates/aura-store/tests/invariant_readstore_surface.rs` (Phase 2: pins the `ReadStore` trait surface so record-append methods stay on the sealed `WriteStore`). |
| §11 | Session-Scoped Approvals | `crates/aura-kernel/src/policy/tests.rs` (`clear_session_approvals`) + §4 matrix's `AskOnce` rows. |
| §12 | Single Writer Per Agent | `crates/aura-store/src/rocks_store/tests_concurrent.rs`. |

---

## 1. Sole External Gateway

**The kernel is the only code that communicates with external systems.**

No code outside the kernel may:
- Call a `ModelProvider` (`complete`, `complete_streaming`)
- Execute an `Action` via `ExecutorRouter`
- Append to the record log. The record-append family
  (`append_entry_atomic`, `append_entry_dequeued`, `append_entry_direct`,
  their `*_with_runtime_capabilities` variants, and `append_entries_batch`)
  lives on the **sealed** `aura_store::WriteStore` trait — see §10. Non-kernel
  callers bind to `Arc<dyn ReadStore>` and may still invoke the explicitly-
  allowed inbox/metadata writes (`enqueue_tx`, `set_agent_status`) that live
  on `ReadStore`.
- Make HTTP calls to domain services (`DomainApi` mutating methods)
- Spawn subprocesses for git mutations (`git push`, `git commit`)

All external interactions are mediated through `Kernel::process()` or `Kernel::reason()`.

The harness is the runtime authorization and execution boundary, not the credential authority.
Org-level credential persistence and canonical secret retrieval must remain outside the harness.

### Verification

```bash
# These patterns must only appear in kernel, executor, gateway, or store-impl code:
rg "\.complete\(" --type rust        # ModelProvider calls
rg "append_entry_atomic" --type rust # Store writes
rg "enqueue_tx" --type rust          # Store inbox writes
rg "Command::new.*git" --type rust   # Git subprocess spawning
```

---

## 2. Every State Change Is a Transaction

**Every mutation to system state passes through `kernel.process(tx, next_seq)` and produces a `RecordEntry`.**

State changes include:
- User message arrives (`UserPrompt`)
- Agent produces a response (`AgentMsg`)
- LLM suggests a tool call (`ToolProposal`)
- Tool execution completes (`ToolExecution`)
- Session boundary (`SessionStart`)
- Async process completes (`ProcessComplete`)
- Automaton starts/stops (`System`)
- Domain API mutation (`System`)
- Authentication state change (`System`)

No state change may occur without a corresponding entry in the record log.

---

## 3. Every LLM Call Is Recorded

**Every call to a model provider passes through `kernel.reason()` and produces a `RecordEntry` with `TransactionType::Reasoning`.**

The entry records:
- Request snapshot: model name, message count, tool count, system prompt hash, token config
- Response metadata: model used, stop reason, token usage (input + output), tool_use block names

For streaming calls, recording occurs when the stream completes (accumulated by the kernel's stream wrapper).

**Enforcement:** `crates/aura-agent/src/recording_stream.rs` —
`streaming_natural_end_records_completed`, `streaming_error_records_failed`,
`streaming_drop_records_failed` cover natural-end, mid-stream error, and
early-drop termination paths for `TransactionType::Reasoning`.

---

## 4. Full Policy Enforcement

**Every tool call passes through `Policy::check()` with the complete permission model before execution.**

The policy pipeline for a `ToolProposal`:

1. Deserialize `ToolProposal` from transaction payload
2. Build `Proposal` with `ActionKind::Delegate` + serialized `ToolCall`
3. `Policy::check(&proposal)` evaluates:
   - Is `ActionKind::Delegate` in `allowed_action_kinds`?
   - Is the tool in `allowed_tools`?
   - What is the `PermissionLevel`?
     - `AlwaysAllow` — proceed
     - `AskOnce` — check `session_approvals`; deny if not approved
     - `RequireApproval` (renamed from `AlwaysAsk` in Phase 6) — deny unless the caller has registered a single-use approval via `Kernel::grant_approval` for the exact `(agent_id, tool, args_hash)` triple
     - `Deny` — deny
4. Decision is recorded: accepted action IDs or rejected proposals with reasons
5. Only approved proposals become `Action`s and are executed

**Corollary:** A `Deny`-only check is insufficient. The full `Policy::check()` must run.

### 4.a Default permissions for high-privilege tools

The shipped `Policy::with_defaults()` preset (`crates/aura-kernel/src/policy/mod.rs::default_tool_permission`) defaults **`run_command` to `PermissionLevel::RequireApproval`** (Wave 5 / T3, renamed from `AlwaysAsk` in Phase 6 of the security audit). The kernel must never invoke arbitrary binaries without an explicit, single-use per-call approval registered via `Kernel::grant_approval` (or `POST /tool-approval`). Read-only FS inspection (`list_files`, `read_file`, `stat_file`, `search_code`) and in-workspace FS writes (`write_file`, `edit_file`, `delete_file`) remain `AlwaysAllow` because they are sandboxed to the workspace root.

Complementary enforcement in `aura-tools`:

- `run_command` rejects the legacy shell form (`program` set, `args` empty) and the explicit `command` field unless the caller passes `allow_shell: true`.
- When `ToolConfig::binary_allowlist` is non-empty, `run_command` resolves `program` with `which` and denies any binary whose file name (stripped of `.exe` on Windows) is not in the allow-list.

**Enforcement:** `crates/aura-kernel/tests/invariant_policy_matrix.rs`
drives every permission level × tool-listing × action-kind × runtime-
capability combination through `Kernel::process_direct` and asserts the
recorded `Decision` (accept vs. reject-with-reason) for each row.

---

## 5. Complete Audit Trail

**Every `RecordEntry` for a `ToolProposal` contains the full decision chain.**

A tool proposal entry must include:
- `proposals`: the `ProposalSet` containing the proposed action
- `decision`: `Decision` with `accepted_action_ids` or `rejected` (with reasons)
- `actions`: the authorized `Action` (empty if rejected)
- `effects`: the `Effect` from execution (empty if rejected)
- `context_hash`: deterministic hash of inputs

This allows offline replay: given the same record, the same decisions can be verified without a live reasoner or executor.

---

## 6. Deterministic Context

**The context hash for a `RecordEntry` is derived solely from the transaction content and the record window.**

```
context_hash = hash(serialize(tx) || seq[0].context_hash || seq[1].context_hash || ...)
```

Re-processing the same transaction against the same record window must produce the same context hash. This enables integrity verification of the record chain.

**Enforcement:** `crates/aura-kernel/tests/invariant_determinism.rs`
uses `proptest` to assert that `hash_tx_with_window` is deterministic,
order-sensitive (swapping adjacent window entries changes the hash),
insertion-sensitive (adding a no-op entry changes the hash), and
transaction-sensitive (mutating the transaction body changes the hash).

---

## 7. Monotonic Sequencing

**Record entries have strictly increasing, contiguous sequence numbers per agent.**

- `next_seq = head_seq + 1`
- No gaps: if entries exist at seq 1 and seq 3, there must be an entry at seq 2
- No duplicates: `append_entry_atomic` rejects sequence mismatches
- Atomicity: inbox dequeue and record append happen in a single `WriteBatch`

**Enforcement:** `crates/aura-store/tests/invariant_atomicity.rs` (fault
injection across the `WriteBatch` boundary asserts no partial state, and
the sequence-mismatch row asserts strict monotonicity). Additional
coverage in `crates/aura-store/src/rocks_store/tests.rs`.

---

## 8. Gateway Transparency

**Kernel gateways implement existing traits. Consumers are unaware of kernel mediation.**

| Gateway | Implements | Consumer |
|---|---|---|
| `KernelModelGateway` | `ModelProvider` | AgentLoop, automatons |
| `KernelToolGateway` | `AgentToolExecutor` | AgentLoop, AgentRunner |
| `KernelDomainGateway` | `DomainApi` | Automatons |

The AgentLoop's public API (`run`, `run_with_events`) accepts `&dyn ModelProvider` and `&dyn AgentToolExecutor`. It must never depend on the concrete gateway types.

This boundary also means the harness executes tools from runtime metadata without becoming the system of record for integration credentials or catalog state.

---

## 9. AgentLoop Isolation

**The AgentLoop never directly accesses kernel-owned resources.**

The AgentLoop must not:
- Import or reference `Store`, `RocksStore`, or any store type
- Import or reference `RecordEntry` or `RecordEntryBuilder`
- Import or reference `Policy` or `PermissionLevel`
- Call `ModelProvider::complete` on anything other than the provider it receives as a parameter
- Call `AgentToolExecutor::execute` on anything other than the executor it receives as a parameter
- Construct `Transaction` objects

The AgentLoop owns: iteration logic, streaming, compaction, budget management, stall detection, message history.

The harness as a whole may receive runtime-resolved capabilities or short-lived secrets through approved gateways, but it must not persist org credentials or become the catalog authority for integrations.

---

## 10. Append-Only Record

**The record log is immutable. Entries are never modified or deleted.**

- The record-append surface (`append_entry_atomic`, `append_entry_dequeued`,
  `append_entry_direct`, and their `*_with_runtime_capabilities` variants,
  plus `append_entries_batch`) lives on the **sealed** `aura_store::WriteStore`
  trait. Non-kernel crates depend only on `aura_store::ReadStore`; the
  kernel's `Arc<dyn WriteStore>` is the only path that can commit a record
  entry. New storage backends cannot be written outside `aura-store`
  because the sealing marker (`aura_store::store::sealed::Sealed`) is
  crate-private.
- No `update_entry`, `delete_entry`, or `truncate_record` operations exist.
- Compaction (in the AgentLoop) operates on in-memory message history, not
  on the persisted record.

**Enforcement:** `crates/aura-store/tests/invariant_atomicity.rs` —
a `static_assertions::assert_impl_all!(RocksStore: WriteStore, Store)`
check pins the sealed-trait surface at compile time, and the fault-
injection rows prove the append path is all-or-nothing.

---

## 11. Session-Scoped Approvals

**`AskOnce` tool approvals are scoped to the current session.**

- `SessionStart` transaction resets all session approvals via `Policy::clear_session_approvals()`
- Approvals do not persist across sessions
- Approvals do not persist across process restarts

---

## 12. Single Writer Per Agent

**At most one task may process a given agent's transaction queue at any time.**

- Enforced by per-agent `Mutex` in the `Scheduler`
- Different agents are fully independent and process concurrently
- The lock is held for the entire drain of the agent's inbox

---

## Declared Exceptions

The following operations intentionally do NOT route through the kernel:

| Operation | Rationale |
|---|---|
| Message history management (`Vec<Message>` mutations in AgentLoop) | Internal orchestration state. The kernel records inputs (UserPrompt) and outputs (AgentMsg). |
| Read-only file operations (workspace_map, file_walkers, error_context, prompts) | No state mutation, no external side effect. |
| Infrastructure bootstrap (`RocksStore::open`, `create_dir_all` for data dirs) | One-time setup, not a runtime state transition. |
| Server listeners (`TcpListener::bind`) | Inbound edge, not an outbound state change. |
| Interactive PTY (terminal.rs) | User-driven interactive shell; different execution model. |
| Tool sandbox setup (`sandbox.rs` directory creation) | Infrastructure for the kernel-managed tool pipeline. |
| Read-only git operations (`git diff`, `git status`, `git log`) in `aura-agent/src/git.rs` | No external side effect. The `is_git_repo` filesystem probe and `list_unpushed_commits` (`git log` scan) stay in `aura-agent` as read-only helpers. Every mutating `git` subprocess (`add`, `commit`, `push`) lives behind the `GitExecutor` in `crates/aura-tools/src/git_tool/` and routes through the kernel's `ToolExecutor`. |
| `git init` bootstrap in `crates/aura-automaton/src/builtins/dev_loop/tick.rs` | One-time creation of a local `.git/` directory when a fresh workspace is first driven by the dev-loop automaton. Has no remote, cannot leak state across agents, and is strictly analogous to `RocksStore::open`. The call-site is pinned by the `Command::new("git")` band in `scripts/check_invariants.sh`; any second `git init` anywhere else is a CI failure. |
| Read-only `DomainApi` calls (`list_tasks`, `get_project`, `get_spec`) | No external mutation. Only mutating calls require kernel mediation. |
| Generation proxy (`session/generation.rs`) for image/3D requests | Pure SSE proxy to `aura-router`; the session does not mutate local state or consume LLM credits. All remote calls use bounded `reqwest` connect/read timeouts. When this surface starts spending credits or persisting artifacts locally it **must** move behind a `KernelGenerationGateway`. **Regression guard:** `crates/aura-node/tests/generation_proxy_guard.rs` reads `session/generation.rs` from disk and fails if `RecordEntry`, `Kernel`, `ModelProvider`, or any `append_entry_*` helper appears in the source — any of those would mean the module has crossed the line and the exception must either be removed or the module re-routed through the kernel gateway. |

Any addition to this list requires explicit justification and documentation.
