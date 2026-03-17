# Aura Swarm MVP Spec (Rust + RocksDB + Tools + Claude Code Reasoner)

## 1) Purpose

Build a Swarm that can run **many deterministic Agents** concurrently, where:

* **All reality is the Record** (append-only log per Agent).
* The **Kernel is deterministic**: it processes Transactions sequentially per Agent.
* **Reasoning is probabilistic** and pluggable (default: Claude Code via TS).
* **All side effects** (filesystem, shell commands, messaging) happen through **Tools/Executors**, and their results re-enter as Transactions.

---

## 2) Core guarantees (must hold)

### Invariants

* **I1: Per-Agent order** — For a given `agent_id`, Record entries are strictly ordered by `seq`.
* **I2: Atomic commit** — A processed Transaction commits **all-or-nothing**: RecordEntry + head_seq + inbox dequeue (+ optional indexes later).
* **I3: No hidden state** — Derived state is either replayable from Record or stored as a derived artifact that is also recorded.
* **I4: Deterministic kernel input** — The Kernel advances only by consuming Transactions and committing RecordEntries.

### Concurrency rule

* **Sequential within an Agent; parallel across Agents.**
* Never process two Transactions for the same Agent concurrently.

---

## 3) High-level architecture

### Runtime components

**Swarm (Rust)**

* Receives Transactions (HTTP/gRPC)
* Persists them to an inbox
* Schedules Agents in parallel
* Runs the Kernel for each Agent sequentially

**Kernel (Rust, deterministic)**

* Builds context from Record window (+ artifacts later)
* Calls Reasoner (probabilistic) to produce Proposals
* Applies Policy/Capabilities
* Produces Actions
* Invokes Executors
* Records Actions + Effects in RecordEntry
* For async side effects, emits Pending + waits for ActionResult Transactions

**aura-store (Rust, RocksDB)**

* One RocksDB per Swarm
* Column families for Record, Agent metadata, Inbox, etc.

**aura-reasoner (Rust client) + aura-gateway-ts (TS process)**

* `aura-gateway-ts` wraps Claude Code SDK
* Exposes **propose-only** API
* Rust Kernel calls gateway through `aura-reasoner`

**aura-tools (Rust)**

* Implements ToolExecutor for filesystem + command execution (cat, ls, etc.)
* Always executed through Actions; results come back as ActionResult Transactions

---

## 4) Repository / crate layout (canonical)

```
aura/
├─ aura-core          // IDs, schemas, hashing, serialization, errors
├─ aura-store         // RocksDB impl (CFs, keys, iterators, batches)
├─ aura-kernel        // deterministic kernel + policy engine
├─ aura-swarm         // router, scheduler, worker runtime (tokio)
├─ aura-reasoner      // Rust client to TS gateway (http/grpc)
├─ aura-executor      // executor trait + orchestration
├─ aura-tools         // ToolExecutor (fs + cmd) + sandbox
└─ aura-gateway-ts    // TS Reasoner Gateway (Claude Code SDK)
```

---

## 5) Data model (MVP)

### Types

#### AgentId

* `agent_id: [u8; 32]` (canonical)
* Derivation recommended: hash of Identity object or UUIDv4 -> hashed to 32 bytes.

#### Identity (MVP)

```text
Identity {
  agent_id: [u8; 32]
  zns_id: string          // e.g. "0://Agent09"
  name: string            // mutable alias
  identity_hash: [u8; 32] // fingerprint
}
```

#### Transaction (immutable input)

```text
Transaction {
  tx_id: [u8; 32]
  agent_id: [u8; 32]
  ts_ms: u64
  kind: enum {
    UserPrompt,
    AgentMsg,
    Trigger,
    ActionResult,
    System
  }
  payload: bytes // versioned envelope
}
```

#### ProposalSet (from Reasoner)

```text
ProposalSet {
  proposals: Vec<Proposal>
  trace: Optional<Trace> // model, latency, etc.
}

Proposal {
  action_kind: ActionKind
  payload: bytes // versioned structured content
  rationale: Optional<string>
}
```

#### Action / Effect

Action kinds (whitepaper-aligned):

```text
ActionKind = { Reason, Memorize, Decide, Delegate }
```

Effect kinds:

```text
EffectKind = { Proposal, Artifact, Belief, Agreement }
EffectStatus = { Committed, Pending, Failed }
```

MVP Action structure:

```text
Action {
  action_id: [u8; 16]
  kind: ActionKind
  payload: bytes
}
```

MVP Effect structure:

```text
Effect {
  action_id: [u8; 16]
  kind: EffectKind
  status: EffectStatus
  payload: bytes
}
```

#### RecordEntry (one per processed Transaction)

