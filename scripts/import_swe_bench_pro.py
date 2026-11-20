#!/usr/bin/env python3
"""Import SWE-Bench Pro V2 tasks (by default its HARD-51 subset) as `evals/swe_bench_pro_hard`.

SWE-Bench Pro (Scale AI, https://github.com/scaleapi/SWE-bench_Pro-os) is
real pull requests in 11 open-source repositories (Go, Python, JavaScript,
TypeScript). Each task is a public container image holding the repository
at the commit before the change, with its dependencies installed, and a
verifier that applies hidden tests and requires every fail-to-pass and
pass-to-pass test to pass. HARD-51 is the 51 tasks that at least two of
five model families failed.

Each task becomes:

    <repo>-<sha>/task.toml                    the PR description as the
                                              prompt; the image as [source]
    <repo>-<sha>/workspace/.swe/              `run`, which runs a command in
                                              a fresh container of the image
                                              with the agent's changes
    <repo>-<sha>/checks/.gate_checks/         `grade.sh` and the verifier
                                              (`tests/`), hidden
    <repo>-<sha>/solution/gold_patch.diff     the reference change

aura copies the repository's tracked files out of the image when a task
first runs (pulling the image, 1-5 GB, if Docker does not have it). The
check grades as Scale's harness does: the run's changes as one patch,
applied in a fresh container of the image, then the hidden tests.

    scripts/import_swe_bench_pro.py                 # HARD-51
    scripts/import_swe_bench_pro.py --all --out evals/swe_bench_pro

Task content is from SWE-Bench Pro (MIT licensed); the repositories keep
their own licenses.
"""

import argparse
import io
import json
import re
import shutil
import sys
import tarfile
import urllib.request
from pathlib import Path, PurePosixPath

REPO = Path(__file__).resolve().parent.parent
SOURCE = "scaleapi/SWE-bench_Pro-os"
COMMIT = "66f92766bba642462d4bbe5479e83f91f9211862"
CACHE = Path.home() / ".cache" / "aura-evals" / f"SWE-bench_Pro-os-{COMMIT}"

LANGUAGE = {
    "NodeBB/NodeBB": "javascript",
    "ansible/ansible": "python",
    "element-hq/element-web": "typescript",
    "flipt-io/flipt": "go",
    "future-architect/vuls": "go",
    "gravitational/teleport": "go",
    "internetarchive/openlibrary": "python",
    "navidrome/navidrome": "go",
    "protonmail/webclients": "typescript",
    "qutebrowser/qutebrowser": "python",
    "tutao/tutanota": "typescript",
}

VERIFIER = ("test.sh", "run_script.sh", "parser.py", "config.json", "test_patch.patch")

APPLY_SH = """\
# Applies the work in /agent (the agent's copy of the repository) to the
# repository in this container as one patch: the changes from the files the
# copy started from to the files it holds now. `.swe` and `.gate_checks` are
# left out, and symlinks keep their targets (the copy holds files there).
set -eu
cd /app 2>/dev/null || cd /testbed
repo=$PWD
git config --global --add safe.directory '*'
base=$(mktemp)
work=$(mktemp)
cp .git/index "$base"
GIT_INDEX_FILE=$base git add -u
cp "$base" "$work"
(cd /agent && GIT_DIR=$repo/.git GIT_INDEX_FILE=$work git --work-tree=/agent add -A -- . ':(exclude).swe' ':(exclude).gate_checks')
GIT_INDEX_FILE=$base git ls-files -s | while read -r mode sha stage path; do
    if [ "$mode" = 120000 ]; then
        GIT_INDEX_FILE=$work git rm -rq --cached --ignore-unmatch -- "$path"
        GIT_INDEX_FILE=$work git update-index --add --cacheinfo "$mode,$sha,$path"
    fi
done
from=$(GIT_INDEX_FILE=$base git write-tree)
to=$(GIT_INDEX_FILE=$work git write-tree)
rm -f "$base" "$work"
git diff --binary "$from" "$to" > /tmp/agent.patch
if [ -s /tmp/agent.patch ]; then
    git apply --whitespace=nowarn /tmp/agent.patch
    git diff --stat "$from" "$to" | tail -1
else
    echo "No changes"
fi
"""

