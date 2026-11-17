set -e
[ -f test_inventory.py ]
python3 -m unittest -q test_inventory
for mutant in .gate_checks/mutants/*.py; do
  dir=$(mktemp -d)
  cp test_inventory.py "$dir/"
  cp "$mutant" "$dir/inventory.py"
  if (cd "$dir" && python3 -m unittest -q test_inventory >/dev/null 2>&1); then
    echo "tests miss the bug in $mutant" >&2
    exit 1
  fi
  rm -rf "$dir"
done
