#!/usr/bin/env python3
"""Import real bug fixes from open-source repositories as `evals/codebases` tasks.

Each task is a merged pull request that closed a GitHub issue, the way
SWE-bench builds its tasks:

    <repo>-<issue>/task.toml                  the issue as the prompt; the
                                              repository and the commit
                                              before the fix as [source]
    <repo>-<issue>/checks/.gate_checks/run.sh puts the fix's tests in place,
                                              runs them, puts the files back
    <repo>-<issue>/checks/.gate_checks/files/ the fix's tests, hidden
    <repo>-<issue>/solution/                  the files the fix changed

The workspace is the whole repository at the commit before the fix, which
aura downloads once into `AURA_HOME/evals/sources` when a task runs; it is
not stored here. Only repositories whose tests need nothing but the
language's toolchain are imported, so checks run offline.

Fixes are taken from the newest merged pull requests whose title says they
fix something, that change between --min-lines and --max-lines lines of
code outside the tests, and that change tests too; the larger ones first.
Each task is proven before it is kept: its check must fail on the commit
before the fix and pass with the fix.

    scripts/import_repo_tasks.py --per-repo 5
    scripts/import_repo_tasks.py --repo yuin/goldmark --list
    scripts/import_repo_tasks.py --pr tobymao/sqlglot#4321

Needs `gh` signed in to GitHub. Repository content keeps its own license
(each repository's LICENSE); the tasks only point at it.
"""

import argparse
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath

REPO = Path(__file__).resolve().parent.parent
CACHE = Path.home() / ".cache" / "aura-evals" / "sources"

REPOS = {
    "tobymao/sqlglot": "python",
    "Python-Markdown/markdown": "python",
    "yuin/goldmark": "go",
    "expr-lang/expr": "go",
    "BurntSushi/toml": "go",
    "pelletier/go-toml": "go",
    "dlclark/regexp2": "go",
    "gomarkdown/markdown": "go",
    "antchfx/xpath": "go",
    "buger/jsonparser": "go",
    "Masterminds/semver": "go",
}

SLUG = {"Python-Markdown/markdown": "python-markdown", "gomarkdown/markdown": "gomarkdown"}

CODE = {"python": (".py",), "go": (".go",)}

LANGUAGE = {"python": "Python", "go": "Go"}

RUN_SH = """\
# Puts the hidden tests in place, runs them, then puts the workspace's
# own files back.
set -u
cd "$(dirname "$0")/.."
hidden=.gate_checks/files
saved=.gate_checks/saved
files=$(cd "$hidden" && find . -type f)
for f in $files; do
  if [ -e "$f" ]; then mkdir -p "$saved/$(dirname "$f")" && cp -p "$f" "$saved/$f"; fi
  mkdir -p "$(dirname "$f")" && cp "$hidden/$f" "$f"
done
{test}
status=$?
for f in $files; do
  if [ -e "$saved/$f" ]; then cp -p "$saved/$f" "$f"; else rm -f "$f"; fi
done
rm -rf "$saved"
exit $status
"""

QUERY = """
query($owner: String!, $name: String!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequests(states: MERGED, first: 50, after: $cursor,
                 orderBy: {field: CREATED_AT, direction: DESC}) {
      pageInfo { hasNextPage endCursor }
      nodes {
        number title
        mergeCommit { oid parents(first: 2) { nodes { oid } } }
        closingIssuesReferences(first: 2) { nodes { number title body } }
        files(first: 100) { totalCount nodes { path additions deletions changeType } }
      }
    }
  }
}
"""

ONE_PR = """
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      number title
      mergeCommit { oid parents(first: 2) { nodes { oid } } }
      closingIssuesReferences(first: 2) { nodes { number title body } }
      files(first: 100) { totalCount nodes { path additions deletions changeType } }
    }
  }
}
"""


def gh_graphql(query: str, **variables) -> dict:
    args = ["gh", "api", "graphql", "-f", f"query={query}"]
    for key, value in variables.items():
        if value is None:
            continue
        args += ["-F" if isinstance(value, int) else "-f", f"{key}={value}"]
    done = subprocess.run(args, capture_output=True, text=True, check=True)
    return json.loads(done.stdout)["data"]


def is_test(lang: str, path: str) -> bool:
    parts = PurePosixPath(path).parts
    name = parts[-1]
    if lang == "go":
        return name.endswith("_test.go") or "testdata" in parts
    return parts[0] == "tests" or name.startswith("test_") or "fixtures" in parts


def is_code(lang: str, path: str) -> bool:
    return path.endswith(CODE[lang])


def source_lines(lang: str, pr: dict) -> int:
    return sum(
        f["additions"] + f["deletions"]
        for f in pr["files"]["nodes"]
        if is_code(lang, f["path"]) and not is_test(lang, f["path"])
    )


