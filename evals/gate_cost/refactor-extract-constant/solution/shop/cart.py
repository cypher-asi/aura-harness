from .config import TAX_RATE


def cart_total(prices):
    """Total of a cart including sales tax."""
    return round(sum(prices) * (1 + TAX_RATE), 2)
