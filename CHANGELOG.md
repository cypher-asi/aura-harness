# Changelog

All notable changes to the Aura agent-first architecture refactor
are tracked here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Channel members show their agent, with what they work on below

##### Changed

- On the MEMBERS tab each member takes two rows. The first names the
  permanent agent it was started from (`rust-engineer`, or `subagent`
  for a subagent of none), with its code and spinner at the right. The
  second, in gray, is what it works on: its title, a subagent's
  subtask, else `Working…` or `Idle`. A click or hover on either row
  picks that member.
- Every `assign` titles the member it goes to with the assignment's
  subject, or the brief's first line when it has none, so a member's
  title is the work it was last given. A member's first typed prompt
  titles it too, like any agent's. The coordinator sees each member's
  title in its channel block: `working on "…"`, or `free, last on "…"`.
- PROJECTS and the other agent lists show a channel member by its
  title, like any agent, instead of as its permanent agent.
- A member's chat bar shows its title, and its input reads
  `Message REN3`.
- Each `assign` to a permanent agent starts a new instance of it, with
  a title of its own; nothing is written to the permanent agent.
  Members keep `task`, and their subagents nest under them on MEMBERS.

### Each channel says where its agents run

##### Added

- The channel form has an `Agents run` field after Project, `Locally`
  or `In the cloud`, chosen with ←→ or Space. In the cloud needs a
  project, whose folder each machine starts from: without one the form
  says so and won't submit. Enter on `Agents run`, the last field,
  submits. Editing a channel shows its setting. Channels saved before
  run their agents locally.
- INFO shows AGENTS RUN. A cloud channel without a usable project adds
  that new members run locally.
- A new member of a cloud channel moves to a cloud machine of its own
  before its first work, with the boot shown in its chat, and its work
  runs there once it is placed. Without a cloud (no `BOXD_API_KEY`), a
  project folder, or a placement that works, the member stays on this
  computer and its chat says why. The coordinator always runs here,
  the setting moves no member already started, and `/cloud` and
  `/local` still move any one member.

##### Changed

- Work sent to an agent while it moves between this computer and a
  cloud machine waits in its queue and starts where the agent ends up,
  instead of being refused.

### ARCHIVE headers spell out the date

##### Changed

- Day headers in the ARCHIVE tab older than Yesterday read in full, as
  `Wednesday, September 16 2026`, instead of `2026-09-16`. `Today`,
  `Yesterday` and `Earlier` are unchanged.

### A channel member's own chat

##### Added

- In CHANNELS, clicking a member on the MEMBERS tab, or Enter on it,
  opens that member's own chat in the middle column, with the channel
  still open. Its bar names the member, with its code, role and
  `#slug`, and a `✕`; the `✕` or Esc on an empty input goes back to the
  channel's chat. A view over the chat closes before the member chat
  does, and a menu or a selected queue item takes Esc first.

##### Fixed

- A member dropped from the channel while its chat is open gives way
  to the coordinator even when the session can't be told at once, so
  it never stays on screen under the channel's name.

### To-dos move to a drawer above the input

##### Changed

- The agent's to-do list no longer runs in the chat. It sits in a drawer
  between the chat and the input while any item is open, during a turn
  and between turns, with `Working on N to-dos` and `done/total` in its
  header. In an open channel it shows the coordinator's list. Clicking
  the header or `/todos` folds it to that row, which
  then shows the item in progress. Long lists start at the item in
  progress and count the rest on a `+N more` row. On a short screen the
  drawer gives up rows before the chat, queue and benchmark panels.
- `/todos` no longer opens a separate list over the chat, and the
  checklist's `N more` / `show less` toggle is gone.

### The git tools commit only the repository the agent works in

##### Changed

- `git_commit`, `git_push` and `git_commit_push` no longer look above
  the agent's workspace for a repository. A workspace that is a folder
  inside a bigger repository, rather than the top of a repository
  itself, fails with "not a git repository". Before, `git_commit`
  staged and committed the whole bigger repository, changes outside
  the workspace included.

### Channel coordinators hand work to agents

##### Changed

- A channel's coordinator is the `coordinator` agent of AGENTS, where
  it can be read and edited. Each channel runs an instance of its own,
  and it can't be started any other way. A `coordinator.md` an earlier
  version wrote and nobody edited is replaced with the new one.
- The coordinator does no work itself: its tools are `assign`,
  `send_to_agent`, `list_agents`, `get_agent_state` and `todo_write`.
  `assign` hands a piece of work to a member by its code, or to a new
  member started from a permanent agent. Members are top-level agents
  of the channel and its project, not the coordinator's subagents.
- The coordinator briefs the goal, the context and what done looks
  like, never how to do it. Whether to use subagents is the member's
  call: every top-level agent has `task`, whatever its definition
  lists.
- What the coordinator writes to the channel is only results, blockers
  and decisions to make, in at most three short lines.
- Members settle details with each other directly with
  `send_to_agent`: a member reaches the channel's other members, its
  coordinator and the top-level agents of its project. A request or an
  assignment is answered when the turn that saw it ends, with the
  agent's last words, and every message also goes to the INBOX tab.
  Two agents can send each other at most 8 requests about one subject.
- A chat shows a message from another agent as one line, `✉` with who
  sent it and what about, which expands like a step.
- A channel's member list shows the coordinator's agents beside it,
  with their subagents below them. At start-up, agents an earlier
  coordinator started as its subagents become top-level members of its
  channel.

### A REPO tab shows the agent's repo, and HOST is an icon

##### Added

- The side panel has a REPO tab after SANDBOX for the git repo the
  agent on screen works in: how its branch stands with its upstream,
  the uncommitted changes, and a timeline, newest first, of its
  commits, merges and rebases, pushes, pulls and fetches, resets,
  checkouts and stashes, with the repo's GitHub pull requests among
  them. A strip of filters along its top narrows the timeline to one
  kind, or lists the branches, tags or worktrees instead. Enter or a
  click shows a row's diff in the chat, `o` opens a PR's page, and
  `/repo [filter]` shows the tab.
- PRs are read with `gh` when it is installed and signed in, else with
  GitHub's REST API and `GH_TOKEN` or `GITHUB_TOKEN`, at most once a
  minute. The token stays in the app and is sent to `api.github.com`
  alone.

##### Changed

- HOST is no longer a labelled tab but a computer icon at the right end
  of the side panel's tab bar, left of `ALL`, centered in its cell; the
  desktop draws it as a screen on a stand. Shift+←/→ reaches it after
  TASKS.

### Cloud agents commit, push and open pull requests on GitHub

##### Added

- A cloud agent works in a real clone of its project, on a branch of
  its own named `aura/<name>-<code>` that starts at this repository's
  HEAD. The project's snapshot holds its git history at the commit it
  was built from, sent as a `git bundle` rather than a copy of `.git`.
  A new machine also gets the commits made since and the uncommitted
  changes. A spawned agent that forks its parent's machine starts its
  own branch from the parent's.
- With a GitHub App set up (`AURA_GITHUB_APP_ID` and
  `AURA_GITHUB_APP_PRIVATE_KEY_PATH`; see "GitHub for cloud agents" in
  `docs/configuration.md`), the agent pushes its branch with `git_push`
  or `git_commit_push` and opens pull requests with `gh pr create`, as
  the App's bot. The app mints a one-hour token for the project's
  repository only, renews it before it expires, and keeps it in a boxd
  secret bound to GitHub. The machine holds only a placeholder, which
  boxd's proxy swaps for the token. A cloud agent can push only its own
  branch. `gh` is now installed on cloud machines.
- Pull and `/local` also bring the agent's commits back. They are
  fetched into this repository's branch of the same name, fast-forward
  only, and never into the branch checked out here.
- The boot console has a "Sharing GitHub access" step that names the
  repository, and says when the App isn't installed on it, with the
  install link. The HOST tab has a GIT section with the agent's branch,
  the tip last fetched here, the repository and the state of its token.

##### Changed

- A pull takes only the work done since the last pull, measured from a
  base the node keeps outside the agent's branch and index. A machine
  no longer commits the agent's whole working tree after each pull, so
  the agent's branch holds only its own commits.

##### Removed

- The Orbit tools (`orbit_*`), `ORBIT_URL` and the `x-token` push path.
  `git_push` and `git_commit_push` push to `origin` with git's own
  credentials and no longer take a `remote_url` or a JWT.

### An agent's chat opens quickly

##### Fixed

- Opening an agent whose chat isn't held yet puts its chat on screen
  much sooner. The session reads the agent's newest record entries
  once for its chat, its plan and its model conversation, where it
  read them three times, and rebuilds the chat while the agent's
  session opens and the agents list is laid out, all away from the
  loop that handles the UI. In a development build the record work of
  a first open went from about 110 ms to about 25 ms for a thousand
  entries.
- The Record panel, the Plans and TASKS tabs, the plan strip and the
  TRACE tab fill in after the chat, side by side. Clicking through
  agents no longer waits on each one's long record or full task folder,
  and switching away stops filling the panels of the agent left.
- The Plans tab reads the task folder once for all plans instead of
  once per plan: with 20 plans and 600 tasks it takes about 40 ms
  instead of 380 ms in a development build.
- A chat that arrives before it would say it is loading (150 ms) shows
  at once instead of fading in; only a slower load fades in.
- A record the agent writes while its Record panel is being read stays
  in the panel.

##### Changed

- Development builds optimize `aura-store-db` and `aura-core-types`
  (`opt-level = 2`), where record entries are decoded: reading a
  thousand of them takes about 8 ms instead of 30 ms.

### Benchmarks have their own app

##### Changed

- BENCH is an app of the agents panel, after AGENTS (Alt+4), instead of
  a tab of the chat panel. Its list shows every benchmark of every
  project, newest first, with what each is about and how long ago it
  started, and `+ New bench` at the bottom. The chat panel shows the
  benchmark picked in the list, or the form for a new one, with no
  agent on screen: the agent's side column, queue and todos are hidden,
  and the input line runs commands but sends no message. Esc goes from
  a benchmark or the form back to the list, and an agent coming on
  screen that BENCH does not show brings PROJECTS back. `/bench` and
  `/bench new` work as before.
- A benchmark in BENCH's list unfolds, with Space or a click on its
  arrow, over the agents that work for it: each arm's agent over its
  runs, and the subagents each run spawned, with a spinner on those at
  work. Enter or a click on one shows its chat in BENCH with all its
  tabs and its side column, and what you type goes to it; Esc on CHAT
  goes back to the list. A benchmark the form starts unfolds as its
  agents come.
- The new-benchmark form has a From row at the top that picks, with
  ←/→, the top-level agent the benchmark starts from, named with its
  project. It starts on the tree last on screen. The benchmark copies
  that agent's workspace and offers its last prompt and sessions.
- `L` on a benchmark's run shows the run's agent on its LOG tab in
  BENCH, under the benchmark, and `d` brings PROJECTS back with the
  run's diff in the chat of the agent the benchmark started from.

### Plans read like a document

##### Changed

- Opening a plan now shows it as a document to read instead of its raw
  Markdown file. This works from the TASKS tab, from a `create_plan` or
  `read_file` step in the chat, and from the sandbox tree. The view
  shows the title, the overview, the todos with their check boxes and
  progress, and then the plan's body set like an agent's reply. The
  frontmatter and the generated-header marker are hidden. In the
  desktop app the text is set in the proportional face, like chat
  replies, with inline code kept in monospace. Opening a plan at a todo
  scrolls to that todo, and `s` shows the source file.
- Plans are written as a short document to read top to bottom instead
  of a checklist of identifiers. The agent is now asked for section
  headings that say what each part is about, an opening paragraph of
  context, plain sentences and full-sentence bullets. Code formatting
  is kept for file paths, commands and code names, and tables, code
  blocks and diagrams appear only where they help. Both the plan-mode
  prompt and the `create_plan` tool description carry this guidance.

### Typing stays quick while a long turn runs

##### Fixed

- Typing in the chat input no longer lags while the agent works through
  a long turn. The running group of steps was laid out from scratch on
  every frame, parsing each call's arguments (hundreds of KB for a file
  write) and diffing each edit several times over; a key in a turn of
  300 steps took about 28 ms to show in a release build. A step that is
  done is now laid out once and kept until it changes, which brings that
  to under 1.5 ms.
- A tool call still running, such as a file write whose content is
  streaming in, no longer parses its arguments and diffs its edit again
  on every frame: those are worked out once each time the call changes,
  and only its header, with the spinner and the seconds, is redrawn. A
  call's arguments are parsed once for its label, diff and size instead
  of three or four times.
- Sizing the input box no longer styles all of its text a second time
  each frame, so a long pasted input stays quick to type into.

##### Changed

- Development builds (`cargo build`, `cargo run`, `cargo test`) optimize
  the dependencies (`opt-level = 2`, built once and then cached) and the
  two UI crates (`opt-level = 1`). Unoptimized, a key and its frame in a
  long session took 10 to 25 ms, and a frame drawn only to turn the
  spinners about 11 ms, many times a second; they now take about 2 ms.
  The first development build after this rebuilds every dependency once.

### Code blocks keep their text inside the border

##### Fixed

- A code line wider than its box wraps onto the rows below inside the
  border instead of running over it. Widths count display columns, so
  tabs, accented and wide characters no longer push the right border
  out, and a pane too narrow for the language label draws a plain top
  border.

### BENCH compares Gate with Opus 5.5 by default

##### Changed

- A new benchmark on the BENCH form runs every task of SWE-bench Pro
  Hard once per arm, Gate (arm A, which routes each turn by its
  difficulty) against Opus 5.5 (arm B). Arm B names the Opus 5.5 you can
  run, on your Claude plan first, then any other route such as
  OpenRouter's `claude-opus-5.5`. With none, the server's strongest Opus
  stands in, so the default form never fails for lack of a Claude plan.
  ←/→ on arm B move on from Opus 5.5 to the other models and back.

### Empty panel messages fit on one line

##### Changed

- Every empty panel says so in one muted line, indented two columns,
  instead of a headline followed by wrapped explanation lines. SANDBOX,
  PROCESSES, TRACE, PLANS, TASKS, KERNEL, MEMBERS, POLICIES, INBOX, the
  ARCHIVE list, the graph overlay, the flow canvas and the chat welcome
  all share the same line, cut with an ellipsis on narrow panels rather
  than wrapped. A tab whose items belong to other agents still points at
  the `*` button, now on the same line.

### The classifier assesses the work, and Gate scores its difficulty

##### Changed

- The classifier no longer picks a difficulty. Its `label` tool asks for
  a short analysis first, then how much code the work changes (one spot,
  one file, a few files, many files), how precisely the request is
  specified, how much exploring it needs, whether it gives a way to
  check the result, and which traps it holds (dates and times, money and
  numbers, text encoding, concurrency, security, parsing, compatibility,
  performance). Gate adds these into points: 2 or fewer is easy, 6 or
  more is hard, anything between is medium. Its prompt now judges the
  work rather than the writing, so a long or carefully written request
  is no longer hard for that alone, and the request's length is no
  longer sent to it.
- A comparison classifies each prompt once, on its own agent, and both
  arms share the label, so they are routed on the same assessment. Like
  the judge's calls, the classifier's are in neither arm's costs:
  `/compare show` gives them a `Classifier:` line of their own beside
  `Judge:`, and the BENCH tab a `classifier` line beside its `judge`
  one, once the comparison ends.

##### Added

- `/compare show` follows a run's category and difficulty with what
  drove it, such as `debugging/hard (a few files, loose spec, deep
  exploring, checkable, traps: concurrency)`, and the BENCH tab's runs
  table has a `why` column with the same. A prompt's record keeps the
  full assessment, analysis included.

### Benchmarks' tokens say what each call was for

##### Changed

- A benchmark's tokens table gives the calls made beside a run's own
  work rows of their own, named for what they were for, such as
  `gpt-6.1-terra · criteria` and `sonnet-5-5 · judge`; `/compare show`
  names them the same way. They were added into their model's row, so
  a Gate arm listed a model neither arm named, the cheapest
  standard-tier model writing its turns' acceptance criteria, with no
  reason given, and Gate's judge calls on the work's own model counted
  as that work. Runs that finished before keep one row per model.

### Benchmarks pick their classifier

##### Added

- A benchmark names the model that classifies its runs' prompts. The
  BENCH tab's form has a Classifier row under the arms: `auto` keeps
  Gate's choice, and ←/→,
  Enter or typing picks another in the `/model` selector, as on a
  difficulty's row. `/compare --classifier <model>` does the same. A
  model you cannot run now is refused with the models you can, the
  estimate shows the classifier, and the benchmark's header and
  `/compare show` name it.

##### Changed

- Gate's classifier is a fast model on a linked Claude or ChatGPT plan
  before a cheaper one paid for by the token: a plan's calls cost
  nothing more. A local fast model still comes first. With a Claude
  plan and OpenRouter both linked, prompts were classified on
  OpenRouter's `gpt-5-nano` rather than the plan's Haiku.
- A pinned arm's run whose prompt cannot be classified goes on, on its
  model, unclassified, as a turn on a pinned model outside a benchmark
  does; it failed at once. It leaves no outcome for Gate, having no
  category to count it under. Gate and by-difficulty arms still need
  the label to route, and their runs still fail without one.

### OpenRouter reasoning replies parse

##### Fixed

- Every benchmark run failed at once with "Gate could not classify the
  prompt" when the classifier was `openrouter/openai/gpt-5-nano`.
  OpenRouter's Anthropic-compatible endpoint answers a reasoning model
  with a `redacted_thinking` block (its encrypted reasoning) before the
  `label` call, and the reply failed to parse. Such a block is now read
  and dropped, in whole replies and in streams, so the label is found.
  A whole reply with a block type the client does not know keeps its
  other blocks rather than failing.
- A benchmark run whose prompt could not be classified was listed under
  the session's model, a model of neither arm. It is listed under its
  arm's: the pinned model, or `gate` or `by difficulty`.

### Gate in crates of its own

##### Changed

- Gate's code is a layer of its own under `crates/gate`, a crate per
  job behind the `aura-gate` facade: the evidence a turn leaves, the
  routing, the follow-up, the judges, the attempt log, `/compare`, the
  lead and Gate's live state. No function in it is over 50 lines and
  no file over 500, and clippy and a test keep it so. Tests pinned
  what Gate records, shows and sends against the old code, and it is
  unchanged but for the entries below.
- `/compare` asks the judge about up to four pairs at once, as many as
  runs may work at once, rather than one after another. The judgments
  keep the pairs' order.
- A turn of `aura -p` that Gate routed records its outcome and its
  attempt line once, after Gate's follow-up, as the terminal's turns
  do. A passed check the judge then found wanting used to leave both a
  `checks_passed` and a `judged_fail` on the record.
- A `/compare` Gate run whose route turned your sign-in down goes on,
  on Gate's next choice, in the same conversation, as a turn in the
  terminal does; it stopped with the API's error. Its later prompts
  leave that model out and, in a comparison that learns, no longer
  explore. A pinned arm's run still stops.

##### Fixed

- Two records an agent made at once, such as the answers to two
  `reason` calls, could take the same sequence number; the second then
  failed and was lost. Each now gets a number of its own.
- `aura gate replay` matched each attempt it read with its line in the
  log by position, so after an attempt it could not read, a line could
  be written again for nothing, or not written when its run showed
  more. Each attempt is now matched with its own line.

### Scrolling stays smooth while many agents work

##### Fixed

- Agents working in chats that are not on screen no longer redraw the
  desktop window or the terminal UI. With 30 agents streaming behind a
  long chat left alone, the desktop window's thread was busy about
  730 ms of every second drawing frames like the one on screen; it is
  now busy about 30 ms. Scrolling that chat with a touchpad meanwhile
  costs about 80 ms of every second, down from 130–160 ms, and each
  frame takes about half as long.
