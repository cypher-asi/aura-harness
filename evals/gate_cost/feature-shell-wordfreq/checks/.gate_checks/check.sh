set -e
tmp=$(mktemp)
printf 'the cat, The dog. the bird; cat\nzebra Zebra apple\n' > "$tmp"
out=$(sh wordfreq.sh "$tmp")
[ "$out" = "$(printf '3 the\n2 cat\n2 zebra')" ]
printf 'b a\n' > "$tmp"
[ "$(sh wordfreq.sh "$tmp")" = "$(printf '1 a\n1 b')" ]
rm -f "$tmp"
