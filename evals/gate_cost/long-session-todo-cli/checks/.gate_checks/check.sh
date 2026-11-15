set -e
rm -f todo.txt
[ "$(sh todo.sh list)" = "(empty)" ]
sh todo.sh add milk
sh todo.sh add "two eggs"
sh todo.sh add bread
[ "$(sh todo.sh list | sed -n 2p)" = "2. two eggs" ]
sh todo.sh done 1
[ "$(sh todo.sh list)" = "$(printf '1. two eggs\n2. bread')" ]
set +e
sh todo.sh frobnicate 2>/dev/null
[ $? -eq 2 ] || exit 1
[ -n "$(sh todo.sh frobnicate 2>&1 >/dev/null)" ] || exit 1
