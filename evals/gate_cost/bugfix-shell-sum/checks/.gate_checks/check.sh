set -e
for n in 0 1 4 10 100; do
  [ "$(sh sum.sh $n)" = "$((n * (n + 1) / 2))" ]
done
