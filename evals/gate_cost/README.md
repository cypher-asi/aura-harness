# Gate cost suite

Twenty small tasks for measuring what Gate saves and what it loses.
Each task has a `workspace/` fixture, hidden `checks/` copied in only
after the run (the agent never sees them), a `category`, and a reference
`solution/` that `tests/eval_suite.rs` uses to prove every task fails as
shipped and passes when solved.

Categories: `bugfix`, `feature`, `refactor`, `tests`, `docs`,
`investigation`, plus the protected ones Gate must not get worse at:
`reasoning`, `unfamiliar`, `long-session`, `ambiguous`, `recovery`.

## Running the comparison

Each arm is one model on the same tasks, except `--model gate`, which
routes each task's prompts and subtasks with Gate's table, lets the
agent lead and delegate on multi-file changes and planning, and records
the verify verdicts in that table
(see [Gate](../../docs/cli.md#gate)). `--budget-usd` stops an arm once
its runs have cost that much.

Under `--model gate`, a turn that fails in the run (a model error, or
its last test or build command failing) is reviewed by Gate's judge,
which never sees the hidden checks. The task's line then shows what the
judge found and what asking it cost, and the results table ends with the
judge's share of the arm's cost: what the arm paid to learn from
failures rather than to do the work.

```sh
aura eval run evals/gate_cost --model claude/claude-opus-5-5 --arm opus --repeat 3 --budget-usd 20
aura eval run evals/gate_cost --model claude/claude-haiku-5-5 --arm haiku --repeat 3 --budget-usd 5
aura eval run evals/gate_cost --model gate --arm gate --repeat 3 --budget-usd 20

aura eval compare --suite gate_cost --arm opus --arm gate
```

With 20 tasks and 3 repeats a comparison can show a large cost drop and
catch a large quality loss, not small ones.

## Judging quality blind

The hidden checks are pass/fail. To compare what they miss (scope, code
quality, accuracy of the reply), `judge.py` shows a judge model each
task's two attempts, the diff and the final reply, unlabeled and in
random order, and asks which is better. Judge calls go through
`aura -p`.

```sh
python3 evals/gate_cost/judge.py --reference opus --arm gate
```

Judgments go to `AURA_HOME/evals/judgments/<reference>-vs-<arm>.jsonl`
with the judge's reasons; the summary gives wins, losses, ties and a
sign test, overall and per category. The judge is a frontier model, so
it may favour the reference's style; read the reasons, not just the
count.
