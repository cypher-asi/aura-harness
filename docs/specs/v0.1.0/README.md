# v0.1.0 Design Specs — Historical

This directory contains the **v0.1.0 design specifications** for the
Aura harness. These documents describe the architecture as it was
imagined during the early 2026 design sprint; they are preserved here
as a historical record of intent, not as a description of the current
code.

## What changed between these specs and the current tree

Several crate names and boundaries in the v0.1.0 specs no longer match
the workspace. In particular:

| Spec reference | Current location |
|---|---|
| `aura-cli` crate (interactive REPL, approvals, slash commands, record tailing) | **Never created.** The intended surface is split between the **root `aura` binary** (`src/` — interactive TUI, login/logout/whoami, embedded API server) and **`aura-node`** (headless HTTP + WebSocket server). See `README.md` → "Binaries". |
| `aura-cli/src/main.rs` | `src/main.rs` (root `aura` binary) |
| `aura-cli/src/session.rs` | `crates/aura-node/src/session/` for the WS-bridged session flow; the TUI session helpers live in `src/session_helpers.rs`. |
| `aura-cli/src/bridge.rs` | `src/event_loop/` + `aura-terminal` (TUI command surface). |
| `aura-executor` crate | Dissolved into `aura-core` + `aura-kernel`. |
| `aura-gateway-ts` (Node.js sidecar) | Removed; the Rust `aura-reasoner` talks directly to the Anthropic API or to `aura-router` via JWT. |
| `aura-session` crate | Dissolved into binary-local session helper modules (`src/session_helpers.rs`, `crates/aura-node/src/session/`). |

Wherever one of the `v0.1.0/` specs references `aura-cli/src/...`,
read it as a reference to the equivalent path under `src/...` in the
root `aura` binary.

## Why these specs are kept

The specs are still useful as:

1. **Intent capture** — they record *why* a system was built this way,
   not just *what* was built.
2. **Cross-reference** for invariants (`docs/invariants.md`) and the
   refactor checklists in `docs/refactoring/`.
3. **Onboarding reading** for contributors who want the original
   rationale behind the record / kernel / tools split.

For the current architecture, start at
[`docs/architecture.md`](../../architecture.md) instead. For current
status, see [`docs/PROGRESS.md`](../../PROGRESS.md).

_Last updated: 2026-04-22 (Phase 5d of the remediation plan, finalising
the `aura-cli` removal)._
