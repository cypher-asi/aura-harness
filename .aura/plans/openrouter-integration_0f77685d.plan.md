---
name: OpenRouter integration
overview: Add OpenRouter as a fourth linked provider (OAuth PKCE or pasted key, saved in the existing Vault). It serves `openrouter/<vendor>/<model>` ids through OpenRouter's Anthropic-compatible Messages endpoint. `/model` gets two tabs, "Ready" and "Sign in", plus filtering and scrolling. Reasoning blocks are tagged with where they came from so one chat can move between providers safely, and each reply footer shows which sign-in paid for its tokens.
todos:
- id: core-kind
  content: Add ProviderKind::OpenRouter and SubscriptionProvider::accept_api_key in aura-core-auth
  status: completed
- id: auth-crate
  content: Create aura-auth-openrouter crate (PKCE loopback/headless, key exchange, key check, credential headers) with fixture tests
  status: completed
  assignee: rust-engineer
- id: auth-manager
  content: Register provider in AuthManager; route pasted sk-or- keys to accept_api_key; persistence test via Vault
  status: completed
  assignee: rust-engineer
- id: catalog
  content: Add OPENROUTER_PREFIX, ModelBackend::OpenRouter, vendor-aware model_key, curated list, source_label, live model table
  status: completed
  assignee: rust-engineer
- id: listing
  content: Implement fetch_openrouter_models (tools-capable filter, context/price info) and cache it like plan_models.json
  status: completed
  assignee: rust-engineer
- id: provider
  content: Wire OpenRouter Anthropic-compatible provider into routed_provider/RoutedProvider via CredentialedProvider
  status: completed
  assignee: rust-engineer
- id: reasoning-origin
  content: Tag reasoning signatures by origin, filter foreign/unsigned blocks per provider, clean tool ids; cross-provider tests
  status: completed
  assignee: rust-engineer
- id: record-route
  content: Record requested route model id on every Reasoning entry in the kernel
  status: completed
- id: selection-gate
  content: Update ModelSelection (refresh lists, groups, limits, reasons) and Gate candidates/dedupe preference
  status: completed
  assignee: rust-engineer
- id: tui-selector
  content: 'Model selector: Ready/Sign in tabs, type-to-filter, scrolling; update desktop snapshot/demo'
  status: completed
  assignee: rust-engineer
- id: footer
  content: Per-sign-in token/cost breakdown in turn footer, record loader, and /cost
  status: completed
- id: cli-docs
  content: Update aura login/whoami wording, docs, README crate count, CHANGELOG
  status: completed
  assignee: engineer
- id: verify
  content: Run fmt, clippy -D warnings, and full test suite
  status: completed
---

# OpenRouter integration

Add OpenRouter as a fourth linked provider (OAuth PKCE or pasted key, saved in the existing Vault). It serves `openrouter/<vendor>/<model>` ids through OpenRouter's Anthropic-compatible Messages endpoint. `/model` gets two tabs, "Ready" and "Sign in", plus filtering and scrolling. Reasoning blocks are tagged with where they came from so one chat can move between providers safely, and each reply footer shows which sign-in paid for its tokens.

## Todos

- [x] Add ProviderKind::OpenRouter and SubscriptionProvider::accept_api_key in aura-core-auth
- [x] Create aura-auth-openrouter crate (PKCE loopback/headless, key exchange, key check, credential headers) with fixture tests (@rust-engineer)
- [x] Register provider in AuthManager; route pasted sk-or- keys to accept_api_key; persistence test via Vault (@rust-engineer)
- [x] Add OPENROUTER_PREFIX, ModelBackend::OpenRouter, vendor-aware model_key, curated list, source_label, live model table (@rust-engineer)
- [x] Implement fetch_openrouter_models (tools-capable filter, context/price info) and cache it like plan_models.json (@rust-engineer)
- [x] Wire OpenRouter Anthropic-compatible provider into routed_provider/RoutedProvider via CredentialedProvider (@rust-engineer)
- [x] Tag reasoning signatures by origin, filter foreign/unsigned blocks per provider, clean tool ids; cross-provider tests (@rust-engineer)
- [x] Record requested route model id on every Reasoning entry in the kernel
- [x] Update ModelSelection (refresh lists, groups, limits, reasons) and Gate candidates/dedupe preference (@rust-engineer)
- [x] Model selector: Ready/Sign in tabs, type-to-filter, scrolling; update desktop snapshot/demo (@rust-engineer)
- [x] Per-sign-in token/cost breakdown in turn footer, record loader, and /cost
- [x] Update aura login/whoami wording, docs, README crate count, CHANGELOG (@engineer)
- [x] Run fmt, clippy -D warnings, and full test suite

