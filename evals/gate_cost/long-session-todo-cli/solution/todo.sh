#!/bin/sh
file=todo.txt
case "$1" in
  add)
    shift
    echo "$*" >> "$file"
    ;;
  list)
    if [ ! -s "$file" ]; then
      echo "(empty)"
    else
      awk '{ print NR ". " $0 }' "$file"
    fi
    ;;
  done)
    [ -f "$file" ] || exit 0
    tmp=$(mktemp)
    awk -v n="$2" 'NR != n' "$file" > "$tmp" && mv "$tmp" "$file"
    ;;
  *)
    echo "usage: sh todo.sh add <text> | list | done <n>" >&2
    exit 2
    ;;
esac
