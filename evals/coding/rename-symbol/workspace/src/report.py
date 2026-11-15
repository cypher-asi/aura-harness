from billing import calc_total


def summary_line(customer, items):
    return f"{customer}: {calc_total(items, tax_rate=0.2):.2f}"
