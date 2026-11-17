def invoice_lines(items):
    """(name, net, tax) for each (name, net) item."""
    return [(name, net, round(net * 0.0725, 2)) for name, net in items]
