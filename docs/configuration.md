# Configuration

The node reads configuration from environment variables via `NodeConfig::from_env()` in [`crates/surface/aura-node/src/config/mod.rs`](../crates/surface/aura-node/src/config/mod.rs); agent and subagent settings come from [`aura-config`](../crates/config/aura-config). Copy [`.env.example`](../.env.example) as a starting point.

## LLM routing

All cloud LLM traffic goes through the AURA router (proxy) with a per-request JWT. There is no direct-provider path: `aura-harness` never calls Anthropic or any other provider itself. The terminal UI can also send `local/*` models to a local Anthropic-compatible server (Ollama); those requests never carry the JWT. Headless `aura-node` is router-only.

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_LOCAL_MODEL` | `qwen3:4b` | Default local model for the terminal UI (sent to Ollama as-is; selected as `local/<name>`). |
| `AURA_LOCAL_MODEL_URL` | `http://127.0.0.1:11434` | Local Anthropic-compatible server (Ollama 0.14+). |
| `AURA_LOCAL_CONTEXT_TOKENS` | `32768` | Context window assumed for local models. Start Ollama with a matching `OLLAMA_CONTEXT_LENGTH`. |
| `AURA_ROUTER_URL` | `https://aura-router.onrender.com` | Proxy router endpoint. |
| `AURA_ROUTER_JWT` | — | JWT for terminal/CLI sessions (or run `aura login`). WebSocket clients supply their own. |
| `AURA_DEFAULT_MODEL` | `claude-opus-4-6` | Model sent to the router only when the request did not pin one. In the terminal UI, setting it overrides `gate` and the saved `/model` choice. Legacy `AURA_ANTHROPIC_MODEL` is still read as a fallback. |
| `AURA_MEMORY_MODEL` | `aura-claude-haiku-4-5` | Low-cost model for background memory extraction and consolidation, independent of the chat model. |
| `AURA_TITLE_MODEL` | Haiku on the chat model's backend | Model that titles an agent's chat after its first turn, shown in the agents list instead of a placeholder name like `Agent 2`. Defaults to `aura-claude-haiku-4-5` for aura-router models and `claude/claude-haiku-4-5` with a linked Claude plan; local and ChatGPT-plan chats use their own model. |
| `AURA_DEFAULT_FALLBACK_MODEL` | — | Optional secondary model used on 429/529 retries. |
| `AURA_MODEL_TIMEOUT_MS` | `300000` | LLM request timeout. |
| `AURA_GENERATION_STREAM_IDLE_TIMEOUT_SECS` | `300` | Maximum quiet window while proxying generation SSE from the router before emitting `STREAM_IDLE_TIMEOUT`. |
| `AURA_LLM_MAX_RETRIES` | `8` | Per-model retry budget before falling back. |
| `AURA_DISABLE_PROMPT_CACHING` | — | `1` / `true` / `yes` disables prompt-caching directives. |
| `AURA_PROMPT_CACHE_TTL` | `5m` | `1h` keeps Anthropic prompt-cache entries for an hour instead of five minutes, so a session that pauses between turns still gets cache hits. Writes cost more than with the 5-minute cache. Read once at startup. |

Further optional tuning (read by `AnthropicConfig::from_env`): `AURA_LLM_BACKOFF_INITIAL_MS` (`250`), `AURA_LLM_BACKOFF_CAP_MS` (`30000`), `AURA_LLM_MIN_REQUEST_INTERVAL_MS` (`0` = disabled), `AURA_LLM_EMERGENCY_BODY_CAP_BYTES` (`524288`; `0` disables), `AURA_LLM_CLOUDFLARE_MAX_RETRIES` (`3`).

