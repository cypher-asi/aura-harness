"""Invoice totals."""


def line_total(price, qty, tax_rate):
    """Price times quantity plus tax, rounded to cents."""
    subtotal = float(price) * qty
    return round(subtotal * (1 + float(tax_rate)), 2)


def invoice_total(lines):
    """Sum of line totals. lines: (price, qty, tax_rate) tuples."""
    return round(sum(line_total(p, q, t) for p, q, t in lines), 2)
