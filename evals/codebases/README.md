# Codebases: real fixes in open-source repositories

21 tasks, each a merged pull request that closed a GitHub issue, built the
way SWE-bench builds its tasks. The agent works in the whole repository at
the commit before the fix, with the issue as its prompt; the check puts the
fix's own tests in place, runs them, and puts the workspace's files back.
They are much harder than the other suites: the agent has to find its way
around a codebase of thousands of lines it has never seen, and the hidden
tests check the fix against cases the issue only hints at.

Each task folder holds `task.toml` (the prompt, and the repository and
commit as `[source]`), `checks/.gate_checks/` (the fix's tests and
`run.sh`) and `solution/` (the files the fix changed). The repository
itself is not stored here: aura downloads it once per commit from GitHub
into `AURA_HOME/evals/sources` when a task first runs. The repositories'
tests need only Python 3 or Go, so checks run offline.

## Running them

In the terminal, `/compare --suite codebases --examples 6 --repeats 3`, or
choose "example coding tasks: codebases" in the BENCH tab's form. From the
shell:

```
aura eval run evals/codebases --model gate --arm gate --budget-usd 10
aura eval run evals/codebases --model claude/claude-opus-5-5 --arm opus --budget-usd 20
aura eval compare --suite codebases --arm opus --arm gate
```

## Adding more

`scripts/import_repo_tasks.py` mines a repository's merged pull requests
(it needs `gh` signed in) and keeps a fix only when its check fails on the
commit before the fix and passes with the fix:

```
scripts/import_repo_tasks.py --repo tobymao/sqlglot --list
scripts/import_repo_tasks.py --repo pelletier/go-toml --per-repo 5
scripts/import_repo_tasks.py --pr tobymao/sqlglot#9243
cargo test --test it eval_suite::codebase -- --include-ignored
```

## Tasks

| Task | Language | Issue | Links |
| --- | --- | --- | --- |
| `expr-823` | Go | WithContext doesn't work with 2 functions within 1 call | [issue](https://github.com/expr-lang/expr/issues/823) · [fix](https://github.com/expr-lang/expr/pull/883) |
| `go-toml-1075` | Go | The serialization regression with 2.4.0 | [issue](https://github.com/pelletier/go-toml/issues/1075) · [fix](https://github.com/pelletier/go-toml/pull/1076) |
| `go-toml-1109` | Go | Regression: encoding.TextUnmarshaler no longer invoked for named types whose underlying Kind matches a native TOML scalar (broken since v2.4.0) | [issue](https://github.com/pelletier/go-toml/issues/1109) · [fix](https://github.com/pelletier/go-toml/pull/1113) |
| `go-toml-931` | Go | Slices of structs are not properly reset when unmarshalling | [issue](https://github.com/pelletier/go-toml/issues/931) · [fix](https://github.com/pelletier/go-toml/pull/934) |
| `gomarkdown-279` | Go | Parsing error when parsing nested bold+ital | [issue](https://github.com/gomarkdown/markdown/issues/279) · [fix](https://github.com/gomarkdown/markdown/pull/355) |
| `gomarkdown-346` | Go | When a fenced code block is on a list, a line starting from dash breaks formatting. | [issue](https://github.com/gomarkdown/markdown/issues/346) · [fix](https://github.com/gomarkdown/markdown/pull/354) |
| `jsonparser-177` | Go | Invalid arguments passed to EachKey can cause a panic | [issue](https://github.com/buger/jsonparser/issues/177) · [fix](https://github.com/buger/jsonparser/pull/190) |
| `python-markdown-1586` | Python | Markdown.markdown stuck in infinite loop, leading to memory leaks | [issue](https://github.com/Python-Markdown/markdown/issues/1586) · [fix](https://github.com/Python-Markdown/markdown/pull/1587) |
| `python-markdown-1694` | Python | Text lost after an unclosed *** (regression in 3.11.1) | [issue](https://github.com/Python-Markdown/markdown/issues/1694) · [fix](https://github.com/Python-Markdown/markdown/pull/1696) |
| `semver-81` | Go | Pre-release versions handling not consistent | [issue](https://github.com/Masterminds/semver/issues/81) · [fix](https://github.com/Masterminds/semver/pull/108) |
| `sqlglot-8776` | Python | postgres: TABLESPACE in CREATE TABLE / INDEX / MATERIALIZED VIEW and USING INDEX TABLESPACE after UNIQUE / PRIMARY KEY fall back to Command | [issue](https://github.com/tobymao/sqlglot/issues/8776) · [fix](https://github.com/tobymao/sqlglot/pull/8792) |
| `sqlglot-8820` | Python | Snowflake to DuckDB: TO_CHAR date format elements in mixed case aren't converted, and lower-case 'dy' and 'dd' are converted wrongly | [issue](https://github.com/tobymao/sqlglot/issues/8820) · [fix](https://github.com/tobymao/sqlglot/pull/8832) |
| `sqlglot-8841` | Python | PostgreSQL to SQLite transpilation generates invalid SQL for parenthesized set operation with ORDER BY | [issue](https://github.com/tobymao/sqlglot/issues/8841) · [fix](https://github.com/tobymao/sqlglot/pull/8843) |
| `sqlglot-8891` | Python | TABLE t shorthand doesn't parse (duckdb, postgres, mysql) | [issue](https://github.com/tobymao/sqlglot/issues/8891) · [fix](https://github.com/tobymao/sqlglot/pull/8897) |
| `sqlglot-9060` | Python | Oracle: VARIANCE and VAR_SAMP parse to the same expression but differ for a single value | [issue](https://github.com/tobymao/sqlglot/issues/9060) · [fix](https://github.com/tobymao/sqlglot/pull/9075) |
| `sqlglot-9065` | Python | Window functions without ORDER BY transpile to SQL that Snowflake, Databricks and BigQuery reject | [issue](https://github.com/tobymao/sqlglot/issues/9065) · [fix](https://github.com/tobymao/sqlglot/pull/9066) |
| `sqlglot-9083` | Python | Snowflake to Spark: DATEADD/TIMESTAMPADD with HOUR, MINUTE or SECOND becomes day-based DATE_ADD | [issue](https://github.com/tobymao/sqlglot/issues/9083) · [fix](https://github.com/tobymao/sqlglot/pull/9085) |
| `sqlglot-9112` | Python | DuckDB: printf-style FORMAT from BigQuery/Spark/SQLite/Postgres is written as DuckDB FORMAT, which uses {} placeholders | [issue](https://github.com/tobymao/sqlglot/issues/9112) · [fix](https://github.com/tobymao/sqlglot/pull/9176) |
| `sqlglot-9235` | Python | SQLite: date arithmetic with + / - INTERVAL produces invalid SQL | [issue](https://github.com/tobymao/sqlglot/issues/9235) · [fix](https://github.com/tobymao/sqlglot/pull/9240) |
| `sqlglot-9242` | Python | (tokenizer)Snowflake: comment markers inside unquoted stage paths are tokenized as comments | [issue](https://github.com/tobymao/sqlglot/issues/9242) · [fix](https://github.com/tobymao/sqlglot/pull/9243) |
| `sqlglot-9280` | Python | SQLite: END and END TRANSACTION are parsed as a column and an alias instead of COMMIT | [issue](https://github.com/tobymao/sqlglot/issues/9280) · [fix](https://github.com/tobymao/sqlglot/pull/9297) |

The repositories' code keeps its own license (see each repository's
`LICENSE`); the tests and changed files kept here are from the linked fixes.
