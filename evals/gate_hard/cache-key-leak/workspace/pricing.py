from cache import memoize
import rates

BASE = {"widget": 10.0, "gadget": 25.0}


@memoize
def get_price(sku, currency="USD"):
    return round(BASE[sku] * rates.rate(currency), 2)


@memoize
def get_tax(region):
    return {"EU": 0.2, "US": 0.07}[region]