def suitable(lang: str, pr: dict, low: int, high: int) -> str | None:
    """Why `pr` can't be a task, or None when it can."""
    if not pr["mergeCommit"] or not pr["closingIssuesReferences"]["nodes"]:
        return "closes no issue"
    if len(pr["closingIssuesReferences"]["nodes"]) > 1:
        return "closes more than one issue"
    if not re.search(r"\bfix", pr["title"], re.I):
        return "not a fix"
    files = pr["files"]["nodes"]
    if pr["files"]["totalCount"] > len(files):
        return "too many files"
    if not any(is_test(lang, f["path"]) for f in files):
        return "changes no tests"
    if not any(is_code(lang, f["path"]) and not is_test(lang, f["path"]) for f in files):
        return "changes no code"
    if any(f["changeType"] in ("DELETED", "RENAMED") and not is_test(lang, f["path"]) for f in files):
        return "deletes or renames a file"
    lines = source_lines(lang, pr)
    if not low <= lines <= high:
        return f"{lines} lines of code"
    return None


def candidates(repo: str, lang: str, pages: int, low: int, high: int) -> list[dict]:
    owner, name = repo.split("/")
    found, cursor = [], None
    for _ in range(pages):
        data = gh_graphql(QUERY, owner=owner, name=name, cursor=cursor)["repository"]["pullRequests"]
        found += [pr for pr in data["nodes"] if suitable(lang, pr, low, high) is None]
        if not data["pageInfo"]["hasNextPage"]:
            break
        cursor = data["pageInfo"]["endCursor"]
    return sorted(found, key=lambda pr: source_lines(lang, pr), reverse=True)


def snapshot(repo: str, commit: str) -> Path:
    """The repository's files at `commit`, downloaded once."""
    owner, name = repo.split("/")
    root = CACHE / f"{owner}-{name}-{commit}"
    if root.is_dir():
        return root
    url = f"https://codeload.github.com/{repo}/tar.gz/{commit}"
    print(f"Downloading {url}", file=sys.stderr)
    with urllib.request.urlopen(url) as response:
        data = response.read()
    partial = root.with_name(root.name + ".partial")
    shutil.rmtree(partial, ignore_errors=True)
    partial.mkdir(parents=True)
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for member in archive.getmembers():
            parts = PurePosixPath(member.name).parts[1:]
            if not parts or not (member.isfile() or member.isdir()):
                continue
            member.name = str(PurePosixPath(*parts))
            archive.extract(member, partial, filter="data")
    partial.rename(root)
    return root


def raw(repo: str, commit: str, path: str) -> bytes:
    url = f"https://raw.githubusercontent.com/{repo}/{commit}/{urllib.parse.quote(path)}"
    with urllib.request.urlopen(url) as response:
        return response.read()


