BEGIN { FS = "," }
NR == 1 { next }
{
    if (!($2 in sum)) {
        names[++n] = $2
    }
    sum[$2] += $3
}
END {
    for (i = 2; i <= n; i++) {
        key = names[i]
        for (j = i - 1; j >= 1 && names[j] > key; j--) {
            names[j + 1] = names[j]
        }
        names[j + 1] = key
    }
    for (i = 1; i <= n; i++) {
        print names[i], sum[names[i]]
    }
}
