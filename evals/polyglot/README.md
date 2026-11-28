# Polyglot suite

Twenty-four Exercism exercises from the [Aider polyglot
benchmark](https://github.com/Aider-AI/polyglot-benchmark), which keeps
the practice exercises most models failed: eight each in Python, Rust and
Go, the ones with the longest reference solutions (a bowling scorer, a
Forth evaluator, a reactive cell system, alphametics, a doubly linked
list with unsafe code, and so on). Each is a whole program to write from
a stub and a specification, against a hidden test suite, so a model that
is only careful with small fixes has no shortcut.

The layout is that of `evals/gate_cost`: `workspace/` holds the stub,
`checks/.gate_checks/` the exercise's tests (copied in only for the
check), `solution/` Exercism's example solution, and `task.toml` the
prompt (the exercise's instructions) and the check command. The Rust
checks run the tests Exercism marks `#[ignore]` too. `tests/it/eval_suite.rs`
proves every task fails as shipped and passes when solved.

## Importing more

`scripts/import_polyglot.py` downloads the benchmark and writes tasks
here. It proves each one before keeping it, so exercises whose stub
already passes, that need crates, or that split their solution over
steps are skipped and the next one is taken.

```sh
scripts/import_polyglot.py --list                      # exercises by size
scripts/import_polyglot.py --per-lang 12               # the 12 largest of each language
scripts/import_polyglot.py --lang rust --per-lang 20
scripts/import_polyglot.py --exercise python/zipper --exercise go/ledger
```

Python, Rust and Go need only their toolchains; the benchmark's C++,
Java and JavaScript exercises need CMake, Gradle or npm packages and are
not imported.

## Running it

In the BENCH tab choose "example tasks: polyglot", or in chat
`/compare --suite polyglot --examples 6 --repeats 3`. From the shell:

```sh
aura eval run evals/polyglot --model claude/claude-opus-5-5 --arm opus --repeat 3 --budget-usd 15
aura eval run evals/polyglot --model gate --arm gate --repeat 3 --budget-usd 15
aura eval compare --suite polyglot --arm opus --arm gate
```

## Attribution

The exercises are copyright © [Exercism](https://exercism.org), from its
[Python](https://github.com/exercism/python),
[Rust](https://github.com/exercism/rust) and
[Go](https://github.com/exercism/go) tracks, used under the MIT License,
as collected by Aider.
