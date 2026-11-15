set -u
here=$(pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
src="$tmp/my src"
dest="$tmp/my dest"
mkdir -p "$src/notes.txt"
nl=$(printf 'x\ny.txt')
for name in "a b.txt" "-n.txt" "$nl" "plain.txt" "*.txt"; do
  printf 'content of %s\n' "$name" > "$src/$name"
done
printf 'skip\n' > "$src/skip.md"
sh "$here/backup.sh" "$src" "$dest" || { echo "backup.sh failed"; exit 1; }
for name in "a b.txt" "-n.txt" "$nl" "plain.txt" "*.txt"; do
  cmp -s "$src/$name" "$dest/$name" || { echo "missing or changed: $name"; exit 1; }
done
count=$(find "$dest" -mindepth 1 -exec printf x \; | wc -c)
[ "$count" -eq 5 ] || { echo "expected 5 entries in DEST, found $count"; exit 1; }
empty="$tmp/empty"
mkdir "$empty"
sh "$here/backup.sh" "$empty" "$tmp/out2" || { echo "failed on an empty SRC"; exit 1; }
count=$(find "$tmp/out2" -mindepth 1 -exec printf x \; | wc -c)
[ "$count" -eq 0 ] || { echo "empty SRC produced $count entries"; exit 1; }
echo ok