- The desktop window shows its frames on the GPU from a thread of its
  own, so waiting for the display's refresh no longer holds up input or
  the agents' updates, and it starts no more frames than the display
  shows. The agents' updates are taken in on a thread of their own,
  and an editor opened from the window is waited for on another.
- On Linux, agents run 10 nice levels below the UI: the threads that
  run them and the builds, tests and tools they start, so a busy
  machine no longer starves the window or the terminal UI.
  `AURA_AGENT_NICE` sets how many levels (0 to 19; 0 leaves them as
  they are).
- A long chat costs a frame only what is on screen: a turn's footer is
  formatted only when its block is laid out again, blocks above the
  screen are stepped over by their height, and drawing stops once the
  screen is full. A streamed chunk and its frame after 2,000 messages
  take 1.4 ms in the desktop window, down from 1.8 ms. Lists of agents,
  projects and processes that arrive unchanged are dropped without a
  redraw.
- A long chat is laid out across up to eight cores when the window is
  resized or the chat is opened. In the desktop window, a chat of
  2,000 messages takes 60–70 ms to lay out at a new width, down from
  160–200 ms, and its first frame about 70 ms, down from about 200 ms;
  in the terminal UI its first frame takes about 70 ms, down from
  180 ms.
- Without a GPU, the desktop window paints each frame in bands across
  up to eight cores.
- Agents in chats that are not on screen send their streamed text to
  the UI five times a second rather than thirty, and the UI's queue of
  updates from agents holds 4,096 rather than 200, so a burst from many
  agents doesn't hold them up.
- The desktop window finds the rows and text it draws in its caches
  with a faster hash.
- Agents write their record entries without waiting on each other's
  writes, and an append no longer reads back and decodes the 50
  entries before it to get their context hashes.

### Benchmark arms with a model per difficulty

##### Added

- A benchmark arm can run work on a model chosen by its difficulty: in
  the BENCH tab's new-benchmark form, set an arm to `by difficulty` and
  pick a model for its Easy, Medium and Hard rows. Enter on a row, or a
  click on it, opens the `/model` selector, with its Ready / Sign in tabs and
  type-to-filter, and puts the model picked there on the row without
  changing the session's model; typing on the row first opens it
  filtered by what was typed, and ←/→ cycles the models in place. Each
  model may come from any route: a linked Claude or
  ChatGPT plan, the cloud router, OpenRouter or a local model.
- The model selector takes the mouse: a click on a model picks it, as
  Enter does, a click outside it closes it, and the wheel moves the
  highlight. Rows of the new-benchmark form can be clicked too; a click
  on the last row asks for the estimate, or starts the benchmark.
- `/compare --a` and `--against` take the same arm as
  `difficulty:easy=<model>,medium=<model>,hard=<model>`.
- Gate's classifier rates each prompt and subtask of such an arm's runs,
  and the work runs on its difficulty's model (the medium one when it is
  unrated) at the effort Gate would give that difficulty. It is not
  followed up, reviewed or moved to a stronger model, so the results
  show what the chosen models do on their own.

##### Fixed

- A benchmark arm pinned to a model Gate does not route to, such as an
  OpenRouter model without a curated price or a second route of a model
  linked on a plan, is no longer rejected when the benchmark is set up.

### INBOX tab in the Kernel panel

##### Added

- The Kernel panel (top right) has an INBOX tab after POLICIES. It lists
  the messages the agents of the run on screen send each other, threaded
  by subject, the most recently active thread first. A thread row shows
  an unread dot, a fold arrow, the subject, its message count, who took
  part and the time of its newest message; ←/→ fold and unfold it to
  list its messages. The header reads `2 NEW` while threads are unread,
  otherwise the thread count.
- `send_to_agent` takes an optional `subject`; messages that share a
  subject are threaded together.
- Clicking a thread or a message, or pressing Enter on it, opens the
  conversation over the chat (`THREAD <subject>`) as a stack of
  foldable messages: `n`/`p` move between them, Enter or Space folds
  one, `e`/`c` unfold or fold them all, and Esc closes the view.
  Opening a thread marks it read.
- `/inbox` (also `/mail`) opens the tab.

### Model aliases

##### Added

- `/model <name>` and `--model <name>` (`aura -p`, `aura runs`,
  `aura eval run`) accept short names and pick the best match you can
  run: `opus` the newest Opus, `deepseek` the best DeepSeek, `sonnet
  4.5` that version, `openrouter opus` that route. Models your
  sign-ins allow come first, then releases over previews, the stronger
  tier, the newer version and the current model's route. Exact ids
  (`claude/claude-opus-4-7`) and full model names (`claude-opus-4-6`,
  `aura-claude-opus-4-7`, `gpt-5.4`) are used as given. The success
  message names the alias, as in `Model: claude/claude-opus-4-7
  (claude plan) — matched "opus"`; a match that needs a sign-in says
  which `/login` to use.

### OpenRouter sign-in

##### Added

- `aura login openrouter` (`/login openrouter` in the TUI) links your
  OpenRouter account through the browser (OAuth PKCE, localhost
  callback), which returns an API key you own. `--paste` (`/login
  openrouter paste`) works on headless machines and also accepts an
  existing `sk-or-…` key. `aura logout openrouter` unlinks it. The key
  is kept in the same vault as plan links, so it survives restarts, and
  belongs to the active identity (local or zOS).
- `openrouter/<vendor>/<model>` models (for example
  `openrouter/anthropic/claude-sonnet-4.6`) go to OpenRouter's
  Anthropic-compatible Messages API and are billed to your OpenRouter
  credits. The account's tool-capable models are loaded from
  `GET /v1/models` and cached in `AURA_HOME/openrouter_models.json`;
  their context windows, output limits and prices come from that list.
- `AURA_OPENROUTER_URL` (default `https://openrouter.ai/api`) sets the
  API base. Values that are not HTTPS and not loopback are ignored.
- New crate `aura-auth-openrouter` with `OpenRouterProvider`.
- The reply footer says who paid for the turn's tokens on the same
  line as the model: `[claude-opus-4-7 · 1.6M tok · $0.998 · via Claude
  plan]`, or each sign-in's share in place of the totals when several
  paid, such as `[m · via Claude plan 25.1k tok · OpenRouter 2.8k tok
  $0.004]`. `/cost` adds a "By sign-in" breakdown.
- Each model call's record entry has `route`, the requested model id
  with its route prefix.

##### Changed

- `/model` has two tabs, "Ready" and "Sign in". Tab or ←/→ switches
  tabs, typing filters by id or label, Backspace edits the filter and
  Esc clears it, then closes. The list scrolls. j/k/q no longer
  navigate.
- Gate routes to OpenRouter models only when their list price is known.
  A model reachable more than one way goes to a linked plan first, then
  aura-router, then OpenRouter.
- One chat can switch providers turn by turn: each provider gets only
  the reasoning blocks it produced, and tool-call ids are normalized.

### To-do boxes are bigger and match in every list

##### Changed

- The desktop window draws the to-do boxes (`☐ ☒ ▣`) larger, with a
  larger cross and fill inside, so they read at a glance.
- The PLANS and TASKS tabs draw their todos like the checklist that
  follows the conversation: `☒` done and faded, `▣` in progress in bold
  white, `☐` pending in gray, and cancelled ones struck through. They
  used to show `✓ ▸ ○ ✗` in the status colors.
- The to-do boxes line up under the icon above them: under the `⬢` of
  the checklist that follows the conversation, under the plan's fold
  arrow in PLANS, and under the task's icon in TASKS. They used to be
  indented two columns.

### POLICIES tab in the Kernel panel

##### Added

- The Kernel panel (top right) has a POLICIES tab next to KERNEL and
  MEMBERS. It shows the policy that the kernel of the agent on screen
  enforces. STATUS covers the work mode, the approval level, whether
  commands can be confined to the folder, whether `ask` calls can
  prompt you, permission hooks, accepted action kinds and the policy
  hash. CAPABILITIES lists the capabilities the agent holds and its
  scope. TOOLS shows each tool the agent is offered as on, ask or off,
  with the rule that decides it. ALWAYS ALLOW lists the project's rules
  (flagged as ignored at `strict`), SESSION GRANTS the answers
  remembered this session, and INTEGRATIONS what is installed. The tool
  states are worked out before a call's arguments, so checks that
  depend on paths can still decide a real call differently.
- The header shows the agent's `MODE/APPROVAL`, in the accent color
  under `yolo`. Every highlight on the tab (section titles, warnings,
  and the on/ask/off marks) uses the theme's accent color. The view
  refreshes when the agent comes on screen, when
  its mode or approval level changes, after an "always allow" answer,
  and whenever the tab opens.
- Open the tab with `/policies` (also `/perms`), or press
  Shift+Left/Right, which now cycles through all three tabs. Up/Down,
  PgUp/PgDn, Home/End and the wheel scroll it.
- The kernel's `Policy` gains `explain_tool` (a dry run of the tool
  gates that records nothing) and `session_tool_states`. `Kernel` gains
  `has_approval_prompter` and `permission_hook_count`.

### Enter again after queueing a message sends it now

##### Fixed

- In the terminal UI, pressing Enter on the empty prompt right after a
  message went into a busy agent's queue now sends that message at once,
  cancelling the running turn. Before, this Enter did nothing if the
  event loop had not yet sent back the queue, or if a panel other than
  the chat had the keys. Only the next key counts: any other key in
  between falls back to the old rule, where Enter on an empty chat
  prompt sends the oldest queued message.

### BENCH shows each arm's total time

##### Added

- A benchmark in the BENCH tab has a `total time` row under `time`, the
  median run: each arm's run times added up, runs under way included,
  as `cost` adds up what they spent. Up to 4 runs work at once, so an
  arm's total can be more than the time since the benchmark started.
  Times of an hour or more read `1h05m` rather than `65m00s`.

### Gate does not nudge a turn that ran its change itself

##### Changed

- A routed code turn that, after its last change, ran the changed code
  itself and saw it succeed is no longer asked to check its change: a
  script that imports what it changed, or a module it read, and prints
  or asserts what it returns; the changed script run on a few inputs;
  or a syntax check such as `python -m py_compile`, `python -m
  compileall` or `sh -n`. Such a run still counts as no check, so the
  harness still runs the project's tests where it finds them and the
  judge still gives the verdict. All 16 nudged runs in Compare #34 and
  #35 had done this before they were nudged. In Compare #34 the work
  after the nudge cost $0.147 of the Gate arm's $0.621 and changed no
  outcome. A command that rewrites a changed file (`sed -i`, `> file`)
  counts as another change, not a check.

### The model doing the work sees its acceptance criteria

##### Changed

- With `AURA_GATE_CRITERIA=log`, the default, the model doing a routed
  code turn is now shown the acceptance criteria after your prompt, as
  it was only with `on`, and told to make sure each holds and that
  something it ran shows it. The record keeps the prompt as typed. The
  review against the criteria stays logged only: replayed over 43
  benchmark attempts with Opus as reviewer (`aura gate replay
  --criteria`), it called 6 of the 8 wrong attempts unfinished, both
  wrong Haiku attempts at ansible-b748edea from Compare #29 and #32
  among them, and 16 of the 35 right ones. Sonnet 5.5 as reviewer
  caught all 8 and called 17 of 39 right ones unfinished. Acting on
  either would send back about two good attempts for each bad one.
  Showing the criteria has not been measured on new runs yet.

##### Fixed

- A criteria writer's answer given as a string of JSON (`items` holding
  the list as text, or the whole answer as text), as a bare list, or
  with a kind spelled `behavior` or `exact-output`, is read. It used to
  count as no criteria, without a word.
