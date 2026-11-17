import rates
from api import price_view
from pricing import get_price, get_tax

assert price_view("widget", "EUR") == "9.00 EUR"
assert price_view("widget", "USD") == "10.00 USD", price_view("widget", "USD")
assert get_price("widget", "GBP") == 8.0
assert get_price("widget", currency="EUR") == 9.0
assert get_price("widget") == 10.0
assert get_price("gadget", "EUR") == 22.5
assert get_tax("EU") == 0.2 and get_tax("US") == 0.07
rates.CALLS.clear()
for _ in range(5):
    get_price("widget", currency="EUR")
    get_price("gadget", "GBP")
assert len(rates.CALLS) <= 2, f"lookups are no longer cached: {rates.CALLS}"
print("ok")
