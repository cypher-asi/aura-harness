# Architectural Invariants

This document defines the invariants that the Aura system must uphold. Every code change should be validated against these rules. Violations are bugs.

---

## 1. Sole External Gateway

**The kernel is the only code that communicates with external systems.**

No code outside the kernel may:
- Call a `ModelProvider` (`complete`, `complete_streaming`)
- Execute an `Action` via `ExecutorRouter`
- Write to the `Store` (`append_entry_atomic`, `enqueue_tx`, `set_agent_status`)
- Make HTTP calls to domain services (`DomainApi` mutating methods)
- Spawn subprocesses for git mutations (`git push`, `git commit`)

All external interactions are mediated through `Kernel::process()` or `Kernel::reason()`.

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
     - `AlwaysAsk` — deny (requires per-use approval)
     - `Deny` — deny
4. Decision is recorded: accepted action IDs or rejected proposals with reasons
5. Only approved proposals become `Action`s and are executed

**Corollary:** A `Deny`-only check is insufficient. The full `Policy::check()` must run.

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

---

## 7. Monotonic Sequencing

**Record entries have strictly increasing, contiguous sequence numbers per agent.**

- `next_seq = head_seq + 1`
- No gaps: if entries exist at seq 1 and seq 3, there must be an entry at seq 2
- No duplicates: `append_entry_atomic` rejects sequence mismatches
- Atomicity: inbox dequeue and record append happen in a single `WriteBatch`

---

## 8. Gateway Transparency

**Kernel gateways implement existing traits. Consumers are unaware of kernel mediation.**

| Gateway | Implements | Consumer |
|---|---|---|
| `KernelModelGateway` | `ModelProvider` | AgentLoop, automatons |
| `KernelToolGateway` | `AgentToolExecutor` | AgentLoop, AgentRunner |
| `KernelDomainGateway` | `DomainApi` | Automatons |

The AgentLoop's public API (`run`, `run_with_events`) accepts `&dyn ModelProvider` and `&dyn AgentToolExecutor`. It must never depend on the concrete gateway types.

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

---

## 10. Append-Only Record

**The record log is immutable. Entries are never modified or deleted.**

- `Store::append_entry_atomic` is the only write path for record entries
- No `update_entry`, `delete_entry`, or `truncate_record` operations exist
- Compaction (in the AgentLoop) operates on in-memory message history, not on the persisted record

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
| Read-only git operations (`git diff`, `git status`, `git log`) | No external side effect. Only mutating git ops (push, commit) require kernel mediation. |
| Read-only `DomainApi` calls (`list_tasks`, `get_project`, `get_spec`) | No external mutation. Only mutating calls require kernel mediation. |

Any addition to this list requires explicit justification and documentation.
