def quote(net):
    """Customer-facing price for a net amount."""
    gross = net + net * 0.0725
    return f"${gross:.2f} incl. tax"
