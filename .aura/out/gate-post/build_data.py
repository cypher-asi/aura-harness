#!/usr/bin/env python3
"""Build the website's Gate chart data from a compare_summary.py export.

    python3 scripts/compare_summary.py --compare 28-37 --suite swe_bench_pro_hard > data/compares.json
    python3 build_data.py data/compares.json > src/features/terminal/charts/gate-data.ts

Only pairs where both arms' runs finished with a hidden-test verdict are
counted. Every number in the generated file comes from the export; the
notes on each Compare are written here by hand from the aura-harness
CHANGELOG and commit log.
"""

from __future__ import annotations

import json
import sys

HEADLINE = 37
SECOND = 36
OPUS = "claude-opus-5-5"
# The four SWE-Bench Pro tasks every Compare from #29 to #37 finished on both arms.
CORE = ["ansible-40ade1f8", "ansible-b748edea", "ansible-c1f2df47", "ansible-d62496fe"]

# What ran before each Compare, and what it led to. From CHANGELOG.md and `git log`.
NOTES = {
    29: {
        "at": "Nov 21, 16:48",
        "before": "Gate picks a reasoning effort with the model (d045c69a)",
        "found": "Medium-difficulty work was sent to Haiku. On ansible-b748edea, Haiku passed tests it wrote for its own code and failed the hidden ones.",
    },
    30: {
        "at": "Nov 21, 17:05",
        "before": "Same design as #29",
        "found": "A reply saying only a lint step could not run sent correct work to the judge, and then to Opus at max effort: 24 minutes and $1.84 for one run.",
    },
    31: {
        "at": "Nov 21, 18:12",
        "before": "Acceptance criteria are written before a code turn (212d8deb)",
        "found": "A piped test run whose exit status came from grep counted as a failure. On Opus, that retried correct work at max effort twice, costing at least $4.55.",
    },
    32: {
        "at": "Nov 21, 19:15",
        "before": "Checks are decided by what they print; no max-effort pass when nothing failed (54b08052)",
        "found": "Haiku again got the medium multipart task and failed it. After this, work that is not easy starts on Sonnet (7fe38459).",
    },
    33: {
        "at": "Nov 21, 23:31",
        "before": "Not-easy work starts on Sonnet; criteria are written on Sonnet; the cost of Gate's own calls is logged",
        "found": "Sonnet had fallen below the success target on hard work, so the table sent two of the five tasks to Opus. On ansible-b748edea, Opus at high and then at max cost $2.43; Opus alone finished the same task for $0.42.",
    },
    36: {
        "at": "Nov 22, 00:44",
        "before": "Work below Opus is judged on its own model; logged reviews run on a quarter of turns (a1d5a562, 0ae25f05)",
        "found": "Every task started and stayed on Sonnet except one, which Opus took over and which was then stopped.",
    },
    37: {
        "at": "Nov 22, 01:14",
        "before": "A turn that ran its change itself is not nudged (d07d3e3a)",
        "found": "Every task started on Sonnet. One moved up to Opus after the judge found a gap, and every task passed on both arms.",
    },
}


def finished(entry: dict) -> list[tuple[str, dict]]:
    out = []
    for task, arms in entry["tasks"].items():
        if "gate" in arms and OPUS in arms and all(r["outcome"] is not None for r in arms.values()):
            out.append((task.split("/")[-1], arms))
    return sorted(out, key=lambda item: int(item[1]["gate"]["slot"].split("-")[1]))


def arm(run: dict) -> dict:
    return {
        "cost": round(run["cost_usd"] or 0.0, 4),
        "wallMin": round((run["wall_ms"] or 0) / 60000, 1),
        "passed": (run["outcome"] or 0) >= 1.0,
    }


def totals(pairs: list[tuple[str, dict]], name: str) -> dict:
    runs = [arms[name] for _, arms in pairs]
    return {
        "cost": round(sum(r["cost_usd"] or 0.0 for r in runs), 2),
        "wallMin": round(sum(r["wall_ms"] or 0 for r in runs) / 60000),
        "passed": sum((r["outcome"] or 0) >= 1.0 for r in runs),
        "runs": len(runs),
    }


def main() -> int:
    compares = {c["compare"]: c for c in json.load(open(sys.argv[1]))}
    head = finished(compares[HEADLINE])

    tasks = []
    for name, arms in head:
        gate = arms["gate"]
        models = []
        for route in gate["routes"]:
            label = route["model"].replace("claude-", "").replace("-5-5", " 5.5").capitalize()
            models.append(f"{label} at {route['effort']}" if route["effort"] else label)
        own = sum(v for k, v in gate["spend_usd"].items() if k.startswith("gate:"))
        tasks.append(
            {
                "task": name,
                "repo": name.rsplit("-", 1)[0],
                "title": gate["title"].split(": ", 1)[-1],
                "category": (gate["routes"][0]["category"] if gate["routes"] else None),
                "gate": {**arm(gate), "route": models, "ownCalls": round(own, 4)},
                "opus": arm(arms[OPUS]),
            }
        )

    spend = {}
    for _, arms in head:
        for key, value in arms["gate"]["spend_usd"].items():
            spend[key] = spend.get(key, 0.0) + value
    calls = {}
    for _, arms in head:
        for key, value in arms["gate"]["calls"].items():
            calls[key] = calls.get(key, 0) + value

    iterations = []
    for number in sorted(NOTES):
        pairs = [p for p in finished(compares[number]) if p[0] in CORE]
        if len(pairs) != len(CORE):
            continue
        gate, opus = totals(pairs, "gate"), totals(pairs, OPUS)
        logged = all("gate:classify" in arms["gate"]["calls"] for _, arms in pairs)
        iterations.append(
            {
                "compare": number,
                **NOTES[number],
                "gate": gate,
                "opus": opus,
                "ratio": round(gate["cost"] / opus["cost"], 2),
                "gateCallsCounted": logged,
            }
        )

    data = {
        "headline": {
            "compare": HEADLINE,
            "pairs": len(head),
            "gate": totals(head, "gate"),
            "opus": totals(head, OPUS),
        },
        "second": {
            "compare": SECOND,
            "pairs": len(finished(compares[SECOND])),
            "gate": totals(finished(compares[SECOND]), "gate"),
            "opus": totals(finished(compares[SECOND]), OPUS),
        },
        "tasks": tasks,
        "spend": {k: round(v, 4) for k, v in sorted(spend.items())},
        "calls": calls,
        "iterations": iterations,
        "core": CORE,
    }

    print("/*")
    print(" * Gate's SWE-Bench Pro HARD-51 results, generated by build_data.py from")
    print(" * aura-harness scripts/compare_summary.py. Do not edit by hand. USD at list")
    print(" * prices; wall time in minutes. Only pairs where both arms finished are counted.")
    print(" */")
    print()
    print(f"export const GATE_RESULTS = {json.dumps(data, indent=2)} as const;")
    return 0


if __name__ == "__main__":
    sys.exit(main())