<!-- end of generated header: edit below -->

## Approach

OpenRouter fits the existing **subscription provider** pattern (Claude and ChatGPT), so most of the work is adding one more variant at each layer.

- **Auth.** OpenRouter's [OAuth PKCE](https://openrouter.ai/docs/guides/overview/auth/oauth) flow returns a user-owned API key:
  - Browser sign-in: `https://openrouter.ai/auth?callback_url=http://localhost:<port>/callback&code_challenge=…&code_challenge_method=S256&state=…`. It accepts any localhost port and echoes `state` back.
  - Headless sign-in: leave out `callback_url` and the page shows a code to paste.
  - Exchange: `POST https://openrouter.ai/api/v1/auth/keys {code, code_verifier, code_challenge_method}` returns `{key}`.
  - The key is stored like plan tokens: `access_token`, no refresh token, an expiry far in the future. The `Vault` (keyring first, file fallback) already keeps it across restarts.
- **Inference.** OpenRouter's Anthropic-compatible endpoint (`https://openrouter.ai/api/v1/messages`, bearer auth) works for every model. That lets us reuse `AnthropicProvider` with `with_sse_for_all_models()` and `without_router_body_extensions()`, wrapped in `CredentialedProvider`. That wrapper already strips the `X-Aura-*` billing/routing metadata before a request goes to a third party.
- **Routing.** The model id carries its route, as today: `openrouter/anthropic/claude-sonnet-4.6` maps to `ModelBackend::OpenRouter` and `ProviderKind::OpenRouter`. `RoutedProvider` already picks the backend per request, so switching model or sign-in in the middle of a chat is already per call. The parts that break today are cross-provider reasoning blocks and tool ids (both fixed below).

## Files to change