- When the writer's reply holds no criteria, the chat says why (`gate:
  claude-sonnet-5-5 (claude plan) wrote no acceptance criteria: its
  reply called no tool and holds no JSON object`). Only a failed call was reported before.

##### Added

- `aura gate replay --criteria [--writer <model>]` writes the
  acceptance criteria of each replayed request (once per request, kept
  in `gate/replay_criteria.json`) and reviews the attempts against
  them, logged as `plain+criteria`. `aura gate eval` now scores reviews
  per reviewer as well, so two reviewers asked the same way can be
  compared.

### A standard model keeps its work until starting on it costs more

##### Changed

- A standard-tier model (Sonnet) that falls below `AURA_GATE_TARGET`
  on a kind of work no longer hands all of it to the frontier tier. It
  does so only when starting there has also become the cheaper way: a
  turn on it costs at least what a turn on the stronger model does,
  times the share of its turns that succeeded. Sonnet at $0.35 a turn
  beside Opus at $1.00 keeps the work at 4 successes in 10 and loses it
  at 3. A fast-tier model is still sent up by the target alone, since
  its own tests pass on work it got wrong.
- A model is judged on its turns of the same difficulty once one has a
  known result, else on the category's. Two failures on hard work used
  to send a category's medium work to Opus too.
- A failed attempt still moves up to the stronger model, so this
  changes where work starts and not whether it can reach Opus.

### A failed check on the strongest model is not run again at more effort

##### Changed

- A turn on the strongest model that ends in an error or with its own
  last test run failing now ends there. It used to go on on the same
  model one effort higher, up to `max`, under a prompt for a stronger
  model taking over. In Compare #33 that second pass cost $1.93 on a
  task Opus had already finished correctly for $0.48: its last run was
  a wide suite in which 27 tests fail with or without the change. Work
  a judge found unfinished on the strongest model still goes on there
  one effort higher, and is now told it is the same model going over
  its own work.

### A test run with the change stashed is not the change's check

##### Fixed

- A test or build run between `git stash` and `git stash pop` (in one
  command line or across calls) ran the code as it was before the
  change, so Gate no longer takes it for the turn's check. When that
  run shows failures, a failing run of the same check with the change
  in place is not counted either, as those failures were there before.
  Models do this to compare failures before and after, and Gate read
  the comparison as `checks_failed`.

### Gate's logged reviews run on a quarter of turns

##### Changed

- A review whose verdict is only logged (`AURA_GATE_REVIEW` or
  `AURA_GATE_CRITERIA` at `log`, the default) is now asked on a sample
  of routed turns, a quarter by default, instead of on every hard turn
  and every turn with acceptance criteria. Such a review changes
  nothing about the turn it costs a call on, and a sample still shows
  how often the reviewer is right. A review Gate acts on (`on`) is
  asked every time, as before.

##### Added

- `AURA_GATE_REVIEW_SAMPLE` (0 to 1, default `0.25`): the share of
  routed turns whose work gets a review that is only logged.

### Gate's closing verdict on Sonnet's work runs on Sonnet

##### Changed

- Work no check vouches for is now judged by the model that did it,
  when that is a standard model or stronger, and by the cheapest
  standard model (Sonnet) when a fast model did it; so is the review
  of work about to stand. Both ran on the strongest Opus, at twice
  Sonnet's price, whatever the work ran on: in Compare #34 Gate's
  checking of Sonnet's work cost more than the work itself and changed
  no outcome. Opus's unchecked work is judged by Opus as before, and
  whether a failed check was the model's fault is still the strongest
  model's call.

### Gate's nudge leaves the last reply a full answer

##### Changed

- When Gate asks a model to check a change it ran no test or build
  for, the model is now asked to end with its whole answer again (what
  it changed and why, what it ran and how that went), not only with
  what it ran: that last reply is the one the user reads and the one
  `/compare`'s judge weighs, and as a test report it lost comparisons
  the work itself had not. Where no test covers the change, it is
  asked to run the build or the changed code on a few cases from the
  request, and not to add test files to a project that has none, a
  change nobody asked for. Work Gate sends back after a review is
  asked for the whole answer in the same way.

### `aura -p` counts Gate's own calls in a run's cost

##### Fixed

- `total_cost_usd` in `aura -p`'s JSON result now includes the calls
  Gate makes for the run's turns: classifying each prompt, writing
  down what code work is checked against, and judging and reviewing
  the work. It left them out, so a run Gate routed read cheaper than
  it was. `usage` still counts the agent's own calls only, and so does
  `--max-cost-usd`.

##### Added

- `gate_cost_usd` in the JSON result: the part of `total_cost_usd`
  that Gate's calls cost, `null` when Gate made none with a price.

### `aura -p` keeps the work's difficulty after a nudge or a move up

##### Fixed

- In `aura -p`, the attempt that runs the check Gate asked for, and
  the attempt on a stronger model after one failed, carry the
  difficulty the classifier gave the work, as they do in the TUI. They
  had none, so hard work whose check passed only once Gate asked for
  it was not reviewed as hard work is, and their lines in Gate's
  attempt log had no difficulty.
- In `aura -p`, the attempt that checks a change is led, or not, as
  the turn that made the change was. It used to decide that again,
  from turns of every difficulty.

### A run's cost reward is counted in dollars

##### Changed

- The `cost` reward of a run is now each model call's list price
  (`usd`) and the run's wall time. It counted tokens before, which
  weigh the same on every model: on a SWE-bench task Gate's Haiku run
  scored -0.55 on cost to Opus's -0.45, though it cost 22 times less.
  Tokens are a reward kind of their own, `tokens`, listed apart by
  `aura runs show`. A call to a model with no known price gets no
  `usd` reward. The labeler's source is `cost/v2`.

### Gate's own calls are in the run log and its cost

##### Fixed

- A run's `summary.json` and `log.json` now hold the calls Gate makes
  for it: classifying the prompt, writing down its criteria, and
  reviewing or judging an attempt. Each is a `model` step by
  `harness/gate` that names its purpose, and a failed one its error.
  They are priced like the agent's own calls, so a run's `cost_usd` is
  what `/compare` shows for it. Before, the log left them out, and a
  criteria writer that failed on every call left no trace in it.
- A compare run's turn starts in its trace before Gate classifies the
  prompt, so Gate's calls for the prompt land in its turn, not the one
  before.

##### Added

- Where Gate sent a prompt's work is a `gate/route` decision in the
  run: the model and effort, the category and difficulty, the reason,
  and whether Gate explored. Each prompt Gate sends within a turn (a
  nudge, a send-back, a stronger model's takeover) is a prompt step of
  that turn, so the log shows what the model was told between attempts.
- The record entry of a failed model call names the model and what the
  call was for, as a successful call's does.

### A piped test run passes only when its output shows it passed

##### Fixed

- A test or build whose output was piped on (`pytest | tail -3`), or
  that another command followed whatever it did, no longer counts as
  passed because the command exited 0: what it printed decides, as it
  already did for a failing exit. Failures fail it, and so does output
  showing that the tests never ran ("no tests ran", "ERROR: file not
  found", "No module named", "command not found") when no test passed.
  Output showing neither a failure nor a pass does not count as a
  check. On a SWE-bench task Claude Haiku 5.5 had two such runs counted
  as passed: one whose import failed and one that found no tests.
- A test run with a stray zero-width character in its command
  (`python\u200b -m pytest`), which `run_command` runs without it, now
  counts as a check.

### Gate notices more admitted gaps and work checked by its own tests

##### Changed

- More ways of admitting a gap in a final reply count as the work not
  being verified: "still unchecked", "do not compare against", "did not
  compare", "not pinned down", "an open question", "did not run it" and
  the like. On a SWE-bench task Claude Haiku 5.5 said all of these after
  its own 14 tests passed, and Gate let the work stand. Gate now has the
  judge look at such work.

##### Added

- A new sign of a failed attempt, `own_tests_only`: the last check
  passed, but ran only tests in files the attempt wrote itself. `aura
  gate eval` scores it, and like the other signs it starts reviews once
  the attempt log shows it is worth one. Attempts log the files they
  wrote from now on, and `aura gate replay` adds them to replayed ones.

### Gate's classifier reads more of a long prompt

##### Changed

- The classifier reads the first 12,000 and the last 4,000 characters
  of a long prompt, up from 3,000 and 1,000, so what makes work hard
  in the middle of a long task (an output that must match a reference
  byte for byte, say) reaches it. On Haiku 5.5 the longest excerpt
  costs about $0.0004 more per prompt.

### Exacting acceptance criteria make work hard by default

##### Changed

- With `AURA_GATE_CRITERIA=log`, the default, criteria that ask for
  output to match a format or a reference exactly now rate the work
  `hard`, as they did with `on`, so it starts at `high` effort and
  never below the standard tier. The worker is shown the criteria, and
  the review acts on them, only with `on`.

### Gate's acceptance criteria get written on Sonnet 5.5

##### Fixed

- Gate never wrote the acceptance criteria of a code turn when the
  writer was Claude Sonnet 5.5, the cheapest standard-tier model on a
  Claude plan: the request forced the `criteria` tool, which Sonnet 5.5
  rejects (`tool_choice: type "tool" and "any" are not supported for
  this model`). Every call failed within a quarter of a second and the
  work was reviewed without criteria. Sonnet 5.5 and Opus 5.5 are now
  asked with `tool_choice: auto` (criteria in JSON text count too), and
  the Anthropic provider sends any forced tool choice to them as `auto`.
- A criteria call that fails says so in the chat, `gate:
  claude-sonnet-5-5 (claude plan) wrote no acceptance criteria: ...`,
  instead of only in the log.

### Gate starts medium and harder work on Sonnet, and Haiku thinks on code

##### Changed

- Only work the classifier found `easy` starts on the fast tier
  (Haiku). Work rated `medium` or `hard`, or not rated, starts on the
  standard tier (Sonnet) or above, small changes included, and does
  not go below it when a fast model has proven itself on the category
  or when Gate explores a cheaper model. In Compare #32 Gate sent a
  multipart-form task the classifier rated a `medium` small change to
  Haiku 5.5, which passed tests it wrote for its own code while the
  benchmark's check failed; Opus got it right.
- On the fast tier, Gate runs code work (`small_change`,
  `multi_file_change`, `debugging`) at `medium` effort or above, and
  other work with thinking off (the route line says `with thinking
  off`). Work that ran with thinking off moves up to a stronger model
  at `medium`.
- `/gate` shows each category's pick for work that is not easy and,
  when easy work goes to another model, marks that one `pick for easy
  work`; a fast model proven on a category shows up there.

##### Fixed

- Haiku 5 thinks at the effort a request asks for: it is sent
  `thinking: {"type": "adaptive", "display": "summarized"}` and
  `output_config.effort` (`low`, `medium` or `high`; `xhigh` and `max`
  ask for `high`). It used to be sent neither, so Gate's effort never
  reached it and its run logs showed no thinking: none in any of
  Haiku's 16 calls on that task. Effort `off` still sends
  `{"type": "disabled"}`.

### Gate stops spending on the strongest model when nothing failed

##### Fixed

- A check whose failing exit status a later command may have set
  (`pytest -q | grep -m2 Error` that matched nothing, `pytest; python -m
  pyflakes x` with pyflakes not installed) is decided by what the check
  printed: `12 passed` passes it, failures fail it, and output showing
  neither says nothing. Such a check used to count as failed, and on
  Opus that retried correct work at max effort: in Compare #31 two such
  retries cost at least $4.55 of Gate's $8.61, against $3.87 for Opus
  alone. The judge is told when a check's verdict came from its output.
  Failed tests or a judged failure on Opus still move it to max effort.
- Work on the strongest model is no longer nudged to check its change
  or checked by the harness: raising its effort is not a stronger model
  to go to. Only the judge looks at its unchecked work, as the docs
  say. The two nudges on Opus in Compare #31 cost $0.49.
- A zero-width space typed into a command (`python\u200b -m pytest`)
  is dropped before the command runs, instead of failing with `command
  not found` and costing a retry, as four calls in Compare #31 did.
  Zero-width characters between non-ASCII characters, as in emoji and
  some scripts, are kept.
- A run that carries on an attempt (a nudge, a send-back or a stronger
  model taking over) records a `thinking_effort` decision only when its
  effort changes, and the decision lists `xhigh` and `max` among the
  alternatives.

### BENCH runs under way show their route

##### Changed

- A run under way shows the model its prompt went to (instead of the
  arm, `gate`) and how the classifier rated it, as soon as it is
  routed; a move to a stronger model shows at once. What it is doing
  (`thinking 2m31s · call 4`) comes after the last column, in the room
  left on the line. It used to sit under `classified` and `difficulty`,
  where it read as their values, and it no longer widens a column, so
  the table holds still as it changes.

### Gate writes down a code turn's acceptance criteria before the work starts

##### Added

- Before a routed code turn starts, a model other than the one doing
  the work (the cheapest standard-tier candidate) reads the whole
  request and lists what the finished work must do that can be checked:
  exact output formats, references the output must match, errors,
  interfaces and behaviours. The classifier reads only the first 3,000
  and last 1,000 characters of a long prompt, so it can miss demands in
  the middle. This call reads up to 40,000, so the review no longer
  depends on the worker's own tests to notice them.
- The review of work about to stand goes through the criteria one at a
  time, saying for each whether it is met, unmet or untested, and a
  test that compares against output written out by hand does not count
  for a criterion that names a reference. The review line in the chat
  says how many were met (`criteria 3/5 met, 1 unmet, 1 untested`),
  and the attempt log keeps each criterion's result. Reviews given
  criteria are scored as their own variant, `plain+criteria`, in
  `aura gate eval`.
- `AURA_GATE_CRITERIA` (`on`, `log` or `off`; default `log`). With
  `log`, the criteria are written and work about to stand that gets no
  other review is reviewed against them, with the verdict only logged.
  With `on`, the worker is shown the criteria after its prompt, strict
  criteria rate the work hard so it starts on a stronger model, and
  work the review finds short of them goes back to its model once, then
  on to a stronger one. The call is shown as `gate:` in the turn's
  footer.

### Gate trusts passed tests when only a lint step could not run

##### Fixed

- A reply whose tests passed but that says a lint or format step could
  not run (`I couldn't run a lint check because pyflakes isn't
  installed`) no longer counts as unverified work or as giving up. Such
  a turn went to the judge, and on Opus a judged failure is retried at
  max effort: in Compare #30 that retried a correct run for 24 minutes
  and $1.84. A sentence that also names tests or a build still counts.
- The BENCH runs table shows a move that only raised the effort on the
  same model as moved up (`to opus-5-5 max`); it said `no` while the
  summary row counted it. Each run's turns now record the effort Gate
  ran them at and, when a move raised it, the one they started at.

### BENCH runs table shows difficulty and wastes less room

##### Changed

- The runs table of a benchmark shows the classifier's difficulty in
  its own column after `classified`. It used to share that column with
  the category and was usually cut off (`small chan…`).
- A run under way shows what it is doing across the columns it has no
  values for yet (classified through led), instead of in a column at
  the end of the row. The long activity text no longer cuts the task
  and classification of every other run.
- `moved up` and `led` are left out when no run moved up or led.

### Each agent keeps its chat panel tab

##### Changed

- The chat panel remembers its tab for each agent. Switching to an
  agent shows the tab it was left on (CHAT, TERMINAL, GRAPH, TIME,
  BENCH or LOG), and an agent shown for the first time opens on CHAT;
  the tab used to stay as it was for whichever agent came next. GRAPH,
  TIME, BENCH and LOG load the agent's own flow, benchmarks or log,
  and TIME keeps its selection. Opening another agent's log (`L` in a
  benchmark) still shows it on LOG, and a benchmark's run picked in
  the agents list while BENCH is open opens on its chat the first
  time, then on the tab it was left on.
- With text typed in the input line, an agent's tab comes back only if
  Enter still does the same with the text: under a typed message an
  agent left on TERMINAL shows CHAT, and under a typed command the
  terminal stays.

### Replies are as long as the model's API allows

##### Changed

- Every model request now allows the reply the model's API allows:
  128k tokens for Claude 4.6 and later and for GPT-5, 64k for Claude 4.5,
  and so on. Before, agent turns stopped at 16k tokens, subagents at 64k,
  and the gate's judges, the classifier, titles, memory and the
  automaton's spec and refinement calls at caps of their own, from 24
  tokens for a title to 32k for a spec. Models
  whose API only bounds the reply by the context window get 128k, and
  models the harness doesn't know get 32k. Local models keep 4k, since
  their reply shares a 32k window with the conversation. The
  `max_tokens` field of a runtime request's model selection, the
  subagent budget's `max_tokens` and `AURA_SUBAGENT_DEFAULT_MAX_TOKENS`
  are gone; older clients that still send them are ignored.

##### Fixed

- A reply that reached the output-token limit before it made a tool call
  was retried with the same reasoning effort, so it was cut off the same
  way again. Opus 5.5, which reasons at max, spent three 16k-token
  replies in a row planning one new module and ended the turn without
  writing it. Such a retry now reasons one step lower for the rest of the
  run and is told nothing it planned was carried out and to act now,
  writing a large file in parts. The two retries allowed now count
  cut-offs in a row, not over the whole run.

### BENCH lists runs in number order

##### Changed

- The BENCH tab listed a benchmark's runs in the order they finished,
  so `b-2` could come before `a-1`. Runs, finished and still running,
  now show in number order, arm by arm within a number, with a line
  between one number and the next.

### Passing tests that never ran the new code don't vouch for it

##### Fixed

- Gate took a routed turn's passing tests as proof of its work even
  when none of them called what the turn added: an attempt that added
  `prepare_multipart` and ran the module's existing tests stood as
  `checks_passed`, and its bug surfaced only in the task's hidden test.
  Now, when the change adds public functions, classes or types and no
  test file in the workspace names any of them, the turn counts as
  unchecked. Its model is nudged to test those names, with expected
  values taken from the request, then the harness check and the judge
  follow as for work with no check. A change that adds or edits a
  test is left alone.

### BENCH shows benchmarks that stopped running as stopped

##### Fixed

- A benchmark whose run ended without recording how (Aura quit, or its
  task failed) stayed `running` in the BENCH tab and `/compare` for
  good, and diff, apply and dismiss refused it as still running. A
  record that says running while nothing runs it now reads as stopped,
  with a note saying why, and a task that fails shows as stopped at
  once.

### Benchmark runs finish sooner and show what they are doing

##### Changed

- Sonnet 5.5 and Opus 5.5 reason at high effort when nobody picked one
  (it was max). At max, one call late in a long task thought for two
  minutes or more, and each call cut off before it ended started that
  thinking over.
- A model stream cut off after a minute or more is restarted one effort
  lower (max and xhigh go to high, high to medium, medium to low), so
  the same long call is not cut off again at the same point. A quick
  drop is restarted as it was.
- In a benchmark run, a read-only subtask such as `reviewer` reasons at
  medium effort unless its model or effort was chosen.
- Gate routes to a reasoning effort as well as a model. Work starts at
  the effort of its difficulty: low for easy work, high for hard work,
  medium for the rest. A failed attempt moves to a stronger model at the
  same effort, and only work that failed on the strongest model goes on
  there one effort higher, up to max. So max is used only once nothing
  less got the work right. A thinking level picked in `/model` still
  holds. The route line says the effort (`gate → claude-opus-4-7 (claude
  plan) at high effort · debugging, escalate`), and the prompt's route
  records it as `effort`.

##### Fixed

- A restart now says how long the attempt ran and what ended it, for
  example "Model stream interrupted after 2m 00s (transport closed
  before stream completed: missing message_stop); restarting the
  response at medium effort". The reason is in the run's steps, as the
  `stream_reset` decision.
- A subtask its caller waits for stops 30 seconds before the caller's
  `task` call gives up, so its run is closed with a reason rather than
  left open with its work lost. Three minutes before then it is told to
  reply with what it found and what it did not get to. A failed subtask
  is not retried when too little of that time is left, and time spent
  waiting for the workspace counts.
- The BENCH view's runs under way show what each one is doing and for
  how long: waiting for the model, thinking, writing or running tools,
  with the call number, how many times the call restarted, and what a
  `task` subtask is doing, for example "running task 8m12s › thinking
  1m40s · call 4 · restarted 2×". They showed only "working".

### Gate classifies long task prompts

##### Fixed

- Gate's classifier often failed on long task prompts, such as a
  SWE-bench issue or a pasted spec, and the run then failed at once with
  "Gate could not classify the prompt". A prompt that starts "Your task
  is to implement..." came last in the classifier's message, and the
  cheap model took it for its own task and started on the work instead
  of labelling it. The classifier now answers by calling a `label` tool,
  forced, whose arguments can only be one of the categories and one of
  the difficulties. Local models still answer in text.
- The prompt reaches the classifier the same way every time: line
  ends, control characters, trailing spaces and runs of blank lines are
  normalized; its size is stated; a long prompt keeps its first 3,000
  and last 1,000 characters around a note of how many were left out
  (it kept only the first 4,000); and its text sits between `<request>`
  tags the prompt cannot close, which the system prompt says hold text
  to label, never to follow.
- A reply without a label is asked for once more before the prompt
  fails, and the error shows the whole reply, tool calls included.

### A MEMBERS tab next to KERNEL

##### Added

- The Kernel panel at the top right has two tabs, KERNEL and MEMBERS.
  MEMBERS lists every agent the run of the agent on screen brought in:
  that agent first, as the run's lead, then each agent it started with
  `task` or `spawn_agent`, nested under the agent that started it, down
  every level. Each row shows the agent's name and the permanent agent
  or role it is an instance of, its state (working, idle, done, failed,
  stopped), and how it joined (`task` or `agent`) with how long it has
  worked. Agents stay listed after the Processes tab drops their rows,
  until `/new` starts another run. Enter or a click opens a member's
  process (its prompt, model, start and end), `o` opens its chat when it
  has one of its own, Shift+←/→ switches the tab, and `/members` opens
  it. The desktop window's Kernel bar draws both tabs.

##### Changed

- The Kernel panel's header is a tab bar. The record count, or the
  member count while MEMBERS is open, sits in an info cell after the
  tabs. Jumping to a record (a turn's records link, the run graph, the
  GRAPH tab) opens the KERNEL tab.

### A LOG tab with a run log to hand to a model

##### Added

- The chat panel has a LOG tab, next to BENCH, and `/log` opens it. It
  shows the agent's latest run as folding panels: the run's metadata,
  totals and problems, its files, each turn, and each step in it, with
  the model's thinking, replies, tool calls and outputs and the
  harness's decisions. Subagents are nested under the step that spawned
  them; on a subagent the tab shows that subagent and the runs under it
  only. The header shows where the log is stored, and `⧉ Copy log` (or
  `c`) copies the whole log as JSON. While the agent or a subagent
  works, the log reloads about once a second (`● live`), keeping open
  panels open and following the newest step when the cursor is on it.
- Benchmark runs are recorded like conversations, as runs of source
  `bench` with their arm, example task and check outcome, so each has a
  `log.json`. `L` in a benchmark opens the picked run's log, or the
  first run under way, live while it works.
- The log of the agent that asked for a benchmark holds the
  benchmark's runs after its own turns (the one under way, else the
  newest), so its LOG tab shows them live instead of saying it has no
  run log. An agent with no run of its own gets a log of just those
  runs, written to `AURA_HOME/logs/<agent>/log.json`.
- Step rows of the LOG tab read at a glance: a model call shows its tool
  calls with their key argument (`read_file src/x.rs`) or its reply's
  first line, each tool result sits under its call (`↳`) with a short
  summary (`12 entries`, `exit 0 · 3 lines`, `no matches`) instead of
  raw output, tokens, cost and time line up in columns, and only
  failed, flagged or running steps are marked. A title too long for the
  header goes on over a second line, and a turn no longer repeats the
  prompt the run is titled by. `⧉ Copy log` and `c`, `y` and `p` copy
  in the desktop app and the terminal, and the button flashes
  `✓ Copied`; clicking it used to copy nothing.
- Every run bundle has a `log.json` (schema `aura.run_log/v1`),
  rewritten after each turn: one redacted JSON document of the run and
  its subagents, meant to be pasted to a model reviewing where the run
  went wrong.

##### Changed

- The model's thinking text is kept in `steps.jsonl` at every trace
  level, not only in `full` blobs.

### The chat leaves room below its last line

##### Changed

- Once a conversation filled the chat panel, its newest line sat on the
  panel's last row, against the border and the input box under it, and
  each new line pushed the text up from there. The chat now keeps a
  quarter of its rows blank below the last line: a reply that reaches
  that point scrolls up with the room left under it, in the terminal and
  in the desktop window. The blank rows are not counted as lines below
  while scrolled up. The terminal pane's output still runs to the bottom
  row, as a terminal's does.

### Lists of fields read as records

##### Changed

- A list of fields, such as the `- Path: …`, `- Name: …`, `- Type: …`
  entries a task prompt lists its new interfaces with, was drawn with a
  blank row between every item when it was written with blank lines
  between them, so the fields of one entry ran into the next and labels
  looked like values. A list item that starts with a short label is now
  a field: its label is muted, the blank lines between two fields are not
  drawn, and where the first label of a run of fields comes back a blank
  row sets the next record apart. Each of these depends only on the item
  and the items above it, so a reply's lists don't jump as it streams in.
  Replies, prompts and the messages agents send each other are all drawn
  this way; blank lines around other items stay as written.

### BENCH: a running benchmark's numbers count the runs under way

##### Fixed

- A benchmark's detail view counted only the runs that had ended, so
  while its first runs worked it read `finished 0/0`, `cost free` and
  `no model calls` for each arm, even with dollars already spent; only
  the runs table below moved. `finished` now reads against the runs
  planned, with how many are working (`0/2, 2 working`), `cost` adds
  what the runs under way have spent so far, and the tokens table adds
  their tokens per model. Medians, cost per fix and the per-task cells
  still wait for runs to end.

### Quitting keeps what running turns did

##### Fixed

- Closing the desktop window, or quitting the terminal, while agents were
  working lost their running turns. The session aborted its event loop
  instead of letting it shut down, so a turn cancelled at quit never saved
  its session: the agent came back with only the prompts and replies the
  record holds, without its tool calls, and an agent still on its first
  turn came back with nothing of that turn's work. Quitting now cancels
  the running turns and waits for them to end and save, for a few seconds
  at most, before the process exits; the terminal gives the screen back
  first.

### Dragging the chat's scrollbar scrolls it

##### Fixed

- In the desktop window the scrollbar on the chat panel's right border
  belonged to the divider beside it: the pointer showed the resize arrows
  over it, and dragging it resized the panels. A divider now takes the gap
  between two panels and the border right of it, and leaves the border
  left of it, where the agents and chat panels draw their scrollbars, to
  the panel.

##### Added

- Dragging the chat panel's scrollbar with the left button scrolls the
  conversation or terminal, or the record, process or file shown over the
  chat, in the terminal and in the desktop window. Taken by the thumb the
  view stays put until the pointer moves; pressed on the track the thumb
  jumps to the pointer. At either end of the track the thumb shows that
  end, however long the chat is, so a chat dragged to the bottom follows
  new output again. A press on the scrollbar starts no text selection, and
  the rows a list over the bottom of the chat covers (attached images,
  `/todos`, the slash and `@` menus) stay that list's. The scrollbar also
  takes presses on up to two blank columns left of the line it draws, so
  it is wider to hit: three columns beside the conversation, two beside
  the terminal or a record, process or file, whose text comes closer.

### The footer under a reply says `gate:` only when Gate chose the model

##### Fixed

- Every prompt is classified, so Gate learns how each model does on each
  kind of work, including turns on a model the user chose. The footer
  under those replies marked the classifier's call `gate:`, as if Gate
  had picked the model: `[claude-opus-5-5 · gate: claude-haiku-5-5 ·
  73.4k tok · $0.034]`. On a turn whose prompt records a `pinned` route
  the call is now marked `classify:`. `gate:` stays for turns Gate
  routes and for its judges, and a reloaded chat shows the same footer.

### Gate reviews work before it stands

##### Added

- `AURA_GATE_REVIEW` (`log`, `on` or `off`; default `log`). Routed work
  that is about to stand on a model below the strongest tier, because
  its check passed or nothing judged it, can be reviewed by the judge's
  model first when the classifier found it hard or it raises a sign the
  attempt log backs. The reviewer goes through the request one
  requirement at a time against everything the turn changed, the
  commands and checks it ran and its final reply. With `log` the verdict
  is shown and logged and nothing else changes. With `on`, work found
  unfinished goes back to the same model once with the findings, is
  reviewed once more, and moves up to a stronger model if it is found
  unfinished again. This applies to the terminal, the desktop app,
  `aura -p`, `aura eval run` and `/compare`, whose summary says how
  often a run's work was sent back. How often the reviewer is right has
  not been measured yet, which is why the default only logs.
- Four more signs in the attempt log: `shell_edit`, `stale_check`,
  `reply_not_covered` and `no_test_changed`. `aura gate eval` shows what
  each sign has earned on the routed attempts below the strongest tier:
  log only, review (raised at least 5 times and wrong at least twice as
  often as attempts of the same difficulty) or acts (raised at least 10
  times and wrong 4 times in 5). Gate reads these tiers from the log, so
  a sign starts to send work to the reviewer when the record backs it
  and not before.
- `review` lines in the attempt log, and a reviews table in
  `aura gate eval`: how often the reviewer said unfinished, how many
  wrong attempts that caught, how many right ones it flagged and what
  the reviews cost.
- `aura gate replay` asks the reviewer about attempts already in the
  log that a benchmark task's check proved right or wrong, from their
  kept `/compare` copies and saved sessions, and logs the answers for
  `aura gate eval`. `--dry-run` lists the attempts with a cost estimate
  and calls nothing; `--thinking`, `--all`, `--task`, `--limit`,
  `--judge`, `--again` and `--jobs` choose what is asked and how.

##### Changed

- Gate's judges see everything a turn changed, shell commands' edits
  included, where it is known: a `/compare` run's copy against the state
  it started from, and for other turns (while reviews are not `off`) the
  git working tree against a snapshot taken when the turn started.
  Before, they saw only what the file tools changed.
- A `/compare` run's `truth` line in the attempt log says where the
  run's copy is kept and the git tree it started from. Lines already
  logged are written again with it at the next start.

### Gate's attempt log

##### Added

- Gate logs each attempt and what later showed its work right or wrong
  to `gate/attempts.jsonl` under the data directory, which is not under
  version control. An `attempt` line holds what Gate made of the attempt
  and its evidence (the tests or builds it ran and what they printed,
  the files it changed, the start of its final reply); a `truth` line
  holds a benchmark task's own check, a rewind, a retry on a stronger
  model or a `/compare pick`. Comparisons already recorded are read back
  in at startup from the record and the saved sessions.
- `aura gate eval` scores Gate's verdicts on that log: how much failed
  work it let stand, per model, and for each sign of a failed attempt
  (a check whose exit status is hidden by a pipe, failures in a passed
  check's output, a reply that says the work was not verified, and
  others) how many failures it would have caught against how many good
  attempts it would have questioned. `--routed`, `--passed`, `--task`,
  `--log` and `--json` narrow or export it.

##### Changed

- A routed turn whose last test or build exited with success, but whose
  final reply says the work was not verified, goes to Gate's judge once
  instead of standing as `checks_passed`. The judge also sees that last
  check, what it printed, and whether the command's exit status was the
  check's own. Work it finds unfinished moves up to a stronger model;
  work it finds done stands. On the recorded benchmark runs, 4 of the 8
  routed attempts that ended this way had failed the task's own check,
  against 6 of all 57 routed attempts whose check passed.

### Sonnet 5.5 and Opus 5 reason at max effort

##### Changed

- Turns and subtasks on Claude Sonnet 5.5, Opus 5 and Opus 5.5 ask for
  `max` reasoning effort on every model call, where they asked for
  `medium`. An effort chosen for the run still wins. Other models keep
  `medium`. The classifier, the judge and titles still run with thinking
  off.

### Gate prices work by what it has cost

##### Fixed

- Whether a turn is led is decided on turns of the same difficulty when
  the classifier gave one. Opus 5.5 led hard SWE-bench Pro tasks because
  leading had proven cheaper on small practice tasks, then spent on its
  own about what doing the work would have cost, and its Sonnet 5
  subtasks as much again: $0.48 against $0.18 for Opus working directly.
- Verifiable work is priced on what work like it, of the same category
  and difficulty, has cost each model in full, subtasks included, and
  every turn counts in that cost, also one nothing checked. A model with
  no turns there costs what the model with the most turns there does,
  times what its work costs against that model's on the kinds of work
  both have done, starting from the ratio of their list prices. Before,
  a model was priced from its list price alone: Gate sent hard SWE-bench
  Pro tasks to Sonnet 5, which lists at half of Opus 5.5's price, and it
  cost $3.23 where Opus cost $1.65.
- An older version of a model you can use gets no routed work when the
  later version costs no more per token. Sonnet 5 got hard work because
  Sonnet 5.5's outcomes had ruled Sonnet 5.5 out and nothing was known
  against Sonnet 5.
- A model's record on other checked work weighs in what Gate expects
  of it on work a check verifies: the prior of its success rate weighs 4
  outcomes plus a quarter of each outcome on the other such categories
  (small changes, multi-file changes, debugging), up to 12. Two failed
  turns on an easy rename had ruled Sonnet 5.5 out of hard multi-file
  work, against 42 debugging turns that all passed, so that work went
  to Opus 5.5 and Gate cost what Opus costs. It now starts on Sonnet
  5.5, which did the recorded tasks for about a third of Opus's cost,
  and moves up when an attempt fails.
- An outcome on work of the difficulty being routed counts once in the
  model's success rate. It was scaled by the difficulty as well, so a
  failure on hard work counted as 1.6 failures.

### BENCH runs show how Gate classified them

##### Changed

- The runs table in BENCH now has a `classified` column with the task type
  and difficulty the classifier gave the run's first prompt, and a
  `moved up` column naming the model Gate moved up to, or `no`, and a
  `led` column saying whether its model led a turn and what the subtasks
  it handed the work to cost.

### Gate starts hard work on the standard tier or above

##### Changed

- Work the classifier found `hard` no longer starts on a fast or local
  model while a standard or frontier one can be used, whether Gate picks
  it by expected cost, because it is proven, or by exploring. On SWE-bench
  Pro tasks Haiku 5.5 ended its turns with passing tests it wrote for its
  own code, or tests that did not cover the change, so Gate counted them
  as done and never moved them up, and the benchmark's own checks failed.

### Gate's classifier labels long task prompts

##### Fixed

- Haiku 5 thinks unless a request turns thinking off, so the
  classifier's 32-token reply went on thinking and long prompts (such as
  SWE-bench Pro tasks) came back `unclassified`, which kept them off
  Gate's cheap-first routing for verifiable work. A request with
  thinking effort `off` now sends `thinking: {"type": "disabled"}` to
  Haiku 5 models. The classifier also says that features count as
  `small_change` or `multi_file_change` and lists the labels again after
  the prompt, as a long prompt otherwise led to made-up labels such as
  `feature` or `implementation`.

### Gate follows up work that checked nothing

##### Added

- A routed turn that ran no test or build of its own no longer ends
  unjudged while a stronger model is there to move up to. It is asked
  once, on the same model, to check the change it made
  (`AURA_GATE_NUDGE`); if it still checked nothing, the harness runs the
  project's tests (`AURA_GATE_CHECK`: `AURA_DOD_TEST_COMMAND`, else the
  command the manifest implies, up to five minutes; a command that
  could not run says nothing); and work that looks unfinished (no
  files changed on verifiable work, a reply that gives up, a turn
  stopped by its budget or a stall) goes to the judge
  (`AURA_GATE_SUSPECT`), which says whether it is done. A failing
  harness check or unfinished work counts against the model and moves
  the turn up a tier, told what was found. In the terminal, `aura -p`
  (a `checked` stream-json event for the harness's check), evals and
  `/compare`.
- `/compare` runs show the models a run moved through
  (`sonnet-4-6 › opus-4-7`), and the arms' summary counts the runs that
  moved up and the runs asked to check.

##### Changed

- Checks run through a shell wrapper (`sh .swe/run 'pytest …'`,
  `bash -c 'cargo test'`) count as the turn's checks. The lead's
  command allow-list still refuses them.
- A `/compare` Gate arm's turns are followed up and judged as routed
  turns are, so a run costs what the same work would outside a
  benchmark. The runs' hidden checks are recorded in Gate's table only
  once every run has ended, so no run is routed by another's check.

### TIME tab, and a GRAPH that stays readable on big flows

##### Added

- A TIME tab in the chat panel, between GRAPH and BENCH: the same flow
  as a timeline, a row per agent, indented under the agent that spawned
  it, with its turns as bars over the run's time, Gate, judge and error
  marks, and its cost, time and check result. A benchmark's runs are
  listed by task, arm A next to arm B, under a row that says how many
  of each arm's runs passed. Rows fold (Space, `z`), `e` keeps only the
  rows that failed, and `+`/`-`/`0` zoom the time. Enter opens the
  selected turn's cards in GRAPH with every other agent folded; Esc
  there goes back to TIME.
- Agents fold on the GRAPH canvas: `c`, or the `▾` before an agent's
  name, puts all its turns on one `agent` card; `C` folds every agent
  but the selected one and the one that spawned it, and `x` opens them
  all. A flow of more than four agents, or of a benchmark, opens with
  every agent but the one on screen folded.
- A map of the whole GRAPH canvas in its bottom left corner when the
  canvas is twice the view or more: cards as blocks coloured by how
  they went, the part on screen lit. A click or drag on it moves the
  view; `m` hides it.
- The flow keeps up to 160 agents and 16,000 steps (was 48 and 4,000),
  so a whole benchmark fits.

### Benchmarks — suites and harder tasks

##### Added

- `/compare --suite <name>` and the BENCH form run the example tasks of
  one suite (`coding`, `gate_cost`, `gate_hard`, `polyglot`, `codebases`
  or `swe_bench_pro_hard`) instead
  of every suite. With `--suite` and no `--examples`, all its tasks run.
- `evals/polyglot`: 24 of the hardest exercises of Aider's polyglot
  benchmark (Exercism, MIT), eight each in Python, Rust and Go. Each
  task's hidden check is the exercise's own tests. The tasks are
  imported by `scripts/import_polyglot.py`, which keeps only tasks that
  fail as shipped and pass with the reference solution.
- `evals/codebases`: 21 real fixes from open-source repositories
  (sqlglot, Python-Markdown, go-toml, gomarkdown, expr, jsonparser,
  semver), built as SWE-bench builds its tasks: the whole repository at
  the commit before the fix, the issue the fix closed as the prompt, and
  the fix's own tests as the hidden check. A task names its repository
  and commit as `[source]`; `/compare`, the BENCH tab and `aura eval run`
  download it once into `AURA_HOME/evals/sources`.
  `scripts/import_repo_tasks.py` mines merged pull requests for more and
  keeps a fix only when its check fails before it and passes after it.
- `evals/swe_bench_pro_hard`: the 51 tasks of SWE-Bench Pro V2's HARD-51
  subset (Scale AI, MIT), in Python, Go and TypeScript, as a suite of the
  BENCH form and `/compare --suite`. A task names its container image as
  `[source]`; aura copies the image's repository into the workspace, the
  agent runs the project's commands in a fresh container of the image
  through `sh .swe/run '<command>'`, and the check grades as SWE-Bench Pro
  does: the changes applied in a fresh container, then the hidden tests.
  Needs Docker. `scripts/import_swe_bench_pro.py` imports them.
- The comparison view shows each arm's cost per fix: its whole cost,
  failed runs included, divided by its check passes.

### Gate — cheaper defaults

##### Changed

- Turns are done by the model Gate routes them to unless leading has
  proven cheaper for the category and model. In a 33-task benchmark,
  every led turn cost more than the strongest model doing the work
  itself: the lead's own reading, planning and checking cost about that
  much before any subtask ran. A frontier model never leads to explore.
  Leading proven on its own no longer wins: the model doing the work
  itself gets its tries until it is proven too (then the cheaper way
  wins) or ruled out, so a lead learned early can't lock the direct way
  out.
- `planning` starts on the standard tier, like every category but
  `small_change`; the frontier tier is reached by moving up.
- A turn model is judged on the turns it did itself, so failed led turns
  no longer move a whole category up a tier.
- Work a check verifies (`small_change`, `multi_file_change`,
  `debugging`) starts on the model it is expected to cost least from,
  counting the judge's review, the stronger models a failed attempt
  moves up to, and what its led turns' subtasks cost. Success rates are estimated from the model's outcomes on
  a prior from its tier and its other categories; exploring samples
  those estimates. `AURA_GATE_CASCADE=off` routes such work by success
  rate again.
- A Gate run in `/compare` whose turn fails its test or build goes on in
  the same conversation on the next tier up, as routed turns do.
- The classifier also says whether a prompt's work looks easy, medium or
  hard, in the same call; it is recorded on the prompt. Easy work starts
  on the fast tier (plans excepted) and hard small changes on the
  standard one. For work a check verifies, the difficulty scales the
  expected chance of failing, learned per model and difficulty.

##### Added

- Each Gate run in `/compare` lists its turns' categories and
  difficulties, whether the lead did the work or led, the lead's and
  subtasks' cost, and how often a turn moved up a tier.
- `/compare --learn` lets Gate arms explore as routed turns do, recording
  each pick's propensity, so a benchmark teaches Gate about models it
  would not pick yet.
- `/gate` replays the recorded turns: how Gate's picks now and each
  model on its own would have done on them, estimated by weighting each
  turn by how unlikely its route was.

### Side panel: one agent or the whole project

##### Added

- The side panel's tab bar ends in an `ALL` button (a box of its own in
  the desktop's title bar). Off, the PROCESSES, TRACE, PLANS and TASKS
  tabs list only the agent on screen and the agents it spawned; on, they
  list every agent's in its project. `App::toggle_pane_scope` and
  `PaneChrome::scope` let a front end that draws the bar work it.

##### Changed

- PROCESSES no longer lists every agent's processes, and PLANS and TASKS
  no longer list the whole workspace's, unless `ALL` is on.
- TRACE keeps one feed of every agent's steps, capped at 300 per agent,
  instead of one feed per chat, so `ALL` can list a project's steps in the
  order they came.

### Terminal: queued-messages panel

##### Added

- Work waiting for a busy agent shows in a panel above the input, in run
  order, with a `N Queued` header that folds the panel (`—` / `+`). Each
  entry can be sent now, edited (messages only, in the input: Enter saves,
  Esc cancels and restores the draft), removed, or dragged to another
  place. Shift+Up in the input moves the keys to the queue: Up/Down select,
  Alt+Up/Down move, Delete removes, Enter or `e` edits, Ctrl+Enter sends
  now, Esc goes back. Clicking a row never moves the keys to the queue.
- `UiEvent::{RemoveQueued, EditQueued, MoveQueued, SendQueuedItemNow}` name
  queued entries by a stable id. The event loop applies them and sends the
  queue again.

##### Changed

- `UiCommand::SetQueued(usize)` is now `UiCommand::SetQueue(Vec<QueuedItemView>)`:
  the queue itself (id, text or label, whether it is editable), not just
  its length.

### Plans — easy to read

##### Changed

- Plan files now read well as Markdown. After the YAML frontmatter comes
  a generated header with the plan's title, its overview and a `## Todos`
  checklist (`[ ]`, `[~]`, `[x]`, `[-]`) kept in sync with the agent's
  todo list. A `<!-- end of generated header: edit below -->` marker ends
  the header, and the plan body follows. Older plan files still load and
  get the header the next time they are written.
