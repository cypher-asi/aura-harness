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
go test -count=1 . ./patcher ./test/issues/823
status=$?
for f in $files; do
  if [ -e "$saved/$f" ]; then cp -p "$saved/$f" "$f"; else rm -f "$f"; fi
done
rm -rf "$saved"
exit $status
