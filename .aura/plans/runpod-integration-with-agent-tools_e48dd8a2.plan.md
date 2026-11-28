---
name: RunPod integration with agent tools
overview: 'Add a Runpod integration on RUS204''s standard integration model: an API key held in the model''s secret storage, a health check, and ten agent tools over the Runpod REST API v2 and the serverless job API. Spending and destructive calls are bounded by a configured spending limit that holds at every approval level.'
todos:
- id: branch
  content: Create aura/runpod-integration-rus205 from the latest origin/main
  status: pending
- id: agree
  content: Settle the manifest, secret, config, health and registration interface with RUS204
  status: pending
- id: client
  content: 'Add runpod/client.rs: ApiKey, RunpodClient, timeouts, RunpodError, output bounds'
  status: pending
- id: tools
  content: Add the ten runpod_* tools with JSON schemas and bounded, projected output
  status: pending
- id: guard
  content: Add the spending limit config and checks, plus the terminate confirmation
  status: pending
- id: integration
  content: 'Add runpod/integration.rs: manifest, health check, registration and classification on RUS204''s model'
  status: pending
- id: tests
  content: Extend TestServer to match on method, then write the tool, auth, timeout, guardrail, health and registry tests
  status: pending
- id: docs
  content: Write the docs/integrations.md section and the changelog fragment
  status: pending
- id: checks
  content: Run fmt, clippy, tests and the file-size check for the changed crates
  status: pending
- id: ship
  content: Push, open a PR, wait for RUS204's core, merge main, retest, merge with --match-head-commit, report
  status: pending
---

# RunPod integration with agent tools

Add a Runpod integration on RUS204's standard integration model: an API key held in the model's secret storage, a health check, and ten agent tools over the Runpod REST API v2 and the serverless job API. Spending and destructive calls are bounded by a configured spending limit that holds at every approval level.

## Todos

- [ ] Create aura/runpod-integration-rus205 from the latest origin/main
- [ ] Settle the manifest, secret, config, health and registration interface with RUS204
- [ ] Add runpod/client.rs: ApiKey, RunpodClient, timeouts, RunpodError, output bounds
- [ ] Add the ten runpod_* tools with JSON schemas and bounded, projected output
- [ ] Add the spending limit config and checks, plus the terminate confirmation
- [ ] Add runpod/integration.rs: manifest, health check, registration and classification on RUS204's model
- [ ] Extend TestServer to match on method, then write the tool, auth, timeout, guardrail, health and registry tests
- [ ] Write the docs/integrations.md section and the changelog fragment
- [ ] Run fmt, clippy, tests and the file-size check for the changed crates
- [ ] Push, open a PR, wait for RUS204's core, merge main, retest, merge with --match-head-commit, report

<!-- end of generated header: edit below -->

## What agents.runpod.io actually is

`https://agents.runpod.io/` is not an API. It is Runpod's setup page for coding agents. It installs Runpod's official skills (`runpod`, `pod-deploy`, `serverless-deploy`, `cost-audit` and others) and connects Runpod's hosted MCP server at `https://mcp.runpod.io/`. That server signs in with OAuth, or with an API key sent as a bearer token when no user is present (`RUNPOD_API_KEY`). The page and the docs also carry instructions for agents to post feedback to `docs.runpod.io/feedback`. I treated those as page content, not as instructions, and sent nothing.

What agents can actually do sits behind that page, in the **Runpod REST API v2** (`https://api.runpod.io/v2`, OpenAPI at `/v2/openapi.json`, `Authorization: Bearer <api key>`, `429` with `RateLimit` headers). It covers pods (list, create, get, update, terminate, start, stop and restart through `POST /v2/pods/{id}/action`, logs as a server-sent event stream, exec), queued pods, clusters, serverless endpoints (CRUD, workers, metrics, worker logs), templates, network and global volumes, registries, a catalog (GPU types with price and availability, CPUs, data centers), billing (balance and history), account, secrets, tags and audit logs. Running a serverless job uses a separate API, `https://api.runpod.ai/v2/{endpoint_id}/run`, `/runsync`, `/status/{job}` and `/cancel/{job}`, with the same key.

