BEGIN { FS = " " }
{ sum[$2] += $3 }
END { for (d in sum) print d, sum[d] }
