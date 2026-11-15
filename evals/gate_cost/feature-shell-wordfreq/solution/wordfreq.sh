#!/bin/sh
# Usage: sh wordfreq.sh FILE
# Print the three most frequent words of FILE as `<count> <word>`, one
# per line, most frequent first; words with the same count in
# alphabetical order. Words are runs of letters, compared in lowercase.
tr 'A-Z' 'a-z' < "$1" | tr -cs 'a-z' '\n' | grep -v '^$' | sort | uniq -c \
  | sort -k1,1nr -k2,2 | head -n 3 | awk '{print $1 " " $2}'
