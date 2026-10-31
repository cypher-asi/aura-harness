# HTTP / WebSocket API

The wire API is Aura's external contract: clients such as `aura-os` talk to the `aura-node` server over HTTP and WebSocket, never by linking workspace crates.

All routes are defined in [`gateway/middleware.rs`](../crates/aura-runtime/src/gateway/middleware.rs) (`create_router`), with handlers grouped under [`gateway/handlers/`](../crates/aura-runtime/src/gateway/handlers). The shared `RouterState` lives in [`gateway/state.rs`](../crates/aura-runtime/src/gateway/state.rs). Paths below use Axum path-parameter syntax. Authentication is described in [configuration.md](configuration.md#authentication).

## Runs

A run is the entry point for all work. Start a chat session, dev-loop automaton, or single-task automaton by posting an [`aura_protocol::RuntimeRequest`](../crates/aura-protocol/src/runtime_request.rs) to `/v1/run`. The [`RuntimeRunResponse`](../crates/aura-protocol/src/runtime_request.rs) carries a `run_id` and the WebSocket path the client opens to receive events (and, on chat runs, to send user messages).

`RuntimeRequest.mode` (`agent` / `plan` / `ask` / `debug`, optional) sets the session mode. When omitted it falls back to `AURA_FLEET_DEFAULT_MODE`, then `agent`. Dev-loop and task-run requests are rejected with `400` unless the resolved mode is `agent`.

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/v1/run` | Start a run (chat / dev-loop / task-run). Body is `RuntimeRequest`; returns `RuntimeRunResponse { run_id, event_stream_url }`. |
| GET  | `/v1/run/list` | List active runs. |
| GET  | `/v1/run/:run_id/status` | Status for one run. |
| POST | `/v1/run/:run_id/pause` | Pause a run. |
| POST | `/v1/run/:run_id/stop` | Stop a run. |

## WebSocket

| Path | Purpose |
|------|---------|
| `/stream/:run_id` | Per-run event stream. Bidirectional on `Chat` runs (user messages in; deltas, tool calls and approvals out); event-only on `DevLoop` / `TaskRun` runs. The `run_id` is the value returned by `POST /v1/run`. |
| `/ws/terminal` | Terminal bridge used by the TUI. |
| `/ws/preview/tcp/:port` | WebSocket-to-TCP bridge for previewing a port served from the workspace. |

TypeScript bindings for the wire types are generated into [`crates/aura-protocol/bindings/`](../crates/aura-protocol/bindings).

## Health and workspace

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Liveness probe. |
| GET | `/api/files` | List files in the configured workspace root. |
| GET | `/api/read-file` | Read a file from the workspace root. |
| PUT | `/api/write-file` | Write a file under the workspace root. |
| GET | `/workspace/resolve` | Resolve a project/workspace slug to a filesystem path. |
| POST | `/workspace/import` | Create a workspace from a JSON list of files (12 MiB body limit). |
| DELETE | `/workspace/:workspace_key` | Delete a workspace. |

### Safe workspaces

Per-session Git worktrees with recoverable checkpoints, managed beside the runtime so the caller never touches the host filesystem.

| Method | Path | Purpose |
|--------|------|---------|
| GET, POST | `/workspace/:workspace_key/safe/:session_id` | Status of / prepare the session's worktree. |
| GET | `/workspace/:workspace_key/safe/:session_id/checkpoints/:checkpoint_id/diff` | Diff for a checkpoint. |
| POST | `/workspace/:workspace_key/safe/:session_id/checkpoints/:checkpoint_id/restore` | Restore a checkpoint. |
| POST | `/workspace/:workspace_key/safe/:session_id/apply` | Apply the worktree's changes to the source project. |

## Transactions and records

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/tx` | Submit a transaction for an agent. |
| GET  | `/tx/status/:agent_id/:tx_id` | Status of a submitted transaction. |
| GET  | `/agents/:agent_id/head` | Latest record sequence for an agent. |
| GET  | `/agents/:agent_id/record` | Paginated record scan. |
| GET  | `/agents/:agent_id/tool_calls/:tool_use_id/audit` | The proposal → policy decision → execution chain for one tool call (`{ proposal, policy_decision, decision, execution }`). `decision` is the decoded `PolicyDecision` (verdict, `decided_by` rule, gate `trace`, live `approval`, `mode`, `policy_hash`); `execution` is `null` for denied or pending calls. 404 if no decision is recorded within the last 10,000 entries. |

`POST /tx` accepts only `user_prompt`, `agent_msg`, `trigger`, `action_result`, and `system`. Tool proposals, `policy_decision`, and `tool_execution` entries are written by the kernel itself.

During a chat run, the session WebSocket also emits a `tool_policy_decision` frame as soon as each decision is persisted (before the tool runs). It carries the same decision fields plus `seq`, `tx_hash`, and `proposal_tx_hash`.

## Tool permissions and defaults

Tri-state (`on` / `off` / `ask`) tool configuration consumed by the policy gate ([invariants §4](invariants.md#4-full-policy-enforcement)). PUTs append a kernel-built record entry, serialized through the per-agent scheduler claim (§12.a).

| Method | Path | Purpose |
|--------|------|---------|
| GET, PUT | `/users/:user_id/tool-defaults` | Read / write the user's `UserToolDefaults` baseline. |
| GET, PUT | `/agents/:agent_id/tool-permissions` | Read / write per-agent `AgentToolPermissions` overrides. |
| GET | `/agents/:agent_id/tools` | Resolved tool list for an agent (catalog ∩ permissions). |

## Memory

Canonical routes are mounted under `/memory/...`; compatibility aliases are mounted under `/api/agents/:agent_id/memory/...` (the `aura-os` proxy uses the alias form). Both cover:

- Facts: list / create / update / delete, fetch by key.
- Events: list / create / delete, bulk-delete.
- Procedures: list / create / update / delete.
- `GET /memory/:agent_id/snapshot` — full memory snapshot.
- `POST /memory/:agent_id/wipe` — clear all memory for an agent.
- `GET /memory/:agent_id/stats` — counts and token budgets.
- `GET, PUT /memory/:agent_id/continuity` — read / update the continuity config.
- `GET /memory/:agent_id/retrieval/latest` — the latest retrieval trace.
- `POST /memory/:agent_id/consolidate` — trigger consolidation.

The alias router also exposes `GET /api/agents/:agent_id/memory` (snapshot) and `DELETE /api/agents/:agent_id/memory` (wipe).

## Skills

| Method | Path | Purpose |
|--------|------|---------|
| GET, POST | `/api/skills` | List available skills / install a skill. |
| GET | `/api/skills/:name` | Skill details. |
| POST | `/api/skills/:name/activate` | Activate a skill. |
| GET, POST | `/api/agents/:agent_id/skills` | Per-agent install list / install. |
| DELETE | `/api/agents/:agent_id/skills/:name` | Uninstall a skill from an agent. |

Legacy aliases for skill list/install/uninstall are mounted under `/api/harness/agents/:agent_id/skills...`.

## Secrets

An in-VM secrets vault. Values are sealed at rest and only returned on explicit `?reveal=true` reads.

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/secrets` | List secret names and metadata (never values). |
| GET, PUT, DELETE | `/secrets/:name` | Read / write / delete one secret. |

## Processes

Scheduled automations (cron + prompt + config) and their run history, sealed at rest.

| Method | Path | Purpose |
|--------|------|---------|
| GET, POST | `/v1/processes` | List / create processes. |
| GET, PUT, DELETE | `/v1/processes/:id` | Read / update / delete a process. |
| POST | `/v1/processes/:id/trigger` | Trigger a process now. |
| GET | `/v1/processes/:id/runs` | Run history for a process. |

## Limits

Every route except `/health` sits behind the optional bearer-token gate. Mutating endpoints (`/tx`, `/v1/run`, `/api/write-file`, run pause/stop, and the safe-workspace writes) get a stricter per-IP rate limit. Request bodies default to a 1 MiB cap, with tighter per-route caps on endpoints that take small or no bodies.