- `create_plan` tidies the body: CRLF becomes LF, a body that came through
  with literal `\n` gets real line breaks, trailing spaces are removed,
  runs of blank lines outside code fences shrink to one, and a leading
  `#` heading that repeats the plan name is dropped.
- The tool schema and plan-mode prompt ask for `##` sections, short
  bullets, `inline code` for paths and fenced code with a language tag.
- Opening a plan at a todo (from PLANS or a task) now lands on its
  checklist line.

### Benchmarks — two arms on the same work

##### Added

- The chat panel has a BENCH tab (`/bench`, `/bench new`) that lists every
  comparison and sets new ones up with a form: the work (your last prompt,
  a typed prompt, or one of this chat's sessions), arm A, arm B, runs per
  arm, check and budget, then the estimate and a confirmation. Opening one
  shows both arms side by side, each prompt's cost and checks, the judge's
  verdicts, and the runs to diff, apply or pick.
- `/compare --session current|<ms>` replays a session prompt by prompt in
  each copy, starting from the files as they were when the session started
  (put back from its file checkpoints). The view shows the session's own
  cost, each arm's cost per prompt, and how many of each prompt's turns
  passed or failed their tests.
- For a replayed session, the blind judge also sees what you kept (the
  session's diff and last reply) and rates each run better than, as good
  as, or worse than it.

##### Changed

- `/compare` arms are A and B, each `gate`, `gate:nodelegate` or a model
  (`--a`, `--b`; `--against` still names arm B), so two models can be
  compared without Gate. Runs are named `a-1`, `b-2`, … and `/compare pick`
  takes `a` or `b`. Older comparisons read as Gate (A) against their other
  arm (B).

### Terminal and desktop — coordinating agent and tasks

##### Added

- In `agent` mode the main agent coordinates: its prompt has it plan with
  `todo_write`, hand each non-trivial item to the best-fitting permanent
  agent with `task`, and keep small edits and simple answers to itself. The
  `task` tool lists each permanent agent with its description, model and
  tools.
- `task` takes a `todo_id`. The item goes in progress, assigned to the agent
  it was handed to (`TodoItem.assignee`, shown as `→ coder`), as soon as the
  call is made. It is done, or pending again on failure, when the agent
  reports back. A detached task reports back through a
  `<task_report todo_id=… ok=…/>` line on its notice. The assignee is
  mirrored into the plan file and replayed from history.
- The parent's record gets a `SubagentSpawn` entry for each `task` call,
  with its `tool_use_id`, `subagent_type`, child name and `todo_id`.
- A TASKS tab next to PLANS in the side panel. Each request whose turn
  calls a tool is a task, and each `task` call is a task under it, done by
  the spawned agent. A task's items follow its agent's todo list. Tasks
  link to the plan and todo they work on, and are kept in
  `.aura/tasks/<id>.json` across restarts. Enter shows the task's agent,
  `p` opens its plan; `t` on a PLANS todo jumps to its task.

##### Changed

- Spawned agents in the projects list are joined to the agent that spawned
  them by tree lines.
- In the GRAPH tab a `task` row's wire runs to the agent doing it, colored
  by how the task went. The agent's card ends `↩ reported to <agent>`, or
  `↩ failed for <agent>`, where its report runs back to the caller.
- With five tabs, the side panel shows short labels (`SBX`, `PRC`) for
  tabs whose full label doesn't fit its cell.

### Reading code with less context

##### Changed

- `read_file` returns numbered lines (`  12|text`, the number as wide as
  the last one shown) for whole files too: at most 2000 lines and 32 KB
  (`ToolConfig::max_read_bytes`, was 64 KB), cut at a whole line, ending
  with the `start_line` to continue from. Lines over 2000 characters are
  cut, and binary files are described instead of dumped. The schema no
  longer offers `max_bytes` or claims a 1 MB default.
- `search_code` and `find_files` skip what the project's `.gitignore`
  (and `.ignore`, git's excludes) leave out, in a git repository or not,
  and walk in name order. `search_code` gains `output_mode`:
  `files_with_matches` and `count` locate matches without printing them.
  It returns at most 500 results whatever `max_results` asks for, and
  cuts matched lines over 300 characters.
- `find_files` lists the most recently modified matches first and says
  when it stopped at 200; `list_files` stops at 500 entries and says so.
- A `run_command` output over its limit keeps its first 3/8 and its last
  5/8 around a note of how much was cut, instead of only its start, so
  errors and test summaries printed last survive.
- Every agent loop leaves its transcript untouched below 70% context
  use (`AgentLoopConfig::compaction_trigger` now defaults to
  `aura_config::COMPACTION_TRIGGER_DEFAULT`), as the terminal already
  did; the node's chat sessions, automatons and spawned agents used to
  start trimming old tool results at 15%.
- Compaction folds a `read_file` result once a later, complete read of
  the same path shows the same lines (`supersede_older_reads`), so a
  file edited and read again is not carried twice.
- The run cache keys `search_code` on `output_mode`, `max_results` and
  `file_pattern` too.
- The chat system prompt asks the model to locate before reading, read
  ranges, batch independent reads and hand broad exploration to an
  explorer subagent.

### Terminal and desktop — chat restyle

##### Changed

- The default `cyber` theme is graphite and gray. The cyan accent is kept
  for status (`● Ready`, the working `⬢`, busy tabs) and the focused
  panel's border; other borders, tabs, selections and menus are neutral. Themes gain a `faint` color for
  borders at rest, markers and separators. Errors and failed counts use a
  softer red (`#e06c75`).
- The input is a box under the Chat panel, as wide as it, with a `→`
  prompt and an `Ask anything` / `Add a follow-up` placeholder. The model,
  approval level, usage (`23% context used`) and status moved to a footer
  under it, on the screen's last row. The side panels end level with the
  input box.
- While a turn runs, its to-do checklist (`☒ ▣ ☐`) and a `⬢ Thinking…`
  line follow the conversation, replacing the progress strip on the Chat
  panel's border. The current to-do is bold, finished ones fade, and past
  ten the rest are counted (`3 more`). `/todos` uses the same boxes.
- The chat is inset two columns, starts a row below the border and stops
  at 110 columns. User prompts start with `→`. Steps are gray throughout
  and a collapsed group reads `6 steps (click to expand)`. Inline code is
  light gray like the reply. Steps, hints and notices are no longer
  italic, and project names and code block labels are no longer bold.
- The header names linked plans in capitals (`CLAUDE MAX 20X · CHATGPT
  PRO`), like the title bar and tabs.
- The desktop window's title bars and tabs fill the selected box in gray
  instead of the accent, and it draws `⬢ ☐ ☒ ▣` itself.

##### Added

- The chat panel expands to fill the window: the `⤢` at the right end of
  its header bar, `Ctrl+X` or `/expand` (`/zoom`, `/fullscreen`) hides
  the other panels, and `⤡` or the same key brings them back. CHAT,
  TERMINAL, GRAPH and a file or record over the chat all expand; over a
  file or record the button sits left of its `✕`. The desktop window
  draws it as a box of corner brackets. `PaneChrome::expand` and
  `App::expand_pane` let other front ends draw and work it.

### Context compaction

##### Changed

- A tool result longer than 32 KB (`aura_config::TOOL_RESULT_MAX_BYTES`)
  enters the transcript as its first and last 16 KB around a marker
  saying how much was cut and how to read the rest, so later requests
  never resend the whole of a huge `search_code` or `read_file` output.
  Events and records still carry the full result.
  `AURA_AGENT_DISABLE_COMPACTION` turns the cap off along with the
  rest of compaction.
- Compaction tiers trim harder as the context fills: the 15% tier now
  cuts older tool results to 3,000 characters and keeps the last 8
  messages, and the 30% tier cuts to 1,500 and keeps 6 (they were the
  other way round). `CompactionConfig::micro()` and `history()` are
  renamed `emergency()` and `minimal()`, and `OverflowStep::MICRO` is
  `OverflowStep::EMERGENCY`.

### Gate router (`aura-agent-gate`)

Gate is now one learning router and the default model. The coordinator
mode that used to share the name is gone. See [Gate](docs/cli.md#gate).

##### Breaking changes

- The coordinator mode is removed: `gate/<model>` ids, `run_blocks`,
  draft-first turns, the tier ladder, the ledger in `AURA_HOME/gate/`
  and the `aura gate report|fit|bench|promote` subcommands.
- `AURA_GATE_POLICY`, `AURA_GATE_EXPLORE_BUDGET_USD`, `AURA_GATE_TIERS`
  and `AURA_GATE_LEDGER` are no longer read. `AURA_GATE_EXPLORE` now
  sets Gate's exploration rate (default `0.05`).
- The payload of a `UserPrompt` transaction is a JSON `PromptPayload`
  (`text`, `category`, `classifier`, `route`, and `kind` for subtasks)
  instead of raw text. There is no backward compatibility: prompts
  recorded before this change read back as empty text. `POST /tx` wraps
  submitted prompt text in a payload.
- New `TransactionType::Outcome` and `SystemKind::Compare`. Consumers
  that mirror these enums must add them.
- The terminal starts on `gate` instead of the local default model, and
  a pinned model that loses its sign-in switches to `gate`.
  `AURA_DEFAULT_MODEL` and a model picked in `/model` still win.

##### Added

- `gate` model id, first in `/model`, accepted by `--model gate`. Gate
  routes among the models your sign-ins allow: installed local models,
  aura-router models when signed in to zOS, `claude/*` with a Claude
  plan, and `chatgpt/*` with a ChatGPT plan. It keeps one route per
  model (the plan first) and tiers them `local`, `fast`, `standard` and
  `frontier`. A model whose route refuses the sign-in is dropped and the
  turn carries on with the next pick. Each turn shows
  `gate → <model> (<route>) · <category>, <reason>`, and the status line
  at the bottom right shows `GATE` in place of the model.
- The footer under a reply covers the whole turn: every model that ran,
  with Gate's calls and the chat title marked, and the tokens and cost
  of all of them, e.g. `[claude-sonnet-5-5 ×2 · gate: claude-haiku-5-5 ·
  title: claude-haiku-5-5 · 27.9k tok · $0.040]`. It used to show the
  last call only. The classifier, the judges and the title call record
  their `purpose` in their `Reasoning` entry, so a reloaded chat shows
  the same footer. Chat titles are written by the classifier's model
  instead of a fixed Haiku 4.5 (`AURA_TITLE_MODEL` still overrides it).
- Every prompt is classified into one of 8 categories by the cheapest
  fast-tier model, through the agent's kernel. Every turn ends with an
  `Outcome` (`error`, `checks_failed`, `checks_passed` or `none`), and
  the judge's findings, rewinds and stronger retries append later ones.
- Gate routes each category to the cheapest model with
  `AURA_GATE_MIN_TRIES` (10) counted outcomes at `AURA_GATE_TARGET`
  (85%) success. Otherwise it uses the category's default, and it
  explores the next cheaper model at `AURA_GATE_EXPLORE`. The table is
  rebuilt from the record at startup and updated as each outcome is
  committed. `/gate` shows it with the current picks, the delegation
  figures and the last 20 routing decisions.
- Subtasks under `gate` are classified and routed with their own table
  rows. Models named by the call, the permanent agent or a council seat
  stay pinned.
- Under `gate` the main agent leads the turns classified
  `multi_file_change` or `planning`, and does the rest itself on the
  model Gate routes it to. On a turn it leads it gets the `gate_lead`
  prompt section and, unless `AURA_GATE_DELEGATE=off`, no file-changing tools,
  capped reads and searches, and a `run_command` limited to checks and
  inspection. New `implement` and `tests` permanent agents (and bundled
  kinds) take the work. `task` gains a `check` command and returns a
  compact result. Read-only agents run in parallel and the rest one at
  a time. A routed subtask that fails its check is retried once on the
  next tier up with its file changes undone. In a git repository that
  includes changes made by shell commands: the workspace folder goes
  back to a snapshot taken before the attempt, leaving ignored files,
  the index, `HEAD` and stash alone.
- `/compare [prompt | --plan f] [--against m | gate:nodelegate]
  [--repeats N] [--check cmd] [--budget $]` runs a task with Gate and
  another model in copies under `AURA_HOME/compare/`, judges each pair
  blindly, and shows time, cost, tokens per model and check passes per
  arm. Subcommands: `go`, `cancel`, `show [n]`, `diff [run]`,
  `pick gate|other`, `apply <run>` and `dismiss`. On a Claude or ChatGPT
  plan it also shows the share of each plan window each arm used, read
  from the usage headers of each response and recorded with the model
  call; each rise between readings goes to the run whose call saw it,
  so parallel runs don't count it twice. Providers that send no such
  headers show tokens per model instead.
  The check runs as a `run_command` call through each run's kernel, so
  it is in the run's record and under the same command policy. Each run
  is unattended in its own copy and runs in `agent` mode at `yolo`.
- `evals/gate_cost`: 20 tasks with hidden checks, categories and
  reference solutions. Eval tasks can set `category`, `tags` and a
  `checks/` folder copied in after the run; `aura eval run` reports each
  run's cost and takes `--budget-usd`. `evals/gate_hard` adds eight
  tasks with traps a careless fix falls into.
- `aura eval run --model gate` runs each task as `aura -p` turns: Gate
  classifies and routes them, the agent leads and delegates where the
  category calls for it, and the
  verify command runs as a `run_command` call through the run's agent's
  kernel, its verdict recorded as a later outcome so eval results count
  in Gate's table. The results table shows the models Gate routed each
  task to. `EvalOptions` takes an `EvalRunner` instead of `config`,
  `tool_config` and `provider`. The runs' agents work at `yolo`, like
  `aura -p` in `agent` mode.
- Gate prices a model it has not tried from its list price scaled by how
  the tried models of its tier (else the nearest tier) cost against
  theirs, so newer, cheaper models are no longer ranked behind older
  ones with history. When the lead is on a frontier model, its subtasks
  are first routed below the frontier tier; an escalated retry may still
  reach it.
- Gate starts work cheap and moves it up on failure. `small_change`
  starts on the cheapest fast-tier model, `planning` on the strongest
  and every other category on the cheapest standard-tier model. A start
  model that can no longer reach the target in a category hands it to
  the next tier up. A routed turn that fails through the model's own
  doing goes on in the same conversation on the next tier up (route
  reason `escalate`, not counted), in the terminal, `aura -p` and
  `aura eval run`.
- Gate's judge reviews failed work. When a routed turn or a subtask's
  first attempt ends in a model error or with its last test or build
  failing, the strongest Opus you can use is shown the request, the
  diff, the final reply and the failure, and says whether the model was
  at fault. If so (`judged_fail`) the failure counts in Gate's table and
  the work moves up a tier, told what the judge found. If not
  (`judged_excused`) it counts for nothing and stays where it is. Work
  that did not fail is never judged, so the judge costs nothing on a
  clean run; `aura eval run` reports its cost per task and in total, and
  `aura -p --output-format stream-json` emits a `judged` event.
  `AURA_GATE_JUDGE=off` switches it off.
- Exploration now goes to the nearest cheaper model the category has
  not ruled out, else the nearest, so a ruled-out model is tried again
  once nothing cheaper is more promising.
- `/good` and `/bad` are removed. How a turn went is learned from its
  own signals and the judge; `/compare pick` still records your pick.
- A `task`, `run_command` or plugin tool call drops the caller's cached
  tool results, so the next read shows what is on disk
  (`aura_config::FILE_NEUTRAL_TOOLS` lists the uncached tools that keep
  the cache).
- On Claude models the thinking effort stays at medium for the whole
  run. Lowering it after the first write made Anthropic rewrite the
  conversation's prompt cache.
- `aura eval run` prints one line per task on stderr as it starts and
  ends (verdict, cost, time, spend so far, routed models), and with
  `--model gate` the classifier and the models Gate routes among.
- The Claude plan's live model list is saved as `plan_models.json`
  beside the model preferences and used when a later fetch fails, with
  a note saying so.
- Under any approval level, the lead's `run_command` limit only narrows
  what the level lets run, and a `task` `check` runs through the
  subtask's kernel at the level the subtask inherited from its parent.
- `aura-app`'s `isolation` module holds the worktree and copy helpers
  `/compare` runs in and the change carrying, diffing and scratch-index
  snapshots `/compare` and the escalation use.

### Terminal — GRAPH tab

##### Added

- The chat panel has a third tab, GRAPH, next to CHAT and TERMINAL
  (Shift+←/→ or a click). It shows the flow between the agent on screen
  and every agent below it as their kernels recorded it, as cards wired
  together on a canvas: a main card per turn, with its prompt and
  replies, and under it on a spine a subcard per model call with its
  tool calls as rows. A `task` row shows what came back from it, and
  the agent that took the work has turns of its own, wired from that
  row. Each agent's turns go down a column under its name until the
  view has no room for the next, then on at the top of the next column;
  agents that work in turn share a band, and bands share the view's
  height. Wires never run backwards.
- Model calls and turns have a line of their own with the tokens read
  and written (the whole prompt, cached or not), the cost and the time;
  a turn's adds up its calls.
- A step opened as JSON says when it happened to the millisecond, and
  which agent spawned its agent. A turn's prompt also shows the turn's
  totals: steps, tokens, cost and time.
- The calls the harness makes around a turn are subcards of their own
  kind, muted: `gate` (classifying the prompt, the first step of its
  turn), `judge` and `title`.
- The tab follows the agents live. Turns are open until folded with the
  `▾` button in a card's title (or Space), and stay as they were put as
  steps arrive and when the agent is shown again. `↑`/`↓` go through a card's
  lines and on to the card above or below, `←`/`→` along the agent's
  turns, `[` / `]` jump between prompts, `f` follows a wire to the
  other agent (opening what is folded over its end), Space folds a
  model call's tool calls or a turn's model calls, `z` folds every
  turn, Enter or a second click opens a step as JSON, `r` opens its
  record entry and `o` switches to its agent. Dragging empty canvas or
  the wheel pans the view over a canvas without edges; `0` brings it
  back to the selection. See
  [The GRAPH tab](docs/cli.md#the-graph-tab).
- The desktop app draws the canvas itself (`App::set_host_canvas`):
  square cards on the window's background, right-angled wires,
  and a view that drags by the pixel, pans smoothly on a touchpad and
  zooms from 25% to 200% around the pointer (Ctrl+wheel, a pinch, or
  `+`/`-`/`0`). Only what is on screen is drawn; text sizes snap to
  half a pixel and turn to bars below 50%, so zooming stays fast. The
  terminal UI leaves the
  canvas cells blank and says which of them no menu or dialog covers
  (`App::flow_canvas_clip`), so those stay on top.
- `UiEvent::OpenFlow` asks the session for an agent's flow;
  `UiCommand::FlowLoad` and `UiCommand::FlowUpsert` carry it as
  `FlowLane`s and `FlowNode`s (`aura_ui::flow`). The session builds it
  in `event_loop/flow.rs` from a record observer on every kernel and
  the record store, off the kernels' and the event loop's tasks. Only
  the boxes on screen are drawn.

### Many agents at once

##### Fixed

- The desktop app no longer slows down and stops answering input while
  many agents stream at once. It rendered the UI once per command the
  agents sent, which with 30 agents took more than the whole second.
  Commands are now applied as they arrive and the window renders at
  most once per frame of the display: a second of 3,000 commands from
  30 agents costs about 50 ms of the window's thread.
- The desktop app shapes only the rows of the grid whose content is
  new. Any change, such as a streamed word or a ticking timer, used to
  repaint and re-shape every row; a row that is unchanged, or that only
  scrolled, now reuses the quads it was drawn with. A frame that comes
  out the same as the one on screen is not shown again, so an idle
  window draws only when the cursor blinks. `AURA_DESKTOP_PERF` reports
  the rows painted anew and the frames skipped.
- The Sandbox tree is read again once per tick rather than after every
  tool call of every agent.
- The terminal UI takes the agents' commands every 5 ms while it waits
  for input, rather than once a frame, so a busy swarm no longer fills
  the command channel and holds up the agents and the event loop.

### Chat titles

##### Added

- Agents with a placeholder name (`Agent`, `Agent N`, `Terminal Agent`)
  get a short title for their chat after their first turn, such as
  "Task build issue", written by Claude Haiku and shown in the agents
  list and the Processes tab. The title is kept in `agents.json`; the
  agent's name and identity don't change. Names the user picked and
  permanent-agent instances are left alone. `AURA_TITLE_MODEL` picks
  another model.

### Projects list

##### Added

- A "+" at the right end of a project's row adds an agent to the project.
  Agents added to a project, this way or by adding a permanent agent to
  it, are listed at the same level as its first agent; only agents
  another agent spawned are indented under it. Registries that linked
  added agents under the first agent are moved on load.
- An ARCHIVE tab in the agents panel. Delete on an agent in PROJECTS (or
  `/archive`) moves it there with the agents under it, and Enter brings
  it back. Shift+Delete (or `/remove-project`) removes the project from
  the list after a `y`; its records and folder stay on disk.

### Approval levels

##### Added

- An approval level per agent, shown as a badge before the model name in
  the terminal and desktop status line, and picked with `/approval
  <level>`, Shift+Tab (`yolo` → `ask` → `strict`), `--approval` or
  `AURA_TUI_APPROVAL`:
  - `ask`: reads run; everything else asks unless a
    project "always allow" rule covers it.
  - `strict`: everything but reads asks, "always allow" rules are
    ignored, and prompts offer no "always" answer.
  - `yolo` (the default): nothing asks, and the agent's commands can write only inside
    the working folder, temp folders and package caches. On Linux this
    is enforced with Landlock; elsewhere commands still ask unless they
    plainly stay in the folder. `AURA_YOLO_WRITABLE_PATHS` adds folders.
- Spawned agents start at their parent's level, and every level change is
  recorded in the agent's `mode_change` entries (`agent/ask` →
  `agent/yolo`).

##### Changed

- `/mode` now picks only the work mode: `agent`, `plan`, `read-only`
  (formerly `/mode ask`) or `debug`. The old `default`, `accept-edits`
  and `unrestricted` modes are gone; `/mode unrestricted` points to
  `/approval yolo`, saved sessions load them as `agent`, and
  `AURA_TUI_MODE` set to one of them is ignored with a warning.
- Shift+Tab cycles the approval level instead of the mode.
- `aura -p` in agent mode runs at `yolo`, so on Linux its commands can no
  longer write outside the working folder. `--approval ask` or `strict`
  denies edits and commands instead.

### Workspace layout

##### Changed

- Crates live in one folder per layer, `crates/<layer>/<crate>`, with
  layers ordered from most to least fundamental: `core`, `config`,
  `store`, `model`, `context`, `plugin`, `exec`, `agent`, `auth`,
  `engine`, `ui`, `surface`. `auth`, `engine` and `ui` are new layers;
  their crates were previously classified as `surface`, and
  `aura-exec-domain-http` moved to `exec`.
- Every crate below `surface` is named `aura-<layer>` or
  `aura-<layer>-<part>`. Renamed, with no compatibility aliases:
  - `aura-protocol` → `aura-core-protocol`
  - `aura-plugin-core` → `aura-plugin`
  - `aura-tools` → `aura-exec-tools`
  - `aura-domain-http` → `aura-exec-domain-http`
  - `aura-ui-core` → `aura-ui`
  - `aura-terminal` → `aura-ui-terminal`
  - `aura-runtime` → `aura-node` (the binary keeps its `aura-node` name)
  - `aura-model-reasoner` → `aura-model`
  - `aura-automaton` → `aura-engine-automaton`
- `aura-surface-cli` split into `aura-app`, the session host shared by
  the terminal, the desktop app and `aura -p`, and `aura-cli`, the
  command line. `aura-desktop` now depends on `aura-app` instead of the
  CLI.
- `ReadStore`, `DequeueToken` and `StoreError` moved to
  `aura-store-record`; RocksDB errors surface as `StoreError::Backend`.
  The tool hook traits, `AutomatonController` and `DomainApi` moved to
  `aura-exec-traits` (still re-exported from `aura-exec-tools`). The
  `/ws/terminal` PTY handler moved from the terminal UI crate into the
  `aura-node` gateway.
- The TypeScript bindings are exported with
  `cargo test -p aura-core-protocol --features typescript export_typescript_bindings`.
  `aura-os` still calls its mirror of the wire types `aura_protocol`, so
  the two no longer share a name.

##### Removed

- `aura-exec-sandbox` and `aura-exec-isolation`. The filesystem
  sandbox the tools use is `sandbox.rs` in `aura-exec-tools`, and the
  workspace copies Gate's `/compare` and escalation use are `aura-app`'s
  `isolation` module.
- The old `aura-core-protocol` placeholder, which only held
  `ProtocolVersion`. It was merged into the wire protocol crate, which
  now carries the `aura-core-protocol` name.

### Run traces (`aura-agent-trace`)

##### Added

- Every terminal and desktop conversation is recorded as a run under
  `AURA_HOME/runs/`, one run per agent conversation, with a scorecard
  written after each turn. Kernel snapshots are kept with the runs so
  `aura runs` can replay them. See [`docs/run-traces.md`](docs/run-traces.md).
- `/graph [id]` opens a run's live graph (step labels, record and file
  links), `/runs` lists recorded runs and `/status` shows this
  conversation's scorecard. The status line shows each model call's
  iteration and running token count.
- `aura runs` (list, show, diff, label, verify, branch), `aura dataset
  build` and `aura eval run/compare` with the `evals/coding` suite.
- The `/graph` view says when each node happened, to the millisecond,
  and what model calls, turns and the run cost. A model call's tokens
  count its whole prompt (they double-counted cached tokens on OpenAI
  models). A call cut off at `max_tokens` is a warning, and a turn
  that ended on a failed call shows it as a failed iteration
  (`turn_end` steps gain `error`). Tool calls show the kernel's policy
  decision (verdict, rule, approver, reason), live too
  (`RunRecorder::subscribe_records`). A `task` call names the run of
  the agent it started and `o` opens it; `o` on a spawned run's root
  node opens its parent. The run node lists every `meta.json` field,
  and missing values show as `null` instead of blank.

### Surface — native desktop app (`aura-desktop`)

##### Added

- `aura` (and so `cargo run`) opens the desktop window when given no
  subcommand; `aura --terminal` (alias `--tui`) or `aura run` runs in the
  terminal, as does `aura` with no display (e.g. over SSH). `aura desktop`
  takes the window's options (`--workspace`, `--demo`, `--theme`, …).
  The window comes from the root package's default `desktop` feature;
  the Docker images and the 1.85 MSRV check build without it.
- Tabs in the desktop window, which work as workspaces. A strip above
  the grid has a numbered box per tab, a `+` box, and a wide box naming
  what the tab in front shows. Each tab holds its own projects and
  agents and shows only those, in its own `aura_terminal::App` with its
  own draft, panels, overlays, scroll and selection, all on the one
  session. The window starts with every project in one tab; `+` starts
  an agent in a new tab, and a project created in a tab stays in it.
  Ctrl+Shift+T / Ctrl+Shift+W open and close tabs (Cmd+T / Cmd+W on
  macOS), Ctrl+1…9 or Alt+1…9 (Cmd+1…9) go to a tab by number, and
  Ctrl+Tab / Ctrl+Shift+Tab move between them. Closing a tab moves its
  projects to the tab before it. The split is kept in `desktop_tabs` in
  the data directory. `UiEvent::FocusAgent` makes an agent the
  session's active one without reloading its chat.
- Right-clicking a project or an agent in the projects list offers
  "Move to workspace ›", whose submenu lists the window's other tabs by
  number and title, and "New workspace". Picking one moves the project
  (an agent moves its whole project) to that tab; a tab left with no
  project closes. "New workspace" is left out for the only project a tab
  lists, which has a tab of its own already. `App::set_workspaces` tells
  a UI the tabs, and `App::take_workspace_move` hands the host the move.
- `aura-desktop`, the terminal UI in a native window (winit) that
  draws itself: a frame is one list of quads over a glyph atlas, drawn
  by the GPU in one call (wgpu) or by the CPU where there is no GPU.
  No UI toolkit is involved. It runs the same in-process session as the terminal (`session::start`) and the
  same `aura_terminal::App`, rendered through a ratatui backend into a
  cell grid, so the screen, keys, mouse selection and copy behave as
  they do in a terminal.
- The grid is painted in Geist Mono (bundled) with one text draw per
  styled run; box drawing, block elements, braille and the status
  symbols are drawn as shapes, so borders join and glyphs missing from
  the font still render. A row's quads are kept until its content
  changes, and glyphs and drawn shapes are rasterized once into an
  atlas.
- Window shortcuts: Ctrl+Shift+C copies, Ctrl+V / Ctrl+Shift+V /
  Shift+Insert paste (images attach, as in the terminal), Ctrl+= /
  Ctrl+- / Ctrl+0 zoom (Cmd on macOS). Turn alerts use the terminal's
  settings, with the window asking for attention in place of the bell.
  `--theme` picks the terminal theme; `AURA_DESKTOP_MONO_FONT`,
  `AURA_DESKTOP_FONT_FILES` and `AURA_DESKTOP_FONT_SIZE` change the
  font. `--demo` runs a scripted session and `AURA_DESKTOP_PERF` logs
  frame timings. `AURA_DESKTOP_RENDERER=cpu` draws on the CPU whatever
  the machine has, and `=gpu` on whatever adapter there is.
- Offscreen snapshot tests draw whole screens with the CPU rasterizer;
  `AURA_DESKTOP_SNAPSHOTS=<dir>` saves them as PNGs, and
  `AURA_DESKTOP_GPU=1` checks the GPU draws the same pixels. Ignored
  `replay_timings` and `swarm_timings` tests report boot, streaming,
  keystroke and frame timings, and a second of thirty agents streaming
  at once; `history_timings` reports frames of a 2,000-message
  conversation and of a long reply streaming after it, `scroll_timings`
  reports frames of a long chat and 400 agents scrolling, and
  `aura-ui-terminal` has `history_timings` and `streaming_timings` for
  the UI alone.
- `aura_terminal::render`, `App::paste_from_clipboard` and
  `alerts::desktop_notify` are public so other frontends can draw the UI
  and share its clipboard and notifications.
- Panels are window panes: the window draws their title bars itself,
  with tabs packed to the left, each as wide as its label and always
  outlined, the shown one filled (in the accent while its panel has
  focus). Panels without tabs show their title in a filled box and their
  info at the right. Dragging the border between two panels resizes
  them. `App::set_host_chrome`, `App::pane_chrome`,
  `App::set_pane_splits`, `App::select_pane_tab` and `App::focus_pane`
  let other frontends do the same.

##### Changed

- The CI MSRV check skips `aura-desktop`, which needs Rust 1.88.
- The window's clipboard is `arboard`'s.
- A reply is laid out a part at a time while it streams, in both
  frontends: what comes before its last blank line or closed code block
  is kept, and only the paragraph, list, table or code block still being
  written is wrapped again. A frame after a chunk costs the same however
  long the reply is: about 0.5 ms in the window at 76,000 characters,
  where it was 3.9 ms. The reply is laid out whole once more when it
  ends.
- The chat reads like a terminal in both frontends: it starts at the
  panel's left edge, prompts are echoed as `> text`, replies follow
  without `YOU` / `AURA` headers, a dim `[model · tokens · cost]` line
  closes each reply, and notices start with `*`.
- The window draws everything in Geist Mono, chat prose included, with
  a block cursor; the proportional Geist faces are no longer bundled.
- The chat scrolls by pixels in the window. A touchpad moves it twice
  as far as the fingers do, and a notch of the wheel eases five rows
  over a few frames; notches in quick succession go further each time,
  up to three times as far, and turning the wheel back stops what is
  easing out. `AURA_DESKTOP_SCROLL_SPEED` scales all of it. The
  projects list scrolls three rows a notch, like the other panels.
  The terminal UI still scrolls by whole rows. Between
  rows, the window draws the chat's body higher than its cells, cut to
  the body, and shows the row below the body coming in. A frame that
  only slides the body is painted from the rows already rendered, with
  no render of the terminal UI. A click goes to the row drawn under
  the pointer. Other panels still scroll by whole rows.
  `App::set_smooth_scroll`, `App::chat_scroll`, `App::chat_row_below`
  and `App::scroll_chat_rows` let other frontends do the same.
- Rows are painted and kept in pieces, cut at the panels' borders. A
  panel that scrolls leaves the pieces of the panels beside it as they
  were. A notch of the wheel over the chat used to paint 2,161 rows
  anew over 160 frames; now it paints 322 pieces.
- The projects list and the archive format only the rows on screen, so
  hundreds of agents cost what fits in the panel.
- Moving the pointer only asks for a frame when what is under it
  changes: a highlight, or a selection being dragged.
  `App::handle_mouse` returns whether the screen may look different.

### Surface — UI protocol and agent session shared by every frontend

##### Added

- `aura-ui-core`: the `UiEvent` / `UiCommand` protocol, input history,
  tool-call formatting, the sandbox file tree and the `/mode` parser,
  with no ratatui dependency. `aura-terminal` re-exports them under the
  same paths (`aura_terminal::events`, `aura_terminal::mode_command`,
  `aura_terminal::input::InputHistory`).
- `aura_surface_cli::session::start`, which opens the agent registry,
  record store, model provider and kernel factory and spawns the event
  loop, returning a `UiSession` (event sender, command receiver and
  `SessionTask`). `SessionTask::finish` stops background commands,
  surfaces an event loop that stopped on an error and aborts one still
  running.

##### Changed

- `aura` (terminal mode) runs on `session::start`. The terminal now
  enters the alternate screen after the session is ready, so a startup
  error prints to the normal screen.

### Headless — `aura -p`

##### Added

- `aura -p "prompt"` (`--print`) runs one prompt without the terminal
  UI, prints the result and exits. `-p -`, or `-p` with no prompt, reads
  the prompt from stdin; piped stdin is appended to a given prompt.
- `--output-format text|json|stream-json`: the final reply; one result
  object with the reply, session id, model calls, usage (cache reads
  and writes included), cache hit rate, estimated cost, compactions,
  todo list, changed files and exit reason; or one event per line (text
  and thinking deltas, tool calls and results, todo updates, per-call
  usage, warnings) ending with that object.
- `--resume <session_id>` and `--continue` pick up a saved session.
  Runs belong to a dedicated "Headless" agent, so they never replace
  the session of an agent the terminal shows, and the terminal can
  `/resume` them.
- `--allowed-tools` / `--disallowed-tools` filter both what the model is
  offered and what the kernel permits. Nothing prompts: a tool call the
  policy would ask about is denied.
- `--max-turns`, `--max-time` and `--max-cost-usd` limit the whole run
  (the time and cost limits default to `AURA_TUI_TURN_TIMEOUT` and
  `AURA_TUI_MAX_TURN_COST_USD`). A limit stops the agent between steps
  and keeps its work in the session.
- `--cwd`/`-d`, `--model` (not saved as the `/model` choice),
  `--provider` and `--verbose`. The global `--mode` applies: `plan`,
  `ask` and `debug` act as the terminal's modes with every approval
  denied, and `agent` lets edits and commands run instead of asking.
  Agents spawned with `task` keep the run's mode and tool filter.
- Exit codes: 0 finished, 1 failed, 2 bad arguments, 3 stopped by a
  limit, 130 interrupted.
- The run waits for the agent's background work after its turn
  (background commands, detached `task` spawns and agents it started
  with `spawn_agent`; up to `AURA_HEADLESS_JOB_WAIT`, default 10
  minutes) and gives it each exit notice or reply as a new turn, then
  stops any command still running.
- stdout carries only the chosen output; logs go to stderr.
- `evals/long_horizon/`: four multi-file feature tasks (three Python,
  one Rust) with hidden tests, and `run.py`, which runs `aura -p
  --output-format json` on each task in a scratch copy, grades it with
  the hidden tests, and writes a JSON and Markdown report with pass/fail,
  cost, cache hit rate, compactions, model calls and wall time. Real
  models run only with `--live` or `AURA_EVALS_LIVE=1`; `--self-check`
  verifies the fixtures against reference solutions without a model.

##### Fixed

- Every model call now reports its tokens and cost to the turn's
  listeners. The streaming transport never did, so the terminal's
  per-reply cost, status line cache and cost figures, and
  `AURA_TUI_MAX_TURN_COST_USD` saw no model calls.

### Terminal — long-running tasks

##### Added

- `todo_write` tool: the agent keeps a plan for multi-step work. The
  loop tracks the list, puts it back after a history summary, and
  carries it across turns and restarts.
- A turn that ends with todo items still open is sent back to the next
  one. It ends anyway when the agent asks the user something, says it
  is blocked, or two nudges in a row leave the list unchanged.
- A message sent to a busy agent goes into its running turn instead of
  waiting for the turn to end. One that arrives too late runs next.
- `run_command` takes `background: true`. The command keeps running
  after the call returns and its output goes to a log file. When it
  exits, the agent that started it gets the exit code and the end of the
  log, in its running turn or in a new one, whether or not it is on
  screen. `job_output` reads a job's log and `job_kill` stops it (a job
  the agent stopped itself sends no message). Quitting the terminal, or
  the end of an `aura -p` run, stops every background command.
- `AURA_TUI_TURN_TIMEOUT` and `AURA_TUI_MAX_TURN_COST_USD` set optional
  per-turn time and cost limits.
- Anthropic requests mark the last two user messages for prompt
  caching (four breakpoints with the system prompt and tools), so a
  large batch of tool results no longer misses the cache.
  `AURA_PROMPT_CACHE_TTL=1h` uses the one-hour cache.
- A status line under the input shows, for the agent on screen, how
  full its context is, its prompt cache hit rate, the session's
  estimated cost and model calls, running background jobs, and queued
  work. The Chat panel's bottom border shows todo progress and the item
  being worked on.
- `/context` breaks down what fills the context window, `/cost` lists
  the session's tokens and cost, `/todos` opens the agent's todo list,
  and `/compact [focus]` summarizes the older history now, keeping
  `focus` in particular.
- Typing `@` in the input opens a fuzzy picker of the agent's files and
  folders. Each `@path` in a sent message is attached for the model:
  up to 64 KiB of a file, or a listing of a folder. The chat and the
  record keep the message as typed.
- Ctrl+V pastes a clipboard image into the message, and pasting the
  path of an image file attaches it. Each image shows as an
  `[image N]` chip and reaches the model as an image block. Images are
  capped at 3.75 MiB each and 5 per message.
- The terminal turns on bracketed paste, so pasted text with newlines
  stays in the input instead of sending each line as a message.
- Ctrl+G opens the input in `$VISUAL` or `$EDITOR` (falling back to
  `vi`). What is saved replaces the input when the editor exits.
- The terminal checkpoints each agent's session after every turn under
  `<data_dir>/sessions/`: the conversation as the model saw it (tool
  calls included), the todo list and usage totals. A restart continues
  from the checkpoint plus whatever the record holds after it, instead
  of from prompts and replies alone. `/resume` lists earlier sessions
  and `/resume <n>` continues one.
- Tool calls that need approval pause the turn and ask in the agent's
  chat. `y` allows the call once, `a` always allows that tool (and, for
  `run_command`, that command, such as `cargo test`) in the current
  project, and `n` or Esc denies it. Saved answers live in
  `<data_dir>/approvals.json`. A prompt nobody answers is denied after
  10 minutes (`AURA_TUI_APPROVAL_TIMEOUT_SECS`), and Ctrl+C denies it and
  stops the turn. An agent that is not on screen asks in its own chat
  and the terminal says which agent is waiting.
- Permission modes: `/mode [name]` and Shift+Tab switch the agent on
  screen between `default` (asks before edits and commands),
  `accept-edits` (file edits run, commands still ask), `plan`
  (read-only) and `unrestricted` (nothing asks). The status line
  starts with the current mode. Each switch is written to the agent's
  record before the next tool call uses it, and spawned agents start in
  their parent's mode.
- The terminal rings the bell when a turn that took at least 10 seconds
  finishes (`AURA_TUI_NOTIFY_AFTER_SECS`) and when an agent needs
  approval. `AURA_TUI_DESKTOP_NOTIFY=1` also sends a desktop
  notification through `notify-send` on Linux or `osascript` on macOS,
  and `AURA_TUI_BELL=0` turns the bell off.
- Terminal agents follow project instructions. Each turn loads
  `AGENTS.md`, `CLAUDE.md`, `.cursor/rules.md` and `.cursor/rules/*.md`
  / `*.mdc` from the agent's working folder and every folder above it
  up to the home directory, plus the user's own `~/.aura/AGENTS.md`.
  The nearest file wins when two disagree. Cursor rules with
  `alwaysApply: true` are included, rules with `globs` are included
  with their globs noted, and rules with only a `description` are
  listed for the agent to read when relevant. The instructions are
  capped at 32 KiB, cutting the most general files first and saying
  so. Files are re-read only when they change, and the section sits
  right after the base system prompt, so unchanged files keep the
  prompt cache warm. `AURA_PROJECT_INSTRUCTIONS=off` turns this off.
- `/init` asks the agent, as a normal turn, to study the project and
  write an `AGENTS.md` at its root with the build and test commands,
  layout and conventions. When the project already has one, the agent
  improves it instead of starting over.
- `/context` lists the instruction files the agent loads, with their
  size and whether any were cut or left out.
- File checkpoints: before the file tools first change a file in a
  turn, the terminal saves its content (or its absence) under
  `<data_dir>/checkpoints/<agent>/`, content-addressed, with a manifest
  per turn. Files changed by shell commands are not covered.
- `/rewind` lists the session's turns; `/rewind <n> [code|chat|both]`
  puts files back as they were before turn `n`, cuts the conversation
  back to before its prompt, or both. The rewind is noted in the record
  as a `rewind` system entry.
- `/diff` shows how the files the latest turn changed differ from before
  it; `/diff session` covers the whole session.
- `ToolExecutor::with_file_change_hook` and the `FileChangeHook` trait
  tell a surface before and after each file a tool changes; tools name
  that file through `Tool::changed_path`. `aura-store-snapshot` gains
  `DirSnapshotStore`, a directory-backed snapshot store.
- `web_fetch` tool: terminal and `aura -p` agents can read a public web
  page as markdown. Scripts, styles and navigation are dropped, and
  headings, links, lists and code blocks are kept. Requests time out
  after 20 seconds, follow at most 5 redirects, read at most 2 MiB, and
  refuse binary content, non-HTTP URLs and local or private addresses
  (`AURA_WEB_FETCH_ALLOW_PRIVATE=1` allows those).
- `web_search` tool: terminal and `aura -p` agents can search the web
  through Brave Search (`BRAVE_API_KEY`), Tavily (`TAVILY_API_KEY`) or a
  SearXNG instance (`AURA_SEARXNG_URL`). `AURA_WEB_SEARCH_PROVIDER`
  picks one when several are set. Without a provider, the tool tells
  the agent and the user what to set. Plan and Ask modes allow both web
  tools; Debug mode does not. `aura -p --disallowed-tools` can turn
  either off.
- Post-edit check: after a step in which the agent edited files, the
  terminal (and `aura -p`) runs the project's check command and gives
  the agent the end of the output if it fails. The command comes from
  `AURA_CHECK_COMMAND` or from a `check:` line or a fenced `check` block
  in the project's `AGENTS.md`. Several edits in one step run the check
  once, and a failure that repeats word for word is reported in one
  line. `AURA_CHECK_TIMEOUT` bounds a run (2 minutes by default).

##### Changed

- The terminal now starts in `default` mode, so edits and commands the
  agent proposes wait for approval. Commands you type in the chat still
  run without asking. Set `AURA_TUI_MODE=unrestricted`, or pick that
  mode with `/mode`, to get the old behaviour where nothing asks.
- Ctrl+C while an approval prompt is open cancels the turn instead of
  quitting the terminal.
- `write_file`, `edit_file` and `delete_file` blocks in the chat show
  their change as a diff (with line numbers) instead of the raw
  arguments, and the block header counts added and removed lines.

- Terminal turns have no iteration cap and no 30-minute timeout.
- The terminal compacts history only from 70% context use
  (`AURA_COMPACTION_TRIGGER`). Below it, old tool results are no longer
  trimmed every step, which rewrote the cached prefix.
- Foreground `run_command` in the terminal defaults to a 2-minute
  timeout (was 5 seconds), and agent commands get no stdin, so they
  can't read keystrokes meant for the terminal UI.
- Build and check commands started by the agent loop are killed when
  the turn that started them is cancelled. After a timeout, their
  output is waited for at most 5 seconds.

### Terminal — plan mode

##### Fixed

- `/plan` leaves plan mode when the session is already in it and returns to the previous permission mode (`default` when the session started in plan mode). While plan mode is on, the status line says `/plan to leave`.

##### Added

- `/plan` (or Shift+Tab to the `plan` mode) puts the agent on screen in
  plan mode: its turns get plan-mode instructions, the kernel holds it
  to reads and Markdown notes, and it researches, asks if the request is
  unclear, and saves a plan with the new `create_plan` tool to
  `.aura/plans/<name>_<id>.plan.md` (YAML frontmatter with todos, then
  Markdown). `/build [plan path]` switches the agent back to a mode that
  edits (the one it had before `/plan`, else `default`), makes the
  plan's todos its `todo_write` list and has it implement the plan; the
  plan file's checklist follows the list as the agent works. The plan's
  open todos show above the input, and a PLAN, ASK or DEBUG badge sits
  next to the model name.
- `/mode ask` and `/mode debug` pick the kernel's read-only `ask` and
  `debug` ceilings. They are not in the Shift+Tab cycle.
- `Kernel::switch_mode` changes a live kernel's mode after appending a
  `mode_change` `System` record; submitted `mode_change` transactions
  are rejected as kernel-only. `Kernel::switch_permissions` switches
  the mode and the per-tool permission table together in one
  `mode_change` entry that names both tables (`ModeChangePayload`
  gained `from_permissions` / `to_permissions`); the terminal's mode
  switches go through it. `Policy` keeps the mode (and its snapshot
  hash) behind a lock, and `mode()` / `set_mode()` wrap the swappable
  policy config.
- `create_plan` counts as a Markdown write for the mode gate, so plan
  mode can use it; `todo_write` is a read.

### Kernel — task-runner tools and agent-host records go through the kernel

##### Changed

- `task_done`, `submit_plan` and `get_task_context` are recorded and
  gated like other tool calls (`ToolProposal` with `source`
  `task_executor`). The task runner's handler only runs once policy
  allows the call, so a denied `task_done` doesn't stop the loop or
  save notes. Calls blocked after a `task_done` rejected for having
  no writes are recorded with `source` `task_guard`.
- The agent host's spawn records and recorded agent messages, and the
  turns of agents it runs, reach the RECORD panel live: the scheduler
  hands its kernels the terminal's record observer, and the host
  notifies it after its own writes.
- TOOLCALL rows say why a tool didn't run (`cached result`,
  `blocked by steering`, `handled by task runner`, …).

##### Added

- `ToolRun` (`Executor`, `Preset`, `Local`) and
  `Kernel::process_tool_runs`, replacing
  `Kernel::process_tools_with_presets`. A `Local` handler is polled
  only after policy allows the call and is timed like an executor run.
- `AgentToolExecutor::spawn_local`.
- `write_system_entry` (returns the stored entry),
  `Scheduler::with_record_observer` and `Scheduler::record_observer`.

### Kernel — every tool call is gated and every entry is timed

##### Changed

- Tool results the agent loop already has (per-run cache hits,
  duplicate reads blocked by steering, invalid tool uses) now go
  through the kernel instead of being returned inline. Each one is
  recorded as `ToolProposal` (with `source` `cache`, `steering_block`
  or `invalid_tool_use`), `PolicyDecision` and `ToolExecution`, and a
  policy denial replaces the cached result. The tool itself still
  doesn't run again.
- `KernelConfig.tool_audit_observer` / `ToolAuditObserver::on_tool_entry`
  is now `KernelConfig.record_observer` / `RecordObserver::on_entry`,
  and is called for every entry the kernel appends (prompts, replies,
  reasoning, session start, process completion), not only tool-call
  entries. The RECORD panel no longer skips ids for reasoning entries,
  only shows the active agent's entries, and receives rows in order.

##### Added

- `ToolExecution.duration_ms`: how long the executor ran (`0` when a
  preset result stood in for it). `Reasoning` payloads carry
  `duration_ms` for the whole model call. The RECORD panel shows both
  in the TOOLRES and LLM rows.
- `PresetToolResult` and `AgentToolExecutor::spawn_preset`.

### Terminal — policy decisions stay with their tool calls

##### Changed

- A tool call's policy decision no longer adds its own `policy:` chat
  line, which split a run of agent steps into separate blocks. The
  decision is kept with the call (live and when restored from the
  record) and shown as `policy: allowed by <rule>` when the step is
  expanded. A decision for a call the chat doesn't show still gets
  a `policy: <tool> …` line.
- New `UiCommand::ToolPolicy` and `ActivityData::Tool.policy`.

### Terminal — PROCESSES tab and background work

##### Added

- The right panel has SANDBOX and PROCESSES tabs. PROCESSES lists
  what is running for your agents: agent turns, `task` spawns, agents
  started with `spawn_agent`, background commands, and the processes
  those commands started, including daemons that detached. Each row
  links to the agent that started it.
- The tab labels show counts: SANDBOX the files in the sandbox (not
  inside `.git`; past 10,000 it reads `10000+`), PROCESSES the rows
  still running.
- Opening a row shows its details, like a record entry: owner, parent
  chain, start and end times, the tool call that started it, pid, cwd
  and command or prompt. From there you can jump to the owning agent,
  open a command's log, or kill the row after confirming. Killing a
  turn or task that waits for an approval denies the call.
- The status line's background-job count comes from the same table.
- `task` takes `spawn_mode: "detached"`. The permanent agent works in
  the background, and its reply arrives as a message when it is done.
- Terminal and `aura -p` agents can use `spawn_agent`,
  `send_to_agent`, `agent_lifecycle`, `get_agent_state` and
  `list_agents`. Spawned agents run in their parent's mode, and their
  replies arrive as messages.
- Every command the tools run carries `AURA_AGENT_ID`,
  `AURA_TOOL_USE_ID` and `AURA_HARNESS_PID` in its environment.

##### Changed

- The PROJECTS and AGENTS tabs are written in capitals and drawn like
  SANDBOX and PROCESSES.

### Terminal — model, tokens and cost on each reply

##### Added

- Each AURA reply header shows, at the right edge, the model that
  wrote it, the tokens of that model call (whole prompt, cached or not,
  plus output) and its cost at API list prices. Replies restored from
  the record show the same. Local models show no cost; Claude and
  ChatGPT plan models show the API-equivalent estimate. On narrow
  chats the cost, then the tokens, are dropped first.
- `debug.llm_call` frames and `Reasoning` record entries carry
  `cache_creation_input_tokens` and `cache_read_input_tokens` when the
  provider reports them.

### Designers — Cursor terminal fonts

##### Changed

- Seeded `ui-designer` and `web-designer` tokens use Cursor's workbench
  sans for UI content and Cursor's terminal monospace for terminal
  output, error messages and code. Headings stay on the sans scale.

### Permanent agents — AURA's default roster

##### Added

- AURA's default specialists are seeded as permanent agents alongside
  `explorer` and `reviewer`: `engineer`, `rust-engineer`,
  `frontend-engineer`, `ui-designer`, `web-designer` and
  `product-manager`. Each keeps AURA's role, routing ("spawn it for")
  and guidelines; the two designers carry AURA's design tokens.

##### Changed

- Seeding adds each built-in definition once, also to an existing
  `agent_defs/`, and records it in `agent_defs/.seeded`. A deleted
  built-in is never re-added, and one whose name is already taken is
  skipped.

### Terminal — agent activity in the chat

The terminal chat shows what the agent is doing while it works: each
stretch of thinking and each tool call is a block between your prompt
and the reply, collapsed to one line and expandable.

##### Added

- Thinking streams into a block in the chat (spinner, running clock and
  its last few lines), then collapses to `Thought for 4.2s`. This
  replaces the three-line Thinker strip under the chat.
- Each tool call is a block: `▸ ✓ read_file src/main.rs · 3 lines ·
  12ms`. While it runs it shows a spinner, its elapsed time, the
  arguments as they stream in, and `retrying (n/m)`. A failed call shows
  the first line of its error.
- `Ctrl+O` expands or collapses every block (new blocks follow suit);
  clicking a block's header toggles just that one. Expanded, a tool call
  shows its full arguments and output (up to 200 lines).
- Kernel `Reasoning` records now keep the model's thinking text
  (`thinking`, `thinking_ms`), so reopening a conversation rebuilds its
  thinking and tool-call blocks from the record.

##### Changed

- Reply text that follows a tool call gets its own message below the
  tool's block.
- Tool calls no longer appear as `$ tool` and `✓ tool: …` system lines.

### Recursive agents replace the fleet

Subagents are now ordinary agents started by other agents, and they can
start agents of their own. One node-wide `AgentHost` in
`aura-engine::agents` replaces the four `aura-fleet-*` crates.

##### Changed

- `task` starts a child of a bundled kind to get a job done and returns
  its result. `general_purpose` children get `task` and `spawn_agent`
  too, so work can be split recursively, down to `AURA_SUBAGENT_MAX_DEPTH`
  levels (default 8). A task child is removed once its job is done.
- `spawn_agent` starts a long-lived named child. It keeps its
  conversation across `send_to_agent` / `delegate_task` messages until
  `agent_lifecycle` stops it. It can take an `initial_prompt`.
- A detached `task` returns at once; the child's result arrives later as
  a message. In a chat session that message starts a new turn once the
  session is idle.
- Children may only narrow what their parent has: mode, permissions,
  model and tools. A spawn that asks for more permissions is rejected.
- `AURA_FLEET_DEFAULT_MODE` is renamed `AURA_DEFAULT_MODE`
  (`AgentConfig::default_mode`). The old name is still read when the new
  one is unset.
- New `AURA_SUBAGENT_MAX_RUNNING` (default 32) caps how many spawned
  agents run at once across the node.

##### Removed

- The `aura-fleet-registry`, `aura-fleet-quota`, `aura-fleet-spawn` and
  `aura-fleet-subagent` crates, and the `fleet` layer.
- `SpawnMode::Batch` and `JoinPolicy`. Run several `task` calls in one
  turn instead.
- Orphan files under `~/.aura/state/orphans/` and the
  `aura agents inspect` / `aura agents reap` commands. Stopping an agent
  now stops its whole subtree.

### Terminal — permanent agents

The terminal's agents panel has two tabs (it has no border title; the
tab bar names it). **Projects** lists the live
agents, and **Agents** is the roster of permanent agents: reusable
definitions that any agent can spawn by name. With the panel focused,
←/→ switches tabs, and clicking a tab shows it.

