# Configuration

The node reads configuration from environment variables via `NodeConfig::from_env()` in [`crates/surface/aura-node/src/config/mod.rs`](../crates/surface/aura-node/src/config/mod.rs); agent and subagent settings come from [`aura-config`](../crates/config/aura-config). Copy [`.env.example`](../.env.example) as a starting point.

## LLM routing

Cloud LLM traffic billed to the zOS account goes through the AURA router (proxy) with a per-request JWT. The terminal UI also calls providers directly for models you link yourself: `claude/*` and `chatgpt/*` with a linked plan, and `openrouter/*` with a linked OpenRouter account, each with your own credentials. The JWT is never sent to them. `local/*` models go to a local Anthropic-compatible server (Ollama); those requests never carry the JWT either. Headless `aura-node` is router-only.

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_LOCAL_MODEL` | `qwen3:4b` | Default local model for the terminal UI (sent to Ollama as-is; selected as `local/<name>`). |
| `AURA_LOCAL_MODEL_URL` | `http://127.0.0.1:11434` | Local Anthropic-compatible server (Ollama 0.14+). |
| `AURA_LOCAL_CONTEXT_TOKENS` | `32768` | Context window assumed for local models. Start Ollama with a matching `OLLAMA_CONTEXT_LENGTH`. |
| `AURA_ROUTER_URL` | `https://aura-router.onrender.com` | Proxy router endpoint. |
| `AURA_OPENROUTER_URL` | `https://openrouter.ai/api` | OpenRouter API base for `openrouter/*` models (`/v1/messages`, `/v1/models`); the sign-in page is derived from it. A value that is not HTTPS and not loopback is ignored. |
| `AURA_ROUTER_JWT` | — | JWT for terminal/CLI sessions (or run `aura login`). WebSocket clients supply their own. |
| `AURA_DEFAULT_MODEL` | `claude-opus-4-6` | Model sent to the router only when the request did not pin one. In the terminal UI, setting it overrides `gate` and the saved `/model` choice. Legacy `AURA_ANTHROPIC_MODEL` is still read as a fallback. |
| `AURA_MEMORY_MODEL` | `aura-claude-haiku-4-5` | Low-cost model for background memory extraction and consolidation, independent of the chat model. |
| `AURA_TITLE_MODEL` | Gate's classifier | Model that titles an agent's chat after its first turn, shown in the agents list instead of a placeholder name like `Agent 2`. Defaults to the model Gate classifies prompts with: the cheapest fast model you can run (Haiku on a linked Claude plan or aura-router), else the local default. |
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
| `AURA_COMPACTION_TRIGGER` | `0.70` | Context use (0.0–1.0) at which the terminal starts compacting history. Below it the transcript is left untouched so its prompt-cache prefix keeps matching. Other agent loops (the node's chat sessions, automatons, spawned agents) use the same `0.70` default (`aura_config::COMPACTION_TRIGGER_DEFAULT`). |
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

Read once at startup by `GateConfig::from_env` ([`crates/config/aura-config/src/gate/`](../crates/config/aura-config/src/gate/mod.rs)), in the terminal, the desktop app and `aura -p`. A variable set to a value it does not accept (a target or exploration rate that is not a number from 0 to 1, a minimum that is not a whole number of at least 1, or a delegation switch that is not on or off) keeps its own default and logs a warning naming the variable, the value and what it accepts. The other Gate variables and every other `aura-config` setting still apply. See [Gate](cli.md#gate).

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_GATE_TARGET` | `0.85` | Success rate (0–1) a model needs in a category to become its pick. |
| `AURA_GATE_MIN_TRIES` | `10` | Counted outcomes a model needs in a category before its success rate is trusted. At least 1. |
| `AURA_GATE_EXPLORE` | `0.05` | Share of routed turns and subtasks (0–1) sent to a cheaper model instead of the pick (the nearest one not ruled out for the category, else the nearest), to learn how it does. `/compare` always routes with `0`. |
| `AURA_GATE_DELEGATE` | `on` | Applies to the turns the agent leads under Gate (where leading has proven cheaper, or is being explored). `off` (or `0`, `false`, `no`) lets the lead keep its file tools and every command, with delegating only recommended. `on` (or `1`, `true`, `yes`) enforces delegation. Any other value keeps delegation enforced, with a warning. |
| `AURA_GATE_JUDGE` | `on` | Whether a judge model (the strongest Opus you can use) reviews routed work that failed and says whether the model was at fault; only that failure then counts against the model and moves the work to a stronger one. `off` (or `0`, `false`, `no`) makes no judge call: every failure counts and moves up. See [Gate](cli.md#gate). |
| `AURA_GATE_NUDGE` | `on` | Whether a routed turn that changed files without running a test or build is asked once, on the same model, to check its change before it ends (while a stronger model is there to move up to). `off` (or `0`, `false`, `no`) skips it. See [Gate](cli.md#gate). |
| `AURA_GATE_CHECK` | `on` | Whether the harness runs the project's tests (`AURA_DOD_TEST_COMMAND`, else the command its manifest implies) on a routed turn that changed files and still checked nothing; a failure moves the work up as a failed check of the turn's own does. `off` skips it. |
| `AURA_GATE_SUSPECT` | `on` | Whether the judge reviews a routed turn that checked nothing yet looks unfinished (verifiable work that changed no files, a reply that gives up, a turn stopped by its budget or a stall); work it finds unfinished counts against the model and moves up. Needs `AURA_GATE_JUDGE`. `off` skips it. |
| `AURA_GATE_REVIEW` | `log` | What Gate does with a review of routed work that is about to stand: work the classifier found hard, or that raises a sign the attempt log backs, on a model below the strongest tier. `log` asks the reviewer (the model that did the work, or the cheapest standard model for a fast model's work), shows its verdict and writes it to the attempt log without acting on it; `on` also sends work it finds unfinished back to the same model once, and moves it up to a stronger model if it is found unfinished again; `off` (or `0`, `false`, `no`) asks for no review. Needs `AURA_GATE_JUDGE`. See [Gate](cli.md#gate). |
| `AURA_GATE_CRITERIA` | `log` | Whether the acceptance criteria of a routed code turn are written down before the work starts, by the cheapest standard-tier model. With `log` and `on` the model doing the work is shown them, and criteria that ask for output to match exactly rate the work `hard`. `log` reviews work about to stand against them on a sample of turns and only logs the verdict; `on` reviews every such turn and sends work found short of them back to its model once, then up to a stronger one; `off` (or `0`, `false`, `no`) writes none. Needs `AURA_GATE_JUDGE`. See [Gate](cli.md#gate). |
| `AURA_GATE_REVIEW_SAMPLE` | `0.25` | Share of routed turns (0–1) whose work gets a review that is only logged, with `AURA_GATE_REVIEW` or `AURA_GATE_CRITERIA` at `log`; the others skip it. A review Gate acts on (`on`) is always asked. |
| `AURA_GATE_CASCADE` | `on` | Whether work a check verifies (`small_change`, `multi_file_change`, `debugging`) starts on the model it is expected to cost least from, counting the stronger models a failed attempt moves up to. `off` (or `0`, `false`, `no`) routes it like other work, by success rate. See [Gate](cli.md#gate). |

## Cloud agents (boxd)

Any agent can run on its own [boxd](https://boxd.sh) microVM instead of this machine: flip the switch at the right of the footer (`⌂ local` / `☁ cloud`), or type `/cloud` and `/local`. The HOST side tab (after PROCESSES) shows the host the agent on screen runs on: this machine's name, OS, addresses, CPU, memory and disk, or the cloud machine's region, size, state, addresses, usage and cost. In project scope it lists every host of the project with its cost per hour.

Settings are read from the environment, which `.env` files can set: the `.env` found from the working directory upward first, then `AURA_HOME/.env` (`~/.aura/.env`), never overriding a variable already set. `aura cloud setup` (or `/cloud setup`) signs in with the `boxd` CLI, mints a key and writes it to `AURA_HOME/.env` with mode `600`. A key added later is picked up the next time cloud mode is used; no restart. `aura cloud status` shows what is loaded, with the key redacted.

| Variable | Default | Meaning |
|---|---|---|
| `BOXD_API_KEY` | unset | boxd API key (`bxd_…`); required for cloud mode. Exchanged for a one-hour token; never sent to a machine. |
| `BOXD_TOKEN` | unset | A boxd token you already hold; wins over the key. |
| `BOXD_BASE_URL` | `https://boxd.sh:9443` | gRPC endpoint (older name: `BOXD_API_URL`). |
| `BOXD_REGION` | Frankfurt | Region for new machines (`nyc` = New York). |
| `BOXD_ORG` | key's org | Organization to act in. |
| `AURA_CLOUD_SIZE` | `4x16` | Machine size: `1x4`, `2x8`, `4x16` or `8x32` (vCPU × GiB). Falls back one size when the org quota refuses it. |
| `AURA_CLOUD_IDLE_HIBERNATE_SECS` | `300` | Idle seconds before a machine hibernates (disk-only billing, ~100 ms wake). |
| `AURA_CLOUD_IDLE_STANDBY_SECS` | `0` | Idle seconds before standby (memory kept, sub-ms wake); `0` = off. |
| `AURA_CLOUD_ISOLATED` | `1` | Create machines isolated (no in-machine boxd CLI signed in as you). |
| `AURA_CLOUD_FORK_ON_SPAWN` | `1` | Give an agent spawned by a cloud agent a fork of its parent's machine. |
| `AURA_CLOUD_PRICE_VCPU_HOUR`, `AURA_CLOUD_PRICE_MEM_GIB_HOUR`, `AURA_CLOUD_PRICE_DISK_GIB_HOUR` | boxd list prices (€0.039, €0.012, €0.00008) | Rates for the live estimate between boxd's meter readings. |

How it works: the first move of a project builds a snapshot holding the finished build, the project's git history at HEAD and a running aura node (rebuilt only when lockfiles, the toolchain or the build command change). A new agent's machine is restored from it and gets the commits made since and the uncommitted changes, never `.env` files, on a branch of its own (below). The agent's record is handed to the machine's node, which becomes its single writer; the app mirrors every entry back into the main RocksDB as the agent works, and always before its machine goes to sleep. Messages between agents on different machines travel over each node's mesh socket. `/local` (or archiving the agent) seals the node, copies the rest of the record, applies the agent's changes with `git apply --3way` (saving the patch under `AURA_HOME/cloud/<agent>/` if it doesn't apply), fetches its commits and destroys the machine. Cloud agents' turns run on aura-router models or on the owner's Claude, ChatGPT and OpenRouter plans (below); Gate routing, `/compare` and local models stay on this machine.

### Plans and several users

Every cloud agent has an **owner**: the user signed in here when it moved to the cloud (a zOS account, or this machine's local profile). Its machine spends that user's plans and only that user can move it back, pull its work, put it to sleep, wake it, archive it or send it work; anyone else signed in sees "Placed by Alice; sign in as Alice to manage it", a `☁ Alice` mark in the projects list and a locked footer switch. Agents only see and message agents of the same owner, and a spawned agent forks its parent's machine only when both have the same owner.

A machine never holds a plan token. For each owner aura keeps three boxd secrets, named `AURA_<owner>_<install>_CLAUDE`, `_CHATGPT` and `_OPENROUTER`, in the key's organization with scope `private-unattached`. Each secret is bound to its provider's host (`api.anthropic.com`, `api.openai.com`, `openrouter.ai`) and replaced in request headers only. A machine's spec lists only its owner's three names, so it holds only `bxds_…` placeholders: boxd's egress proxy puts the real token into requests to that host and nowhere else. `<owner>` is 8 hex characters hashed from the user, never their id or email.

The app refreshes every placing owner's tokens 15 minutes before they expire and rotates the secrets every 2 minutes when a token changed. Rotation keeps the placeholder, so running machines use the new token on their next request. A plan the owner has not linked holds `unlinked`, so linking it later (`/login claude`) reaches their running machines at once, and signing out of it revokes it on all of them. Picking a plan model for a cloud agent whose owner has not linked that plan shows "Sign in to Claude (`/login claude`) to use it from cloud agents". The HOST tab's MODELS section shows each plan as linked, with when its token was last synced, or not linked; its Owner fact and the project rows say whose plan and money each machine spends. To remove an owner's secrets by hand: `boxd env rm AURA_<owner>_<install>_CLAUDE` (and the other two).

Claude Pro/Max and ChatGPT plans are meant for one person's interactive use: a user's machines share that user's plan limits, and running many may conflict with the provider's terms. OpenRouter keys bill the owner's OpenRouter credits. If this machine is offline longer than a token lives (about an hour for ChatGPT), cloud turns on that plan fail until it is back.

### GitHub for cloud agents

A cloud agent works in a real clone of its project. The snapshot holds the project's history at the commit it was built from, sent as a `git bundle` rather than a copy of `.git`, so this repository's config, hooks and credentials stay here (a shallow clone can't be bundled: run `git fetch --unshallow` first). A new machine also gets the commits made since and the uncommitted changes, and starts a branch of its own, `aura/<name>-<code>`, at this repository's HEAD. A spawned agent that forks its parent's machine starts its own branch from the parent's. The agent commits with `git_commit`, pushes with `git_push` or `git_commit_push` to its own branch only (pushing any other branch is refused before git runs), and opens pull requests with `gh pr create`.

Pull (the HOST tab's Pull button or `/cloud pull`) and `/local` bring the work back in two ways. The working-tree changes the machine made since the last pull are applied to the project folder, as before. Its commits are fetched into this repository's branch of the same name, fast-forward only. A pull never moves the branch checked out here (it says to switch to another branch and pull again), and it leaves alone a branch here that has commits the agent's branch lacks.

Pushing needs a GitHub App:

1. Create a GitHub App in the organization (Settings → Developer settings → GitHub Apps → New GitHub App) with no webhook and these repository permissions: **Contents** read and write, **Pull requests** read and write, and **Workflows** read and write only if you set `AURA_GITHUB_APP_WORKFLOWS=1`.
2. Generate a private key and save the `.pem` on this machine, for example as `~/.aura/github-app.pem` with mode `600`.
3. Install the App on the repositories cloud agents may push to.
4. Set the variables below.
5. Protect `main` with a ruleset that requires a pull request and blocks force pushes and deletion, so the App's bot can only propose changes to it.

| Variable | Default | Meaning |
|---|---|---|
| `AURA_GITHUB_APP_ID` | unset | The App's client ID (`Iv23li…`) or its numeric App ID. |
| `AURA_GITHUB_APP_PRIVATE_KEY_PATH` | unset | The App's private key (PEM), read on this machine only; `~` expands. |
| `AURA_GITHUB_APP_WORKFLOWS` | `0` | Also let agents' tokens change `.github/workflows/`. Off by default, because a workflow change can read the repository's Actions secrets. |

A machine never holds the key or a token. The app signs a short-lived App JWT with the key and mints a one-hour installation token limited to the project's one repository and the permissions above. It stores the token as a boxd secret named `AURA_GH_<install>_<repo>` (`<repo>` is 8 hex characters hashed from `owner/name`), bound to `github.com` and `api.github.com` and replaced in request headers only. The machine's git credential store and `gh` config hold only the secret's `bxds_…` placeholder, and boxd's egress proxy puts the real token into requests to GitHub. Tokens are minted again 15 minutes before they expire and the secret is updated in place, so running machines keep pushing. Commits, pushes and pull requests are by `<app-slug>[bot]`.

Without the App, or when it isn't installed on the repository, the secret holds `unlinked`. The agent still commits on its branch and pulls still bring the commits back, but a push fails with a message saying the App lacks access, and the boot console shows "GitHub App not installed on owner/repo" with the install link. A project with no `origin`, or one not on GitHub, gets no secret and no remote on the machine. The HOST tab's GIT section shows the agent's branch, the tip last fetched here, the repository and the state of its token.

## Node runtime

| Variable | Default | Description |
|----------|---------|-------------|
| `AURA_DATA_DIR` (alias `DATA_DIR`) | OS local app data `aura/node`; `./aura_data` fallback | Data directory for RocksDB and workspaces. |
| `AURA_LISTEN_ADDR` (alias `BIND_ADDR`) | `127.0.0.1:8080` | HTTP bind address. |
| `SYNC_WRITES` | `false` | Sync RocksDB writes (`true` / `1`). |
| `RECORD_WINDOW_SIZE` | `50` | Kernel context record window. |
| `AURA_PROJECT_BASE` | — | Remaps incoming project paths to `{base}/{slug}` (remote VM mode). |
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
