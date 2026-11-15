def cart_total(prices):
    """Total of a cart including sales tax."""
    return round(sum(prices) * (1 + 0.0725), 2)
