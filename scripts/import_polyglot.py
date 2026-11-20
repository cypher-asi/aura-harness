#!/usr/bin/env python3
"""Import exercises of the Aider polyglot benchmark as `evals/polyglot` tasks.

The benchmark (https://github.com/Aider-AI/polyglot-benchmark) holds the
Exercism practice exercises that most models failed, per language. Each
becomes a task in the layout of `evals/gate_cost`:

    <lang>-<exercise>/task.toml          prompt from the exercise's instructions
    <lang>-<exercise>/workspace/...      the stub files the agent edits
    <lang>-<exercise>/checks/.gate_checks/...  the exercise's tests, hidden
    <lang>-<exercise>/solution/...       Exercism's example solution

Python, Rust and Go are imported (their tests need nothing but the
toolchain). By default the exercises with the longest example solutions
are taken, as the harder ones. Each task is proven before it is kept: its
check must fail on the stub and pass with the example solution, so
exercises that already work (refactoring ones), need crates or split
their solution over steps are left out, and the next one is taken.

    scripts/import_polyglot.py --per-lang 8
    scripts/import_polyglot.py --exercise rust/forth --exercise go/bowling
    scripts/import_polyglot.py --list

Exercise content is copyright Exercism, MIT licensed.
"""

import argparse
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ARCHIVE = "https://github.com/Aider-AI/polyglot-benchmark/archive/refs/heads/main.tar.gz"
REPO = Path(__file__).resolve().parent.parent
LANGS = ("python", "rust", "go")

VERIFY = {
    "python": "PYTHONPATH=. python3 -m unittest discover -s .gate_checks -p '*_test.py' -q",
    "rust": (
        "mkdir -p tests && cp .gate_checks/*.rs tests/ && "
        "cargo test --offline --quiet -- --include-ignored; s=$?; "
        "for f in .gate_checks/*.rs; do rm -f \"tests/${f##*/}\"; done; "
        "rmdir tests 2>/dev/null; exit $s"
    ),
    "go": (
        "cp .gate_checks/*_test.go . && go test ./...; s=$?; "
        "for f in .gate_checks/*_test.go; do rm -f \"${f##*/}\"; done; exit $s"
    ),
}

LANGUAGE = {"python": "Python", "rust": "Rust", "go": "Go"}


def fetch(source: str | None) -> Path:
    """The benchmark's root: `source` if it is a folder, else downloaded."""
    if source and Path(source).is_dir():
        return Path(source)
    cache = Path.home() / ".cache" / "aura-evals"
    root = cache / "polyglot-benchmark-main"
    if not root.is_dir():
        print(f"Downloading {ARCHIVE}", file=sys.stderr)
        with urllib.request.urlopen(source or ARCHIVE) as response:
            data = response.read()
        cache.mkdir(parents=True, exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            archive.extractall(cache, filter="data")
    return root


def exercises(root: Path, lang: str) -> list[Path]:
    practice = root / lang / "exercises" / "practice"
    return sorted(p for p in practice.iterdir() if (p / ".meta" / "config.json").is_file())


def size(exercise: Path) -> int:
    """Lines of the example solution, a proxy for how hard it is."""
    files = json.loads((exercise / ".meta" / "config.json").read_text())["files"]
    return sum(
        sum(1 for line in (exercise / f).read_text().splitlines() if line.strip())
        for f in files.get("example", [])
    )


def toml_string(text: str) -> str:
    escaped = text.replace("\\", "\\\\").replace('"""', '\\"\\"\\"')
    return f'"""\n{escaped}"""'


def instructions(exercise: Path) -> str:
    docs = exercise / ".docs"
    parts = [docs / "introduction.md", docs / "instructions.md", docs / "instructions.append.md"]
    return "\n\n".join(p.read_text().strip() for p in parts if p.is_file())


def convert(lang: str, exercise: Path, out: Path) -> str:
    config = json.loads((exercise / ".meta" / "config.json").read_text())
    files = config["files"]
    solution_files = [f for f in files["solution"] if f != "Cargo.toml"]
    tests = files["test"] + files.get("editor", [])
    slug = f"{lang}-{exercise.name}"
    task = out / slug
    if task.exists():
        shutil.rmtree(task)
    workspace, checks, solution = task / "workspace", task / "checks" / ".gate_checks", task / "solution"
    hidden = set(tests) | {".meta", ".docs"}
    for path in exercise.rglob("*"):
        relative = path.relative_to(exercise)
        if path.is_dir() or relative.parts[0] in (".meta", ".docs") or str(relative) in hidden:
            continue
        if lang == "rust" and relative.parts[0] == "tests":
            continue
        target = workspace / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, target)
    for test in tests:
        target = checks / Path(test).name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(exercise / test, target)
    examples = files["example"]
    if len(examples) != len(solution_files):
        raise ValueError(f"{slug}: {examples} do not map onto {solution_files}")
    for example, target in zip(examples, solution_files):
        (solution / target).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(exercise / example, solution / target)
    title = config.get("blurb", exercise.name).rstrip(".")
    prompt = (
        f"{instructions(exercise)}\n\n"
        f"Implement this in {LANGUAGE[lang]} in {', '.join(solution_files)}. Keep the names "
        "of the existing functions, types and modules: hidden tests use them. Use only the "
        "standard library.\n"
    )
    (task / "task.toml").write_text(
        f"title = {json.dumps(title)}\n"
        'category = "polyglot"\n'
        f'tags = ["hard", "{lang}", "exercism"]\n'
        f"prompt = {toml_string(prompt)}\n"
        f"verify = {json.dumps(VERIFY[lang])}\n"
        "max_iterations = 30\n"
    )
    return slug