```text
RecordEntry {
  seq: u64
  tx: Transaction

  kernel_version: u32
  context_hash: [u8; 32]            // hash of deterministic inputs used to decide

  proposals: ProposalSet            // recorded verbatim
  decision: Decision                // accept/reject reasoning

  actions: Vec<Action>              // authorized actions
  effects: Vec<Effect>              // durable results (or pending placeholders)
}
```

Decision:

```text
Decision {
  accepted_action_ids: Vec<[u8;16]>
  rejected: Vec<{proposal_index: u32, reason: string}>
}
```

### Serialization

* Use **Protobuf** (recommended) or **CBOR** for values.
* All value types must be versioned: `message_version` field or envelope.

---

## 6) Storage spec: `aura-store` (RocksDB)

### One DB per Swarm

* Single RocksDB instance, created/opened with fixed CF list.

### Column Families (MVP)

* `record` — append-only per-agent entries
* `agent_meta` — head_seq, inbox cursors, status
* `inbox` — durable per-agent tx queue

Optional later:

* `indexes`, `artifacts`, `checkpoints`

### Keyspace (byte-ordered, big-endian numbers)

#### Record

* Key: `R | agent_id(32) | seq(u64be)`
* Value: `RecordEntryV1`

#### Agent metadata

* Key: `M | agent_id(32) | field`
  Fields (MVP):
* `head_seq: u64be` (default 0)
* `inbox_head: u64be`
* `inbox_tail: u64be`
* `status: u8` (Active, Paused, Dead)
* `schema_version: u32be`

#### Inbox

* Key: `Q | agent_id(32) | inbox_seq(u64be)`
* Value: `TransactionV1`

### Atomic commit protocol (must implement)

All updates for processing exactly one Transaction are committed in one RocksDB `WriteBatch`:

Batch includes:

1. Put `record` entry `R|agent|next_seq` → RecordEntry
2. Put `agent_meta` `head_seq` → next_seq
3. Delete `inbox` key `Q|agent|inbox_head` (or current dequeued key)
4. Put `agent_meta` `inbox_head` → inbox_head+1 (if you store cursors)

If the batch fails: **nothing is committed**.

### Durability modes

* Dev: WAL on, `sync=false`
* Prod: WAL on, `sync=true` for "acknowledged" commits
* Expose this as Swarm config.

---

## 7) Swarm runtime (parallel orchestration)

### Runtime model (MVP: single process)

Use Tokio.

**Router**

* `POST /tx` accepts Transaction JSON/proto
* validates
* `store.enqueue_tx(tx)` (durable)
* returns 202 + tx_id

**Scheduler**

* repeatedly picks Agents that have inbox items
* dispatches them to worker tasks
* ensures at most one active worker per agent at a time

**Per-agent single writer**

* Use `DashMap<AgentId, tokio::sync::Mutex<()>>` or a sharded lock table
* Worker must acquire lock before dequeue+process loop

### Worker loop (per Agent)

Pseudo:

```text
lock(agent)
while let Some(tx) = store.dequeue_tx(agent):
  kernel.process(tx) -> RecordEntry (includes actions/effects)
  store.append_entry_atomic(agent, entry, inbox_advance)
unlock(agent)
```

### Agent lifecycle (MVP)

* `status` in `agent_meta` can be Active/Paused/Dead
* Scheduler ignores non-Active agents

---

## 8) Kernel spec (deterministic)

### Inputs

* Current `Transaction`
* Deterministic context derived from:

  * last N RecordEntries (MVP: configurable window, e.g. 50)
  * Identity (optional if stored)
  * Tool outputs from ActionResult transactions (already in Record)

### Context building (MVP)

* Fetch last N entries by scanning `record` range for agent and taking tail.
* Build a compact context:

  * tx payload
  * last N actions/effects
  * last N tool outputs (if any)
* Compute `context_hash` as hash of:

  * tx bytes + canonical serialization of record window entries' minimal fields

### Reasoning call (probabilistic)

* Kernel calls `Reasoner.propose(context)` and gets ProposalSet.
* Kernel records ProposalSet verbatim in RecordEntry.

### Authorization & policy (MVP)

Policy is deterministic and must be applied every time.

Must check:

* action kind allowlist
* tool allowlist for Tool calls
* path restrictions for filesystem tools
* command allowlist (or disabled by default)
* max bytes / max runtime constraints
* any "capability exists" check (ToolExecutor present, Message executor present, etc.)

### Action construction

* Kernel converts accepted Proposals into Actions.
* Kernel can also synthesize internal Actions (e.g., "Memorize summary") later.
* For MVP, keep it simple: accept limited proposal forms.

### Execution orchestration

Kernel invokes `ExecutorRouter.execute(action)` for each action in deterministic order.

