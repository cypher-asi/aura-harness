#!/usr/bin/env python3
"""Long-horizon evals for `aura -p`.

Each task under `tasks/<id>/` is a small project (`project/`), a feature
request (`task.json`), tests the agent never sees (`hidden_tests/`), and a
reference solution (`reference/`) used only by `--self-check`.

For every task the runner copies the project to a scratch directory, runs
`aura -p <prompt> --output-format json` in it with its own data directory,
then copies the hidden tests in and runs the task's test command. It
writes `report.json` and `report.md` with pass/fail, cost, cache hit rate,
compactions, model calls and wall time per task.

Real models cost money, so a run needs `--live` or `AURA_EVALS_LIVE=1`.
`--provider mock` exercises the pipeline for free (the mock model solves
nothing), and `--self-check` checks the fixtures without any model.

Standard library only; Python 3.8+.
"""

import argparse
import datetime
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

SUITE = Path(__file__).resolve().parent
REPO = SUITE.parents[1]
TASKS = SUITE / "tasks"
# How long `aura -p` may overrun `--max-time` before the runner kills it.
AGENT_GRACE_S = 120
DEFAULT_AGENT_TIMEOUT_S = 2 * 60 * 60
TAIL_LINES = 40


def load_tasks(selected: Optional[List[str]]) -> List[Dict[str, Any]]:
    tasks = []
    for task_dir in sorted(p for p in TASKS.iterdir() if (p / "task.json").is_file()):
        task = json.loads((task_dir / "task.json").read_text(encoding="utf-8"))
        task["dir"] = task_dir
        tasks.append(task)
    if selected:
        known = {t["id"] for t in tasks}
        unknown = sorted(set(selected) - known)
        if unknown:
            sys.exit(f"unknown task(s): {', '.join(unknown)}; known: {', '.join(sorted(known))}")
        tasks = [t for t in tasks if t["id"] in selected]
    return tasks


def overlay(src: Path, dest: Path) -> None:
    """Copy the tree `src` over `dest`, replacing files that exist."""
    for path in src.rglob("*"):
        if path.is_file() and "__pycache__" not in path.parts:
            target = dest / path.relative_to(src)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)


def fresh_project(task: Dict[str, Any], work: Path) -> Path:
    project = work / "project"
    shutil.copytree(
        task["dir"] / "project",
        project,
        ignore=shutil.ignore_patterns("__pycache__", "target"),
    )
    return project


def tail(text: str, lines: int = TAIL_LINES) -> str:
    return "\n".join(text.splitlines()[-lines:])


def project_env(project: Path) -> Dict[str, str]:
    """Environment for commands in `project`. Each scratch project builds
    into its own target directory: a shared `CARGO_TARGET_DIR` would let a
    copy reuse another copy's artifacts, since the crates share a name and
    copied files keep their old modification times."""
    return dict(os.environ, CARGO_TARGET_DIR=str(project.parent / "target"))


def run_tests(task: Dict[str, Any], project: Path) -> Dict[str, Any]:
    timeout = task.get("test_timeout_s", 600)
    started = time.monotonic()
    try:
        proc = subprocess.run(
            task["test_command"],
            cwd=project,
            env=project_env(project),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
        )
        passed, output = proc.returncode == 0, proc.stdout
    except subprocess.TimeoutExpired as e:
        passed = False
        output = (e.stdout or "") if isinstance(e.stdout, str) else ""
        output += f"\n[tests timed out after {timeout}s]"
    return {
        "passed": passed,
        "output_tail": tail(output),
        "duration_s": round(time.monotonic() - started, 2),
    }


def add_hidden_tests(task: Dict[str, Any], project: Path) -> None:
    overlay(task["dir"] / "hidden_tests", project / task["hidden_tests_dest"])


# ---------------------------------------------------------------------------
# --self-check
# ---------------------------------------------------------------------------


def self_check(tasks: List[Dict[str, Any]], scratch: Path) -> int:
    """Each task's visible tests pass as shipped, its hidden tests fail
    before the feature exists, and the reference solution passes them."""
    failures = 0
    for task in tasks:
        checks = []
        work = scratch / f"{task['id']}-bare"
        project = fresh_project(task, work)
        checks.append(("visible tests pass as shipped", run_tests(task, project)["passed"]))
        add_hidden_tests(task, project)
        checks.append(("hidden tests fail before the feature", not run_tests(task, project)["passed"]))

        work = scratch / f"{task['id']}-reference"
        project = fresh_project(task, work)
        overlay(task["dir"] / "reference", project)
        add_hidden_tests(task, project)
        result = run_tests(task, project)
        checks.append(("reference solution passes every test", result["passed"]))

        for name, ok in checks:
            print(f"{'ok  ' if ok else 'FAIL'} {task['id']}: {name}")
            failures += 0 if ok else 1
        if not checks[-1][1]:
            print(result["output_tail"])
    print(f"\n{len(tasks)} task(s) checked, {failures} failure(s)")
    return 1 if failures else 0


# ---------------------------------------------------------------------------
# Eval runs
# ---------------------------------------------------------------------------