| Area | Files | Change |
|---|---|---|
| Core auth | `crates/core/aura-core-auth/src/provider.rs`, `traits.rs` | `ProviderKind::OpenRouter` (`openrouter`, alias `or`); add to `SUBSCRIPTIONS` and `is_subscription`; update the `UnknownProvider` message. New default method `SubscriptionProvider::accept_api_key(&self, key) -> Result<LinkGrant>` that returns `Invalid` by default |
| New crate | `crates/auth/aura-auth-openrouter/` (`lib.rs`, `provider.rs`), root `Cargo.toml` | `OpenRouterProvider: SubscriptionProvider` (details below) |
| Auth manager | `crates/auth/aura-auth/src/manager.rs`, `Cargo.toml`, `lib.rs` docs | Register the provider in `AuthManager::new()`. In `finish_link`, a pasted `sk-or-…` value goes to `accept_api_key` instead of `exchange`. Skip refresh for keys that never expire |
| Model catalog | `crates/model/aura-model/src/model_catalog.rs` | Changes listed below |
| Live list | `crates/model/aura-model/src/plan_models.rs` | `fetch_openrouter_models(resolver)`: `GET /api/v1/models`, keep models whose `supported_parameters` includes `tools`, map to `ModelOption` and `LiveModelInfo` |
| Providers | `provider_factory.rs`, `routed.rs` | `RoutedProvider::with_openrouter`. `routed_provider()` builds `AnthropicConfig::openrouter()` (`AURA_OPENROUTER_URL`, default `https://openrouter.ai/api`) |
| Reasoning origin | `anthropic/convert.rs`, `anthropic/mod.rs`, `openai_responses/mod.rs` | Tag reasoning blocks with where they came from (below) |
| Kernel record | `crates/agent/aura-agent-kernel/src/kernel/{reason.rs,stream.rs}` | Record `"route": request.model` (the full id we asked for) on every `Reasoning` entry, success or failure. Today only the model name the upstream echoes is recorded, so the sign-in that served the call is lost |
| App selection | `crates/surface/aura-app/src/model_selection.rs` | Changes listed below |
| App auth | `crates/surface/aura-app/src/event_loop/auth.rs`, `crates/surface/aura-cli/src/auth_commands.rs` | Mostly picked up through `is_subscription()`. Wording: "OpenRouter models bill your OpenRouter credits"; `/login openrouter paste` takes a code or an `sk-or-` key |
| Footer | `crates/surface/aura-app/src/turn_calls.rs`, `record_loader.rs`, `crates/ui/aura-ui/src/events.rs` | Tokens and cost per sign-in (below) |
| TUI | `crates/ui/aura-ui-terminal/src/app/{commands.rs,keys.rs,mod.rs}`, `renderer/overlays.rs`, `renderer/segments.rs`, `renderer/chat_lines.rs` | Model selector tabs, filter and scrolling; second footer line (below) |
| Desktop | `crates/surface/aura-desktop/src/{demo.rs,shell/snapshot.rs}` | Fill the new fields |
| Docs | `docs/configuration.md`, `docs/running.md`, `docs/cli.md`, `docs/architecture.md`, `README.md` (crate count), `CHANGELOG.md` | OpenRouter sign-in, env vars, tabs, footer |

### `OpenRouterProvider` details

- `redirect()`:
  - loopback: `localhost` + `/callback`.
  - manual: a sentinel redirect URI; `authorization_url` leaves out `callback_url` when it sees it.
- `client_id()`: the constant `"openrouter"`.
- `exchange()`: `POST /api/v1/auth/keys`. Map 403 to `Rejected` and network failures to `Unavailable`.
- `refresh()`: returns `ReauthRequired`.
- `plan_tier()`: best effort, from `GET /api/v1/key` (free tier, or credits left).
- `accept_api_key()`: checks the key with `GET /api/v1/key`.
- `credential()`: bearer is the key, plus the `HTTP-Referer` and `X-Title: Aura` attribution headers; no preamble.
- Tests use the axum fixture setup copied from `aura-auth-claude`.

### `model_catalog.rs` changes

- Add `OPENROUTER_PREFIX`, `ModelBackend::OpenRouter` (label `openrouter`, `required_provider` → `OpenRouter`), and branches in `backend_for` and `upstream_model_name`.
- `model_key` removes the vendor segment on OpenRouter ids: `openrouter/anthropic/claude-sonnet-4.6` becomes `claude-sonnet-4-6`. Prices, tiers, effort rules and Gate's de-duplication then apply unchanged.
- Add a small curated `OPENROUTER_MODELS` list. It is shown in the "Sign in" tab before the account is linked.
- Add `LiveModelInfo { context_window, max_output, price }`, kept in a documented process-wide table that the OpenRouter listing fills in. `max_output_tokens`, `estimate_cost_usd` and `aura_engine::model_context::context_window_for_model` read it as a fallback, so a 32k-context model is never assumed to have 200k.
- Add `ModelBackend::source_label()`: `local`, `aura-router`, `Claude plan`, `ChatGPT plan`, `OpenRouter`.

### Reasoning-origin tagging

- Thinking signatures that OpenRouter returns get the prefix `openrouter:`. Signatures already carry a provenance prefix for OpenAI reasoning (`openai-responses:`); this follows the same pattern.
- Each Anthropic-shaped provider is configured with the origin it accepts:
  - OpenRouter: keeps only `openrouter:` blocks and removes the prefix before sending.
  - Claude direct and aura-router: drop `openrouter:` blocks and unsigned blocks.
  - Local: keeps unsigned blocks.