GRADE_SH = """\
# Grades the work as SWE-Bench Pro does: its changes as one patch, applied in
# a fresh container of the task's image, then the task's hidden tests.
set -u
cd "$(dirname "$0")/.."
exec docker run --rm -v "$PWD":/agent:ro -v "$PWD/.gate_checks":/grader:ro \\
    -v "$PWD/.gate_checks/tests":/tests:ro --entrypoint /bin/bash "$(cat .gate_checks/image)" \\
    -c 'bash /grader/apply.sh && bash /tests/test.sh'
"""

RUN = """\
#!/bin/sh
# Runs a shell command in a fresh container of this task's image, with your
# changes applied to its copy of the repository and no network access. The
# project's dependencies are installed there, not here. For example:
#   sh .swe/run 'go test ./lib/...'
here=$(cd "$(dirname "$0")/.." && pwd)
exec docker run --rm --network none -v "$here":/agent:ro -e SWE_COMMAND="$*" \\
    --entrypoint /bin/bash "$(cat "$here/.swe/image")" \\
    -c 'bash /agent/.swe/apply.sh >/dev/null || exit 1; cd /app 2>/dev/null || cd /testbed; eval "$SWE_COMMAND"'
"""

HELPER = (
    "The repository is your working folder. Its dependencies and test tools are installed only "
    "in this task's container image, not here: to run a command there (tests, builds, scripts) "
    "with your changes applied, use `sh .swe/run '<command>'` from the working folder, for "
    "example `sh .swe/run '{example}'`. Each call starts a fresh container without network "
    "access and prints the command's output. Hidden tests check your change after you finish; "
    "the repository's existing tests must keep passing."
)

EXAMPLE = {
    "NodeBB/NodeBB": "redis-server --daemonize yes && NODE_ENV=test TEST_ENV=development npx mocha test/<file>.js",
    "ansible/ansible": "python -m pytest -q test/units/<path>",
    "element-hq/element-web": "npx jest --maxWorkers=1 test/<path>",
    "flipt-io/flipt": "go test ./internal/<package>/...",
    "future-architect/vuls": "go test ./<package>/...",
    "gravitational/teleport": "go test ./lib/<package>/...",
    "internetarchive/openlibrary": "pytest -q openlibrary/<path>",
    "navidrome/navidrome": "go test -tags netgo ./<package>/...",
    "protonmail/webclients": "yarn workspace proton-mail test --testPathPattern=<path>",
    "qutebrowser/qutebrowser": "QT_QPA_PLATFORM=offscreen python -m pytest -q tests/unit/<path>",
    "tutao/tutanota": "cd test && node test",
}


def fetch() -> Path:
    """The SWE-Bench Pro repository's `v2` folder at the pinned commit."""
    if (CACHE / "v2").is_dir():
        return CACHE / "v2"
    url = f"https://codeload.github.com/{SOURCE}/tar.gz/{COMMIT}"
    print(f"Downloading {url}", file=sys.stderr)
    with urllib.request.urlopen(url) as response:
        data = response.read()
    partial = CACHE.with_name(CACHE.name + ".partial")
    shutil.rmtree(partial, ignore_errors=True)
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for member in archive.getmembers():
            parts = PurePosixPath(member.name).parts[1:]
            if not parts or parts[0] != "v2" or not (member.isfile() or member.isdir()):
                continue
            member.name = str(PurePosixPath(*parts))
            archive.extract(member, partial, filter="data")
    partial.rename(CACHE)
    return CACHE / "v2"


def toml_string(text: str) -> str:
    escaped = text.replace("\\", "\\\\").replace('"""', '\\"\\"\\"')
    return f'"""\n{escaped}\n"""'


