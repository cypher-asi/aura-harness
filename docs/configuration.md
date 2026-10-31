# Configuration

The node reads configuration from environment variables via `NodeConfig::from_env()` in [`crates/aura-runtime/src/config/mod.rs`](../crates/aura-runtime/src/config/mod.rs); agent and fleet settings come from [`aura-config`](../crates/aura-config). Copy [`.env.example`](../.env.example) as a starting point.

## LLM routing

All cloud LLM traffic goes through the AURA router (proxy) with a per-request JWT. There is no direct-provider path: `aura-harness` never calls Anthropic or any other provider itself. The terminal UI can also send `local/*` models to a local Anthropic-compatible server (Ollama); those requests never carry the JWT. Headless `aura-node` is router-only.

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_LOCAL_MODEL` | `qwen3:4b` | Default local model for the terminal UI (sent to Ollama as-is; selected as `local/<name>`). |
| `AURA_LOCAL_MODEL_URL` | `http://127.0.0.1:11434` | Local Anthropic-compatible server (Ollama 0.14+). |
| `AURA_LOCAL_CONTEXT_TOKENS` | `32768` | Context window assumed for local models. Start Ollama with a matching `OLLAMA_CONTEXT_LENGTH`. |
| `AURA_ROUTER_URL` | `https://aura-router.onrender.com` | Proxy router endpoint. |
| `AURA_ROUTER_JWT` | — | JWT for terminal/CLI sessions (or run `aura login`). WebSocket clients supply their own. |
| `AURA_DEFAULT_MODEL` | `claude-opus-4-6` | Model sent to the router only when the request did not pin one. In the terminal UI, setting it overrides the local default and the saved `/model` choice. Legacy `AURA_ANTHROPIC_MODEL` is still read as a fallback. |
| `AURA_MEMORY_MODEL` | `aura-claude-haiku-4-5` | Low-cost model for background memory extraction and consolidation, independent of the chat model. |
| `AURA_DEFAULT_FALLBACK_MODEL` | — | Optional secondary model used on 429/529 retries. |
| `AURA_MODEL_TIMEOUT_MS` | `300000` | LLM request timeout. |
| `AURA_GENERATION_STREAM_IDLE_TIMEOUT_SECS` | `300` | Maximum quiet window while proxying generation SSE from the router before emitting `STREAM_IDLE_TIMEOUT`. |
| `AURA_LLM_MAX_RETRIES` | `8` | Per-model retry budget before falling back. |
| `AURA_DISABLE_PROMPT_CACHING` | — | `1` / `true` / `yes` disables prompt-caching directives. |

Further optional tuning (read by `AnthropicConfig::from_env`): `AURA_LLM_BACKOFF_INITIAL_MS` (`250`), `AURA_LLM_BACKOFF_CAP_MS` (`30000`), `AURA_LLM_MIN_REQUEST_INTERVAL_MS` (`0` = disabled), `AURA_LLM_EMERGENCY_BODY_CAP_BYTES` (`524288`; `0` disables), `AURA_LLM_CLOUDFLARE_MAX_RETRIES` (`3`).

## Agent mode

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_FLEET_DEFAULT_MODE` | `agent` | Session mode when the request (or `--mode`) does not set one: `agent`, `plan`, `ask` or `debug`. A non-`agent` default makes dev-loop and task-run requests fail with `400` unless they pass `mode: "agent"`. |

## Node runtime

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_DATA_DIR` (alias `DATA_DIR`) | OS local app data `aura/node`; `./aura_data` fallback | Data directory for RocksDB and workspaces. |
| `AURA_LISTEN_ADDR` (alias `BIND_ADDR`) | `127.0.0.1:8080` | HTTP bind address. |
| `SYNC_WRITES` | `false` | Sync RocksDB writes (`true` / `1`). |
| `RECORD_WINDOW_SIZE` | `50` | Kernel context record window. |
| `AURA_PROJECT_BASE` | — | Remaps incoming project paths to `{base}/{slug}` (remote VM mode). |
| `ORBIT_URL` | `https://orbit-sfvu.onrender.com` | Orbit service URL. |
| `AURA_STORAGE_URL` | `https://aura-storage.onrender.com` | Aura Storage service URL. |
| `AURA_NETWORK_URL` | `https://aura-network.onrender.com` | Aura Network service URL. |
| `AURA_OS_SERVER_URL` (alias `AURA_SERVER_BASE_URL`) | `http://127.0.0.1:19847` on loopback binds, else unset | Routes spec/task/project/log writes (and the cross-agent `send_to_agent` hook) through `aura-os-server`. |
| `AURA_ALLOW_UNRESTRICTED_FULL_ACCESS` | `false` | Operator ceiling letting FullAccess sessions bypass command allowlists. Mirrors `--allow-unrestricted-full-access`. |
| `AURA_NODE_REQUIRE_AUTH` | `false` | Enable the bearer-token gate (see below). |
| `AURA_NODE_AUTH_TOKEN` | — | Shared-secret bearer token used when auth is on. |

## Authentication

By default (`AURA_NODE_REQUIRE_AUTH` unset or `0`) the node accepts requests without an `Authorization` header. That suits a loopback-bound local setup.

Set `AURA_NODE_REQUIRE_AUTH=1` to require a shared-secret bearer token on every route except `/health` (missing or wrong tokens get `401`). The node then:

1. Resolves the token from `AURA_NODE_AUTH_TOKEN`, then `$AURA_DATA_DIR/auth_token` (mode `0600` on Unix), then mints a 32-byte hex value and prints it to stderr on first run.
2. Attaches `require_bearer_mw` to the protected routes, and checks again inside `/stream/:run_id`.
3. Prints the embedded TUI API server token to stderr so a browser or `curl` can use it.

Running a non-loopback listener (`AURA_LISTEN_ADDR=0.0.0.0:...`) without auth is a deliberate trust decision; pair it with firewall or network-level controls.