def parse_duration_s(raw: str) -> Optional[int]:
    """Seconds in an `aura -p --max-time` value: 90s, 45m, 2h, or minutes."""
    raw = raw.strip().lower()
    units = {"h": 3600, "m": 60, "s": 1}
    unit = units.get(raw[-1:], None)
    number = raw[:-1] if unit else raw
    try:
        value = int(number)
    except ValueError:
        return None
    return value * (unit or 60)


def find_aura(explicit: Optional[str]) -> str:
    if explicit:
        return explicit
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
    for profile in ("release", "debug"):
        candidate = target / profile / "aura"
        if candidate.is_file():
            return str(candidate)
    found = shutil.which("aura")
    if found:
        return found
    sys.exit("no aura binary found; build it with `cargo build --release` or pass --aura")


def aura_command(args: argparse.Namespace, aura: str, task: Dict[str, Any], project: Path) -> List[str]:
    cmd = [aura, "-p", task["prompt"], "--output-format", "json", "--cwd", str(project)]
    if args.provider:
        cmd += ["--provider", args.provider]
    if args.model:
        cmd += ["--model", args.model]
    if args.max_turns:
        cmd += ["--max-turns", str(args.max_turns)]
    if args.max_time:
        cmd += ["--max-time", args.max_time]
    if args.max_cost_usd:
        cmd += ["--max-cost-usd", str(args.max_cost_usd)]
    if args.mode:
        cmd += ["--mode", args.mode]
    return cmd


def parse_result(stdout: str) -> Optional[Dict[str, Any]]:
    for line in reversed(stdout.strip().splitlines()):
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and value.get("type") == "result":
            return value
    return None


