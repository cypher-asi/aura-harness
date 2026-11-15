from invoice import line_total, invoice_total

cases = [
    (("19.99", 3, "0.0825"), "64.92"),
    (("0.50", 1, "0.25"), "0.63"),
    (("2.14", 1, "0.25"), "2.68"),
    (("10.5", 1, "0"), "10.50"),
    (("-0.50", 1, "0.25"), "-0.63"),
    (("1.15", 3, "0"), "3.45"),
    (("0.01", 1, "0.5"), "0.02"),
]
for args, want in cases:
    got = line_total(*args)
    assert got == want, (args, got, want)
assert invoice_total([("0.50", 1, "0.25"), ("2.14", 1, "0.25")]) == "3.31"
assert invoice_total([("0.10", 1, "0")] * 10) == "1.00"
assert invoice_total([("0.01", 1, "0.5")] * 3) == "0.06"
assert invoice_total([]) == "0.00"
print("ok")
