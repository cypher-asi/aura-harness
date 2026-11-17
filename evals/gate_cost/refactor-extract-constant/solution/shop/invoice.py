from .config import TAX_RATE


def invoice_lines(items):
    """(name, net, tax) for each (name, net) item."""
    return [(name, net, round(net * TAX_RATE, 2)) for name, net in items]