Important:

* Executors may return:

  * `Effect { status=Committed }` (sync)
  * `Effect { status=Pending }` (async/external)
  * `Effect { status=Failed }`

### ActionResult flow

For Pending external actions:

* External subsystem produces `Transaction(kind=ActionResult)` later.
* Kernel processes it like any other transaction and updates state by recording it.

**Replay rule:** during replay, **never call Reasoner** and **never run Tools**. The Record already contains proposals/decisions/results.

---

## 9) Executors (deterministic boundary)

### Executor trait

```rust
pub trait Executor: Send + Sync {
  fn execute(&self, ctx: &ExecuteContext, action: &Action) -> Result<Effect>;
}
```

`ExecuteContext` includes:

* `agent_id`
* `action_id`
* swarm config limits
* working directory for tools (sandbox root)
* handles for messaging, store (if needed), etc.

### ExecutorRouter (aura-executor)

* Dispatch by action type / payload subtype:

  * `ReasonExecutor` (optional if you ever represent Reason as an action; in MVP Kernel calls Reasoner directly)
  * `ToolExecutor` (**aura-tools**)
  * `MessageExecutor` (optional MVP)
  * `ArtifactExecutor` (optional MVP)
  * `DelegateExecutor` (stub or used for non-tool external calls)

**MVP requirement:** include `ToolExecutor`. Others can be stubs.

---

## 10) Tools component (NEW): `aura-tools`

Tools are how the system safely performs **filesystem** and **command** operations.

### Key design rule

* Tools are **only invoked via Actions** authorized by Kernel Policy.
* Tool outputs return as:

  * immediate Effect (sync)
  * or Pending + ActionResult Transaction (if you choose async execution)

### Tool action payload schema (MVP)

Use `Action.kind = Delegate` with a `tool_call` payload:

```text
ToolCall {
  tool: string  // "fs.ls", "fs.read", "cmd.run", ...
  args: bytes   // versioned args per tool
}
```

### Tool allowlist (MVP)

Filesystem:

* `fs.ls { path }`
* `fs.read { path, max_bytes }`
* `fs.stat { path }`

Command:

* `cmd.run { program, args[], cwd, timeout_ms, max_stdout_bytes, max_stderr_bytes }`

**Default stance (recommended):**

* enable filesystem tools
* disable command tools unless explicitly allowed in config/policy

### Sandbox + safety constraints (must implement)

Per-agent workspace root:

* `workspace_root = <swarm_data_dir>/workspaces/<agent_id_hex>/`

Path rules:

* All paths must resolve under `workspace_root`
* No `..` escapes
* Use canonicalization and prefix checks

Limits:

* Max read bytes (e.g., 1–5MB)
* Max write bytes (if enabled later)
* Max command runtime (e.g., 2–10s)
* Max stdout/stderr bytes (e.g., 256KB)
* Command allowlist (e.g., only `ls`, `cat`, `rg` initially) OR disable commands

### Tool outputs (Effect payload)

For sync tools:

```text
ToolResult {
  tool: string
  ok: bool
  exit_code: Optional<i32>
  stdout: bytes
  stderr: bytes
  metadata: map<string,string> // e.g. stat fields
}
```

Return as:

* `Effect.kind = Agreement` (or Artifact) with status `Committed`

If async:

* return Pending Effect immediately
* later emit ActionResult Transaction with ToolResult

**MVP recommendation:** run tools synchronously with strict timeouts, return Committed Effect.

---

## 11) Reasoner integration: `aura-reasoner` + `aura-gateway-ts`

### What the TS gateway does

* Wrap Claude Code SDK
* Implements "propose-only"
* Returns structured proposals for the Kernel to authorize
* Must not execute tools directly; it only suggests tool calls

### Rust `aura-reasoner`

* Client library to call TS gateway
* Handles retries/timeouts
* Returns `ProposalSet` to Kernel

### API contract (HTTP JSON acceptable for MVP)

`POST /propose`
Request:

```json
{
  "agent_id": "base64",
  "tx": { "tx_id":"...", "kind":"UserPrompt", "payload":"..." },
  "record_window": [ /* compact record summaries */ ],
  "limits": { "max_proposals": 8 }
}
```

Response:

```json
{
  "proposals": [
    {
      "action_kind": "Delegate",
      "payload": { "tool_call": { "tool":"fs.read", "args": { "path":"src/main.rs" } } },
      "rationale": "Need to inspect file"
    }
  ],
  "trace": { "model":"claude-code", "latency_ms": 900 }
}
```

### Determinism note

* The kernel records the proposals it received.
* Replay uses the recorded proposals/decisions; it does not call the gateway.

---

## 12) Public API (Swarm endpoints)