def parse_id(instance: str) -> tuple[str, str]:
    """The repository (`owner/name`) and commit of an instance id."""
    match = re.match(r"instance_(.+?)__(.+)-([0-9a-f]{40})(?:-v.*)?$", instance)
    if not match:
        raise ValueError(f"unexpected instance id {instance}")
    owner, name, sha = match.groups()
    return f"{owner}/{name}", sha


def title_of(instruction: str) -> str:
    """The PR description's title: the text after `Title`, on its line or the next."""
    body = instruction.split("<pr_description>", 1)[-1]
    lines = [re.sub(r"[*#`]", "", line).strip() for line in body.splitlines()]
    lines = [line for line in lines if line]
    title = lines[0] if lines else ""
    for i, line in enumerate(lines):
        match = re.match(r"(?i)(?:issue\s+)?title\s*:?\s*(.*)$", line)
        if match:
            title = match.group(1).strip() or (lines[i + 1] if i + 1 < len(lines) else "")
            break
    title = title.strip().rstrip(".")
    return title[:117] + "..." if len(title) > 120 else title


def convert(v2: Path, instance: str, out: Path) -> str:
    task_dir = v2 / "tasks" / instance
    repo, sha = parse_id(instance)
    language = LANGUAGE[repo]
    slug = f"{repo.split('/')[1].lower()}-{sha[:8]}"
    image = re.search(r'docker_image = "([^"]+)"', (task_dir / "task.toml").read_text()).group(1)
    task = out / slug
    if task.exists():
        shutil.rmtree(task)
    swe = task / "workspace" / ".swe"
    swe.mkdir(parents=True)
    (swe / "run").write_text(RUN)
    (swe / "apply.sh").write_text(APPLY_SH)
    (swe / "image").write_text(image + "\n")
    hidden = task / "checks" / ".gate_checks"
    (hidden / "tests").mkdir(parents=True)
    for name in VERIFIER:
        shutil.copy2(task_dir / "tests" / name, hidden / "tests" / name)
    (hidden / "grade.sh").write_text(GRADE_SH)
    (hidden / "apply.sh").write_text(APPLY_SH)
    (hidden / "image").write_text(image + "\n")
    (task / "solution").mkdir()
    shutil.copy2(task_dir / "solution" / "gold_patch.diff", task / "solution" / "gold_patch.diff")
    instruction = (task_dir / "instruction.md").read_text()
    instruction = re.sub(
        r"^A code repository is available in the `/(app|testbed)` directory\. ",
        "",
        instruction,
    )
    prompt = f"{instruction.strip()}\n\n{HELPER.format(example=EXAMPLE[repo])}\n"
    (task / "task.toml").write_text(
        f"# SWE-Bench Pro V2: {instance}\n"
        f"# Upstream: https://github.com/{repo}/commit/{sha}\n"
        f"title = {json.dumps(f'{repo.split('/')[1]}: {title_of(instruction)}')}\n"
        'category = "swe-bench-pro"\n'
        f'tags = ["swe-bench-pro", "{language}", "{repo.split("/")[1].lower()}"]\n'
        f"prompt = {toml_string(prompt)}\n"
        'verify = "sh .gate_checks/grade.sh"\n'
        "max_iterations = 150\n"
        "\n[source]\n"
        f'image = "{image}"\n'
    )
    return slug


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", default=str(REPO / "evals" / "swe_bench_pro_hard"))
    parser.add_argument("--all", action="store_true", help="every V2 task, not only HARD-51")
    parser.add_argument("--instance", action="append", default=[], help="an instance id")
    args = parser.parse_args()
    v2 = fetch()
    if args.instance:
        instances = args.instance
    elif args.all:
        instances = sorted(p.name for p in (v2 / "tasks").iterdir() if p.is_dir())
    else:
        instances = (v2 / "hard51_ids.txt").read_text().split()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for instance in instances:
        print(convert(v2, instance, out))


if __name__ == "__main__":
    main()