def passes(task: Path, layers: tuple[str, ...], verify: str) -> bool:
    with tempfile.TemporaryDirectory() as tmp:
        for layer in layers:
            if (task / layer).is_dir():
                shutil.copytree(task / layer, tmp, dirs_exist_ok=True)
        try:
            done = subprocess.run(
                ["sh", "-c", verify],
                cwd=tmp,
                capture_output=True,
                timeout=600,
                env={**os.environ, "CARGO_TARGET_DIR": f"{tmp}/target"},
            )
        except subprocess.TimeoutExpired:
            return False
        return done.returncode == 0


def proven(lang: str, task: Path) -> bool:
    """Whether `task`'s check fails as shipped and passes when solved."""
    verify = VERIFY[lang]
    return not passes(task, ("workspace", "checks"), verify) and passes(
        task, ("workspace", "solution", "checks"), verify
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--source", help="a local checkout or an archive URL")
    parser.add_argument("--out", default=str(REPO / "evals" / "polyglot"))
    parser.add_argument("--lang", action="append", choices=LANGS)
    parser.add_argument("--per-lang", type=int, default=8)
    parser.add_argument("--exercise", action="append", default=[], help="<lang>/<exercise>")
    parser.add_argument("--list", action="store_true", help="list exercises by size and stop")
    parser.add_argument("--no-prove", action="store_true", help="keep tasks without proving them")
    args = parser.parse_args()
    root = fetch(args.source)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    def keep(lang: str, exercise: Path) -> bool:
        try:
            slug = convert(lang, exercise, out)
        except (ValueError, KeyError) as e:
            print(f"skipped {lang}/{exercise.name}: {e}", file=sys.stderr)
            return False
        if args.no_prove or proven(lang, out / slug):
            print(slug)
            return True
        shutil.rmtree(out / slug)
        print(f"skipped {slug}: its check does not fail as shipped and pass when solved", file=sys.stderr)
        return False

    for name in args.exercise:
        lang, _, exercise = name.partition("/")
        keep(lang, root / lang / "exercises" / "practice" / exercise)
    for lang in [] if args.exercise else args.lang or LANGS:
        ranked = sorted(exercises(root, lang), key=size, reverse=True)
        if args.list:
            for exercise in ranked:
                print(f"{lang}/{exercise.name}\t{size(exercise)}")
            continue
        kept = 0
        for exercise in ranked:
            if kept == args.per_lang:
                break
            kept += keep(lang, exercise)


if __name__ == "__main__":
    main()