MVP endpoints (HTTP):

1. **Submit Transaction**

* `POST /tx`
* body: TransactionV1
* response: `{ accepted: true, tx_id }`

2. **Agent head**

* `GET /agents/{agent_id}/head`
* response: `{ head_seq }`

3. **Scan record**

* `GET /agents/{agent_id}/record?from_seq=N&limit=M`
* response: RecordEntry list

4. **Health**

* `GET /health`

Optional:

* `POST /agents` create agent identity record
* `POST /agents/{id}/pause|resume`

---

## 13) Rust interfaces (canonical)

### Store

```rust
pub trait Store: Send + Sync {
  fn enqueue_tx(&self, tx: Transaction) -> anyhow::Result<()>;
  fn dequeue_tx(&self, agent: AgentId) -> anyhow::Result<Option<(u64, Transaction)>>; // returns inbox_seq too
  fn get_head_seq(&self, agent: AgentId) -> anyhow::Result<u64>;
  fn append_entry_atomic(
    &self,
    agent: AgentId,
    next_seq: u64,
    entry: &RecordEntry,
    dequeued_inbox_seq: u64,
  ) -> anyhow::Result<()>;
  fn scan_record(&self, agent: AgentId, from_seq: u64, limit: usize) -> anyhow::Result<Vec<RecordEntry>>;
}
```

### Reasoner

```rust
pub trait Reasoner: Send + Sync {
  fn propose(&self, req: ProposeRequest) -> anyhow::Result<ProposalSet>;
}
```

### Executor

```rust
pub trait Executor: Send + Sync {
  fn execute(&self, ctx: &ExecuteContext, action: &Action) -> anyhow::Result<Effect>;
}
```

---

## 14) Processing algorithm (exact)

For each Agent:

1. Acquire per-agent lock
2. Dequeue one Transaction from inbox
3. Read head_seq
4. Compute `next_seq = head_seq + 1`
5. Load record window (last N entries)
6. Build context + compute `context_hash`
7. Call Reasoner → proposals
8. Policy authorizes proposals → actions
9. Execute actions → effects (some pending)
10. Build RecordEntry
11. WriteBatch atomic commit:

    * record entry
    * head_seq update
    * inbox dequeue
12. Release lock
13. Repeat until inbox empty

---

## 15) Error handling (MVP rules)

* If Reasoner fails/timeouts:

  * Kernel records empty proposals + Decision explaining reasoner failure
  * Kernel may emit a fallback action: "respond with error" (optional) or do nothing
* If ToolExecutor fails:

  * return Failed effect with error payload
  * still commit the RecordEntry (so failure is recorded)
* If RocksDB write fails:

  * do not advance head_seq
  * do not delete inbox key
  * transaction remains pending for retry

---

## 16) Observability (minimum)

* Structured logs (tracing):

  * agent_id, tx_id, seq, durations (context build, reasoner, tools, commit)
* Metrics:

  * inbox depth per agent (optional)
  * processed tx/sec
  * reasoner latency
  * tool exec latency + failures
  * commit latency

---

## 17) Testing requirements (must)

### Determinism tests

* Given a fixed Record, replay must yield identical derived state outputs (at least identical head_seq and entry decoding).
* Kernel must not call Reasoner/Tools during replay mode.

### Atomicity tests

* Simulate crash between steps: ensure no partial commits.
* Verify either both record entry + head_seq move, or neither.

### Concurrency tests

* Many agents in parallel: ensure no agent has seq gaps or duplicated seq.

### Tool sandbox tests

* Path traversal attempts must fail.
* Command timeouts must terminate.

---

## 18) MVP scope (what development must deliver)

### Must have

* `aura-store`: record + agent_meta + inbox CFs, keyspace, atomic WriteBatch
* `aura-swarm`: router + scheduler + per-agent lock + workers
* `aura-kernel`: context builder + policy + record entry builder + execution orchestration
* `aura-reasoner`: client to TS gateway
* `aura-gateway-ts`: propose-only Claude Code integration
* `aura-tools`: ToolExecutor with fs.ls/fs.read/fs.stat + sandboxing

### Nice to have (optional but easy)

* tx_id index
* simple CLI: `replay`, `tail`, `inspect`

### Not MVP (explicitly excluded)

* Multi-node writers (leases)
* Wallet/KMS integration
* Cross-swarm replication
* Global ordering across agents

---

## 19) Extension points (designed in)

You can add later without breaking the MVP design:

* `artifacts` CF + Memorize actions
* `decisions/beliefs` policy evolution
* checkpoints CF for faster replay
* multi-node swarms via external leases
* additional Reasoners (OpenAI/local planners) via gateway multiplexing
* richer Tools (write/patch/apply) with stricter policies