- The ChatGPT converter already ignores signatures it doesn't recognise.
- Tool-use ids are cleaned to `^[a-zA-Z0-9_-]+$`, the same way for `tool_use` and `tool_result`.

### `model_selection.rs` changes

- `refresh_plans` loads the Claude and OpenRouter lists at the same time and caches each next to `tui.json`, so a failed load falls back to the last saved list.
- New `group_heading` for OpenRouter models: `OpenRouter · <vendor>`.
- `unusable_reason` tells the user to run `/login openrouter`.
- `ModelLimits::configure` gets an arm for the new backend.

### Footer per sign-in

- `Tally` groups calls by source, using `backend_for(route)` and falling back to the model name.
- `SetReplyModel` and `MessageData` get `sources: Vec<SourceUsage { label, tokens, cost_usd }>`, defaulting to empty so older records still load.
- `/cost` shows the same breakdown.

### Model selector

- Two tabs:
  - **Ready (n)**: Gate plus the models whose `available` flag is true.
  - **Sign in (m)**: the rest, grouped by provider and marked `/login <provider>`. Picking one still starts the sign-in and switches to the model when it succeeds.
- Tab / ←/→ switch tabs.
- Typing filters by id or label. Backspace edits the filter; Esc clears it first, then closes.
- The list scrolls with the cursor, which is needed with hundreds of OpenRouter models.

### Gate

Gate does not route across the full OpenRouter catalog. It only considers OpenRouter models whose `model_key` is in the curated `PRICES` table, so their tier and price are known. When the same model is reachable more than one way, it prefers the linked plan, then aura-router, then OpenRouter. The change goes in `aura-agent-gate/src/router.rs` (`candidates`, `is_plan`).

## Footer example

```text
[claude-sonnet-4-6 ×2 · gate: claude-haiku-4.5 · 27.9k tok · $0.040]
[via Claude plan 25.1k tok · OpenRouter 2.8k tok $0.004]
```

## Risks

- **Process-wide live-model table.** This is shared global state, which the project rules discourage. It is written in one place (`ModelSelection` after a listing), read only as a fallback by pure lookups, and its invariants are documented in the module. Tests use a scoped guard. Without it, unknown OpenRouter models would get wrong context windows and the agent would compact too late.
- **Compatibility of OpenRouter's Anthropic endpoint.** OpenRouter only guarantees it for Anthropic upstreams. Other vendors may handle tools or thinking differently. Unsigned reasoning is dropped as described above, and a 400 from a model drops it from Gate (existing `drop_model`) with a clear message.
- **Revoked keys.** The key has no expiry we can read, so a revoked key only shows up as a 401 at inference time. The message points to `/login openrouter`, and `/whoami` checks `/api/v1/key`.
- **Exhaustive matches.** Adding enum variants breaks every exhaustive `match`. The compiler finds them; the main ones are in `model_selection.rs`, `aura-agent-gate`, `routed.rs` and `model_catalog.rs`.
- **Selector keys.** j/k/q stop being shortcuts in the selector because typed letters now filter.

## Tests

- `aura-auth-openrouter`: authorization URL with and without the callback, code exchange, API-key check, and that `Debug` output hides secrets.
- `AuthManager`: link, then a new manager on the same `Vault` still sees it; unlink.
- `model_catalog`: `backend_for`, `upstream_model_name`, `model_key` on OpenRouter ids, and lookups that fall back to the live table.
- `routed.rs`: OpenRouter requests reach the OpenRouter backend with the key and without `X-Aura-*` headers, against a mock server.
- Reasoning origin: Claude to OpenRouter to Claude in one conversation never sends a foreign signature.
- `turn_calls`: the per-source breakdown, including records written before `route` existed.
- TUI: tab switching, filtering, scrolling, choosing a model from the "Sign in" tab, and the two-line footer.
- Gate: candidates with OpenRouter linked, and which route wins for a duplicate model.
- Gates: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all --all-features`.
