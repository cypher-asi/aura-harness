---
name: POLICIES tab in the Kernel panel
overview: 'Add a POLICIES tab next to KERNEL and MEMBERS in the top-right Kernel panel. It shows the policy the kernel of the agent on screen enforces: mode, approval level, capabilities and scope, how each tool resolves (on/ask/off and which rule decides), project "always allow" rules, session grants, and the general guardrail status.'
todos:
- id: kernel-api
  content: 'Kernel: extract tool-call evaluation; add `explain_tool`, `session_tool_states`, prompter/hook accessors + tests'
  status: completed
  assignee: rust-engineer
- id: protocol
  content: 'aura-ui: add `PolicyView`, `ToolPolicyView`, `UiCommand::SetPolicies`, `UiEvent::RequestPolicies`'
  status: completed
- id: app-builder
  content: 'aura-app: new `policies.rs` building the view; `TerminalApprovals::rules_for`; unit tests'
  status: completed
  assignee: rust-engineer
- id: app-wiring
  content: 'aura-app: send on switch/startup/mode change/always-allow; handle `RequestPolicies`; loop test'
  status: completed
- id: ui-state
  content: 'TUI: `RecordsTab::Policies`, tab cycling, `Hud::policies`, keys/scroll, `/policies` command'
  status: completed
- id: ui-render
  content: 'TUI: `renderer/policies.rs`, header info, hint, empty state'
  status: completed
- id: tests-fix
  content: Update tab-bar string tests (members, renderer, desktop chrome) and add new UI/render tests
  status: completed
- id: docs
  content: STYLEGUIDE bar diagram and CHANGELOG entry
  status: completed
- id: verify
  content: Run fmt, clippy -D warnings, tests for kernel, aura-ui, aura-app, aura-ui-terminal, aura-desktop
  status: completed
---

# POLICIES tab in the Kernel panel

Add a POLICIES tab next to KERNEL and MEMBERS in the top-right Kernel panel. It shows the policy the kernel of the agent on screen enforces: mode, approval level, capabilities and scope, how each tool resolves (on/ask/off and which rule decides), project "always allow" rules, session grants, and the general guardrail status.

## Todos

- [x] Kernel: extract tool-call evaluation; add `explain_tool`, `session_tool_states`, prompter/hook accessors + tests (@rust-engineer)
- [x] aura-ui: add `PolicyView`, `ToolPolicyView`, `UiCommand::SetPolicies`, `UiEvent::RequestPolicies`
- [x] aura-app: new `policies.rs` building the view; `TerminalApprovals::rules_for`; unit tests (@rust-engineer)
- [x] aura-app: send on switch/startup/mode change/always-allow; handle `RequestPolicies`; loop test
- [x] TUI: `RecordsTab::Policies`, tab cycling, `Hud::policies`, keys/scroll, `/policies` command
- [x] TUI: `renderer/policies.rs`, header info, hint, empty state
- [x] Update tab-bar string tests (members, renderer, desktop chrome) and add new UI/render tests
- [x] STYLEGUIDE bar diagram and CHANGELOG entry
- [x] Run fmt, clippy -D warnings, tests for kernel, aura-ui, aura-app, aura-ui-terminal, aura-desktop

<!-- end of generated header: edit below -->

## Approach

The kernel already holds everything in `Policy` (`PolicyConfig` + session grants). The UI gets none of it except mode and approval. The event loop builds a read-only `PolicyView` snapshot from the agent's kernel and sends it as `UiCommand::SetPolicies`, addressed to that agent through `AgentUi`/`ForAgent`. The UI keeps it in the per-agent `Hud`, so it moves with agent switches like `instructions` does. The new tab renders it as a scrollable, read-only document.

Data flow:

```mermaid
sequenceDiagram
    participant UI as aura-ui-terminal
    participant Loop as aura-app event loop
    participant K as Kernel / Policy
    UI->>Loop: UiEvent::RequestPolicies(agent) (tab opened)
    Loop->>K: config(), explain_tool(t), session_tool_states(), snapshot_hash()
    Loop->>UI: ForAgent{ SetPolicies(PolicyView) }
    Note over Loop,UI: also pushed on agent switch, mode/approval switch, "always allow"
```

