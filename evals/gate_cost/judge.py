#!/usr/bin/env python3
"""Blind pairwise review of two eval arms on the gate cost suite.

For each task, the runs of the reference arm and the candidate arm are
paired in start order. A judge model sees both attempts (the task's
prompts, each attempt's diff against the fixture, and each final reply)
in random order without arm names, and picks the better one. Judge
calls go through `aura -p`, so they are recorded like any other model
call. Both attempts usually passed the hidden checks, so the judge is
told to weigh what the checks do not: scope, code quality and the
accuracy of the reply.

    python3 evals/gate_cost/judge.py --reference opus --arm gate-v2

Judgments are appended to AURA_HOME/gate/judgments/<reference>-vs-<arm>.jsonl
and a summary is printed. Pairs already judged there are skipped.
"""

import argparse
import collections
import difflib
import json
import math
import os
import random
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

SKIP_DIRS = {".gate_checks", "__pycache__", "target", "aura_data", ".aura", ".git"}
MAX_DIFF = 12_000
MAX_REPLY = 4_000

JUDGE_PROMPT = """You are reviewing two attempts at the same coding task, made by different \
agents in identical copies of the same small repository. Both may have passed the task's \
automated checks, so judge what the checks miss:

- correctness and completeness against everything the user asked, including edge cases;
- scope: no changes nobody asked for;
- code quality: clear, idiomatic, consistent with the surrounding code;
- the final reply: accurate about what was done, no false claims, useful to the user.

## The user's messages

{prompts}

## Attempt A

### Changes
```diff
{diff_a}
```

### Final reply
{reply_a}

## Attempt B

### Changes
```diff
{diff_b}
```

### Final reply
{reply_b}

Explain your judgment in two to four sentences, naming the concrete differences that \
decided it. Then end with one line that is exactly `WINNER: A`, `WINNER: B` or `WINNER: TIE` \
(TIE only when neither is meaningfully better)."""


def aura_home() -> Path:
    return Path(os.environ.get("AURA_HOME", Path.home() / ".aura"))


def read_tree(root: Path) -> dict[str, list[str]]:
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            path = Path(dirpath) / name
            try:
                out[str(path.relative_to(root))] = path.read_text().splitlines()
            except (UnicodeDecodeError, OSError):
                out[str(path.relative_to(root))] = ["<binary>"]
    return out


def diff(before: dict, after: dict) -> str:
    chunks = []
    for name in sorted(set(before) | set(after)):
        old, new = before.get(name), after.get(name)
        if old == new:
            continue
        chunks.extend(
            difflib.unified_diff(
                old or [],
                new or [],
                f"a/{name}" if old is not None else "/dev/null",
                f"b/{name}" if new is not None else "/dev/null",
                lineterm="",
                n=2,
            )
        )
    text = "\n".join(chunks) or "(no changes)"
    if len(text) > MAX_DIFF:
        text = text[:MAX_DIFF] + "\n... (diff cut)"
    return text


def conversation(run_dir: Path) -> tuple[list[str], list[str]]:
    """User prompts and the last model reply of each turn."""
    prompts, replies = [], {}
    steps = run_dir / "steps.jsonl"
    if not steps.exists():
        return prompts, []
    for line in steps.read_text().splitlines():
        step = json.loads(line)
        action = step.get("action", {})
        if action.get("type") == "user_prompt":
            prompts.append(action.get("text", ""))
        elif step.get("actor", {}).get("kind") == "model" and action.get("text"):
            replies[step.get("turn")] = action["text"]
    return prompts, [replies[t] for t in sorted(replies)]


def runs_by_task(arm: str) -> dict[str, list[dict]]:
    out = collections.defaultdict(list)
    for run_dir in (aura_home() / "runs").iterdir():
        try:
            meta = json.loads((run_dir / "meta.json").read_text())
            summary = json.loads((run_dir / "summary.json").read_text())
        except (OSError, json.JSONDecodeError):
            continue
        task = meta.get("task_id", "")
        if meta.get("arm") != arm or not task.startswith("gate_cost/"):
            continue
        out[task.split("/", 1)[1]].append(
            {"run": run_dir, "meta": meta, "outcome": summary.get("scorecard", summary).get("outcome")}
        )
    for runs in out.values():
        runs.sort(key=lambda r: r["meta"].get("started_at_ms", 0))
    return out


def attempt(suite: Path, task: str, run: dict) -> dict:
    workspace = Path(run["meta"]["workspace"])
    prompts, replies = conversation(run["run"])
    reply = "\n\n".join(replies) or "(no reply)"
    if len(reply) > MAX_REPLY:
        reply = reply[:MAX_REPLY] + "\n... (reply cut)"
    return {
        "prompts": prompts,
        "diff": diff(read_tree(suite / task / "workspace"), read_tree(workspace)),
        "reply": reply,
    }


