#!/usr/bin/env python3
"""Summarise `/compare` runs from their run bundles, as JSON.

Reads `AURA_HOME/runs/*/{meta,summary,steps}` (read only) and prints one
object per comparison: per task, each arm's cost, wall time, hidden-check
verdict, Gate's routes, and the cost of every model call split by what it
was for (the work on each model, and Gate's classify, criteria, review and
judge calls). Calls are priced from their recorded token usage at the list
prices in `crates/model/aura-model/src/model_catalog.rs`, and the script
checks that they add up to the run's recorded `cost_usd`.

    scripts/compare_summary.py --compare 37 --suite swe_bench_pro_hard
    scripts/compare_summary.py --compare 28-37 > compares.json
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

# USD per million tokens: input, output, cache write, cache read.
# Mirrors `model_catalog.rs`; extend it when a run used another model.
PRICES = {
    "claude-opus-5-5": (4.0, 20.0, 5.0, 0.2),
    "claude-sonnet-5-5": (2.0, 10.0, 2.5, 0.1),
    "claude-haiku-5-5": (0.1, 0.5, 0.125, 0.01),
}

TITLE = re.compile(r"Compare #(\d+) (\S+): (.*)")


def model_name(policy_id: str) -> str:
    """`claude/claude-sonnet-5-5` -> `claude-sonnet-5-5`."""
    return policy_id.rsplit("/", 1)[-1]


def call_cost(model: str, usage: dict) -> float | None:
    price = PRICES.get(model)
    if price is None:
        return None
    tin, tout, twrite, tread = price
    return (
        usage.get("input", 0) * tin
        + usage.get("output", 0) * tout
        + usage.get("cache_write", 0) * twrite
        + usage.get("cache_read", 0) * tread
    ) / 1e6


def parse_range(text: str) -> set[int]:
    out: set[int] = set()
    for part in text.split(","):
        if "-" in part:
            lo, hi = part.split("-", 1)
            out.update(range(int(lo), int(hi) + 1))
        elif part:
            out.add(int(part))
    return out


def read_run(run_dir: Path) -> dict | None:
    try:
        meta = json.loads((run_dir / "meta.json").read_text())
    except (OSError, json.JSONDecodeError):
        return None
    match = TITLE.match(meta.get("title", ""))
    if not match:
        return None
    try:
        summary = json.loads((run_dir / "summary.json").read_text())
    except (OSError, json.JSONDecodeError):
        summary = {}

    routes = []
    spend: dict[str, float] = defaultdict(float)
    calls: dict[str, int] = defaultdict(int)
    unpriced = 0
    first_model = None
    steps_path = run_dir / "steps.jsonl"
    if steps_path.exists():
        for line in steps_path.open():
            step = json.loads(line)
            action = step.get("action", {})
            actor = step.get("actor", {})
            if action.get("type") == "harness" and action.get("name") == "route":
                route = action.get("action", {})
                obs = step.get("observation", {})
                features = obs.get("features", {})
                trigger = obs.get("trigger", {})
                routes.append(
                    {
                        "model": model_name(route.get("model", "")),
                        "effort": route.get("effort"),
                        "category": features.get("category"),
                        "difficulty": features.get("difficulty"),
                        "reason": trigger.get("reason"),
                        "explored": trigger.get("explored"),
                    }
                )
                if first_model is None:
                    first_model = model_name(route.get("model", ""))
                continue
            if action.get("type") != "model" or "usage" not in step:
                continue
            model = model_name(step.get("policy", {}).get("id", ""))
            if actor.get("kind") == "harness":
                bucket = f"gate:{action.get('purpose', 'other')}"
            elif first_model is None or model == first_model:
                bucket = "work"
            else:
                bucket = "escalated"
            cost = call_cost(model, step["usage"])
            calls[bucket] += 1
            if cost is None:
                unpriced += 1
            else:
                spend[bucket] += cost

    return {
        "compare": int(match.group(1)),
        "slot": match.group(2),
        "title": match.group(3),
        "run_id": meta.get("run_id"),
        "task": meta.get("task_id"),
        "arm": meta.get("arm"),
        "started_at_ms": meta.get("started_at_ms"),
        "ended_at_ms": meta.get("ended_at_ms"),
        "cost_usd": summary.get("cost_usd"),
        "wall_ms": summary.get("wall_ms"),
        "outcome": summary.get("outcome"),
        "model_calls": summary.get("model_calls"),
        "files_changed": summary.get("files_changed"),
        "routes": routes,
        "calls": dict(calls),
        "spend_usd": {k: round(v, 6) for k, v in spend.items()},
        "priced_usd": round(sum(spend.values()), 6),
        "unpriced_calls": unpriced,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--home", default=os.environ.get("AURA_HOME", str(Path.home() / ".aura")))
    parser.add_argument("--compare", default="", help="numbers or ranges, e.g. 28-37 or 36,37")
    parser.add_argument("--suite", default="", help="keep tasks whose id starts with this, e.g. swe_bench_pro_hard")
    args = parser.parse_args()

    wanted = parse_range(args.compare)
    runs = []
    for run_dir in sorted((Path(args.home) / "runs").iterdir()):
        run = read_run(run_dir)
        if run is None or (wanted and run["compare"] not in wanted):
            continue
        if args.suite and not (run["task"] or "").startswith(args.suite):
            continue
        runs.append(run)

    for run in runs:
        recorded = run["cost_usd"] or 0.0
        if run["unpriced_calls"] == 0 and abs(run["priced_usd"] - recorded) > max(0.01, 0.02 * recorded):
            print(
                f"warning: {run['run_id']} priced ${run['priced_usd']:.4f} from steps, recorded ${recorded:.4f}",
                file=sys.stderr,
            )

    compares: dict[int, dict] = {}
    for run in runs:
        entry = compares.setdefault(run["compare"], {"compare": run["compare"], "tasks": {}})
        arm = "gate" if "gate" in (run["arm"] or "") else model_name((run["arm"] or "").split("·")[-1].strip())
        entry["tasks"].setdefault(run["task"], {})[arm] = run

    for entry in compares.values():
        finished = [t for t in entry["tasks"].values() if all(r.get("outcome") is not None for r in t.values()) and len(t) > 1]
        entry["pairs_finished"] = len(finished)
        totals: dict[str, dict] = {}
        for task in finished:
            for arm, run in task.items():
                tot = totals.setdefault(arm, {"cost_usd": 0.0, "wall_ms": 0, "passed": 0, "runs": 0})
                tot["cost_usd"] = round(tot["cost_usd"] + (run["cost_usd"] or 0.0), 6)
                tot["wall_ms"] += run["wall_ms"] or 0
                tot["passed"] += int((run["outcome"] or 0) >= 1.0)
                tot["runs"] += 1
        entry["totals_finished_pairs"] = totals

    json.dump([compares[n] for n in sorted(compares)], sys.stdout, indent=2)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
