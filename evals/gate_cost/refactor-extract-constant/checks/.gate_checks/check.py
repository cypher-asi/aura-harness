import pathlib

from shop import config
from shop.cart import cart_total
from shop.invoice import invoice_lines
from shop.quote import quote

assert config.TAX_RATE == 0.0725
hits = [p for p in pathlib.Path("shop").rglob("*.py") if "0.0725" in p.read_text()]
assert [p.name for p in hits] == ["config.py"], hits
for name in ["cart.py", "invoice.py", "quote.py"]:
    assert "TAX_RATE" in pathlib.Path("shop", name).read_text(), name
assert cart_total([10, 20]) == 32.17
assert invoice_lines([("a", 100)]) == [("a", 100, 7.25)]
assert quote(10) == "$10.72 incl. tax"