##### Added

- Permanent agents are Markdown files in `<data_dir>/agent_defs/`. YAML
  frontmatter sets `name`, `description`, and the optional `model`,
  `tools` (an allow-list; default is every built-in tool except `task`)
  and `skills`. The body is the rules the agent follows. `explorer` and
  `reviewer` are seeded the first time the directory is created.
- The roster spawns the selected permanent agent (Enter) and then shows
  Projects with the new instance. It also edits it in
  `$VISUAL`/`$EDITOR` (`e`), creates one (`n` or the `+` row), and
  deletes it after confirmation (`d`). `/spawn <name>` starts an
  instance from the chat.
- The terminal's agents get the `task` tool. Its `subagent_type` is
  one of the permanent agents. The call registers an instance (for
  example `reviewer #1`) under the calling agent, runs it with the
  definition's rules, skills, tools and model, and returns its final
  reply. Instances stay under Projects, indented under their parent
  and marked with a spinner while they work. Switching to an instance
  continues its conversation under the same rules.
- `agents.json` entries gain optional `definition` and `parent` fields.
  Existing files load unchanged.

### Auditable tool calls — policy decisions in the record

Every tool call is now three record entries, so the record shows which
policy rule allowed (or denied) each call.

##### Breaking changes

- Record schema `SCHEMA_VERSION` is now `3`. A tool call is recorded
  as a bare `ToolProposal` entry, then a `PolicyDecision` entry
  (references the proposal hash; carries the decision), then, for
  approved calls only, a `ToolExecution` entry (references the
  decision hash; carries the actions and effects). Batched calls record
  every proposal/decision pair before any execution. The kernel writes
  only v3; earlier entries stay readable.
