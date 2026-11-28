---
name: Integrations system and panel
overview: 'Add one standard way to plug any external service into aura: a manifest-and-trait model with a registry, sealed credential storage, health checks, agent tools and access rules. Move GitHub and boxd onto it, and add an Integrations icon next to BENCH that lists every integration with its status and actions, in the terminal and the desktop app.'
todos:
- id: core-crate
  content: 'Create aura-engine-integrations: types, trait, context, errors, registry, access'
  status: pending
- id: secrets
  content: Sealed SecretStore (SealCipher + local store key), memory store, redacted Secret
  status: pending
- id: tools
  content: Tool adapter, <id>_<tool> naming, tools_for(agent, channel), mode-gate classification
  status: pending
- id: core-tests-docs
  content: Core tests, docs/integrations.md (inventory, design, how-to), fragment
  status: pending
- id: pr-a
  content: 'PR A: reset branch to main, push, PR, bring in main, merge pinned; tell RUS205'
  status: pending
- id: builtins
  content: GitHub and boxd integrations with status, test, connect, disconnect and tests
  status: pending
- id: app-wiring
  content: aura-app registry service, kernel tool registration, REPO tab token, boxd key fallback
  status: pending
- id: pr-b
  content: 'PR B: tests, fmt, clippy, push, PR, bring in main, merge pinned'
  status: pending
- id: header-icons
  content: Generalize header icon tabs (terminal + desktop), plug glyph, Integrations tab
  status: pending
- id: panel
  content: Integrations panel rows, dots, actions, modal input, UiCommand/UiEvent views
  status: pending
- id: styleguide
  content: STYLEGUIDE.md, docs update, UI tests, fragment
  status: pending
- id: pr-c
  content: 'PR C: tests, fmt, clippy, push, PR, bring in main, merge pinned; report'
  status: pending
---

# Integrations system and panel

Add one standard way to plug any external service into aura: a manifest-and-trait model with a registry, sealed credential storage, health checks, agent tools and access rules. Move GitHub and boxd onto it, and add an Integrations icon next to BENCH that lists every integration with its status and actions, in the terminal and the desktop app.

## Todos

- [ ] Create aura-engine-integrations: types, trait, context, errors, registry, access
- [ ] Sealed SecretStore (SealCipher + local store key), memory store, redacted Secret
- [ ] Tool adapter, <id>_<tool> naming, tools_for(agent, channel), mode-gate classification
- [ ] Core tests, docs/integrations.md (inventory, design, how-to), fragment
- [ ] PR A: reset branch to main, push, PR, bring in main, merge pinned; tell RUS205
- [ ] GitHub and boxd integrations with status, test, connect, disconnect and tests
- [ ] aura-app registry service, kernel tool registration, REPO tab token, boxd key fallback
- [ ] PR B: tests, fmt, clippy, push, PR, bring in main, merge pinned
- [ ] Generalize header icon tabs (terminal + desktop), plug glyph, Integrations tab
- [ ] Integrations panel rows, dots, actions, modal input, UiCommand/UiEvent views
- [ ] STYLEGUIDE.md, docs update, UI tests, fragment
- [ ] PR C: tests, fmt, clippy, push, PR, bring in main, merge pinned; report

<!-- end of generated header: edit below -->

## What exists today

External services are wired in five separate ways, and none of them shares code with another:

| Service | Credentials | Config | Agent tools | Status / health | UI |
|---|---|---|---|---|---|
| GitHub (local) | `gh` CLI login, or `GH_TOKEN`/`GITHUB_TOKEN` read in `aura-app/src/event_loop/repo/github.rs` (`Api::from_env`) | none | git tools in `aura-exec-tools/src/git_tool`, not GitHub's API | REPO tab says "Sign in with gh auth login…" on failure | REPO tab only |
| GitHub (cloud) | GitHub App key path in `.env` (`AURA_GITHUB_APP_*`, `aura-config/src/cloud`); per-repo installation tokens kept as boxd host-bound secrets (`aura-engine-cloud/src/github`, `repos.rs`) | `.env` | `git_ship` and pushes on machines | none | none |
| boxd | `BOXD_API_KEY` or `BOXD_TOKEN` written in plain text to `~/.aura/.env` by `aura cloud setup` (`aura-config/src/cloud/credential.rs`) | `.env` (`BOXD_API_URL`…) | none | `aura cloud status` (CLI only) | cloud mark on agents |
| Model plans (`aura auth`) | `aura-auth` vault: OS keychain, plain `0600` file fallback | `auth/index.json` | none | `aura whoami` | header dot |
| Trusted integrations (Notion, GitHub, Linear, Slack, Brave, Resend, Google) | Sent per request by aura-os in `installed_integrations` and `installed_tools` (`aura-core-types` `InstalledToolRuntimeIntegration`), run by `aura-exec-tools/src/resolver/trusted` | aura-os | yes, behind the kernel's `integration_gate` | none | none |
| Plugin connectors | `aura-plugin` manifest `connectors` (id and endpoint only) | `config.toml` | none (registry only) | none | `aura plugins list` |