## Agent mode

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_DEFAULT_MODE` | `agent` | Session mode when the request (or `--mode`) does not set one: `agent`, `plan`, `ask` or `debug`. A non-`agent` default makes dev-loop and task-run requests fail with `400` unless they pass `mode: "agent"`. The older name `AURA_FLEET_DEFAULT_MODE` is still read when the new one is unset. |
| `AURA_TRACE` | `decisions` in the terminal UI; unset elsewhere | Run-trace capture level: `off`, `decisions` or `full`. The terminal UI records every conversation unless set to `off`; the gateway and automaton tasks record only when it is set. See [run-traces.md](run-traces.md). |
| `AURA_SUBAGENT_MAX_RUNNING` | `32` | Most spawned agents (from `task` or `spawn_agent`) that may run at once across the node. A spawn beyond the limit fails with an error the parent agent sees. |

## Terminal long runs

A terminal turn has no iteration cap and no time limit: it runs until the agent is done, you cancel it, or one of these limits stops it.

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_TUI_TURN_TIMEOUT` | — (no limit) | Stop a turn that runs longer than this: `90s`, `45m`, `2h`, or a bare number of minutes. |
| `AURA_TUI_MAX_TURN_COST_USD` | — (no limit) | Stop a turn once its model calls cost more than this many US dollars at API list prices. |
| `AURA_COMPACTION_TRIGGER` | `0.70` | Context use (0.0–1.0) at which the terminal starts compacting history. Below it the transcript is left untouched so its prompt-cache prefix keeps matching. |
| `AURA_TUI_MODE` | `AURA_DEFAULT_MODE` | Work mode the terminal starts in when `--mode` is not given: `agent`, `plan`, `read-only` or `debug`. The removed names `default`, `accept-edits` and `unrestricted` are ignored with a warning; use `AURA_TUI_APPROVAL` for how much asks. `aura -p` does not read it. |
| `AURA_TUI_APPROVAL` | `yolo` | Approval level the terminal and desktop start at when `--approval` is not given: `yolo` (nothing asks; commands write only inside the folder), `ask` or `strict`. `aura -p` does not read it. |
| `AURA_YOLO_WRITABLE_PATHS` | — | Extra folders, separated like `PATH`, that `yolo` commands may write to besides the working folder, temp folders and package caches. |
| `AURA_TUI_APPROVAL_TIMEOUT_SECS` | `600` | Seconds an approval prompt waits for an answer. When it runs out, the tool call is denied and the turn goes on. |
| `AURA_TUI_NOTIFY_AFTER_SECS` | `10` | A finished turn that took at least this many seconds rings the bell (and sends a desktop notification if those are on). `0` alerts after every turn. |
| `AURA_TUI_BELL` | `1` | Set to `0` to stop the terminal bell for finished turns and approval prompts. |
| `AURA_TUI_DESKTOP_NOTIFY` | `0` | Set to `1` to also send desktop notifications, through `notify-send` on Linux or `osascript` on macOS. A notifier that hangs is killed after 5 seconds. |
| `AURA_CHECK_COMMAND` | — (from `AGENTS.md`) | Command run after each step in which the agent edited files; a failure is given to the agent. Overrides any `check:` line or `check` block in the project's `AGENTS.md`. `off` disables the check entirely. |
| `AURA_CHECK_TIMEOUT` | `120` | Time limit for one check run: seconds, or `90s` / `5m`. At most 30 minutes. |
| `AURA_WEB_FETCH_ALLOW_PRIVATE` | `false` | Let `web_fetch` read `localhost` and private-network addresses (e.g. a local dev server). Redirects are checked the same way. |
| `AURA_WEB_SEARCH_PROVIDER` | first configured | `brave`, `tavily`, `searxng` or `off`. Without it, `web_search` uses the first provider whose variable below is set. |
| `BRAVE_API_KEY` (alias `BRAVE_SEARCH_API_KEY`) | — | Brave Search API key for `web_search`. |
| `TAVILY_API_KEY` | — | Tavily API key for `web_search`. |
| `AURA_SEARXNG_URL` | — | Root URL of a SearXNG instance for `web_search`, e.g. `http://localhost:8888`. Its `settings.yml` must list `json` under `search.formats`. |

Web requests time out after 20 seconds (10 seconds to connect). Without a search provider, `web_search` is still offered and answers with what to set.

## Project instructions

Terminal agents load `AGENTS.md`, `CLAUDE.md` and Cursor rules from their working folder and the folders above it, plus `AGENTS.md` in `AURA_HOME` (default `~/.aura/AGENTS.md`) for instructions that apply to every project. See [Project instructions](cli.md#project-instructions).

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_PROJECT_INSTRUCTIONS` | on | Set to `off`, `0`, `false` or `no` to stop loading project instruction files into the terminal's system prompt. |

## Terminal editor

Ctrl+G in the input, and editing a permanent agent, open the user's editor.

| Variable | Default | Description |
|----------|---------|-------------|
| `VISUAL` | — | Editor command, tried first. It may carry arguments, e.g. `code --wait`. |
| `EDITOR` | — | Editor command used when `VISUAL` is unset or empty. With neither set, the terminal runs `vi`. |
| `TMPDIR` | system temp folder | Where Ctrl+G writes the input for the editor (on Unix, readable only by you). The file is removed when the editor exits. |

## `aura -p` runs

[`aura -p`](cli.md#aura--p-one-prompt-without-the-terminal) reads the variables above too. There, `AURA_TUI_TURN_TIMEOUT` and `AURA_TUI_MAX_TURN_COST_USD` are the defaults for `--max-time` and `--max-cost-usd` and limit the whole run rather than each turn.

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_DATA_DIR` | `./aura_data` in the working directory | Data directory: agent list, record store and session checkpoints. A running terminal holds the store's lock, so give `aura -p` its own directory to run both at once. |
| `AURA_HEADLESS_JOB_WAIT` | `10m` | Longest wait for the agent's background jobs after it ends its turn (`90s`, `45m`, `2h`, or a bare number of minutes). Jobs still running then are stopped. |
| `RUST_LOG` | warnings only | Log filter. Logs go to stderr so stdout stays machine-readable. |
| `AURA_EVALS_LIVE` | unset | Set to `1` to let [`evals/long_horizon/run.py`](../evals/long_horizon/README.md) call real models (same as its `--live` flag). |

## Gate

Read once at startup by `GateConfig::from_env` ([`crates/config/aura-config/src/gate.rs`](../crates/config/aura-config/src/gate.rs)), in the terminal, the desktop app and `aura -p`. A variable set to a value it does not accept (a target or exploration rate that is not a number from 0 to 1, a minimum that is not a whole number of at least 1, or a delegation switch that is not on or off) keeps its own default and logs a warning naming the variable, the value and what it accepts. The other Gate variables and every other `aura-config` setting still apply. See [Gate](cli.md#gate).

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_GATE_TARGET` | `0.85` | Success rate (0–1) a model needs in a category to become its pick. |
| `AURA_GATE_MIN_TRIES` | `10` | Counted outcomes a model needs in a category before its success rate is trusted. At least 1. |
| `AURA_GATE_EXPLORE` | `0.05` | Share of routed turns and subtasks (0–1) sent to the next cheaper model instead of the pick, to learn how it does. `/compare` always routes with `0`. |
| `AURA_GATE_DELEGATE` | `on` | `off` (or `0`, `false`, `no`) lets the lead agent under Gate keep its file tools and every command, with delegating only recommended. `on` (or `1`, `true`, `yes`) enforces delegation. Any other value keeps delegation enforced, with a warning. |

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
