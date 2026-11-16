# Gate hard suite

Eight tasks with a trap a careless fix falls into: SemVer prerelease
precedence, half-up money rounding, daylight-saving gaps and overlaps,
shell file names with spaces and newlines, a cache key that drops
arguments, a retry spec with a deadline, display width of wide and
combining characters, and cursor pagination with tied timestamps.
`evals/gate_cost` has a ceiling (a fast model alone passes all of it), so
this suite is where a cheap first attempt is most likely to be wrong.

The layout is the same as `evals/gate_cost`: `workspace/`, hidden
`checks/`, a reference `solution/`, and `tests/eval_suite.rs` proves each
task fails as shipped and passes when solved.

```sh
aura eval run evals/gate_hard --model claude/claude-haiku-5-5 --arm haiku --repeat 3 --budget-usd 2
aura eval run evals/gate_hard --model claude/claude-opus-5-5 --arm opus --repeat 3 --budget-usd 10

aura eval compare --suite gate_hard --arm opus --arm haiku
python3 evals/gate_cost/judge.py --suite evals/gate_hard --reference opus --arm haiku
```