- `ProcessResult.entry` for a tool call is the last entry written (the
  execution, or the decision when the call did not run). The new
  `ProcessResult.policy_decision` points at the decision and proposal.
- `PolicyDecision` and `ToolExecution` transactions are kernel-only:
  `process_direct` / `process_dequeued` reject them.

##### Added

- `aura_core_types::PolicyDecision`: the verdict, the deciding
  `PolicyRule` (`source` + `rule`, e.g.
  `user_default.per_tool[run_command]=on`), the trace of every gate
  evaluated, any live approval (hook or user, with its remember scope),
  the agent mode, and a hash of the evaluated policy configuration.
- `KernelConfig.tool_audit_observer` (`ToolAuditObserver`): called as
  each tool-call entry is persisted.
- WebSocket `tool_policy_decision` outbound message, sent when a
  decision is recorded and before the tool runs.
- `GET /agents/:agent_id/tool_calls/:tool_use_id/audit` returns the
  proposal, decision, and execution entries for one tool call.
- Terminal UI: tool-call entries appear live in the RECORD panel, each
  decision adds a `policy: <tool> allowed by <rule>` chat line, and the
  record detail view has a "Policy decision" section.

### Simplification — enforce modes, collapse crates, sync docs

Three phases that make the runtime match what it documents: the
`AgentMode` enum is now enforced, scaffolding crates with no
production caller are gone, and the principles are restated as the
four the code actually upholds. The workspace goes from 51 to 39
member crates.

