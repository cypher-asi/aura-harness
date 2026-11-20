# CLI reference

## Binaries

| Binary | Entry point | Purpose |
|--------|-------------|---------|
| `aura` | [`src/main.rs`](../src/main.rs) | The main CLI. Opens the desktop window by default (the terminal UI with `--terminal`, or when there is no display), runs one prompt and exits with `aura -p`, runs headless with `aura run --ui none`, and hosts the account, plugin and subagent commands below. The command line lives in [`aura-cli`](../crates/surface/aura-cli); the agent session behind the terminal, the desktop window and `aura -p` lives in [`aura-app`](../crates/surface/aura-app). |
| `aura-node` | [`crates/surface/aura-node/src/main.rs`](../crates/surface/aura-node/src/main.rs) | Standalone headless server exposing the [HTTP / WebSocket API](api.md). |

## Commands

Defined in [`crates/surface/aura-cli/src/cli.rs`](../crates/surface/aura-cli/src/cli.rs).

| Command | Description |
|---------|-------------|
| `aura desktop` (default) | Run the agent in the desktop window; `--workspace`, `--demo` and the `aura run` flags below. Needs a build with the default `desktop` feature. See [running.md](running.md#desktop-app). |
| `aura run`, `aura --terminal` | Run the agent in the terminal (`--ui none`: as a headless server). Flags below. |
| `aura -p "prompt"` | Run one prompt without the terminal UI, print the result and exit. See [`aura -p`](#aura--p-one-prompt-without-the-terminal). |
| `aura login [zos\|claude\|chatgpt] [--paste]` | `zos` (default) signs in with email and password and unlocks the aura-router models. `claude` and `chatgpt` link your own Claude Pro/Max or ChatGPT plan through the browser; `--paste` uses a code you paste instead of a local redirect (SSH, headless). `/login [provider] [paste]` in the TUI does the same. |
| `aura logout [zos\|claude\|chatgpt] [--all]` | Sign out of zOS (linked plans are kept for when you sign back in), unlink one plan, or with `--all` unlink every plan and sign out. `/logout [provider]` in the TUI. |
| `aura whoami` | Show the zOS sign-in and each plan link's status. |
| `aura auth move --to-zos` | Attach plans linked while signed out to the signed-in zOS account. |
| `aura hello` | Print `Hello, World!` and exit (smoke test). |
| `aura migrate [--dry-run]` | Migrate aura state (a no-op stub today). |
| `aura plugins install <dir> [--trust]` | Install a plugin from a local directory into `AURA_HOME/plugins/`. |
| `aura plugins list` | List installed plugins and their active versions. |
| `aura plugins enable <id> [--yes\|--no]` / `disable <id>` | Toggle a plugin in `AURA_HOME/config.toml`, answering the trust prompt if required. |
| `aura runs list\|show\|diff\|label\|verify\|branch` | Inspect, compare, label and branch recorded runs in `AURA_HOME/runs/`. See [run-traces.md](run-traces.md). |
| `aura eval run [SUITE]` / `aura eval compare` | Run a task suite (default `evals/coding`) with optional `--set key=value` harness arms and a `--budget-usd` cap, and compare arms or a baseline. |
| `aura dataset build --format sft\|preference\|rl\|harness-bandit` | Export recorded runs as training data. |

## Sign-in and model routes

Each model id names the route that serves it, and the user picks it in `/model`:

| Model id | Route | Needs |
|----------|-------|-------|
| `local/<name>` | Local Ollama server | nothing |
| `aura-*` and other aliases | aura-router, billed to the zOS account | `aura login` |
| `claude/<model>` | `api.anthropic.com`, billed to your Claude plan | `aura login claude` |
| `chatgpt/<model>` | OpenAI Responses API, billed to your ChatGPT plan | `aura login chatgpt` |
| `gate` (default) | The route of the model Gate picks for the turn. See [Gate](#gate). | nothing; Gate uses what you are signed in to |

A pinned model's route never falls back to another one. Under `gate` the route can change between turns, so each turn shows `gate → <model> (<route>)` in the chat, while the status line at the bottom right shows `GATE` instead of the model. If a pinned model loses its sign-in (logout, unlink, revoked token), the terminal switches to `gate`.

Plan links belong to the identity that made them: this machine's local profile while signed out, otherwise the zOS user. Tokens are stored in the OS keyring, or in `~/.aura/auth/secrets/` (mode 0600) when no keyring is available. Link metadata is in `~/.aura/auth/index.json`.

The Claude link uses the OAuth client that Claude Code uses. Anthropic does not publish it for other apps, and its terms may not allow plan use outside Claude Code. `aura login claude` prints this warning before you sign in.

## Gate

`gate` is the default model and the first entry in `/model` ("Gate"); `--model gate` selects it for `aura -p`. `AURA_DEFAULT_MODEL` and a model you pick in `/model` still win, and a pinned model that loses its sign-in switches back to `gate`. Gate routes the turns of the terminal, the desktop app and `aura -p`, and learns which model is good enough for each kind of work.

- **Candidates.** Gate routes among the models you can use right now: installed local models, aura-router models when signed in to zOS, `claude/*` models with a Claude plan linked and `chatgpt/*` models with a ChatGPT plan linked. A model reachable on two routes is kept once, on your plan, since the plan is already paid for. Each model has a tier: `local`, `fast` (Haiku, GPT mini), `standard` (Sonnet) or `frontier` (Opus, GPT-5). The list follows your logins, logouts, links, unlinks and local installs. If a model's route refuses your sign-in mid-turn, Gate drops that model until your sign-ins change and carries on with its next pick; the refusal counts against no model.
- **Visible routes.** Each turn starts with a line such as `gate → claude-sonnet-4-6 (claude plan) · question, default`: the model, who pays for it, the prompt's category, and why Gate chose it (`default`, `proven` or `explore`). The status line at the bottom right shows `GATE` while Gate is the model, and in place of how full the context is (each pick has its own window) the number of models the session's calls went to. On a picked model the context share is of that model's window. A routed subtask shows `gate → <model> (<route>) · subtask, <category>, <reason>`.
- **Classifying.** One short call to the cheapest fast-tier model you can use (the local default when there is none) labels each prompt `question`, `small_change`, `multi_file_change`, `debugging`, `research`, `planning`, `writing` or `other`, and says whether its work looks `easy`, `medium` or `hard` (recorded on the prompt; a reply without one leaves it unknown). If the call fails, gives no label or takes more than 20 seconds, the prompt is `unclassified`. Under `gate` the turn waits for the label; on a pinned model the classifier runs beside the turn, so the turn still adds evidence without waiting.
- **Routing.** Candidates are ordered by expected cost: local models first, then the mean cost Gate has seen per turn. A model not tried yet is priced from its list price, scaled by how the tried models of its tier (else the nearest tier) cost against their list prices, so a newer, cheaper model is not ranked behind an older one only because it has no history. Gate picks the cheapest model with at least `AURA_GATE_MIN_TRIES` counted outcomes in the category and a success rate of at least `AURA_GATE_TARGET`. Otherwise, or when it is cheaper than that model, it uses the model the category starts on: the cheapest fast-tier model for `small_change` and for work the classifier found `easy`, and the cheapest standard-tier model for the rest, `planning` (whatever its difficulty) and `hard` small changes included (the next tier up when you have none of that tier). A frontier model is reached by moving up, not by default. A start model that can no longer reach `AURA_GATE_TARGET` over its first `AURA_GATE_MIN_TRIES` outcomes in a category, or is below it after them, hands the category to the next tier up. A turn model is judged on the turns it did itself: a led turn that failed may have failed in its subtasks. With probability `AURA_GATE_EXPLORE` it tries a cheaper model instead: the nearest one the category's outcomes have not ruled out, else the nearest one, so a model that was ruled out is tried again once nothing cheaper is more promising and comes back when those tries prove it. See [Gate](configuration.md#gate) for the settings.
- **Work a check verifies.** `small_change`, `multi_file_change` and `debugging` end in a test or build that passes or fails, and a failed attempt goes on with a stronger model, so Gate routes them by expected total cost instead (with `AURA_GATE_CASCADE` on, the default). An attempt on a model costs what it is expected to cost, plus, when it fails, the judge's review and what the work then costs on the cheapest model of the next tier up. The chance of failing is Gate's estimate of the model's success rate on the category: its own outcomes there, on a prior from its tier (50% local, 70% fast, 85% standard, 92% frontier, weighing as 4 outcomes) moved toward how it did on other categories. When the classifier gave a difficulty, the chance of failing is scaled by how much more or less often the model fails at that difficulty than overall, across categories: half as often for `easy` and 1.6 times as often for `hard` until it has outcomes there. Gate starts the work on the model with the lowest expected total, provided it is expected to succeed at least 60% of the time (any model of the strongest tier may start it). A subtask counts one move up, as it gets only one. So a cheap model starts the work while it keeps succeeding, and a frontier model priced below a standard one starts it too. With probability `AURA_GATE_EXPLORE` the success rates are drawn from those estimates instead of taken at their means, so a model little is known of is tried about as often as it may be the cheapest.
- **Failed turns: the judge and moving up.** A routed turn fails when it ends in a model error or with its last test or build command failing. Gate then asks a judge, the strongest Opus you can use (else the strongest frontier model), whether the model was at fault. The judge sees your request, the diff of the files the turn changed with file tools, its final reply and how it failed (the failing command and the end of its output, or the error), and answers in a few sentences shown in the chat as `Gate's judge (<model>, $<cost>): …`.
  - **The model's fault** (`judged_fail`): the failure counts against the model in Gate's table, and the turn goes on in the same conversation on the cheapest model of the next tier up, with the files as they are and what the judge found in its first message. This repeats until the turn ends well or no model is stronger. The stronger models' work is recorded with route reason `escalate` and not counted.
  - **Not the model's doing** (`judged_excused`: the environment got in the way, the check was failing anyway, the request cannot be done): the failure counts for nothing and the turn ends on the model it ran on.
  - **No verdict** (the judge's call failed, took more than 90 seconds or gave no clear answer), or `AURA_GATE_JUDGE=off`: the failure counts and the turn moves up, as if the model were at fault.

  The judge is only asked about failures, and never about a turn on a model of its own tier, so what it costs is paid only when there is something to learn. This applies to the terminal, the desktop app, `aura -p` and `aura eval run`. A turn the judge is not asked about because nothing is stronger simply ends as failed.
- **Outcomes.** A turn ends with `error` (the turn failed), `checks_failed` or `checks_passed` (the last test, build or type-check command it ran), or `none`. Later signals can judge it again: the judge's finding on a failed turn (above), and `/rewind` of the turn or the same prompt sent again (at least 90% alike) on a stronger tier, which are failures. A pick in `/compare pick` counts over everything else for the runs it rates; otherwise the latest outcome with a result counts, `judged_excused` takes the failure before it back, and `none` counts for nothing. Gate rebuilds its table from the record at startup and updates it as each outcome is written, so the next turn already uses it.
- **Subtasks.** Under `gate` each `task` call is classified and routed the same way, from subtask rows that learn separately from turn rows. A subtask keeps its model when the call or the permanent agent names one, when it is a council seat, or when you pinned the main model. After a `task` or `run_command` call the caller's cached file reads are dropped, so it reads what the subtask or command left on disk. Its outcome comes from the task's `check` if it has one, else from the last test or build command the subagent ran. An error is a failure, and so is the first attempt of a subtask the parent dispatches again in the same turn (`redone`). A first attempt that failed is reviewed by the judge like a failed turn before it is retried on a stronger model: an excused failure is not retried and counts for nothing, and the retry is told what the judge found. Rewinds rate the turn only, not its subtasks.
- **`/gate`** shows the table per level (turns, subtasks), category and model, with tries, success rate, average cost and the current pick. A replay of the recorded turns follows: how they went, and how Gate's picks now and each model on its own would have done on them, with how many turns each estimate rests on. A policy is judged on the turns it would have routed the same way, each weighted by one over the chance its route had of being picked (inverse propensity weighting), so explored routes stand for the turns that went elsewhere; costs include subtasks but not the stronger models a failed turn moved up to. It also shows how much of the active agent's cost and model calls went to subtasks (overall and for its last 5 turns), subtasks per turn, the escalation rate, and the last 20 routing decisions.

`aura eval run --model gate` runs each task like an `aura -p` run under `gate`: its prompts are classified and routed with Gate's table (exploring as `AURA_GATE_EXPLORE` says), the agent leads the turns that call for a lead (see [Delegation under Gate](#delegation-under-gate)), its subtasks are routed and escalated, and each prompt ends with its outcome. While it runs, it prints the classifier, the models Gate routes among, and one line per task (verdict, cost, time, spend so far, the models used and, when a turn failed, what the judge found and what asking it cost) on stderr. Route reasons are Gate's own (`default`, `proven`, `explore`, `escalate`). A failed verify command counts against the model that did the work (after a move up, that work is not counted) and is not retried or shown to the judge. Each run is an agent named `Eval <suite>/<task>` under the Headless agent, working in the task's workspace, on the data directory of the current folder (`AURA_DATA_DIR`, else `./aura_data`), so a terminal can't have that data directory open at the same time. The run is unattended, so its agent works in `agent` mode at `yolo`, like `aura -p`. After the turns, the task's verify command runs as a `run_command` call through that agent's kernel (wrapped in `sh -c`, with cargo building into the workspace's own `target`), and its verdict is recorded as a later `checks_passed` or `checks_failed` outcome of the last prompt, so eval results count in Gate's table. The results table shows, under each task, the models Gate routed its turns and subtasks to and what the judge found, and its `usd` column and `--budget-usd` include the subtasks', classifier's and judge's calls; a last line gives the judge's share of the cost. Other models, and `aura eval run` with no `--model`, run the plain agent loop on one model as before. `/compare` measures Gate against another model side by side.

### Delegation under Gate

Under `gate`, your agent does each turn itself by default, with its full tools, on the model Gate routes it to: a lead's own reading, planning and checking cost about what doing the work does, before any subtask runs. Gate learns whether leading pays, per category and model, on turns classified `multi_file_change`, `planning`, `debugging` or `research`: led and direct turns are compared on success and on total cost, the subtasks' included, and with probability `AURA_GATE_EXPLORE` a turn is led to find out. A frontier model leads only once leading is proven cheaper on it. On a turn it leads, it decides and integrates, and hands the work to subtasks that Gate routes to cheaper models. Its system prompt tells it to split the work into self-contained tasks. Each task gets a goal, files, constraints, a `check` command and what to report. The lead sends independent tasks together and delegates even small edits. With `AURA_GATE_DELEGATE` on (the default) this is enforced on those turns:

- the lead has no `write_file`, `edit_file`, `delete_file`, `apply_patch`, `git_commit` or `git_commit_push`;
- it sees at most 200 lines per `read_file` and 50 hits per `search_code` or `find_files`;
- `run_command` runs only checks (tests, builds, type checks, linters in check mode) and inspection commands (`git status`, `diff`, `log`, `show`, `ls`, `pwd`), optionally piped into filters such as `tail` or `grep`; command substitution, file redirection and background jobs are refused.

A refused call is recorded with the refusal as its result. These limits come on top of the approval level, never instead of it: at `ask` or `strict` a command the lead may run still asks first. `AURA_GATE_DELEGATE=off` gives the lead its full tools, and its prompt only recommends delegating. Instances of permanent agents never lead, and neither does an agent on a pinned model.

- **Agents to delegate to.** `implement` (one focused change) and `tests` (writes and runs tests) are seeded next to `explorer` and `reviewer`. The bundled subagent kinds used by `aura-node` gain `implement` and `tests` too.
- **`check`.** A `task` call can name a command that the app runs in the workspace after the subagent finishes, as a `run_command` call through the subagent's kernel, at the approval level the subagent took from its parent. Its exit status is the subtask's verdict.
- **Compact results.** The caller gets back the subagent's reply (up to 4,000 characters), the files it changed, and the check with the last 20 lines of output when it failed, not the transcript.
- **Below the lead's tier.** When the lead is on a frontier model and you can use a standard-tier one, its subtasks are routed among the models below the frontier tier, so the top tier is not paid for twice. An escalated retry may still reach the lead's tier.
- **One at a time.** Permanent agents whose tool lists change no files (such as `explorer` and `reviewer`) run side by side. The others run one at a time per workspace, so their edits never collide.
- **One escalation.** If a subtask Gate routed fails its check or errors, its changes are undone and the subtask runs once more on the next tier up (the cheapest model there), with the failure output. The caller is told about the retry. Only the first attempt counts in Gate's table; the retry is recorded with route reason `escalate`.
  - In a git repository, the workspace folder is put back as it was before the first attempt: files it changed or deleted, by any tool or shell command (a code generator, `sed -i`, `cargo fmt`), get their earlier content back, uncommitted changes from before included, and files it created are removed. Files git ignores (such as `target/`) are left alone, and so are your index, `HEAD`, branches and stash; commits the attempt made stay. Folders a removed file leaves empty stay too, and a file the attempt replaced with a folder that still holds ignored files is not put back.
  - This needs the attempt to have the workspace to itself: it must hold the workspace's write lock, and its caller must wait for it. Anything else that changes files in that folder during the attempt (another agent's own tools, a command you type) is undone as well. A subtask run in the background, a read-only one, or one started by a write subtask (which shares its parent's lock) only gets its file tools' changes undone.
  - Outside a git repository, only the files the attempt's file tools changed are put back; changes made by shell commands stay.

### `/compare` and the BENCH tab: two arms on the same work

`/compare` runs the same work on two arms, A and B, several times each, in copies of your workspace. Each arm is Gate or a model, so it compares Gate with a model, two models, or Gate with and without enforced delegation. The work is a prompt, a plan, or an earlier session replayed prompt by prompt. It then shows each arm's time, cost and quality. The chat panel's BENCH tab runs the same comparisons from a form (see [The BENCH tab](#the-bench-tab)).

| Command | What it does |
|---------|--------------|
| `/compare [prompt \| --plan <file> \| --session current\|<ms>] [--a <arm>] [--b <arm>] [--repeats N] [--check <cmd>] [--budget $] [--learn]` | Prepare a comparison and show its estimated cost at list prices. With no prompt it uses your last prompt; `--plan` runs `/build` on that plan in each copy; `--session` replays a session of this chat (`current`, or the one that started at that Unix millisecond) prompt by prompt. An arm is `gate`, `gate:nodelegate` (Gate whose lead keeps its tools), or a model id or part of one, such as `sonnet`. `--a` defaults to `gate`; `--b` (also `--against`) defaults to the strongest Opus you can use (else the strongest frontier model), or to `gate` when arm A is a model. Against `gate:nodelegate`, a plain `gate` arm always enforces delegation. `--repeats` sets the runs per arm (default 3, at most 10). `--check` defaults to the project's check command (`AURA_CHECK_COMMAND` or `AGENTS.md`). `--budget` stops starting runs once the finished ones cost that much. `--learn` lets Gate arms explore (see Runs). |
| `/compare go` | Start the prepared comparison. |
| `/compare cancel` | Drop the prepared comparison, or stop the running one; runs under way end where they are. |
| `/compare` | List past comparisons. `/compare show [n]` shows one, by default the latest. |
| `/compare diff [run]` | Show a run's changes (`a-1`, `b-2`, …), or the first run of each arm. |
| `/compare pick a\|b` | Rate every run of that arm good and every run of the other bad in Gate's table. The arm can also be named by what it runs on (`gate`, `opus`) when only one arm does. |
| `/compare apply <run>` | Apply that run's changes to your workspace and remove the copies. |
| `/compare dismiss` | Remove the copies without applying anything. |

- **Runs.** Each run is an agent under a `Compare #n` agent in the Agents panel. It works in its own copy under `AURA_HOME/compare/`: a detached git worktree with your uncommitted changes carried over, or a copy without build output for a folder that is not a git repository. Nobody approves a run's calls, so it works in `agent` mode at `yolo` whatever your own level: nothing asks, and its commands may only write inside its copy (where Linux Landlock is unavailable, commands that may reach outside it still ask, as at `yolo` anywhere). Up to 4 runs work at once, alternating between the arms. A Gate arm routes with exploration off, unless `--learn` is given: then it explores as a routed turn does (`AURA_GATE_EXPLORE`), so the runs teach Gate about models it would not pick yet; each prompt records whether it explored and how likely its model was to be picked. A turn of its run that ends with a failed test or build goes on in the same conversation on the cheapest model of the next tier up, as a routed turn does (without asking the judge; the run's check judges the result). A model arm runs its turns and its subtasks on that model.
- **Replaying a session.** Each run sends the session's prompts in order on one conversation, each prompt classified, routed and recorded like a turn of its own. The prompts are the ones the session's file checkpoints name (those still in the conversation), else every message you typed. Each copy starts from the files as they were when the session started: the files the session changed are put back from their checkpoints. Changes made by shell commands are not undone, and files outside the workspace stay as they are. The comparison keeps the session's own cost (calls, list price and models) to measure the arms against, and what you kept from it: the session's changes to the files as they are now (later edits show up too, which the view notes for past sessions) and its last reply. A run that fails or is stopped ends at that prompt. The check, diff and judge come after the last prompt.
- **Quality.** The check runs in each copy after its run, as a `run_command` call through that run's kernel, so it is in the run's record and gets the same command policy as the run's own commands. A check whose first program is not on the command allowlist (`grep`, for example) is shown as "could not run" and gives no verdict; start it with an allowed program such as `sh -c '…'` or `cargo`. A check that runs past 20 minutes fails. Then a blind judge, the strongest Opus (or frontier) model, pairs the finished runs of arm A with arm B's, in order, and judges each pair. It sees the task (every prompt, for a session), both diffs and both final replies, in random order and without arm or model names, and picks one or calls a tie. For a replayed session it also sees what you kept, as a reference, and rates each attempt `BETTER`, `AS_GOOD` or `WORSE` than it.
- **View.** For each arm: median wall time, total and median cost at list prices (subtasks and classifier calls included), tokens per model, check passes, the judge's wins, losses and ties, and, for a replayed session, how many runs the judge rated better than, as good as or worse than what you kept. The judge's cost is shown separately. The view says so when the arms are within one check pass of each other. With more than one prompt, a row per prompt shows each arm's mean turn cost and how many of its turns' last test or build command passed or failed, or errored. A replayed session's own cost is shown under the arms. In the list of runs, each Gate run says how its turns were routed: their categories and difficulties, whether the lead did them itself (`direct`) or led, what the lead and its subtasks cost when subtasks ran, and how often a turn moved up a tier.
- **Plan usage.** When runs work on a Claude plan (`claude/*`) or a ChatGPT plan (`chatgpt/*`), the view shows per arm the share of each plan window's limit the arm's runs used, such as `Claude plan 5h window +3.1%, 7d window +0.4%`. It comes from the usage headers those providers send with each response (Claude: `anthropic-ratelimit-unified-<window>-utilization` and `-reset`; ChatGPT: `x-codex-primary-*` and `x-codex-secondary-*`), which each model call records. A reading covers the whole account, so runs working at the same time see each other's usage. Instead of taking each run's first and last reading, which would count that overlap once per run, every rise of a window between one reading and the next, across all runs in the order their calls were recorded, goes to the run whose call saw it. Nothing is counted twice, and the arms' shares add up to how far the window moved during the comparison. The first reading of a window only sets where it started, so the call that took it isn't counted. A lower reading that arrives late adds nothing, and a window that resets counts again from zero. Usage outside the comparison while it runs, such as another session on the same plan, lands on whichever run reads the window next. Other providers (aura-router models, API keys, local models) report no plan usage. When no call reported any, the view says so and shows tokens per model, which is what draws a plan down.
- **Feeds Gate.** Each run records each prompt with route reason `compare` and its turn's outcome, and the check's verdict as a later outcome of the last prompt, so comparisons count in Gate's table.
- **Older comparisons.** Comparisons recorded before arms A and B read as Gate (A) against their other arm (B), with their runs, judgments and picks as they were.

## The BENCH tab

The chat panel's BENCH tab lists every comparison, newest first, with each arm's median cost and check passes, and keeps them up to date while they run. `/bench` opens it and `/bench new` opens its form. With nothing typed in the input line:

- **List.** ↑/↓ moves, Enter opens a benchmark, `n` sets up a new one, `r` reloads, `c` stops the one running, Esc goes back to CHAT.
- **Form.** ↑/↓ moves between rows and ←/→ changes the work, either arm and the runs per arm. The work is your last prompt, a prompt you type, or one of this chat's sessions (up to 20, with their prompt count and cost). The arms offer `gate`, `gate:nodelegate` and every model you can use; arm B also offers its default. The prompt, check and budget rows take what you type in the input line when you press Enter. Enter on the last row shows the estimate, and pressing it again starts the benchmark. Esc drops it.
- **Benchmark.** The arms side by side, as in `/compare show`, with each run's cost next to the replayed session's (`0.40×`), a row per prompt, the judge's verdict on each pair with the first line of its reasoning, and the runs. ←/→ picks a run, `d` shows its diff (or the first run of each arm), `y` applies it, `a` or `b` rates that arm better (as `/compare pick`), `x` removes the copies, and ↑/↓ and the wheel scroll.

## Long tasks in the terminal

- **No caps.** A turn keeps going until the agent finishes, you press Ctrl+C, or a limit from [Terminal long runs](configuration.md#terminal-long-runs) stops it.
- **Todo list.** For multi-step work the agent keeps a plan with the `todo_write` tool. If it stops with items still open, it is sent back to the next one. It ends anyway when it asks you something, says it is blocked, or twice in a row leaves the list unchanged. The list survives history summaries and restarts.
- **Delegating.** In `agent` mode the agent on screen coordinates: it writes the plan with `todo_write`, hands each piece that is more than a small edit to the permanent agent whose description fits it best, with `task` and the item's `todo_id`, and answers simple questions itself. The item goes in progress, assigned to that agent (`→ coder` in the todo list and the PLANS tab), as soon as the call is made. When the agent reports back the item is done, or pending again if it failed; this happens when its reply comes in, even for a task run in the background. Spawned agents sit under the agent that spawned them in the projects list, joined to it by tree lines. In the GRAPH tab each `task` row is wired to the agent doing it: the wire is colored by how the task went, and the agent's card ends `↩ reported to <agent>`.
- **Tasks.** Work the agents do is kept as tasks in `.aura/tasks/<id>.json` in the workspace, listed in the side panel's TASKS tab. A request becomes a task once its turn calls a tool, and each `task` call becomes a task under it, done by the agent it spawned. A task's items follow its agent's todo list, showing what is done and what is left. A task ends done, failed or stopped. A request ends open if items were left, and so does a task still running when the app quit. Enter shows the task's agent, `p` opens the plan it works on at its todo, and Left/Right or Space fold it. In the PLANS tab, `t` on a todo jumps to the task doing it.
- **Talking to a busy agent.** A message you send while the agent works goes into its running turn, and the agent reads it before its next step. A message that arrives as the turn ends runs as the next turn. Shell commands typed in the chat still wait for the agent's current work.
- **Background commands.** `run_command` with `background: true` starts a job and returns at once. The agent checks it with `job_output` and stops it with `job_kill`. When a job exits, the agent that started it is told: inside its running turn, or in a new turn if it is idle. Quitting the terminal stops every job.
- **Foreground commands** time out after 2 minutes unless the call sets `timeout_ms`, up to 10 minutes.
- **Status line.** The row under the input shows the agent on screen: context use (`ctx 62% ▰▰▰▰▱ 124k/200k`, yellow from 75%, red from 90%), prompt cache hits, estimated cost at API list prices, model calls, running background jobs (`2 bg`), queued work, and how often the history was summarized. Parts that don't fit are dropped from the right. The Chat panel's bottom border shows todo progress, e.g. `[3/7] ▸ Write the parser`.
- **`/context`** shows how full the context window is and what fills it (system prompt, tools, skills, conversation) as of the last turn.
- **`/cost`** lists the session's model calls, tokens (new input, cache reads and writes, output) and estimated cost.
- **`/todos`** opens or closes the agent's full todo list above the input.
- **Sessions survive restarts.** After every turn the terminal saves the agent's session (the conversation as the model saw it, tool calls included, plus its todo list and usage totals) under `<data_dir>/sessions/<agent>/`. On restart the agent continues from there, with anything the record holds after the checkpoint replayed on top. Without a usable checkpoint it falls back to the prompts and replies in the record. `/new` starts a fresh session; the old one stays saved.
- **`/resume`** lists the agent's earlier sessions, newest first; **`/resume <n>`** continues number `n` (the agent must be idle). The chat shows the resumed conversation and the status line its usage.
- **Web tools.** `web_fetch` reads a public web page as markdown (scripts, styles and navigation dropped; headings, links, lists and code kept). It refuses binary files, non-HTTP URLs, and localhost or private-network addresses unless `AURA_WEB_FETCH_ALLOW_PRIVATE=1`. Long pages come back in parts the agent can page through. `web_search` returns titles, URLs and snippets from the provider set in [Terminal long runs](configuration.md#terminal-long-runs); without one, it says what to set. Plan and Ask modes allow both tools.
- **Post-edit check.** If the project defines a check command, the terminal runs it after each step in which the agent edited files and gives the agent the last 80 lines of output when it fails. The chat shows a warning for a failure and a note when the check passes again. Several edits in one step run the check once, and a failure that repeats word for word is reported in one line. Set the command with `AURA_CHECK_COMMAND`, or put it in the project's `AGENTS.md`, either as a line or as a fenced block whose lines run in order and stop at the first failure:

  ````markdown
  check: cargo check --workspace

  ```check
  cargo fmt --check
  cargo clippy -- -D warnings
  ```
  ````

  The first `check:` line or `check` block wins; `check:` lines inside other code blocks are ignored. The command runs in the agent's project folder through the shell, with `AURA_CHECK_TIMEOUT` as its time limit. A check from `AGENTS.md` runs without asking, so read the `AGENTS.md` of a project you don't trust, or set `AURA_CHECK_COMMAND=off`.
- **`/compact [focus]`** summarizes the older history now instead of waiting for context pressure, keeping the first message and the recent steps as they are. Text after the command tells the summary what to keep. A busy agent does it when its current work ends. The summary replaces history only in the running session; the record keeps every message.
- **File checkpoints.** Before the agent's file tools (`write_file`, `edit_file`, `delete_file`) first change a file in a turn, the terminal saves what the file held, or that it did not exist. Each content is stored once, by hash, under `<data_dir>/checkpoints/<agent>/blobs/`, and each turn has a manifest under `<data_dir>/checkpoints/<agent>/turns/` with its time, prompt and the files it changed. Edits by a subagent count toward the turn that started it. Files changed by shell commands (`run_command` or a command you type) are not seen, so they are not checkpointed and cannot be rewound.
- **`/rewind`** lists the turns of the current session, newest first, with their time, prompt and how many files each changed. **`/rewind <n> [code|chat|both]`** undoes turn `n` and every turn after it (the agent must be idle). `code` puts back each file those turns changed as it was before turn `n`, and removes files that did not exist then. `chat` cuts the conversation the model sees back to just before turn `n`'s prompt and saves the session. `both` is the default. If turn `n` was summarized away by a history summary, only `code` is possible. The rewind is noted in the agent's record, and the record itself keeps every message.
- **Diffs.** A `write_file`, `edit_file` or `delete_file` block in the chat shows the change as a diff with line numbers when you expand it, and its header counts the added and removed lines (`Edited main.rs · +3 -1`). `edit_file` blocks restored from history show the replaced text against its replacement. **`/diff`** shows how the files the latest turn changed differ now from before that turn; **`/diff session`** does the same for every file changed since the session started.
- **Approvals.** When a tool call needs your approval, the turn pauses and the agent's chat shows what it wants to run. Press `y` to allow it once, `a` to always allow it in this project, or `n` (or Esc) to deny it. "Always" is saved per project folder in `<data_dir>/approvals.json`: for `run_command` it covers the program and its subcommand (`cargo test`, not every `cargo` command) and never matches a shell script that chains or redirects commands; for other tools it covers the tool. Commands you type in the chat are never asked about. A prompt left unanswered is denied after 10 minutes, and Ctrl+C denies it and stops the turn. An agent that is not on screen asks in its own chat, and the terminal tells you which agent is waiting.
- **Approval levels.** The badge before the model name in the status line shows how much the agent on screen asks: `/approval <level>` picks one, and Shift+Tab steps `yolo` → `ask` → `strict`.
  - `ask` runs reads and searches, and asks before file edits, commands and everything else unless a project "always allow" rule covers it.
  - `strict` asks before everything but reads, even what an "always allow" rule covers, and offers no "always" answer.
  - `yolo` (the default) asks nothing. The agent's commands can write only inside the folder the terminal started in, plus temp folders and package caches (`~/.cache`, `~/.cargo`, `~/.npm`, `/tmp`, ...); reading works everywhere. On Linux this is enforced by Landlock (kernel 5.13 or later), and a command that tries to write elsewhere fails with a permission error. Where Landlock is not available, `yolo` still asks before a command unless the command plainly stays in the folder (no shell syntax, no `..`, `~` or outside absolute paths), and the terminal warns when it starts at `yolo` or you pick it. `AURA_YOLO_WRITABLE_PATHS` adds writable folders. Commands you type, and calls you approve at a prompt, are not confined.

  The starting level comes from `--approval`, then `AURA_TUI_APPROVAL`, else `yolo`; it is never remembered between sessions.
- **Modes.** `/mode` lists the work modes and `/mode <name>` picks one for the agent on screen: `agent` (the default) can edit files and run commands, `plan` can read, run commands and write Markdown plans (`/plan`), `read-only` only reads, and `debug` reads and runs sandboxed probes. The approval level decides which of the allowed calls ask first; reading files and searching never ask. A badge shows any mode other than `agent`. Each switch of mode or level is written to the agent's record (as `agent/ask` → `agent/yolo`) before the next tool call uses it, and agents it spawns start with the same mode and level. The starting mode comes from `--mode`, then `AURA_TUI_MODE`, then `AURA_DEFAULT_MODE`. The old mode names `default`, `accept-edits` and `unrestricted` are gone: `/mode unrestricted` points you to `/approval yolo`.
- **Notifications.** The terminal rings the bell when a turn that took at least 10 seconds finishes and when an agent needs approval. Desktop notifications are off unless you set `AURA_TUI_DESKTOP_NOTIFY=1`. See [Terminal long runs](configuration.md#terminal-long-runs) for the settings.

## Project instructions

Terminal agents follow the instructions a project keeps for coding agents. Before each turn the terminal looks for these files in the agent's working folder (its project's folder, or the folder the terminal was started in) and in every folder above it, stopping below your home directory:

- `AGENTS.md` (or `agents.md`)
- `CLAUDE.md`
- `.cursor/rules.md` and the `.md` / `.mdc` files in `.cursor/rules/`

Your own instructions for every project go in `~/.aura/AGENTS.md` (`$AURA_HOME/AGENTS.md` when `AURA_HOME` is set).

- **Precedence.** The files go into the system prompt from the most general (yours, then the folders furthest up) to the most specific (the working folder). The prompt tells the agent that the later file wins when two disagree, and that what you say in the chat beats them all. A `CLAUDE.md` that is a symlink to `AGENTS.md` is read once.
- **Cursor rules.** A rule with `alwaysApply: true`, or with no frontmatter, is always included. A rule with `globs` is included with its globs, and the agent applies it only to matching files. A rule with only a `description` is listed by path and description for the agent to read when it is relevant. A rule with none of these is meant to be named by hand and is skipped.
- **Size limit.** Instructions are capped at 32 KiB in total. The nearest files are kept first; a file that no longer fits is cut at a line end with a note saying so, or left out and listed by path when too little room is left.
- **Changes.** Files are re-read when their size or modification time changes, so an edit applies from the next turn. Unchanged files produce the same bytes every turn, right after the base system prompt, so they stay in the prompt cache.
- **`/context`** lists the files the agent loads, with their size and whether each was cut, left out, or only listed.
- **`/init`** asks the agent, as a normal turn, to study the repository and write an `AGENTS.md` at the project root: what the project is, build, run, lint and test commands, the layout, and conventions and pitfalls. If the project already has an `AGENTS.md`, the agent improves it instead of starting over. A busy agent starts it when its current work ends.

Set `AURA_PROJECT_INSTRUCTIONS=off` to stop loading these files.

## Input in the terminal

- **`@` mentions.** Typing `@` opens a file picker over the agent's workspace, ranked by name and path prefix, then substring, then letters in order. `↑`/`↓` move, Enter inserts `@path`, Tab completes (on a folder it stays open to go into it), and Esc closes it for that word. The picker skips `target/`, `.git/`, `node_modules/` and plain names or `*.ext` patterns from the workspace's root `.gitignore`, and it leaves out paths that contain spaces. When you send the message, each `@path` that names a file or folder inside the workspace is attached: the model gets up to 64 KiB of each file (with a note when it is cut), at most 256 KiB per message, and a listing of up to 200 entries for a folder. Binary files are described, not shown. The chat and the record keep the message as you typed it. Mentions that don't resolve stay plain text.
- **Images.** Ctrl+V pastes an image from the clipboard, or the clipboard's text when it holds no image. Pasting the path of a PNG, JPEG, GIF or WebP file (as terminals do when you drop a file on them) attaches that file. Each image puts an `[image N]` chip in the input; deleting the chip drops the image. The model gets the images as image blocks before the message text, and the chat and the record show the chips. An image may be at most 3.75 MiB (5 MiB once encoded), and a message holds at most 5. A running turn only takes text, so a message with images sent to a busy agent waits for its current work to end. Reading the clipboard uses `wl-paste` or `xclip` on Linux, `osascript` or `pbpaste` on macOS, and PowerShell on Windows, and gives up after 2 seconds.
- **Ctrl+G** opens the input in your editor (`$VISUAL`, then `$EDITOR`, then `vi`; see [Terminal editor](configuration.md#terminal-editor)). When the editor exits, what you saved replaces the input, newlines included, ready to edit further or send with Enter. If the editor fails, the input stays as it was.
- **Pasting.** Pasted text arrives in one piece when the terminal supports bracketed paste: newlines stay in the input (shown as `↵`) instead of sending the message line by line.

## The GRAPH tab

The chat panel has four tabs: CHAT, TERMINAL, GRAPH and BENCH (see [The BENCH tab](#the-bench-tab)). Shift+←/→ moves between them while the panel has the focus, and so does clicking one. The `⤢` at the right end of the panel's header bar, Ctrl+X or `/expand` expands the panel to fill the window, whichever tab or view it shows, hiding the other panels until `⤡` (or Ctrl+X, or `/expand`) restores them. GRAPH shows what the agent on screen and every agent below it did, as their kernels recorded it, as turns of cards wired together on a canvas. Each agent's turns go down a column, one under the next, until the view has no room for another; they go on at the top of the next column to the right.

- **Turns.** A turn is a main card, `turn`, with the prompt that started it and the replies it gave as rows (`↳`), and under it, joined to it by a line down its left side, a subcard for each model call it made: the model, a line with the tokens it read and wrote (`↑48.2k ↓516`), its cost at list prices and how long it took, and the tool calls it made as rows. The turn's own line has the same for all its calls together, with the time from its prompt to its last step. Calls the harness makes around the agent's work are subcards too, in a muted colour and named for what they did: `gate` when Gate asked a small model what the prompt is, to pick the model that takes it (the turn's first step), `judge` when Gate's judge reviewed failed work, and `title` when the chat was named after its first turn. A `message` from another agent is a small subcard of its own. What a `task` came back with is shown on the `task` row; a result with no row is a subcard. A card lists up to ten rows; past that its last line says how many more it has. The mark before the kind is the step's state: `●` running or waiting, `✓` done, `!` held up, `✗` failed or denied. A turn takes the state it ended in.
- **Agents.** When a `task` or `spawn` hands work to another agent, that agent's turn is a main card of its own, right of the row that started it, with its own model calls under it. Each agent's columns are in a band. The agent on screen is the top band, without a name; every other agent's name is above its columns, kept on screen while any of its cards is. Agents that work one after another share a band, and agents that work at the same time get one each. When the bands are taller than the view together, their columns get shorter so that all of them fit. A turn is never left of another agent's card it has a wire from.
- **Wires** run from a card's right side into another's left: from a `task` row to the turn of the agent that took the work, from a `spawn` to the agent it started, from a message to where the other agent got it, and to an agent's next turn when it starts a new column. The selected card's wires are highlighted, and so are the cards at their other ends.
- **Moving.** `↑`/`↓` (or `k`/`j`) go through the lines of a card, then on to the card under or over it, or the nearest card in the band below or above; `←`/`→` (or `h`/`l`) go to the agent's turn before or after. `[` and `]` jump between prompts, and Home/End go to the first card and the newest step. `f` follows the selected line's wire to the other agent, and again to come back. The view keeps the selection in sight and follows new steps until you move off the newest one; End follows again.
- **Panning.** Drag empty canvas, or use the wheel (Shift+wheel, or a wheel that tilts, goes sideways); PgUp/PgDn move a page. The canvas has no edges: the view goes as far past the cards as it is taken, and stays there until the selection moves or `0` brings it back to the selection. In the desktop app the canvas moves with the pointer by the pixel and touchpads pan smoothly; the app draws the cards and right-angled wires itself, with menus and dialogs over them.
- **Zooming** (desktop app). Ctrl+wheel or a pinch zooms around the pointer, from 25% to 200%; `+` and `-` zoom around the middle of the view and `0` goes back to 100% around the selection. Zoomed out past 50%, text is drawn as bars, to show the shape of a long flow. The terminal does not zoom.
- **Folding.** Every turn is open until you fold it. Clicking the `▾` button in a card's title folds it, and the `▸` there opens it again: a turn folds to its main card, which says how many steps it hides and which its wires then leave from, and a model call folds its tool calls into it. Space does the same for the selected card's model call, or else its turn, and `z` folds or unfolds every turn. Following a wire into something folded opens it. What you fold stays folded as the agents add steps, and when you come back to an agent after showing another.
- **Opening.** Clicking a line selects it, and a second click (or Enter) opens everything known about the step as JSON: the agent, the time, the transaction hash, and the prompt, arguments, result or reply, each cut to 1,200 characters. `r` opens the step's entry in the RECORD panel, which has the rest; the panel lists the agent on screen, so for another agent's card press `o` first. `o` switches to the agent the card belongs to (agents started with `spawn_agent` have no chat to switch to). Esc goes back to CHAT.

The keys work while the input line is empty; typing a message or a `/command` works as in CHAT.

The flow is read from the record store when the tab opens: the last 1,500 entries of each agent, for up to 48 agents (of more, the ones that worked last), keeping the newest 4,000 steps. It then follows the agents live, dropping the oldest steps as new ones arrive. Agents started with `task` and with `spawn_agent` both appear; an agent above the one on screen does not, so open the lead agent to see a whole delegation.

## `aura -p`: one prompt without the terminal

`aura -p "prompt"` (or `--print`) runs one prompt to the end, prints the result, and exits. It is meant for scripts, CI and evals.

```sh
aura -p "fix the failing test in src/parser.rs"
git diff | aura -p "review this change" --output-format json
aura -p - < task.md --max-time 30m --max-cost-usd 5
aura -p "now add docs for it" --continue
```

- **Prompt.** The text after `-p`. `-p -`, or `-p` with no text, reads the prompt from stdin. When stdin is piped and a prompt is given too, stdin is appended to the prompt after a blank line. Scripts that pipe nothing should give stdin `/dev/null` so the run does not wait for input.
- **Same agent machinery as the terminal.** The run uses the terminal's session code: long-run settings (no iteration cap, the todo list keeps the agent going), background jobs, history compaction, session checkpoints, the web tools, the post-edit check, project instructions, and the permanent agents for `task`. It works in the current directory (or `--cwd`) and keeps its data in `AURA_DATA_DIR`, else `./aura_data`, like the terminal.
- **Sessions.** Runs belong to a dedicated agent named "Headless" in the data directory's agent list, so they never replace the session of an agent the terminal shows. Each run starts a new session unless `--continue` (the Headless agent's latest session) or `--resume <session_id>` is given. A session id is `<agent id>:<session start ms>`, printed as `session_id` in the `json` and `stream-json` output; `--resume` also accepts ids of terminal agents' sessions, and a bare number names a session of the Headless agent. The terminal can open the Headless agent and `/resume` its sessions too.
- **Background jobs.** When the agent ends its turn while jobs it started are still running, the run waits for them (up to `AURA_HEADLESS_JOB_WAIT`, default 10 minutes, and within `--max-time`) and gives the agent each exit notice as a new turn, as the terminal does. Jobs still running at the end are stopped.
- **Permissions.** Nothing prompts. A tool call the policy would ask about is denied, and so is any tool outside `--allowed-tools` or inside `--disallowed-tools`; filtered tools are also left out of what the model is offered. `todo_write` is always allowed unless disallowed, and `job_output` / `job_kill` come with `run_command`. `--mode plan`, `ask` and `debug` behave as the terminal's `plan`, `read-only` and `debug` modes at the `ask` level, with every ask denied. `--mode agent` (the default) runs at `yolo`: edits and commands run, and on Linux commands can write only inside the working folder. `--approval ask` or `strict` makes a run deny those calls instead. `AURA_TUI_MODE` and `AURA_TUI_APPROVAL` only set the terminal's starting mode and level. Agents spawned with `task` get the same mode and tool filter.
- **Limits.** `--max-turns` counts model calls across the whole run. `--max-time` and `--max-cost-usd` also apply to the whole run and default to `AURA_TUI_TURN_TIMEOUT` and `AURA_TUI_MAX_TURN_COST_USD`. A limit stops the agent between steps, so the work done so far is kept in the session and `--resume` can continue it. A limit that is reached while the agent writes its final reply does not count as stopping it.
- **Output.** stdout carries only the chosen format; logs go to stderr (warnings only by default, `--verbose` or `RUST_LOG` for more).
- **Store lock.** RocksDB lets one process open a data directory at a time, so `aura -p` cannot share a data directory with a running terminal. Point `AURA_DATA_DIR` elsewhere to run both.

### Flags for `-p`

Each requires `-p`.

| Flag | Default | Description |
|------|---------|-------------|
| `--output-format <text\|json\|stream-json>` | `text` | What is printed; see below. |
| `--resume <SESSION_ID>` | — | Continue the saved session with this id. |
| `-c, --continue` | off | Continue the Headless agent's latest session. |
| `--allowed-tools <TOOLS>` | all | Only these tools may run (comma- or space-separated, repeatable). |
| `--disallowed-tools <TOOLS>` | none | These tools may not run. Wins over `--allowed-tools`. |
| `--max-turns <N>` | no limit | Stop after N model calls. |
| `--max-time <DURATION>` | `AURA_TUI_TURN_TIMEOUT` | Stop after this long: `90s`, `45m`, `2h`, or a bare number of minutes. |
| `--max-cost-usd <USD>` | `AURA_TUI_MAX_TURN_COST_USD` | Stop once model calls cost more than this at API list prices. |
| `-d, --cwd <DIR>` | current directory | Work in this directory. |
| `--model <MODEL>` | the terminal's choice | Model for this run only; the `/model` choice is not changed. A local model whose server is not ready fails the run at once. |
| `--provider <anthropic\|mock>` | `anthropic` | `mock` answers without a model, for tests. |
| `--verbose` | off | Debug logging to stderr. |
| `--mode <agent\|plan\|ask\|debug>` | `AURA_DEFAULT_MODE`, else `agent` | Global flag; the session mode. |

### Output formats

- **`text`** prints the agent's final reply. Errors go to stderr.
- **`json`** prints one object:

  | Field | Meaning |
  |-------|---------|
  | `type` | Always `"result"`. |
  | `exit_reason` | `success`, `max_turns`, `max_time`, `max_cost`, `error` or `interrupted`. |
  | `is_error` | `true` unless `exit_reason` is `success`. |
  | `result` | The agent's final reply. |
  | `session_id` | Pass to `--resume`. |
  | `model` | Model the run used. |
  | `num_turns` | Model calls made by this run. |
  | `agent_turns` | Agent turns: the prompt plus one per background-job notice. |
  | `duration_ms` | Wall time. |
  | `usage` | `calls`, `input_tokens` (neither read from nor written to the cache), `cache_read_tokens`, `cache_write_tokens`, `output_tokens`, `cache_hit_rate` (cache reads over all prompt tokens) and `cost_usd`. |
  | `total_cost_usd` | Estimated cost at API list prices, or `null` when no call had a price. |
  | `compactions` | History summaries made to stay within the context window. |
  | `todos` | The agent's todo list at the end (`id`, `content`, `status`). |
  | `files_changed` | Files the agent wrote, edited or deleted with its file tools. |
  | `error` | What went wrong, when `is_error`. |

  A failure before the run starts (for example an unknown `--resume` id) is reported the same way with `exit_reason: "error"`.
- **`stream-json`** prints one JSON object per line as the run goes, each with a `type`: `init` (session id, agent id, model, working directory, mode, offered tools, whether it resumed), `text_delta`, `thinking_delta`, `tool_use` (a call started: `id`, `name`), `tool_result` (`id`, `name`, `input`, `content`, `is_error`), `stream_reset` (drop text streamed since the last tool result), `todos`, `usage` (one model call's tokens and cost), `compacted`, `warning`, `error`, `notice` (a background-job notice starts another turn), `judged` (Gate's judge reviewed a failed turn: `model`, `judge`, `at_fault`, `reasons`, `cost_usd`), and finally the `result` object above.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | The agent finished. |
| 1 | The run failed (model error, unknown session, unreadable data directory, …). |
| 2 | Bad command-line arguments. |
| 3 | `--max-turns`, `--max-time` or `--max-cost-usd` stopped the run. |
| 130 | Interrupted with Ctrl+C. |

## Global flags

| Flag | Default | Description |
|------|---------|-------------|
| `--mode <agent\|plan\|ask\|debug>` | `AURA_DEFAULT_MODE`, else `agent` | Session mode for the terminal UI and `aura -p`. The kernel denies any tool call the mode does not allow. `ask` (also `read-only`) is the terminal's `read-only` mode. `/mode` changes it later. Headless `--ui none` ignores the flag with a warning; there, each `POST /v1/run` carries its own optional `mode`. |
| `--approval <yolo\|ask\|strict>` | `AURA_TUI_APPROVAL`, else `yolo` | What tool calls ask first; see [approval levels](#long-tasks-in-the-terminal). `aura -p` cannot ask, so there every ask is denied, and without the flag `--mode agent` runs at `yolo` (edits and commands run, commands write only inside the folder) and other modes at `ask`. |
| `-p, --print [PROMPT]` | — | Run one prompt without the terminal UI; see [`aura -p`](#aura--p-one-prompt-without-the-terminal). |

## `aura run` flags

| Flag | Default | Description |
|------|---------|-------------|
| `--ui <terminal\|none>` | `terminal` | Terminal UI (ratatui) or headless node. |
| `--theme <name>` | `cyber` | One of `cyber`, `matrix`, `synthwave`, `minimal`. |
| `-d, --dir <path>` | — | Override the working / data directory. |
| `--provider <anthropic\|mock>` | `anthropic` | Model provider for the session. `anthropic` serves `local/*` models from Ollama and everything else from aura-router. |
| `-v, --verbose` | off | Verbose tracing output. |
| `--allow-unrestricted-full-access` | off | Let FullAccess sessions bypass command allowlists (operator opt-in). |

## Plan mode in the terminal

Plan mode lets the agent research a task and write a plan before it touches code.

| Command | What it does |
|---------|--------------|
| `/plan` | Switch to plan mode. `/plan` again leaves it and returns to the previous mode (`agent` when the session started in plan mode). In plan mode the agent can read, search, run commands and write Markdown files, but not edit other files, use the network or spawn agents. |
| Shift+Tab | Cycle the approval level `ask` → `strict` → `yolo`; the work mode stays. |
| `/mode <name>` | Switch work mode (`agent`, `plan`, `read-only`, `debug`). |
| `/approval <level>` | Switch approval level (`yolo`, `ask`, `strict`). |
| `/build [plan path]` | Leave plan mode and implement the latest plan, or the given one. |

In plan mode, ask for what you want; the agent may ask a question or two, then saves a plan to `.aura/plans/<name>_<id>.plan.md` in its workspace: YAML frontmatter (`name`, `overview`, `todos`) followed by the plan in Markdown. Keep chatting to revise it. Once there is a plan, a message that is just "implement" (or "implement it", "build it", "go ahead", "do it", "proceed") does what `/build` does; anything longer goes to the agent as feedback on the plan. While you build, the agent marks todos `in_progress` and `completed` with `todo_write`, and the strip above the input shows its progress. The mode badge next to the model name shows any mode other than agent.

Every mode switch is written to the agent's record as a `mode_change` entry, and the kernel enforces the new mode from then on. A mode can't be changed while a turn is running.
