set -e
[ "$(awk -f report.awk data.csv)" = "$(printf 'arch 300\ndev 250.5\nops 150')" ]
tmp=$(mktemp)
printf 'name,dept,salary\nx,zeta,1\ny,alpha,2\nz,mid,3\nw,alpha,4\n' > "$tmp"
[ "$(awk -f report.awk "$tmp")" = "$(printf 'alpha 6\nmid 3\nzeta 1')" ]
rm -f "$tmp"