#### Phase 1 — `AgentMode` enforcement (commit `421421e`)

##### Breaking changes

- `aura_protocol::RuntimeRequest` gains `mode: Option<AgentMode>`
  (`"agent" | "plan" | "ask" | "debug"`, absent = `agent`).
  Consumers that mirror the struct (e.g. `aura-os`) must add the
  field to round-trip it.
- `POST /v1/run` with `task_run` / `dev_loop` and a resolved mode
  other than `agent` returns `400`. Setting
  `AURA_FLEET_DEFAULT_MODE` to a non-agent mode therefore rejects
  those requests unless they pass `mode: "agent"` explicitly.

##### Added

- Mode gate (`aura-agent-kernel` `policy/check/mode_gate.rs`): the
  first stage of `evaluate_delegate`. Every catalog tool maps to a
  `ToolModeClass` via an explicit table; unclassified tools are
  treated as `Network`, and file-write tools targeting `.md` /
  `.markdown` paths are classified as `WriteMarkdown` so Plan mode
  can write plans. Denials are recorded with the mode and violation in the
  reason.
- `PolicyConfig::mode` / `with_mode`, plumbed through the gateway
  session helpers, `child_runner` (child specs keep their own
  mode), and the CLI runner.
- `aura_core_modes::resolve_session_mode` + `AgentModeInputs`
  (moved from `aura-fleet-daemon`) and the `aura_config::session_mode`
  wrapper: request → `AURA_FLEET_DEFAULT_MODE` → `agent`.
- `aura --mode <agent|plan|ask|debug>` CLI flag.
- Tests: per-mode policy matrix, mode-before-tool-state ordering,
  a kernel Plan-mode denial/record test, and
  `aura-engine/tests/mode_gate_catalog.rs`, which fails if a
  catalog tool lacks a classification.

##### Removed

- `core_to_modes_mode` conversion shim.

#### Phase 2 — crate collapse (commits `a14a22d`, `5a1b294`)

##### Removed

- Crates with no production caller: `aura-fleet-daemon`,
  `aura-fleet-dispatch`, `aura-fleet-mailbox`, `aura-exec-policy`,
  `aura-exec-conflict`, `aura-surface-sdk`, plus
  `tests/sdk_smoke.rs` and `tests/daemon_shutdown.rs`.
- Re-export shells, with dependents repointed at the owning crate:
  `aura-exec-tools` / `aura-exec-runner` → `aura-tools`,
  `aura-agent-loop` → `aura-agent`, `aura-surface-automaton` →
  `aura-automaton`, `aura-surface-auth` → `aura-auth`,
  `aura-surface-terminal` → `aura-terminal`.
- The `tests/layer_boundary.rs` warn-only allowlist; the test is
  now `no_upward_layer_dependencies` and fails on any upward edge.

##### Changed

- `aura-terminal` owns the `/mode` slash-command parser
  (`SlashModeCommand`) formerly in `aura-surface-terminal`. It is
  still not wired into the TUI input loop.
- Doc comments that described removed or unused machinery now say
  so (`SessionStop` is reserved with no production writer,
  `load_enabled_plugins` has no production caller, the conflict
  config knobs have no consumer, fleet registry/quota/lease state
  is per chat session).

#### Phase 3 — principles and docs

##### Changed

- `README.md` is now a short landing page: what AURA is, the four
  principles (per-agent audit log, mediated effects, one writer per
  agent, the wire API is the contract), how to run, links to the
  reference pages, directory structure and license. The reference
  material moved to new pages: `docs/running.md`, `docs/api.md`
  (now listing every route, including workspace, secrets and
  processes), `docs/cli.md`, `docs/configuration.md` and
  `docs/development.md`.
- `docs/invariants.md`: reorganized under the four principles with
  every `§N` anchor preserved; adds §4.0 (mode gate), a definition
  of "deterministic", and corrects §5.a (AuditedLite replay ends in
  `SnapshotMissing`). Declared exceptions are frozen.
- `docs/architecture.md`: new layer map, `AgentMode` resolution
  chain, tool authorization pipeline diagram, and removal of the
  deleted crates.

### Architecture refactor — aura-runtime gateway refactor (Phases A–C)

Phases A through C of the `aura-runtime gateway refactor` plan
(`.cursor/plans/aura-runtime_gateway_refactor_09dbd8ab.plan.md`)
collapse `aura-runtime` into a thin HTTP/WS gateway + composition
root by (1) unifying the two divergent session-start request
shapes into one canonical wire-level `RuntimeRequest`, (2) lifting
the orchestration engine + subagent dispatcher out of the gateway
crate, and (3) reorganizing the remaining gateway files into a
handler-grouped layout. Each phase ended green on
`cargo fmt --check`, `cargo clippy -D warnings`, `cargo check
--workspace`, `cargo test --all --all-features`, and the
end-to-end smoke check (`POST /v1/run` → `WS /stream/:run_id`
round-trip).

#### Phase A — Wire-shape unification (commits `ce1cbaf` + `7fe0229`)

##### Breaking changes

- Collapse `SessionInit` + `AutomatonStartRequest` into a single
  canonical [`aura_protocol::RuntimeRequest`] shape. The wire
  surface moves from `WS /stream` + first-frame `session_init`
  and `POST /automaton/start` to `POST /v1/run` +
  `WS /stream/:run_id`. The legacy types and endpoints are
  deleted with no aliases. External consumers of `aura-protocol`
  (e.g. `aura-os`) must migrate; the sibling-repo migration
  landed back-to-back in `aura-os` on its own `main`.
- Rename `aura_protocol::AgentIdentityWire` to
  `aura_protocol::AgentPersona`, now nested as the `persona`
  field inside the new `AgentIdentity` wrapper.
- Remove `InboundMessage::SessionInit` variant. WS-protocol
  tests that exercised the deleted first-frame contract are
  deleted.
- Lifecycle endpoints rename for consistency: `/automaton/list`
  / `/automaton/:id/status` / `/automaton/:id/pause` /
  `/automaton/:id/stop` become `/v1/run/list` /
  `/v1/run/:id/status` / `/v1/run/:id/pause` /
  `/v1/run/:id/stop`.

