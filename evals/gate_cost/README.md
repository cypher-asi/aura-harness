# Gate cost suite

Twenty small tasks for measuring what the cost gate saves and what it
loses. Each task has a `workspace/` fixture, hidden `checks/` copied in
only after the run (the agent never sees them), a `category`, and a
reference `solution/` that `tests/eval_suite.rs` uses to prove every task
fails as shipped and passes when solved.

Categories: `bugfix`, `feature`, `refactor`, `tests`, `docs`,
`investigation`, plus the protected ones the gate must not get worse at:
`reasoning`, `unfamiliar`, `long-session`, `ambiguous`, `recovery`.

## Running the comparison

Each arm is one model on the same tasks. `--budget-usd` stops an arm
once its runs have cost that much.

```sh
aura eval run evals/gate_cost --model claude/claude-opus-5-5 --arm opus --repeat 3 --budget-usd 20
aura eval run evals/gate_cost --model claude/claude-haiku-5-5 --arm haiku --repeat 3 --budget-usd 5
aura eval run evals/gate_cost --model gate/claude/claude-opus-5-5 --arm gate --repeat 3 --budget-usd 20

aura gate bench evals/gate_cost --reference opus --cheap haiku --arm gate
```

`aura gate bench` pairs arms by task and prints pass rate, cost per run
(with the share spent by gate workers), the cost-matched mixture of the
reference and cheap arms, and the pass-rate difference to the reference
with a 95% interval from resampling tasks, overall and per category.
With 20 tasks and 3 repeats it can show a large cost drop and catch a
large quality loss, not small ones.

## Judging quality blind

The hidden checks are pass/fail. To compare what they miss (scope, code
quality, accuracy of the reply), `judge.py` shows a judge model each
task's two attempts, the diff and the final reply, unlabeled and in
random order, and asks which is better. Judge calls go through
`aura -p`.

```sh
python3 evals/gate_cost/judge.py --reference opus --arm gate
```

Judgments go to `AURA_HOME/gate/judgments/<reference>-vs-<arm>.jsonl`
with the judge's reasons; the summary gives wins, losses, ties and a
sign test, overall and per category. The judge is a frontier model, so
it may favour the reference's style; read the reasons, not just the
count.

## Learning from runs

Every gate attempt is appended to `AURA_HOME/gate/ledger.jsonl`.

```sh
aura gate report                  # active policy and what the ledger shows
aura gate fit                     # writes AURA_HOME/gate/policies/vN.json
AURA_GATE_POLICY=~/.aura/gate/policies/v1.json \
  aura eval run evals/gate_cost --model gate/claude/claude-opus-5-5 --arm gate-v1 --repeat 3
aura gate promote ~/.aura/gate/policies/v1.json --arm gate-v1 --reference opus --cheap haiku
```

`promote` refuses unless every gate attempt of the arm was routed by that
policy, the arm costs less than the reference, its pass-rate interval
stays above `-margin` overall and in every category, and it beats the
cost-matched mixture.