def run_task(args: argparse.Namespace, aura: str, task: Dict[str, Any], scratch: Path) -> Dict[str, Any]:
    work = scratch / task["id"]
    project = fresh_project(task, work)
    env = dict(project_env(project), AURA_DATA_DIR=str(work / "aura_data"))
    timeout = DEFAULT_AGENT_TIMEOUT_S
    if args.max_time and parse_duration_s(args.max_time):
        timeout = parse_duration_s(args.max_time) + AGENT_GRACE_S

    print(f"→ {task['id']}: running aura -p", flush=True)
    started = time.monotonic()
    killed = False
    try:
        proc = subprocess.run(
            aura_command(args, aura, task, project),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            timeout=timeout,
        )
        stdout, stderr, code = proc.stdout, proc.stderr, proc.returncode
    except subprocess.TimeoutExpired as e:
        killed = True
        stdout = e.stdout if isinstance(e.stdout, str) else ""
        stderr = e.stderr if isinstance(e.stderr, str) else ""
        code = None
    wall = time.monotonic() - started
    (work / "aura.stderr.log").write_text(stderr or "", encoding="utf-8")
    report = parse_result(stdout or "") or {}

    add_hidden_tests(task, project)
    tests = run_tests(task, project)
    usage = report.get("usage") or {}
    todos = report.get("todos") or []
    record = {
        "task": task["id"],
        "title": task["title"],
        "passed": tests["passed"],
        "exit_reason": report.get("exit_reason") or ("killed" if killed else "no_result"),
        "aura_exit_code": code,
        "num_turns": report.get("num_turns", 0),
        "agent_turns": report.get("agent_turns", 0),
        "cost_usd": report.get("total_cost_usd"),
        "cache_hit_rate": usage.get("cache_hit_rate"),
        "input_tokens": usage.get("input_tokens", 0),
        "cache_read_tokens": usage.get("cache_read_tokens", 0),
        "cache_write_tokens": usage.get("cache_write_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "compactions": report.get("compactions", 0),
        "todos_total": len(todos),
        "todos_open": sum(1 for t in todos if t.get("status") in ("pending", "in_progress")),
        "files_changed": report.get("files_changed", []),
        "wall_time_s": round(wall, 1),
        "session_id": report.get("session_id"),
        "error": report.get("error") or (tail(stderr, 5) if not report else None),
        "test_output_tail": tests["output_tail"],
        "work_dir": str(work) if args.keep else None,
    }
    mark = "PASS" if record["passed"] else "FAIL"
    cost = "n/a" if record["cost_usd"] is None else f"${record['cost_usd']:.2f}"
    print(f"  {mark} {task['id']}: {record['exit_reason']}, {record['num_turns']} calls, {cost}, {record['wall_time_s']}s")
    return record


def summarize(records: List[Dict[str, Any]]) -> Dict[str, Any]:
    prompt_tokens = sum(r["input_tokens"] + r["cache_read_tokens"] + r["cache_write_tokens"] for r in records)
    cache_reads = sum(r["cache_read_tokens"] for r in records)
    priced = [r["cost_usd"] for r in records if r["cost_usd"] is not None]
    passed = sum(1 for r in records if r["passed"])
    return {
        "tasks": len(records),
        "passed": passed,
        "pass_rate": round(passed / len(records), 3) if records else None,
        "total_cost_usd": round(sum(priced), 4) if priced else None,
        "cache_hit_rate": round(cache_reads / prompt_tokens, 3) if prompt_tokens else None,
        "compactions": sum(r["compactions"] for r in records),
        "model_calls": sum(r["num_turns"] for r in records),
        "wall_time_s": round(sum(r["wall_time_s"] for r in records), 1),
    }


def fmt(value: Any, kind: str = "") -> str:
    if value is None:
        return "–"
    if kind == "usd":
        return f"${value:.2f}"
    if kind == "pct":
        return f"{value * 100:.0f}%"
    return str(value)


def markdown(meta: Dict[str, Any], summary: Dict[str, Any], records: List[Dict[str, Any]]) -> str:
    lines = [
        "# Long-horizon eval report",
        "",
        f"- Started: {meta['started']}",
        f"- aura: `{meta['aura']}`, provider `{meta['provider'] or 'anthropic'}`, model `{meta['model'] or 'default'}`",
        f"- Limits: max turns {fmt(meta['max_turns'])}, max time {fmt(meta['max_time'])}, max cost {fmt(meta['max_cost_usd'], 'usd')}",
        "",
        f"**{summary['passed']}/{summary['tasks']} passed** · cost {fmt(summary['total_cost_usd'], 'usd')} · "
        f"cache hit rate {fmt(summary['cache_hit_rate'], 'pct')} · {summary['model_calls']} model calls · "
        f"{summary['compactions']} compactions · {summary['wall_time_s']}s",
        "",
        "| Task | Result | Exit reason | Model calls | Cost | Cache hits | Compactions | Open todos | Wall time |",
        "|------|--------|-------------|-------------|------|------------|-------------|------------|-----------|",
    ]
    for r in records:
        lines.append(
            f"| {r['task']} | {'pass' if r['passed'] else 'fail'} | {r['exit_reason']} | {r['num_turns']} | "
            f"{fmt(r['cost_usd'], 'usd')} | {fmt(r['cache_hit_rate'], 'pct')} | {r['compactions']} | "
            f"{r['todos_open']}/{r['todos_total']} | {r['wall_time_s']}s |"
        )
    failed = [r for r in records if not r["passed"]]
    if failed:
        lines += ["", "## Failures", ""]
        for r in failed:
            lines += [f"### {r['task']}", ""]
            if r["error"]:
                lines += [f"aura: {r['error']}", ""]
            lines += ["```", r["test_output_tail"], "```", ""]
    return "\n".join(lines) + "\n"


def run_evals(args: argparse.Namespace, tasks: List[Dict[str, Any]], scratch: Path) -> int:
    aura = find_aura(args.aura)
    started = datetime.datetime.now()
    out = Path(args.out) if args.out else REPO / "target" / "evals" / "long-horizon" / started.strftime("%Y%m%d-%H%M%S")
    out.mkdir(parents=True, exist_ok=True)
    records = [run_task(args, aura, task, scratch) for task in tasks]
    summary = summarize(records)
    meta = {
        "started": started.isoformat(timespec="seconds"),
        "aura": aura,
        "provider": args.provider,
        "model": args.model,
        "max_turns": args.max_turns,
        "max_time": args.max_time,
        "max_cost_usd": args.max_cost_usd,
    }
    report = {"suite": "long_horizon", "meta": meta, "summary": summary, "tasks": records}
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    (out / "report.md").write_text(markdown(meta, summary, records), encoding="utf-8")
    print(f"\n{summary['passed']}/{summary['tasks']} passed; report in {out}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--self-check", action="store_true", help="check the fixtures; no model is called")
    parser.add_argument("--live", action="store_true", help="allow real (paid) model calls; same as AURA_EVALS_LIVE=1")
    parser.add_argument("--tasks", help="comma-separated task ids (default: all)")
    parser.add_argument("--aura", help="aura binary (default: target/release, then target/debug, then PATH)")
    parser.add_argument("--provider", help="aura --provider (`mock` needs no --live)")
    parser.add_argument("--model", help="aura --model")
    parser.add_argument("--mode", help="aura --mode")
    parser.add_argument("--max-turns", type=int, help="aura --max-turns per task")
    parser.add_argument("--max-time", help="aura --max-time per task, e.g. 30m")
    parser.add_argument("--max-cost-usd", type=float, help="aura --max-cost-usd per task")
    parser.add_argument("--out", help="report directory (default: target/evals/long-horizon/<timestamp>)")
    parser.add_argument("--keep", action="store_true", help="keep the scratch projects for inspection")
    args = parser.parse_args()

    tasks = load_tasks(args.tasks.split(",") if args.tasks else None)
    live_ok = args.live or os.environ.get("AURA_EVALS_LIVE") == "1"
    if not args.self_check and args.provider != "mock" and not live_ok:
        print(
            "Refusing to call a real model without --live or AURA_EVALS_LIVE=1 "
            "(use --self-check or --provider mock for free runs).",
            file=sys.stderr,
        )
        return 2

    scratch = Path(tempfile.mkdtemp(prefix="aura-long-horizon-"))
    try:
        if args.self_check:
            return self_check(tasks, scratch)
        return run_evals(args, tasks, scratch)
    finally:
        if args.keep:
            print(f"scratch projects kept in {scratch}")
        else:
            shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