def judge(aura: str, model: str, prompt: str) -> tuple[str | None, str, float]:
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ, AURA_DATA_DIR=str(Path(tmp) / "aura_data"))
        proc = subprocess.run(
            [aura, "-p", "-", "--output-format", "json", "--model", model, "--mode", "ask",
             "--cwd", tmp, "--max-turns", "2"],
            input=prompt, capture_output=True, text=True, env=env, timeout=600,
        )
    try:
        out = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return None, proc.stderr[-500:], 0.0
    text = out.get("result") or out.get("error") or ""
    found = re.findall(r"^\s*\**WINNER:\s*(A|B|TIE)\**\s*$", text, re.MULTILINE | re.IGNORECASE)
    return (found[-1].upper() if found else None), text, out.get("total_cost_usd") or 0.0


def sign_test(wins: int, losses: int) -> float:
    """Two-sided exact sign test p-value, ties dropped."""
    n = wins + losses
    if n == 0:
        return 1.0
    k = min(wins, losses)
    tail = sum(math.comb(n, i) for i in range(k + 1)) / 2**n
    return min(1.0, 2 * tail)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--suite", default=str(Path(__file__).resolve().parent))
    ap.add_argument("--reference", required=True)
    ap.add_argument("--arm", required=True)
    ap.add_argument("--model", default="claude/claude-opus-5-5")
    ap.add_argument("--aura", default="aura")
    ap.add_argument("--max-pairs", type=int, default=3, help="pairs per task")
    ap.add_argument("--dry-run", action="store_true", help="print the first prompt and stop")
    args = ap.parse_args()

    suite = Path(args.suite)
    ref_runs, arm_runs = runs_by_task(args.reference), runs_by_task(args.arm)
    out_dir = aura_home() / "gate" / "judgments"
    out_dir.mkdir(parents=True, exist_ok=True)
    out_path = out_dir / f"{args.reference}-vs-{args.arm}.jsonl"
    done = set()
    if out_path.exists():
        for line in out_path.read_text().splitlines():
            j = json.loads(line)
            done.add((j["reference_run"], j["arm_run"]))

    spent = 0.0
    with out_path.open("a") as out:
        for task in sorted(set(ref_runs) & set(arm_runs)):
            category = tomllib.loads((suite / task / "task.toml").read_text()).get("category", "general")
            for i, (ref, cand) in enumerate(list(zip(ref_runs[task], arm_runs[task]))[: args.max_pairs]):
                key = (ref["run"].name, cand["run"].name)
                if key in done:
                    continue
                a_ref = attempt(suite, task, ref)
                a_arm = attempt(suite, task, cand)
                arm_is_a = random.Random(f"{task}/{i}").random() < 0.5
                first, second = (a_arm, a_ref) if arm_is_a else (a_ref, a_arm)
                prompt = JUDGE_PROMPT.format(
                    prompts="\n\n".join(f"{n + 1}. {p}" for n, p in enumerate(a_ref["prompts"])),
                    diff_a=first["diff"], reply_a=first["reply"],
                    diff_b=second["diff"], reply_b=second["reply"],
                )
                if args.dry_run:
                    print(prompt)
                    return 0
                verdict, text, cost = judge(args.aura, args.model, prompt)
                spent += cost
                winner = None
                if verdict == "TIE":
                    winner = "tie"
                elif verdict in ("A", "B"):
                    winner = args.arm if (verdict == "A") == arm_is_a else args.reference
                record = {
                    "task": task, "category": category, "pair": i,
                    "reference_run": key[0], "arm_run": key[1],
                    "reference_outcome": ref["outcome"], "arm_outcome": cand["outcome"],
                    "arm_shown_as": "A" if arm_is_a else "B", "verdict": verdict,
                    "winner": winner, "judge_model": args.model, "cost_usd": cost, "reasoning": text,
                }
                out.write(json.dumps(record) + "\n")
                out.flush()
                print(f"{task:32} pair {i}: {winner or 'unparsed'}", flush=True)

    rows = [json.loads(l) for l in out_path.read_text().splitlines()]
    summary(rows, args.reference, args.arm, spent)
    return 0


def summary(rows: list[dict], reference: str, arm: str, spent: float) -> None:
    def count(rs):
        c = collections.Counter(r["winner"] for r in rs)
        return c[arm], c[reference], c["tie"], sum(1 for r in rs if r["winner"] is None)

    w, l, t, u = count(rows)
    print(f"\n{arm} vs {reference}: {len(rows)} judged pairs")
    print(f"  {arm} better {w}, {reference} better {l}, tie {t}, unparsed {u}; "
          f"sign test p = {sign_test(w, l):.3f}  (judging cost ${spent:.2f} this run)")
    by_cat = collections.defaultdict(list)
    for r in rows:
        by_cat[r["category"]].append(r)
    print(f"  {'category':16}{'pairs':>6}{arm:>10}{reference:>10}{'tie':>6}")
    for cat in sorted(by_cat):
        w, l, t, _ = count(by_cat[cat])
        print(f"  {cat:16}{len(by_cat[cat]):>6}{w:>10}{l:>10}{t:>6}")


if __name__ == "__main__":
    sys.exit(main())