#### Phase B — Engine extraction + subagent into agent layer (commit `97fcce6`)

##### Internal refactor

- Extract orchestration engine into new `aura-engine` crate
  (surface layer): `scheduler.rs`, `worker.rs`,
  `automaton/{mod,build,dispatch,event_channel}.rs`,
  `memory_observer.rs`, `capabilities.rs` (formerly
  `runtime_capabilities.rs`), `executor.rs` (formerly
  `executor_factory.rs`), `model_context.rs`, plus the
  surface-layer `RuntimeChildRunner` impl of
  [`aura_fleet_spawn::ChildRunner`].
- Split subagent dispatch across three layers: `aura-tools`
  (exec) still declares `SubagentDispatchHook`;
  `aura-agent-subagent` (agent) absorbs `SubagentRegistry` +
  bundled kinds + the pure-data adapter helpers
  (`parent_context_from_request`, `overrides_from_request`,
  `narrow_permissions`, `legacy_permissions_to_modes`,
  `core_to_modes_*`); new `aura-fleet-subagent` (fleet) owns
  the concrete `FleetSubagentDispatcher` impl. All edges
  remain downward — no new `WARN_ONLY_UPWARD_EDGES` entries.
  The `task_tool_*` integration tests move with the dispatcher
  into `crates/aura-fleet-subagent/tests/`.
- Move top-level `tool_permissions.rs` helpers
  (`load_agent_tool_context`, `validate_*`) into
  `aura-tools::permissions` (the router-layer HTTP handler
  stays in `aura-runtime`).
- `aura-surface-cli` import paths updated:
  `aura_runtime::memory_observer::MemoryTurnObserver` →
  `aura_engine::MemoryTurnObserver`. `aura_runtime::*::*`
  legacy paths preserved via thin re-export modules in
  `aura-runtime/src/lib.rs` (`scheduler`, `memory_observer`).

#### Phase C — Infrastructure extraction + gateway reorg (commit `2a4ec67`)

##### Internal refactor

- Pull HTTP `DomainApi` (`domain.rs` + `jwt_domain.rs`) into a
  new `aura-domain-http` crate (surface layer); `aura-runtime`
  no longer depends on `reqwest` for domain calls. (Note:
  `reqwest` is retained as a direct dep of `aura-runtime`
  because `gateway/session/cross_agent_hook.rs` still issues
  outbound HTTP for the cross-agent callback path.)
- Move terminal PTY WebSocket handler from
  `aura-runtime/src/terminal.rs` to `aura-terminal/src/ws.rs`;
  gateway keeps a one-line `terminal_ws_handler` in
  `gateway/middleware.rs` that delegates to
  `aura_terminal::ws::handle_terminal_ws`. `portable-pty`
  drops out of the `aura-runtime` deps.
- Reorganize `aura-runtime/src/router/` into
  `aura-runtime/src/gateway/`:
  `mod.rs` (module declarations + `pub use`),
  `middleware.rs` (`create_router` + middleware stack +
  health + terminal-WS delegate), `state.rs`
  (`RouterState` + `RouterStateConfig`), `handlers/*`
  (per-endpoint bundles: `run.rs`, `run_ws.rs`, `files.rs`,
  `tx.rs`, `memory/`, `skills.rs`, `tool_permissions.rs`,
  `util.rs`), `session/*` (per-WebSocket-connection protocol
  layer: `chat.rs`, `cross_agent_hook.rs`, `generation.rs`,
  `helpers.rs`, `partial_json.rs`, `state.rs`, `tool_approval.rs`).
- Workspace `Cargo.toml`: add `aura-engine`, `aura-domain-http`,
  `aura-fleet-subagent` to `[workspace].members` with matching
  `KNOWN_CRATES` entries and `//! Layer:` doc-tags. Trim
  `aura-runtime/Cargo.toml` deps that moved out (`aura-fleet-*`
  for spawn/registry/quota, `aura-memory`, `aura-automaton`,
  `aura-skills`, `aura-agent-subagent` reached only via the
  engine, `aura-prompts`, `aura-context-skills`,
  `aura-reasoner`, `aura-kernel`, `portable-pty`, `which`);
  add the new surface deps (`aura-engine`, `aura-domain-http`,
  `aura-terminal`, `aura-fleet-subagent`).

### Architecture refactor — Phase 10 residual cleanup

`refactor(arch-phase-10)` consolidates the five carve-outs the
prior subagents flagged but couldn't cleanly fit into the
tightened scopes of Phases 7b, 8, and 9. Each carve-out is paired
with a fresh test pin, and the closed-taxonomy schema bump is
done in one combined increment so the on-disk audit log never
sees an intermediate half-migrated shape.

- **Carve-out 1 — Binary body migration.** The full body of
  `src/main.rs` (the `aura` binary) and
  `crates/aura-runtime/src/main.rs` (the `aura-node` binary) is
  lifted out of the binary crates. The new entry points
  [`aura_surface_cli::run`] and [`aura_runtime::run_node`]
  (re-exported as [`aura_surface_cli::run_node`] to keep the
  documented surface path callable) own every line of useful
  logic. The two `main.rs` files are reduced to ≤ 10 lines
  apiece — a `tokio::main`, a `dotenvy::dotenv` call, optional
  tracing-subscriber init, and the delegated `run().await` /
  `run_node().await`. The `tests/cli_golden.rs` pins remain
  byte-identical across the rename.
- **Carve-out 2 — `RecordKind::SessionStop`.** A new variant
  emitted by [`FleetDaemon::run`] at session-end via
  [`aura_agent_kernel::write_system_record`]. Carries
  `session_id`, `agent_id`, `total_iterations`,
  `total_input_tokens`, `total_output_tokens`, `duration_ms`,
  and `clean_shutdown: bool`. Sessions registered through the
  new `FleetDaemon::record_session` API are emitted as one row
  each on shutdown.
- **Carve-out 3 — Spawn-record on-disk wire format.**
  `SubagentSpawn` audit rows now use the typed
  `TransactionType::SubagentSpawn` variant instead of the
  Phase 7a `TransactionType::System` + JSON discriminator
  workaround. The payload retains the same `OverrideManifest`
  shape but no longer carries the `kind: "subagent_spawn"`
  field. The `SubagentResult` Phase 7a guarantee (byte-identical
  to callers who don't override anything) is preserved — only
  the on-disk audit row changes shape.
- **Carve-out 4 — `daemon.run()` external shutdown token.**
  [`FleetDaemon::run`] reshapes to
  `pub async fn run(&self, shutdown: CancellationToken) -> Result<(), FleetDaemonError>`.
  The mailbox loop selects on `shutdown.cancelled()` alongside
  the receiver; on shutdown it cancels every in-flight child
  through the daemon-owned `fleet_shutdown` token, polls
  [`FleetRegistry::running_count`] for cooperative drain inside
  the configurable grace window (default 30s,
  [`DaemonConfig::shutdown_grace`]), hands off
  `SpawnMode::Detached` survivors to the [`OrphanStore`], and
  emits one `RecordKind::SessionStop { clean_shutdown }` per
  tracked session.
- **Carve-out 5a — `PreToolUse` mid-flight cancellation.**
  `PreToolUse` hooks now fire BEFORE tool dispatch inside the
  kernel (`crates/aura-agent-kernel/src/kernel/tools/single.rs`
  + `batch.rs`). `HookOutcome::Block` aborts the dispatch and
  produces a synthetic `Effect::failed` whose payload embeds the
  documented `{"kind": "tool_call_blocked_by_hook", "tool_name":
  ..., "tool_use_id": ..., "reason": ...}` JSON discriminator so
  the audit log captures the block without consuming a separate
  sequence number (the single tool-proposal `RecordEntry` is the
  one source of truth). The agent loop sees a clean rejection
  through the synthetic effect.
- **Carve-out 5b — `PermissionRequest` kernel-side wiring.**
  [`KernelConfig`] gains a `pub plugin_hooks:
  Option<Arc<PluginHookHost>>` field (defaulting to `None` for
  backward compat). When set, `Kernel::resolve_prompt_verdict`
  fires `PermissionRequest` hooks BEFORE the interactive
  `ToolApprovalPrompter` runs. `HookOutcome::Approve`
  short-circuits to `PolicyVerdict::Allow`; `HookOutcome::Deny`
  short-circuits to `PolicyVerdict::Deny`; any other outcome
  falls through to the interactive prompt.
- **Carve-out 5c — `ConnectorRegistry` last-wins API.**
  [`ConnectorRegistry`] gains `replace` and `remove` methods.
  The Phase 8 plugin materialiser
  (`aura-plugin-core::runtime::load_plugin_contributions`) now
  calls `replace` instead of `register` when a plugin-supplied
  connector conflicts with an existing one, logging the
  displacement with `tracing::info!`.

#### Schema-version bump — `aura-store-record::SCHEMA_VERSION`

The closed-taxonomy enum additions are combined into a single
schema-version increment: `SCHEMA_VERSION` advances from `1`
(implicit in Phases 1 through 9 — the constant did not yet
exist) to `2`. The new variants are
`RecordKind::SessionStop`,
`RecordKind::ToolCallBlockedByHook`, and
`TransactionType::SubagentSpawn`. Pre-bump audit rows continue
to deserialize: serde-default arms for `RecordKind` and
`TransactionType` fall back to the Phase 2 forward-compat
`Unknown` variants, so an older `aura-node` reading a
schema-v2 log degrades to "I don't know that variant, skip it"
rather than panicking. The kernel WRITES only the new format
going forward. Operators with mixed-version fleets should
upgrade readers before writers; otherwise pre-bump readers
will treat all SessionStop / ToolCallBlockedByHook /
SubagentSpawn rows as `Unknown`-class — recoverable but not
queryable. There is no destructive migration step.

### Architecture refactor — Phases 1 through 9

The architecture refactor lands the 10-layer crate stack (`core` →
`store` → `config` → `model` → `context` → `plugin` → `exec` →
`agent` → `fleet` → `surface`) with strict downward-only
dependencies enforced by `tests/layer_boundary.rs`. Every phase
left the workspace `cargo fmt` / `cargo clippy -D warnings` /
`cargo test --workspace` green.

- **Phase 1** — `refactor(arch-phase-1)` (`1851ad6`): split
  `aura-core` into `aura-core-types`, `aura-core-protocol`,
  `aura-core-permissions`, `aura-core-modes`, and `aura-core-auth`;
  introduce the closed `AgentMode` / `KernelMode` / `SpawnMode` /
  `JoinPolicy` / `ReplayMode` / `SandboxMode` enums; add
  advisory `tests/layer_boundary.rs`.
- **Phase 2** — `refactor(arch-phase-2)` (`7307b01`): split the
  store layer into `aura-store-db`, `aura-store-record`,
  `aura-store-snapshot` (the snapshot crate ships as a V1 no-op
  stub); preserve `aura-store` as a compatibility shell.
- **Phase 3** — `refactor(arch-phase-3)` (`6d2f69d`): rename the
  model + context crates into the layered `aura-model-reasoner`,
  `aura-context-prompts`, `aura-context-memory`,
  `aura-context-compaction`, `aura-context-skills`; add the
  `aura-agent-steering` placeholder.
- **Phase 4a** — `refactor(arch-phase-4a)` (`e15ff72`): extend
  `aura-config` with the fleet / subagent / conflict / plugins
  TOML tables, the `AURA_HOME` resolution helper, and the
  `aura migrate` stub.
- **Phase 4b** — `refactor(arch-phase-4b)` (`e8108ce`): plugin
  schema + install + cache + marketplace + `aura plugins` CLI;
  introduces `aura-plugin-api` (in-process contributor traits)
  and `aura-plugin-core` (declarative manifest pipeline).
- **Phase 4c** — `refactor(arch-phase-4c)` (`89201f6`): plugin
  runtime surfaces — `aura-plugin-hooks` (hook engine + 10
  Codex/Claude lifecycle events), `aura-plugin-mcp` (stdio
  JSON-RPC client), `aura-plugin-connectors`, plus the trust
  prompt flow.
- **Phase 5** — `refactor(arch-phase-5)` (`70f18ee`): exec layer
  split — `aura-exec-conflict`, `aura-exec-isolation`,
  `aura-exec-policy`, `aura-exec-sandbox`, `aura-exec-tools`,
  `aura-exec-runner`; introduce `ConflictRegistry` and
  `WorktreeIsolation`.
- **Phase 6a** — `refactor(arch-phase-6a)` (`a316784`) +
  follow-up `refactor(arch-phase-6a-audit)` (`5a220dc`): agent
  layer split — `aura-agent-kernel`, `aura-agent-loop`,
  `aura-agent-subagent`, `aura-agent-steering`; introduces the
  `KernelMode::{Audited, AuditedLite}` tiering.
- **Phase 6b** — `refactor(arch-phase-6b)` (`48356ba`): replay
  wiring — `ReplayConsumer`, `ReplayError`, `RecordLog::scan`,
  `KernelConfig::replay_from`.
- **Phase 6c** — `refactor(arch-phase-6c)` (`8aa9441`):
  context-memory inversion — `TurnSummary` + `ModelProvider`
  injection breaks the four remaining `aura-context-memory ->
  aura-agent` upward edges; `MemoryTurnObserver` relocated to
  `aura-runtime`; first warn-only edge promoted to fail-on-detect.
- **Phase 7a** — `refactor(arch-phase-7a)` (`682e888`): fleet
  daemon composition root — `aura-fleet-registry`,
  `aura-fleet-spawn`, `aura-fleet-dispatch`, `aura-fleet-quota`,
  `aura-fleet-daemon`; per-parent audit-append lease replaces
  the old single `spawn_lock`; task tool routed through
  `aura-agent-subagent::derive_subagent ->
  aura-fleet-spawn::spawn`.
- **Phase 7b** — `refactor(arch-phase-7b)` (`5f90526`): full
  `SpawnMode::{Detached, Batch}` + `aura-fleet-mailbox` +
  `JoinPolicy` enforcement + quota enforcement + orphan handoff
  on parent death + the complete `SubagentOverrides` /
  `OverrideManifest` validation surface.
- **Phase 8** — `refactor(arch-phase-8)` (`dcfd0c9`): plugin
  runtime integration end-to-end — skills wired into
  `aura-context-skills`, MCP merge into `aura-plugin-mcp`, hooks
  into `aura-plugin-hooks`, connectors into
  `aura-plugin-connectors`; hook events fired at every documented
  agent-loop / fleet-spawn lifecycle point; hook sandbox env
  scrubbing; backward compat verified (empty `~/.aura/plugins/`
  yields zero behavioural diff).
- **Phase 9** — `refactor(arch-phase-9)`: surface layer + SDK +
  strict layer boundary enforcement. See the per-section list
  below.

### Added — Phase 9

- New `aura-surface-cli` crate at the `surface` layer hosting the
  CLI composition-root types (`ModeFlag`, `parse_mode_str`,
  `CliModeInputs`, `version_banner`).
- New `aura-surface-sdk` crate exposing the external
  `AuraClient` / `AuraSession` shape over `aura-core-protocol`,
  with the documented `SessionConfig::mode: Option<AgentMode>`
  field that feeds the AgentMode resolution priority chain.
- New `aura-surface-terminal` relocation shell over the legacy
  `aura-terminal` crate. Adds the typed `SlashModeCommand` for
  parsing `/mode <agent|plan|ask|debug>` from the TUI input
  layer.
- New `aura-surface-automaton` relocation shell over the legacy
  `aura-automaton` crate.
- New `aura-surface-auth` relocation shell over the legacy
  `aura-auth` crate (`ZosClient`, `CredentialStore`). The token
  primitive types (`AccessToken`, `RefreshToken`, `Token`,
  `StoredSession`, `AuthError`) stay at the `core` layer in
  `aura-core-auth` and are re-exported through the surface
  shell.
- `FleetConfig::default_mode: AgentMode` field — the daemon
  default rung of the documented `AgentMode` resolution priority
  chain. Overridable via the new `AURA_FLEET_DEFAULT_MODE`
  environment variable.
- `aura_fleet_daemon::resolve_session_mode` helper +
  `AgentModeInputs` struct implementing the documented priority:
  CLI flag > TUI slash > SDK field > daemon default >
  `AgentMode::Agent` fallback. Children inherit the resolved
  mode through the existing `derive_subagent` chain (narrowing
  only).
- Top-level `aura --mode <agent|plan|ask|debug>` global CLI flag
  (clap-derived `ModeFlag` from `aura-surface-cli`).
- `tests/layer_boundary.rs` strict-mode promotion. The
  `WARN_ONLY_UPWARD_EDGES` allowlist now carries a single
  explicitly-documented Phase 10 follow-up entry
  (`aura-tools -> aura-kernel`); every other upward edge fails
  CI. Phase 9 also introduces the
  `every_crate_carries_a_matching_layer_doc_tag` test that
  parses each `crates/<name>/src/lib.rs` for the
  `//! Layer: <layer>` doc-comment and asserts consistency with
  the `KNOWN_CRATES` table.
- `crates/aura-fleet-daemon/tests/mode_resolution_priority.rs`
  with an `insta` snapshot pinning the priority order across
  ten input combinations.
- New `machete` job in `.github/workflows/ci.yml` running
  `cargo machete` against the workspace.

### Changed — Phase 9

- `aura-agent` no longer depends on `aura-auth`.
  `default_agent_config(model)` now delegates to the new
  `default_agent_config_with_auth(model, auth_token)` helper;
  surface-layer callers (`src/session_helpers.rs`) compose the
  env-var lookup (`load_auth_token`) with the credential-store
  lookup (`aura_auth::CredentialStore::load_token`) before
  invoking it.
- `aura-auth` reclassified from `core` to `surface` in
  `KNOWN_CRATES` since `ZosClient` (HTTP) and `CredentialStore`
  (OS keyring) are surface-layer concerns per the plan's
  cross-cutting "Secrets" ownership bullet.
- `deny.toml` finalised: explicit MIT / Apache-2.0 / BSD / ISC
  / MPL / Unicode allow-list; 30-day advisory window enforced
  via the `[advisories]` block (paired with the documented
  `audit` job soft-fail in `.github/workflows/ci.yml`);
  crates.io-only sources. The `openssl-sys` ban is documented
  as a TODO (the workspace's `reqwest 0.11` still pulls in
  native-tls today; flipping to `rustls-tls` is tracked as a
  Phase 10 follow-up). Duplicate-major detection for `tokio` /
  `serde` runs via the existing `multiple-versions = "warn"` +
  `highlight = "all"` toggle.
- `aura --help` now lists the top-level `--mode` global flag.
- The `audit` CI job retains `continue-on-error: true` (the
  hard-fail gate is `cargo deny check`'s 30-day advisory window
  in `deny.toml`); the YAML now carries an explanatory comment.
- `docs/architecture.md` carries a Phase 9 layer overview +
  refreshed per-layer crate inventory.

### Deprecated — Phase 9

- `aura_agent::session_bootstrap::load_auth_token` no longer
  reads the OS keyring; surface-layer callers chain
  `aura_auth::CredentialStore::load_token` explicitly. The
  agent-layer function remains for the env-var path.

### Notes — Phase 9 follow-ups

- `WARN_ONLY_UPWARD_EDGES` retains a single
  `aura-tools -> aura-kernel` entry. The clean fix is to
  relocate `ExecuteContext` / `Executor` / `ExecutorError` /
  `SpawnHook` out of `aura-agent-kernel` to the exec layer.
  Tracked as a Phase 10 follow-up; documented in the test
  source.
- The full body migration of `src/main.rs` (the `aura` binary)
  and `crates/aura-runtime/src/main.rs` (the `aura-node`
  binary) into `aura-surface-cli::run` is incremental. Phase 9
  ships the surface-layer type and dependency topology;
  Phase 10 lifts the binary bodies.
- The `openssl-sys` ban in `deny.toml` is documented as a TODO
  pending the workspace migration to `reqwest`'s `rustls-tls`
  feature.
