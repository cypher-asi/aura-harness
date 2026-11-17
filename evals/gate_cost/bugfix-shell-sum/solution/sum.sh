#!/bin/sh
# Print the sum of 1..N.
n=$1
total=0
i=1
while [ "$i" -le "$n" ]; do
  total=$((total + i))
  i=$((i + 1))
done
echo "$total"
