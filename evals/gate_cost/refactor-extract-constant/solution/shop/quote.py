from .config import TAX_RATE


def quote(net):
    """Customer-facing price for a net amount."""
    gross = net + net * TAX_RATE
    return f"${gross:.2f} incl. tax"