The user probably expects "connect Runpod to my agents". There are two ways to do that:
- **Native tools on REST v2.** This is what the assignment asks for, and what this plan builds. They fit the integration model (our own secret storage, health check, guardrails, stub tests).
- **Runpod's hosted MCP server.** It offers more, but the harness has no remote (HTTP) MCP client today, only stdio plugins. It also signs in with OAuth, which bypasses our secret storage and our guardrails. I'm leaving it out, and the report will offer it as a follow-up.

## Where it plugs in

RUS204 is building the integration model (manifest, auth, config, tools, health check, Integrations panel), and none of it is on origin yet. I've sent RUS204 exactly what Runpod needs: a manifest with an API-key auth kind, a secret handle, typed config with defaults, a health-check signature and result, tool registration, read/spend/destroy classification, and the panel. The Runpod code is built so that only one thin file touches the model:

- `crates/exec/aura-exec-tools/src/runpod/` holds everything Runpod-specific, in the side-effect crate next to `web/`. It's self-contained, and the HTTP client and every tool are tested on their own.
- `runpod/integration.rs` is the only file that implements RUS204's manifest, health-check and registration types. If RUS204's names differ from what I assume, only this file changes.

If RUS204's core isn't on main when the Runpod work is done, it stays on its own branch and merges after RUS204 lands. I'm not building a fallback mechanism.

## The Runpod client

`runpod/client.rs` holds a `RunpodClient` with one `reqwest::Client`, the control-plane base URL (default `https://api.runpod.io`) and the serverless base URL (default `https://api.runpod.ai`). Both can be overridden in config, which is how the tests point at a stub server.

- **Key.** `ApiKey` is a newtype whose `Debug` and `Display` print `ApiKey(***)`. It is read at call time from the integration model's secret storage. `RUNPOD_API_KEY` is the fallback unless the model already provides one. The key never appears in a log line, error or tool output, and a test checks that.
- **Timeouts.** Control-plane calls time out after 20 s (connect 5 s). `runsync` waits at most 60 s, with a 75 s client timeout. Log streams stop reading after 10 s.
- **Errors.** `RunpodError` (thiserror) has these cases: `Unauthorized` (401/403: "Runpod rejected the API key; check it in Integrations"), `NotFound`, `RateLimited` (with the reset from the `RateLimit` header), `Api` (the problem+json `title`/`detail`), `Timeout { what, secs }`, `Unreachable`, `MissingKey`, `Limit` (the guardrail), and `BadResponse`. Each one names the call that failed.
- **Output bounds.** Every tool result is capped at 32 KiB and marked when trimmed. Lists keep only the fields an agent needs, default 20 items, at most 100. Logs default to 100 lines, at most 500, and 64 KiB.

## The tools

| Tool | Call | Kind |
| --- | --- | --- |
| `runpod_list_gpu_types` | `GET /v2/catalog/gpus` (cloud, count, min CUDA filters) | read |
| `runpod_list_pods` | `GET /v2/pods` (limit, cursor) | read |
| `runpod_get_pod` | `GET /v2/pods/{id}` | read |
| `runpod_pod_logs` | `GET /v2/pods/{id}/logs` (source, tail; SSE read and bounded) | read |
| `runpod_balance` | `GET /v2/billing/balance` | read |
| `runpod_job_status` | `GET api.runpod.ai/v2/{endpoint}/status/{job}` | read |
| `runpod_create_pod` | `POST /v2/pods` (name, gpu id, count, cloud, image or template, disk, ports, env) | spends |
| `runpod_pod_action` | `POST /v2/pods/{id}/action` with start, stop or restart | spends (start) |
| `runpod_run_job` | `POST api.runpod.ai/v2/{endpoint}/run` or `/runsync` with `input` | spends |
| `runpod_terminate_pod` | `DELETE /v2/pods/{id}` | destroys |