So there is no list of "what services aura talks to", no shared credential path (boxd's key sits in plain text), no common health check and no panel.

## The model: `aura-engine-integrations`

A new crate, `crates/engine/aura-engine-integrations` (`//! Layer: engine`). It sits in the engine layer so it can use `aura-store-db` (sealing), `aura-exec-tools` (the `Tool` trait), `aura-engine-cloud` (boxd) and `reqwest`. The interface is the one sent to RUS205:

- `IntegrationId` is a newtype for `[a-z0-9-]{1,32}`.
- `Manifest` holds:
  - `id`, `name`, `icon` (a terminal glyph plus a desktop glyph), `description` and `provides`;
  - `auth: AuthKind`, which is `None`, `ApiKey { label, env, hint }`, `CliLogin { program, login }`, or `OAuth { authorize_url, scopes }` (declared only for now);
  - `config: Vec<ConfigField>`, `tools: Vec<ToolSpec { name, description, input_schema, effect: Read|Write }>`;
  - `cloud: CloudDelivery`, which is `Never` or `HostBound { env }`.
- The `Integration` trait has three methods:
  - `manifest()`;
  - `status(&ctx)`, which is local and quick (Connected via what, NotConnected with a hint, or Misconfigured);
  - `health(&ctx)`, one real call bounded by `ctx.timeout()`, and `call_tool(&ctx, tool, args)`.
- `IntegrationContext` is scoped to one id. It offers `secret()` (the stored secret, else the manifest's env vars), `config(key)`, a shared `reqwest` client with a timeout, and `timeout()`.
- `Secret` wraps `Zeroizing<String>`, and its `Debug` is redacted. `IntegrationError` uses `thiserror` and carries the id, tool and timeout values. No message ever includes a secret or a response body beyond the service's error message.
- `Registry` covers:
  - `add`, which rejects duplicate ids, plus `list` and `get`;
  - `connect(id, secret)`, which runs `health` with the new secret before storing it;
  - `configure(id, key, value)` and `disconnect(id)`;
  - `set_access(id, Access)`;
  - `views()`, which gives the panel each integration's status;
  - `tools_for(agent, channel)`.
- Tools are exposed as `IntegrationToolAdapter: aura_exec_tools::Tool`, named `<id>_<tool>` with `-` turned into `_`. They're registered on an agent's executor only when the integration is connected and `Access` allows that agent or channel. `Access` allows all agents by default, or lists agent ids and channel ids. Each tool's `effect` is classified in the kernel mode gate: `Read` counts as Invoke, so it runs in plan and ask mode, and `Write` counts as Network. A name lookup covers this, and `mode_gate_catalog` is updated to match.

### Credentials, never in plain text

- Secrets go into one sealed file, `<data_dir>/integrations/secrets.sealed`, a JSON map encrypted with AES-256-GCM. It uses `aura-store-db`'s `SealCipher` and the local store's own key from #93 (`local_store::KeySource::for_data_dir`, kept in the OS keychain or `AURA_STATE_KEY_FILE`). The write is atomic and the file is `0600`. Behind it is a `SecretStore` trait, with an in-memory version for tests.
- Non-secret config and access go in `<data_dir>/integrations/config.json`.
- Secrets are never logged: `Secret`'s `Debug` and `Display` are redacted, and tracing gets only ids.
- **Cloud machines:** with `CloudDelivery::Never` (the default), nothing is sent. With `HostBound { env }`, the hub sets the secret as a boxd host-bound secret, as `repos.rs` and `plans.rs` already do. The machine only gets a placeholder, which boxd swaps for the real value on outbound requests. GitHub's App tokens already work this way. boxd's own key never goes to a machine.
- Every external call has a timeout: 10 seconds for a health check by default, set per call for tools, and 20 seconds for `gh`, as the REPO tab does today.

## GitHub and boxd on the model

Both live in `aura-engine-integrations/src/builtin/` and keep working exactly as today. The model only reads what is already there, and adds connect, test and disconnect.

- **GitHub** (`AuthKind::CliLogin { gh auth login }`, plus a token):
  - **Status** comes from `gh auth status`, with a 20-second timeout; else `GH_TOKEN` or `GITHUB_TOKEN`; else a token stored from the panel. It also notes when the hub has a GitHub App configured for cloud pushes. The status says which source is in use.
  - **Test** calls `GET https://api.github.com/user`.
  - **Connect** with a token stores it sealed. The REPO tab's `Api` reads the stored token after the env vars: one small change in `repo/github.rs`, after checking with RUS207.
  - **Disconnect** removes only a stored token. An env token or a `gh` login says how to remove it.
  - It has no agent tools yet, because git tools and `git_ship` stay as they are.
- **boxd** (`AuthKind::ApiKey { env: [BOXD_TOKEN, BOXD_API_KEY] }`):
  - **Status** comes from the cloud config: env or `.env`, else the sealed key.
  - **Test** exchanges the key for a token against `BOXD_API_URL`, reusing `aura-engine-cloud`'s client and its timeout.
  - **Connect** stores the key sealed. Cloud startup then uses `CloudConfig` with that key when the environment sets none, through a new constructor in `aura-config`.
  - **Disconnect** removes only the sealed key.
  - It has no agent tools.

**Later migrations,** which I'll list in the docs:
- `aura cloud setup` should write the sealed store instead of `.env`.
- The `aura-auth` vault's plain fallback file should be sealed.
- aura-os trusted integrations and plugin connectors should show in the same list.
- GitHub agent tools (PRs, issues) and an OAuth flow.

## The panel

- **Header:** `AgentsTab` gets an `Integrations` tab drawn as an icon, a plug, just left of BENCH's wrench. It uses the same icon cell, tooltip ("Integrations") and active state. This needs the single icon tab generalized to a list in both places:
  - terminal: `AgentsTab::ICON_TABS`, `header::bar_end`, `IconTab` and the stored hit areas;
  - desktop: `chrome` `icon_tabs: Vec`, plus a new `plug` glyph in `glyphs.rs`.

  HOST's icon on the side panel goes through the same list, and the Alt+digit and Left/Right order gets the new tab. I'll check this with FEN6 first.
- **Body:** a new file, `renderer/agents/integrations.rs`. Each integration is a row with a colored dot in RUS206's style (connected in the success color, not connected muted, failed in the error color, testing in the warning color), then its icon, its name, and what it provides. Selecting a row expands its status line and the actions Connect, Configure, Test and Disconnect, as clickable spans and keys `c` `e` `t` `d`. Connect for an API key, and Configure, open the existing modal input, masked for secrets. Connect for a CLI login shows the command to run.
- **Wiring:** `UiCommand::SetIntegrations { views }` and `UiEvent::Integration { id, action }`, with views in `aura-ui/src/events/views/integrations.rs`. In aura-app, a new `event_loop/integrations.rs` owns the `Registry`. It builds the registry at startup with the local store key, answers actions off the event loop, and publishes views. It also registers integration tools in `harness/kernel.rs` next to the web tools.
- The desktop draws the same panel body, and only its header icons are its own.
- `crates/ui/aura-ui-terminal/STYLEGUIDE.md` gets the header diagram with both icons and the panel layout.

## How it lands

My machine can only push `aura/fewer-merge-conflicts-rus204`, so instead of a fresh branch per PR, I'll reset that branch to the latest `origin/main` before each one. Each PR's commits then descend from the merged previous tip, and the push needs no force. Each PR merges with `gh pr merge --merge --match-head-commit <sha>` right after bringing in main.

1. **PR A, the core model (small, first, for RUS205):**
   - the crate with the types, trait, registry, access, the sealed `SecretStore`, the tool adapter and its naming, and the mode-gate hook;
   - `docs/integrations.md`, with the inventory above, the design and "How to add an integration";
   - a fragment.

   Tests: registry duplicates and listing; secrets never in plain text (the sealed file's bytes hold no secret and `is_sealed` is true; `Debug` is redacted; a wrong key fails); health timeout and errors; connect stores only after a passing health check; tool exposure filtered by connection and access; adapter names and dispatch.
2. **PR B, GitHub, boxd and the app wiring:** the two built-ins with fake-`gh`, fake-HTTP and env tests; the registry service in aura-app; tool registration on kernels; the REPO tab reading the stored token; boxd's sealed-key fallback. Tests: both listed through the registry with the right status, and an app test that a connected fake integration's tool is on the kernel and a disallowed agent gets none.
3. **PR C, the UI:** the generalized header icon tabs in the terminal and desktop, the plug glyph, the panel, actions and modal, STYLEGUIDE and a fragment. Tests: the icon sits right before BENCH with a tooltip and an active state, clicks map to the tab, and the panel renders connected and not-connected rows with dot colors and actions; also the desktop chrome icon boxes.

Before each PR: fmt, clippy on the changed crates (touched files clean), `scripts/check_file_size.sh` for new files, and tests for every changed crate.

## What could go wrong

- If RUS205 needs more from the context, for example long polls or per-call config, the interface changes in PR A before it lands. That's why it goes first.
- Generalizing the header could collide with FEN6 or RUS206. I've asked both which functions they're editing, and I'll merge main in right before PR C.
- The boxd key fallback touches `aura-config`. If it gets invasive, boxd's Connect will tell the user to run `aura cloud setup`, and I'll record the sealed-key path as a migration instead.
