---
title: "Gate: Routing Every Turn to the Cheapest Model That Gets It Right"
slug: gate-router-design
summary: How Aura's Gate router starts work on a cheaper model, checks what it did, moves it up only when it fails, and learns from its own record. On 20 hard SWE-Bench Pro tasks it matched Opus 5.5 at 52% of the cost.
---

## Abstract

Most model routers decide once, before any work is done, using a classifier trained offline on one-shot prompts. A coding agent's turn is different: it runs for minutes, calls the model dozens of times, and ends in something that can be checked. Gate is the router behind Aura's `gate` model id. It labels each turn, starts it on the cheapest model proven good enough for that kind of work, reads the evidence the turn leaves behind, and moves the work to a stronger model only when the evidence says the model failed. Everything it learns is a fold over Aura's append-only record, so there is no training run, and replaying the record rebuilds the same routing table.

On 20 tasks from the HARD-51 subset of SWE-Bench Pro, each run once on Gate and once on Claude Opus 5.5 alone, both arms passed all 20 hidden test suites. Gate cost $8.40 against Opus's $16.20, 48% less, and took 111 minutes of wall time against 128. That includes Gate's own classifier, criteria, judge and review calls. Getting there took seven comparisons on the same tasks, and in three of the first five Gate cost more than Opus. This post describes the design, the evidence, and what broke along the way.

## 1. The idea in simple terms

Picture a workshop with three engineers: a junior who is fast and cheap, a mid-level engineer, and a principal who is excellent and costs twice as much as the mid-level one. Giving every ticket to the principal works, but most of the money goes on tickets the mid-level engineer would have closed just as well. Giving every ticket to the junior is cheap until the hard ones come back broken.

A good lead does three things. They size up each ticket before handing it out. They check the work when it comes back, and run the tests rather than taking the engineer's word for it. And when something fails, they ask whether the engineer got it wrong or the build was already broken, before passing the ticket to someone more senior. Over time they learn who can be trusted with what.

Gate does the same with models. Each turn starts with a line saying what it chose and why:

```text
gate → claude-sonnet-5-5 (claude plan) at high effort · multi_file_change, default
```