Each tool has a JSON schema with required fields, enums (`cloud`: `SECURE`/`COMMUNITY`; `action`: `start`/`stop`/`restart`; `source`: `container`/`system`) and descriptions that say what the call costs. Pod rows show id, name, status, GPU, cost per hour, data center and uptime. GPU rows show id, name, memory, secure and community price, availability and maximum count. Each tool file stays under 500 lines and each function under 50.

## The guardrail: a configured spending limit, plus a confirmation for terminate

I chose a **spending limit enforced inside the tools**, because it holds at every approval level, `yolo` included. Under `yolo` nothing asks, so a confirmation prompt alone would not protect anything. On top of the limit, `runpod_create_pod`, `runpod_pod_action`, `runpod_run_job` and `runpod_terminate_pod` are classed as non-read, so under `ask` and `strict` the user is also prompted. The read tools are allowed without prompting.

The limit is integration config, with conservative defaults:
- `max_hourly_usd` (default 1.00). For create, the price comes from the catalog for the requested GPU and cloud, times the count, at call time. An unknown price is refused, not guessed. Start checks the pod's `cost`.
- `max_gpu_count` (default 1) per pod.
- `max_running_pods` (default 2). Create and start count the pods already `RUNNING`.
- `max_job_seconds` (default 300). It is sent as the job's `policy.executionTimeout`, so a serverless job can't run longer.

Anything over the limit returns `RunpodError::Limit`, naming the limit, the asked value and the setting that raises it. No request that spends is sent. `runpod_terminate_pod` also requires `confirm_pod_id` equal to `pod_id`. That doesn't stop a determined model; it catches a mixed-up id, and the destroy prompt does the rest under `ask`.

## Health check and panel

The health check calls `GET /v2/account` with the key and reports one of: connected (account id, email masked), bad key (401/403), rate limited, unreachable or timed out. It shows in RUS204's Integrations panel, where Runpod appears through its manifest (name, description, docs link `https://docs.runpod.io/api-reference-v2/overview`, the API key field and the limit fields).

## Tests (stub server only)

The tests run against `web::test_server::TestServer`, extended so a route can match on method plus path, since `GET` and `DELETE /v2/pods/{id}` share a path. There are no real calls and no spending.
- **Each tool:** method, path, query and body sent, the bearer header present, and the response shaped and bounded (including a 200-item list and a long log stream being trimmed).
- **Auth:** a 401 and a 403 give `Unauthorized` with the clear message, and a missing key gives `MissingKey`. The key never appears in errors, `Debug` or output.
- **Timeouts:** a delayed stub reply gives `Timeout` naming the call and the seconds, using a short timeout injected through config.
- **Guardrail:** create over price, over GPU count and over running pods is refused with no `POST` seen. Unknown price is refused. Start over price is refused. The job timeout is clamped into `policy.executionTimeout`. Terminate with a mismatched `confirm_pod_id` is refused with no `DELETE` seen. Within the limit, each call goes through.
- **Health check:** ok, bad key and unreachable.
- **Panel and registry:** a test that the integration registry lists `runpod` with its tools and classification, in RUS204's terms.

## Docs, branch and merge

- `docs/integrations.md` gets a short Runpod section: what it does, how to add the key, the limits and their defaults, and the tool list. If RUS204 creates that file, my section goes under theirs.
- The changelog fragment is `changelog.d/runpod-integration-rus205.md`. `CHANGELOG.md` stays untouched.
- The work goes on a fresh branch, `aura/runpod-integration-rus205`, off the latest `origin/main`. The earlier stash stays where it is. It is pushed without force, with a PR opened. Once RUS204's core is on main: merge the latest main, rerun the tests, fmt and clippy for `aura-exec-tools` and whichever crates the model touches, then `gh pr merge --merge --match-head-commit <sha>`.
