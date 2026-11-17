from billing import compute_total


def summary_line(customer, items):
    return f"{customer}: {compute_total(items, tax_rate=0.2):.2f}"
