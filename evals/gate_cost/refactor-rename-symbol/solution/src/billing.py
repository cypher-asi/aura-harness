"""Billing helpers."""


def compute_total(items, tax_rate=0.0):
    """Sum item prices and apply tax."""
    subtotal = sum(price for _, price in items)
    return round(subtotal * (1 + tax_rate), 2)


def calc_discounted(items, discount):
    """Total after a flat discount, never below zero."""
    return max(compute_total(items) - discount, 0)
