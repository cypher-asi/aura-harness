"""Invoice totals."""
from decimal import ROUND_HALF_UP, Decimal

CENT = Decimal("0.01")


def line_total(price, qty, tax_rate):
    """Price times quantity plus tax, rounded to cents (halves away from zero)."""
    amount = Decimal(price) * qty * (1 + Decimal(tax_rate))
    return str(amount.quantize(CENT, rounding=ROUND_HALF_UP))


def invoice_total(lines):
    """Sum of the rounded line totals. lines: (price, qty, tax_rate) tuples."""
    total = sum((Decimal(line_total(p, q, t)) for p, q, t in lines), Decimal("0"))
    return str(total.quantize(CENT))
