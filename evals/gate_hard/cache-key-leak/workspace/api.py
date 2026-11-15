from pricing import get_price


def price_view(sku, currency):
    return f"{get_price(sku, currency=currency):.2f} {currency}"