## What the tab shows

Header info cell: `AGENT/ASK` (mode/approval). It is muted normally and uses the warning color under `yolo`. Body sections, top to bottom:

- **STATUS**: mode, approval level and its description, command confinement (Landlock available or not), live prompts (prompter wired: yes/no, timeout), permission hooks (none/N), policy hash (first 8 hex characters), max proposals, allowed action kinds.
- **CAPABILITIES**: the granted `Capability` list (`n/12`) and the scope (`all orgs, projects, agents`, or the whitelists).
- **TOOLS**: a summary `18 on · 9 ask · 0 off`, then one row per offered tool with a glyph (`✓`/`?`/`✗`), its state, and the deciding rule (`mode.plan`, `user_default.per_tool[...]`, `agent_capability.requires SpawnAgent`, `session_grant...`).
- **ALWAYS ALLOW (this project)**: rule labels from `approvals.json`, such as `` `cargo test` commands``. Shows "ignored at strict" when the level is strict.
- **SESSION GRANTS**: tool=state, or "none".
- **INTEGRATIONS**: installed integrations, or "none".

Empty state, when the agent has no open kernel: "No kernel open for this agent."

## Files to change

| File | Change |
|---|---|
| `crates/agent/aura-agent-kernel/src/policy/check/delegate_gate.rs` | Extract `evaluate_tool_call_explained(&ToolCall, ..)` from `evaluate_delegate_explained` (no behavior change) |
| `crates/agent/aura-agent-kernel/src/policy/check/mod.rs` | `pub fn explain_tool(&self, tool) -> PolicyEvaluation` (dry run with `{}` args, no side effects); `pub fn session_tool_states(&self) -> BTreeMap<String, ToolState>` |
| `crates/agent/aura-agent-kernel/src/kernel/mod.rs` | `has_approval_prompter()`, `permission_hook_count()` (from `plugin_hooks.is_empty(PermissionRequest)`) |
| `crates/agent/aura-agent-kernel/src/policy/check/tests.rs` | Tests for `explain_tool` (plan mode denies `write_file` by `mode`; ask fallback; capability deny) and `session_tool_states` |
| `crates/ui/aura-ui/src/events.rs` | `PolicyView`, `ToolPolicyView` (name, state label, decided_by); `UiCommand::SetPolicies(PolicyView)`; `UiEvent::RequestPolicies(String)` |
| `crates/surface/aura-app/src/event_loop/policies.rs` (new) | `policy_view(kernel, tools, rules, live)` builds the view (pure, unit-tested); `send_policies_of(state, agent_id)` |
| `crates/surface/aura-app/src/event_loop/approvals.rs` | `pub fn rules_for(&self, agent_id) -> Vec<String>` (labels of the folder's rules) |
| `crates/surface/aura-app/src/event_loop/{agents.rs,modes.rs,mod.rs}` | Send policies in `show_active_agent`, at startup (next to `send_mode`), after a successful `switch`, after `answer_approval(.., Always)`; dispatch `UiEvent::RequestPolicies` |
| `crates/ui/aura-ui-terminal/src/app/members.rs` | `RecordsTab::Policies` ("POLICIES"/"POL"); `ALL` has 3; replace `other()` with `next()`/`prev()` for Shift+Left/Right; `click_records_panel` branch; `show_records_tab` sends `RequestPolicies` when Policies opens |
| `crates/ui/aura-ui-terminal/src/app/policies.rs` (new) | Scroll state, `show_policies()` (`/policies`), keys (Up/Down/PgUp/PgDn/Home/End), `sync_policies_scroll` |
| `crates/ui/aura-ui-terminal/src/app/{hud.rs,commands.rs,keys.rs,mod.rs,slash_menu.rs}` | `Hud::policies: Option<PolicyView>`; handle `SetPolicies`; route Records focus keys and wheel scroll by tab; `/policies` (alias `/perms`) command and menu entry |
| `crates/ui/aura-ui-terminal/src/renderer/policies.rs` (new) | `render_policies`, built from flattened lines, with scrollbar and hint `↑↓ scroll  ⇧←→ tabs` |
| `crates/ui/aura-ui-terminal/src/renderer/{panels.rs,side_list.rs,mod.rs}` | Header info for Policies; dispatch to `render_policies`; `POLICIES_HINT` |
| `crates/surface/aura-desktop/src/chrome.rs` | Extend `the_kernel_bar_has_its_tabs_and_keeps_its_count` to 3 tabs (the chrome is generic otherwise) |
| `crates/ui/aura-ui-terminal/STYLEGUIDE.md`, `CHANGELOG.md` | Bar diagram with three tabs; `[Unreleased]` Added entry |

## Key code sketch

```rust
// aura-agent-kernel policy/check/mod.rs
/// How `tool` would be decided now, without arguments: mode, capability,
/// integration, session grant and tri-state gates. Records nothing.
#[must_use]
pub fn explain_tool(&self, tool: &str) -> PolicyEvaluation {
    let call = ToolCall { tool: tool.to_string(), args: serde_json::json!({}) };
    let mut trace = Vec::new();
    let (verdict, rule) = self.evaluate_tool_call_explained(&call, None, &mut trace);
    PolicyEvaluation::new(verdict, rule, trace)
}
```

```rust
// aura-ui events.rs
pub struct PolicyView {
    pub mode: PermissionMode,
    pub approval: ApprovalLevel,
    pub policy_hash: String,
    pub action_kinds: Vec<String>,
    pub max_proposals: usize,
    pub capabilities: Vec<String>,
    pub capability_total: usize,
    pub scope: String,
    pub tools: Vec<ToolPolicyView>,
    pub always_allow: Vec<String>,
    pub session_grants: Vec<String>,
    pub integrations: Vec<String>,
    pub commands_confined: bool,
    pub live_prompts: bool,
    pub permission_hooks: usize,
}
```

## Tests

- Kernel: `explain_tool` verdicts and rules under plan/agent mode, a narrowed capability bundle, and the ask fallback. `session_tool_states` reflects `remember_tool_state_for_session`.
- aura-app `policies.rs`: `policy_view` for `agent/ask` (reads on, writes ask, rule labels), for `plan/strict` (writes off by `mode.plan`, rules flagged ignored), and for `yolo`. The event loop test checks that an agent switch sends `SetPolicies` after `SetMode`, and that `RequestPolicies` answers for the active agent.
- UI app: Shift+Right cycles KERNEL to MEMBERS to POLICIES to KERNEL; opening POLICIES sends `RequestPolicies(active)`; `/policies` shows it focused; `SetPolicies` for a background agent lands in its view and shows after switching.
- Renderer: the tab lists STATUS/TOOLS sections and glyphs; the header info shows `AGENT/ASK`; the empty state; the hosted chrome hands over three tabs.

## Risks

- **Tab widths:** `POLICIES` (8 characters) widens every cell from 9 to 10 columns, and three tabs leave less room for the info cell. On a narrow panel the count falls back to `info_short` or disappears. Tests that match `"│ KERNEL  │ MEMBERS │"` or locate the bar by `"│ KERNEL"` (`renderer/members.rs`, `renderer/tests.rs` around lines 1051 and 4085) need updating to the new padding.
- **Dry-run accuracy:** `explain_tool` uses empty args. Path-sensitive gates (markdown writes in plan mode, scope checks) can differ for real calls, so the TOOLS section says "per tool, before arguments".
- **Staleness:** "always allow" rules and session grants change outside a mode switch. The push after `Always` answers and the request when the tab opens cover this, but there is no live subscription.
- **Exhaustive matches** on `RecordsTab` (`keys.rs`, `mod.rs` scroll, `members.rs`) must all gain the new arm. The compiler finds them.