def changed_files(repo: str, base: str, head: str) -> list[dict]:
    done = subprocess.run(
        ["gh", "api", f"repos/{repo}/compare/{base}...{head}", "--jq", "[.files[] | select((.patch // \"\") | contains(\"Subproject commit\") | not) | {filename, status}]"],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(done.stdout)


def test_command(lang: str, tests: list[str], code: list[str]) -> str | None:
    if lang == "go":
        packages = set()
        for path in tests + code:
            parts = PurePosixPath(path).parts
            if "testdata" in parts:
                parts = parts[: parts.index("testdata")] + ("x",)
            directory = "/".join(parts[:-1])
            packages.add(f"./{directory}" if directory else ".")
        return "go test -count=1 " + " ".join(sorted(packages))
    modules = sorted(
        str(PurePosixPath(path).with_suffix("")).replace("/", ".")
        for path in tests
        if PurePosixPath(path).name.startswith("test_") and path.endswith(".py")
    )
    if not modules:
        return None
    return "python3 -m unittest -q " + " ".join(modules)


def clean_issue(body: str) -> str:
    body = re.sub(r"<!--.*?-->", "", body or "", flags=re.S)
    body = re.sub(r"\n{3,}", "\n\n", body).strip()
    return body[:8000]


def task_name(repo: str, issue: int) -> str:
    return f"{SLUG.get(repo, repo.split('/')[1].lower())}-{issue}"


def toml_string(text: str) -> str:
    escaped = text.replace("\\", "\\\\").replace('"""', '\\"\\"\\"')
    return f'"""\n{escaped}\n"""'


def convert(repo: str, lang: str, pr: dict, out: Path) -> Path | None:
    owner, name = repo.split("/")
    issue = dict(pr["closingIssuesReferences"]["nodes"][0])
    issue["title"] = " ".join(issue["title"].replace("<!--", "").replace("-->", "").split())
    head = pr["mergeCommit"]["oid"]
    base = pr["mergeCommit"]["parents"]["nodes"][0]["oid"]
    files = changed_files(repo, base, head)
    if any(f["status"] in ("removed", "renamed") and not is_test(lang, f["filename"]) for f in files):
        print(f"skipped {repo}#{pr['number']}: deletes or renames a file", file=sys.stderr)
        return None
    tests = [f["filename"] for f in files if is_test(lang, f["filename"]) and f["status"] != "removed"]
    code = [f["filename"] for f in files if not is_test(lang, f["filename"]) and is_code(lang, f["filename"])]
    solution = [f["filename"] for f in files if not is_test(lang, f["filename"])]
    command = test_command(lang, tests, code)
    if not command:
        print(f"skipped {repo}#{pr['number']}: no test module to run", file=sys.stderr)
        return None
    task = out / task_name(repo, issue["number"])
    if task.exists():
        shutil.rmtree(task)
    hidden = task / "checks" / ".gate_checks"
    for path in tests:
        target = hidden / "files" / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(raw(repo, head, path))
    hidden.mkdir(parents=True, exist_ok=True)
    (hidden / "run.sh").write_text(RUN_SH.replace("{test}", command))
    for path in solution:
        target = task / "solution" / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(raw(repo, head, path))
    prompt = (
        f"You are working in a checkout of {repo} ({LANGUAGE[lang]}). Resolve this GitHub issue.\n\n"
        f"# {issue['title']}\n\n{clean_issue(issue['body'])}\n\n"
        "Fix it in the library's code. Tests that check the fix are hidden and run after you "
        "finish; the repository's existing tests must keep passing. Use only what is in the "
        "repository and the standard library.\n"
    )
    (task / "task.toml").write_text(
        f"# Issue: https://github.com/{repo}/issues/{issue['number']}\n"
        f"# Fix:   https://github.com/{repo}/pull/{pr['number']}\n"
        f"title = {json.dumps(f'{name}: {issue['title']}')}\n"
        'category = "codebase"\n'
        f'tags = ["hard", "{lang}", "{name.lower()}"]\n'
        f"prompt = {toml_string(prompt)}\n"
        'verify = "sh .gate_checks/run.sh"\n'
        "max_iterations = 80\n"
        "\n[source]\n"
        f'repo = "https://github.com/{repo}"\n'
        f'commit = "{base}"\n'
    )
    return task


def passes(task: Path, base: Path, solved: bool) -> bool:
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copytree(base, tmp, dirs_exist_ok=True)
        layers = ("solution", "checks") if solved else ("checks",)
        for layer in layers:
            shutil.copytree(task / layer, tmp, dirs_exist_ok=True)
        try:
            done = subprocess.run(
                ["sh", "-c", "sh .gate_checks/run.sh"],
                cwd=tmp,
                capture_output=True,
                timeout=900,
                env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
            )
        except subprocess.TimeoutExpired:
            return False
        if solved and done.returncode != 0:
            tail = (done.stdout + done.stderr).decode(errors="replace")[-1500:]
            print(f"  {task.name} fails when solved:\n{tail}", file=sys.stderr)
        return done.returncode == 0


def proven(repo: str, task: Path, base_commit: str) -> bool:
    base = snapshot(repo, base_commit)
    go_mod = base / "go.mod"
    if go_mod.is_file() and re.search(r"^\s*require\b", go_mod.read_text(), re.M):
        print(f"  {task.name} needs Go modules, which a check can't download", file=sys.stderr)
        return False
    return not passes(task, base, False) and passes(task, base, True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", default=str(REPO / "evals" / "codebases"))
    parser.add_argument("--repo", action="append", choices=sorted(REPOS))
    parser.add_argument("--per-repo", type=int, default=5)
    parser.add_argument("--pages", type=int, default=6, help="pages of 50 merged pull requests")
    parser.add_argument("--min-lines", type=int, default=30)
    parser.add_argument("--max-lines", type=int, default=400)
    parser.add_argument("--pr", action="append", default=[], help="<owner>/<repo>#<number>")
    parser.add_argument("--list", action="store_true", help="list the candidates and stop")
    parser.add_argument("--no-prove", action="store_true", help="keep tasks without proving them")
    args = parser.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    def keep(repo: str, pr: dict) -> bool:
        lang = REPOS[repo]
        try:
            task = convert(repo, lang, pr, out)
        except (urllib.error.URLError, subprocess.CalledProcessError) as e:
            print(f"skipped {repo}#{pr['number']}: {e}", file=sys.stderr)
            issue = pr["closingIssuesReferences"]["nodes"][0]["number"]
            shutil.rmtree(out / task_name(repo, issue), ignore_errors=True)
            return False
        if task is None:
            return False
        base = pr["mergeCommit"]["parents"]["nodes"][0]["oid"]
        if args.no_prove or proven(repo, task, base):
            print(f"{task.name}\t{repo}#{pr['number']}\t{source_lines(lang, pr)} lines", flush=True)
            return True
        shutil.rmtree(task)
        print(f"skipped {repo}#{pr['number']}: its check does not fail before the fix and pass after it", file=sys.stderr)
        return False

    for spec in args.pr:
        repo, _, number = spec.partition("#")
        owner, name = repo.split("/")
        pr = gh_graphql(ONE_PR, owner=owner, name=name, number=int(number))["repository"]["pullRequest"]
        keep(repo, pr)
    for repo in [] if args.pr else args.repo or REPOS:
        lang = REPOS[repo]
        found = candidates(repo, lang, args.pages, args.min_lines, args.max_lines)
        if args.list:
            for pr in found:
                issue = pr["closingIssuesReferences"]["nodes"][0]
                print(f"{repo}#{pr['number']}\t{source_lines(lang, pr)}\t#{issue['number']} {issue['title']}")
            continue
        kept = 0
        for pr in found:
            if kept == args.per_repo:
                break
            kept += keep(repo, pr)


if __name__ == "__main__":
    main()