That line names the model, who pays for it (here, the user's Claude plan), the reasoning effort, the category the prompt was given, and the reason for the choice: `default` (where this kind of work starts), `proven` (the cheapest model the record shows is good enough) or `explore` (a deliberate try of a cheaper model, so the table keeps learning).

## 2. How others route

Routing between models of different price and strength has grown into a field of its own, and an industry. OpenAI describes GPT-5 as "a unified system" with "a real-time router that quickly decides which to use based on conversation type, complexity, tool needs, and your explicit intent", trained on signals such as when users switch models.[1](https://openai.com/index/introducing-gpt-5/) OpenRouter sells a router tuned for coding agents that rates each turn with a fast decision model and weighs measured pass rate, speed and cost per solved task.[2](https://openrouter.ai/docs/guides/routing/routers/auto-code.md) Not Diamond sells routing for coding agents on the promise of frontier quality at a fraction of the price.[3](https://www.notdiamond.ai/) The research comes in roughly five threads.

**Cascades: try cheap first, defer on doubt.** FrugalGPT is an LLM cascade that learns which combinations of models to use for which queries. It reports matching GPT-4 with up to 98% lower cost.[4](https://arxiv.org/abs/2305.05176) Later work asks what the deferral signal should be: whether a weaker model's sampled answers agree with one another,[5](https://arxiv.org/abs/2310.03094) or token-level uncertainty, since uncertainty over a whole sequence is biased by its length.[6](https://arxiv.org/abs/2404.10136) AutoMix has the small model verify its own answer with a few-shot prompt, then routes on that noisy signal, and reports over 50% lower cost at comparable quality.[7](https://arxiv.org/abs/2310.12963)

**Learned routers: decide before the work.** Hybrid LLM trains a router to predict a query's difficulty and send it to a small or a large model, making up to 40% fewer calls to the large one.[8](https://arxiv.org/abs/2404.14618) RouteLLM trains routers on Chatbot Arena preference data to choose between a strong and a weak model.[9](https://arxiv.org/abs/2406.18665) Its authors report cost reductions of over 85% on MT Bench at 95% of GPT-4's quality.[10](https://www.lmsys.org/blog/2024-07-01-routellm/) RouterBench collected 405,000 inference outcomes to compare such routers on a common footing,[11](https://arxiv.org/abs/2403.12031) and a 2025 survey reviews the routing strategies proposed since.[12](https://arxiv.org/abs/2502.00409)

**Routing as a policy.** Router-R1 makes the router an LLM itself, trained with reinforcement learning to call models over several rounds and combine their answers.[13](https://arxiv.org/abs/2506.09033) Arch-Router routes on preferences the user defines, matching each query to a domain and action the user has mapped to a model.[14](https://arxiv.org/abs/2506.16655) Anthropic's guide to agent design treats routing as a basic workflow: "routing classifies an input and directs it to a specialized followup task", and works well "where classification can be handled accurately".[15](https://www.anthropic.com/research/building-effective-agents)

**Routing agents, not prompts.** Most of the above route a single prompt to a single answer. MasRouter extends routing to multi-agent systems, choosing collaboration modes and roles as well as models.[16](https://arxiv.org/abs/2502.11133) ProgRouter argues that "existing cascade routing methods make one-shot, query-level decisions" and routes each step of a workflow by its progress so far.[17](https://arxiv.org/abs/2608.25992) TwinRouterBench makes the same point about evaluation: router benchmarks "evaluate routers only on one-shot prompts" and "never test whether a cheaper replacement preserves downstream task success". Its benchmark scores routers on SWE-bench by real task resolution and real API spend.[18](https://arxiv.org/abs/2605.18859) Triage is the closest in spirit to Gate. It routes software engineering tasks across light, standard and heavy tiers ("e.g., Haiku, Sonnet, Opus") to "the cheapest model tier whose output passes the same verification gate as the expensive model".[19](https://arxiv.org/abs/2604.07494)

**When cheap routing goes wrong.** A recent audit across 19 RouterBench task families found that the cheapest router that passed its audit went on to violate its service contract on new requests in 32% of deployments.[20](https://arxiv.org/abs/2611.20417) Online formulations treat the choice as an ongoing experiment with regret bounds, querying models in turn and learning only from the outcome of the answer it deploys.[21](https://arxiv.org/abs/2606.07392) That is the contextual-bandit setting, in which exploration choices are logged with their probabilities so that other policies can be evaluated offline on the same data.[22](https://arxiv.org/abs/1003.0146)

Two older ideas carry much of the weight in Gate's design. Strong models can serve as judges of other models' work,[23](https://arxiv.org/abs/2306.05685) and an agent retried with written feedback on what went wrong does better than one simply retried.[24](https://arxiv.org/abs/2303.11366)

| | When it decides | What it routes on | How it learns | Unit | How the result is checked |
|---|---|---|---|---|---|
| FrugalGPT, AutoMix | after each answer | the answer's estimated reliability | offline, or few-shot | query | learned scoring or self-verification |
| RouteLLM, Hybrid LLM | before the work | predicted win rate or difficulty | offline, on preference or benchmark data | query | not checked |
| Router-R1 | during, over rounds | an LLM policy | reinforcement learning | query | rule-based reward |
| GPT-5's router | before each reply | conversation type, complexity, intent | continuously, on user signals | message | not stated |
| OpenRouter Auto Code | each turn | a fast model's rating, measured pass rate and cost | from measured runs | agent turn | failure signals such as failing tests |
| Triage | before the task | code health metrics | heuristics or a trained classifier | SWE task | a verification gate |
| **Gate** | **before, then after the work** | **category and difficulty, then the turn's evidence** | **online, a fold over its own record** | **agent turn and subtask** | **the turn's tests, Gate's follow-ups, a judge** |

The difference that matters is the right-hand column. A router that decides only before the work has to be right the first time, so it leans towards the expensive model whenever it is unsure. A router that can check the result can afford to be wrong cheaply, as long as the check is good and moving up is cheap. Most of Gate's design, and most of its bugs, are about making that check good.

## 3. Architecture in layers

```chart
gate-architecture
```

Gate has three layers, and the boundaries between them are the most important design decision.

**Decide: `aura-agent-gate`.** This crate holds every decision as a pure function: which models the user's sign-ins allow, which one a turn goes to, at what effort, how the classifier's and judge's requests are built and their replies read, which outcome signal a turn ended with, and the table of what each model has proven. It never calls a model and never touches the disk. That makes every routing rule unit-testable and replayable over recorded work, and leaves the crate unable to do anything the agent's own permissions do not allow.

**Run: the `aura-app` event loop.** The terminal, the desktop app, `aura -p` and benchmark runs share one event loop that drives Gate. It sends the classifier's, the criteria writer's and the judge's requests through the agent's kernel like any other model call, runs checks as `run_command` proposals under the same command policy as the agent's own, and puts files back when a subtask has to be retried.

**Record: the kernel and its record.** Every prompt is a transaction that carries its route, and every way a prompt can end is an `Outcome` transaction that refers back to it. Gate's table is rebuilt from these transactions at startup and updated as each new one is committed, so the next decision always sees the last outcome.

## 4. Walking through a turn

The examples in this section are real. They come from Compare #37, a benchmark run of SWE-Bench Pro tasks with each task run once on Gate and once on Opus 5.5 alone, in isolated copies of each repository. It covers the 20 tasks that had finished on both arms when this was written. The comparison was still working through more.

### 4.1 Candidates and tiers

Gate routes only among models the user can call right now: installed local models, models on Aura's router when signed in, and Claude or ChatGPT models when those plans are linked. A model reachable two ways is kept once, on the user's plan, since the plan is already paid for. Each model has a tier: `local`, `fast` (Haiku, GPT mini), `standard` (Sonnet) or `frontier` (Opus, GPT-5). Tiers matter more than names. They set where work starts and what "a stronger model" means when work moves up.

### 4.2 Classify

One short call to the cheapest fast model labels the prompt with one of eight categories (`question`, `small_change`, `multi_file_change`, `debugging`, `research`, `planning`, `writing`, `other`) and a difficulty (`easy`, `medium` or `hard`). The classifier answers by calling a `label` tool rather than in prose. Long SWE-Bench prompts start "Your task is to implement…", and a cheap model given that as plain text tends to start doing the work instead of labelling it. A call that fails, or takes more than 20 seconds, leaves the prompt `unclassified`.

In Compare #37 this was Haiku 5.5: 20 calls of 1.4 to 3.9 seconds, $0.008 in all. It labelled 15 tasks `multi_file_change`, 4 `small_change` and 1 `debugging`, and all 20 `hard`.

Unlike the learned routers in section 2, the classifier is not the router. It only decides which row of the table to read.

### 4.3 Pick a model and an effort

The table has a row per level (turn or subtask), category and model, holding how many tries the model has had, how many succeeded and what they cost. Gate picks the cheapest model with at least 10 counted outcomes in the category and a success rate of at least 85% (`AURA_GATE_MIN_TRIES`, `AURA_GATE_TARGET`). Without one, it uses the category's start model: the cheapest fast-tier model for work the classifier found `easy`, and the cheapest standard-tier model for everything else, including all planning. Work that is not easy never starts below the standard tier.

Code work (`small_change`, `multi_file_change`, `debugging`) ends in a test or build that passes or fails, so Gate prices it as a cascade. The expected cost of starting on a model is its attempt, plus, if the attempt fails, the judge and the next tier's attempt, weighted by Gate's estimate of the model's chance of failing that kind of work at that difficulty. A frontier model starts the work only when that sum says it is cheaper. Otherwise it is reached by moving up.

Two refinements keep this honest. Candidates are ordered by expected cost, not list price, because a model that is cheaper per token can need several times the tokens. An untried model is priced from its list price, scaled by how the tried models of its tier compare with theirs. And a model is ruled out early, as soon as it can no longer reach the target over its first 10 tries (`Row::can_still_reach`): two failures in its first two tries is enough.

A table that only ever picks its current best never finds out whether something cheaper would do. Gate's answer is exploration. With probability `AURA_GATE_EXPLORE` (5%), a routed turn tries the nearest cheaper model the table has not ruled out (else the nearest one, never below the standard tier for work that is not easy), and records that it explored and how likely the choice was, so the outcome can be weighed fairly afterwards. This is the logged-propensity idea from the bandit literature.[22](https://arxiv.org/abs/1003.0146) Comparisons switch exploration off unless asked, so a benchmark measures the policy rather than its experiments.

Effort comes from difficulty: `low` for easy work, `medium` for medium or unrated work, `high` for hard, and never `max` to start with. On the fast tier, code work runs at `medium` or above, since at `low` a model may skip thinking on steps it finds simple, and other work runs with thinking off. The route recorded on one #37 prompt reads:

```json
{"category": "debugging", "difficulty": "hard", "model": "claude/claude-sonnet-5-5", "effort": "high", "explored": false}
```

### 4.4 Write down what done means

Before a code turn starts, Gate has a standard-tier model turn the request into acceptance criteria, each marked as a behaviour, an interface or an exact output. On an Element task in #37 (bulk sign-out of devices), Sonnet wrote 12, among them:

> The bulk sign-out action has `data-testid="sign-out-selection-cta"`.
>
> When the active device filter changes, all selected devices are cleared.

The worker is shown the criteria after the prompt, and reviews check the work against them one by one. That review is still logged only (`AURA_GATE_CRITERIA=log`), for reasons given in section 4.6. Across #37 the criteria cost $0.41, 5% of Gate's spend.

### 4.5 Do the work, alone or as a lead

The turn runs in the ordinary agent loop on the chosen model. Work that can be split (multi-file changes, planning, debugging, research) may instead be led. The lead model keeps its reading and checking tools, loses its file-changing ones, gets capped reads, and hands edits to subtasks that Gate routes on their own, below the frontier tier when the lead is on it and a standard model exists. Whether to lead is learned the same way the model is. By default the model does the work itself. Leading takes over once the record shows it is cheaper for that kind of work, or once the model doing the work itself is ruled out. On a frontier model it takes over only once proven cheaper. No turn in #37 was led.

### 4.6 Verify: read the evidence

This is where a router that checks results earns its savings, and where most of Gate's bugs have been. A turn of code work ends in one of four signals: an error, `checks_failed`, `checks_passed`, or nothing to go on. Reading which one is harder than it sounds.

**Which command was the check.** Gate takes the turn's last test or build command, including wrapped ones such as `sh .swe/run 'go test ./internal/config/...'`. When the exit status may belong to another command (`pytest | grep Error` that matched nothing), the output decides: `12 passed` passes it, failures fail it, and output showing neither says nothing. A test run made with the change stashed away tested the old code, so it does not count either.

**Work that checked nothing.** While a stronger model is there to move up to, a code turn that ran no test or build is followed up one step at a time until something says how it went. First it is asked, once, to run the tests that cover its change and then give its whole answer again. If it still has not, the harness runs the project's own test command. If nothing could run, a judge reads the request, the diff, the commands and the final reply, and says whether the work is done.

**A pass that proves less than it seems.** Some passing checks prove little. A reply that says the work was not verified ("I couldn't build against the real ECR module here because it isn't in the offline container", from a flipt task in #37) sends the turn to the judge despite its passing tests. So do tests that ran only files the attempt wrote itself. Each such sign is logged with what later proved the attempt right or wrong, and a sign triggers a review only once the log shows it means something.

**Review before work stands.** Gate can also review work that is about to stand, against the request and its criteria. Replayed over logged benchmark attempts, an Opus reviewer given the criteria called 6 of 8 wrong attempts unfinished, but also 16 of 35 right ones. Acting on its verdicts would send back between two and three good attempts for every bad one, so reviews stay logged only, on a quarter of turns. The #37 review of the Element task found 11 of its 12 criteria met and one untested. The task passed its hidden tests.

### 4.7 Judge: whose fault was it?

A failed turn is not yet a reason to pay for a stronger model. Tests fail because of the environment, because they were failing before the change, or because the request cannot be done. So Gate asks a judge one narrow question. The question is not "is this correct?" but "was the model at fault?". The verdict is either `judged_fail`, which counts against the model and moves the work up, or `judged_excused`, which counts for nothing and leaves the work where it is.

The judge of a failed check is the strongest Opus available, since that verdict decides whether to pay for Opus. Work that checked nothing is judged on the model that did it when that is a standard model or stronger, because Opus costs twice what Sonnet does per token,[27](https://platform.claude.com/docs/en/about-claude/pricing) and judging Sonnet's work on Opus had come to cost more than the work. The judge of a failed check is never asked about work on its own tier, and is paid for only when a check failed. Reviews and the judge of unchecked work also run on some work that passed, which is where most of the judging spend goes.

In #37 six Sonnet turns ended with their own tests failing. The Opus judge excused all six, so none moved up, and all six passed the task's hidden tests. One more turn's tests passed, but Gate did not take them as proof that the change worked and sent the turn to the judge of unchecked work. Sonnet, judging its own tier's work, found:

> An empty string, a bare unit such as `MB`, or a lone `.` still passes the new regex, because the number part can be empty. Those inputs then fail at `float()` with `can't interpret following number`, not the required `can't interpret following string`.

That was a real bug, and the work moved up. Gate's judging and reviewing across the 20 tasks cost $0.82 over 16 calls.

### 4.8 Escalate

Work the judge blames goes on in the same conversation, with the files as the failed attempt left them, on the cheapest model of the next tier up. Its first message carries the judge's findings, as in Reflexion,[24](https://arxiv.org/abs/2303.11366) and it is told that it is a stronger model taking over. The task above moved from Sonnet to Opus at high effort, fixed the regex, and passed. It cost $0.35 on Gate against $0.30 on Opus alone, the only one of the 20 where Gate cost more.

Effort is raised only on the strongest model, and only after a judge has found its work unfinished there. Each failed attempt that moves up costs a full pass on a dearer model, so moving up has to be rare and deliberate. A subtask gets one retry. Before it runs, the files are put back from the attempt's checkpoint, and from a git snapshot when the subtask had the workspace to itself, so the retry starts from where the subtask started, not from where it failed.

### 4.9 Learn

Learning needs no training run. The table is a fold over the record. Each prompt counts once, with its latest outcome that says how the work went. A later outcome that does, such as the judge's verdict, takes the earlier one's place rather than adding to it, and an excused failure takes the prompt out of the table. Work that moved up is not counted for the stronger model at all, because that model only ever sees work that has already failed once, and the first attempt's outcome already says how the route did. Outcomes may arrive in any order and more than once, so the startup scan and the live feed can overlap, and which outcome counts depends only on record sequence numbers. Replaying the same record always gives the same table.

This is also where the "cheapest feasible router fails" result[20](https://arxiv.org/abs/2611.20417) bites. A model proven on 10 tries can still let you down on the 11th. Gate's defence is that a proven model stays under the same checks: every turn it does is verified and judged, and its row keeps moving.

## 5. Evaluation

### 5.1 Method

SWE-Bench Pro is a benchmark of long-horizon changes to large production codebases.[25](https://arxiv.org/abs/2509.16941) We use the 51-task HARD-51 subset of its V2 release.[26](https://github.com/scaleapi/SWE-bench_Pro-os) Each task runs in its own container image. The agent works on a copy of the repository, runs commands in a fresh container without network, and is graded as SWE-Bench Pro grades: its changes are applied as one patch in a fresh container, and the task's hidden tests must all pass, including every test that passed before.

A comparison (`/compare` in Aura) runs each task once per arm, in its own git worktree, without anyone approving calls. The Gate arm routes as a user's turns would, with exploration off, and is followed up, judged and moved up as usual. Neither arm sees the hidden tests. The Opus arm runs every turn and subtask on Opus 5.5. Costs are list prices[27](https://platform.claude.com/docs/en/about-claude/pricing) applied to each call's recorded token usage, including cache reads and writes. Gate's cost includes its classifier, criteria, judge and review calls. The figures below are generated from the run logs by a script in the Aura repository, which also checks that the per-call prices add up to each run's recorded cost.

### 5.2 Results

```chart
gate-savings
```

Both arms passed all 20 tasks. Gate cost 48% less and took 13% less wall time. It was cheaper on 19 of the 20 tasks, at between 0.26× and 0.88× of Opus's cost, and faster on 15.

```chart
gate-swe-tasks
```

Every task started on Sonnet 5.5 at high effort, because the classifier rated every one of them hard and hard work starts on the standard tier. Thirteen ended with their own tests passing, six with failing tests the judge excused, and one moved up to Opus. In #37, then, Gate's saving is close to the per-token price gap between Sonnet and Opus, less what checking cost. What the router adds is knowing when Sonnet's work can be left to stand. The cost of finding that out is visible:

```chart
gate-spend
```

Gate's own calls were 15% of its spend, about two thirds of it on judging and reviewing. That overhead is the price of a router that checks results. It is also the obvious place to save next, since most of those verdicts left the work where it was.

### 5.3 How the design got here

The design above is the result of seven comparisons on these tasks. Four of the SWE-Bench Pro tasks ran in every comparison from #29 to #37, so the cost ratio on them tracks what each change bought:

```chart
gate-iterations
```

Each regression had the same shape: a check that read a signal wrong, followed by an expensive move up.

- **#29 and #32: Haiku passed its own tests.** Medium-difficulty work started on the fast tier, and on an Ansible task about multipart forms Haiku wrote tests for its own code, passed them, and failed the hidden ones. Fast models' own tests pass on work they got wrong, so Gate never saw a failure to move up from. Work that is not easy now starts on the standard tier.
- **#30 and #31: correct work retried at max effort.** A reply saying only that a lint step could not run counted as unverified. A piped test command whose exit status came from `grep` counted as a failure. On Opus, both sent correct work round again at maximum effort, and in #31 two such retries cost at least $4.55. Checks are now decided by what they print, and the strongest model is pushed harder only after a judge finds its work unfinished.
- **#33: the table overreacted.** Sonnet had fallen below the success target on hard work, so the table started two of the five tasks on Opus. On one of them, a first pass on Opus and a second at max effort cost $2.43 together, on a task Opus alone finished for $0.42. A second pass on the strongest model now needs a judge's finding. A later change, made after #37 and not yet measured, keeps work on a standard model until starting on it has also become dearer than starting on the next tier.
- **Between #33 and #36: the checking cost more than the work.** On a separate suite (Compare #34, on Aura's own `gate_hard` tasks), Gate's checking of Sonnet's work on Opus cost more than the work itself and changed no outcome. Unchecked work below the strongest tier is now judged on its own model, and logged reviews are sampled.

The first four runs in the chart predate the logging of Gate's own calls, so their Gate cost leaves out the classifier and judge. They understate Gate's cost, and the regressions were worse than shown.

## 6. Threats to validity

- **Small samples.** #37 is 20 tasks so far, each run once per arm, and the iteration chart rests on 4. The Opus arm's cost on those same 4 tasks ranged from $2.00 to $3.09 between comparisons. Single runs are noisy, and a 48% saving on 20 tasks is a result, not a constant.
- **A ceiling on pass rate.** Both arms passed every finished task in #37, so this run cannot show Gate keeping quality on work Opus can do and Sonnet cannot. It can only show that it did not lose any. The earlier comparisons, in which Gate failed one task that Opus passed, are the evidence of that risk.
- **One provider and one plan.** Every model was a Claude model on one plan, so the tiers were Haiku, Sonnet and Opus. With other providers, the price gaps and the failure modes of the cheap tier differ.
- **List prices, not plan usage.** On a subscription plan, what matters is the share of the plan's usage window a run consumes. Aura's comparison view reports that too. This post uses list prices because readers can check them.
- **A moving target.** Gate changed between every pair of comparisons, and the post describes the router as of Nov 22, 2026. #37 ran on the build of commit `d07d3e3a`. A change committed after it, to when a standard model keeps its work, has not yet been measured on these tasks.
- **Hidden tests as the only grade.** Passing the hidden tests is necessary but not the whole of quality. Aura's comparisons also run a blind judge over the two arms' diffs and replies, and that verdict for #37 is not reported here.

Public router leaderboards are beginning to report cost per task next to quality,[28](https://openrouter.ai/benchmarks/routers) and step-level benchmarks such as TwinRouterBench[18](https://arxiv.org/abs/2605.18859) would let Gate be compared with other routers on the same tasks. That is the next evaluation to run.

## 7. Limits and next steps

- **The classifier's mistakes cost twice.** A hard task labelled easy starts on the fast tier. Since fast models' own tests pass on work they got wrong, the mistake may never surface as a failure. Starting every not-easy task on the standard tier limits the damage, at the price of rarely using the fast tier for code.
- **Turns that end with nothing to go on teach nothing.** They count for their cost only.
- **Thresholds are global.** The 85% target and 10 tries apply to every category, and nothing decays, so a model that improves or regresses is noticed only as fast as new outcomes outweigh old ones.
- **The judge is a model too.** Its excusals in #37 were right six times out of six, but every verdict is worth checking against what later proved the work right or wrong. That is what Gate's attempt log and its `aura gate eval` report are for.

The direction is clear from the data. Gate saves money by letting a cheaper model's work stand, and it loses money when it misreads the evidence and pays for a stronger model to redo work that was already right. The work ahead is making the evidence cheaper to read and harder to misread.

## Appendix: settings

| Setting | Default | What it does |
|---|---|---|
| `AURA_GATE_TARGET` | `0.85` | Success rate a model needs to be proven on a kind of work |
| `AURA_GATE_MIN_TRIES` | `10` | Counted outcomes before a model can be proven or ruled out |
| `AURA_GATE_EXPLORE` | `0.05` | Share of routed turns that try a cheaper model to keep learning |
| `AURA_GATE_DELEGATE` | `on` | Makes a led turn delegate: no file tools, only check and inspection commands. Off, the lead keeps its tools and is only asked to delegate |
| `AURA_GATE_JUDGE` | `on` | Asks the judge about failed and unchecked work |
| `AURA_GATE_NUDGE` | `on` | Asks a turn that checked nothing to check its change |
| `AURA_GATE_CHECK` | `on` | Runs the project's tests on a change nothing checked |
| `AURA_GATE_CRITERIA` | `log` | Writes acceptance criteria and shows them to the worker; `on` also acts on reviews against them |
| `AURA_GATE_REVIEW` | `log` | Reviews work before it stands; `log` records the verdict, `on` acts on it |
| `AURA_GATE_REVIEW_SAMPLE` | `0.25` | Share of turns that get a logged review |

## Sources

1. OpenAI, [Introducing GPT-5](https://openai.com/index/introducing-gpt-5/).
2. OpenRouter, [Auto Code](https://openrouter.ai/docs/guides/routing/routers/auto-code.md), documentation.
3. Not Diamond, [Model Routing for Coding Agents](https://www.notdiamond.ai/).
4. Chen, Zaharia and Zou, [FrugalGPT: How to Use Large Language Models While Reducing Cost and Improving Performance](https://arxiv.org/abs/2305.05176), arXiv, May 2023.
5. Yue et al., [Large Language Model Cascades with Mixture of Thoughts Representations for Cost-efficient Reasoning](https://arxiv.org/abs/2310.03094), ICLR 2024.
6. Gupta et al., [Language Model Cascades: Token-level uncertainty and beyond](https://arxiv.org/abs/2404.10136), arXiv, Apr 2024.
7. Aggarwal et al., [AutoMix: Automatically Mixing Language Models](https://arxiv.org/abs/2310.12963), arXiv, Oct 2023.
8. Ding et al., [Hybrid LLM: Cost-Efficient and Quality-Aware Query Routing](https://arxiv.org/abs/2404.14618), arXiv, Apr 2024.
9. Ong et al., [RouteLLM: Learning to Route LLMs with Preference Data](https://arxiv.org/abs/2406.18665), arXiv, Jun 2024.
10. LMSYS, [RouteLLM: An Open-Source Framework for Cost-Effective LLM Routing](https://www.lmsys.org/blog/2024-07-01-routellm/), Jul 1, 2024.
11. Hu et al., [RouterBench: A Benchmark for Multi-LLM Routing System](https://arxiv.org/abs/2403.12031), arXiv, Mar 2024.
12. Varangot-Reille et al., [Doing More with Less: A Survey on Routing Strategies for Resource Optimisation in Large Language Model-Based Systems](https://arxiv.org/abs/2502.00409), arXiv, Feb 2025.
13. Zhang, Feng and You, [Router-R1: Teaching LLMs Multi-Round Routing and Aggregation via Reinforcement Learning](https://arxiv.org/abs/2506.09033), arXiv, Jun 2025.
14. Tran et al., [Arch-Router: Aligning LLM Routing with Human Preferences](https://arxiv.org/abs/2506.16655), arXiv, Jun 2025.
15. Anthropic, [Building effective agents](https://www.anthropic.com/research/building-effective-agents).
16. Yue et al., [MasRouter: Learning to Route LLMs for Multi-Agent Systems](https://arxiv.org/abs/2502.11133), arXiv, Feb 2025.
17. Li, Abdelmoniem and Wang, [ProgRouter: Online Progress-Guided Orchestration for Multi-Agent LLM Workflows under Quality-Cost Tradeoffs](https://arxiv.org/abs/2608.25992), arXiv, Aug 2026.
18. Yang et al., [TwinRouterBench: Fast Static and Live Dynamic Evaluation for Realistic Agentic LLM Routing](https://arxiv.org/abs/2605.18859), arXiv, May 2026.
19. Madeyski, [Triage: Routing Software Engineering Tasks to Cost-Effective LLM Tiers via Code Quality Signals](https://arxiv.org/abs/2604.07494), arXiv, Apr 2026.
20. Hu et al., [When the Cheapest Feasible Router Fails: Auditing Deployment Commitments in Cost-Aware LLM Routing](https://arxiv.org/abs/2611.20417), arXiv, Nov 2026.
21. Belloni, Chen and Wei, [Online Pandora's Box for Contextual LLM Cascading](https://arxiv.org/abs/2606.07392), arXiv, Jun 2026.
22. Li, Chu, Langford and Schapire, [A Contextual-Bandit Approach to Personalized News Article Recommendation](https://arxiv.org/abs/1003.0146), WWW 2010.
23. Zheng et al., [Judging LLM-as-a-Judge with MT-Bench and Chatbot Arena](https://arxiv.org/abs/2306.05685), arXiv, Jun 2023.
24. Shinn et al., [Reflexion: Language Agents with Verbal Reinforcement Learning](https://arxiv.org/abs/2303.11366), arXiv, Mar 2023.
25. Deng et al., [SWE-Bench Pro: Can AI Agents Solve Long-Horizon Software Engineering Tasks?](https://arxiv.org/abs/2509.16941), arXiv, Sep 2025.
26. Scale AI, [SWE-bench_Pro-os](https://github.com/scaleapi/SWE-bench_Pro-os), GitHub.
27. Anthropic, [Pricing](https://platform.claude.com/docs/en/about-claude/pricing), Claude documentation.
28. OpenRouter, [Model routing benchmarks](https://openrouter.ai/benchmarks/routers).
